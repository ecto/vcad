/**
 * vcad Fabricate — lean geometry measurement for cost models.
 *
 * A trimmed-down cousin of the inspect_cad math: evaluates the document and
 * aggregates volume / surface area / bounding box across all parts. Cost
 * models only need volume, footprint, and the max bounding dimension, so this
 * intentionally skips per-part mass / center-of-mass.
 *
 * Tolerant by design: a PCB/ecad document (or any doc the kernel can't mesh
 * into solids) yields ok:false with zeroed metrics, and the caller falls back
 * to caller-supplied dimensions (e.g. boardAreaMm2 for PCBs).
 */

import type { Document } from "@vcad/ir";
import type { Engine, TriangleMesh } from "@vcad/engine";
import type { GeometryMetrics } from "./types.js";

function vertex(mesh: TriangleMesh, index: number): [number, number, number] {
  const i = index * 3;
  return [mesh.positions[i], mesh.positions[i + 1], mesh.positions[i + 2]];
}

/** Signed volume of the tetrahedron (origin, p1, p2, p3). */
function signedTetVolume(
  p1: [number, number, number],
  p2: [number, number, number],
  p3: [number, number, number],
): number {
  return (
    (p1[0] * (p2[1] * p3[2] - p3[1] * p2[2]) -
      p2[0] * (p1[1] * p3[2] - p3[1] * p1[2]) +
      p3[0] * (p1[1] * p2[2] - p2[1] * p1[2])) /
    6.0
  );
}

function triArea(
  p1: [number, number, number],
  p2: [number, number, number],
  p3: [number, number, number],
): number {
  const ax = p2[0] - p1[0],
    ay = p2[1] - p1[1],
    az = p2[2] - p1[2];
  const bx = p3[0] - p1[0],
    by = p3[1] - p1[1],
    bz = p3[2] - p1[2];
  const cx = ay * bz - az * by;
  const cy = az * bx - ax * bz;
  const cz = ax * by - ay * bx;
  return Math.sqrt(cx * cx + cy * cy + cz * cz) / 2.0;
}

const EMPTY: GeometryMetrics = {
  ok: false,
  parts: 0,
  volume_mm3: 0,
  surface_area_mm2: 0,
  footprint_mm2: 0,
  max_dim_mm: 0,
  bbox: null,
};

/** Evaluate `ir` and aggregate geometry metrics across all parts. */
export function measureDocument(ir: Document, engine: Engine): GeometryMetrics {
  let scene: { parts: Array<{ mesh: TriangleMesh }> };
  try {
    scene = engine.evaluate(ir) as { parts: Array<{ mesh: TriangleMesh }> };
  } catch {
    return { ...EMPTY };
  }
  if (!scene.parts || scene.parts.length === 0) return { ...EMPTY };

  let volume = 0;
  let area = 0;
  const min: [number, number, number] = [Infinity, Infinity, Infinity];
  const max: [number, number, number] = [-Infinity, -Infinity, -Infinity];

  for (const part of scene.parts) {
    const mesh = part.mesh;
    const numTris = mesh.indices.length / 3;
    for (let t = 0; t < numTris; t++) {
      const p1 = vertex(mesh, mesh.indices[t * 3]);
      const p2 = vertex(mesh, mesh.indices[t * 3 + 1]);
      const p3 = vertex(mesh, mesh.indices[t * 3 + 2]);
      volume += signedTetVolume(p1, p2, p3);
      area += triArea(p1, p2, p3);
      for (const p of [p1, p2, p3]) {
        for (let k = 0; k < 3; k++) {
          if (p[k] < min[k]) min[k] = p[k];
          if (p[k] > max[k]) max[k] = p[k];
        }
      }
    }
  }

  if (!Number.isFinite(min[0])) return { ...EMPTY };

  const dx = max[0] - min[0];
  const dy = max[1] - min[1];
  const dz = max[2] - min[2];

  return {
    ok: true,
    parts: scene.parts.length,
    volume_mm3: Math.abs(volume),
    surface_area_mm2: area,
    footprint_mm2: dx * dy,
    max_dim_mm: Math.max(dx, dy, dz),
    bbox: { min, max },
  };
}
