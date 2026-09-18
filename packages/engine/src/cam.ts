/**
 * The typed client for the kernel's CAM surface.
 *
 * Every function here is a thin, *typed* shell over one `vcad-cam-api` entry
 * point reached through the kernel WASM — the same request schema the native
 * app posts through its C ABI and an agent posts through MCP. There is
 * deliberately no machining logic on this side: a second implementation of
 * "is this job safe to run?" would be a second answer, and the whole point of
 * the verification oracle is that there is exactly one.
 *
 * Field names below are the Rust ones, verbatim (`feed_mm_min`, not
 * `feedMmMin`). They are wire names, and renaming them here would mean this
 * file has to be edited every time the schema moves, which is how the
 * granular bindings drifted in the first place.
 *
 * ## Two things the raw bindings get wrong, and this file fixes
 *
 * 1. **`{"error": …}` is a document, not an exception.** Every entry point
 *    answers one shape so callers have one decode path, which is right for
 *    the wire and wrong for a caller: it makes "the request was malformed"
 *    look exactly like a result until somebody remembers to check. Here a
 *    malformed request throws {@link CamError}.
 *
 * 2. **A blocked job has no `gcode` key at all.** That is the fail-closed
 *    contract, and the types say so: {@link CamJobResult} is a discriminated
 *    union on `blocked`, and the blocked arm types `gcode` as `never`, so
 *    `job.gcode` does not compile until the caller has narrowed. An export
 *    button wired straight to `result.gcode` cannot be written.
 *
 * ## Where `error` is *not* a failure
 *
 * Two answers carry an `error` string and are still results, and the typed
 * shapes keep them:
 *
 * - a **blocked job** (`blocked: true` + `error`): the sentence is the
 *   refusal, and the job report around it — tool checks, notes, policy — is
 *   what the operator has to read;
 * - a **torn section** (`error` + `gaps`): the solid does not close at that Z,
 *   and the gap positions are the information. A thrown string would destroy
 *   them, and healing the outline would machine something that is not the
 *   part. {@link CamOutlineResult} discriminates on `torn`.
 */

import type { Document } from "@vcad/ir";

// ---------------------------------------------------------------------------
// Host
// ---------------------------------------------------------------------------

/**
 * What this client needs from the kernel. `Engine` satisfies it structurally,
 * so the app passes its engine and the tests pass a stub.
 */
export interface CamHost {
  camJob<T = Record<string, unknown>>(request: unknown): T;
  camVerifyGcode<T = Record<string, unknown>>(request: unknown): T;
  camFit<T = Record<string, unknown>>(request: unknown): T;
  camMaterials<T = Record<string, unknown>>(): T;
  camRecommendFeeds<T = Record<string, unknown>>(request: unknown): T;
  camCheckFeeds<T = Record<string, unknown>>(request: unknown): T;
  camGear<T = Record<string, unknown>>(request: unknown): T;
  camOutlineFromDocument<T = Record<string, unknown>>(
    doc: unknown,
    partIndex: number,
    z: number,
    autoZ: boolean,
    options?: unknown,
  ): T;
}

/**
 * A CAM call that could not be answered at all: a malformed request, a number
 * the kernel refused by name, a binding missing from the WASM build.
 *
 * This is *not* how a refused job arrives. A job the oracle rejects is a
 * successful call whose answer is "no" — see {@link CamJobBlocked}.
 */
export class CamError extends Error {
  /** Which entry point answered, e.g. `"cam_job"`. */
  readonly call: string;

  constructor(call: string, message: string) {
    super(message);
    this.name = "CamError";
    this.call = call;
  }
}

// ---------------------------------------------------------------------------
// Shared report shapes (mirrors of vcad_kernel_cam::verify2d)
// ---------------------------------------------------------------------------

/** Whether a failed check blocks the job or only warns. */
export type CamSeverity = "Error" | "Warning";

/** One offending place, with enough detail to find it in the program. */
export interface CamViolation {
  /** G-code line number, or toolpath segment index. */
  index: number;
  /** Where in the stock frame (mm). */
  xy: [number, number];
  /** Z there (mm). */
  z: number;
  [key: string]: unknown;
}

/** One of the oracle's checks. */
export interface CamCheckReport {
  /** Short name, e.g. `"gouge"`. This is what `policy.blocked_by` lists. */
  name: string;
  /** True when nothing was found. */
  pass: boolean;
  severity: CamSeverity;
  violation_count: number;
  /** Worst value seen (mm, or mm/min); 0 when clean. */
  worst: number;
  examples: CamViolation[];
  /** What was measured, and against what. */
  note: string;
}

/** Wall the full-depth passes never reached. */
export interface CamMaterialLeftReport {
  check: CamCheckReport;
  unswept_area: number;
  max_standoff: number;
  reachable_band_area: number;
  untouched_walls: number;
  walls: number;
  tab_area: number;
}

/** Depth against the stock. */
export interface CamDepthReport {
  check: CamCheckReport;
  deepest_z: number;
  floor_z: number;
  /** Negative means the cutter went past the stock underside. */
  remaining_under_part: number;
  features: number;
}

/** Tabs, as the moves actually cut them. */
export interface CamTabAudit {
  check: CamCheckReport;
  observations: Array<Record<string, unknown>>;
  tab_count: number;
  passes_below_tabs: number;
}

/** Swept extents, and whether the machine can reach them. */
export interface CamEnvelopeReport {
  check: CamCheckReport;
  work_min: [number, number, number];
  work_max: [number, number, number];
  machine_min: [number, number, number] | null;
  machine_max: [number, number, number] | null;
  /** Stock the job needs around the part, `[-X, -Y, +X, +Y]` (mm). */
  stock_margin: [number, number, number, number];
}

/** Stock the job frees completely. */
export interface CamLoosePiecesReport {
  check: CamCheckReport;
  pieces: Array<Record<string, unknown>>;
  frame_pieces: Array<Record<string, unknown>>;
  /** True when the job never breaks through, so nothing can come free. */
  skin_holds: boolean;
}

/** The whole answer: one job, eight checks. */
export interface CamVerification {
  /** False when any error-level check failed. */
  pass: boolean;
  gouge: CamCheckReport;
  material_left: CamMaterialLeftReport;
  rapids: CamCheckReport;
  depth: CamDepthReport;
  tabs: CamTabAudit;
  envelope: CamEnvelopeReport;
  loose: CamLoosePiecesReport;
  plunges: CamCheckReport;
  /** How many moves were replayed (after arcs were sampled). */
  moves: number;
}

/**
 * Every check of a verification, in one list, reached the same way whether the
 * kernel nests it under `.check` or not.
 *
 * The oracle reports three checks bare (`gouge`, `rapids`, `plunges`) and five
 * wrapped in a richer report. A UI that walks them by hand gets that wrong
 * once and then silently shows five checks instead of eight.
 */
export function camChecks(v: CamVerification | undefined): CamCheckReport[] {
  if (!v) return [];
  const out = [
    v.gouge,
    v.material_left?.check,
    v.rapids,
    v.depth?.check,
    v.tabs?.check,
    v.envelope?.check,
    v.loose?.check,
    v.plunges,
  ];
  return out.filter((c): c is CamCheckReport => !!c && typeof c.name === "string");
}

/** A note the job attached to itself. */
export interface CamNote {
  level: "Info" | "Caution" | "Warning" | "Danger" | string;
  text: string;
}

// ---------------------------------------------------------------------------
// cam_job
// ---------------------------------------------------------------------------

/** `stock { thickness, margin | bbox, spoilboard? }`. Units are mm. */
export interface CamStockRequest {
  thickness: number;
  margin?: number;
  /** `[min_x, min_y, max_x, max_y]`. */
  bbox?: [number, number, number, number];
  /** Sacrificial board under the stock. Required before any cut goes past
   *  the underside. */
  spoilboard?: number;
}

/** `machine { name?, travel?, work_offset?, max_feed?, max_accel?, spindle, class }`. */
export interface CamMachineRequest {
  name?: string;
  travel?: { min: [number, number, number]; max: [number, number, number] };
  work_offset?: [number, number, number];
  max_feed?: number;
  max_accel?: number;
  /** `"dial"` — a trim router whose `S` word does nothing — or `"controlled"`. */
  spindle?: "dial" | "controlled";
  class?: "hobby" | "benchtop" | "vmc";
}

/** One entry of the job's tool library. */
export interface CamToolRequest {
  /** `T<number>`, as the program will say it. */
  number: number;
  name?: string;
  kind?:
    | "flat_end_mill"
    | "ball_end_mill"
    | "bull_end_mill"
    | "v_bit"
    | "drill"
    | "face_mill";
  diameter: number;
  flutes?: number;
  /** Usable cutting length, mm. Defaults to three diameters. */
  flute_length?: number;
  stickout?: number;
  shank_diameter?: number;
  centre_cutting?: boolean;
  holder?: { diameter: number; length: number; taper_angle?: number };
  corner_radius?: number;
  angle?: number;
}

/** The six things a 2.5D job can ask the cutter to do. */
export type CamOperationKind =
  | "face"
  | "pocket"
  | "contour_outside"
  | "contour_inside"
  | "drill"
  | "helical_bore";

/** A hole centre, as the outline tools hand them back or with its own depth. */
export type CamHoleRequest =
  | [number, number]
  | { x: number; y: number; depth?: number };

/** One operation of a job. */
export interface CamOperationRequest {
  name?: string;
  /** The tool number, which must be in `tools`. */
  tool: number;
  kind: CamOperationKind;

  /** Closed polyline in the stock frame, for a shaped pocket or a contour. */
  contour?: Array<[number, number]>;
  /** Material a pocket clears around and keeps. Only a pocket has these. */
  islands?: Array<Array<[number, number]>>;
  holes?: CamHoleRequest[];
  /** `[x0, y0, x1, y1]`, for `face` and a rectangular `pocket`. */
  rectangle?: [number, number, number, number];
  /** Bore centre, for `helical_bore`. */
  x?: number;
  y?: number;
  /** Bore diameter, which has to be larger than the cutter. */
  diameter?: number;
  pitch?: number;

  depth: number;
  stepdown: number;
  stepover?: number;
  feed: number;
  plunge: number;
  rpm: number;

  direction?: "climb" | "conventional";
  entry?: string;
  ramp_angle?: number;
  lead_in?: boolean;
  lead_radius?: number;
  offset?: number;
  stock_to_leave?: number;
  finish_stepdowns?: number;
  spring_pass?: boolean;
  finish_feed?: number;
  finish_pass?: boolean;

  tabs?: number;
  tab_positions?: number[];
  tab_width?: number;
  tab_height?: number;

  placement?: { dx?: number; dy?: number; rotation_deg?: number };

  /** Onion skin. Negative breaks through into a declared spoilboard. */
  bottom_allowance?: number;
  thin_slot?: { strategy: "refuse" | "centre_line"; tolerance?: number };

  cycle?: "spot" | "straight" | "peck" | "chip_break";
  peck_depth?: number;
  retreat?: number;
  dwell?: number;
  clearance?: number;
  peck_clearance?: number;
  through?: boolean;

  order?: number;
  role?: "facing" | "inside_feature" | "outside_profile";
  /** Lower phases run first. Give a phase rather than lying about the role. */
  phase?: number;
}

/** The part the job is meant to make, for the oracle to check against. */
export interface CamPartRequest {
  outer: Array<[number, number]>;
  holes?: Array<Array<[number, number]>>;
}

/** Job-wide options. */
export interface CamJobOptions {
  arc_fit?: { tolerance: number };
  tool_change?:
    | { type: "m6" }
    | { type: "manual_pause_reprobe"; probe_macro?: string | null };
  spin_up_seconds?: number;
  park_z?: number;
  safe_z?: number;
  wcs?: string;
  end?: string;
  post?: "grbl" | "linuxcnc";
  /** Defaults to true. Turning it off is how an unverified job happens. */
  verify?: boolean;
  part?: CamPartRequest;
  placement?: { dx?: number; dy?: number; rotation_deg?: number };
}

/** A whole job request. */
export interface CamJobRequest {
  name?: string;
  stock: CamStockRequest;
  machine?: CamMachineRequest;
  tools: CamToolRequest[];
  operations: CamOperationRequest[];
  options?: CamJobOptions;
  /** Per-check severity override, e.g. `{"material_left": "warning"}`. */
  verify_policy?: Record<string, "error" | "warning">;
}

/** One move of the preview polyline. */
export interface CamMove {
  to: [number, number, number];
  rapid: boolean;
  /** mm/min; absent on a rapid. */
  feed?: number;
}

/** A block of the program, as a range of `moves`. The ranges partition it. */
export interface CamOpRange {
  block: "preamble" | "tool_start" | "tool_change" | "operation" | "postamble";
  name?: string;
  op_index?: number;
  tool?: number;
  /** First index into `moves`. */
  start: number;
  /** One past the last index into `moves`. */
  end: number;
  /** Acceleration-aware duration of this block, seconds. */
  seconds: number;
}

/** A tool-geometry finding against one operation. */
export interface CamToolCheck {
  op: string;
  op_index: number;
  tool: number;
  severity: "error" | "warning";
  kind: string;
  message: string;
}

/** What the oracle decided, and what the caller asked it to decide. */
export interface CamPolicy {
  verified: boolean;
  /** `"gcode"` or `"toolpath"`; absent when verification was off. */
  replayed?: "gcode" | "toolpath";
  /** Names of the checks that block. Empty means nothing does. */
  blocked_by: string[];
  /** Names of the checks that failed but only warn. */
  warnings: string[];
}

/** What every job answer carries, refused or not. */
interface CamJobCommon {
  name?: string;
  policy: CamPolicy;
  notes: CamNote[];
  tool_checks?: CamToolCheck[];
  moves?: CamMove[];
  op_ranges?: CamOpRange[];
  duration?: { naive_s: number; accel_aware_s: number };
  tool_sequence?: number[];
  fit?: Array<Record<string, unknown>>;
  report?: Array<Record<string, unknown>>;
  stock?: { thickness: number; spoilboard: number; underside_z: number };
  verification?: CamVerification;
  verification_by_tool?: Array<Record<string, unknown>>;
  arc_fit?: Record<string, unknown>;
  tab_placement?: Array<Record<string, unknown>>;
  island_clearance?: Array<Record<string, unknown>>;
}

/**
 * A job the oracle refused.
 *
 * `gcode` is `never`, not `undefined`: the key is absent from the document,
 * and typing it this way means an export path has to narrow on `blocked`
 * before it can even name the field.
 */
export interface CamJobBlocked extends CamJobCommon {
  blocked: true;
  gcode?: never;
  /** The refusal, in a sentence. Present on the early refusals; otherwise the
   *  reason is `policy.blocked_by` and the danger note. */
  error?: string;
}

/** A job that verified, with the program to run. */
export interface CamJobPassed extends CamJobCommon {
  blocked: false;
  gcode: string;
}

/** A job answer: refused, or passed with a program. Narrow on `blocked`. */
export type CamJobResult = CamJobBlocked | CamJobPassed;

/**
 * Build, post and verify a whole job.
 *
 * Throws {@link CamError} when the request could not be read at all. A job
 * the oracle refuses is not an error: it comes back `blocked: true` with the
 * checks that failed and no G-code anywhere in the answer.
 */
export function camJob(host: CamHost, request: CamJobRequest): CamJobResult {
  const out = host.camJob<Record<string, unknown>>(request);
  // The one carve-out in the whole surface: a refused job may carry both
  // `blocked` and `error`, and there the sentence is the refusal rather than
  // a failure to answer.
  if (typeof out.error === "string" && out.blocked !== true) {
    throw new CamError("cam_job", out.error);
  }
  if (typeof out.blocked !== "boolean") {
    throw new CamError(
      "cam_job",
      "the kernel answered a job with no `blocked` verdict, so there is no way to tell whether it is safe to run. Rebuild packages/kernel-wasm.",
    );
  }
  if (out.blocked === false && typeof out.gcode !== "string") {
    throw new CamError(
      "cam_job",
      "the job was not blocked but no G-code came back. This is a bug in the CAM surface, not in the job.",
    );
  }
  if (out.blocked === true && "gcode" in out) {
    // Fail closed on the fail-closed contract itself: if this ever fires, the
    // kernel handed an export path a program it had just refused.
    throw new CamError(
      "cam_job",
      "the job was blocked but carried G-code anyway. Refusing it here rather than letting it reach an export button.",
    );
  }
  return out as unknown as CamJobResult;
}

// ---------------------------------------------------------------------------
// cam_verify_gcode
// ---------------------------------------------------------------------------

/** Verify arbitrary G-code text against the part it is meant to make. */
export interface CamVerifyRequest {
  gcode: string;
  part: CamPartRequest;
  stock: CamStockRequest;
  tool_diameter: number;
  bottom_allowance?: number;
  machine?: CamMachineRequest;
  tabs?: Array<{ width: number; height: number }>;
  centre_cutting?: boolean;
  tolerance?: number;
}

/** The verdict, plus the eight checks behind it. */
export interface CamVerifyResult {
  pass: boolean;
  blocked: boolean;
  policy: { verified: boolean; blocked_by: string[] };
  verification: CamVerification;
}

/** Is *this* program the one that makes *this* part? */
export function camVerifyGcode(
  host: CamHost,
  request: CamVerifyRequest,
): CamVerifyResult {
  return unwrap("cam_verify_gcode", host.camVerifyGcode(request));
}

// ---------------------------------------------------------------------------
// cam_fit
// ---------------------------------------------------------------------------

/** Cutter-fit request for one contour, one tool and one side. */
export interface CamFitRequest {
  contour: Array<[number, number]>;
  tool_diameter: number;
  side?: "inside" | "outside";
  grid?: number;
  min_area?: number;
}

/** Corners this cutter cannot reach, and the largest one that still fits. */
export interface CamFitResult {
  fits: boolean;
  side: "inside" | "outside";
  /** The number that answers "so what should I use?". */
  largest_tool_diameter: number;
  report: {
    fits: boolean;
    unreachable: { count: number; total_area: number; max_standoff: number };
    necks: Array<Record<string, unknown>>;
    [key: string]: unknown;
  };
}

/** Does this cutter fit this contour? */
export function camFit(host: CamHost, request: CamFitRequest): CamFitResult {
  return unwrap("cam_fit", host.camFit(request));
}

// ---------------------------------------------------------------------------
// cam_outline_from_document
// ---------------------------------------------------------------------------

/** Tolerances for sectioning a solid. */
export interface CamSectionOptions {
  weld_tolerance?: number;
  heal_tolerance?: number;
  prismatic?: boolean;
  prismatic_tolerance?: number;
  circle_tolerance?: number;
  segments?: number;
}

/** One closed area of the section: a boundary and the openings inside it. */
export interface CamOutlineRegion {
  outer: Array<[number, number]>;
  holes: Array<Array<[number, number]>>;
  area: number;
}

/** An opening that is round enough to be called a hole. */
export interface CamOutlineCircle {
  region: number;
  hole: number;
  center: [number, number];
  diameter: number;
  rms_error: number;
  max_error: number;
}

/** Where the section failed to close. */
export interface CamOutlineGap {
  distance: number;
  [key: string]: unknown;
}

/**
 * A section that does not close: the solid is torn at that Z.
 *
 * This is a result, not a failure — the gaps say where. Healing the outline
 * would hand the cutter a wall the part does not have, so the panel shows the
 * refusal and the gaps and stops.
 */
export interface CamOutlineTorn {
  torn: true;
  /** The refusal, in a sentence. */
  error: string;
  gaps: CamOutlineGap[];
  z: number;
  mesh_source: string;
}

/** A section that closed. */
export interface CamOutlineSectioned {
  torn: false;
  /** `"raw_tessellation"` (a B-rep was reached), `"stored_mesh"` or
   *  `"export_mesh"` — the repaired mesh, which can tear by 0.4 mm. */
  mesh_source: "raw_tessellation" | "stored_mesh" | "export_mesh" | string;
  z: number;
  auto_z: boolean;
  z_range: [number, number];
  /** What the stock has to be at least, for this part to come out of it. */
  suggested_stock_thickness: number;
  plane_nudge: number;
  healed: number;
  discarded_slivers: number;
  regions: CamOutlineRegion[];
  circles: CamOutlineCircle[];
  bounds: [number, number, number, number];
  area: number;
  prismatic: { prismatic?: boolean; refused?: string; [k: string]: unknown } | null;
  outline: Record<string, unknown>;
}

/** A section: torn, or closed. Narrow on `torn`. */
export type CamOutlineResult = CamOutlineTorn | CamOutlineSectioned;

/**
 * A machining contour out of a document part.
 *
 * Pass `"auto"` for `z` to section at the mid-height of the part's own bounds,
 * which is what a constant-section part wants and what the panel uses.
 *
 * The document goes into the kernel whole rather than being evaluated here
 * first, and that is the point: only the kernel still holds the B-rep, and
 * only its **raw** tessellation sections cleanly.
 */
export function camOutlineFromDocument(
  host: CamHost,
  doc: Document,
  partIndex: number,
  z: number | "auto",
  options: CamSectionOptions = {},
): CamOutlineResult {
  const auto = z === "auto";
  const out = host.camOutlineFromDocument<Record<string, unknown>>(
    doc,
    partIndex,
    auto ? 0 : z,
    auto,
    options,
  );
  if (typeof out.error === "string") {
    // A torn section carries its gaps; anything else is a call that failed.
    if (Array.isArray(out.gaps)) {
      return {
        torn: true,
        error: out.error,
        gaps: out.gaps as CamOutlineGap[],
        z: typeof out.z === "number" ? out.z : NaN,
        mesh_source: String(out.mesh_source ?? "unknown"),
      };
    }
    throw new CamError("cam_outline_from_document", out.error);
  }
  return { torn: false, ...(out as unknown as Omit<CamOutlineSectioned, "torn">) };
}

// ---------------------------------------------------------------------------
// cam_materials / cam_recommend / cam_check_feeds
// ---------------------------------------------------------------------------

/** How loudly a note should be shown. */
export type CamNoteLevel = "Info" | "Caution" | "Warning" | "Danger";

/** A single piece of advice attached to a recommendation or a check. */
export interface CamFeedNote {
  level: CamNoteLevel | string;
  text: string;
}

/** One material of the table. */
export interface CamMaterial {
  /** Stable identifier, e.g. `"aluminium-6061-t6"`. Never renamed. */
  id: string;
  name: string;
  family: string;
  difficulty_rank: number;
  coolant: string;
  router_class_ok: boolean;
  [key: string]: unknown;
}

/** The material table and the vocabularies that go with it. */
export interface CamMaterialsResult {
  materials: CamMaterial[];
  operations: CamFeedsOp[];
  machine_classes: Array<"hobby" | "benchtop" | "vmc">;
  spindles: Array<"dial" | "controlled">;
}

/** What the cut is doing to the material, which is what sets the engagement. */
export type CamFeedsOp = "slot" | "profile" | "pocket" | "drill" | "finish";

/** `{ material, op, tool, machine }`. */
export interface CamRecommendRequest {
  material: string;
  op?: CamFeedsOp;
  tool: {
    diameter: number;
    flutes?: number;
    kind?: string;
    flute_length?: number;
  };
  machine?: CamMachineRequest;
}

/** Feeds, speeds, step sizes and the dial to set by hand. */
export interface CamRecommendation {
  material_id: string;
  op: string;
  tool_diameter_mm: number;
  flutes: number;
  rpm: number;
  /**
   * The dial position to set by hand. When this is present the `S` word in
   * the G-code does nothing, which is the whole reason it exists.
   */
  dial: string | null;
  surface_speed_m_min: number;
  chipload_mm: number;
  feed_mm_min: number;
  plunge_mm_min: number;
  ramp_angle_deg: number;
  stepdown_mm: number;
  stepover_mm: number;
  finish_allowance_mm: number;
  coolant: string;
  notes: CamFeedNote[];
}

/** A recommendation, with what it was computed for. */
export interface CamRecommendResult {
  material: { id: string; name: string; coolant: string };
  machine: { name: string; class: string };
  /** True when the `S` word does nothing and `recommendation.dial` is the
   *  only speed setting that matters. */
  dial_spindle: boolean;
  recommendation: CamRecommendation;
}

/** `{ material, op, tool, machine, settings }`. */
export interface CamCheckFeedsRequest extends CamRecommendRequest {
  settings: {
    feed: number;
    plunge: number;
    rpm: number;
    stepdown: number;
    stepover?: number;
  };
}

/** A second opinion on numbers the operator already has. */
export interface CamCheckFeedsResult {
  material: { id: string; name: string };
  worst_level: CamNoteLevel | null;
  /** Nothing above a caution. */
  ok: boolean;
  notes: CamFeedNote[];
}

/** The material table, with the hazards attached to each entry. */
export function camMaterials(host: CamHost): CamMaterialsResult {
  return unwrap("cam_materials", host.camMaterials());
}

/** Feeds, speeds, stepdown, stepover and the router dial to set. */
export function camRecommend(
  host: CamHost,
  request: CamRecommendRequest,
): CamRecommendResult {
  return unwrap("cam_recommend", host.camRecommendFeeds(request));
}

/** Check feeds and speeds the caller already has against the material. */
export function camCheckFeeds(
  host: CamHost,
  request: CamCheckFeedsRequest,
): CamCheckFeedsResult {
  return unwrap("cam_check_feeds", host.camCheckFeeds(request));
}

// ---------------------------------------------------------------------------
// cam_gear
// ---------------------------------------------------------------------------

/** One gear, in the terms it is cut in. */
export interface CamGearSpec {
  module: number;
  teeth: number;
  pressure_angle?: number;
  profile_shift?: number;
  backlash_thinning?: number;
  internal?: boolean;
  face_width?: number;
}

/** `{ gear, cutter_diameter, pin_diameter, measured, span, contours, … }`. */
export interface CamGearRequest {
  gear: CamGearSpec;
  cutter_diameter?: number;
  pin_diameter?: number;
  /** An over-pins reading off the part, for the compensation. */
  measured?: number;
  span?: boolean;
  contours?: boolean;
  tooth?: number;
  chordal_tolerance?: number;
  flank_tolerance?: number;
  planetary?: {
    sun: CamGearSpec;
    planet: CamGearSpec;
    ring: CamGearSpec;
    planets?: number;
  };
}

/** Gear geometry, over-pins measurement and measured compensation. */
export interface CamGearResult {
  contours?: {
    tooth: unknown;
    tooth_space: Array<[number, number]>;
    full_profile: Array<[number, number]>;
    tool_centre_path: Array<[number, number]>;
  };
  [key: string]: unknown;
}

/** Gear geometry: report, contours, over-pins, compensation. */
export function camGear(host: CamHost, request: CamGearRequest): CamGearResult {
  return unwrap("cam_gear", host.camGear(request));
}

// ---------------------------------------------------------------------------
// Plumbing
// ---------------------------------------------------------------------------

/**
 * Turn the shared `{"error": …}` document into a thrown {@link CamError}.
 *
 * Only for the entry points where an `error` key really is a failure —
 * `cam_job` and `cam_outline_from_document` handle their own, because for
 * them it is not.
 */
function unwrap<T>(call: string, out: unknown): T {
  const doc = out as Record<string, unknown>;
  if (doc && typeof doc.error === "string") {
    throw new CamError(call, doc.error);
  }
  return out as T;
}

/**
 * The whole surface bound to one host, for callers that would otherwise pass
 * the same engine eight times.
 */
export function camClient(host: CamHost) {
  return {
    job: (request: CamJobRequest) => camJob(host, request),
    verifyGcode: (request: CamVerifyRequest) => camVerifyGcode(host, request),
    fit: (request: CamFitRequest) => camFit(host, request),
    outlineFromDocument: (
      doc: Document,
      partIndex: number,
      z: number | "auto",
      options?: CamSectionOptions,
    ) => camOutlineFromDocument(host, doc, partIndex, z, options),
    materials: () => camMaterials(host),
    recommend: (request: CamRecommendRequest) => camRecommend(host, request),
    checkFeeds: (request: CamCheckFeedsRequest) => camCheckFeeds(host, request),
    gear: (request: CamGearRequest) => camGear(host, request),
  };
}

/** The whole surface bound to one host. */
export type CamClient = ReturnType<typeof camClient>;
