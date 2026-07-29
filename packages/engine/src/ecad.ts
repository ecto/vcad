/**
 * ECAD (Electronics) WASM wrappers.
 *
 * Lazy-loads the kernel WASM and provides typed wrappers for DRC, ERC,
 * netlist generation, routing, and zone fill.
 */

import type {
  Document,
  SchematicSheet,
  Pcb,
  Trace,
  Vec2,
  PcbLayer,
  DerivedPart,
  Receipt,
  ReceiptStatus,
  Zone,
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
  | "UnstitchedPad"
  | "SilkscreenClearance"
  | "CourtyardOverlap"
  | "AcidTrap"
  | "Keepout"
  | "Short"
  | "NetIslands"
  | "SameNetBypass";

export type DrcSeverity = "Error" | "Warning";

/** Where a DRC violation originates — separates synthesized land-pattern
 *  artifacts from genuine layout faults. Mirrors the Rust `DrcProvenance`. */
export type DrcProvenance = "intra_footprint" | "inter_component" | "routing";

export interface DrcViolationResult {
  rule: DrcRuleType;
  severity: DrcSeverity;
  position: Vec2;
  message: string;
  actual: number;
  required: number;
  /** Footprint-internal, between two components, or routing/board-level. */
  provenance: DrcProvenance;
  /** True when a generated (synthesized) footprint land pattern is involved. */
  generated: boolean;
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

/** Per-plane group outcome of a design-constraint solve. */
export interface ConstraintGroupReport {
  node: number;
  status: string;
  converged: boolean;
  iterations: number;
  residualNorm: number;
  dof: number;
  constraintCount: number;
}

/** Aggregate report from the document-level design-constraint solver. */
export interface DesignSolveReport {
  converged: boolean;
  groups: ConstraintGroupReport[];
  movedFootprints: string[];
  movedVertices: string[];
  movedSketches: number[];
  drivenValues: Array<{ id: string; value: number }>;
  residuals: Array<{ id: string; residual: number; driven: boolean }>;
  errors: string[];
  warnings: string[];
}

/**
 * Solve the document's design constraints in the kernel. Returns the updated
 * document (footprint positions/rotations, outline vertices, sketch points,
 * back-annotated driven dimensions) plus the solve report. Fail-closed: a
 * missing kernel or a parse error is an `errored` outcome, never a silent
 * no-op "success".
 */
export async function solveDesignConstraints(
  doc: Document,
  options?: { extraFixed?: Array<{ node: number; ref: string }> },
): Promise<VerifyOutcome<{ document: Document; report: DesignSolveReport }>> {
  return verifyWithKernel("design constraints", (wasm) => {
    if (typeof wasm.solveDesignConstraints !== "function") {
      throw new Error("kernel WASM predates solveDesignConstraints");
    }
    return JSON.parse(
      wasm.solveDesignConstraints(JSON.stringify(doc), JSON.stringify(options ?? {})),
    ) as { document: Document; report: DesignSolveReport };
  });
}

/**
 * Validate and measure the document's constraints without mutating anything —
 * every dimensional constraint's current value lands in `drivenValues`.
 */
export async function checkDesignConstraints(
  doc: Document,
): Promise<VerifyOutcome<DesignSolveReport>> {
  return verifyWithKernel("design constraints (check)", (wasm) => {
    if (typeof wasm.checkDesignConstraints !== "function") {
      throw new Error("kernel WASM predates checkDesignConstraints");
    }
    return JSON.parse(wasm.checkDesignConstraints(JSON.stringify(doc))) as DesignSolveReport;
  });
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

/**
 * Run DRC with the geometric checks scoped to an axis-aligned region (mm) —
 * the incremental verify-on-write entry point. Only elements intersecting the
 * region are subjects of the clearance/width/drill/edge checks (each still
 * judged against the whole board); connectivity (shorts, net islands,
 * unrouted nets) always runs board-global. Same three-state contract as
 * {@link runDrc}.
 *
 * A kernel WASM built before the scoped binding falls back to a full-board
 * run — a correct (slower) superset, and still element-wise comparable across
 * a before/after pair as long as both snapshots take the same path.
 */
export async function runDrcInRegion(
  pcb: Pcb,
  min: Vec2,
  max: Vec2,
): Promise<VerifyOutcome<DrcViolationResult[]>> {
  const wasm = await loadEcadWasm();
  if (wasm && typeof wasm.ecadCheckDrcInRegion !== "function") {
    return runDrc(pcb);
  }
  return verifyWithKernel(
    "DRC(region)",
    (w) =>
      w.ecadCheckDrcInRegion(
        JSON.stringify(pcb),
        min.x,
        min.y,
        max.x,
        max.y,
      ) as DrcViolationResult[],
  );
}

/**
 * Discriminated outcome for fab-readiness probes that must distinguish a real
 * pass from an *unverifiable* one. Surface the kernel error verbatim so callers
 * can fail closed and quote the exact failing field.
 *
 * - `unavailable`: the ECAD kernel WASM isn't loaded (or predates the binding).
 * - `error`: the kernel ran but threw — almost always a serde parse failure
 *   whose message names the offending field (e.g. ``missing field `thickness```).
 */
export type EcadProbe<T> =
  | { ok: true; value: T }
  | { ok: false; reason: "unavailable" | "error"; message: string };

/**
 * Run DRC, surfacing failures as an {@link EcadProbe} instead of swallowing them.
 * Use this (not `runDrc`) anywhere a parse failure must read as *unverifiable*.
 */
export async function tryRunDrc(pcb: Pcb): Promise<EcadProbe<DrcViolationResult[]>> {
  const wasm = await loadEcadWasm();
  if (!wasm) {
    return { ok: false, reason: "unavailable", message: "ECAD kernel WASM not loaded" };
  }
  try {
    return { ok: true, value: wasm.ecadCheckDrc(JSON.stringify(pcb)) as DrcViolationResult[] };
  } catch (e) {
    return { ok: false, reason: "error", message: e instanceof Error ? e.message : String(e) };
  }
}

// ---------------------------------------------------------------------------
// Fab preparation (calibrate → route/certify → fix loop → prune → receipt)
// ---------------------------------------------------------------------------

/** Options for {@link runFabPrep} (mirrors `vcad_ecad_fabprep::FabPrepOptions`). */
export interface FabPrepOptions {
  /** Derive and apply rule calibration from the board's own declared via classes. */
  calibrate_rules?: boolean;
  /** Route or certify the connections the board arrived without. */
  route_remaining?: boolean;
  /** Maximum strip-and-re-route rounds. */
  max_rounds?: number;
  /** Search knobs for the complete window router. */
  verdict?: { budget: number; max_cluster: number };
  /** Remove copper reaching no pad or pour of its net before the final DRC. */
  prune_dangling?: boolean;
  /** DRC rule names whose route-attributable violations are explicitly waived. */
  accept_rules?: string[];
}

/**
 * The fab-prep receipt (mirrors `vcad_ecad_fabprep::FabPrepReport`). Typed
 * loosely on purpose: the Rust side owns the shape, and mirroring every field
 * here would be one more place to drift.
 */
export interface FabPrepReport {
  converged: boolean;
  blocker: string | null;
  calibration_requested: boolean;
  calibration: {
    applied: {
      rule: string;
      declared: number;
      calibrated: number;
      justification: string;
    }[];
    refused: { rule: string; requested: number; floor: number; reason: string }[];
  };
  initial_verdict: Record<string, unknown> | null;
  rounds: Record<string, unknown>[];
  pruned_traces: number;
  pruned_vias: number;
  connectivity: { on_arrival: number; on_completion: number };
  accepted_rules: string[];
  delta: {
    rules: {
      rule: string;
      baseline: number;
      final_count: number;
      route_attributable: number;
      mode: string;
      route_fixable: boolean;
      accepted: boolean;
    }[];
    baseline_total: number;
    final_total: number;
    route_attributable_total: number;
    route_attributable_fixable: number;
    route_attributable_accepted: number;
    offenders: {
      rule: string;
      severity: string;
      position: [number, number];
      message: string;
      required: number;
      nets: string[];
    }[];
  };
  board: Record<string, number>;
}

/**
 * Run the whole fab-preparation pipeline and return the fixed board plus its
 * DRC-delta receipt.
 *
 * Same three-state contract as {@link tryRunDrc}: a kernel that cannot parse the
 * board reports `error`, never a converged run. The caller writes the returned
 * board back to the session only on `ok` — and ships it only when
 * `report.converged`.
 */
export async function runFabPrep(
  pcb: Pcb,
  options?: FabPrepOptions,
): Promise<EcadProbe<{ report: FabPrepReport; pcb: Pcb }>> {
  const wasm = await loadEcadWasm();
  if (!wasm) {
    return { ok: false, reason: "unavailable", message: "ECAD kernel WASM not loaded" };
  }
  if (typeof wasm.ecadFabPrep !== "function") {
    return {
      ok: false,
      reason: "unavailable",
      message: "kernel WASM predates ecadFabPrep",
    };
  }
  try {
    const out = wasm.ecadFabPrep(
      JSON.stringify(pcb),
      options ? JSON.stringify(options) : undefined,
    ) as { report: FabPrepReport; pcb: Pcb };
    return { ok: true, value: out };
  } catch (e) {
    return { ok: false, reason: "error", message: e instanceof Error ? e.message : String(e) };
  }
}

/** Per-round status from the stepwise fab-prep driver. */
export interface FabPrepRoundStatus {
  done: boolean;
  round: number;
  attributable: number;
  max_rounds: number;
}

/**
 * Chunked form of {@link runFabPrep}: drives the kernel's stepwise
 * `FabPrepRun` session one strip-and-re-route round at a time, invoking
 * `onRound` between kernel calls so a host can stream progress while the
 * Node event loop is unblocked. Bit-identical outcome to the one-shot call —
 * `run_fab_prep` is implemented on the same kernel session type. Falls back
 * to the one-shot path on a kernel that predates `FabPrepRun`.
 */
export async function runFabPrepChunked(
  pcb: Pcb,
  options?: FabPrepOptions,
  onRound?: (status: FabPrepRoundStatus) => void | Promise<void>,
): Promise<EcadProbe<{ report: FabPrepReport; pcb: Pcb }>> {
  const wasm = await loadEcadWasm();
  if (!wasm) {
    return { ok: false, reason: "unavailable", message: "ECAD kernel WASM not loaded" };
  }
  const Runner = (wasm as { FabPrepRun?: new (pcb: string, opts?: string) => FabPrepRunHandle })
    .FabPrepRun;
  if (typeof Runner !== "function") {
    return runFabPrep(pcb, options);
  }
  try {
    const run = new Runner(JSON.stringify(pcb), options ? JSON.stringify(options) : undefined);
    try {
      for (;;) {
        const status = run.round() as FabPrepRoundStatus;
        await onRound?.(status);
        if (status.done) break;
      }
      const out = run.finish() as { report: FabPrepReport; pcb: Pcb };
      return { ok: true, value: out };
    } finally {
      run.free?.();
    }
  } catch (e) {
    return { ok: false, reason: "error", message: e instanceof Error ? e.message : String(e) };
  }
}

interface FabPrepRunHandle {
  round(): unknown;
  finish(): unknown;
  free?(): void;
}

// ---------------------------------------------------------------------------
// PCB Design-for-Manufacturing (fab-profile capability checks)
// ---------------------------------------------------------------------------

/** Supported PCB fab profiles (mirrors `vcad_ecad_pcb::PcbFabProfile`). */
export type PcbFabProfile = "jlcpcb" | "pcbway" | "generic_2layer" | "generic_4layer";

/** DFM severity tier (mirrors `vcad_ecad_pcb::dfm::DfmSeverity`). */
export type PcbDfmSeverity = "error" | "warning" | "info";

/** A representative location of a DFM finding (board mm). */
export interface PcbDfmLocation {
  x: number;
  y: number;
  label: string;
  /** The two net names in contact, for net-pair findings (clearance). */
  nets?: [string, string];
}

/** Pass/fail verdict for one DFM rule (mirrors `PcbDfmRuleResult`). */
export interface PcbDfmRuleResult {
  rule: string;
  passed: boolean;
  applicable: boolean;
  severity: PcbDfmSeverity;
  units: string;
  limit: number;
  measured: number | null;
  violations: number;
  message: string;
  locations: PcbDfmLocation[];
}

/** Full DFM verdict for a board against one fab profile (mirrors `PcbDfmReport`). */
export interface PcbDfmReport {
  profile: string;
  profile_name: string;
  pack_version: string;
  copper_weight_oz: number;
  copper_layer_count: number;
  passed: boolean;
  error_count: number;
  warning_count: number;
  rules: PcbDfmRuleResult[];
}

/**
 * Run PCB Design-for-Manufacturing checks against a fab profile. Where DRC
 * validates a board against its own declared rules, this validates the geometry
 * against a fab house's published process capability. Returns null only if the
 * ECAD WASM is unavailable; an unknown profile throws.
 */
export async function runPcbDfm(
  pcb: Pcb,
  profile: PcbFabProfile,
  rulePackToml?: string,
): Promise<PcbDfmReport | null> {
  const wasm = await loadEcadWasm();
  if (!wasm || typeof wasm.ecadDfmCheck !== "function") return null;
  return wasm.ecadDfmCheck(JSON.stringify(pcb), profile, rulePackToml ?? "") as PcbDfmReport;
}

/** Return the bundled default DFM rule-pack TOML for a fab profile. */
export async function getPcbDfmPack(profile: PcbFabProfile): Promise<string | null> {
  const wasm = await loadEcadWasm();
  if (!wasm || typeof wasm.ecadDfmDefaultPack !== "function") return null;
  return wasm.ecadDfmDefaultPack(profile) as string;
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

/** One galvanic island of a net's copper. */
export interface NetIsland {
  pad_count: number;
  node_count: number;
  position: Vec2;
}

/** Galvanic-continuity analysis for one net's realized copper — the
 *  realized-geometry check that gates power/PDN and impedance verdicts. */
export interface NetContinuity {
  net: string;
  /** Disjoint galvanic islands: 0 = no copper, 1 = continuous, ≥2 = split. */
  islands: number;
  total_pads: number;
  connected_pads: number;
  /** connected_pads / total_pads, in [0, 1]. */
  coverage: number;
  /** Stitching vias on the net. */
  vias: number;
  /** True when the net has at least one piece of realized copper. */
  realized: boolean;
  /** True when the net's copper forms exactly one galvanic island. */
  continuous: boolean;
  /** Largest stranded (non-main) island when split; null otherwise. */
  worst_island: NetIsland | null;
}

/** Analyze a net's realized-copper galvanic continuity (islands, pad coverage,
 *  stitching vias, worst stranded island). Returns null if WASM is unavailable. */
export async function netContinuity(pcb: Pcb, net: string): Promise<NetContinuity | null> {
  const wasm = await loadEcadWasm();
  if (!wasm) return null;
  try {
    return wasm.ecadNetContinuity(JSON.stringify(pcb), net) as NetContinuity;
  } catch (e) {
    console.warn("[ECAD] Net continuity failed:", e);
    return null;
  }
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
  /** Soft-iron saturation polarization J_s (T). Omit to keep the iron linear. */
  ironJsT?: number | null;
  /** Stator tooth geometry — without it the model cannot see tooth saturation. */
  teeth?: TeethSpecInput | null;
}

/** Stator tooth geometry (mirrors `vcad_ecad_sim::TeethSpec`). */
export interface TeethSpecInput {
  slots: number;
  toothWidthMm: number;
  meanRadiusMm: number;
  /** Iron path length through the tooth body (mm). 0 = report only, no reluctance. */
  toothPathMm?: number;
}

/** Full MEC solve (mirrors `vcad_ecad_sim::AirGapSolution`). */
export interface AirGapSolutionResult {
  bGapTesla: number;
  bToothTesla: number | null;
  bIronTesla: number | null;
  toothConcentration: number | null;
  nonlinear: boolean;
  iterations: number;
  converged: boolean;
  warnings: string[];
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

/**
 * Full air-gap MEC solve: gap/tooth/yoke flux, saturation state, warnings.
 * Null if ECAD WASM is unavailable — or predates the binding, in which case the
 * caller should fall back to {@link airgapFluxDensity}.
 */
export async function airgapSolve(spec: AirGapSpecInput): Promise<AirGapSolutionResult | null> {
  const wasm = await loadEcadWasm();
  if (!wasm) return null;
  try {
    const fn = (wasm as unknown as { ecadAirgapSolve?: (s: string) => unknown }).ecadAirgapSolve;
    if (typeof fn !== "function") return null;
    return fn(JSON.stringify(spec)) as AirGapSolutionResult;
  } catch (e) {
    console.warn("[ECAD] airgapSolve failed:", e);
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

/** A via produced by the whole-board auto-router. Outer-layer span = through
 *  via; anything else is a blind/buried (micro)via chosen by the 3D search. */
export interface RoutedVia {
  position: Vec2;
  net: string;
  /** Top of the via's copper span. */
  start_layer: PcbLayer;
  /** Bottom of the via's copper span. */
  end_layer: PcbLayer;
}

/** Why a connection could not be routed, and where — so an agent or human can
 *  act on it instead of staring at a bare "unrouted" list. */
export interface UnroutedDiagnostic {
  net: string;
  from: Vec2;
  to: Vec2;
  /** Other nets blocking the corridor, most-blocking first. */
  blocking_nets: string[];
  /** Min corner of the congested region (mm). */
  region_min: Vec2;
  /** Max corner of the congested region (mm). */
  region_max: Vec2;
  /** A copper layer with the best chance (fewest blockers), if any is clearer. */
  suggested_layer?: string;
  /** Where dropping a via to `suggested_layer` would likely help. */
  suggested_via?: Vec2;
  /** Human-readable explanation of the obstruction. */
  reason: string;
}

/** Result of {@link routeAll}. */
export interface RouteAllResult {
  traces: RoutedTrace[];
  vias: RoutedVia[];
  /**
   * Copper pours synthesized for high-current nets. **Must be added to the
   * board along with the traces and vias** — the routing assumes them: a poured
   * net is carried by its plane, so its pads were stitched to the plane instead
   * of traced to each other. Absent on kernels that predate pour synthesis.
   */
  zones?: Zone[];
  routed_nets: string[];
  unrouted_nets: string[];
  /** Per-unrouted-connection diagnostics; empty when fully routed. */
  diagnostics: UnroutedDiagnostic[];
  /** Fraction of attempted connections routed, in [0, 1]. */
  routability: number;
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
  effort = 1,
): Promise<RouteAllResult> {
  const empty: RouteAllResult = {
    traces: [],
    vias: [],
    zones: [],
    routed_nets: [],
    unrouted_nets: [],
    diagnostics: [],
    routability: 1,
  };
  const wasm = await loadEcadWasm();
  if (!wasm) return empty;
  try {
    return wasm.ecadRouteAll(
      JSON.stringify(pcb),
      width,
      JSON.stringify(netsFilter),
      effort,
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

/** Options for {@link matchTraceLengths}. */
export interface LengthMatchOptions {
  /** Target routed length in mm; defaults to the longest net in the group. */
  target_length?: number;
  /** A net counts as matched within this of the target (mm, default 0.1). */
  tolerance?: number;
  /** Maximum meander amplitude in mm (default 2.0). */
  max_amplitude?: number;
  /** Meander period spacing in mm (default 1.0). */
  spacing?: number;
  /** Meander pattern style (default "trombone"). */
  style?: "trombone" | "sawtooth";
  /** Measure + verdict only; generate no meanders. */
  check_only?: boolean;
}

/** Per-net outcome of {@link matchTraceLengths}. */
export interface NetLengthReport {
  net: string;
  length_before: number;
  length_after: number;
  matched: boolean;
  tuned: boolean;
  skip_reason?: string;
  /** Replacement traces for the net (only when `tuned`). */
  new_traces?: Trace[];
}

/** Result of {@link matchTraceLengths}. */
export interface LengthMatchResult {
  target_length: number;
  tolerance: number;
  all_matched: boolean;
  nets: NetLengthReport[];
}

/**
 * Length-match a group of nets by generating clearance-checked meanders on the
 * shorter ones. Pure: replacement traces come back as data for the caller to
 * commit. Returns null when the ECAD kernel WASM is unavailable, or when the
 * kernel rejects the request (e.g. an unrecognized `style` — the binding
 * refuses a typo rather than silently defaulting to Trombone).
 */
export async function matchTraceLengths(
  pcb: Pcb,
  nets: string[],
  opts: LengthMatchOptions = {},
): Promise<LengthMatchResult | null> {
  const wasm = await loadEcadWasm();
  // Guard on the export: a stale checked-in WASM build predates this binding.
  if (!wasm || typeof wasm.ecadLengthMatch !== "function") return null;
  try {
    return wasm.ecadLengthMatch(
      JSON.stringify(pcb),
      JSON.stringify(nets),
      JSON.stringify(opts),
    ) as LengthMatchResult;
  } catch (e) {
    console.warn("[ECAD] Length matching failed:", e);
    return null;
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
  const probe = await tryExportFabFiles(pcb);
  if (probe.ok) return probe.value;
  if (probe.reason === "error") console.warn("[ECAD] Fab export failed:", probe.message);
  return null;
}

/**
 * Attempt fabrication-file serialization, surfacing the kernel error instead of
 * collapsing it to `null`. The error message is the serde failure verbatim — so
 * a readiness gate can report the exact field the board can't serialize on
 * (`missing field \`thickness\``, `invalid type: null, expected f64`, …) rather
 * than a blank "export failed". See {@link EcadProbe}.
 */
export async function tryExportFabFiles(pcb: Pcb): Promise<EcadProbe<FabFile[]>> {
  const wasm = await loadEcadWasm();
  if (!wasm) {
    return { ok: false, reason: "unavailable", message: "ECAD kernel WASM not loaded" };
  }
  try {
    return { ok: true, value: wasm.ecadExportFab(JSON.stringify(pcb)) as FabFile[] };
  } catch (e) {
    return { ok: false, reason: "error", message: e instanceof Error ? e.message : String(e) };
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

/**
 * Export a Pcb to a native, editable KiCad 9 `.kicad_pcb` board file (the
 * inverse of {@link parseKicadPcb}). Returns null if the ECAD WASM is
 * unavailable so callers can distinguish "no kernel" from "export failed".
 */
export async function exportKicadPcb(pcb: Pcb): Promise<string | null> {
  const wasm = await loadEcadWasm();
  if (!wasm) return null;
  try {
    return wasm.exportKicadPcb(JSON.stringify(pcb)) as string;
  } catch (e) {
    console.warn("[ECAD] KiCad PCB export failed:", e);
    return null;
  }
}

/**
 * Export a SchematicSheet to a native, editable KiCad 9 `.kicad_sch`
 * schematic file. Returns null if the ECAD WASM is unavailable.
 */
export async function exportKicadSch(sheet: SchematicSheet): Promise<string | null> {
  const wasm = await loadEcadWasm();
  if (!wasm) return null;
  try {
    return wasm.exportKicadSch(JSON.stringify(sheet)) as string;
  } catch (e) {
    console.warn("[ECAD] KiCad schematic export failed:", e);
    return null;
  }
}

/**
 * Export a linked KiCad 9 project bundle (`<name>.kicad_pro` / `.kicad_sch` /
 * `.kicad_pcb`) with footprint→symbol cross-probe paths. Returns
 * `[filename, contents]` pairs, or null if the ECAD WASM is unavailable or
 * predates the bundle export.
 */
export async function exportKicadProject(
  sheet: SchematicSheet,
  pcb: Pcb,
  name: string,
): Promise<Array<[string, string]> | null> {
  // Structural cast: the checked-in WASM package may predate this binding
  // (artifacts are only refreshed on main), so probe for it at runtime.
  const wasm = (await loadEcadWasm()) as {
    exportKicadProject?: (sheetJson: string, pcbJson: string, name: string) => unknown;
  } | null;
  if (!wasm || typeof wasm.exportKicadProject !== "function") return null;
  try {
    return wasm.exportKicadProject(
      JSON.stringify(sheet),
      JSON.stringify(pcb),
      name,
    ) as Array<[string, string]>;
  } catch (e) {
    console.warn("[ECAD] KiCad project export failed:", e);
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
  /** Base-color alpha 0..1; below 1 = alpha-blended translucent material
   *  (the soldermask shell). Older WASM omits it (opaque). */
  alpha?: number;
  /** Board layer this mesh belongs to ("FCu", "BCu", "In1Cu", …, "FSilkS")
   *  when layer-specific; layer-spanning meshes (board body, vias,
   *  components) omit it. */
  layer?: string;
  /** Per-entity triangle ranges for picking/highlighting (copper meshes). */
  entities?: PcbPreviewEntity[];
}

/** A triangle range inside a {@link PcbPreviewMesh} belonging to one PCB
 *  entity — maps a raycast faceIndex (or a net) back to board data. */
export interface PcbPreviewEntity {
  /** "trace" | "trace_arc" | "zone" | "pad" | "via". */
  kind: string;
  /** Index into the corresponding Pcb collection (pads: within footprint). */
  index: number;
  /** Footprint index (pads only). */
  footprint?: number;
  /** Net the entity belongs to, when it has one. */
  net?: string;
  /** First index (into `indices`) of the entity's triangle range. */
  start: number;
  /** Number of indices in the range (multiple of 3). */
  count: number;
}

/**
 * Generate layered, colored preview meshes for a PCB — substrate, copper,
 * component bodies, and silkscreen — for 3D rendering. Returns an empty array
 * when the ECAD WASM is unavailable or the build predates the binding.
 */
export async function pcbPreviewMeshes(pcb: Pcb): Promise<PcbPreviewMesh[]> {
  const probe = await tryPcbPreviewMeshes(pcb);
  if (probe.ok) return probe.value;
  if (probe.reason === "error") console.warn("[ECAD] pcbPreviewMeshes failed:", probe.message);
  return [];
}

/**
 * Attempt to build the layered preview meshes, surfacing failures. An `error`
 * means the board solid trapped during evaluation (the geometry can't be
 * visualized); an `ok` result with an empty array means the kernel ran but the
 * board produced no renderable geometry. A readiness gate treats both as a
 * renderability blocker rather than a silent empty preview. See {@link EcadProbe}.
 */
export async function tryPcbPreviewMeshes(
  pcb: Pcb,
): Promise<EcadProbe<PcbPreviewMesh[]>> {
  const wasm = await loadEcadWasm();
  if (!wasm || typeof wasm.ecadPcbPreviewMeshes !== "function") {
    return {
      ok: false,
      reason: "unavailable",
      message: "PCB preview-mesh binding unavailable (kernel WASM missing or predates it)",
    };
  }
  try {
    return {
      ok: true,
      value: wasm.ecadPcbPreviewMeshes(JSON.stringify(pcb)) as PcbPreviewMesh[],
    };
  } catch (e) {
    return { ok: false, reason: "error", message: e instanceof Error ? e.message : String(e) };
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
// Schematic → circuit mapping + stateless analyses (#583 seam + #588 bindings)
// ---------------------------------------------------------------------------

/** One device of a mapped/authored circuit spec (`p`/`n` node ids, 0 = ground). */
export interface CircuitSpecDevice {
  kind:
    | "resistor"
    | "capacitor"
    | "inductor"
    | "vsource"
    | "isource"
    | "diode"
    | "led"
    | "motor";
  p: number;
  n: number;
  value?: number;
}

/** One component that blocked schematic→circuit conversion (fail-closed). */
export interface CircuitBlocker {
  reference: string;
  message: string;
}

/** Options for {@link circuitFromSchematic}. */
export interface CircuitMapOptions {
  /** Refdes to stub as open circuits (power symbols, connectors, ICs). */
  stubAsOpen?: string[];
  /** Net names to collapse onto ground beyond GND/VSS-style names. */
  groundNets?: string[];
  /** Supply rails: inject a vsource-to-ground per net at the given voltage. */
  supplies?: Array<{ net: string; volts: number }>;
}

/** Result of {@link circuitFromSchematic}: mapped spec or blocker list. */
export interface CircuitMapResult {
  ok: boolean;
  blockers?: CircuitBlocker[];
  devices?: CircuitSpecDevice[];
  numNodes: number;
  /** Net name → node id (ground nets and aliases → 0). */
  nodeOfNet: Record<string, number>;
  /** Refdes → device id. */
  deviceOfRef: Record<string, number>;
  groundNets: string[];
  stubbed: string[];
  /** Injected supply rails: net name → vsource device id. */
  supplySourceOfNet: Record<string, number>;
  unconnectedSupplies: string[];
}

/** DC operating-point result (camelCase mirror of `WasmDcSolution`). */
export interface CircuitDcResult {
  nodeVoltages: number[];
  deviceCurrents: number[];
  /** Tellegen residual Σ v·i (W) — nonzero only through solver error. */
  powerBalanceW: number;
  newtonIterations: number;
}

/** AC sweep result: per-omega complex node voltages as re/im arrays. */
export interface CircuitAcResult {
  source: number;
  points: Array<{
    omega: number;
    nodeVoltagesRe: number[];
    nodeVoltagesIm: number[];
  }>;
}

/** Adjoint tune result (camelCase mirror of `WasmTuneResult`). */
export interface CircuitTuneResult {
  tunedValues: Array<{ device: number; before: number; after: number }>;
  iterations: number;
  objectiveBefore: number;
  objectiveAfter: number;
  response?: Array<{
    frequencyHz: number;
    magnitudeBefore: number;
    magnitudeAfter: number;
    magnitudeTarget: number;
  }>;
  achievedCutoffHz?: number;
  achievedQFactor?: number;
  achievedDcVoltage?: number;
}

/**
 * Map a schematic sheet to a simulatable circuit spec via the fail-closed
 * netlist seam. Returns null if the ECAD WASM build lacks the binding;
 * blockers come back as data (`ok: false`), never as silent skips.
 */
export async function circuitFromSchematic(
  sheet: SchematicSheet,
  options: CircuitMapOptions = {},
): Promise<CircuitMapResult | null> {
  const wasm = await loadEcadWasm();
  const fn = (
    wasm as unknown as {
      circuitFromSchematic?: (sch: string, opts: string) => CircuitMapResult;
    } | null
  )?.circuitFromSchematic;
  if (typeof fn !== "function") return null;
  return fn(JSON.stringify(sheet), JSON.stringify(options));
}

/** DC operating point of a `{devices:[...]}` spec. Null if WASM unavailable. */
export async function circuitDcOperatingPoint(spec: {
  devices: CircuitSpecDevice[];
}): Promise<CircuitDcResult | null> {
  const wasm = await loadEcadWasm();
  const fn = (
    wasm as unknown as {
      circuitDcOperatingPoint?: (spec: string) => CircuitDcResult;
    } | null
  )?.circuitDcOperatingPoint;
  if (typeof fn !== "function") return null;
  return fn(JSON.stringify(spec));
}

/** Small-signal AC sweep driven by device `sourceId` at `omegas` (rad/s). */
export async function circuitAcResponse(
  spec: { devices: CircuitSpecDevice[] },
  sourceId: number,
  omegas: number[],
): Promise<CircuitAcResult | null> {
  const wasm = await loadEcadWasm();
  const fn = (
    wasm as unknown as {
      circuitAcResponse?: (
        spec: string,
        sourceId: number,
        omegas: Float64Array,
      ) => CircuitAcResult;
    } | null
  )?.circuitAcResponse;
  if (typeof fn !== "function") return null;
  return fn(JSON.stringify(spec), sourceId, new Float64Array(omegas));
}

/** Adjoint gradient-descent tuning toward a filter or DC target. */
export async function circuitTune(
  spec: { devices: CircuitSpecDevice[] },
  tune: {
    filter?: { cutoffHz: number; qFactor: number; sourceId: number; outNode: number };
    dc?: { node: number; dcVoltage: number };
    freeDevices: Array<{ device: number; min?: number; max?: number }>;
    maxIters?: number;
  },
): Promise<CircuitTuneResult | null> {
  const wasm = await loadEcadWasm();
  const fn = (
    wasm as unknown as {
      circuitTune?: (spec: string, tune: string) => CircuitTuneResult;
    } | null
  )?.circuitTune;
  if (typeof fn !== "function") return null;
  return fn(JSON.stringify(spec), JSON.stringify(tune));
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
