/**
 * Cross-domain PCB ↔ enclosure verification.
 *
 * vcad is the only stack with both a real BRep CAD kernel and a PCB engine, so
 * it can do something no EDA tool can: cross-check a board against the physical
 * case it lives in. This module is the verification core — given a board
 * outline + features and the enclosure cavity (extracted from a solid mesh), it
 * answers four questions a fab house can't:
 *
 *   1. Does the board fit the cavity with clearance?
 *   2. Do tall components clear the lid? (stack height vs cavity depth)
 *   3. Do the mounting holes land on the case standoffs?
 *   4. Do edge connectors line up with the wall cutouts?
 *
 * Everything here is pure (numbers in, verdict out) so it unit-tests without a
 * kernel. The mesh → cavity/standoff/opening extraction lives in
 * `extractEnclosureFeatures`, which is also pure (it walks triangle arrays).
 *
 * Frames: enclosure features are in **enclosure-world** (Z-up, mm). A board is
 * authored in its own **board-local** frame (origin-corner outline, board
 * bottom at z=0, top at z=thickness). A {@link BoardPlacement} maps board-local
 * → world (Z-rotation then translation), the same convention the renderer and
 * `getPcbBoardTransform` use.
 */

import type { BoardOutline, Footprint, Pcb, Vec2, Vec3 } from "@vcad/ir";

// ===========================================================================
// Types
// ===========================================================================

/** Axis-aligned interior void of an enclosure, in enclosure-world coords. */
export interface EnclosureCavity {
  minX: number;
  maxX: number;
  minY: number;
  maxY: number;
  /** Top surface of the cavity floor (Z); the board's underside rests above. */
  floorZ: number;
  /** Underside of the lid / top of the usable cavity (Z). */
  ceilZ: number;
  /** True when a closed top was detected; false for an open-top case. */
  hasLid: boolean;
}

/** A boss/post rising from the cavity floor that a screw threads into. */
export interface Standoff {
  x: number;
  y: number;
  /** Z of the post's top face (where the board lands). */
  topZ: number;
  /** Approximate post radius (mm). */
  radius: number;
}

/** Which outer wall a feature sits against. */
export type WallEdge = "minX" | "maxX" | "minY" | "maxY";

/** An opening cut through a wall (e.g. a USB/JST port). */
export interface WallOpening {
  edge: WallEdge;
  /** Center of the opening along the wall (world XY). */
  center: Vec2;
  /** Opening width along the wall tangent (mm). */
  width: number;
  zMin: number;
  zMax: number;
}

/** Outer bounds + extracted interior features of an enclosure solid. */
export interface EnclosureFeatures {
  outer: { minX: number; maxX: number; minY: number; maxY: number; minZ: number; maxZ: number };
  cavity: EnclosureCavity | null;
  standoffs: Standoff[];
  openings: WallOpening[];
}

/** Maps board-local coordinates into the enclosure-world frame. */
export interface BoardPlacement {
  /** Board-local origin in enclosure-world coordinates. */
  offset: Vec3;
  /** CCW rotation about Z, in degrees. */
  rotationDeg: number;
}

/** A board mounting hole, in board-local coordinates. */
export interface MountingHole {
  x: number;
  y: number;
  diameter: number;
  ref?: string;
}

/** An edge connector, in board-local coordinates. */
export interface ConnectorRef {
  ref: string;
  x: number;
  y: number;
  /** Nearest board edge the connector exits through (board-local AABB). */
  edge: WallEdge | null;
  /** Component body height above the board (mm); 0 when unknown. */
  height: number;
}

/**
 * Per-component vertical extent in board-local Z (board bottom = 0, top =
 * thickness). Front parts sit above `thickness`; back parts dip below 0.
 */
export interface ComponentExtent {
  ref: string;
  front: boolean;
  topZ: number;
  bottomZ: number;
}

export type CheckStatus = "pass" | "fail" | "warn" | "skip";

/** One cross-domain verification line. */
export interface EnclosureFitCheck {
  id: string;
  label: string;
  status: CheckStatus;
  detail: string;
  measurements?: Record<string, number | string | boolean>;
}

/** The full cross-domain verdict. */
export interface EnclosureFitReport {
  /** True when no check failed (warnings do not flip this; they are surfaced). */
  ok: boolean;
  /** True only when nothing failed AND nothing warned (fully verified). */
  verified: boolean;
  summary: string;
  clearance: number;
  placement: BoardPlacement;
  checks: EnclosureFitCheck[];
}

/** Input to the pure verification core. */
export interface EnclosureFitInput {
  outline: BoardOutline;
  cavity: EnclosureCavity;
  standoffs?: Standoff[];
  openings?: WallOpening[];
  mountingHoles?: MountingHole[];
  connectors?: ConnectorRef[];
  componentExtents?: ComponentExtent[];
  /** Where the board sits; auto-fit (centered, on standoffs) when omitted. */
  placement?: BoardPlacement;
  /** All-round clearance the board needs from the cavity walls (mm). Default 0.5. */
  clearance?: number;
  /** Board-bottom lift above the floor when no standoffs are given (mm). Default 0. */
  standoffHeight?: number;
  /** Hole-to-standoff alignment tolerance (mm). Default 0.6. */
  holeTolerance?: number;
}

// ===========================================================================
// Geometry helpers
// ===========================================================================

const DEFAULT_CLEARANCE = 0.5;
const DEFAULT_HOLE_TOL = 0.6;

/** Axis-aligned bounds of a polygon. */
export function outlineAabb(outline: BoardOutline): {
  minX: number;
  maxX: number;
  minY: number;
  maxY: number;
} {
  let minX = Infinity;
  let maxX = -Infinity;
  let minY = Infinity;
  let maxY = -Infinity;
  for (const v of outline.vertices) {
    if (v.x < minX) minX = v.x;
    if (v.x > maxX) maxX = v.x;
    if (v.y < minY) minY = v.y;
    if (v.y > maxY) maxY = v.y;
  }
  return { minX, maxX, minY, maxY };
}

/** Map a board-local point into the enclosure-world frame. */
export function toWorld(p: { x: number; y: number; z?: number }, placement: BoardPlacement): Vec3 {
  const t = (placement.rotationDeg * Math.PI) / 180;
  const cos = Math.cos(t);
  const sin = Math.sin(t);
  return {
    x: placement.offset.x + p.x * cos - p.y * sin,
    y: placement.offset.y + p.x * sin + p.y * cos,
    z: placement.offset.z + (p.z ?? 0),
  };
}

/** Nearest cavity (or AABB) edge to a point, by perpendicular distance. */
function nearestEdge(
  x: number,
  y: number,
  b: { minX: number; maxX: number; minY: number; maxY: number },
): WallEdge {
  const d: Array<[WallEdge, number]> = [
    ["minX", Math.abs(x - b.minX)],
    ["maxX", Math.abs(x - b.maxX)],
    ["minY", Math.abs(y - b.minY)],
    ["maxY", Math.abs(y - b.maxY)],
  ];
  d.sort((a, c) => a[1] - c[1]);
  return d[0][0];
}

const round2 = (n: number) => Math.round(n * 100) / 100;

// ===========================================================================
// Board feature extraction (Pcb → board-local features)
// ===========================================================================

/** World (board-frame) position of a pad on its footprint. */
function padWorld(fp: Footprint, pad: { position: Vec2; rotation?: number }): Vec2 {
  const t = ((fp.rotation ?? 0) * Math.PI) / 180;
  const cos = Math.cos(t);
  const sin = Math.sin(t);
  return {
    x: fp.position.x + pad.position.x * cos - pad.position.y * sin,
    y: fp.position.y + pad.position.x * sin + pad.position.y * cos,
  };
}

const MOUNT_RE = /mount(ing)?[_-]?hole|mountingpad|mounthole/i;

/**
 * Mounting holes the board declares, in board-local coords. Sourced from
 * MountingHole footprints (their origin) and from any NPTH pad (its drilled
 * position). Diameter comes from the drill spec, else the pad/footprint size.
 */
export function mountingHolesFromPcb(pcb: Pcb): MountingHole[] {
  const holes: MountingHole[] = [];
  for (const fp of pcb.footprints) {
    const isMount = MOUNT_RE.test(fp.footprintName) || MOUNT_RE.test(fp.ref);
    if (isMount) {
      // Diameter from the first pad's drill, else its outer size, else M3.
      const pad = fp.pads[0];
      let dia = 3.2;
      if (pad) {
        if (pad.drill && typeof pad.drill === "object") {
          const dd = pad.drill as { diameter?: number };
          if (typeof dd.diameter === "number") dia = dd.diameter;
        } else if (pad.shape.type === "Circle") {
          dia = pad.shape.diameter;
        }
      }
      holes.push({ x: round2(fp.position.x), y: round2(fp.position.y), diameter: round2(dia), ref: fp.ref });
      continue;
    }
    for (const pad of fp.pads) {
      if (pad.padType !== "NPTH") continue;
      const w = padWorld(fp, pad);
      let dia = 3.2;
      const dd = pad.drill as { diameter?: number } | undefined;
      if (dd && typeof dd.diameter === "number") dia = dd.diameter;
      else if (pad.shape.type === "Circle") dia = pad.shape.diameter;
      holes.push({ x: round2(w.x), y: round2(w.y), diameter: round2(dia), ref: fp.ref });
    }
  }
  return holes;
}

/**
 * Map kernel component meshes (board-local, board bottom z=0) to per-component
 * Z extents. Structurally typed so the engine's `ComponentMesh` and the pure
 * core stay decoupled. `front` comes from the matching footprint (default
 * front), and decides whether the part rises toward the lid or dips toward the
 * floor.
 */
export function componentExtentsFromMeshes(
  meshes: Array<{ footprint_ref: string; positions: ArrayLike<number> }>,
  pcb: Pcb,
): ComponentExtent[] {
  const frontByRef = new Map(pcb.footprints.map((fp) => [fp.ref, fp.front ?? true]));
  const out: ComponentExtent[] = [];
  for (const m of meshes) {
    let minZ = Infinity;
    let maxZ = -Infinity;
    for (let i = 2; i < m.positions.length; i += 3) {
      const z = m.positions[i];
      if (z < minZ) minZ = z;
      if (z > maxZ) maxZ = z;
    }
    if (!Number.isFinite(minZ)) continue;
    out.push({
      ref: m.footprint_ref,
      front: frontByRef.get(m.footprint_ref) ?? true,
      topZ: round2(maxZ),
      bottomZ: round2(minZ),
    });
  }
  return out;
}

const CONNECTOR_REF_RE = /^(J|CN|CON|USB|P)\d/i;
const CONNECTOR_NAME_RE =
  /usb|type[-_]?c|micro|mini|conn|header|jst|molex|rj45|hdr|terminal|socket|receptacle|barrel|dcjack/i;

/**
 * Edge connectors the board declares, in board-local coords, each tagged with
 * the nearest board edge (so the cutout check knows which wall to look at).
 */
export function connectorsFromPcb(pcb: Pcb, outline: BoardOutline): ConnectorRef[] {
  const aabb = outlineAabb(outline);
  const out: ConnectorRef[] = [];
  for (const fp of pcb.footprints) {
    const isConn =
      CONNECTOR_REF_RE.test(fp.ref) ||
      CONNECTOR_NAME_RE.test(fp.footprintName) ||
      CONNECTOR_NAME_RE.test(fp.value ?? "");
    if (!isConn) continue;
    out.push({
      ref: fp.ref,
      x: round2(fp.position.x),
      y: round2(fp.position.y),
      edge: nearestEdge(fp.position.x, fp.position.y, aabb),
      height: 0,
    });
  }
  return out;
}

// ===========================================================================
// Verification core
// ===========================================================================

/** Auto-fit placement: center the board in the cavity, resting on standoffs. */
function autoPlacement(input: EnclosureFitInput, clearance: number): BoardPlacement {
  const { cavity } = input;
  const a = outlineAabb(input.outline);
  const boardW = a.maxX - a.minX;
  const boardH = a.maxY - a.minY;
  const cavW = cavity.maxX - cavity.minX;
  const cavH = cavity.maxY - cavity.minY;
  // Center the outline AABB inside the cavity (board-local origin offset so
  // the outline's min corner lands at the centered position).
  const offX = cavity.minX + (cavW - boardW) / 2 - a.minX;
  const offY = cavity.minY + (cavH - boardH) / 2 - a.minY;
  const standoffTop =
    input.standoffs && input.standoffs.length > 0
      ? Math.max(...input.standoffs.map((s) => s.topZ))
      : cavity.floorZ + (input.standoffHeight ?? 0);
  void clearance;
  return { offset: { x: round2(offX), y: round2(offY), z: round2(standoffTop) }, rotationDeg: 0 };
}

/** Check 1 — board fits the cavity footprint with clearance on all sides. */
function checkBoardFit(input: EnclosureFitInput, placement: BoardPlacement, clearance: number): EnclosureFitCheck {
  const { cavity } = input;
  let minX = Infinity;
  let maxX = -Infinity;
  let minY = Infinity;
  let maxY = -Infinity;
  for (const v of input.outline.vertices) {
    const w = toWorld(v, placement);
    if (w.x < minX) minX = w.x;
    if (w.x > maxX) maxX = w.x;
    if (w.y < minY) minY = w.y;
    if (w.y > maxY) maxY = w.y;
  }
  const marginMinX = minX - cavity.minX;
  const marginMaxX = cavity.maxX - maxX;
  const marginMinY = minY - cavity.minY;
  const marginMaxY = cavity.maxY - maxY;
  const worst = Math.min(marginMinX, marginMaxX, marginMinY, marginMaxY);
  const sides: Array<[string, number]> = [
    ["-X", marginMinX],
    ["+X", marginMaxX],
    ["-Y", marginMinY],
    ["+Y", marginMaxY],
  ];
  const tight = sides.filter(([, m]) => m < clearance).map(([s]) => s);
  const ok = worst >= clearance - 1e-6;
  return {
    id: "board_fit",
    label: "Board fits cavity with clearance",
    status: ok ? "pass" : "fail",
    detail: ok
      ? `Board fits with ${round2(worst)}mm worst-case clearance (need ${clearance}mm)`
      : worst < 0
        ? `Board overhangs the cavity by ${round2(-worst)}mm on ${tight.join(", ")}`
        : `Clearance on ${tight.join(", ")} is ${round2(worst)}mm < required ${clearance}mm`,
    measurements: {
      worst_clearance_mm: round2(worst),
      margin_minus_x: round2(marginMinX),
      margin_plus_x: round2(marginMaxX),
      margin_minus_y: round2(marginMinY),
      margin_plus_y: round2(marginMaxY),
      board_w: round2(maxX - minX),
      board_h: round2(maxY - minY),
      cavity_w: round2(cavity.maxX - cavity.minX),
      cavity_h: round2(cavity.maxY - cavity.minY),
    },
  };
}

/** Check 2 — tall components clear the lid; back parts clear the floor. */
function checkLidClearance(input: EnclosureFitInput, placement: BoardPlacement, clearance: number): EnclosureFitCheck {
  const { cavity } = input;
  const extents = input.componentExtents ?? [];
  if (extents.length === 0) {
    return {
      id: "lid_clearance",
      label: "Components clear the lid",
      status: "skip",
      detail: "No component heights available (kernel component meshes unavailable)",
    };
  }
  const cavityDepth = cavity.ceilZ - cavity.floorZ;
  // Front parts rise above the board top into the cavity.
  const front = extents.filter((e) => e.front);
  const back = extents.filter((e) => !e.front);
  let tallest = { ref: "", top: -Infinity };
  for (const e of front) {
    const top = placement.offset.z + e.topZ;
    if (top > tallest.top) tallest = { ref: e.ref, top };
  }
  const lidGap = cavity.ceilZ - tallest.top; // free space above tallest part
  const topOk = front.length === 0 || lidGap >= clearance - 1e-6;

  // Back parts dip below the board into the standoff gap toward the floor.
  let lowest = { ref: "", bottom: Infinity };
  for (const e of back) {
    const bot = placement.offset.z + e.bottomZ;
    if (bot < lowest.bottom) lowest = { ref: e.ref, bottom: bot };
  }
  const floorGap = back.length > 0 ? lowest.bottom - cavity.floorZ : Infinity;
  const botOk = back.length === 0 || floorGap >= -1e-6;

  const ok = topOk && botOk;
  let detail: string;
  if (ok) {
    detail = `Tallest part ${tallest.ref || "—"} leaves ${round2(lidGap)}mm under the lid (cavity depth ${round2(cavityDepth)}mm)`;
    if (back.length > 0 && Number.isFinite(floorGap)) {
      detail += `; back-side ${lowest.ref} clears floor by ${round2(floorGap)}mm`;
    }
  } else if (!topOk) {
    detail = `${tallest.ref} is ${round2(-lidGap + clearance)}mm too tall — it ${lidGap < 0 ? "punches through" : "is within clearance of"} the lid (cavity depth ${round2(cavityDepth)}mm)`;
  } else {
    detail = `Back-side ${lowest.ref} collides with the floor by ${round2(-floorGap)}mm — raise the standoffs`;
  }
  return {
    id: "lid_clearance",
    label: "Components clear the lid",
    status: ok ? "pass" : "fail",
    detail,
    measurements: {
      cavity_depth_mm: round2(cavityDepth),
      tallest_ref: tallest.ref || "none",
      lid_gap_mm: Number.isFinite(lidGap) ? round2(lidGap) : "n/a",
      stack_top_z: Number.isFinite(tallest.top) ? round2(tallest.top) : "n/a",
      floor_gap_mm: Number.isFinite(floorGap) ? round2(floorGap) : "n/a",
    },
  };
}

/** Check 3 — every mounting hole lands on a case standoff. */
function checkMountingHoles(input: EnclosureFitInput, placement: BoardPlacement): EnclosureFitCheck {
  const holes = input.mountingHoles ?? [];
  const standoffs = input.standoffs ?? [];
  const tol = input.holeTolerance ?? DEFAULT_HOLE_TOL;
  if (holes.length === 0) {
    return {
      id: "mounting_holes",
      label: "Mounting holes land on standoffs",
      status: "skip",
      detail: "Board declares no mounting holes",
    };
  }
  if (standoffs.length === 0) {
    return {
      id: "mounting_holes",
      label: "Mounting holes land on standoffs",
      status: "skip",
      detail: `Board has ${holes.length} mounting hole(s) but no standoffs were detected in the enclosure`,
    };
  }
  let matched = 0;
  let worst = 0;
  const misses: string[] = [];
  for (const h of holes) {
    const w = toWorld(h, placement);
    let best = Infinity;
    for (const s of standoffs) {
      const d = Math.hypot(w.x - s.x, w.y - s.y);
      if (d < best) best = d;
    }
    if (best <= tol) {
      matched++;
      if (best > worst) worst = best;
    } else {
      misses.push(`${h.ref ?? "hole"}@(${round2(w.x)},${round2(w.y)}) is ${round2(best)}mm off`);
    }
  }
  const ok = matched === holes.length;
  return {
    id: "mounting_holes",
    label: "Mounting holes land on standoffs",
    status: ok ? "pass" : "fail",
    detail: ok
      ? `All ${holes.length} mounting holes align to standoffs (worst offset ${round2(worst)}mm, tol ${tol}mm)`
      : `${matched}/${holes.length} holes align — ${misses.join("; ")}`,
    measurements: {
      holes_total: holes.length,
      holes_matched: matched,
      standoffs: standoffs.length,
      tolerance_mm: tol,
      worst_offset_mm: round2(worst),
    },
  };
}

/** Check 4 — edge connectors line up with wall cutouts. */
function checkConnectors(input: EnclosureFitInput, placement: BoardPlacement, clearance: number): EnclosureFitCheck {
  const conns = input.connectors ?? [];
  const openings = input.openings ?? [];
  if (conns.length === 0) {
    return {
      id: "connector_cutouts",
      label: "Connectors align to wall cutouts",
      status: "skip",
      detail: "Board declares no edge connectors",
    };
  }
  // Connector world positions and which cavity wall each faces.
  const cav = input.cavity;
  let aligned = 0;
  const problems: string[] = [];
  for (const c of conns) {
    const w = toWorld(c, placement);
    const wallEdge = nearestEdge(w.x, w.y, cav);
    // The lateral coordinate along that wall.
    const along = wallEdge === "minX" || wallEdge === "maxX" ? w.y : w.x;
    const onWall = openings.filter((o) => o.edge === wallEdge);
    if (onWall.length === 0) {
      problems.push(`${c.ref} faces the ${wallEdge} wall but it has no cutout`);
      continue;
    }
    const hit = onWall.find((o) => {
      const oc = o.edge === "minX" || o.edge === "maxX" ? o.center.y : o.center.x;
      return Math.abs(along - oc) <= o.width / 2 + clearance;
    });
    if (hit) {
      aligned++;
    } else {
      const nearest = onWall.reduce((best, o) => {
        const oc = o.edge === "minX" || o.edge === "maxX" ? o.center.y : o.center.x;
        const off = Math.abs(along - oc) - o.width / 2;
        return off < best ? off : best;
      }, Infinity);
      problems.push(`${c.ref} on ${wallEdge} misses its cutout by ${round2(nearest)}mm`);
    }
  }
  const ok = aligned === conns.length;
  // No openings detected anywhere is a detection gap, not a hard failure.
  const status: CheckStatus = ok ? "pass" : openings.length === 0 ? "warn" : "fail";
  return {
    id: "connector_cutouts",
    label: "Connectors align to wall cutouts",
    status,
    detail: ok
      ? `All ${conns.length} connector(s) line up with wall cutouts`
      : openings.length === 0
        ? `No wall cutouts detected; ${conns.length} connector(s) would be enclosed: ${problems.join("; ")}`
        : `${aligned}/${conns.length} connectors aligned — ${problems.join("; ")}`,
    measurements: {
      connectors_total: conns.length,
      connectors_aligned: aligned,
      wall_openings: openings.length,
    },
  };
}

/**
 * Run the four cross-domain checks and assemble the verdict. Pure: pass it
 * extracted features and it returns a report — no kernel, no I/O.
 */
export function checkEnclosureFit(input: EnclosureFitInput): EnclosureFitReport {
  const clearance = input.clearance ?? DEFAULT_CLEARANCE;
  const placement = input.placement ?? autoPlacement(input, clearance);

  const checks: EnclosureFitCheck[] = [
    checkBoardFit(input, placement, clearance),
    checkLidClearance(input, placement, clearance),
    checkMountingHoles(input, placement),
    checkConnectors(input, placement, clearance),
  ];

  const failed = checks.filter((c) => c.status === "fail");
  const warned = checks.filter((c) => c.status === "warn");
  const passed = checks.filter((c) => c.status === "pass");
  const ok = failed.length === 0;
  const verified = ok && warned.length === 0;

  let summary: string;
  if (failed.length > 0) {
    summary = `Enclosure fit: FAIL — ${failed.map((c) => c.label.toLowerCase()).join("; ")}`;
  } else if (warned.length > 0) {
    summary = `Enclosure fit: UNVERIFIED — ${passed.length} passed, ${warned.length} warning(s): ${warned
      .map((c) => c.detail)
      .join("; ")}`;
  } else {
    summary = `Enclosure fit: PASS — ${passed.length}/${checks.length} checks (${checks
      .filter((c) => c.status === "pass")
      .map((c) => c.label.toLowerCase())
      .join(", ")})`;
  }

  return { ok, verified, summary, clearance, placement, checks };
}

// ===========================================================================
// Auto-derive a board from the cavity (the co-design starting point)
// ===========================================================================

/** Options for {@link deriveBoardFromCavity}. */
export interface DeriveBoardOptions {
  /** All-round wall clearance (mm). Default 0.5. */
  clearance?: number;
  /** Board thickness (mm). Default 1.6. */
  thickness?: number;
  /** Mounting-hole diameter (mm). Default 3.2 (M3 clearance). */
  holeDiameter?: number;
  /** Board lift above the floor when there are no standoffs (mm). Default 0. */
  standoffHeight?: number;
}

/**
 * Seed a board from an enclosure cavity: a rectangular outline inset by the
 * clearance, mounting holes over each detected standoff, and the placement that
 * drops it back into the case. The mirror of {@link checkEnclosureFit} — derive,
 * then verify the result holds.
 */
export function deriveBoardFromCavity(
  cavity: EnclosureCavity,
  standoffs: Standoff[],
  opts: DeriveBoardOptions = {},
): { outline: BoardOutline; mountingHoles: MountingHole[]; placement: BoardPlacement } {
  const clearance = opts.clearance ?? DEFAULT_CLEARANCE;
  const thickness = opts.thickness ?? 1.6;
  const holeDia = opts.holeDiameter ?? 3.2;
  const w = round2(cavity.maxX - cavity.minX - 2 * clearance);
  const h = round2(cavity.maxY - cavity.minY - 2 * clearance);
  const outline: BoardOutline = {
    vertices: [
      { x: 0, y: 0 },
      { x: w, y: 0 },
      { x: w, y: h },
      { x: 0, y: h },
    ],
    thickness,
  };
  const standoffTop =
    standoffs.length > 0
      ? Math.max(...standoffs.map((s) => s.topZ))
      : cavity.floorZ + (opts.standoffHeight ?? 0);
  const offX = cavity.minX + clearance;
  const offY = cavity.minY + clearance;
  const placement: BoardPlacement = {
    offset: { x: round2(offX), y: round2(offY), z: round2(standoffTop) },
    rotationDeg: 0,
  };
  // Holes in board-local coords; keep only those inside the outline.
  const mountingHoles: MountingHole[] = [];
  for (const s of standoffs) {
    const lx = round2(s.x - offX);
    const ly = round2(s.y - offY);
    if (lx >= 0 && lx <= w && ly >= 0 && ly <= h) {
      mountingHoles.push({ x: lx, y: ly, diameter: holeDia });
    }
  }
  return { outline, mountingHoles, placement };
}
