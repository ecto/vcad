/**
 * inspect (v2) — geometry properties over a doc handle.
 *
 * Wraps the v1 inspect logic but takes a `DocRef` (handle or inline IR)
 * and returns the universal envelope. Per-part properties land in the
 * envelope's `result`; aggregate stats are already in `stats`.
 */

import type { Document, NodeId } from "@vcad/ir";
import type { Engine, TriangleMesh } from "@vcad/engine";
import type { ToolResult } from "../envelope.js";
import { fail, ok } from "../envelope.js";
import { resolveRef } from "../handles.js";
import type { DocRef } from "../types.js";

export const inspectV2Schema = {
  type: "object" as const,
  properties: {
    doc: { description: "Doc handle or inline IR." },
    parts: {
      type: "array" as const,
      items: { type: "string" as const },
      description:
        "Optional list of part names (NodeRefs) to include in `per_part`. Omit for all.",
    },
  },
  required: ["doc"],
};

interface InspectV2Input {
  doc: DocRef;
  parts?: string[];
}

interface PartProperties {
  volume_mm3: number;
  surface_area_mm2: number;
  bbox: { min: { x: number; y: number; z: number }; max: { x: number; y: number; z: number }; size: { x: number; y: number; z: number } };
  center_of_mass: { x: number; y: number; z: number };
  triangles: number;
  mass_g?: number;
  density_kg_m3?: number;
  material?: string;
}

interface InspectResult {
  aggregate: PartProperties;
  per_part: Record<string, PartProperties>;
  validity: { manifold: boolean; closed: boolean; orientable: boolean; warnings: string[] };
}

export function inspectV2(input: unknown, engine: Engine): ToolResult {
  const startedAt = performance.now();
  const args = (input ?? {}) as InspectV2Input;
  if (args.doc === undefined) return fail("invalid_input", "Missing `doc`.");

  const { doc, handle } = resolveRef(args.doc);
  const scene = engine.evaluate(doc);

  if (scene.parts.length === 0) {
    return fail("empty_document", "Document has no parts to inspect.");
  }

  const filter = args.parts ? new Set(args.parts) : null;
  const rootName = (rootId: NodeId, idx: number): string =>
    doc.nodes[String(rootId)]?.name ?? `part_${idx + 1}`;

  const perPart: Record<string, PartProperties> = {};
  let totalVolume = 0;
  let totalArea = 0;
  let totalTriangles = 0;
  let totalMass = 0;
  let hasMass = false;
  let weightedCx = 0,
    weightedCy = 0,
    weightedCz = 0;
  const aggBox = freshBox();

  for (let i = 0; i < scene.parts.length; i++) {
    const part = scene.parts[i];
    const root = doc.roots[i];
    const name = root ? rootName(root.root, i) : `part_${i + 1}`;
    if (filter && !filter.has(name)) continue;

    const props = computeMeshProperties(part.mesh);
    const materialKey = part.material ?? root?.material ?? "default";
    const density = doc.materials?.[materialKey]?.density;
    const massG = density ? (props.volume / 1e9) * density * 1000 : undefined;

    perPart[name] = {
      volume_mm3: r3(props.volume),
      surface_area_mm2: r3(props.area),
      bbox: bboxWithSize(props.bbox),
      center_of_mass: r3vec(props.centroid),
      triangles: props.triangles,
      mass_g: massG !== undefined ? r3(massG) : undefined,
      density_kg_m3: density,
      material: materialKey,
    };

    totalVolume += props.volume;
    totalArea += props.area;
    totalTriangles += props.triangles;
    weightedCx += props.centroid.x * props.volume;
    weightedCy += props.centroid.y * props.volume;
    weightedCz += props.centroid.z * props.volume;
    expandBox(aggBox, props.bbox);
    if (massG !== undefined) {
      totalMass += massG;
      hasMass = true;
    }
  }

  const aggregate: PartProperties = {
    volume_mm3: r3(totalVolume),
    surface_area_mm2: r3(totalArea),
    bbox: bboxWithSize(aggBox),
    center_of_mass:
      totalVolume > 1e-10
        ? r3vec({ x: weightedCx / totalVolume, y: weightedCy / totalVolume, z: weightedCz / totalVolume })
        : { x: 0, y: 0, z: 0 },
    triangles: totalTriangles,
    mass_g: hasMass ? r3(totalMass) : undefined,
  };

  const result: InspectResult = {
    aggregate,
    per_part: perPart,
    validity: assessValidity(doc, scene.parts.length),
  };

  return ok({ result, handle, doc, engine, startedAt });
}

interface BBox {
  min: { x: number; y: number; z: number };
  max: { x: number; y: number; z: number };
}

function freshBox(): BBox {
  return {
    min: { x: Infinity, y: Infinity, z: Infinity },
    max: { x: -Infinity, y: -Infinity, z: -Infinity },
  };
}

function expandBox(into: BBox, from: BBox): void {
  into.min.x = Math.min(into.min.x, from.min.x);
  into.min.y = Math.min(into.min.y, from.min.y);
  into.min.z = Math.min(into.min.z, from.min.z);
  into.max.x = Math.max(into.max.x, from.max.x);
  into.max.y = Math.max(into.max.y, from.max.y);
  into.max.z = Math.max(into.max.z, from.max.z);
}

function bboxWithSize(b: BBox) {
  if (!isFinite(b.min.x)) {
    return {
      min: { x: 0, y: 0, z: 0 },
      max: { x: 0, y: 0, z: 0 },
      size: { x: 0, y: 0, z: 0 },
    };
  }
  return {
    min: r3vec(b.min),
    max: r3vec(b.max),
    size: r3vec({ x: b.max.x - b.min.x, y: b.max.y - b.min.y, z: b.max.z - b.min.z }),
  };
}

function computeMeshProperties(mesh: TriangleMesh) {
  let volume = 0;
  let area = 0;
  let cx = 0,
    cy = 0,
    cz = 0;
  const bbox = freshBox();
  const tri = mesh.indices.length / 3;

  for (let t = 0; t < tri; t++) {
    const i0 = mesh.indices[t * 3] * 3;
    const i1 = mesh.indices[t * 3 + 1] * 3;
    const i2 = mesh.indices[t * 3 + 2] * 3;
    const ax = mesh.positions[i0],
      ay = mesh.positions[i0 + 1],
      az = mesh.positions[i0 + 2];
    const bx = mesh.positions[i1],
      by = mesh.positions[i1 + 1],
      bz = mesh.positions[i1 + 2];
    const cxv = mesh.positions[i2],
      cyv = mesh.positions[i2 + 1],
      czv = mesh.positions[i2 + 2];

    const v =
      (ax * (by * czv - cyv * bz) -
        bx * (ay * czv - cyv * az) +
        cxv * (ay * bz - by * az)) /
      6;
    volume += v;

    const ex1 = bx - ax,
      ey1 = by - ay,
      ez1 = bz - az;
    const ex2 = cxv - ax,
      ey2 = cyv - ay,
      ez2 = czv - az;
    const cxn = ey1 * ez2 - ez1 * ey2;
    const cyn = ez1 * ex2 - ex1 * ez2;
    const czn = ex1 * ey2 - ey1 * ex2;
    area += Math.hypot(cxn, cyn, czn) / 2;

    cx += (v * (ax + bx + cxv)) / 4;
    cy += (v * (ay + by + cyv)) / 4;
    cz += (v * (az + bz + czv)) / 4;

    if (ax < bbox.min.x) bbox.min.x = ax;
    if (ay < bbox.min.y) bbox.min.y = ay;
    if (az < bbox.min.z) bbox.min.z = az;
    if (ax > bbox.max.x) bbox.max.x = ax;
    if (ay > bbox.max.y) bbox.max.y = ay;
    if (az > bbox.max.z) bbox.max.z = az;
    if (bx < bbox.min.x) bbox.min.x = bx;
    if (by < bbox.min.y) bbox.min.y = by;
    if (bz < bbox.min.z) bbox.min.z = bz;
    if (bx > bbox.max.x) bbox.max.x = bx;
    if (by > bbox.max.y) bbox.max.y = by;
    if (bz > bbox.max.z) bbox.max.z = bz;
    if (cxv < bbox.min.x) bbox.min.x = cxv;
    if (cyv < bbox.min.y) bbox.min.y = cyv;
    if (czv < bbox.min.z) bbox.min.z = czv;
    if (cxv > bbox.max.x) bbox.max.x = cxv;
    if (cyv > bbox.max.y) bbox.max.y = cyv;
    if (czv > bbox.max.z) bbox.max.z = czv;
  }

  const abs = Math.abs(volume);
  if (abs > 1e-10) {
    cx /= volume;
    cy /= volume;
    cz /= volume;
  }
  return { volume: abs, area, bbox, centroid: { x: cx, y: cy, z: cz }, triangles: tri };
}

function assessValidity(doc: Document, partCount: number) {
  const warnings: string[] = [];
  if (partCount === 0) warnings.push("no parts");
  for (const root of doc.roots) {
    if (!doc.nodes[String(root.root)]) {
      warnings.push(`dangling root: ${root.root}`);
    }
  }
  return {
    manifold: warnings.length === 0,
    closed: warnings.length === 0,
    orientable: true,
    warnings,
  };
}

const r3 = (n: number) => Math.round(n * 1000) / 1000;
const r3vec = (v: { x: number; y: number; z: number }) => ({
  x: r3(v.x),
  y: r3(v.y),
  z: r3(v.z),
});
