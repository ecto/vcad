/**
 * Tests for the 3DP print-then-measure loop: predict_print (pre-print
 * snapshot of the design's measurables) and record_measurement (as-built
 * capture → receipt-vs-reality delta report).
 */

import { describe, it, expect, beforeAll, beforeEach } from "vitest";
import { Engine } from "@vcad/engine";
import type { Document } from "@vcad/ir";
import {
  predictPrint,
  recordMeasurement,
  clearPrintCheckState,
} from "../tools/print-check.js";
import { openDocument, documents } from "../tools/session.js";

let engine: Engine;

/** 20×10×5 plate — volume 1000 mm³, so PLA (1240 kg/m³) mass is 1.24 g. */
function makePlateDoc(): Document {
  return {
    version: "0.1",
    nodes: {
      "1": {
        id: 1,
        name: "Plate",
        op: { type: "Cube", size: { x: 20, y: 10, z: 5 } },
      },
    },
    materials: {},
    part_materials: {},
    roots: [{ root: 1, material: "default" }],
  };
}

function openWith(doc: Document): string {
  const open = openDocument({ initial: doc });
  return JSON.parse(open.content[0].text).document_id as string;
}

const parse = (r: { content: Array<{ type: "text"; text: string }> }) =>
  JSON.parse(r.content[0].text);

beforeAll(async () => {
  engine = await Engine.init();
});

beforeEach(() => {
  documents.clear();
  clearPrintCheckState();
});

describe("predict_print", () => {
  it("snapshots kernel bbox and density-derived mass as measurables", () => {
    const documentId = openWith(makePlateDoc());
    const prediction = parse(
      predictPrint(
        { document_id: documentId, material_density_kg_m3: 1240, material_name: "PLA" },
        engine,
      ),
    );

    expect(prediction.version).toBe(1);
    expect(prediction.bbox_mm).toEqual({ x: 20, y: 10, z: 5 });
    expect(prediction.volume_mm3).toBeCloseTo(1000, 3);
    expect(prediction.material).toEqual({ name: "PLA", density_kg_m3: 1240 });
    expect(prediction.doc_fingerprint).toMatch(/^[0-9a-f]{32}$/);

    const ids = prediction.measurables.map((m: { id: string }) => m.id);
    expect(ids).toEqual(["bbox_x", "bbox_y", "bbox_z", "mass"]);
    const mass = prediction.measurables.find((m: { id: string }) => m.id === "mass");
    expect(mass.predicted).toBeCloseTo(1.24, 3);
    expect(mass.unit).toBe("g");
    // The 100%-infill caveat must be spelled out.
    expect(prediction.assumptions.join(" ")).toContain("100% infill");
  });

  it("merges declared measurables, letting them win id collisions", () => {
    const documentId = openWith(makePlateDoc());
    const prediction = parse(
      predictPrint(
        {
          document_id: documentId,
          measurables: [
            {
              id: "hole_3mm",
              label: "Small hole diameter",
              kind: "diameter",
              axis: "XY",
              feature: "hole",
              predicted: 3,
            },
            { id: "bbox_x", label: "Custom X", kind: "dimension", axis: "X", predicted: 19.5 },
          ],
        },
        engine,
      ),
    );
    const ids = prediction.measurables.map((m: { id: string }) => m.id);
    expect(ids).toContain("hole_3mm");
    // Declared bbox_x replaced the auto one.
    const bx = prediction.measurables.filter((m: { id: string }) => m.id === "bbox_x");
    expect(bx).toHaveLength(1);
    expect(bx[0].predicted).toBe(19.5);
    // No density anywhere → no mass measurable.
    expect(ids).not.toContain("mass");
    const hole = prediction.measurables.find((m: { id: string }) => m.id === "hole_3mm");
    expect(hole.unit).toBe("mm"); // defaulted from kind
  });

  it("rejects malformed declared measurables", () => {
    const documentId = openWith(makePlateDoc());
    expect(() =>
      predictPrint(
        {
          document_id: documentId,
          measurables: [{ id: "x", label: "X", kind: "distance", predicted: 1 }],
        },
        engine,
      ),
    ).toThrow(/kind must be one of/);
  });
});

describe("record_measurement", () => {
  it("joins measurements against the cached prediction and reports deltas", () => {
    const documentId = openWith(makePlateDoc());
    predictPrint({ document_id: documentId, material_density_kg_m3: 1240 }, engine);

    const report = parse(
      recordMeasurement({
        document_id: documentId,
        measurements: { bbox_x: 19.9, bbox_y: 9.95, bbox_z: 5.05, mass: 1.2 },
        printer: "Bambu X1C",
        material: "PLA Basic black",
        process: "0.2mm layers, 100% infill",
      }),
    );

    expect(report.version).toBe(1);
    expect(report.stale).toBe(false);
    expect(report.context.printer).toBe("Bambu X1C");
    expect(report.rows).toHaveLength(4);
    const bx = report.rows.find((r: { id: string }) => r.id === "bbox_x");
    expect(bx.delta).toBeCloseTo(-0.1, 6);
    expect(report.aggregates.mass.measured_g).toBe(1.2);
    expect(["pass", "attention", "fail"]).toContain(report.verdict);
  });

  it("flags a stale report when the document changed after prediction", () => {
    const documentId = openWith(makePlateDoc());
    predictPrint({ document_id: documentId }, engine);

    // Mutate the session document in place — same id, different content.
    const doc = documents.get(documentId)!;
    (doc.nodes["1"]!.op as { size: { x: number } }).size.x = 25;

    const report = parse(
      recordMeasurement({ document_id: documentId, measurements: { bbox_x: 19.9 } }),
    );
    expect(report.stale).toBe(true);
    expect(report.summary).toContain("STALE");
  });

  it("replays an inline prediction with no session state at all", () => {
    const documentId = openWith(makePlateDoc());
    const prediction = parse(
      predictPrint({ document_id: documentId, material_density_kg_m3: 1240 }, engine),
    );

    // Simulate a cold instance: both registries wiped.
    documents.clear();
    clearPrintCheckState();

    const report = parse(
      recordMeasurement({
        measurements: { bbox_x: 19.8, mass: 1.19 },
        prediction,
      }),
    );
    expect(report.rows).toHaveLength(2);
    expect(report.missing).toContain("bbox_y");
    // Session is gone → staleness is unknowable, not asserted.
    expect(report.stale).toBe(false);
  });

  it("errors helpfully when there is no prediction to measure against", () => {
    const documentId = openWith(makePlateDoc());
    expect(() =>
      recordMeasurement({ document_id: documentId, measurements: { bbox_x: 19.9 } }),
    ).toThrow(/predict_print first|prediction/);
    expect(() => recordMeasurement({ measurements: { bbox_x: 19.9 } })).toThrow(
      /document_id.*prediction|prediction/,
    );
  });

  it("rejects empty or non-numeric measurement sets", () => {
    const documentId = openWith(makePlateDoc());
    predictPrint({ document_id: documentId }, engine);
    expect(() =>
      recordMeasurement({ document_id: documentId, measurements: {} }),
    ).toThrow(/empty/);
    expect(() =>
      recordMeasurement({
        document_id: documentId,
        measurements: { bbox_x: "abc" },
      }),
    ).toThrow(/finite number/);
  });
});
