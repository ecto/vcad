/**
 * vcad Fabricate — lean geometry measurement for cost models.
 *
 * A trimmed-down cousin of the inspect_cad math: evaluates the document and
 * aggregates volume / surface area / bounding box across all parts via the
 * kernel's `computeMeshProperties` (shared through `tools/inspect.js` — no
 * local mesh math). Cost models only need volume, footprint, and the max
 * bounding dimension, so this intentionally skips per-part mass /
 * center-of-mass.
 *
 * Tolerant by design: a PCB/ecad document (or any doc the kernel can't mesh
 * into solids) yields ok:false with zeroed metrics, and the caller falls back
 * to caller-supplied dimensions (e.g. boardAreaMm2 for PCBs).
 */

import type { Document } from "@vcad/ir";
import type { Engine, EvaluatedScene, TriangleMesh } from "@vcad/engine";
import { computeMeshProperties } from "../tools/inspect.js";
import { sceneExportUnits, worldMesh } from "../export/scene-units.js";
import type { GeometryMetrics } from "./types.js";

const EMPTY: GeometryMetrics = {
  ok: false,
  parts: 0,
  volume_mm3: 0,
  surface_area_mm2: 0,
  footprint_mm2: 0,
  max_dim_mm: 0,
  bbox: null,
};

/** Evaluate `ir` and aggregate geometry metrics across all parts.
 *
 * Assembly-authored documents have no scene roots — their geometry is
 * instances placed by the joint tree — so instances are folded in world-space
 * alongside root parts, matching what `inspect_cad` reports. */
export function measureDocument(ir: Document, engine: Engine): GeometryMetrics {
  let scene: EvaluatedScene;
  try {
    scene = engine.evaluate(ir);
  } catch {
    return { ...EMPTY };
  }
  const bodies: TriangleMesh[] = sceneExportUnits(scene).map(worldMesh);
  if (bodies.length === 0) return { ...EMPTY };

  let volume = 0;
  let area = 0;
  const min: [number, number, number] = [Infinity, Infinity, Infinity];
  const max: [number, number, number] = [-Infinity, -Infinity, -Infinity];

  for (const mesh of bodies) {
    const props = computeMeshProperties(mesh);
    volume += props.volume;
    area += props.area;
    min[0] = Math.min(min[0], props.bbox.min.x);
    min[1] = Math.min(min[1], props.bbox.min.y);
    min[2] = Math.min(min[2], props.bbox.min.z);
    max[0] = Math.max(max[0], props.bbox.max.x);
    max[1] = Math.max(max[1], props.bbox.max.y);
    max[2] = Math.max(max[2], props.bbox.max.z);
  }

  if (!Number.isFinite(min[0])) return { ...EMPTY };

  const dx = max[0] - min[0];
  const dy = max[1] - min[1];
  const dz = max[2] - min[2];

  return {
    ok: true,
    parts: bodies.length,
    volume_mm3: volume,
    surface_area_mm2: area,
    footprint_mm2: dx * dy,
    max_dim_mm: Math.max(dx, dy, dz),
    bbox: { min, max },
  };
}
