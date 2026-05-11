import type {
  Document,
  Node,
  NodeId,
  CsgOp,
  Sketch2DOp,
  Text2DOp,
  SketchSegment2D,
  SweepOp,
  LoftOp,
  Transform3D,
  ImportedMeshOp,
  PcbBoardOp,
  EmbroideryPatternOp,
  PartInstanceOp,
} from "@vcad/ir";
import { resolveDocument } from "./expressions.js";
import type {
  EvaluatedScene,
  EvaluatedPartDef,
  EvaluatedInstance,
  TriangleMesh,
} from "./mesh.js";
import type { Solid } from "@vcad/kernel-wasm";
import { solveForwardKinematics } from "./kinematics.js";
import { buildSheetMetalChain, evaluateSheetMetalChain } from "./sheet-metal.js";

/** Debug flag - set to true to enable verbose console logging */
const DEBUG_EVAL = false;

/** Options for document evaluation */
export interface EvaluateOptions {
  /** Skip O(n²) clash detection for faster updates during parametric editing */
  skipClashDetection?: boolean;
}

/** Type for the kernel module */
interface KernelModule {
  Solid: typeof Solid;
  evaluateDocument?: (docJson: string, skipClashDetection: boolean) => unknown;
  /** Resolve a stdlib part → sub-document JSON. */
  buildPart?: (path: string, paramsJson: string) => string;
}

/** Shape of the WASM evaluator result */
interface WasmEvaluatedScene {
  parts: Array<{ mesh: WasmMesh; material: string }>;
  partDefs?: Array<{ id: string; mesh: WasmMesh }>;
  instances?: Array<{
    instance_id: string;
    part_def_id: string;
    name?: string;
    mesh: WasmMesh;
    material: string;
    transform?: Transform3D;
  }>;
  clashes: Array<WasmMesh>;
  failures?: Array<{ scope: string; node_id: number; error: string }>;
}

interface WasmMesh {
  positions: number[];
  indices: number[];
  normals?: number[];
  faceKinds?: number[] | Uint8Array;
}

/** Convert a WASM mesh to a TriangleMesh (typed arrays). */
function wasmMeshToTriangleMesh(m: WasmMesh): TriangleMesh {
  return {
    positions: new Float32Array(m.positions),
    indices: new Uint32Array(m.indices),
    normals: m.normals ? new Float32Array(m.normals) : undefined,
    faceKinds: m.faceKinds ? new Uint8Array(m.faceKinds) : undefined,
  };
}

/**
 * Evaluate a vcad IR Document into an EvaluatedScene.
 *
 * Prefers the Rust WASM evaluator (evaluateDocument) when available, which
 * handles ALL CsgOp variants including Sketch2D, Extrude, Revolve, Sweep,
 * Loft, Text2D, ImportedMesh, assembly with forward kinematics, and clash
 * detection.
 *
 * Falls back to the TypeScript evaluator when the WASM evaluator is not
 * available (e.g., older WASM builds).
 */
export function evaluateDocument(
  doc: Document,
  kernel: KernelModule,
  options: EvaluateOptions = {},
): EvaluatedScene {
  // Resolve parameters + bindings before either backend sees the doc.
  // The Rust WASM evaluator also performs this internally, but applying it
  // here means the TS fallback, clash meshes, and any downstream consumers
  // that inspect the raw CsgOp see concrete f64 values.
  const resolved = resolveDocument(doc);
  doc = resolved.doc;

  // Expand any PartInstance nodes next. Both the Rust WASM evaluator and
  // the TS fallback only understand regular CsgOps; parts are a reference
  // layer that lives above eval. Runs after parameter resolution so parts
  // see concrete f64s for any parameter-bound fields in their params map.
  doc = expandPartInstances(doc, kernel);

  // Try the Rust WASM evaluator first
  if (kernel.evaluateDocument) {
    try {
      const docJson = JSON.stringify(doc);
      const result = kernel.evaluateDocument(
        docJson,
        options.skipClashDetection ?? false,
      ) as WasmEvaluatedScene;

      // Post-process: generate TS-side meshes for types the Rust evaluator
      // doesn't tessellate (e.g. EmbroideryPattern), and route sheet-metal
      // roots through the dedicated kernel binding.
      const visibleRoots = doc.roots.filter((e) => e.visible !== false);
      const parts = result.parts.map((p, i) => {
        // If the WASM evaluator returned an empty mesh, check if it's an
        // embroidery pattern and generate the mesh in TypeScript.
        if (p.mesh.positions.length === 0 && i < visibleRoots.length) {
          const emb = findEmbroideryPattern(visibleRoots[i].root, doc.nodes);
          if (emb) {
            const baseMesh = embroideryPatternToMesh(emb.pattern);
            const mesh = transformMesh(baseMesh, emb.transform);
            return { mesh, material: p.material };
          }
          // Sheet-metal: the regular evaluator returns empty for these ops
          // (kernel.evaluateDocument knows nothing about them). The kernel's
          // `evaluateSheetMetalChain` does the actual work.
          const chain = buildSheetMetalChain(visibleRoots[i].root, doc.nodes);
          if (chain) {
            try {
              const { mesh, sheetMetal } = evaluateSheetMetalChain(
                chain,
                kernel as unknown as Parameters<typeof evaluateSheetMetalChain>[1],
              );
              return { mesh, material: p.material, sheetMetal };
            } catch (e) {
              const msg = e instanceof Error ? e.message : String(e);
              console.warn(
                `[ENGINE] sheet-metal eval failed at root[${i}] (node ${visibleRoots[i].root}): ${msg}`,
              );
              return { mesh: wasmMeshToTriangleMesh(p.mesh), material: p.material };
            }
          }
        }
        return { mesh: wasmMeshToTriangleMesh(p.mesh), material: p.material };
      });

      if (result.failures && result.failures.length > 0) {
        for (const f of result.failures) {
          console.warn(
            `[ENGINE] feature eval failed at ${f.scope} (node ${f.node_id}): ${f.error}`,
          );
        }
      }

      return {
        parts,
        partDefs: result.partDefs?.map((pd) => ({
          id: pd.id,
          mesh: wasmMeshToTriangleMesh(pd.mesh),
        })),
        instances: result.instances?.map((inst) => ({
          instanceId: inst.instance_id,
          partDefId: inst.part_def_id,
          name: inst.name,
          mesh: wasmMeshToTriangleMesh(inst.mesh),
          material: inst.material,
          transform: inst.transform,
        })),
        clashes: result.clashes.map(wasmMeshToTriangleMesh),
        failures: result.failures,
      };
    } catch (e) {
      console.warn("[ENGINE] WASM evaluateDocument failed, falling back to TS:", e);
      // Fall through to TS evaluator
    }
  }

  // Fallback: TypeScript evaluator
  return evaluateDocumentTS(doc, kernel, options);
}

// =========================================================================
// TypeScript fallback evaluator
// =========================================================================

/** Convert IR sketch segment to WASM format */
function convertSegment(seg: SketchSegment2D) {
  if (seg.type === "Line") {
    return {
      type: "Line" as const,
      start: [seg.start.x, seg.start.y],
      end: [seg.end.x, seg.end.y],
    };
  } else {
    return {
      type: "Arc" as const,
      start: [seg.start.x, seg.start.y],
      end: [seg.end.x, seg.end.y],
      center: [seg.center.x, seg.center.y],
      ccw: seg.ccw,
    };
  }
}

/** Convert IR Sketch2D op to WASM profile format */
function convertSketchToProfile(op: Sketch2DOp) {
  return {
    origin: [op.origin.x, op.origin.y, op.origin.z],
    x_dir: [op.x_dir.x, op.x_dir.y, op.x_dir.z],
    y_dir: [op.y_dir.x, op.y_dir.y, op.y_dir.z],
    segments: op.segments.map(convertSegment),
  };
}

/** Extract a TriangleMesh from a Solid. */
function solidToMesh(solid: Solid): TriangleMesh {
  const meshData = solid.getMesh();
  const positions = new Float32Array(meshData.positions);
  const indices = new Uint32Array(meshData.indices);

  // Validate indices - check for out-of-bounds references
  const numVertices = positions.length / 3;
  let hasInvalidIndices = false;
  for (let i = 0; i < indices.length; i++) {
    if (indices[i] >= numVertices) {
      hasInvalidIndices = true;
      break;
    }
  }

  if (hasInvalidIndices) {
    const validIndices: number[] = [];
    for (let i = 0; i < indices.length; i += 3) {
      const i0 = indices[i];
      const i1 = indices[i + 1];
      const i2 = indices[i + 2];
      if (i0 < numVertices && i1 < numVertices && i2 < numVertices) {
        validIndices.push(i0, i1, i2);
      }
    }
    return {
      positions,
      indices: new Uint32Array(validIndices),
      normals: meshData.normals ? new Float32Array(meshData.normals) : undefined,
    };
  }

  return {
    positions,
    indices,
    normals: meshData.normals ? new Float32Array(meshData.normals) : undefined,
  };
}

/** Transform info extracted from node chain */
interface TransformInfo {
  translate: { x: number; y: number; z: number };
  rotate: { x: number; y: number; z: number };
  scale: { x: number; y: number; z: number };
}

/**
 * Find an ImportedMesh in the node chain and extract transforms.
 */
function findImportedMesh(
  rootId: NodeId,
  nodes: Record<string, Node>,
): { mesh: ImportedMeshOp; transform: TransformInfo } | null {
  const transform: TransformInfo = {
    translate: { x: 0, y: 0, z: 0 },
    rotate: { x: 0, y: 0, z: 0 },
    scale: { x: 1, y: 1, z: 1 },
  };

  let current = rootId;
  while (true) {
    const node = nodes[String(current)];
    if (!node) return null;

    if (node.op.type === "ImportedMesh") {
      return { mesh: node.op, transform };
    }

    if (node.op.type === "Translate") {
      transform.translate = node.op.offset;
      current = node.op.child;
    } else if (node.op.type === "Rotate") {
      transform.rotate = node.op.angles;
      current = node.op.child;
    } else if (node.op.type === "Scale") {
      transform.scale = node.op.factor;
      current = node.op.child;
    } else {
      return null;
    }
  }
}

/**
 * Find an EmbroideryPattern in the node chain and extract transforms.
 */
export function findEmbroideryPattern(
  rootId: NodeId,
  nodes: Record<string, Node>,
): { pattern: EmbroideryPatternOp; transform: TransformInfo } | null {
  const transform: TransformInfo = {
    translate: { x: 0, y: 0, z: 0 },
    rotate: { x: 0, y: 0, z: 0 },
    scale: { x: 1, y: 1, z: 1 },
  };

  let current = rootId;
  while (true) {
    const node = nodes[String(current)];
    if (!node) return null;

    if (node.op.type === "EmbroideryPattern") {
      return { pattern: node.op, transform };
    }

    if (node.op.type === "Translate") {
      transform.translate = node.op.offset;
      current = node.op.child;
    } else if (node.op.type === "Rotate") {
      transform.rotate = node.op.angles;
      current = node.op.child;
    } else if (node.op.type === "Scale") {
      transform.scale = node.op.factor;
      current = node.op.child;
    } else {
      return null;
    }
  }
}

/**
 * Generate ribbon-quad mesh from embroidery stitch data.
 * Each consecutive stitch pair becomes a thin quad (2 triangles) at Z=0.
 */
export function embroideryPatternToMesh(op: EmbroideryPatternOp): TriangleMesh {
  const RIBBON_HALF_WIDTH = 0.15; // 0.3mm total width
  const allPositions: number[] = [];
  const allIndices: number[] = [];
  const allColors: number[] = [];

  for (const group of op.design.stitch_groups) {
    // Resolve thread color (default to mid-gray)
    const thread = op.design.threads[group.thread_index];
    const r = thread ? thread.color[0] / 255 : 0.5;
    const g = thread ? thread.color[1] / 255 : 0.5;
    const b = thread ? thread.color[2] / 255 : 0.5;

    const stitches = group.stitches;
    for (let i = 0; i < stitches.length - 1; i++) {
      const [x0, rawY0] = stitches[i];
      const [x1, rawY1] = stitches[i + 1];
      // Flip Y: embroidery uses Y-down, CAD uses Y-up
      const y0 = -rawY0;
      const y1 = -rawY1;

      const dx = x1 - x0;
      const dy = y1 - y0;
      const len = Math.sqrt(dx * dx + dy * dy);
      if (len < 1e-6) continue;

      // Perpendicular direction
      const px = (-dy / len) * RIBBON_HALF_WIDTH;
      const py = (dx / len) * RIBBON_HALF_WIDTH;

      const base = allPositions.length / 3;
      // 4 vertices: left-start, right-start, right-end, left-end
      allPositions.push(x0 + px, y0 + py, 0);
      allPositions.push(x0 - px, y0 - py, 0);
      allPositions.push(x1 - px, y1 - py, 0);
      allPositions.push(x1 + px, y1 + py, 0);

      // 4 vertex colors (same thread color per quad)
      allColors.push(r, g, b, r, g, b, r, g, b, r, g, b);

      // 2 triangles
      allIndices.push(base, base + 1, base + 2);
      allIndices.push(base, base + 2, base + 3);
    }
  }

  return {
    positions: new Float32Array(allPositions),
    indices: new Uint32Array(allIndices),
    colors: new Float32Array(allColors),
  };
}

/**
 * Apply a transform to mesh positions.
 */
export function transformMesh(
  mesh: TriangleMesh,
  transform: TransformInfo,
): TriangleMesh {
  const { translate, rotate, scale } = transform;
  const positions = new Float32Array(mesh.positions.length);

  const rx = (rotate.x * Math.PI) / 180;
  const ry = (rotate.y * Math.PI) / 180;
  const rz = (rotate.z * Math.PI) / 180;

  const cx = Math.cos(rx), sx = Math.sin(rx);
  const cy = Math.cos(ry), sy = Math.sin(ry);
  const cz = Math.cos(rz), sz = Math.sin(rz);

  const m00 = cy * cz;
  const m01 = sx * sy * cz - cx * sz;
  const m02 = cx * sy * cz + sx * sz;
  const m10 = cy * sz;
  const m11 = sx * sy * sz + cx * cz;
  const m12 = cx * sy * sz - sx * cz;
  const m20 = -sy;
  const m21 = sx * cy;
  const m22 = cx * cy;

  for (let i = 0; i < mesh.positions.length; i += 3) {
    const x = mesh.positions[i] * scale.x;
    const y = mesh.positions[i + 1] * scale.y;
    const z = mesh.positions[i + 2] * scale.z;

    const rx2 = m00 * x + m01 * y + m02 * z;
    const ry2 = m10 * x + m11 * y + m12 * z;
    const rz2 = m20 * x + m21 * y + m22 * z;

    positions[i] = rx2 + translate.x;
    positions[i + 1] = ry2 + translate.y;
    positions[i + 2] = rz2 + translate.z;
  }

  let normals = mesh.normals;
  if (mesh.normals) {
    normals = new Float32Array(mesh.normals.length);
    for (let i = 0; i < mesh.normals.length; i += 3) {
      const nx = mesh.normals[i];
      const ny = mesh.normals[i + 1];
      const nz = mesh.normals[i + 2];

      normals[i] = m00 * nx + m01 * ny + m02 * nz;
      normals[i + 1] = m10 * nx + m11 * ny + m12 * nz;
      normals[i + 2] = m20 * nx + m21 * ny + m22 * nz;
    }
  }

  return { positions, indices: mesh.indices, normals, colors: mesh.colors };
}

/**
 * TypeScript fallback evaluator.
 *
 * Used when the WASM `evaluateDocument` is not available. Produces `Solid`
 * objects alongside meshes (needed for STEP export and BRep ray tracing).
 */
export function evaluateDocumentTS(
  doc: Document,
  kernel: KernelModule,
  options: EvaluateOptions = {},
): EvaluatedScene {
  const { Solid } = kernel;
  const cache = new Map<NodeId, Solid>();

  // Traditional mode: evaluate roots (filter out hidden parts)
  const visibleRoots = doc.roots.filter((entry) => entry.visible !== false);
  const solids: Solid[] = [];
  const failures: { scope: string; node_id: number; error: string }[] = [];
  const emptyMesh = (): TriangleMesh => ({
    positions: new Float32Array(0),
    indices: new Uint32Array(0),
  });
  const parts = visibleRoots.map((entry, idx) => {
    // Check if this is an EmbroideryPattern
    const embPattern = findEmbroideryPattern(entry.root, doc.nodes);
    if (embPattern) {
      const baseMesh = embroideryPatternToMesh(embPattern.pattern);
      const mesh = transformMesh(baseMesh, embPattern.transform);
      solids.push(Solid.empty());
      return { mesh, material: entry.material };
    }

    // Check if this is an imported mesh (doesn't go through Solid pipeline)
    const imported = findImportedMesh(entry.root, doc.nodes);
    if (imported) {
      const baseMesh: TriangleMesh = {
        positions: new Float32Array(imported.mesh.positions),
        indices: new Uint32Array(imported.mesh.indices),
        normals: imported.mesh.normals ? new Float32Array(imported.mesh.normals) : undefined,
      };
      const mesh = transformMesh(baseMesh, imported.transform);
      solids.push(Solid.empty());
      return { mesh, material: entry.material };
    }

    // Sheet-metal — route the chain through the kernel binding.
    const smChain = buildSheetMetalChain(entry.root, doc.nodes);
    if (smChain) {
      try {
        const { mesh, sheetMetal } = evaluateSheetMetalChain(
          smChain,
          kernel as unknown as Parameters<typeof evaluateSheetMetalChain>[1],
        );
        solids.push(Solid.empty());
        return { mesh, material: entry.material, sheetMetal };
      } catch (e) {
        const msg = e instanceof Error ? e.message : String(e);
        failures.push({ scope: `root[${idx}]`, node_id: entry.root, error: msg });
        console.warn(
          `[ENGINE] sheet-metal eval failed at root[${idx}] (node ${entry.root}): ${msg}`,
        );
        solids.push(Solid.empty());
        return { mesh: emptyMesh(), material: entry.material };
      }
    }

    try {
      const solid = evaluateNode(entry.root, doc.nodes, Solid, cache, 0);
      const mesh = solidToMesh(solid);
      solids.push(solid);
      return {
        mesh,
        material: entry.material,
        solid: solid,
      };
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      failures.push({ scope: `root[${idx}]`, node_id: entry.root, error: msg });
      console.warn(
        `[ENGINE] feature eval failed at root[${idx}] (node ${entry.root}): ${msg}`,
      );
      solids.push(Solid.empty());
      return { mesh: emptyMesh(), material: entry.material };
    }
  });

  // Assembly mode
  let evaluatedPartDefs: EvaluatedPartDef[] | undefined;
  let evaluatedInstances: EvaluatedInstance[] | undefined;

  if (doc.partDefs && Object.keys(doc.partDefs).length > 0 && doc.instances && doc.instances.length > 0) {
    const worldTransforms = solveForwardKinematics(doc);

    const partDefMeshes = new Map<string, TriangleMesh>();
    evaluatedPartDefs = [];
    for (const [id, partDef] of Object.entries(doc.partDefs)) {
      try {
        const solid = evaluateNode(partDef.root, doc.nodes, Solid, cache, 0);
        const mesh = solidToMesh(solid);
        partDefMeshes.set(id, mesh);
        evaluatedPartDefs.push({ id, mesh, solid });
      } catch (e) {
        const msg = e instanceof Error ? e.message : String(e);
        failures.push({ scope: `partDef[${JSON.stringify(id)}]`, node_id: partDef.root, error: msg });
        console.warn(
          `[ENGINE] partDef eval failed at ${id} (node ${partDef.root}): ${msg}`,
        );
        const empty = emptyMesh();
        partDefMeshes.set(id, empty);
        evaluatedPartDefs.push({ id, mesh: empty });
      }
    }

    evaluatedInstances = [];
    for (const instance of doc.instances) {
      const mesh = partDefMeshes.get(instance.partDefId);
      if (!mesh) continue;

      const worldTransform = worldTransforms.get(instance.id) ?? instance.transform;
      const partDef = doc.partDefs[instance.partDefId];
      const material = instance.material ?? partDef?.defaultMaterial ?? "default";

      evaluatedInstances.push({
        instanceId: instance.id,
        partDefId: instance.partDefId,
        name: instance.name,
        mesh,
        material,
        transform: worldTransform,
      });
    }
  }

  // Clash detection
  const clashes: TriangleMesh[] = [];
  if (!options.skipClashDetection) {
    for (let i = 0; i < solids.length; i++) {
      for (let j = i + 1; j < solids.length; j++) {
        const intersection = solids[i].intersection(solids[j]);
        if (!intersection.isEmpty()) {
          const meshData = intersection.getMesh();
          if (meshData.positions.length > 0) {
            clashes.push({
              positions: new Float32Array(meshData.positions),
              indices: new Uint32Array(meshData.indices),
              normals: meshData.normals
                ? new Float32Array(meshData.normals)
                : undefined,
            });
          }
        }
      }
    }
  }

  return {
    parts,
    partDefs: evaluatedPartDefs,
    instances: evaluatedInstances,
    clashes,
    failures: failures.length > 0 ? failures : undefined,
  };
}

function evaluateNode(
  nodeId: NodeId,
  nodes: Record<string, Node>,
  Solid: typeof import("@vcad/kernel-wasm").Solid,
  cache: Map<NodeId, import("@vcad/kernel-wasm").Solid>,
  depth = 0,
): import("@vcad/kernel-wasm").Solid {
  const cached = cache.get(nodeId);
  if (cached) return cached;

  const node = nodes[String(nodeId)];
  if (!node) throw new Error(`Missing node: ${nodeId}`);

  const result = evaluateOp(node.op, nodes, Solid, cache, depth);
  cache.set(nodeId, result);
  return result;
}

function evaluateOp(
  op: CsgOp,
  nodes: Record<string, Node>,
  Solid: typeof import("@vcad/kernel-wasm").Solid,
  cache: Map<NodeId, import("@vcad/kernel-wasm").Solid>,
  depth = 0,
): import("@vcad/kernel-wasm").Solid {
  switch (op.type) {
    case "Cube":
      return Solid.cube(op.size.x, op.size.y, op.size.z);

    case "Cylinder":
      return Solid.cylinder(op.radius, op.height, op.segments || undefined);

    case "Sphere":
      return Solid.sphere(op.radius, op.segments || undefined);

    case "Cone":
      return Solid.cone(
        op.radius_bottom,
        op.radius_top,
        op.height,
        op.segments || undefined,
      );

    case "Empty":
      return Solid.empty();

    case "Union": {
      const left = evaluateNode(op.left, nodes, Solid, cache, depth + 1);
      const right = evaluateNode(op.right, nodes, Solid, cache, depth + 1);
      return left.union(right);
    }

    case "Difference": {
      const left = evaluateNode(op.left, nodes, Solid, cache, depth + 1);
      const right = evaluateNode(op.right, nodes, Solid, cache, depth + 1);
      return left.difference(right);
    }

    case "Intersection": {
      const left = evaluateNode(op.left, nodes, Solid, cache, depth + 1);
      const right = evaluateNode(op.right, nodes, Solid, cache, depth + 1);
      return left.intersection(right);
    }

    case "Translate": {
      const child = evaluateNode(op.child, nodes, Solid, cache, depth + 1);
      return child.translate(op.offset.x, op.offset.y, op.offset.z);
    }

    case "Rotate": {
      const child = evaluateNode(op.child, nodes, Solid, cache, depth + 1);
      return child.rotate(op.angles.x, op.angles.y, op.angles.z);
    }

    case "Scale": {
      const child = evaluateNode(op.child, nodes, Solid, cache, depth + 1);
      return child.scale(op.factor.x, op.factor.y, op.factor.z);
    }

    case "Sketch2D":
      return Solid.empty();

    case "Extrude": {
      const sketchNode = nodes[String(op.sketch)];
      if (!sketchNode) {
        throw new Error(`Extrude references missing node: ${op.sketch}`);
      }

      const direction = new Float64Array([
        op.direction.x,
        op.direction.y,
        op.direction.z,
      ]);

      // Handle Text2D nodes (text extrusion)
      if (sketchNode.op.type === "Text2D") {
        const textOp = sketchNode.op as Text2DOp;
        const origin = new Float64Array([textOp.origin.x, textOp.origin.y, textOp.origin.z]);
        const xDir = new Float64Array([textOp.x_dir.x, textOp.x_dir.y, textOp.x_dir.z]);
        const yDir = new Float64Array([textOp.y_dir.x, textOp.y_dir.y, textOp.y_dir.z]);

        return Solid.textExtrude(
          textOp.text,
          origin,
          xDir,
          yDir,
          direction,
          textOp.height,
          textOp.font || undefined,
          textOp.alignment || undefined,
          textOp.letter_spacing ?? undefined,
          textOp.line_spacing ?? undefined,
        );
      }

      if (sketchNode.op.type !== "Sketch2D") {
        throw new Error(`Extrude references invalid sketch node: ${op.sketch} (type=${sketchNode.op.type})`);
      }
      const profile = convertSketchToProfile(sketchNode.op);
      const profileJson = JSON.stringify(profile);
      const hasTwist = op.twist_angle !== undefined && Math.abs(op.twist_angle) > 1e-12;
      const hasScale = op.scale_end !== undefined && Math.abs(op.scale_end - 1.0) > 1e-12;
      return (hasTwist || hasScale)
        ? Solid.extrudeWithOptions(
            profileJson,
            direction,
            op.twist_angle ?? 0,
            op.scale_end ?? 1.0
          )
        : Solid.extrude(profileJson, direction);
    }

    case "Revolve": {
      const sketchNode = nodes[String(op.sketch)];
      if (!sketchNode || sketchNode.op.type !== "Sketch2D") {
        throw new Error(`Revolve references invalid sketch node: ${op.sketch}`);
      }
      const profile = convertSketchToProfile(sketchNode.op);
      const profileJson = JSON.stringify(profile);
      const axisOrigin = new Float64Array([
        op.axis_origin.x,
        op.axis_origin.y,
        op.axis_origin.z,
      ]);
      const axisDir = new Float64Array([
        op.axis_dir.x,
        op.axis_dir.y,
        op.axis_dir.z,
      ]);
      return Solid.revolve(profileJson, axisOrigin, axisDir, op.angle_deg);
    }

    case "LinearPattern": {
      const child = evaluateNode(op.child, nodes, Solid, cache, depth + 1);
      return child.linearPattern(
        op.direction.x,
        op.direction.y,
        op.direction.z,
        op.count,
        op.spacing,
      );
    }

    case "CircularPattern": {
      const child = evaluateNode(op.child, nodes, Solid, cache, depth + 1);
      return child.circularPattern(
        op.axis_origin.x,
        op.axis_origin.y,
        op.axis_origin.z,
        op.axis_dir.x,
        op.axis_dir.y,
        op.axis_dir.z,
        op.count,
        op.angle_deg,
      );
    }

    case "Shell": {
      const child = evaluateNode(op.child, nodes, Solid, cache, depth + 1);
      return child.shell(op.thickness);
    }

    case "Fillet": {
      const child = evaluateNode(op.child, nodes, Solid, cache, depth + 1);
      return child.fillet(op.radius);
    }

    case "Chamfer": {
      const child = evaluateNode(op.child, nodes, Solid, cache, depth + 1);
      return child.chamfer(op.distance);
    }

    case "Sweep": {
      const sketchNode = nodes[String(op.sketch)];
      if (!sketchNode || sketchNode.op.type !== "Sketch2D") {
        throw new Error(`Sweep references invalid sketch node: ${op.sketch}`);
      }
      const profile = convertSketchToProfile(sketchNode.op);
      const profileJson = JSON.stringify(profile);

      if (op.path.type === "Line") {
        const start = new Float64Array([
          op.path.start.x,
          op.path.start.y,
          op.path.start.z,
        ]);
        const end = new Float64Array([
          op.path.end.x,
          op.path.end.y,
          op.path.end.z,
        ]);
        return Solid.sweepLine(
          profileJson,
          start,
          end,
          op.twist_angle,
          op.scale_start,
          op.scale_end,
          op.orientation,
        );
      } else {
        return Solid.sweepHelix(
          profileJson,
          op.path.radius,
          op.path.pitch,
          op.path.height,
          op.path.turns,
          op.twist_angle,
          op.scale_start,
          op.scale_end,
          op.path_segments,
          op.arc_segments,
          op.orientation,
        );
      }
    }

    case "Loft": {
      const profiles = op.sketches.map((sketchId) => {
        const sketchNode = nodes[String(sketchId)];
        if (!sketchNode || sketchNode.op.type !== "Sketch2D") {
          throw new Error(`Loft references invalid sketch node: ${sketchId}`);
        }
        return convertSketchToProfile(sketchNode.op);
      });
      return Solid.loft(JSON.stringify(profiles), op.closed);
    }

    case "ImportedMesh":
      return Solid.empty();

    case "Text2D":
      return Solid.empty();

    case "PcbBoard":
      return Solid.empty();

    case "EmbroideryPattern":
      return Solid.empty();

    case "PartInstance":
      // PartInstance nodes are expanded before evaluation via
      // `expandPartInstances`. If one slips through, treat as empty to
      // avoid crashing the evaluator — the warning has already been
      // logged during expansion.
      return Solid.empty();

    case "SheetMetalBaseFlangeRect":
    case "SheetMetalEdgeFlange":
      // Sheet-metal ops bypass the Solid pipeline — root-level detection
      // in `evaluateDocument` routes the chain to the kernel's
      // `evaluateSheetMetalChain` and attaches the result. Encountering
      // one here means the op got composed with another body, which the
      // foundation tier doesn't support yet.
      return Solid.empty();
  }
}


/**
 * Expand every `PartInstance` node in the document by resolving it through
 * the kernel's `buildPart` export and splicing the resulting sub-document
 * into the parent. The `PartInstance` node itself is replaced with a
 * `Translate` wrapper pointing at the sub-doc's root, so parent references
 * by NodeId continue to resolve.
 *
 * Sub-document NodeIds are remapped to a fresh disjoint range so they
 * don't collide with the parent document's ids.
 */
function expandPartInstances(doc: Document, kernel: KernelModule): Document {
  const build = kernel.buildPart;
  if (typeof build !== "function") return doc;

  const hasAny = Object.values(doc.nodes).some(
    (n) => n.op.type === "PartInstance",
  );
  if (!hasAny) return doc;

  const out: Document = JSON.parse(JSON.stringify(doc));
  const existingIds = Object.keys(out.nodes).map(Number);
  let nextId = (existingIds.length > 0 ? Math.max(...existingIds) : 0) + 1;

  for (const [idStr, node] of Object.entries(out.nodes)) {
    if (node.op.type !== "PartInstance") continue;
    const op = node.op as PartInstanceOp;

    let subJson: string;
    try {
      subJson = build(op.path, JSON.stringify(op.params ?? {}));
    } catch (err) {
      console.warn(`[parts] failed to build ${op.path}:`, err);
      node.op = { type: "Empty" };
      continue;
    }

    let subDoc: Document;
    try {
      subDoc = JSON.parse(subJson) as Document;
    } catch (err) {
      console.warn(`[parts] invalid sub-doc from ${op.path}:`, err);
      node.op = { type: "Empty" };
      continue;
    }

    const rootEntry = subDoc.roots[0];
    if (!rootEntry) {
      console.warn(`[parts] ${op.path} produced no scene root`);
      node.op = { type: "Empty" };
      continue;
    }

    // Remap sub-doc NodeIds into a fresh range.
    const idMap = new Map<number, number>();
    for (const subId of Object.keys(subDoc.nodes).map(Number)) {
      idMap.set(subId, nextId++);
    }

    for (const [subIdStr, subNode] of Object.entries(subDoc.nodes)) {
      const subId = Number(subIdStr);
      const newId = idMap.get(subId)!;
      const remapped = remapNodeIds(subNode as Node, idMap);
      remapped.id = newId;
      out.nodes[String(newId)] = remapped;
    }

    const newRootId = idMap.get(rootEntry.root);
    if (newRootId === undefined) {
      console.warn(`[parts] ${op.path} root id not in remap table`);
      node.op = { type: "Empty" };
      continue;
    }

    // Replace the PartInstance with a transparent Translate wrapper so the
    // parent's NodeId keeps pointing at valid geometry without changing its
    // own id. `idStr` stays as the node key in the document.
    void idStr;
    node.op = {
      type: "Translate",
      child: newRootId,
      offset: { x: 0, y: 0, z: 0 },
    };
  }

  return out;
}

function remapNodeIds(node: Node, idMap: Map<number, number>): Node {
  const m = (id: number) => idMap.get(id) ?? id;
  const op = node.op;
  let newOp: CsgOp = op;
  switch (op.type) {
    case "Union":
    case "Difference":
    case "Intersection":
      newOp = { ...op, left: m(op.left), right: m(op.right) };
      break;
    case "Translate":
    case "Rotate":
    case "Scale":
    case "LinearPattern":
    case "CircularPattern":
    case "Shell":
    case "Fillet":
    case "Chamfer":
      newOp = { ...op, child: m(op.child) };
      break;
    case "Extrude":
    case "Revolve":
    case "Sweep":
      newOp = { ...op, sketch: m(op.sketch) };
      break;
    case "Loft":
      newOp = { ...op, sketches: op.sketches.map(m) };
      break;
    default:
      newOp = op;
  }
  return { ...node, op: newOp };
}
