import type { Document } from "@vcad/ir";

// Types for mesh data
export interface TriangleMesh {
  positions: Float32Array;
  indices: Uint32Array;
}

export interface EvaluatedPart {
  mesh: TriangleMesh;
  material: string;
}

export interface EvaluatedScene {
  parts: EvaluatedPart[];
  instances: unknown[];
  clashes: unknown[];
}

// Engine wrapper for browser-only usage
// This uses a string-based dynamic import to avoid static analysis
let enginePromise: Promise<unknown> | null = null;

/**
 * Evaluate an IR document and return triangle meshes.
 * Must be called from browser context only.
 */
export async function evaluateDocument(doc: Document): Promise<EvaluatedScene> {
  if (typeof window === "undefined") {
    throw new Error("evaluateDocument can only be called in browser");
  }

  if (!enginePromise) {
    enginePromise = import("@vcad/engine").then(async (mod) => {
      return (mod as { Engine: { init: () => Promise<unknown> } }).Engine.init();
    });
  }

  const engine = await enginePromise as { evaluate: (doc: Document) => EvaluatedScene };
  return engine.evaluate(doc);
}

/**
 * Compute geometric properties from a mesh.
 */
export function computeMeshStats(mesh: TriangleMesh): {
  triangleCount: number;
  volume: number;
  surfaceArea: number;
  boundingBox: {
    min: { x: number; y: number; z: number };
    max: { x: number; y: number; z: number };
  };
} {
  const positions = mesh.positions;
  const indices = mesh.indices;
  const triangleCount = indices.length / 3;

  // Compute bounding box
  let minX = Infinity, minY = Infinity, minZ = Infinity;
  let maxX = -Infinity, maxY = -Infinity, maxZ = -Infinity;

  for (let i = 0; i < positions.length; i += 3) {
    const x = positions[i]!;
    const y = positions[i + 1]!;
    const z = positions[i + 2]!;
    minX = Math.min(minX, x);
    minY = Math.min(minY, y);
    minZ = Math.min(minZ, z);
    maxX = Math.max(maxX, x);
    maxY = Math.max(maxY, y);
    maxZ = Math.max(maxZ, z);
  }

  // Compute volume and surface area using signed tetrahedron method
  let volume = 0;
  let surfaceArea = 0;

  for (let i = 0; i < indices.length; i += 3) {
    const i0 = indices[i]! * 3;
    const i1 = indices[i + 1]! * 3;
    const i2 = indices[i + 2]! * 3;

    const ax = positions[i0]!, ay = positions[i0 + 1]!, az = positions[i0 + 2]!;
    const bx = positions[i1]!, by = positions[i1 + 1]!, bz = positions[i1 + 2]!;
    const cx = positions[i2]!, cy = positions[i2 + 1]!, cz = positions[i2 + 2]!;

    // Signed volume of tetrahedron with origin
    volume += (ax * (by * cz - bz * cy) + bx * (cy * az - cz * ay) + cx * (ay * bz - az * by)) / 6;

    // Triangle area via cross product
    const abx = bx - ax, aby = by - ay, abz = bz - az;
    const acx = cx - ax, acy = cy - ay, acz = cz - az;
    const crossX = aby * acz - abz * acy;
    const crossY = abz * acx - abx * acz;
    const crossZ = abx * acy - aby * acx;
    surfaceArea += Math.sqrt(crossX * crossX + crossY * crossY + crossZ * crossZ) / 2;
  }

  return {
    triangleCount,
    volume: Math.abs(volume),
    surfaceArea,
    boundingBox: {
      min: { x: minX, y: minY, z: minZ },
      max: { x: maxX, y: maxY, z: maxZ },
    },
  };
}
