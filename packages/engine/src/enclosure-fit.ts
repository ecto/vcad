/**
 * Cross-domain PCB ↔ enclosure verification — thin wrapper around the Rust
 * kernel.
 *
 * vcad is the only stack with both a real BRep CAD kernel and a PCB engine, so
 * it can do something no EDA tool can: cross-check a board against the physical
 * case it lives in. This module answers four questions a fab house can't:
 *
 *   1. Does the board fit the cavity with clearance?
 *   2. Do tall components clear the lid? (stack height vs cavity depth)
 *   3. Do the mounting holes land on the case standoffs?
 *   4. Do edge connectors line up with the wall cutouts?
 *
 * The implementation lives in `crates/vcad-kernel-enclosure` (Rust is the
 * single source of truth); this file is the JS-facing surface. The mesh →
 * cavity/standoff/opening extraction is in `./enclosure-mesh.js`.
 *
 * Frames: enclosure features are in **enclosure-world** (Z-up, mm). A board is
 * authored in its own **board-local** frame (origin-corner outline, board
 * bottom at z=0, top at z=thickness). A {@link BoardPlacement} maps board-local
 * → world (Z-rotation then translation), the same convention the renderer and
 * `getPcbBoardTransform` use.
 *
 * All entry points are synchronous and require the kernel WASM singleton to be
 * initialized (`await getKernelWasm()` / `Engine.init()`) — they throw
 * otherwise, rather than silently returning a wrong verdict.
 */

import type { BoardOutline, Pcb, Vec2, Vec3 } from "@vcad/ir";
import { getKernelWasmSync } from "./wasm-singleton.js";

// ===========================================================================
// Types — the wire shapes the Rust crate serializes (see
// crates/vcad-kernel-enclosure/src/fit.rs).
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

/** Input to the verification core. */
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

// ===========================================================================
// Kernel bridge
// ===========================================================================

/** The enclosure entry points on the kernel WASM module. */
type EnclosureWasm = {
  enclosure_features(positions: Float64Array, indices: Uint32Array): string;
  enclosure_fit(inputJson: string): string;
  enclosure_derive_board(cavityJson: string, standoffsJson: string, optionsJson: string): string;
  enclosure_mounting_holes(pcbJson: string): string;
  enclosure_connectors(pcbJson: string, outlineJson: string): string;
  enclosure_component_extents(meshesJson: string, pcbJson: string): string;
  enclosure_outline_aabb(outlineJson: string): string;
  enclosure_to_world(x: number, y: number, z: number, placementJson: string): string;
};

/**
 * The initialized kernel module, narrowed to the enclosure exports.
 *
 * Throws rather than degrading: every caller here produces a verification
 * verdict, and a silent fallback would report "fits" for a board nobody
 * checked.
 */
export function enclosureWasm(): EnclosureWasm {
  const wasm = getKernelWasmSync() as unknown as EnclosureWasm | null;
  if (!wasm?.enclosure_fit) {
    throw new Error(
      "enclosure checks require the kernel WASM to be initialized — await getKernelWasm() (or Engine.init()) first",
    );
  }
  return wasm;
}

/** Drop `undefined` fields so Rust's `Option` deserialization sees them absent. */
function compact<T extends object>(obj: T): T {
  return Object.fromEntries(Object.entries(obj).filter(([, v]) => v !== undefined)) as T;
}

// ===========================================================================
// Geometry helpers
// ===========================================================================

/** Axis-aligned bounds of a polygon. */
export function outlineAabb(outline: BoardOutline): {
  minX: number;
  maxX: number;
  minY: number;
  maxY: number;
} {
  return JSON.parse(enclosureWasm().enclosure_outline_aabb(JSON.stringify(outline)));
}

/** Map a board-local point into the enclosure-world frame. */
export function toWorld(p: { x: number; y: number; z?: number }, placement: BoardPlacement): Vec3 {
  return JSON.parse(
    enclosureWasm().enclosure_to_world(p.x, p.y, p.z ?? 0, JSON.stringify(placement)),
  );
}

// ===========================================================================
// Board feature extraction (Pcb → board-local features)
// ===========================================================================

/**
 * Mounting holes the board declares, in board-local coords. Sourced from
 * MountingHole footprints (their origin) and from any NPTH pad (its drilled
 * position). Diameter comes from the drill spec, else the pad/footprint size.
 */
export function mountingHolesFromPcb(pcb: Pcb): MountingHole[] {
  return JSON.parse(enclosureWasm().enclosure_mounting_holes(JSON.stringify(pcb)));
}

/**
 * Map kernel component meshes (board-local, board bottom z=0) to per-component
 * Z extents. Structurally typed so the engine's `ComponentMesh` and the
 * verification core stay decoupled. `front` comes from the matching footprint
 * (default front), and decides whether the part rises toward the lid or dips
 * toward the floor.
 */
export function componentExtentsFromMeshes(
  meshes: Array<{ footprint_ref: string; positions: ArrayLike<number> }>,
  pcb: Pcb,
): ComponentExtent[] {
  const payload = meshes.map((m) => ({
    footprint_ref: m.footprint_ref,
    positions: Array.from(m.positions),
  }));
  return JSON.parse(
    enclosureWasm().enclosure_component_extents(JSON.stringify(payload), JSON.stringify(pcb)),
  );
}

/**
 * Edge connectors the board declares, in board-local coords, each tagged with
 * the nearest board edge (so the cutout check knows which wall to look at).
 */
export function connectorsFromPcb(pcb: Pcb, outline: BoardOutline): ConnectorRef[] {
  return JSON.parse(
    enclosureWasm().enclosure_connectors(JSON.stringify(pcb), JSON.stringify(outline)),
  );
}

// ===========================================================================
// Verification core
// ===========================================================================

/**
 * Run the four cross-domain checks and assemble the verdict. Pass it extracted
 * features and it returns a report — no I/O beyond the kernel call.
 */
export function checkEnclosureFit(input: EnclosureFitInput): EnclosureFitReport {
  return JSON.parse(enclosureWasm().enclosure_fit(JSON.stringify(compact(input))));
}

// ===========================================================================
// Auto-derive a board from the cavity (the co-design starting point)
// ===========================================================================

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
  return JSON.parse(
    enclosureWasm().enclosure_derive_board(
      JSON.stringify(cavity),
      JSON.stringify(standoffs),
      JSON.stringify(compact(opts)),
    ),
  );
}
