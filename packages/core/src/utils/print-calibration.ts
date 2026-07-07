/**
 * Print-then-measure calibration — the 3DP receipt-vs-reality delta engine.
 *
 * The user's own printer is the one fab rail with zero vendor dependency, so
 * every print can capture a (predicted, measured) pair for near-zero cost.
 * This module owns the pure computation of that loop: a `PrintPrediction`
 * snapshot taken before printing, a set of caliper/scale measurements taken
 * after, and the `CalibrationReport` that joins them — per-feature deltas
 * plus the aggregates a printer profile can actually act on (axis scale
 * factors, hole undersize, wall/flow offset).
 *
 * No I/O, no kernel, no MCP — same layering as the receipt engine next door.
 * The MCP tools (`predict_print`, `record_measurement`) and the
 * examples/calibration-coupon scripts wrap this.
 *
 * Design doc: docs/plans/2026-07-07-3dp-print-then-measure.md
 */

import { hashHex, HASH_ALGO } from "./receipt/hash.js";

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

/** Recursively sort object keys so serialization is order-independent. */
function canonicalize(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(canonicalize);
  if (value && typeof value === "object") {
    const out: Record<string, unknown> = {};
    for (const k of Object.keys(value as Record<string, unknown>).sort()) {
      out[k] = canonicalize((value as Record<string, unknown>)[k]);
    }
    return out;
  }
  return value;
}

/** Content fingerprint of a document IR (or any JSON value). Deterministic
 *  across key order; used to pair a measurement with the exact design it
 *  measured. */
export function fingerprintDocument(doc: unknown): string {
  return hashHex(JSON.stringify(canonicalize(doc)));
}

/** Default ± tolerance for a measurable that doesn't declare one:
 *  dimensions/diameters get a well-tuned-FDM envelope, mass gets 5%. */
export function defaultTolerance(kind: MeasurableKind, predicted: number): number {
  if (kind === "mass") return Math.abs(predicted) * 0.05;
  return Math.max(0.1, Math.abs(predicted) * 0.002);
}

const round = (n: number, places = 4): number => {
  const p = 10 ** places;
  return Math.round(n * p) / p;
};

/** Least-squares scale through the origin: minimizes Σ(measured − s·predicted)². */
function fitScale(pairs: Array<{ predicted: number; measured: number }>): number {
  let num = 0;
  let den = 0;
  for (const p of pairs) {
    num += p.measured * p.predicted;
    den += p.predicted * p.predicted;
  }
  return den > 0 ? num / den : 1;
}

function mean(xs: number[]): number {
  return xs.reduce((a, b) => a + b, 0) / xs.length;
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
  const context = options?.context ?? {};
  const recordedAt = options?.recorded_at ?? new Date().toISOString();

  const rows: DeltaRow[] = [];
  const missing: string[] = [];
  const seen = new Set<string>();

  for (const m of prediction.measurables) {
    seen.add(m.id);
    const value = measurements[m.id];
    if (value === undefined || value === null || !Number.isFinite(value)) {
      missing.push(m.id);
      continue;
    }
    const tolerance = m.tolerance ?? defaultTolerance(m.kind, m.predicted);
    const delta = value - m.predicted;
    rows.push({
      id: m.id,
      label: m.label,
      kind: m.kind,
      ...(m.axis !== undefined && { axis: m.axis }),
      ...(m.feature !== undefined && { feature: m.feature }),
      predicted: m.predicted,
      measured: value,
      unit: m.unit,
      delta: round(delta),
      delta_pct: m.predicted !== 0 ? round((100 * delta) / m.predicted) : 0,
      tolerance: round(tolerance),
      within_tolerance: Math.abs(delta) <= tolerance,
    });
  }

  const unknown = Object.keys(measurements).filter((id) => !seen.has(id));

  // ── Aggregates ────────────────────────────────────────────────────────
  // Axis scale fits use only span-like rows (overall, steps, undeclared).
  // Holes, bosses, and thin walls carry systematic process offsets
  // (undersize, over-extrusion) that have their own aggregates below —
  // letting them into the fit would misread flow error as shrinkage.
  const scaleExcluded: ReadonlySet<string> = new Set(["hole", "boss", "wall"]);
  const axisScales: AxisScale[] = [];
  for (const axis of ["X", "Y", "Z", "XY"] as const) {
    const pairs = rows.filter(
      (r) =>
        r.kind !== "mass" &&
        r.axis === axis &&
        r.predicted > 0 &&
        (r.feature === undefined || !scaleExcluded.has(r.feature)),
    );
    if (pairs.length === 0) continue;
    axisScales.push({ axis, n: pairs.length, scale: round(fitScale(pairs), 5) });
  }

  const holeDeltas = rows
    .filter((r) => r.feature === "hole" && r.kind === "diameter")
    .map((r) => r.delta);
  const wallDeltas = rows
    .filter((r) => r.feature === "wall")
    .map((r) => r.delta);
  const massRow = rows.find((r) => r.kind === "mass");

  const aggregates: CalibrationReport["aggregates"] = { axis_scales: axisScales };
  if (holeDeltas.length > 0) aggregates.hole_offset_mm = round(mean(holeDeltas));
  if (wallDeltas.length > 0) aggregates.wall_offset_mm = round(mean(wallDeltas));
  if (massRow) {
    aggregates.mass = {
      predicted_g: massRow.predicted,
      measured_g: massRow.measured,
      delta_pct: massRow.delta_pct,
    };
  }

  // ── Suggestions — the aggregates translated into profile knobs ────────
  const suggestions: string[] = [];
  const inPlane = axisScales.filter((s) => s.axis === "X" || s.axis === "Y" || s.axis === "XY");
  if (inPlane.length > 0) {
    const s = mean(inPlane.map((a) => a.scale));
    if (Math.abs(s - 1) > 0.001) {
      const comp = round(100 / s, 2);
      suggestions.push(
        `XY prints ${s < 1 ? "small" : "large"} by ${round(Math.abs(1 - s) * 100, 2)}% — ` +
          `set shrinkage/scale compensation to ${comp}%`,
      );
    }
  }
  const zScale = axisScales.find((s) => s.axis === "Z");
  if (zScale && Math.abs(zScale.scale - 1) > 0.001) {
    suggestions.push(
      `Z prints ${zScale.scale < 1 ? "short" : "tall"} by ${round(Math.abs(1 - zScale.scale) * 100, 2)}% — ` +
        `check first-layer squish and Z scale compensation`,
    );
  }
  if (aggregates.hole_offset_mm !== undefined && aggregates.hole_offset_mm < -0.05) {
    suggestions.push(
      `holes print ${round(-aggregates.hole_offset_mm, 2)}mm undersize — ` +
        `enable hole compensation or drill/ream to size`,
    );
  }
  if (aggregates.wall_offset_mm !== undefined && Math.abs(aggregates.wall_offset_mm) > 0.05) {
    suggestions.push(
      `thin walls print ${round(Math.abs(aggregates.wall_offset_mm), 2)}mm ` +
        `${aggregates.wall_offset_mm > 0 ? "thick — lower" : "thin — raise"} the flow ratio`,
    );
  }
  if (massRow && !massRow.within_tolerance) {
    suggestions.push(
      `mass is ${round(Math.abs(massRow.delta_pct), 1)}% ${massRow.delta > 0 ? "over" : "under"} ` +
        `prediction — check infill density and material density (assumed ` +
        `${prediction.material?.density_kg_m3 ?? "unknown"} kg/m³)`,
    );
  }

  // ── Verdict ───────────────────────────────────────────────────────────
  const out = rows.filter((r) => !r.within_tolerance);
  const gross = rows.some(
    (r) => Math.abs(r.delta) > 3 * r.tolerance && r.tolerance > 0,
  );
  let verdict: CalibrationVerdict = "pass";
  if (out.length > 0) verdict = "attention";
  if (gross || (rows.length > 0 && out.length > rows.length / 2)) verdict = "fail";

  const stale =
    options?.current_doc_fingerprint !== undefined &&
    options.current_doc_fingerprint !== prediction.doc_fingerprint;

  const bits: string[] = [
    `${rows.length - out.length}/${rows.length} within tolerance`,
  ];
  for (const s of inPlane.length > 0
    ? [`XY scale ${round(mean(inPlane.map((a) => a.scale)) * 100, 2)}%`]
    : []) {
    bits.push(s);
  }
  if (zScale) bits.push(`Z scale ${round(zScale.scale * 100, 2)}%`);
  if (aggregates.hole_offset_mm !== undefined) {
    bits.push(`holes ${aggregates.hole_offset_mm > 0 ? "+" : ""}${round(aggregates.hole_offset_mm, 2)}mm`);
  }
  if (stale) bits.push("STALE — design changed after prediction");

  return {
    version: 1,
    ...(prediction.document_id !== undefined && { document_id: prediction.document_id }),
    doc_fingerprint: prediction.doc_fingerprint,
    stale,
    prediction_created_at: prediction.created_at,
    recorded_at: recordedAt,
    context,
    rows,
    missing,
    unknown,
    aggregates,
    suggestions,
    verdict,
    summary: bits.join("; "),
    fingerprintAlgo: HASH_ALGO,
  };
}
