/**
 * verify_spec — spec-first "TDD for CAD" verification.
 *
 * Two layers are exercised: the pure receipt builder (`unifiedFromSpec`,
 * deterministic, no engine) and the MCP tool (`verifySpec`, measured against
 * a real kernel evaluation). Both must honor the unified receipt's fail-closed
 * rollup: a passing spec is a `pass`, a failing claim names measured-vs-
 * expected, and anything the kernel can't measure is `unverifiable`, never a
 * silent pass.
 */

import { beforeAll, beforeEach, describe, expect, it } from "vitest";
import { Engine } from "@vcad/engine";
import { createDocument } from "@vcad/ir";
import type { DesignReceipt } from "@vcad/ir";
import {
  RECEIPT_SCHEMA,
  overallVerdict,
  summarize,
  unifiedFromSpec,
  type SpecMeasurement,
} from "../receipt-unified.js";
import { verifySpec } from "../tools/verify-spec.js";
import { sheetMetalCreate } from "../tools/sheet-metal.js";
import { documents, registerSession } from "../tools/session.js";

let engine: Engine;

beforeAll(async () => {
  engine = await Engine.init();
});

beforeEach(() => {
  documents.clear();
});

/** A well-formed measurement of a 10×20×30 box at the origin. */
function boxMeasurement(): SpecMeasurement {
  return {
    volume_mm3: 6000,
    bounding_box: { min: { x: 0, y: 0, z: 0 }, max: { x: 10, y: 20, z: 30 } },
    center_of_mass: { x: 5, y: 10, z: 15 },
    watertight: true,
    parts: 1,
  };
}

/**
 * Structural validation against the unified receipt schema (vcad.receipt/1),
 * mirroring the Rust `DesignReceipt` shape. No JSON-schema file ships for the
 * receipt (Rust is the source of truth), so this asserts the invariants the
 * schema encodes: the tag, well-formed claims, enum verdicts, and a summary
 * consistent with the fail-closed rollup.
 */
function expectValidReceipt(receipt: DesignReceipt): void {
  expect(receipt.schema).toBe(RECEIPT_SCHEMA);
  expect(Array.isArray(receipt.claims)).toBe(true);
  expect(receipt.claims.length).toBeGreaterThan(0);
  for (const c of receipt.claims) {
    expect(typeof c.id).toBe("string");
    expect(c.id.length).toBeGreaterThan(0);
    expect(typeof c.domain).toBe("string");
    expect(typeof c.description).toBe("string");
    expect(["pass", "fail", "unverifiable"]).toContain(c.verdict);
    expect(c.oracle).toBeDefined();
    expect(typeof c.oracle.id).toBe("string");
    expect(typeof c.oracle.version).toBe("string");
    // An unverifiable claim must carry its reason (schema invariant).
    if (c.verdict === "unverifiable") expect(typeof c.details).toBe("string");
    for (const q of [c.predicted, c.measured]) {
      if (q !== undefined) {
        expect(["number", "boolean", "string"]).toContain(typeof q.value);
        if (q.unit !== undefined) expect(typeof q.unit).toBe("string");
      }
    }
  }
  // Summary must agree with the standalone rollup.
  const summary = summarize(receipt);
  expect(summary.total).toBe(receipt.claims.length);
  expect(summary.overall).toBe(overallVerdict(receipt.claims));
}

describe("unifiedFromSpec (pure builder)", () => {
  it("passing spec: every claim measured, rolls up to pass", () => {
    const receipt = unifiedFromSpec(
      {
        bbox_min: { x: 0, y: 0, z: 0 },
        bbox_max: { x: 10, y: 20, z: 30 },
        volume: { min: 5990, max: 6010 },
        watertight: true,
        part_count: 1,
        center_of_mass: { x: 5, y: 10, z: 15, tol: 0.1 },
      },
      boxMeasurement(),
      "doc-pass",
    );
    expect(receipt.document_id).toBe("doc-pass");
    expect(receipt.claims.every((c) => c.verdict === "pass")).toBe(true);
    expect(overallVerdict(receipt.claims)).toBe("pass");
    // Spot-check measured-vs-expected is carried, not just a bare verdict.
    const vol = receipt.claims.find((c) => c.id === "spec.volume")!;
    expect(vol.measured).toEqual({ value: 6000, unit: "mm^3" });
    expectValidReceipt(receipt);
  });

  it("failing spec: names the failing claim with measured vs expected", () => {
    const receipt = unifiedFromSpec(
      {
        volume: { min: 7000, max: 8000 },
        bbox_max: { x: 10, y: 20, z: 25 }, // z is off by 5 mm
        part_count: 2,
      },
      boxMeasurement(),
    );
    expect(overallVerdict(receipt.claims)).toBe("fail");

    const vol = receipt.claims.find((c) => c.id === "spec.volume")!;
    expect(vol.verdict).toBe("fail");
    expect(vol.measured).toEqual({ value: 6000, unit: "mm^3" });
    expect(vol.predicted).toEqual({ value: "[7000, 8000]", unit: "mm^3" });

    const bz = receipt.claims.find((c) => c.id === "spec.bbox.max.z")!;
    expect(bz.verdict).toBe("fail");
    expect(bz.predicted).toEqual({ value: 25, unit: "mm" });
    expect(bz.measured).toEqual({ value: 30, unit: "mm" });
    // The in-tolerance axes on the same corner still pass.
    expect(receipt.claims.find((c) => c.id === "spec.bbox.max.x")!.verdict).toBe("pass");

    const pc = receipt.claims.find((c) => c.id === "spec.part_count")!;
    expect(pc.verdict).toBe("fail");
    expect(pc.measured).toEqual({ value: 1, unit: "count" });

    const failed = summarize(receipt).failed;
    expect(failed).toBeGreaterThanOrEqual(3);
    expectValidReceipt(receipt);
  });

  it("unverifiable: a claim the kernel can't evaluate never rolls up to pass", () => {
    // Kernel could not evaluate the document at all → null measurement.
    const receipt = unifiedFromSpec(
      { bbox_min: { x: 0 }, volume: { min: 1 }, watertight: true, part_count: 1 },
      null,
      "doc-unverifiable",
    );
    expect(receipt.claims.every((c) => c.verdict === "unverifiable")).toBe(true);
    expect(receipt.claims.every((c) => typeof c.details === "string")).toBe(true);
    expect(overallVerdict(receipt.claims)).not.toBe("pass");
    expect(overallVerdict(receipt.claims)).toBe("unverifiable");
    expectValidReceipt(receipt);
  });

  it("unverifiable: a null center of mass yields unverifiable, not pass", () => {
    const measurement: SpecMeasurement = { ...boxMeasurement(), center_of_mass: null };
    const receipt = unifiedFromSpec({ center_of_mass: { x: 5 } }, measurement);
    const com = receipt.claims.find((c) => c.id === "spec.com.x")!;
    expect(com.verdict).toBe("unverifiable");
    expect(com.details).toContain("no enclosed volume");
    expect(overallVerdict(receipt.claims)).toBe("unverifiable");
  });

  it("empty spec is unverifiable, never a vacuous pass", () => {
    const receipt = unifiedFromSpec({}, boxMeasurement());
    expect(receipt.claims).toHaveLength(1);
    expect(receipt.claims[0]!.id).toBe("spec.empty");
    expect(receipt.claims[0]!.verdict).toBe("unverifiable");
    expect(overallVerdict(receipt.claims)).toBe("unverifiable");
    expectValidReceipt(receipt);
  });

  it("an unbounded volume range asserts nothing and is unverifiable", () => {
    const receipt = unifiedFromSpec({ volume: {} }, boxMeasurement());
    const vol = receipt.claims.find((c) => c.id === "spec.volume")!;
    expect(vol.verdict).toBe("unverifiable");
    expect(overallVerdict(receipt.claims)).not.toBe("pass");
  });
});

describe("verifySpec (MCP tool)", () => {
  function out(result: { content: Array<{ type: string; text: string }> }): {
    receipt: DesignReceipt;
    summary: ReturnType<typeof summarize>;
  } {
    return JSON.parse(result.content[0].text);
  }

  function cubeDoc() {
    const doc = engine.evalVcadSource("[cube 10 20 30]");
    if (!doc) throw new Error("engine build lacks loon support");
    return doc;
  }

  it("passes a spec that matches the measured geometry, with a fingerprint", () => {
    const id = registerSession(cubeDoc());
    const { receipt, summary } = out(
      verifySpec(
        {
          document_id: id,
          spec: {
            bbox_min: { x: 0, y: 0, z: 0 },
            bbox_max: { x: 10, y: 20, z: 30 },
            volume: { min: 5950, max: 6050 },
            watertight: true,
            part_count: 1,
            center_of_mass: { x: 5, y: 10, z: 15, tol: 0.2 },
          },
        },
        engine,
      ),
    );
    expect(summary.overall).toBe("pass");
    expect(receipt.document_id).toBe(id);
    // sha256 hex fingerprint of the design snapshot.
    expect(receipt.document_fingerprint).toMatch(/^[0-9a-f]{64}$/);
    expectValidReceipt(receipt);
  });

  it("fails and names the off-spec claim with measured vs expected", () => {
    const id = registerSession(cubeDoc());
    const { receipt, summary } = out(
      verifySpec(
        { document_id: id, spec: { volume: { min: 100000 }, bbox_max: { z: 5 } } },
        engine,
      ),
    );
    expect(summary.overall).toBe("fail");
    const vol = receipt.claims.find((c) => c.id === "spec.volume")!;
    expect(vol.verdict).toBe("fail");
    expect(Number(vol.measured!.value)).toBeCloseTo(6000, 0);
    const bz = receipt.claims.find((c) => c.id === "spec.bbox.max.z")!;
    expect(bz.verdict).toBe("fail");
    expect(Number(bz.measured!.value)).toBeCloseTo(30, 3);
    expectValidReceipt(receipt);
  });

  it("a spec the kernel can't measure is unverifiable, never a pass", () => {
    // An empty document has no bounding box / center of mass to measure.
    const id = registerSession(createDocument());
    const { receipt, summary } = out(
      verifySpec(
        { document_id: id, spec: { bbox_min: { x: 0 }, center_of_mass: { x: 0 } } },
        engine,
      ),
    );
    expect(summary.overall).not.toBe("pass");
    expect(summary.overall).toBe("unverifiable");
    expect(receipt.claims.some((c) => c.verdict === "unverifiable")).toBe(true);
    expect(receipt.claims.some((c) => c.verdict === "pass")).toBe(false);
    expectValidReceipt(receipt);
  });

  it("mirrors each claim's spec bound as `expected` in the tool output", () => {
    // Display alias of the schema's `predicted` — readers of the tool result
    // looking for "expected" must find the bound, not nothing.
    const id = registerSession(cubeDoc());
    const { receipt } = out(
      verifySpec(
        { document_id: id, spec: { volume: { min: 5950, max: 6050 }, watertight: true } },
        engine,
      ),
    );
    for (const c of receipt.claims) {
      const raw = c as unknown as Record<string, unknown>;
      expect(raw.expected).toBeDefined();
      expect(raw.expected).toEqual(c.predicted);
    }
    expectValidReceipt(receipt);
  });
});

/**
 * Field-repro regression: a SheetMetalBaseFlangePolygon with hole loops used
 * to tessellate non-watertight (caps ignored the holes, side quads wound
 * inside-out) and measure ~2374 mm³ against ~6890 mm³ of truth, so this exact
 * spec graded `fail` while the STEP export and sheet_metal_cost were sound.
 */
describe("verifySpec on sheet-metal flanges with hole loops", () => {
  function ring(
    cx: number,
    cy: number,
    r: number,
    n: number,
    ccw: boolean,
  ): Array<{ x: number; y: number }> {
    const pts = Array.from({ length: n }, (_, i) => {
      const a = (2 * Math.PI * i) / n;
      return { x: cx + r * Math.cos(a), y: cy + r * Math.sin(a) };
    });
    if (!ccw) pts.reverse();
    return pts;
  }

  function createFlange(args: Record<string, unknown>): string {
    const res = sheetMetalCreate(args, engine);
    const body = JSON.parse(res.content[0].text) as { document_id: string };
    expect(body.document_id).toBeTruthy();
    return body.document_id;
  }

  function grade(
    id: string,
    volume: { min: number; max: number },
  ): ReturnType<typeof summarize> {
    const res = verifySpec(
      { document_id: id, spec: { volume, watertight: true, part_count: 1 } },
      engine,
    );
    const parsed = JSON.parse(res.content[0].text) as {
      receipt: DesignReceipt;
      summary: ReturnType<typeof summarize>;
    };
    expectValidReceipt(parsed.receipt);
    return parsed.summary;
  }

  it("64-gon disc flange with a bore and four BCD holes passes volume + watertight", () => {
    const holes = [ring(0, 0, 4.2, 16, false)];
    for (let k = 0; k < 4; k++) {
      const a = (2 * Math.PI * k) / 4;
      holes.push(ring(11 * Math.cos(a), 11 * Math.sin(a), 1.65, 12, false));
    }
    const id = createFlange({
      outline: ring(0, 0, 29, 64, true),
      holes,
      thickness: 2.7,
      material: "steel-mild",
    });
    // Analytic n-gon volume 6888.1 mm³, bounds ±1%.
    const summary = grade(id, { min: 6820, max: 6960 });
    expect(summary.overall).toBe("pass");
    expect(summary.failed).toBe(0);
  });

  it("square flange with a single rectangular hole passes volume + watertight", () => {
    const id = createFlange({
      outline: [
        { x: 0, y: 0 },
        { x: 20, y: 0 },
        { x: 20, y: 10 },
        { x: 0, y: 10 },
      ],
      // CW hole, matching the documented hole-winding convention.
      holes: [
        [
          { x: 5, y: 3 },
          { x: 5, y: 7 },
          { x: 8, y: 7 },
          { x: 8, y: 3 },
        ],
      ],
      thickness: 1,
      material: "steel-mild",
    });
    // Analytic (200 − 12) × 1 = 188 mm³, bounds ±1%.
    const summary = grade(id, { min: 186.1, max: 189.9 });
    expect(summary.overall).toBe("pass");
    expect(summary.failed).toBe(0);
  });
});
