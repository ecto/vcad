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
  },
  required: ["measurements"],
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

/** Record as-built measurements and emit the receipt-vs-reality delta. */
export function recordMeasurement(input: unknown): ToolResult {
  const args = (input ?? {}) as Record<string, unknown>;
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
      "Record as-built measurements (caliper dimensions, scale mass) of a printed part against its predict_print snapshot and emit the receipt-vs-reality delta report: per-feature deltas with tolerances, per-axis scale factors (X/Y/Z shrinkage), hole undersize and thin-wall flow offsets, and concrete printer-profile suggestions. Accepts the prediction inline (from a saved prediction.json) when the session's warm instance is gone. Partial measurements are fine.",
    inputSchema: recordMeasurementSchema,
    handler: (a) => recordMeasurement(a),
    behavior: behavior({}),
  },
];
