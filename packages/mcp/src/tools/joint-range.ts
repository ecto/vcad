/**
 * `check_joint_range` — joint range-of-motion clearance sweep.
 *
 * For each revolute joint (or the named ones), drive it from zero toward each
 * of its limits in fixed steps with every other joint at zero, and report the
 * first angle each way where the moving subtree's overlap with the rest of
 * the assembly grows past its zero-pose overlap by more than a threshold
 * (mm³), plus which part pairs collide there. The zero-pose overlap is the
 * baseline because real mechanisms carry deliberate overlaps — coaxial
 * drums, a motor seated in its bore — and those are design, not collisions.
 *
 * Overlap volume is measured on a fixed world lattice of vertical ray
 * columns: each column is cast through each mesh once, crossings accumulate
 * a winding number, and the inside intervals along z are exact. Volume is
 * Σ h² · |moving ∩ rest| per column, with each side unioned first, so parts
 * overlapping within one side are not double-counted. Stationary columns are
 * cast once; only the moving subtree re-casts per step.
 *
 * This is the TS twin of `vcad_kernel_assembly::check_joint_range` (the
 * `vcad check-joint-range` CLI); keep the two in step.
 */

import { solveForwardKinematics, type Engine, type TriangleMesh } from "@vcad/engine";
import type { Document, Transform3D } from "@vcad/ir";
import { getSession } from "./session-core.js";
import { behavior, type ToolDef } from "./tool-def.js";
import { placeMesh, poseableParts, round6, type PoseablePart } from "./sweep.js";

/** Ceiling on sweep work so one call can't wedge the server. */
const MAX_STEPS_PER_SIDE = 720;

export interface JointRangeOptions {
  joints?: string[];
  stepDeg?: number;
  thresholdMm3?: number;
  resolutionMm?: number;
  /** "all" (default) or "chain": only the joint's own kinematic chain. */
  scope?: "all" | "chain";
}

interface CollidingPair {
  moving: string;
  stationary: string;
  new_overlap_mm3: number;
}

interface RangeSide {
  limit_deg: number;
  free_to_deg: number;
  first_collision_deg: number | null;
  new_overlap_mm3: number | null;
  hits: CollidingPair[];
}

export interface JointRangeRow {
  joint_id: string;
  name?: string;
  limits_deg: [number, number];
  moving: string[];
  baseline_overlap_mm3: number;
  negative: RangeSide;
  positive: RangeSide;
}

export class JointRangeError extends Error {}

type Spans = Array<[number, number]>;
type Columns = Map<number, Spans>;

/** Column key: two signed 16-bit-ish indices packed into one number. */
const key = (i: number, j: number) => i * 1_048_576 + j;

class Lattice {
  readonly eps: number;
  constructor(readonly h: number) {
    // Keeps rays off the axis-aligned edges and vertices CAD meshes are full of.
    this.eps = h * 1.234567e-4;
  }
  center(i: number) {
    return (i + 0.5) * this.h + this.eps;
  }
  cast(mesh: TriangleMesh): Columns {
    const p = mesh.positions;
    const idx = mesh.indices;
    const hits = new Map<number, Array<[number, number]>>();
    for (let t = 0; t + 2 < idx.length; t += 3) {
      const a = idx[t] * 3, b = idx[t + 1] * 3, c = idx[t + 2] * 3;
      const ax = p[a], ay = p[a + 1], az = p[a + 2];
      const bx = p[b], by = p[b + 1], bz = p[b + 2];
      const cx = p[c], cy = p[c + 1], cz = p[c + 2];
      const area = (bx - ax) * (cy - ay) - (cx - ax) * (by - ay);
      if (Math.abs(area) < 1e-12) continue;
      // A ray going +z enters where the normal faces down.
      const sign = area < 0 ? 1 : -1;
      const i0 = Math.ceil((Math.min(ax, bx, cx) - this.eps) / this.h - 0.5);
      const i1 = Math.floor((Math.max(ax, bx, cx) - this.eps) / this.h - 0.5);
      const j0 = Math.ceil((Math.min(ay, by, cy) - this.eps) / this.h - 0.5);
      const j1 = Math.floor((Math.max(ay, by, cy) - this.eps) / this.h - 0.5);
      for (let i = i0; i <= i1; i++) {
        const x = this.center(i);
        for (let j = j0; j <= j1; j++) {
          const y = this.center(j);
          const w0 = ((bx - x) * (cy - y) - (cx - x) * (by - y)) / area;
          const w1 = ((cx - x) * (ay - y) - (ax - x) * (cy - y)) / area;
          const w2 = 1 - w0 - w1;
          if (w0 < 0 || w1 < 0 || w2 < 0) continue;
          const z = w0 * az + w1 * bz + w2 * cz;
          const k = key(i, j);
          let list = hits.get(k);
          if (!list) hits.set(k, (list = []));
          list.push([z, sign]);
        }
      }
    }
    const out: Columns = new Map();
    for (const [k, crossings] of hits) {
      crossings.sort((u, v) => u[0] - v[0]);
      let winding = 0;
      let start = 0;
      const spans: Spans = [];
      for (const [z, s] of crossings) {
        const was = winding !== 0;
        winding += s;
        const is = winding !== 0;
        if (!was && is) start = z;
        else if (was && !is && z > start) spans.push([start, z]);
      }
      if (spans.length > 0) out.set(k, spans);
    }
    return out;
  }
}

function union(spans: Spans): Spans {
  spans.sort((u, v) => u[0] - v[0]);
  const out: Spans = [];
  for (const [a, b] of spans) {
    const last = out[out.length - 1];
    if (last && a <= last[1]) last[1] = Math.max(last[1], b);
    else out.push([a, b]);
  }
  return out;
}

function intersectLen(a: Spans, b: Spans): number {
  let i = 0, j = 0, len = 0;
  while (i < a.length && j < b.length) {
    const lo = Math.max(a[i][0], b[j][0]);
    const hi = Math.min(a[i][1], b[j][1]);
    if (hi > lo) len += hi - lo;
    if (a[i][1] < b[j][1]) i++;
    else j++;
  }
  return len;
}

interface Overlap {
  total: number;
  pairs: Map<string, number>;
}

function overlap(
  moving: Array<[number, Columns]>,
  stationary: Map<number, Array<[number, Spans]>>,
  area: number,
): Overlap {
  const pairs = new Map<string, number>();
  const movingByColumn = new Map<number, Spans>();
  for (const [m, cols] of moving) {
    for (const [k, spans] of cols) {
      const stat = stationary.get(k);
      if (!stat) continue;
      for (const [s, sspans] of stat) {
        const v = intersectLen(spans, sspans);
        if (v > 0) {
          const pk = `${m}|${s}`;
          pairs.set(pk, (pairs.get(pk) ?? 0) + v * area);
        }
      }
      let list = movingByColumn.get(k);
      if (!list) movingByColumn.set(k, (list = []));
      list.push(...spans.map((s) => [s[0], s[1]] as [number, number]));
    }
  }
  let total = 0;
  for (const [k, spans] of movingByColumn) {
    const s = union(stationary.get(k)!.flatMap(([, sp]) => sp.map((x) => [x[0], x[1]] as [number, number])));
    total += intersectLen(union(spans), s) * area;
  }
  return { total, pairs };
}

function sameTransform(a?: Transform3D, b?: Transform3D): boolean {
  if (!a || !b) return a === b;
  const d = (u: { x: number; y: number; z: number }, v: { x: number; y: number; z: number }) =>
    Math.abs(u.x - v.x) + Math.abs(u.y - v.y) + Math.abs(u.z - v.z);
  return d(a.translation, b.translation) < 1e-6 && d(a.rotation, b.rotation) < 1e-7;
}

/** Sampled angles from zero toward `end`: every `step`, plus `end` itself. */
export function sideAngles(end: number, step: number): number[] {
  if (end === 0) return [];
  const dir = Math.sign(end);
  const out: number[] = [];
  for (let k = 1; k * step < Math.abs(end) - 1e-9; k++) out.push(dir * k * step);
  out.push(end);
  return out;
}

/** Run the sweep. Joint states on `doc` are not touched (a clone is swept). */
export function checkJointRange(
  stored: Document,
  engine: Engine,
  opts: JointRangeOptions = {},
): JointRangeRow[] {
  const step = opts.stepDeg ?? 5;
  const threshold = opts.thresholdMm3 ?? 200;
  const h = opts.resolutionMm ?? 1;
  if (!(step > 0) || !(h > 0)) throw new JointRangeError("`step_deg` and `resolution_mm` must be positive.");
  if (!stored.instances || stored.instances.length === 0) {
    throw new JointRangeError("Document has no instances — not an assembly.");
  }

  // The sweep is defined about the zero pose: zero every joint on a clone.
  const doc = structuredClone(stored) as Document;
  const joints = doc.joints ?? [];
  for (const j of joints) j.state = 0;
  const isRevolute = (j: (typeof joints)[number]) => (j.kind as { type?: string }).type === "Revolute";

  let targets: number[];
  if (!opts.joints || opts.joints.length === 0) {
    targets = joints.map((_, i) => i).filter((i) => isRevolute(joints[i]));
    if (targets.length === 0) throw new JointRangeError("Document has no revolute joints to sweep.");
  } else {
    targets = opts.joints.map((w) => {
      let i = joints.findIndex((j) => j.id === w);
      if (i < 0) i = joints.findIndex((j) => j.name === w);
      if (i < 0) {
        const known = joints.filter(isRevolute).map((j) => j.name ?? j.id).join(", ");
        throw new JointRangeError(`No joint "${w}". Revolute joints: ${known || "none"}`);
      }
      if (!isRevolute(joints[i])) {
        throw new JointRangeError(`Joint "${w}" is not revolute; only revolute joints are swept.`);
      }
      return i;
    });
  }

  const parts: PoseablePart[] = poseableParts(doc, engine).filter((p) => p.instanceId);
  const ids = parts.map((p) => p.instanceId!);
  const zero = solveForwardKinematics(doc);

  // Which instances each revolute joint moves: perturb it and diff FK.
  const moves = new Map<string, Set<number>>();
  for (const j of joints) {
    if (!isRevolute(j)) continue;
    j.state = 17;
    const w = solveForwardKinematics(doc);
    j.state = 0;
    const moved = new Set<number>();
    ids.forEach((id, k) => {
      if (!sameTransform(w.get(id), zero.get(id))) moved.add(k);
    });
    moves.set(j.id, moved);
  }

  const lattice = new Lattice(h);
  const area = h * h;
  const zeroCols = parts.map((p) => lattice.cast(placeMesh(p.localMesh, zero.get(p.instanceId!) ?? p.transform)));

  const rows: JointRangeRow[] = [];
  for (const ji of targets) {
    const joint = joints[ji];
    const movingSet = moves.get(joint.id) ?? new Set<number>();
    const moving = [...movingSet].sort((a, b) => a - b);
    const subset = (a: Set<number>, b: Set<number>) => [...a].every((x) => b.has(x));
    const stationaryIdx = ids
      .map((_, k) => k)
      .filter((k) => !movingSet.has(k))
      .filter(
        (k) =>
          opts.scope !== "chain" ||
          [...moves].every(([other, set]) => !set.has(k) || other === joint.id || subset(movingSet, set)),
      );
    const stationary = new Map<number, Array<[number, Spans]>>();
    for (const s of stationaryIdx) {
      for (const [k, spans] of zeroCols[s]) {
        let list = stationary.get(k);
        if (!list) stationary.set(k, (list = []));
        list.push([s, spans]);
      }
    }

    const kind = joint.kind as { limits?: [number, number] };
    const limits: [number, number] = kind.limits ?? [-180, 180];

    const measure = (deg: number): Overlap => {
      joint.state = deg;
      const w = solveForwardKinematics(doc);
      joint.state = 0;
      const cols = moving.map(
        (k) => [k, lattice.cast(placeMesh(parts[k].localMesh, w.get(ids[k]) ?? parts[k].transform))] as [number, Columns],
      );
      return overlap(cols, stationary, area);
    };

    const base = measure(0);
    const sweepSide = (end: number): RangeSide => {
      const side: RangeSide = {
        limit_deg: end,
        free_to_deg: end,
        first_collision_deg: null,
        new_overlap_mm3: null,
        hits: [],
      };
      const angles = sideAngles(end, step);
      if (angles.length > MAX_STEPS_PER_SIDE) {
        throw new JointRangeError(
          `Sweeping "${joint.name ?? joint.id}" to ${end}° at ${step}° steps is ${angles.length} poses (limit ${MAX_STEPS_PER_SIDE}). Raise \`step_deg\`.`,
        );
      }
      let prev = 0;
      for (const deg of angles) {
        const o = measure(deg);
        const grew = o.total - base.total;
        if (grew > threshold) {
          side.free_to_deg = prev;
          side.first_collision_deg = round6(deg);
          side.new_overlap_mm3 = round6(grew);
          side.hits = [...o.pairs]
            .map(([pk, v]) => {
              const [m, s] = pk.split("|").map(Number);
              return {
                moving: ids[m],
                stationary: ids[s],
                new_overlap_mm3: round6(v - (base.pairs.get(pk) ?? 0)),
              };
            })
            .filter((p) => p.new_overlap_mm3 > 0)
            .sort((a, b) => b.new_overlap_mm3 - a.new_overlap_mm3)
            .slice(0, 8);
          break;
        }
        prev = deg;
      }
      side.free_to_deg = round6(side.free_to_deg);
      return side;
    };

    rows.push({
      joint_id: joint.id,
      ...(joint.name ? { name: joint.name } : {}),
      limits_deg: limits,
      moving: moving.map((k) => ids[k]),
      baseline_overlap_mm3: round6(base.total),
      negative: sweepSide(Math.min(limits[0], 0)),
      positive: sweepSide(Math.max(limits[1], 0)),
    });
  }
  return rows;
}

export const checkJointRangeSchema = {
  type: "object" as const,
  properties: {
    document_id: { type: "string" as const, description: "Session id of the jointed assembly." },
    joints: {
      type: "array" as const,
      items: { type: "string" as const },
      description: "Joint ids or names to sweep. Default: every revolute joint.",
    },
    step_deg: { type: "number" as const, description: "Sweep increment in degrees (default 5)." },
    threshold_mm3: {
      type: "number" as const,
      description: "New overlap past the zero pose that counts as a collision, mm³ (default 200).",
    },
    resolution_mm: {
      type: "number" as const,
      description: "Overlap-measurement column pitch in mm (default 1). Smaller is more accurate and slower.",
    },
    scope: {
      type: "string" as const,
      enum: ["all", "chain"],
      description:
        "What the moving subtree is checked against. `all` (default): every instance the joint does not move. `chain`: only geometry moved by no joint or only by this joint's ancestors — other limbs are posture, not design.",
    },
  },
  required: ["document_id"],
} as const;

export function checkJointRangeTool(args: Record<string, unknown>, engine: Engine) {
  const documentId = String(args.document_id ?? "");
  if (!documentId) return err("Pass `document_id` (the open CAD session).");
  let doc: Document;
  try {
    doc = getSession(documentId);
  } catch (e) {
    return err(e instanceof Error ? e.message : String(e));
  }
  const num = (v: unknown) => (v === undefined || v === null ? undefined : Number(v));
  let rows: JointRangeRow[];
  try {
    rows = checkJointRange(doc, engine, {
      joints: Array.isArray(args.joints) ? args.joints.map(String) : undefined,
      stepDeg: num(args.step_deg),
      thresholdMm3: num(args.threshold_mm3),
      resolutionMm: num(args.resolution_mm),
      scope: args.scope === "chain" ? "chain" : "all",
    });
  } catch (e) {
    if (e instanceof JointRangeError) return err(e.message);
    throw e;
  }
  const colliding = rows.filter((r) => r.negative.first_collision_deg !== null || r.positive.first_collision_deg !== null);
  const body = {
    document_id: documentId,
    joints: rows,
    summary: `${rows.length} joint(s) swept; ${colliding.length} collide before a limit.`,
    note: "Each joint is swept from 0 toward each limit with all other joints at 0. `free_to_deg` is the last collision-free sampled angle; `first_collision_deg` the first where overlap grew past the zero pose by more than the threshold, with `hits` naming the part pairs whose overlap grew. Volumes are sampled on a column lattice from evaluated tessellations. The session document is not modified.",
  };
  return {
    content: [{ type: "text" as const, text: JSON.stringify(body) }],
    structuredContent: { joint_range: body, document_id: documentId },
  };
}

function err(text: string) {
  return {
    content: [{ type: "text" as const, text: `Error: ${text}` }],
    isError: true as const,
  };
}

export const toolDefs: ToolDef[] = [
  {
    name: "check_joint_range",
    pack: null,
    description:
      "Joint range-of-motion clearance sweep for a jointed assembly. For each revolute joint (or the named `joints`), drives it from 0 toward each declared limit in `step_deg` steps with every other joint at 0, and reports the first angle each direction where the moving subtree's overlap with the rest of the assembly grows past its zero-pose overlap by more than `threshold_mm3` — plus `hits`, the part pairs that collide there. The zero-pose overlap is the baseline, so deliberate nested/coaxial overlaps don't count. `scope: \"chain\"` ignores other limbs (geometry moved by joints that aren't this joint's ancestors). Read-only: the session is not modified. Use `render_view` with `joint_state` to look at a reported pose, and `measure` with `sweep` for a two-part distance curve.",
    inputSchema: checkJointRangeSchema,
    handler: (a, c) => checkJointRangeTool(a, c.engine),
    behavior: behavior({}),
  },
];
