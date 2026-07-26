/**
 * inspect_cad tool — query geometry properties for an open session
 * document. Sums volume, surface area, mass (when materials carry
 * density), and the overall bounding box across every part.
 *
 * Per-part inspection lives in `inspect_part` / `describe_scene` (dispatched
 * through the kernel registry surface) and part-pair clearance in `measure`;
 * all three reuse the tessellation-bound mesh math exported from here.
 */

import {
  getKernelWasmSync,
  transformMesh,
  type Engine,
  type TriangleMesh,
} from "@vcad/engine";
import type { Document } from "@vcad/ir";
import { isoperimetricViolation } from "./integrity.js";
import { applyJointState, jointStateSchemaProp, type PoseInfo } from "./pose.js";
import { resolveDocInput } from "./session-core.js";
import { behavior, type ToolDef } from "./tool-def.js";

export const inspectCadSchema = {
  type: "object" as const,
  properties: {
    document_id: {
      type: "string" as const,
      description: "Session id from open_document.",
    },
    document: {
      type: "object" as const,
      description:
        "Inline Document IR to inspect instead of a session. Use this stateless " +
        "path when no `document_id` is resident (e.g. a cold serverless instance).",
    },
    joint_state: jointStateSchemaProp,
  },
};

/** An axis-aligned bounding box in kernel (Z-up, mm) coordinates. */
export interface BoundingBox {
  min: { x: number; y: number; z: number };
  max: { x: number; y: number; z: number };
}

/** Per-part mass information. */
export interface PartMassInfo {
  name: string;
  volume_mm3: number;
  material: string;
  density_kg_m3?: number;
  mass_g?: number;
}

export interface InspectResult {
  volume_mm3: number;
  surface_area_mm2: number;
  bounding_box: BoundingBox;
  center_of_mass: { x: number; y: number; z: number };
  triangles: number;
  parts: number;
  mass_g?: number;
  part_masses?: PartMassInfo[];
  /**
   * Geometry-integrity violations (e.g. a part whose volume/area pair
   * breaks the isoperimetric bound A³ ≥ 36πV², which no real solid can).
   * Absent when the inspection is clean.
   */
  warnings?: string[];
  /** Applied `joint_state` pose and the FK transforms it resolved to.
   *  Absent when the document was measured in its stored pose. */
  pose?: PoseInfo;
}

/** Compute properties for a single mesh. Exported so `measure`, the
 *  per-part `inspect_part` / `describe_scene` tools, and the fabricate cost
 *  models share one implementation of the tessellation-bound
 *  volume/area/bbox/centroid math — the kernel's `compute_mesh_properties`
 *  (crates/vcad-kernel-tessellate/src/mesh_props.rs), reached through the
 *  WASM binding. There is deliberately no TS reimplementation: the kernel
 *  is the single source of truth, and the WASM module is always initialized
 *  before any tool can evaluate a document. */
export function computeMeshProperties(mesh: TriangleMesh): {
  volume: number;
  area: number;
  bbox: BoundingBox;
  centroid: { x: number; y: number; z: number };
  triangles: number;
} {
  const wasm = getKernelWasmSync() as {
    computeMeshProperties?: (
      positions: Float32Array,
      indices: Uint32Array,
    ) => {
      volume: number;
      area: number;
      bbox: BoundingBox;
      centerOfMass: { x: number; y: number; z: number };
      triangles: number;
    };
  } | null;
  if (!wasm) {
    throw new Error("kernel WASM is not initialized — call Engine.init first");
  }
  if (typeof wasm.computeMeshProperties !== "function") {
    throw new Error(
      "computeMeshProperties is not in this kernel build — rebuild vcad-kernel-wasm",
    );
  }
  const props = wasm.computeMeshProperties(
    mesh.positions instanceof Float32Array
      ? mesh.positions
      : new Float32Array(mesh.positions),
    mesh.indices instanceof Uint32Array
      ? mesh.indices
      : new Uint32Array(mesh.indices),
  );
  return {
    volume: props.volume,
    area: props.area,
    bbox: props.bbox,
    centroid: props.centerOfMass,
    triangles: props.triangles,
  };
}

/** Evaluate a document and aggregate its geometry properties. Shared by
 *  `inspect_cad` and `predict_print` (which snapshots the same numbers as
 *  pre-print measurables). */
export function computeInspection(ir: Document, engine: Engine): InspectResult {
  // Evaluate the document
  const scene = engine.evaluate(ir);

  // Assembly instances carry part-local meshes plus a world transform;
  // fold them in as world-placed units so assembly-only documents (no
  // scene roots) inspect instead of erroring.
  const instanceUnits = (scene.instances ?? []).map((inst) => ({
    mesh: inst.transform
      ? transformMesh(inst.mesh, {
          translate: inst.transform.translation,
          rotate: inst.transform.rotation,
          scale: inst.transform.scale,
        })
      : inst.mesh,
    material: inst.material ?? "default",
    name: inst.name ?? inst.instanceId,
  }));

  if (scene.parts.length === 0 && instanceUnits.length === 0) {
    throw new Error("Document has no parts to inspect");
  }

  // Aggregate properties across all parts
  let totalVolume = 0;
  let totalArea = 0;
  let totalTriangles = 0;
  let totalMass = 0;
  let hasMassData = false;
  let weightedCx = 0,
    weightedCy = 0,
    weightedCz = 0;

  const bbox: BoundingBox = {
    min: { x: Infinity, y: Infinity, z: Infinity },
    max: { x: -Infinity, y: -Infinity, z: -Infinity },
  };

  const partMasses: PartMassInfo[] = [];
  const warnings: string[] = [];

  // Find the root nodes to get part names
  const rootNameMap = new Map<number, string>();
  for (const root of ir.roots) {
    const node = ir.nodes[String(root.root)];
    if (node?.name) {
      rootNameMap.set(root.root, node.name);
    }
  }

  const units = [
    ...scene.parts.map((part, i) => {
      const rootEntry = ir.roots[i];
      return {
        mesh: part.mesh,
        material: part.material ?? "default",
        name: rootEntry
          ? rootNameMap.get(rootEntry.root) ?? `part_${i + 1}`
          : `part_${i + 1}`,
      };
    }),
    ...instanceUnits,
  ];

  for (let i = 0; i < units.length; i++) {
    const part = units[i];
    const props = computeMeshProperties(part.mesh);

    totalVolume += props.volume;
    totalArea += props.area;
    totalTriangles += props.triangles;

    // Weight centroid by volume
    weightedCx += props.centroid.x * props.volume;
    weightedCy += props.centroid.y * props.volume;
    weightedCz += props.centroid.z * props.volume;

    // Expand bounding box
    bbox.min.x = Math.min(bbox.min.x, props.bbox.min.x);
    bbox.min.y = Math.min(bbox.min.y, props.bbox.min.y);
    bbox.min.z = Math.min(bbox.min.z, props.bbox.min.z);
    bbox.max.x = Math.max(bbox.max.x, props.bbox.max.x);
    bbox.max.y = Math.max(bbox.max.y, props.bbox.max.y);
    bbox.max.z = Math.max(bbox.max.z, props.bbox.max.z);

    // Compute mass if material has density
    const materialKey = part.material;
    const material = ir.materials?.[materialKey];
    const density = material?.density;

    const partName = part.name;

    // Isoperimetric impossibility: A³ ≥ 36πV² for any real solid, so a
    // violating (volume, area) pair means the volume integral is corrupt
    // (wrong-but-watertight boolean result) — flag it instead of returning
    // the impossible numbers silently.
    const impossible = isoperimetricViolation(props.volume, props.area);
    if (impossible) {
      warnings.push(
        `part "${partName}" volume ${Math.round(props.volume * 1000) / 1000} mm³ is isoperimetrically impossible ` +
          `for its ${Math.round(props.area * 1000) / 1000} mm² of surface (A³ ≥ 36πV² for any real solid; this ` +
          `area can enclose at most ≈${Math.round(impossible.max_volume_mm3 * 1000) / 1000} mm³) — the volume ` +
          `integral is corrupt, do not trust this geometry`,
      );
    }

    const partMassInfo: PartMassInfo = {
      name: partName,
      volume_mm3: Math.round(props.volume * 1000) / 1000,
      material: materialKey,
    };

    if (density) {
      // mass (kg) = volume (mm³) / 1e9 * density (kg/m³)
      // mass (g) = mass (kg) * 1000
      const massKg = (props.volume / 1e9) * density;
      const massG = massKg * 1000;
      partMassInfo.density_kg_m3 = density;
      partMassInfo.mass_g = Math.round(massG * 1000) / 1000;
      totalMass += massG;
      hasMassData = true;
    }

    partMasses.push(partMassInfo);
  }

  // Compute overall center of mass
  const com =
    totalVolume > 1e-10
      ? {
          x: weightedCx / totalVolume,
          y: weightedCy / totalVolume,
          z: weightedCz / totalVolume,
        }
      : { x: 0, y: 0, z: 0 };

  const result: InspectResult = {
    volume_mm3: Math.round(totalVolume * 1000) / 1000,
    surface_area_mm2: Math.round(totalArea * 1000) / 1000,
    bounding_box: {
      min: {
        x: Math.round(bbox.min.x * 1000) / 1000,
        y: Math.round(bbox.min.y * 1000) / 1000,
        z: Math.round(bbox.min.z * 1000) / 1000,
      },
      max: {
        x: Math.round(bbox.max.x * 1000) / 1000,
        y: Math.round(bbox.max.y * 1000) / 1000,
        z: Math.round(bbox.max.z * 1000) / 1000,
      },
    },
    center_of_mass: {
      x: Math.round(com.x * 1000) / 1000,
      y: Math.round(com.y * 1000) / 1000,
      z: Math.round(com.z * 1000) / 1000,
    },
    triangles: totalTriangles,
    parts: units.length,
  };

  // Add mass data if any materials have density
  if (hasMassData) {
    result.mass_g = Math.round(totalMass * 1000) / 1000;
    result.part_masses = partMasses;
  }

  if (warnings.length > 0) {
    result.warnings = warnings;
  }

  return result;
}

export function inspectCad(
  input: unknown,
  engine: Engine,
): { content: Array<{ type: "text"; text: string }> } {
  const args = (input ?? {}) as Record<string, unknown>;
  const { doc: stored } = resolveDocInput(args);
  // Measure the pose the caller asked for, not just the zero pose.
  const { doc: ir, pose } = applyJointState(stored, args.joint_state);

  const result = computeInspection(ir, engine);
  if (pose) result.pose = pose;

  return {
    content: [
      {
        type: "text",
        text: JSON.stringify(result, null, 2),
      },
    ],
  };
}

export const toolDefs: ToolDef[] = [
  {
    name: "inspect_cad",
    pack: null,
    description:
      "Inspect an open session document to get aggregate geometry properties: volume, surface area, bounding box, center of mass, triangle count, and mass (if material density is known). For per-part detail use `inspect_part` (one part) or `describe_scene` (every part at once); for the gap or overlap between two parts use `measure`. Pass `joint_state` to measure a jointed assembly at a real pose (joint id or name → degrees, or mm for sliders) rather than its zero pose.",
    inputSchema: inspectCadSchema,
    handler: (a, c) => inspectCad(a, c.engine),
    behavior: behavior({}),
  },
];
