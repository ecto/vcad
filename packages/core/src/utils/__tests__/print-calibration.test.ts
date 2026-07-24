import { describe, it, expect, beforeAll } from "vitest";
import { getKernelWasm } from "../../wasm-singleton.js";
import {
  buildCalibrationReport,
  defaultTolerance,
  fingerprintDocument,
  type PrintPrediction,
} from "../print-calibration.js";

/** A coupon-shaped prediction: XY spans, Z steps, holes, walls, mass. */
function prediction(): PrintPrediction {
  return {
    version: 1,
    document_id: "doc1",
    doc_fingerprint: fingerprintDocument({ nodes: { "1": { op: "Cube" } } }),
    created_at: "2026-07-07T00:00:00.000Z",
    material: { name: "PLA", density_kg_m3: 1240 },
    volume_mm3: 20000,
    bbox_mm: { x: 80, y: 32, z: 12 },
    assumptions: ["mass assumes 100% infill"],
    measurables: [
      { id: "bbox_x", label: "Overall X", kind: "dimension", axis: "X", feature: "overall", predicted: 80, unit: "mm" },
      { id: "bbox_y", label: "Overall Y", kind: "dimension", axis: "Y", feature: "overall", predicted: 32, unit: "mm" },
      { id: "step_z_6", label: "Step 1 height", kind: "dimension", axis: "Z", feature: "step", predicted: 6, unit: "mm" },
      { id: "step_z_12", label: "Step 4 height", kind: "dimension", axis: "Z", feature: "step", predicted: 12, unit: "mm" },
      { id: "hole_3mm", label: "Small hole", kind: "diameter", axis: "XY", feature: "hole", predicted: 3, unit: "mm" },
      { id: "hole_8mm", label: "Large hole", kind: "diameter", axis: "XY", feature: "hole", predicted: 8, unit: "mm" },
      { id: "fin_1_2", label: "Middle fin", kind: "dimension", axis: "X", feature: "wall", predicted: 1.2, unit: "mm" },
      { id: "mass", label: "Part mass", kind: "mass", predicted: 24.8, unit: "g" },
    ],
  };
}

describe("buildCalibrationReport", () => {
  // The wrapper delegates to vcad-kernel-calibration via kernel-wasm.
  beforeAll(async () => {
    await getKernelWasm();
  });

  it("joins measurements to measurables and computes signed deltas", () => {
    const report = buildCalibrationReport(prediction(), {
      bbox_x: 79.82,
      bbox_y: 31.94,
      step_z_6: 6.02,
      step_z_12: 12.05,
      hole_3mm: 2.85,
      hole_8mm: 7.88,
      fin_1_2: 1.26,
      mass: 24.1,
    });

    const bx = report.rows.find((r) => r.id === "bbox_x")!;
    expect(bx.delta).toBeCloseTo(-0.18, 6);
    expect(bx.delta_pct).toBeCloseTo(-0.225, 3);
    expect(report.missing).toEqual([]);
    expect(report.unknown).toEqual([]);
    expect(report.fingerprintAlgo).toBe("fnv1a-128");
  });

  it("fits per-axis scale factors by least squares", () => {
    // Everything exactly 0.5% small in XY, exact in Z.
    const p = prediction();
    const report = buildCalibrationReport(p, {
      bbox_x: 80 * 0.995,
      bbox_y: 32 * 0.995,
      step_z_6: 6,
      step_z_12: 12,
      hole_3mm: 3 * 0.995,
      hole_8mm: 8 * 0.995,
      fin_1_2: 1.2 * 0.995,
      mass: 24.8,
    });
    const x = report.aggregates.axis_scales.find((s) => s.axis === "X")!;
    const z = report.aggregates.axis_scales.find((s) => s.axis === "Z")!;
    expect(x.scale).toBeCloseTo(0.995, 4);
    expect(z.scale).toBeCloseTo(1.0, 4);
    // Holes and walls never enter the scale fits — their systematic offsets
    // (undersize, flow) would masquerade as shrinkage. The only XY-axis rows
    // here are holes, so no XY fit is emitted, and the X fit excludes the fin.
    expect(report.aggregates.axis_scales.find((s) => s.axis === "XY")).toBeUndefined();
    expect(x.n).toBe(1);
    // 0.5% off should surface a shrinkage-compensation suggestion.
    expect(report.suggestions.some((s) => s.includes("shrinkage/scale compensation"))).toBe(true);
  });

  it("aggregates hole undersize and wall offset separately", () => {
    const report = buildCalibrationReport(prediction(), {
      hole_3mm: 2.8, // −0.2
      hole_8mm: 7.9, // −0.1
      fin_1_2: 1.3, // +0.1 (walls fat = flow high)
    });
    expect(report.aggregates.hole_offset_mm).toBeCloseTo(-0.15, 6);
    expect(report.aggregates.wall_offset_mm).toBeCloseTo(0.1, 6);
    expect(report.suggestions.some((s) => s.includes("undersize"))).toBe(true);
    expect(report.suggestions.some((s) => s.includes("flow ratio"))).toBe(true);
  });

  it("tracks missing and unknown measurement ids without failing", () => {
    const report = buildCalibrationReport(prediction(), {
      bbox_x: 79.9,
      typo_id: 5.0,
    });
    expect(report.missing).toContain("mass");
    expect(report.missing).toContain("hole_3mm");
    expect(report.unknown).toEqual(["typo_id"]);
    expect(report.rows).toHaveLength(1);
  });

  it("verdicts: pass when all within tolerance, fail on gross error", () => {
    const perfect = buildCalibrationReport(prediction(), {
      bbox_x: 80.02,
      bbox_y: 32.01,
      step_z_6: 6.01,
      step_z_12: 11.98,
      hole_3mm: 3.02,
      hole_8mm: 7.95,
      fin_1_2: 1.21,
      mass: 24.9,
    });
    expect(perfect.verdict).toBe("pass");

    // bbox_x tolerance is max(0.1, 0.2%·80)=0.16; 3mm off is >3× — gross.
    const gross = buildCalibrationReport(prediction(), { bbox_x: 77.0 });
    expect(gross.verdict).toBe("fail");
  });

  it("flags a stale pairing when the document changed after prediction", () => {
    const p = prediction();
    const report = buildCalibrationReport(
      p,
      { bbox_x: 79.9 },
      { current_doc_fingerprint: fingerprintDocument({ nodes: { "1": { op: "Sphere" } } }) },
    );
    expect(report.stale).toBe(true);
    expect(report.summary).toContain("STALE");
  });

  it("mass rows use the 5% default tolerance and skip axis fits", () => {
    expect(defaultTolerance("mass", 24.8)).toBeCloseTo(1.24, 6);
    expect(defaultTolerance("dimension", 80)).toBeCloseTo(0.16, 6);
    expect(defaultTolerance("dimension", 3)).toBeCloseTo(0.1, 6); // floor

    const report = buildCalibrationReport(prediction(), { mass: 23.9 });
    expect(report.aggregates.mass).toEqual({
      predicted_g: 24.8,
      measured_g: 23.9,
      delta_pct: expect.closeTo(-3.629, 3),
    });
    expect(report.aggregates.axis_scales).toEqual([]);
    expect(report.rows[0]!.within_tolerance).toBe(true);
  });

  it("fingerprintDocument is key-order independent", () => {
    expect(fingerprintDocument({ a: 1, b: { c: 2, d: 3 } })).toBe(
      fingerprintDocument({ b: { d: 3, c: 2 }, a: 1 }),
    );
    expect(fingerprintDocument({ a: 1 })).not.toBe(fingerprintDocument({ a: 2 }));
  });
});
