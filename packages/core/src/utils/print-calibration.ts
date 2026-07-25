/**
 * Print-then-measure calibration — the 3DP receipt-vs-reality delta engine.
 *
 * The user's own printer is the one fab rail with zero vendor dependency, so
 * every print can capture a (predicted, measured) pair for near-zero cost.
 * This module is now a thin wrapper: the pure computation (per-feature
 * deltas, axis scale fits, hole/wall offsets, verdicts, profile suggestions,
 * document fingerprinting) lives in the Rust crate
 * `vcad-kernel-calibration`, reached through the kernel-wasm bindings in
 * `crates/vcad-kernel-wasm/src/calibration.rs`. The Rust port is pinned
 * bit-for-bit against the original TS on fixture data
 * (`crates/vcad-kernel-calibration/tests/ts_parity.rs`).
 *
 * All three APIs REQUIRE the wasm singleton to be initialized
 * (`getKernelWasm()` / `Engine.init()`); they throw a descriptive error
 * otherwise — deliberate, matching the expressions wrapper: a silent TS
 * fallback would reintroduce the dual-implementation drift this port
 * removed. The MCP tools (`predict_print`, `record_measurement`) and the
 * examples/calibration-coupon scripts wrap this.
 *
 * Design doc: docs/plans/2026-07-07-3dp-print-then-measure.md
 */

import { getKernelWasmSync } from "../wasm-singleton.js";

/** What kind of physical quantity a measurable is. */
export type MeasurableKind = "dimension" | "diameter" | "mass";

/** Print-frame axis a linear measurable lies along ("XY" = in-plane, e.g. a
 *  hole diameter — holes and bosses deform isotropically in the layer plane). */
export type MeasurableAxis = "X" | "Y" | "Z" | "XY";

/** Aggregation bucket for a measurable — which calibration signature it
 *  feeds. `hole` and `wall` power the offset aggregates; `overall`/`step`/
 *  `boss` participate only in the axis scale fits. */
export type MeasurableFeature =
  | "overall"
  | "step"
  | "hole"
  | "boss"
  | "wall";

/** One thing the human will measure on the printed part. */
export interface Measurable {
  /** Stable id joining prediction to measurement (e.g. "hole_3mm"). */
  id: string;
  /** Human instruction — what to measure and where. */
  label: string;
  kind: MeasurableKind;
  /** Required for dimensions/diameters; meaningless for mass. */
  axis?: MeasurableAxis;
  feature?: MeasurableFeature;
  /** Design-intent value in `unit`. */
  predicted: number;
  unit: "mm" | "g";
  /** ± tolerance in `unit`; defaulted per kind when omitted. */
  tolerance?: number;
}

/** The pre-print snapshot of everything the design claims to be. */
export interface PrintPrediction {
  version: 1;
  document_id?: string;
  /** fnv1a-128 of the canonicalized document IR — staleness detection. */
  doc_fingerprint: string;
  created_at: string;
  material?: { name?: string; density_kg_m3?: number };
  /** Kernel-evaluated solid volume. */
  volume_mm3: number;
  bbox_mm: { x: number; y: number; z: number };
  /** Honest caveats ("mass assumes 100% infill", …). */
  assumptions: string[];
  measurables: Measurable[];
}

/** Where and how the physical part was made and measured. */
export interface MeasurementContext {
  printer?: string;
  material?: string;
  /** Free-form process notes: layer height, infill, temperature… */
  process?: string;
  measured_at?: string;
}

/** One joined (predicted, measured) row of the report. */
export interface DeltaRow {
  id: string;
  label: string;
  kind: MeasurableKind;
  axis?: MeasurableAxis;
  feature?: MeasurableFeature;
  predicted: number;
  measured: number;
  unit: "mm" | "g";
  /** measured − predicted. */
  delta: number;
  /** 100 · delta / predicted. */
  delta_pct: number;
  tolerance: number;
  within_tolerance: boolean;
}

/** Least-squares fit measured ≈ scale·predicted over one axis's dimensions. */
export interface AxisScale {
  axis: MeasurableAxis;
  /** Number of rows in the fit. */
  n: number;
  scale: number;
}

export type CalibrationVerdict = "pass" | "attention" | "fail";

/** The receipt-vs-reality delta artifact. */
export interface CalibrationReport {
  version: 1;
  document_id?: string;
  doc_fingerprint: string;
  /** True when the document changed between prediction and recording. */
  stale: boolean;
  prediction_created_at: string;
  recorded_at: string;
  context: MeasurementContext;
  rows: DeltaRow[];
  /** Measurable ids the human never measured. */
  missing: string[];
  /** Measurement ids that match no measurable. */
  unknown: string[];
  aggregates: {
    axis_scales: AxisScale[];
    /** Mean(measured − predicted) over hole diameters, mm. */
    hole_offset_mm?: number;
    /** Mean(measured − predicted) over thin walls, mm. */
    wall_offset_mm?: number;
    mass?: { predicted_g: number; measured_g: number; delta_pct: number };
  };
  /** Printer-profile corrections derived from the aggregates. */
  suggestions: string[];
  verdict: CalibrationVerdict;
  summary: string;
  fingerprintAlgo: string;
}

/** The calibration surface of the kernel-wasm module. */
interface CalibrationWasm {
  calibrationFingerprintDocument(docJson: string): string;
  calibrationDefaultTolerance(kind: string, predicted: number): number;
  buildCalibrationReportJson(
    predictionJson: string,
    measurementsJson: string,
    optionsJson: string,
  ): string;
}

function wasm(): CalibrationWasm {
  const mod = getKernelWasmSync();
  if (!mod) {
    throw new Error(
      "print-calibration requires the kernel WASM module — await getKernelWasm() (or Engine.init()) before calling",
    );
  }
  return mod as unknown as CalibrationWasm;
}

/** Content fingerprint of a document IR (or any JSON value). Deterministic
 *  across key order; used to pair a measurement with the exact design it
 *  measured. */
export function fingerprintDocument(doc: unknown): string {
  return wasm().calibrationFingerprintDocument(JSON.stringify(doc));
}

/** Default ± tolerance for a measurable that doesn't declare one:
 *  dimensions/diameters get a well-tuned-FDM envelope, mass gets 5%. */
export function defaultTolerance(kind: MeasurableKind, predicted: number): number {
  return wasm().calibrationDefaultTolerance(kind, predicted);
}

/** Join a prediction with measured values into the delta report.
 *
 *  `measurements` maps measurable id → measured value in the measurable's
 *  own unit. Ids present in only one side land in `missing`/`unknown`
 *  rather than failing — a partial worksheet is still data. */
export function buildCalibrationReport(
  prediction: PrintPrediction,
  measurements: Record<string, number>,
  options?: {
    context?: MeasurementContext;
    /** Fingerprint of the document as it exists NOW (for staleness). */
    current_doc_fingerprint?: string;
    recorded_at?: string;
  },
): CalibrationReport {
  const wire = {
    ...(options?.context !== undefined && { context: options.context }),
    ...(options?.current_doc_fingerprint !== undefined && {
      current_doc_fingerprint: options.current_doc_fingerprint,
    }),
    // The kernel has no clock — stamp the default here.
    recorded_at: options?.recorded_at ?? new Date().toISOString(),
  };
  // Entries (not the object) so the Rust side sees insertion order and
  // `unknown` preserves it; non-finite values JSON-stringify to null, which
  // Rust counts as missing — same as the original TS.
  const json = wasm().buildCalibrationReportJson(
    JSON.stringify(prediction),
    JSON.stringify(Object.entries(measurements)),
    JSON.stringify(wire),
  );
  return JSON.parse(json) as CalibrationReport;
}
