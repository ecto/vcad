/**
 * predict_print / record_measurement — the 3DP print-then-measure loop.
 *
 * The one fab rail with zero vendor dependency: the user's own printer is the
 * effector, the human carries the part, calipers and a kitchen scale are the
 * oracle. `predict_print` snapshots what the design CLAIMS (bbox, mass at a
 * density, caller-declared feature dimensions) before printing;
 * `record_measurement` joins the as-built numbers against that snapshot and
 * emits the receipt-vs-reality delta report — per-feature deltas plus the
 * aggregates a printer profile can act on (axis scales, hole undersize, flow).
 *
 * Pure math lives in @vcad/core's print-calibration module; this file is the
 * MCP plumbing. Predictions and reports ride the same warm-instance lifetime
 * as sessions (see session.ts) — `record_measurement` therefore also accepts
 * the full prediction inline, so a cold instance (or a prediction.json saved
 * to disk, as examples/calibration-coupon does) can replay it.
 *
 * ── Two things a measurement can close ─────────────────────────────────────
 *
 * `record_measurement` started life bound to one thing: a `PrintPrediction`
 * from `predict_print`. That left every *other* prediction in the system with
 * no way to be closed — a `vcad.cam-claims/1` over-pins dimension is exactly
 * as measurable as a printed hole, and stayed `Provisional` forever because
 * no tool could hand it a caliper reading.
 *
 * So there are now two paths through this tool, chosen by what the call
 * carries, and they do not interact:
 *
 * - **`measurements` (a map of measurable id → value)** — the original 3DP
 *   path, unchanged: joined against a `PrintPrediction`, out comes the
 *   calibration delta report.
 * - **`claim`** — the receipt path: the reading is bound to a claim on a
 *   deposited claim-family report, the family decides `Holds` or `Violated`,
 *   and the claim comes back on a **measured** basis so the receipt can stop
 *   reading Provisional. For CAM's `gear.over_pins` the same call also returns
 *   the cutter compensation the reading implies.
 *
 * Design doc: docs/plans/2026-07-07-3dp-print-then-measure.md
 */

import type { Engine } from "@vcad/engine";
import {
  buildCalibrationReport,
  fingerprintDocument,
  type CalibrationReport,
  type Measurable,
  type MeasurableAxis,
  type MeasurableFeature,
  type MeasurableKind,
  type MeasurementContext,
  type PrintPrediction,
} from "@vcad/core";
import { getSession } from "./session-core.js";
import { computeInspection } from "./inspect.js";
import { behavior, type ToolDef } from "./tool-def.js";
import {
  bindMeasurement,
  claimReports,
  type ClaimReportEntry,
} from "./claim-registry.js";

const MEASURABLE_KINDS: readonly MeasurableKind[] = ["dimension", "diameter", "mass"];
const MEASURABLE_AXES: readonly MeasurableAxis[] = ["X", "Y", "Z", "XY"];
const MEASURABLE_FEATURES: readonly MeasurableFeature[] = [
  "overall",
  "step",
  "hole",
  "boss",
  "wall",
];

// Warm-instance registries, keyed by document_id — same lifetime model as the
// in-memory session map and the artifact store. NOT durable across cold
// serverless instances; the inline `prediction` arg on record_measurement is
// the durable path.
const predictions = new Map<string, PrintPrediction>();
const reports = new Map<string, CalibrationReport[]>();

/** Test/reset hook — mirrors the session map's lifecycle helpers. */
export function clearPrintCheckState(): void {
  predictions.clear();
  reports.clear();
}

export const predictPrintSchema = {
  type: "object" as const,
  properties: {
    document_id: {
      type: "string" as const,
      description: "Session id from open_document.",
    },
    material_density_kg_m3: {
      type: "number" as const,
      description:
        "Filament density for the mass prediction (e.g. PLA 1240, PETG 1270, ABS 1050). Falls back to the document's material densities when omitted; without either, no mass measurable is emitted.",
    },
    material_name: {
      type: "string" as const,
      description: "Material label recorded in the prediction (e.g. \"PLA\").",
    },
    measurables: {
      type: "array" as const,
      description:
        "Named features the human will measure, with design-intent values — step heights, hole diameters, wall thicknesses. Merged with the auto measurables (bbox_x/y/z, mass). Derive `predicted` from the same parameters that built the geometry so prediction and part cannot drift.",
      items: {
        type: "object" as const,
        properties: {
          id: { type: "string" as const, description: "Stable id, e.g. \"hole_3mm\"." },
          label: {
            type: "string" as const,
            description: "Measurement instruction, e.g. \"Small hole diameter (front-left)\".",
          },
          kind: { type: "string" as const, enum: [...MEASURABLE_KINDS] },
          axis: {
            type: "string" as const,
            enum: [...MEASURABLE_AXES],
            description: "Print-frame axis; \"XY\" for in-plane diameters.",
          },
          feature: {
            type: "string" as const,
            enum: [...MEASURABLE_FEATURES],
            description: "Aggregation bucket (hole/wall feed the offset aggregates).",
          },
          predicted: { type: "number" as const, description: "Design-intent value." },
          unit: { type: "string" as const, enum: ["mm", "g"] },
          tolerance: {
            type: "number" as const,
            description: "± tolerance; defaults to max(0.1mm, 0.2%) for dimensions, 5% for mass.",
          },
        },
        required: ["id", "label", "kind", "predicted"],
      },
    },
  },
  required: ["document_id"],
};

export const recordMeasurementSchema = {
  type: "object" as const,
  properties: {
    document_id: {
      type: "string" as const,
      description:
        "Session id the prediction was taken from. Optional when a full `prediction` is passed inline.",
    },
    measurements: {
      type: "object" as const,
      description:
        "Measured values keyed by measurable id, in each measurable's own unit (mm or g), e.g. {\"bbox_x\": 79.82, \"hole_3mm\": 2.85, \"mass\": 26.4}. A partial set is fine — unmeasured ids are reported as missing, not errors.",
      additionalProperties: { type: "number" as const },
    },
    printer: {
      type: "string" as const,
      description: "Which machine printed the part (e.g. \"Bambu X1C\").",
    },
    material: {
      type: "string" as const,
      description: "Filament actually used (e.g. \"PLA Basic, black\").",
    },
    process: {
      type: "string" as const,
      description: "Free-form process notes: layer height, infill, temperatures.",
    },
    prediction: {
      type: "object" as const,
      description:
        "A full PrintPrediction (as returned by predict_print) to measure against. Overrides the cached prediction — use this to replay a prediction.json after the session's warm instance recycled.",
    },
    claim: {
      type: "string" as const,
      description:
        "Close a receipt claim instead of a print prediction: the claim's name on a deposited claim-family report, e.g. \"gear.over_pins\". Needs document_id and value. The family decides Holds or Violated and the claim moves onto a measured basis, so the receipt can stop reading Provisional. A claim that is arithmetic on a program rather than a dimension of a part (job.no_gouge) is refused, not recorded.",
    },
    claim_report_id: {
      type: "string" as const,
      description:
        "Which deposited report the claim is on (from cam_job / cam_gear's `claims.id`). Optional when the document holds exactly one report carrying that claim.",
    },
    subject: {
      type: "string" as const,
      description:
        "The claim's subject when it has one, e.g. \"20T-m1\" — required when the report claims the same thing about several subjects (a sun and a planet both have an over-pins dimension).",
    },
    kind: {
      type: "string" as const,
      description:
        "How the reading was taken: \"over_pins\" (with pin_diameter), \"caliper\" (with feature), \"span\" (with teeth), or \"hole_diameter\". Defaults to over_pins when pin_diameter is given.",
    },
    value: {
      type: "number" as const,
      description: "The reading, in the claim's own unit.",
    },
    unit: {
      type: "string" as const,
      description:
        "The unit the reading is in. Checked against the claim's unit and refused on a mismatch — an inch reading recorded against a mm claim is a scrapped part, not a rounding error.",
    },
    pin_diameter: {
      type: "number" as const,
      description: "Pin or ball diameter the reading was taken over, mm.",
    },
    feature: {
      type: "string" as const,
      description: "What was measured, for a caliper reading, e.g. \"across the flats\".",
    },
    teeth: {
      type: "number" as const,
      description: "Teeth spanned, for a base-tangent (span) reading.",
    },
    tolerance: {
      type: "number" as const,
      description:
        "Acceptance half-width on |measured − predicted|, in the claim's unit. The claim holds when the difference is inside tolerance + uncertainty. Required: a measurement with no stated tolerance cannot decide anything.",
    },
    uncertainty: {
      type: "number" as const,
      description: "One-sigma uncertainty of the reading, same unit. Default 0.",
    },
    instrument: {
      type: "string" as const,
      description:
        "Instrument provenance, e.g. \"Mitutoyo 293-340 s/n 12345\", \"Ø1.5 gauge pins\". Recorded on the claim — a measurement nobody can trace is not evidence.",
    },
  },
};

type ToolResult = { content: Array<{ type: "text"; text: string }> };

const jsonResult = (value: unknown): ToolResult => ({
  content: [{ type: "text", text: JSON.stringify(value, null, 2) }],
});

function parseMeasurables(raw: unknown): Measurable[] {
  if (raw === undefined) return [];
  if (!Array.isArray(raw)) throw new Error("measurables must be an array");
  return raw.map((entry, i) => {
    const m = entry as Record<string, unknown>;
    const id = String(m.id ?? "");
    const label = String(m.label ?? "");
    const kind = m.kind as MeasurableKind;
    const predicted = Number(m.predicted);
    if (!id || !label) throw new Error(`measurables[${i}]: id and label are required`);
    if (!MEASURABLE_KINDS.includes(kind)) {
      throw new Error(`measurables[${i}]: kind must be one of ${MEASURABLE_KINDS.join(", ")}`);
    }
    if (!Number.isFinite(predicted)) {
      throw new Error(`measurables[${i}]: predicted must be a finite number`);
    }
    if (m.axis !== undefined && !MEASURABLE_AXES.includes(m.axis as MeasurableAxis)) {
      throw new Error(`measurables[${i}]: axis must be one of ${MEASURABLE_AXES.join(", ")}`);
    }
    if (
      m.feature !== undefined &&
      !MEASURABLE_FEATURES.includes(m.feature as MeasurableFeature)
    ) {
      throw new Error(
        `measurables[${i}]: feature must be one of ${MEASURABLE_FEATURES.join(", ")}`,
      );
    }
    const unit = m.unit === undefined ? (kind === "mass" ? "g" : "mm") : m.unit;
    if (unit !== "mm" && unit !== "g") {
      throw new Error(`measurables[${i}]: unit must be "mm" or "g"`);
    }
    return {
      id,
      label,
      kind,
      ...(m.axis !== undefined && { axis: m.axis as MeasurableAxis }),
      ...(m.feature !== undefined && { feature: m.feature as MeasurableFeature }),
      predicted,
      unit,
      ...(m.tolerance !== undefined && { tolerance: Number(m.tolerance) }),
    };
  });
}

/** Snapshot the design's predicted measurables before printing. */
export function predictPrint(input: unknown, engine: Engine): ToolResult {
  const args = (input ?? {}) as Record<string, unknown>;
  const documentId = String(args.document_id ?? "");
  const ir = getSession(documentId);

  const inspection = computeInspection(ir, engine);
  const bbox = inspection.bounding_box;
  const size = {
    x: Math.round((bbox.max.x - bbox.min.x) * 1000) / 1000,
    y: Math.round((bbox.max.y - bbox.min.y) * 1000) / 1000,
    z: Math.round((bbox.max.z - bbox.min.z) * 1000) / 1000,
  };

  const declared = parseMeasurables(args.measurables);

  const auto: Measurable[] = [
    {
      id: "bbox_x",
      label: "Overall length along X (widest span, caliper jaws)",
      kind: "dimension",
      axis: "X",
      feature: "overall",
      predicted: size.x,
      unit: "mm",
    },
    {
      id: "bbox_y",
      label: "Overall depth along Y (widest span, caliper jaws)",
      kind: "dimension",
      axis: "Y",
      feature: "overall",
      predicted: size.y,
      unit: "mm",
    },
    {
      id: "bbox_z",
      label: "Overall height along Z (tallest point off the bed)",
      kind: "dimension",
      axis: "Z",
      feature: "overall",
      predicted: size.z,
      unit: "mm",
    },
  ];

  const assumptions: string[] = [
    "dimensions are model-space; no shrinkage compensation applied",
  ];

  // Mass: explicit density arg wins; else use the document's per-part
  // densities when inspect_cad already resolved them.
  const densityArg = args.material_density_kg_m3;
  let massG: number | undefined;
  let density: number | undefined;
  if (densityArg !== undefined) {
    density = Number(densityArg);
    if (!Number.isFinite(density) || density <= 0) {
      throw new Error("material_density_kg_m3 must be a positive number");
    }
    massG = Math.round(((inspection.volume_mm3 / 1e9) * density) * 1e6) / 1000;
    assumptions.push(
      `mass assumes 100% infill (solid) at ${density} kg/m³ — print solid or ignore the mass row`,
    );
  } else if (inspection.mass_g !== undefined) {
    massG = inspection.mass_g;
    density = inspection.part_masses?.[0]?.density_kg_m3;
    assumptions.push(
      "mass assumes 100% infill (solid) at the document's material densities — print solid or ignore the mass row",
    );
  }
  if (massG !== undefined) {
    auto.push({
      id: "mass",
      label: "Part mass on a scale, grams",
      kind: "mass",
      predicted: massG,
      unit: "g",
    });
  }

  // Declared measurables win on id collision — the caller knows the design.
  const declaredIds = new Set(declared.map((m) => m.id));
  const measurables = [...auto.filter((m) => !declaredIds.has(m.id)), ...declared];

  const materialName = args.material_name !== undefined ? String(args.material_name) : undefined;
  const prediction: PrintPrediction = {
    version: 1,
    document_id: documentId,
    doc_fingerprint: fingerprintDocument(ir),
    created_at: new Date().toISOString(),
    ...((materialName !== undefined || density !== undefined) && {
      material: {
        ...(materialName !== undefined && { name: materialName }),
        ...(density !== undefined && { density_kg_m3: density }),
      },
    }),
    volume_mm3: inspection.volume_mm3,
    bbox_mm: size,
    assumptions,
    measurables,
  };

  predictions.set(documentId, prediction);
  return jsonResult(prediction);
}

// ─── The receipt-claim path ──────────────────────────────────────────────────

/** One claim as it sits in a family's serialized claim set. */
interface StoredClaim {
  name: string;
  subject?: string;
  status: string;
  basis: string;
  value?: number;
  measured?: number;
  unit: string;
}

/** The claims inside a deposited report, or `[]` if it will not parse. */
function storedClaims(entry: ClaimReportEntry): StoredClaim[] {
  try {
    const set = JSON.parse(entry.report) as { claims?: StoredClaim[] };
    return Array.isArray(set.claims) ? set.claims : [];
  } catch {
    return [];
  }
}

/**
 * The reading's `kind`, in the family's own wire form.
 *
 * Fail-closed on the details each kind needs: a pin measurement without the
 * pin diameter cannot be turned back into a tooth thickness, and guessing a
 * diameter would produce a confident, wrong compensation.
 */
function measurementKind(args: Record<string, unknown>): unknown {
  const pin = typeof args.pin_diameter === "number" ? args.pin_diameter : undefined;
  const declared =
    typeof args.kind === "string" ? args.kind.toLowerCase() : pin !== undefined ? "over_pins" : "";
  switch (declared) {
    case "over_pins":
      if (pin === undefined || !(pin > 0)) {
        throw new Error(
          "an over-pins reading needs the `pin_diameter` it was taken with — the prediction was made for a specific pin, and a reading over a different one is a different number.",
        );
      }
      return { OverPins: { pin_diameter: pin } };
    case "caliper": {
      const feature = typeof args.feature === "string" ? args.feature : "";
      if (!feature) {
        throw new Error(
          'a caliper reading needs `feature` — what was measured, e.g. "across the flats".',
        );
      }
      return { Caliper: { feature } };
    }
    case "span": {
      const teeth = typeof args.teeth === "number" ? Math.round(args.teeth) : 0;
      if (teeth <= 0) {
        throw new Error("a span reading needs `teeth` — how many teeth the anvils spanned.");
      }
      return { Span: { teeth } };
    }
    case "hole_diameter":
      return "HoleDiameter";
    default:
      throw new Error(
        'kind must be one of "over_pins", "caliper", "span" or "hole_diameter" (or give pin_diameter and it is taken as over_pins).',
      );
  }
}

/**
 * Bind a caliper/pin/scale reading to a claim on a deposited report.
 *
 * The whole point of the ladder: until this runs, a `Predicted` claim can
 * never be better than `Provisional`, and the receipt it sits in can never
 * roll up better than `provisional` either. This is what closes it — or
 * violates it, which is just as useful and far more common on a first part.
 */
function recordClaimMeasurement(
  args: Record<string, unknown>,
  engine: Engine | undefined,
): ToolResult {
  const claim = String(args.claim);
  const documentId = args.document_id !== undefined ? String(args.document_id) : "";
  if (!documentId) {
    throw new Error(
      "closing a claim needs `document_id` — the claims live on the document the oracle was run against.",
    );
  }
  if (!engine) {
    throw new Error(
      "closing a claim needs the kernel engine; it is unavailable in this context.",
    );
  }
  const value = args.value;
  if (typeof value !== "number" || !Number.isFinite(value)) {
    throw new Error("`value` must be a finite number — the reading off the part.");
  }
  const tolerance = args.tolerance;
  if (typeof tolerance !== "number" || !(tolerance >= 0)) {
    throw new Error(
      "`tolerance` must be a non-negative number: a measurement with no stated acceptance band cannot decide whether a claim holds.",
    );
  }
  const instrument = typeof args.instrument === "string" ? args.instrument.trim() : "";
  if (!instrument) {
    throw new Error(
      '`instrument` is required — a reading nobody can trace back to a tool is not evidence. e.g. "Ø1.5 gauge pins" or "Mitutoyo 293-340".',
    );
  }
  const subject = typeof args.subject === "string" ? args.subject : undefined;

  const doc = getSession(documentId);
  const deposits = claimReports(doc);
  if (deposits.length === 0) {
    throw new Error(
      `document ${documentId} carries no claim reports — run cam_job or cam_gear with this document_id first, so there is a prediction to close.`,
    );
  }

  // Which report. Named explicitly, or the one that actually carries the
  // claim; two candidates is an ambiguity to surface, not to pick from.
  let entry: ClaimReportEntry | undefined;
  if (typeof args.claim_report_id === "string" && args.claim_report_id) {
    entry = deposits.find((r) => r.id === args.claim_report_id);
    if (!entry) {
      throw new Error(
        `no claim report '${String(args.claim_report_id)}' on this document. It holds: ${deposits.map((r) => r.id).join(", ")}.`,
      );
    }
  } else {
    const candidates = deposits.filter((r) =>
      storedClaims(r).some(
        (c) => c.name === claim && (subject === undefined || c.subject === subject),
      ),
    );
    if (candidates.length === 0) {
      throw new Error(
        `no report on this document carries a claim named '${claim}'${subject ? ` for subject '${subject}'` : ""}. Reports here: ${deposits.map((r) => `${r.id} (${r.schema})`).join(", ")}.`,
      );
    }
    if (candidates.length > 1) {
      throw new Error(
        `'${claim}' appears on ${candidates.length} reports (${candidates.map((r) => r.id).join(", ")}) — name one with claim_report_id, or narrow it with subject.`,
      );
    }
    entry = candidates[0];
  }

  // The target claim, before binding: its unit and predicted value are what
  // the reading is checked against.
  const before = storedClaims(entry).filter(
    (c) => c.name === claim && (subject === undefined || c.subject === subject),
  );
  if (before.length === 0) {
    throw new Error(
      `report '${entry.id}' has no claim '${claim}'${subject ? ` for subject '${subject}'` : ""}. It claims: ${storedClaims(entry).map((c) => (c.subject ? `${c.name}[${c.subject}]` : c.name)).join(", ")}.`,
    );
  }
  if (before.length > 1) {
    throw new Error(
      `'${claim}' is claimed for ${before.length} subjects on report '${entry.id}' (${before.map((c) => c.subject ?? "—").join(", ")}) — say which with subject.`,
    );
  }
  const target = before[0];
  // A unit mismatch is not a rounding error, it is a scrapped part.
  if (typeof args.unit === "string" && args.unit && args.unit !== target.unit) {
    throw new Error(
      `'${claim}' is claimed in ${target.unit === "1" ? "a dimensionless unit" : target.unit}, and the reading says ${String(args.unit)}. Convert it rather than recording it in the wrong unit.`,
    );
  }

  const measurement = {
    claim,
    ...(target.subject !== undefined ? { subject: target.subject } : {}),
    kind: measurementKind(args),
    value,
    uncertainty:
      typeof args.uncertainty === "number" && args.uncertainty >= 0 ? args.uncertainty : 0,
    tolerance,
    instrument,
  };

  const bound = bindMeasurement(doc, engine, entry.id, measurement);

  // What the claim says now — the ladder having moved, or not.
  const after = storedClaims(bound.entry).filter((c) => c.name === claim);
  const closed = after.find((c) => c.subject === target.subject) ?? after[0];
  // A compensation claim, when the family derived one, is a NEW prediction
  // for the next part: it supersedes the one just closed and is itself only
  // Provisional. Surfacing it here is what makes the loop a loop.
  const superseding = storedClaims(bound.entry).filter(
    (c) => (c as { supersedes?: string }).supersedes === claim,
  );

  return jsonResult({
    ok: true,
    bound: {
      claim,
      ...(target.subject !== undefined ? { subject: target.subject } : {}),
      report_id: bound.entry.id,
      schema: bound.entry.schema,
      predicted: target.value,
      measured: value,
      unit: target.unit,
      delta:
        typeof target.value === "number"
          ? Number((value - target.value).toFixed(6))
          : undefined,
      status: closed?.status,
      basis: closed?.basis,
      instrument,
    },
    derived: bound.derived,
    superseded_by: superseding.map((c) => ({
      claim: c.name,
      status: c.status,
      basis: c.basis,
      predicted: c.value,
    })),
    note: bound.note,
    next:
      closed?.status === "Violated"
        ? "The part is outside the band. `derived` carries what to change; re-cut, then record the next measurement — the compensated claim is itself only Provisional until that part is measured too."
        : "build_receipt on this document now carries this claim on a measured basis.",
    honesty:
      "One part, one reading. A single measurement inside tolerance closes this claim for this part; it is not a process capability.",
  });
}

/** Record as-built measurements and emit the receipt-vs-reality delta. */
export function recordMeasurement(input: unknown, engine?: Engine): ToolResult {
  const args = (input ?? {}) as Record<string, unknown>;

  // Two paths, chosen by what the call carries. `claim` means the receipt
  // ladder; `measurements` means the print-calibration report. Asking for
  // both at once is a confusion worth surfacing, not silently resolving.
  if (typeof args.claim === "string" && args.claim) {
    if (args.measurements !== undefined) {
      throw new Error(
        "pass either `claim` (close a receipt claim) or `measurements` (join a print prediction), not both — they close different things.",
      );
    }
    return recordClaimMeasurement(args, engine);
  }
  if (args.measurements === undefined) {
    throw new Error(
      "pass `measurements` (a map of measurable id → value, against a predict_print snapshot) or `claim` (a claim name on a deposited claim report, with value/tolerance/instrument).",
    );
  }

  const documentId = args.document_id !== undefined ? String(args.document_id) : undefined;

  let prediction: PrintPrediction | undefined;
  if (args.prediction !== undefined && args.prediction !== null) {
    const p = args.prediction as PrintPrediction;
    if (!Array.isArray(p.measurables) || typeof p.doc_fingerprint !== "string") {
      throw new Error(
        "prediction must be a PrintPrediction as returned by predict_print (measurables + doc_fingerprint)",
      );
    }
    prediction = p;
  } else if (documentId !== undefined) {
    prediction = predictions.get(documentId);
  }
  if (!prediction) {
    throw new Error(
      documentId
        ? `No prediction recorded for document ${documentId} on this instance. ` +
          `Run predict_print first, or pass the saved prediction JSON inline via the \`prediction\` arg.`
        : "Pass document_id (after predict_print) or a full `prediction` object.",
    );
  }

  const rawMeasurements = args.measurements;
  if (
    rawMeasurements === null ||
    typeof rawMeasurements !== "object" ||
    Array.isArray(rawMeasurements)
  ) {
    throw new Error("measurements must be an object of {measurable_id: number}");
  }
  const measurements: Record<string, number> = {};
  for (const [id, value] of Object.entries(rawMeasurements as Record<string, unknown>)) {
    const n = Number(value);
    if (!Number.isFinite(n)) {
      throw new Error(`measurements.${id} must be a finite number (got ${String(value)})`);
    }
    measurements[id] = n;
  }
  if (Object.keys(measurements).length === 0) {
    throw new Error("measurements is empty — nothing to record");
  }

  const context: MeasurementContext = {
    ...(args.printer !== undefined && { printer: String(args.printer) }),
    ...(args.material !== undefined && { material: String(args.material) }),
    ...(args.process !== undefined && { process: String(args.process) }),
    measured_at: new Date().toISOString(),
  };

  // Staleness: only checkable when the session is still open here.
  let currentFingerprint: string | undefined;
  const sessionId = documentId ?? prediction.document_id;
  if (sessionId !== undefined) {
    try {
      currentFingerprint = fingerprintDocument(getSession(sessionId));
    } catch {
      // Session gone (cold instance / closed) — report without staleness info.
    }
  }

  const report = buildCalibrationReport(prediction, measurements, {
    context,
    ...(currentFingerprint !== undefined && {
      current_doc_fingerprint: currentFingerprint,
    }),
  });

  if (sessionId !== undefined) {
    const list = reports.get(sessionId) ?? [];
    list.push(report);
    reports.set(sessionId, list);
  }

  return jsonResult(report);
}

export const toolDefs: ToolDef[] = [
  {
    name: "predict_print",
    pack: "print",
    description:
      "Snapshot the design's predicted measurables BEFORE 3D-printing it: kernel-evaluated bbox and mass (at a filament density), plus caller-declared feature dimensions (step heights, hole diameters, wall thicknesses) with design-intent values. Returns a PrintPrediction — save it; after printing, record_measurement joins caliper/scale readings against it. The prediction doubles as the guided measurement worksheet (each measurable carries a human instruction label).",
    inputSchema: predictPrintSchema,
    handler: (a, c) => predictPrint(a, c.engine),
    behavior: behavior({}),
  },
  {
    name: "record_measurement",
    pack: "print",
    description:
      "Close a prediction with a reading off the real part. Two things it can close. (1) `measurements`: as-built caliper/scale numbers against a predict_print snapshot, answering the receipt-vs-reality delta report — per-feature deltas with tolerances, per-axis scale factors (X/Y/Z shrinkage), hole undersize and thin-wall flow offsets, and concrete printer-profile suggestions. Accepts the prediction inline (from a saved prediction.json) when the session's warm instance is gone; partial measurements are fine. (2) `claim`: a named claim on a claim-family report deposited by cam_job / cam_gear — the family decides Holds or Violated, the claim moves onto a measured basis so build_receipt stops reading Provisional, and for a gear's over-pins dimension the same call returns the cutter compensation the reading implies for the next part. Fails closed: a measurement aimed at a claim that is arithmetic on a program rather than a dimension of a part is refused, as is one in the wrong unit or with no stated tolerance or instrument.",
    inputSchema: recordMeasurementSchema,
    handler: (a, c) => recordMeasurement(a, c.engine),
    behavior: behavior({}),
  },
];
