/**
 * topology_optimize tool — SIMP topology optimization in the kernel.
 *
 * Finds the stiffest material layout for a design domain under given loads
 * and supports, using only a target fraction of the domain's volume. The
 * domain is either an existing part's volume ("lightweight this bracket" —
 * material only appears where the part already is) or an axis-aligned box
 * ("grow me a bracket spanning this envelope").
 *
 * The optimization runs once and the result is frozen into the document as
 * an `ImportedMesh` part — like every commercial CAD topopt workflow, the
 * organic result is a generated body, not a parametric feature that
 * re-solves on every evaluation.
 */

import type { Document, ImportedMeshOp, Node } from "@vcad/ir";
import { createDocument } from "@vcad/ir";
import type {
  Engine,
  TopoOptResult,
  TopoOptSpec,
  TriangleMesh,
} from "@vcad/engine";
import { getSession, registerSession } from "./session-core.js";
import { behavior, type ToolDef } from "./tool-def.js";

const regionSchema = {
  type: "object" as const,
  required: ["min", "max"],
  properties: {
    min: {
      type: "array" as const,
      items: { type: "number" as const },
      minItems: 3,
      maxItems: 3,
      description: "Minimum corner [x, y, z] in mm.",
    },
    max: {
      type: "array" as const,
      items: { type: "number" as const },
      minItems: 3,
      maxItems: 3,
      description: "Maximum corner [x, y, z] in mm.",
    },
  },
};

export const topologyOptimizeSchema = {
  type: "object" as const,
  required: ["loads", "supports"],
  properties: {
    document_id: {
      type: "string" as const,
      description:
        "Session document. Required with `part`; optional with `domain_box` " +
        "(omitted = a new document is created for the result).",
    },
    part: {
      type: "string" as const,
      description:
        "Part id or name whose volume becomes the design domain — material " +
        "only appears inside this part. Mutually exclusive with `domain_box`.",
    },
    domain_box: {
      ...regionSchema,
      description:
        "Axis-aligned box design domain in mm (world frame, Z-up). " +
        "Mutually exclusive with `part`.",
    },
    loads: {
      type: "array" as const,
      minItems: 1,
      description:
        "Loads. Each is a total force vector (N) distributed over the grid " +
        "nodes inside its region. Regions are world-frame boxes; a " +
        "zero-thickness box selects the nearest plane of nodes.",
      items: {
        type: "object" as const,
        required: ["region", "force"],
        properties: {
          region: regionSchema,
          force: {
            type: "array" as const,
            items: { type: "number" as const },
            minItems: 3,
            maxItems: 3,
            description: "Total force [fx, fy, fz].",
          },
        },
      },
    },
    supports: {
      type: "array" as const,
      minItems: 1,
      description: "Fixed (anchored) regions.",
      items: {
        type: "object" as const,
        required: ["region"],
        properties: {
          region: regionSchema,
          fix: {
            type: "array" as const,
            items: { type: "boolean" as const },
            minItems: 3,
            maxItems: 3,
            description:
              "Which translations are fixed [x, y, z]; default all true.",
          },
        },
      },
    },
    volume_fraction: {
      type: "number" as const,
      description:
        "Fraction of the domain volume to keep as material, in (0, 1). " +
        "Default 0.3.",
    },
    resolution: {
      type: "number" as const,
      description:
        "Voxels along the longest domain axis (8–128). Higher = finer " +
        "detail but slower. Default 48.",
    },
    max_iterations: {
      type: "number" as const,
      description: "Max SIMP iterations. Default 40.",
    },
    filter_radius: {
      type: "number" as const,
      description:
        "Sensitivity filter radius in voxels — larger removes thin members. " +
        "Default 1.5.",
    },
    name: {
      type: "string" as const,
      description: "Name for the optimized part (default derives from the source).",
    },
    material: {
      type: "string" as const,
      description: "Material key for the optimized part.",
    },
    hide_source: {
      type: "boolean" as const,
      description:
        "With `part`: hide the source part after inserting the optimized " +
        "body (the source node is kept for undo/reference). Default true.",
    },
  },
};

interface TopoArgs {
  document_id?: string;
  part?: string;
  domain_box?: { min: [number, number, number]; max: [number, number, number] };
  loads?: TopoOptSpec["loads"];
  supports?: TopoOptSpec["supports"];
  volume_fraction?: number;
  resolution?: number;
  max_iterations?: number;
  filter_radius?: number;
  name?: string;
  material?: string;
  hide_source?: boolean;
}

/** Resolve a part (by root id or name) to its evaluated, placed mesh. */
export function resolvePartMesh(
  doc: Document,
  engine: Engine,
  wanted: string,
): { mesh: TriangleMesh; rootIndex: number; name?: string; material?: string } {
  const scene = engine.evaluate(doc);
  const visibleRoots = doc.roots.filter((e) => e.visible !== false);
  for (let i = 0; i < scene.parts.length && i < visibleRoots.length; i++) {
    const root = visibleRoots[i];
    const rootId = String(root.root);
    const node = doc.nodes[rootId];
    if (rootId !== wanted && node?.name !== wanted) continue;
    const mesh = scene.parts[i].mesh;
    if (!mesh || mesh.positions.length === 0) {
      throw new Error(`part "${wanted}" evaluated to an empty mesh`);
    }
    return {
      mesh,
      rootIndex: doc.roots.indexOf(root),
      name: node?.name ?? undefined,
      material: root.material ?? undefined,
    };
  }
  throw new Error(
    `part "${wanted}" not found — pass a root part id or exact name`,
  );
}

const round5 = (v: number) => Number(v.toPrecision(5));

export function topologyOptimizeTool(
  args: Record<string, unknown>,
  engine: Engine,
  progress?: (current: number, total?: number, message?: string) => void,
): { content: Array<{ type: "text"; text: string }> } {
  const a = args as TopoArgs;

  if (!a.loads?.length) throw new Error("topology_optimize: `loads` required");
  if (!a.supports?.length)
    throw new Error("topology_optimize: `supports` required");
  if (!!a.part === !!a.domain_box) {
    throw new Error(
      "topology_optimize: pass exactly one of `part` or `domain_box`",
    );
  }
  if (a.part && !a.document_id) {
    throw new Error("topology_optimize: `part` requires `document_id`");
  }

  const spec: TopoOptSpec = {
    loads: a.loads,
    supports: a.supports,
    volume_fraction: a.volume_fraction,
    resolution: a.resolution,
    max_iterations: a.max_iterations,
    filter_radius: a.filter_radius,
  };

  // Resolve the document (or mint one for box-domain runs).
  let doc: Document;
  let documentId: string;
  let minted = false;
  if (a.document_id) {
    documentId = String(a.document_id);
    doc = getSession(documentId);
  } else {
    doc = createDocument();
    documentId = registerSession(doc);
    minted = true;
  }

  // Run the optimization on the chosen domain.
  let result: TopoOptResult;
  let sourceName: string | undefined;
  let sourceMaterial: string | undefined;
  let sourceRootIndex = -1;
  // The kernel loop is stepwise (one SIMP iteration per call), so progress
  // notifications flush between iterations instead of bursting at the end.
  const totalIters = a.max_iterations ?? 40;
  const onStep = progress
    ? (s: { done: boolean; iteration: number; compliance: number; change: number }) => {
        if (s.iteration > 0) {
          progress(
            s.iteration,
            totalIters,
            `SIMP iteration ${s.iteration}/${totalIters}: compliance ${Number(
              s.compliance.toPrecision(5),
            )}, change ${Number(s.change.toPrecision(3))}`,
          );
        }
      }
    : undefined;
  if (a.part) {
    const resolved = resolvePartMesh(doc, engine, a.part);
    sourceName = resolved.name;
    sourceMaterial = resolved.material;
    sourceRootIndex = resolved.rootIndex;
    result = engine.topologyOptimizeMeshChunked(resolved.mesh, spec, onStep);
  } else {
    const box = a.domain_box!;
    result = engine.topologyOptimizeBoxChunked(box.min, box.max, spec, onStep);
  }

  const triangles = result.mesh.indices.length / 3;
  if (triangles === 0) {
    throw new Error(
      "topology optimization produced an empty structure — check that loads " +
        "and supports touch the design domain and volume_fraction isn't too low",
    );
  }

  // Freeze the result into the document as an ImportedMesh part.
  const partName =
    a.name ?? (sourceName ? `${sourceName} (optimized)` : "topopt");
  const op: ImportedMeshOp = {
    type: "ImportedMesh",
    positions: Array.from(result.mesh.positions),
    indices: Array.from(result.mesh.indices),
    normals: result.mesh.normals ? Array.from(result.mesh.normals) : undefined,
    source: "topology_optimize",
  };
  const existingIds = Object.keys(doc.nodes).map(Number);
  const nodeId = (existingIds.length ? Math.max(...existingIds) : 0) + 1;
  const node: Node = { id: nodeId, name: partName, op };
  doc.nodes[String(nodeId)] = node;
  doc.roots.push({
    root: nodeId,
    material: a.material ?? sourceMaterial ?? "default",
  });

  if (a.part && sourceRootIndex >= 0 && a.hide_source !== false) {
    doc.roots[sourceRootIndex] = {
      ...doc.roots[sourceRootIndex],
      visible: false,
    };
  }

  const history = result.complianceHistory.map(round5);
  return {
    content: [
      {
        type: "text",
        text: JSON.stringify(
          {
            document_id: documentId,
            part_id: String(nodeId),
            name: partName,
            iterations: result.iterations,
            converged: result.converged,
            compliance: {
              initial: history[0],
              final: history[history.length - 1],
              history,
            },
            volume_fraction_achieved: round5(result.volumeFraction),
            grid: result.grid,
            voxel_size_mm: round5(result.voxelSize),
            triangles,
            ...(minted
              ? { note: "New document created for the optimized part." }
              : {}),
            ...(a.part && a.hide_source !== false
              ? { source_part_hidden: a.part }
              : {}),
          },
          null,
          2,
        ),
      },
    ],
  };
}

export const toolDefs: ToolDef[] = [
  {
    name: "topology_optimize",
    pack: null,
    description:
      "SIMP topology optimization: find the stiffest material layout under given loads and " +
      "supports using only `volume_fraction` of the design domain — the organic, bone-like " +
      "structures topology optimization is known for. The domain is either an existing part's " +
      "volume (`part`: lightweight it in place) or an axis-aligned box (`domain_box`: grow a " +
      "bracket into an envelope). Loads and supports are world-frame box regions (mm, Z-up); " +
      "a zero-thickness box selects the nearest plane of structure nodes (e.g. a face of the " +
      "domain). The result is inserted into the document as a frozen mesh part; compliance " +
      "history and achieved volume fraction are reported. Deterministic for a given spec. " +
      "Cost scales with resolution³ — start at the default 48 and refine.",
    inputSchema: topologyOptimizeSchema,
    handler: (args, ctx) => topologyOptimizeTool(args, ctx.engine, ctx.progress),
    behavior: behavior({ writesDoc: true, geometry: true, mount: true }),
  },
];
