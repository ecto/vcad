/**
 * ECAD (Electronics) WASM wrappers.
 *
 * Lazy-loads the kernel WASM and provides typed wrappers for DRC, ERC,
 * netlist generation, routing, and zone fill.
 */

import type {
  SchematicSheet,
  Pcb,
  Vec2,
  PcbLayer,
  DerivedPart,
  Receipt,
  ReceiptStatus,
} from "@vcad/ir";

// ---------------------------------------------------------------------------
// Result types (mirrors Rust serde output)
// ---------------------------------------------------------------------------

export type DrcRuleType =
  | "Clearance"
  | "MinTraceWidth"
  | "MinDrill"
  | "AnnularRing"
  | "EdgeClearance"
  | "HoleToHole"
  | "UnconnectedNet"
  | "SilkscreenClearance"
  | "CourtyardOverlap"
  | "AcidTrap"
  | "Keepout"
  | "Short"
  | "NetIslands";

export type DrcSeverity = "Error" | "Warning";

export interface DrcViolationResult {
  rule: DrcRuleType;
  severity: DrcSeverity;
  position: Vec2;
  message: string;
  actual: number;
  required: number;
}

export type ErcSeverity = "Error" | "Warning";

export interface ErcViolationResult {
  severity: ErcSeverity;
  message: string;
  position: Vec2 | null;
}

export interface NetConnection {
  component_ref: string;
  pin_number: string;
}

export interface NetlistNet {
  name: string;
  connections: NetConnection[];
}

export interface NetlistResult {
  nets: NetlistNet[];
}

export interface RouteResult {
  net: string;
  segments: [Vec2, Vec2][];
  vias: Vec2[];
  success: boolean;
}

export interface FilledZoneResult {
  polygons: Vec2[][];
  net: string;
  layer: PcbLayer;
}

// ---------------------------------------------------------------------------
// Three-state verification outcome
// ---------------------------------------------------------------------------

/**
 * Outcome of a kernel verification wrapper (DRC / ERC / route critique).
 *
 * The kernel can fail to even *deserialize* its input — e.g. a malformed layer
 * name like `"In1.Cu"` (it must be `"In1Cu"`). When that happens the board was
 * never actually checked, so reporting "0 violations / clean" is a dangerous
 * false-clean a caller could ship on. This type forces callers to branch on
 * three distinct states instead of collapsing the error into an empty result:
 *
 * - `{ status: "ok", value }`  — the kernel ran. `value` may itself be an empty
 *   list (genuinely clean) or carry violations.
 * - `{ status: "errored", … }` — the kernel could not parse/run the input.
 *   NEVER treat this as clean. `offending_field` names the bad token when it can
 *   be recovered from the error message (e.g. the malformed layer).
 */
export type VerifyOutcome<T> =
  | { status: "ok"; value: T }
  | { status: "errored"; reason: string; offending_field?: string };

// ---------------------------------------------------------------------------
// Lazy WASM loader
// ---------------------------------------------------------------------------

// eslint-disable-next-line @typescript-eslint/no-explicit-any
let wasmModule: any = null;

async function loadEcadWasm(): Promise<typeof wasmModule | null> {
  if (wasmModule) return wasmModule;
  try {
    const wasm = await import("@vcad/kernel-wasm");
    if (typeof (wasm as Record<string, unknown>).isEcadAvailable !== "function") {
      return null;
    }
    wasmModule = wasm;
    return wasmModule;
  } catch {
    return null;
  }
}

/** serde's unknown-variant / unknown-field / missing-field errors name the
 *  offending token in backticks, e.g. ``unknown variant `In1.Cu`, expected one
 *  of …``. Pull it out so callers can point the user straight at the bad field. */
function offendingFieldFromError(message: string): string | undefined {
  const m =
    /unknown variant `([^`]+)`/.exec(message) ??
    /unknown field `([^`]+)`/.exec(message) ??
    /missing field `([^`]+)`/.exec(message);
  return m ? m[1] : undefined;
}

/**
 * Invoke a kernel WASM verification function, mapping a missing kernel or a
 * thrown deserialize/eval error into a distinct `errored` {@link VerifyOutcome}
 * — never a false-clean empty result. `label` names the check for logs and the
 * surfaced reason.
 */
async function verifyWithKernel<T>(
  label: string,
  call: (wasm: NonNullable<typeof wasmModule>) => T,
): Promise<VerifyOutcome<T>> {
  const wasm = await loadEcadWasm();
  if (!wasm) {
    return { status: "errored", reason: `${label} unavailable: kernel WASM not loaded` };
  }
  try {
    return { status: "ok", value: call(wasm) };
  } catch (e) {
    const reason = e instanceof Error ? e.message : String(e);
    console.warn(`[ECAD] ${label} failed:`, e);
    const offending = offendingFieldFromError(reason);
    return offending
      ? { status: "errored", reason, offending_field: offending }
      : { status: "errored", reason };
  }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/** Check if ECAD WASM features are available. */
export async function isEcadAvailable(): Promise<boolean> {
  const wasm = await loadEcadWasm();
  return wasm !== null;
}

/**
 * Run Design Rule Check on a PCB.
 *
 * Returns a three-state {@link VerifyOutcome}: `ok` (the kernel ran — the value
 * is clean when empty, or carries violations) vs `errored` (the kernel could
 * not parse the board, e.g. a malformed layer name). Crucially it never reports
 * a parse failure as a clean/empty result — that false-clean is exactly what a
 * caller could ship on.
 */
export async function runDrc(pcb: Pcb): Promise<VerifyOutcome<DrcViolationResult[]>> {
  return verifyWithKernel(
    "DRC",
    (wasm) => wasm.ecadCheckDrc(JSON.stringify(pcb)) as DrcViolationResult[],
  );
}

/** Read-only audit of one net's routing. */
export interface NetCritique {
  net: string;
  routed: boolean;
  routed_length_mm: number;
  segment_count: number;
  via_count: number;
  layers: string[];
  min_clearance_mm: number | null;
  required_clearance_mm: number;
  drc_issues: string[];
}

/**
 * Audit a single net's routing quality (length, vias, margin, DRC issues).
 *
 * Returns a three-state {@link VerifyOutcome}: `ok` with the critique, or
 * `errored` when the kernel could not parse the board (or isn't loaded). A
 * parse failure is never silently reported as "no critique".
 */
export async function critiqueRoute(
  pcb: Pcb,
  net: string,
): Promise<VerifyOutcome<NetCritique>> {
  return verifyWithKernel(
    "Route critique",
    (wasm) => wasm.ecadCritiqueRoute(JSON.stringify(pcb), net) as NetCritique,
  );
}

/**
 * Outcome of a kernel ERC run, kept fail-closed: `unavailable` (kernel WASM
 * not loaded) and `error` (kernel rejected the sheet) are distinct from `ok`
 * with an empty list (kernel ran and found nothing). A caller that only sees
 * `ok` can treat the schematic as verified; the other two mean "unverifiable",
 * never "clean".
 */
export type ErcOutcome =
  | { status: "ok"; violations: ErcViolationResult[] }
  | { status: "unavailable" }
  | { status: "error"; message: string };

/**
 * Run the kernel Electrical Rule Check, reporting whether it actually executed.
 *
 * Keeps "kernel not loaded" and "kernel rejected the sheet" distinct from
 * "kernel ran clean", so verification surfaces (and the pin-type/floating-power
 * rules in the MCP run_erc tool) can fail closed instead of presenting an
 * unevaluated schematic as passing. {@link runErc} adapts this to the shared
 * {@link VerifyOutcome} shape.
 */
export async function checkErc(sheet: SchematicSheet): Promise<ErcOutcome> {
  const wasm = await loadEcadWasm();
  if (!wasm) return { status: "unavailable" };
  try {
    const violations = wasm.ecadCheckErc(JSON.stringify(sheet)) as ErcViolationResult[];
    return { status: "ok", violations };
  } catch (e) {
    const message = e instanceof Error ? e.message : String(e);
    console.warn("[ECAD] ERC failed:", e);
    return { status: "error", message };
  }
}

/**
 * Run Electrical Rule Check on a schematic.
 *
 * Returns a three-state {@link VerifyOutcome} consistent with {@link runDrc} /
 * {@link critiqueRoute}: `ok` (clean when empty, or with violations) vs
 * `errored` when the kernel was unavailable or could not parse the schematic.
 * Never reports a parse failure as a clean/empty result. Delegates to
 * {@link checkErc}, folding its `unavailable`/`error` states onto `errored`.
 */
export async function runErc(
  sheet: SchematicSheet,
): Promise<VerifyOutcome<ErcViolationResult[]>> {
  const outcome = await checkErc(sheet);
  if (outcome.status === "ok") return { status: "ok", value: outcome.violations };
  const reason =
    outcome.status === "unavailable"
      ? "ERC unavailable: kernel WASM not loaded"
      : outcome.message;
  const offending =
    outcome.status === "error" ? offendingFieldFromError(outcome.message) : undefined;
  return offending
    ? { status: "errored", reason, offending_field: offending }
    : { status: "errored", reason };
}

/** Inputs for the analytical motor evaluator (mirrors `vcad_ecad_sim::MotorSpec`). */
export interface MotorSpecInput {
  polePairs: number;
  turnsPerPhase: number;
  windingFactor: number;
  innerRMm: number;
  outerRMm: number;
  phaseResistanceOhm: number;
  supplyVoltageV: number;
  airgapFluxTesla: number;
}

/** Headline analytical motor performance (mirrors `vcad_ecad_sim::MotorPerformance`). */
export interface MotorPerformanceResult {
  ktNmPerA: number;
  keVSPerRad: number;
  noLoadSpeedRadS: number;
  stallTorqueNm: number;
  curve: Array<{ speedRadS: number; torqueNm: number }>;
}

/** Inputs for the cored air-gap MEC model (mirrors `vcad_ecad_sim::AirGapSpec`). */
export interface AirGapSpecInput {
  remanenceTesla: number;
  magnetThicknessMm: number;
  recoilMuRel: number;
  airgapMm: number;
  magnetAreaMm2: number;
  gapAreaMm2: number;
  ironMuRel?: number | null;
  ironPathMm: number;
  ironAreaMm2: number;
}

/** Evaluate first-order analytical motor performance. Null if ECAD WASM is unavailable. */
export async function evaluateMotor(
  spec: MotorSpecInput,
): Promise<MotorPerformanceResult | null> {
  const wasm = await loadEcadWasm();
  if (!wasm) return null;
  try {
    const fn = (wasm as unknown as { ecadEvaluateMotor?: (s: string) => unknown })
      .ecadEvaluateMotor;
    if (typeof fn !== "function") return null;
    return fn(JSON.stringify(spec)) as MotorPerformanceResult;
  } catch (e) {
    console.warn("[ECAD] evaluateMotor failed:", e);
    return null;
  }
}

/** Air-gap flux density (tesla) via the MEC reluctance model. Null if ECAD WASM is unavailable. */
export async function airgapFluxDensity(spec: AirGapSpecInput): Promise<number | null> {
  const wasm = await loadEcadWasm();
  if (!wasm) return null;
  try {
    const fn = (wasm as unknown as { ecadAirgapFluxDensity?: (s: string) => number })
      .ecadAirgapFluxDensity;
    if (typeof fn !== "function") return null;
    return fn(JSON.stringify(spec));
  } catch (e) {
    console.warn("[ECAD] airgapFluxDensity failed:", e);
    return null;
  }
}

/** Generate a netlist from a schematic sheet. */
export async function generateNetlist(sheet: SchematicSheet): Promise<NetlistResult> {
  const wasm = await loadEcadWasm();
  if (!wasm) return { nets: [] };
  try {
    return wasm.ecadGenerateNetlist(JSON.stringify(sheet)) as NetlistResult;
  } catch (e) {
    console.warn("[ECAD] Netlist generation failed:", e);
    return { nets: [] };
  }
}

/** Route a net between two points on the PCB. */
export async function routeNet(
  pcb: Pcb,
  net: string,
  start: Vec2,
  end: Vec2,
  width: number,
): Promise<RouteResult> {
  const wasm = await loadEcadWasm();
  const fail: RouteResult = { net, segments: [], vias: [], success: false };
  if (!wasm) return fail;
  try {
    return wasm.ecadRouteNet(
      JSON.stringify(pcb),
      net,
      start.x,
      start.y,
      end.x,
      end.y,
      width,
    ) as RouteResult;
  } catch (e) {
    console.warn("[ECAD] Routing failed:", e);
    return fail;
  }
}

/**
 * Route a net with the push-and-shove router.
 *
 * Continuous-space counterpart to {@link routeNet}: it detours around existing
 * copper on *other* nets, producing cleaner diagonal paths than the grid/wave
 * router. Same result shape; returns a failed result if the kernel is
 * unavailable.
 */
export async function routeNetShove(
  pcb: Pcb,
  net: string,
  start: Vec2,
  end: Vec2,
  width: number,
): Promise<RouteResult> {
  const wasm = await loadEcadWasm();
  const fail: RouteResult = { net, segments: [], vias: [], success: false };
  if (!wasm) return fail;
  try {
    return wasm.ecadRouteNetShove(
      JSON.stringify(pcb),
      net,
      start.x,
      start.y,
      end.x,
      end.y,
      width,
    ) as RouteResult;
  } catch (e) {
    console.warn("[ECAD] Push-shove routing failed:", e);
    return fail;
  }
}

/**
 * Route a net with the avoiding A* maze router.
 *
 * Stronger avoidance than {@link routeNetShove}: it searches a grid and tests
 * every step against the exact clearance oracle, so the route clears *all*
 * copper on `layer` — traces, pads, and vias — not just other-net trace
 * bounding boxes. Every returned segment is clearance-legal by construction.
 * Same result shape; returns a failed result if the kernel is unavailable.
 */
export async function routeNetMaze(
  pcb: Pcb,
  net: string,
  start: Vec2,
  end: Vec2,
  width: number,
  layer = "FCu",
): Promise<RouteResult> {
  const wasm = await loadEcadWasm();
  const fail: RouteResult = { net, segments: [], vias: [], success: false };
  if (!wasm) return fail;
  try {
    return wasm.ecadRouteNetMaze(
      JSON.stringify(pcb),
      layer,
      net,
      start.x,
      start.y,
      end.x,
      end.y,
      width,
    ) as RouteResult;
  } catch (e) {
    console.warn("[ECAD] Maze routing failed:", e);
    return fail;
  }
}

/** A trace produced by the whole-board auto-router. */
export interface RoutedTrace {
  start: Vec2;
  end: Vec2;
  width: number;
  layer: string;
  net: string;
}

/** A transition via produced by the whole-board auto-router. */
export interface RoutedVia {
  position: Vec2;
  net: string;
}

/** Result of {@link routeAll}. */
export interface RouteAllResult {
  traces: RoutedTrace[];
  vias: RoutedVia[];
  routed_nets: string[];
  unrouted_nets: string[];
}

/**
 * Auto-route a whole board over the incremental clearance oracle.
 *
 * Routes every unrouted net against a single growing route session (so each net
 * avoids the ones before it), retrying on the back layer with transition vias
 * that are probed on both layers before being placed. Every returned trace and
 * via is clearance-legal; nets that cannot be routed legally are reported in
 * `unrouted_nets` rather than shipped as shorting copper. Returns an empty
 * result if the kernel is unavailable.
 */
export async function routeAll(
  pcb: Pcb,
  width: number,
  netsFilter: string[] = [],
): Promise<RouteAllResult> {
  const empty: RouteAllResult = { traces: [], vias: [], routed_nets: [], unrouted_nets: [] };
  const wasm = await loadEcadWasm();
  if (!wasm) return empty;
  try {
    return wasm.ecadRouteAll(
      JSON.stringify(pcb),
      width,
      JSON.stringify(netsFilter),
    ) as RouteAllResult;
  } catch (e) {
    console.warn("[ECAD] Auto-route failed:", e);
    return empty;
  }
}

/** Result of {@link routeDiffPair}: the two routed legs, or `success:false`. */
export interface DiffPairResult {
  success: boolean;
  p?: RouteResult;
  n?: RouteResult;
}

/**
 * Route a declared differential pair (P/N) coupled and length-matched. Gap and
 * leg width come from the pair's diff-pair net class. Returns `success:false`
 * when the pair can't be resolved (each net needs exactly two pads) or the
 * kernel is unavailable.
 */
export async function routeDiffPair(
  pcb: Pcb,
  netP: string,
  netN: string,
): Promise<DiffPairResult> {
  const wasm = await loadEcadWasm();
  if (!wasm) return { success: false };
  try {
    return wasm.ecadRouteDiffPair(JSON.stringify(pcb), netP, netN) as DiffPairResult;
  } catch (e) {
    console.warn("[ECAD] Diff-pair routing failed:", e);
    return { success: false };
  }
}

/** A single generated fabrication output file. */
export interface FabFile {
  name: string;
  content: string;
}

/**
 * Generate all fabrication outputs for a PCB: Gerber layer files, an
 * Excellon drill file (when the board has holes), pick-and-place CSV, and
 * BOM CSV. Returns null if the ECAD WASM is unavailable so callers can
 * distinguish "no kernel" from "export failed".
 */
export async function exportFabFiles(pcb: Pcb): Promise<FabFile[] | null> {
  const wasm = await loadEcadWasm();
  if (!wasm) return null;
  try {
    return wasm.ecadExportFab(JSON.stringify(pcb)) as FabFile[];
  } catch (e) {
    console.warn("[ECAD] Fab export failed:", e);
    return null;
  }
}

/** Parse a KiCad .kicad_pcb file into a Pcb struct. */
export async function parseKicadPcb(content: string): Promise<Pcb | null> {
  const wasm = await loadEcadWasm();
  if (!wasm) return null;
  try {
    return wasm.parseKicadPcb(content) as Pcb;
  } catch (e) {
    console.warn("[ECAD] KiCad PCB parse failed:", e);
    return null;
  }
}

/** Fill copper pour zones on the PCB. */
export async function fillZones(pcb: Pcb): Promise<FilledZoneResult[]> {
  const wasm = await loadEcadWasm();
  if (!wasm) return [];
  try {
    return wasm.ecadFillZones(JSON.stringify(pcb)) as FilledZoneResult[];
  } catch (e) {
    console.warn("[ECAD] Zone fill failed:", e);
    return [];
  }
}

// ---------------------------------------------------------------------------
// Builtin symbol/footprint library
// ---------------------------------------------------------------------------

export interface SymbolGraphic {
  type: "Rect" | "Line" | "Circle" | "Polyline";
  x?: number;
  y?: number;
  width?: number;
  height?: number;
  x1?: number;
  y1?: number;
  x2?: number;
  y2?: number;
  cx?: number;
  cy?: number;
  r?: number;
  points?: Vec2[];
}

export interface FootprintTemplate {
  name: string;
  pads: import("@vcad/ir").Pad[];
  graphics: import("@vcad/ir").FootprintGraphic[];
}

/**
 * Resolution of a footprint id by the parametric engine: the generated land
 * pattern plus whether it was a real package-family match or a generic
 * placeholder (so callers can warn loudly instead of placing wrong geometry).
 */
export interface FootprintResolution {
  template: FootprintTemplate | null;
  /** True when a real family land pattern was generated; false for a placeholder. */
  matched: boolean;
  /** Recognized family (e.g. "QFN", "SOIC", "DPAK"), or null for the fallback. */
  family: string | null;
  /** Human-readable explanation of what was generated or why it fell back. */
  note: string;
}

export interface SymbolDef {
  id: string;
  name: string;
  prefix: string;
  defaultValue: string;
  pins: import("@vcad/ir").SchematicPin[];
  graphics: SymbolGraphic[];
  footprintTemplate: FootprintTemplate | null;
}

/** Get all builtin symbol definitions from the Rust library. */
export async function builtinSymbols(): Promise<SymbolDef[]> {
  const wasm = await loadEcadWasm();
  if (!wasm) return [];
  try {
    return wasm.ecadBuiltinSymbols() as SymbolDef[];
  } catch (e) {
    console.warn("[ECAD] builtinSymbols failed:", e);
    return [];
  }
}

/**
 * Resolve a KiCad-style footprint name (e.g.
 * "Package_SO:SOIC-8_3.9x4.9mm_P1.27mm") to a parametric footprint template.
 * `pinCount` drives the fallback when the name isn't recognized.
 */
export async function footprintForName(
  name: string,
  pinCount: number,
): Promise<FootprintTemplate | null> {
  const wasm = await loadEcadWasm();
  if (!wasm || typeof wasm.ecadFootprintForName !== "function") return null;
  try {
    return (wasm.ecadFootprintForName(name, pinCount) as FootprintTemplate) ?? null;
  } catch (e) {
    console.warn("[ECAD] footprintForName failed:", e);
    return null;
  }
}

/**
 * Resolve a footprint id to a land pattern *plus* resolution status — like
 * {@link footprintForName} but exposing whether the id matched a real package
 * family or fell back to a generic placeholder. Returns `null` only when the
 * kernel is unavailable (distinct from a resolution whose `template` is null).
 */
export async function resolveFootprint(
  name: string,
  pinCount: number,
): Promise<FootprintResolution | null> {
  const wasm = await loadEcadWasm();
  if (!wasm || typeof wasm.ecadResolveFootprint !== "function") return null;
  try {
    return (wasm.ecadResolveFootprint(name, pinCount) as FootprintResolution) ?? null;
  } catch (e) {
    console.warn("[ECAD] resolveFootprint failed:", e);
    return null;
  }
}

// ---------------------------------------------------------------------------
// Ratsnest + PCB geometry
// ---------------------------------------------------------------------------

export interface RatsnestLine {
  net: string;
  from: Vec2;
  to: Vec2;
  fp_ref: string;
  pad_num: string;
}

/** Compute ratsnest lines for unrouted net connections. */
export async function computeRatsnest(
  pcb: Pcb,
  netlist: NetlistResult,
): Promise<RatsnestLine[]> {
  const wasm = await loadEcadWasm();
  if (!wasm) return [];
  try {
    return wasm.ecadComputeRatsnest(
      JSON.stringify(pcb),
      JSON.stringify(netlist),
    ) as RatsnestLine[];
  } catch (e) {
    console.warn("[ECAD] computeRatsnest failed:", e);
    return [];
  }
}

// ---------------------------------------------------------------------------
// Component 3D meshes
// ---------------------------------------------------------------------------

export interface ComponentMesh {
  footprint_ref: string;
  positions: number[];
  indices: number[];
  normals: number[];
  color: [number, number, number];
  metalness: number;
  /** PBR roughness, 0..1 (older WASM payloads may omit it). */
  roughness?: number;
  /** Emissive RGB 0..1 (linear); `[0,0,0]` = not emissive (LEDs glow). */
  emissive?: [number, number, number];
}

/** Generate 3D component body meshes for all footprints on a PCB. */
export async function componentMeshes(pcb: Pcb): Promise<ComponentMesh[]> {
  const wasm = await loadEcadWasm();
  if (!wasm) return [];
  try {
    return wasm.ecadComponentMeshes(JSON.stringify(pcb)) as ComponentMesh[];
  } catch (e) {
    console.warn("[ECAD] componentMeshes failed:", e);
    return [];
  }
}

/**
 * A single colored sub-mesh of a layered PCB preview (see
 * `vcad_eval::pcb_preview`). The board is split into a green substrate, gold
 * copper, real component bodies, and white silkscreen so a lit GLB viewer
 * renders a recognizable board instead of one gray slab.
 */
export interface PcbPreviewMesh {
  /** Role: "mask" | "substrate" | "copper" | "pour" | "via" | "component" | "silkscreen". */
  role: string;
  /** Flat vertex positions [x,y,z,...] (mm, board-local, centered on z=0). */
  positions: number[];
  /** Triangle indices. */
  indices: number[];
  /** Per-vertex normals [nx,ny,nz,...]. */
  normals: number[];
  /** Base color RGB, 0..1 (linear). */
  color: [number, number, number];
  /** PBR metalness, 0..1. */
  metalness: number;
  /** PBR roughness, 0..1. */
  roughness: number;
  /** Emissive RGB 0..1 (linear); `[0,0,0]` = not emissive (older WASM omits). */
  emissive?: [number, number, number];
  /** KHR_materials_clearcoat factor 0..1 (glossy soldermask). */
  clearcoat?: number;
  /** Clearcoat roughness, 0..1. */
  clearcoat_roughness?: number;
}

/**
 * Generate layered, colored preview meshes for a PCB — substrate, copper,
 * component bodies, and silkscreen — for 3D rendering. Returns an empty array
 * when the ECAD WASM is unavailable or the build predates the binding.
 */
export async function pcbPreviewMeshes(pcb: Pcb): Promise<PcbPreviewMesh[]> {
  const wasm = await loadEcadWasm();
  if (!wasm || typeof wasm.ecadPcbPreviewMeshes !== "function") return [];
  try {
    return wasm.ecadPcbPreviewMeshes(JSON.stringify(pcb)) as PcbPreviewMesh[];
  } catch (e) {
    console.warn("[ECAD] pcbPreviewMeshes failed:", e);
    return [];
  }
}

// ---------------------------------------------------------------------------
// Circuit simulation (lumped-element transient solver)
// ---------------------------------------------------------------------------

/** A snapshot from the circuit simulator. */
export interface CircuitObservation {
  time: number;
  nodeVoltages: number[];
  deviceCurrents: number[];
  /** Rotor angle (rad) per device id; 0 for non-motors. */
  rotorAngles: number[];
  /** Rotor angular velocity (rad/s) per device id; 0 for non-motors. */
  rotorSpeeds: number[];
}

/** A live circuit simulation handle (a WASM `CircuitSim` instance). */
export interface CircuitSimHandle {
  /** Advance `n` timesteps and return the final observation. */
  step(n: number): CircuitObservation;
  /** Current state without advancing. */
  observe(): CircuitObservation;
  /** Reset to the power-on state. */
  reset(): void;
  /** Mutate a device's primary scalar (drive a switch / scrubbed value). */
  setValue(deviceId: number, value: number): void;
  /** Configured timestep (s). */
  dt(): number;
  /** Release the WASM-side instance. */
  free(): void;
}

/**
 * Build a live circuit simulation from a spec JSON
 * (`{ dt, devices: [{ kind, p, n, value }] }`). Returns null if the ECAD WASM
 * isn't available or the spec is invalid.
 */
export async function createCircuitSim(specJson: string): Promise<CircuitSimHandle | null> {
  const wasm = await loadEcadWasm();
  if (!wasm || typeof wasm.CircuitSim !== "function") return null;
  try {
    return new wasm.CircuitSim(specJson) as CircuitSimHandle;
  } catch (e) {
    console.warn("[circuit-sim] build failed:", e);
    return null;
  }
}

// ---------------------------------------------------------------------------
// Generative parts catalog + verified substitution (vcad-ecad-parts/-verify)
// ---------------------------------------------------------------------------

/** A typed, dimensioned spec value (mirrors Rust serde, tag `dim`). */
export interface SpecValue {
  dim:
    | "Resistance"
    | "Capacitance"
    | "Inductance"
    | "Voltage"
    | "Current"
    | "Power"
    | "Frequency"
    | "Tolerance";
  value: number;
}

export type ComponentClass = "Resistor" | "Capacitor" | "Inductor";

/** A manufacturer cross-reference. */
export interface ElecXref {
  mpn: string;
  manufacturer: string;
  datasheet: string | null;
}

/** A fully-resolved part: binding + generated geometry. */
export interface ResolvedPart {
  family_id: string;
  class: ComponentClass;
  value: string;
  value_si: SpecValue;
  tolerance: number | null;
  package: string;
  derived: DerivedPart;
  mpns: ElecXref[];
}

export type FootprintCompat =
  | "Identical"
  | "Compatible"
  | "NeedsReroute"
  | "Incompatible";

/** A proposed alternative part with its compatibility verdict. */
export interface Alternative {
  part: ResolvedPart;
  spec_distance: number;
  compat: FootprintCompat;
}

/** The outcome of proving a substitution against the board's DRC. */
export interface Substitution {
  reference: string;
  drop_in: boolean;
  added: DrcViolationResult[];
  removed: DrcViolationResult[];
  before_count: number;
  after_count: number;
}

/**
 * Resolve a free-text query (e.g. `"10k 0603 1%"`) into one fully-specified
 * part — footprint, symbol, 3D body, and MPN cross-references — generated from
 * a parametric package. Returns null when the query has no resolvable value or
 * the ECAD WASM is unavailable.
 */
export async function resolvePart(query: string): Promise<ResolvedPart | null> {
  const wasm = await loadEcadWasm();
  if (!wasm || typeof wasm.ecadResolvePart !== "function") return null;
  try {
    return (wasm.ecadResolvePart(query) as ResolvedPart | null) ?? null;
  } catch (e) {
    console.warn("[ECAD] resolvePart failed:", e);
    return null;
  }
}

/**
 * Spec-search the catalog, returning the best match plus its nearest E-series
 * neighbours (spec-distance ranked). Empty if unavailable or unresolvable.
 */
export async function searchEcadParts(
  query: string,
  limit = 5,
): Promise<ResolvedPart[]> {
  const wasm = await loadEcadWasm();
  if (!wasm || typeof wasm.ecadSearchParts !== "function") return [];
  try {
    return (wasm.ecadSearchParts(query, limit) as ResolvedPart[]) ?? [];
  } catch (e) {
    console.warn("[ECAD] searchParts failed:", e);
    return [];
  }
}

/** JSON manifest of the parametric part families. `null` if unavailable. */
export async function partsManifest(): Promise<string | null> {
  const wasm = await loadEcadWasm();
  if (!wasm || typeof wasm.ecadPartsManifest !== "function") return null;
  try {
    return wasm.ecadPartsManifest() as string;
  } catch {
    return null;
  }
}

/** A resolved jellybean pin: its definition plus an auto-generated symbol position. */
export interface PartDefPin {
  number: string;
  name: string;
  /** PinType variant name (Input, Output, PowerInput, OpenCollector, ...). */
  pin_type: string;
  x: number;
  y: number;
}

/**
 * A resolved jellybean part: the universal pin definitions for the requested
 * footprint plus the metadata needed to place and document it. Returned by the
 * curated parts database (`vcad_ecad_parts::jellybean`).
 */
export interface ResolvedPartDef {
  name: string;
  matched_alias: string | null;
  description: string | null;
  footprint: string;
  footprint_known: boolean;
  footprints: string[];
  pins: PartDefPin[];
  datasheet_url: string | null;
  app_notes: string[];
  warnings: string[];
}

/**
 * Resolve a named jellybean part (e.g. `"NE555"`) and optional footprint into
 * its pin definitions — number, name, electrical type, and symbol position.
 * Returns null when the name is not in the curated database, or the ECAD WASM
 * is unavailable.
 */
export async function resolvePartDef(
  name: string,
  footprint?: string,
): Promise<ResolvedPartDef | null> {
  const wasm = await loadEcadWasm();
  if (!wasm || typeof wasm.ecadResolvePartDef !== "function") return null;
  try {
    return (
      (wasm.ecadResolvePartDef(name, footprint) as ResolvedPartDef | null) ?? null
    );
  } catch (e) {
    console.warn("[ECAD] resolvePartDef failed:", e);
    return null;
  }
}

/** JSON manifest of the curated jellybean catalog. `null` if unavailable. */
export async function jellybeanManifest(): Promise<string | null> {
  const wasm = await loadEcadWasm();
  if (!wasm || typeof wasm.ecadJellybeanManifest !== "function") return null;
  try {
    return wasm.ecadJellybeanManifest() as string;
  } catch {
    return null;
  }
}

/**
 * Propose spec-compatible alternatives for the part a query resolves to, each
 * classified by footprint compatibility (Identical / NeedsReroute / …).
 */
export async function findAlternatives(query: string): Promise<Alternative[]> {
  const wasm = await loadEcadWasm();
  if (!wasm || typeof wasm.ecadFindAlternatives !== "function") return [];
  try {
    return (wasm.ecadFindAlternatives(query) as Alternative[]) ?? [];
  } catch (e) {
    console.warn("[ECAD] findAlternatives failed:", e);
    return [];
  }
}

/**
 * PROVE a substitution: swap `reference` on the board for the part that
 * `candidateQuery` resolves to, re-derive its footprint, re-place at the same
 * anchor, re-run DRC (including connectivity), and return the before/after
 * delta with a `drop_in` verdict. Null if the candidate is unresolvable.
 */
export async function verifySubstitution(
  pcb: Pcb,
  reference: string,
  candidateQuery: string,
): Promise<Substitution | null> {
  const wasm = await loadEcadWasm();
  if (!wasm || typeof wasm.ecadVerifySubstitution !== "function") return null;
  try {
    return (
      (wasm.ecadVerifySubstitution(
        JSON.stringify(pcb),
        reference,
        candidateQuery,
      ) as Substitution | null) ?? null
    );
  } catch (e) {
    console.warn("[ECAD] verifySubstitution failed:", e);
    return null;
  }
}

/** Build a re-runnable verification Receipt for the current board state. */
export async function buildReceipt(pcb: Pcb): Promise<Receipt | null> {
  const wasm = await loadEcadWasm();
  if (!wasm || typeof wasm.ecadBuildReceipt !== "function") return null;
  try {
    return (wasm.ecadBuildReceipt(JSON.stringify(pcb)) as Receipt) ?? null;
  } catch (e) {
    console.warn("[ECAD] buildReceipt failed:", e);
    return null;
  }
}

/** Re-run a Receipt against the current board → Holds | Stale | Violated. */
export async function verifyReceipt(
  pcb: Pcb,
  receipt: Receipt,
): Promise<ReceiptStatus | null> {
  const wasm = await loadEcadWasm();
  if (!wasm || typeof wasm.ecadVerifyReceipt !== "function") return null;
  try {
    return (
      (wasm.ecadVerifyReceipt(
        JSON.stringify(pcb),
        JSON.stringify(receipt),
      ) as ReceiptStatus) ?? null
    );
  } catch (e) {
    console.warn("[ECAD] verifyReceipt failed:", e);
    return null;
  }
}
