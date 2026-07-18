/**
 * Mutation integrity metrics — the trust layer under every geometry edit.
 *
 * Field report (torr session 3): every kernel bug in the catalogue produced
 * a *successful* mutation response; corrupt geometry was only caught by
 * recomputing expected volumes out-of-band. This module closes that gap:
 * every mutation response carries `integrity` — volume, bounding box,
 * center of mass, watertightness, and (for parts built around a circular
 * pattern) the CoM's distance from the pattern axis, which is a free
 * integrity certificate for rotationally symmetric parts. Cheap invariant
 * violations are surfaced as `warnings`.
 */

import type { Document } from "@vcad/ir";
import type { Engine, TriangleMesh } from "@vcad/engine";
import { transformMesh } from "@vcad/engine";

/** Compact aggregate geometry report attached to mutation results. */
export interface IntegrityReport {
  volume_mm3: number;
  surface_area_mm2: number;
  bounding_box: {
    min: { x: number; y: number; z: number };
    max: { x: number; y: number; z: number };
  } | null;
  center_of_mass: { x: number; y: number; z: number } | null;
  /** Every directed mesh edge has a matching opposite. */
  watertight: boolean;
  /** Unpaired directed edges across all parts (0 when watertight). */
  open_edges: number;
  parts: number;
  /**
   * Assembly instances included in the aggregate (world-posed). Present only
   * when the document places instances; without it a successful N-instance
   * assembly reported `parts: 0, volume: 0` — indistinguishable from a
   * silent failure.
   */
  instances?: number;
  triangles: number;
  /**
   * Distance (mm) from the aggregate CoM to each distinct circular-pattern
   * axis in the document. A patterned part's CoM belongs ON its axis;
   * drift is the cheapest possible corruption signal. Omitted when the
   * document has no circular patterns.
   */
  com_axis_distance_mm?: number[];
  /** Cheap invariant violations worth a second look. Empty when clean. */
  warnings: string[];
}

const round3 = (v: number): number => Math.round(v * 1000) / 1000;

/**
 * Isoperimetric impossibility test. Every real solid satisfies
 * A³ ≥ 36·π·V² (equality only for the sphere), and the bound holds exactly
 * for closed, consistently wound meshes too — so a (volume, area) pair that
 * violates it cannot bound any solid. In the field this is the signature of
 * a wrong-but-watertight boolean result: the mesh looks fine, but the
 * volume integral reports material the surface could not possibly enclose
 * (e.g. ~661 mm³ from ~100 mm², where 100 mm² can enclose at most ~94 mm³).
 *
 * Returns the maximum volume the given area could enclose when the pair is
 * impossible, or null when the pair is geometrically consistent. The 0.1%
 * slack absorbs f32 mesh coordinates and reporting round-off; genuine
 * violations overshoot the bound by orders of magnitude.
 */
export function isoperimetricViolation(
  volume_mm3: number,
  area_mm2: number,
): { max_volume_mm3: number } | null {
  const volume = Math.abs(volume_mm3);
  if (!(volume > 1e-9) || !(area_mm2 > 0)) return null;
  const bound = 36 * Math.PI * volume * volume;
  const cube = area_mm2 * area_mm2 * area_mm2;
  if (cube >= bound * 0.999) return null;
  return { max_volume_mm3: Math.sqrt(cube / (36 * Math.PI)) };
}

/** Signed tetra volume of a triangle against the origin. */
function signedVolume(
  p1: readonly number[],
  p2: readonly number[],
  p3: readonly number[],
): number {
  return (
    (p1[0] * (p2[1] * p3[2] - p3[1] * p2[2]) -
      p2[0] * (p1[1] * p3[2] - p3[1] * p1[2]) +
      p3[0] * (p1[1] * p2[2] - p2[1] * p1[2])) /
    6.0
  );
}

/** Unsigned area of a triangle. */
function triangleArea(
  p1: readonly number[],
  p2: readonly number[],
  p3: readonly number[],
): number {
  const ax = p2[0] - p1[0];
  const ay = p2[1] - p1[1];
  const az = p2[2] - p1[2];
  const bx = p3[0] - p1[0];
  const by = p3[1] - p1[1];
  const bz = p3[2] - p1[2];
  const cx = ay * bz - az * by;
  const cy = az * bx - ax * bz;
  const cz = ax * by - ay * bx;
  return Math.sqrt(cx * cx + cy * cy + cz * cz) / 2;
}

/**
 * Count unpaired directed edges in a mesh. Vertices are matched by
 * quantized position (meshes may duplicate vertices per face), and each
 * undirected edge must be traversed once in each direction by the
 * triangle winding for the surface to be closed.
 */
function countOpenEdges(mesh: TriangleMesh): number {
  const quantum = 1e-5;
  const keyOf = (i: number): string => {
    const b = i * 3;
    return (
      Math.round(mesh.positions[b] / quantum) +
      "," +
      Math.round(mesh.positions[b + 1] / quantum) +
      "," +
      Math.round(mesh.positions[b + 2] / quantum)
    );
  };
  // Net directed traversal count per undirected edge: +1 forward, −1
  // backward. Closed surface ⇒ every entry nets to zero.
  const net = new Map<string, number>();
  const tris = mesh.indices.length / 3;
  for (let t = 0; t < tris; t++) {
    const ks = [
      keyOf(mesh.indices[t * 3]),
      keyOf(mesh.indices[t * 3 + 1]),
      keyOf(mesh.indices[t * 3 + 2]),
    ];
    for (let e = 0; e < 3; e++) {
      const a = ks[e];
      const b = ks[(e + 1) % 3];
      if (a === b) continue; // degenerate (pinch column) edge
      const forward = a < b;
      const key = forward ? `${a}|${b}` : `${b}|${a}`;
      net.set(key, (net.get(key) ?? 0) + (forward ? 1 : -1));
    }
  }
  let open = 0;
  for (const count of net.values()) {
    open += Math.abs(count);
  }
  return open;
}

/** Distinct circular-pattern axes in the document (deduped by line). */
function circularPatternAxes(
  doc: Document,
): Array<{ origin: [number, number, number]; dir: [number, number, number] }> {
  const axes: Array<{
    origin: [number, number, number];
    dir: [number, number, number];
  }> = [];
  for (const node of Object.values(doc.nodes)) {
    const op = node.op as {
      type?: string;
      axis_origin?: { x: number; y: number; z: number };
      axis_dir?: { x: number; y: number; z: number };
    };
    if (op.type !== "CircularPattern" || !op.axis_origin || !op.axis_dir) {
      continue;
    }
    const len = Math.hypot(op.axis_dir.x, op.axis_dir.y, op.axis_dir.z);
    if (len < 1e-12) continue;
    const dir: [number, number, number] = [
      op.axis_dir.x / len,
      op.axis_dir.y / len,
      op.axis_dir.z / len,
    ];
    const origin: [number, number, number] = [
      op.axis_origin.x,
      op.axis_origin.y,
      op.axis_origin.z,
    ];
    const sameLine = axes.some((a) => {
      const cross = Math.hypot(
        a.dir[1] * dir[2] - a.dir[2] * dir[1],
        a.dir[2] * dir[0] - a.dir[0] * dir[2],
        a.dir[0] * dir[1] - a.dir[1] * dir[0],
      );
      if (cross > 1e-9) return false;
      // Parallel: same line iff origin offset is along the axis.
      const d = [
        origin[0] - a.origin[0],
        origin[1] - a.origin[1],
        origin[2] - a.origin[2],
      ];
      const along = d[0] * a.dir[0] + d[1] * a.dir[1] + d[2] * a.dir[2];
      const radial = Math.hypot(
        d[0] - along * a.dir[0],
        d[1] - along * a.dir[1],
        d[2] - along * a.dir[2],
      );
      return radial < 1e-9;
    });
    if (!sameLine) axes.push({ origin, dir });
  }
  return axes;
}

/**
 * Evaluate the document and compute the integrity report. Returns null when
 * the engine cannot evaluate (report is best-effort — a mutation must never
 * fail because its integrity accounting did).
 */
export function computeIntegrity(
  doc: Document,
  engine: Engine,
): IntegrityReport | null {
  let scene: {
    parts: Array<{ mesh: TriangleMesh }>;
    instances?: Array<{
      mesh: TriangleMesh;
      transform?: {
        translation: [number, number, number];
        rotation: [number, number, number, number];
        scale: [number, number, number];
      };
    }>;
  };
  try {
    scene = engine.evaluate(doc);
  } catch {
    return null;
  }

  // Assembly instances contribute world-posed meshes to the aggregate, the
  // same way check_clearance's candidate set bakes FK transforms. Without
  // this, an assembly-only document reported parts: 0, volume: 0.
  const instanceMeshes: TriangleMesh[] = [];
  for (const inst of scene.instances ?? []) {
    const mesh = inst.transform
      ? transformMesh(inst.mesh, {
          translate: inst.transform.translation,
          rotate: inst.transform.rotation,
          scale: inst.transform.scale,
        })
      : inst.mesh;
    if (!mesh || mesh.positions.length === 0) continue;
    instanceMeshes.push(mesh);
  }

  let volume = 0;
  let area = 0;
  let cx = 0;
  let cy = 0;
  let cz = 0;
  let triangles = 0;
  let openEdges = 0;
  const bbox = {
    min: { x: Infinity, y: Infinity, z: Infinity },
    max: { x: -Infinity, y: -Infinity, z: -Infinity },
  };
  const warnings: string[] = [];

  const meshes: TriangleMesh[] = [
    ...scene.parts.map((p) => p.mesh),
    ...instanceMeshes,
  ];
  for (let partIndex = 0; partIndex < meshes.length; partIndex++) {
    const mesh = meshes[partIndex];
    const tris = mesh.indices.length / 3;
    triangles += tris;
    let partVolume = 0;
    let partArea = 0;
    for (let t = 0; t < tris; t++) {
      const p = (k: number): number[] => {
        const b = mesh.indices[t * 3 + k] * 3;
        return [
          mesh.positions[b],
          mesh.positions[b + 1],
          mesh.positions[b + 2],
        ];
      };
      const [p1, p2, p3] = [p(0), p(1), p(2)];
      const v = signedVolume(p1, p2, p3);
      partVolume += v;
      partArea += triangleArea(p1, p2, p3);
      cx += (v * (p1[0] + p2[0] + p3[0])) / 4;
      cy += (v * (p1[1] + p2[1] + p3[1])) / 4;
      cz += (v * (p1[2] + p2[2] + p3[2])) / 4;
      for (const q of [p1, p2, p3]) {
        bbox.min.x = Math.min(bbox.min.x, q[0]);
        bbox.min.y = Math.min(bbox.min.y, q[1]);
        bbox.min.z = Math.min(bbox.min.z, q[2]);
        bbox.max.x = Math.max(bbox.max.x, q[0]);
        bbox.max.y = Math.max(bbox.max.y, q[1]);
        bbox.max.z = Math.max(bbox.max.z, q[2]);
      }
    }
    if (partVolume < 0) {
      warnings.push(
        `part mesh has net negative volume (${round3(partVolume)} mm³) — inverted winding`,
      );
    }
    const impossible = isoperimetricViolation(partVolume, partArea);
    if (impossible) {
      warnings.push(
        `part ${partIndex + 1} volume ${round3(Math.abs(partVolume))} mm³ is isoperimetrically impossible for its ` +
          `${round3(partArea)} mm² of surface (A³ ≥ 36πV² for any real solid; this area can enclose at most ` +
          `≈${round3(impossible.max_volume_mm3)} mm³) — the volume integral is corrupt, do not trust this geometry`,
      );
    }
    volume += partVolume;
    area += partArea;
    openEdges += countOpenEdges(mesh);
  }

  const hasBbox = Number.isFinite(bbox.min.x);
  const com =
    Math.abs(volume) > 1e-10
      ? { x: cx / volume, y: cy / volume, z: cz / volume }
      : null;

  if (hasBbox && Math.abs(volume) <= 1e-10 && triangles > 0) {
    warnings.push(
      "geometry has surface area but no enclosed volume — likely a degenerate or fully cancelled body",
    );
  }
  if (openEdges > 0) {
    warnings.push(`mesh is not watertight (${openEdges} unpaired edges)`);
  }
  if (com && hasBbox) {
    const eps =
      1e-6 *
      Math.max(
        bbox.max.x - bbox.min.x,
        bbox.max.y - bbox.min.y,
        bbox.max.z - bbox.min.z,
        1,
      );
    const inside =
      com.x >= bbox.min.x - eps &&
      com.x <= bbox.max.x + eps &&
      com.y >= bbox.min.y - eps &&
      com.y <= bbox.max.y + eps &&
      com.z >= bbox.min.z - eps &&
      com.z <= bbox.max.z + eps;
    if (!inside) {
      warnings.push(
        "center of mass falls outside the bounding box — volume integral is unreliable (open or inconsistently wound mesh)",
      );
    }
  }

  const report: IntegrityReport = {
    volume_mm3: round3(Math.abs(volume)),
    surface_area_mm2: round3(area),
    bounding_box: hasBbox
      ? {
          min: {
            x: round3(bbox.min.x),
            y: round3(bbox.min.y),
            z: round3(bbox.min.z),
          },
          max: {
            x: round3(bbox.max.x),
            y: round3(bbox.max.y),
            z: round3(bbox.max.z),
          },
        }
      : null,
    center_of_mass: com
      ? { x: round3(com.x), y: round3(com.y), z: round3(com.z) }
      : null,
    watertight: openEdges === 0,
    open_edges: openEdges,
    parts: scene.parts.length,
    ...(instanceMeshes.length > 0 ? { instances: instanceMeshes.length } : {}),
    triangles,
    warnings,
  };

  const axes = circularPatternAxes(doc);
  if (axes.length > 0 && com) {
    report.com_axis_distance_mm = axes.map((axis) => {
      const d = [
        com.x - axis.origin[0],
        com.y - axis.origin[1],
        com.z - axis.origin[2],
      ];
      const along =
        d[0] * axis.dir[0] + d[1] * axis.dir[1] + d[2] * axis.dir[2];
      return round3(
        Math.hypot(
          d[0] - along * axis.dir[0],
          d[1] - along * axis.dir[1],
          d[2] - along * axis.dir[2],
        ),
      );
    });
    const worst = Math.max(...report.com_axis_distance_mm);
    // Scale-relative drift gate: a patterned part's CoM belongs on its
    // axis; 0.5% of the part's extent of drift is beyond numeric noise.
    if (hasBbox) {
      const extent = Math.max(
        bbox.max.x - bbox.min.x,
        bbox.max.y - bbox.min.y,
        bbox.max.z - bbox.min.z,
      );
      if (worst > Math.max(0.05, extent * 0.005)) {
        warnings.push(
          `center of mass is ${worst} mm off the circular-pattern axis — patterned geometry may be corrupt or asymmetric`,
        );
      }
    }
  }

  return report;
}

/**
 * Merge an integrity report into a tool result: into
 * `structuredContent.integrity`, and into the first JSON text block (or as
 * a trailing text block when the first block isn't JSON, e.g.
 * create_cad_loon's raw document output). Structurally typed to accept any
 * ToolResult-shaped value — image and resource blocks pass through
 * untouched.
 */
export function appendIntegrity(
  result: {
    content: Array<{ type: string; text?: string }>;
    structuredContent?: Record<string, unknown>;
  },
  integrity: IntegrityReport,
): void {
  result.structuredContent = { ...result.structuredContent, integrity };

  const block = result.content[0];
  if (block && block.type === "text" && typeof block.text === "string") {
    try {
      const parsed = JSON.parse(block.text) as Record<string, unknown>;
      parsed.integrity = integrity;
      block.text = JSON.stringify(parsed);
      return;
    } catch {
      // fall through — first block is not JSON
    }
  }
  result.content.push({
    type: "text",
    text: JSON.stringify({ integrity }),
  });
}
