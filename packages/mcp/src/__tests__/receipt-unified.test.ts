import { describe, it, expect } from "vitest";
import type { Receipt } from "@vcad/ir";
import type { EnclosureFitReport } from "@vcad/engine";
import {
  RECEIPT_SCHEMA,
  overallVerdict,
  summarize,
  unifiedFromPcbReceipt,
  unverifiablePcbReceipt,
} from "../receipt-unified.js";

function pcbReceipt(overrides: Partial<Receipt> = {}): Receipt {
  return {
    board_hash: "abcd1234",
    design_rules_hash: "ef567890",
    drc_backend: "vcad-ecad-pcb 0.9.4",
    drc: { total: 0, by_rule: [], violations: [] },
    power_integrity: [],
    parts: [
      { reference: "U1", footprint: "SOIC-8", value: "MCU", mpn: "ATTINY85" },
      { reference: "R1", footprint: "R_0603", value: "10k" },
    ],
    ...overrides,
  };
}

describe("unifiedFromPcbReceipt", () => {
  it("clean board yields a passing pcb.drc.clean claim with oracle version", () => {
    const unified = unifiedFromPcbReceipt(pcbReceipt(), "doc-1");
    expect(unified.schema).toBe(RECEIPT_SCHEMA);
    expect(unified.document_id).toBe("doc-1");
    expect(unified.document_fingerprint).toBe("abcd1234");

    const clean = unified.claims.find((c) => c.id === "pcb.drc.clean")!;
    expect(clean.verdict).toBe("pass");
    expect(clean.oracle).toEqual({ id: "vcad-ecad-pcb", version: "0.9.4" });
    expect(clean.predicted).toEqual({ value: 0, unit: "violations" });
    expect(clean.measured).toEqual({ value: 0, unit: "violations" });
    expect(overallVerdict(unified.claims)).toBe("pass");
  });

  it("violations fail the board claim and add per-rule claims", () => {
    const unified = unifiedFromPcbReceipt(
      pcbReceipt({
        drc: {
          total: 3,
          by_rule: [
            { rule: "Clearance", count: 2 },
            { rule: "CourtyardOverlap", count: 1 },
          ],
          violations: ["Clearance|a|0|0", "Clearance|b|1|1", "CourtyardOverlap|c|2|2"],
        },
      }),
    );
    expect(unified.claims.find((c) => c.id === "pcb.drc.clean")!.verdict).toBe("fail");
    const clearance = unified.claims.find((c) => c.id === "pcb.drc.Clearance")!;
    expect(clearance.verdict).toBe("fail");
    expect(clearance.measured).toEqual({ value: 2, unit: "violations" });
    expect(overallVerdict(unified.claims)).toBe("fail");
    const summary = summarize(unified);
    expect(summary.failed).toBe(3);
    expect(summary.overall).toBe("fail");
  });

  it("a split power plane fails its continuity claim even with clean DRC", () => {
    const unified = unifiedFromPcbReceipt(
      pcbReceipt({
        power_integrity: [
          {
            net: "GND",
            islands: 1,
            continuous: true,
            coverage: 1,
            connected_pads: 14,
            total_pads: 14,
            vias: 6,
          },
          {
            net: "+3V3",
            islands: 4,
            continuous: false,
            coverage: 0.5,
            connected_pads: 5,
            total_pads: 10,
            vias: 0,
          },
        ],
      }),
    );
    const planes = unified.claims.filter((c) => c.id === "pcb.power.continuity");
    expect(planes).toHaveLength(2);
    const gnd = planes.find((c) => c.subject === "net:GND")!;
    expect(gnd.verdict).toBe("pass");
    const v33 = planes.find((c) => c.subject === "net:+3V3")!;
    expect(v33.verdict).toBe("fail");
    expect(v33.measured).toEqual({ value: 4, unit: "islands" });
    expect(v33.predicted).toEqual({ value: 1, unit: "islands" });
    // DRC is clean but the receipt still fails overall — the split plane gates.
    expect(overallVerdict(unified.claims)).toBe("fail");
  });

  it("provenance claim counts parts and flags missing MPNs", () => {
    const unified = unifiedFromPcbReceipt(pcbReceipt());
    const prov = unified.claims.find((c) => c.id === "pcb.provenance.parts")!;
    expect(prov.verdict).toBe("pass");
    expect(prov.measured).toEqual({ value: 2, unit: "parts" });
    expect(prov.details).toContain("1 part(s) without an MPN");
  });

  it("sourcing claim is informational and present only when captured", () => {
    expect(
      unifiedFromPcbReceipt(pcbReceipt()).claims.some(
        (c) => c.id === "pcb.sourcing.snapshot",
      ),
    ).toBe(false);
    const unified = unifiedFromPcbReceipt(
      pcbReceipt({
        sourcing: { lines: [{ mpn: "ATTINY85", stock: 100, unit_price: 1.2, currency: "USD" }] },
      }),
    );
    const sourcing = unified.claims.find((c) => c.id === "pcb.sourcing.snapshot")!;
    expect(sourcing.verdict).toBe("pass");
    expect(sourcing.details).toContain("never gates");
  });

  it("backend string without a version parses fail-closed as unknown", () => {
    const unified = unifiedFromPcbReceipt(pcbReceipt({ drc_backend: "mystery-drc" }));
    expect(unified.claims[0]!.oracle).toEqual({ id: "mystery-drc", version: "unknown" });
  });

  it("enclosure-fit checks map to claims; skip is unverifiable, not clean", () => {
    const fit = {
      ok: true,
      verified: false,
      summary: "fits with warnings",
      clearance: 0.5,
      placement: { x: 0, y: 0, rotationDeg: 0, z: 3 },
      checks: [
        { id: "board_outline", label: "board fits cavity", status: "pass", detail: "1.2 mm margin" },
        { id: "lid_clearance", label: "tallest part clears lid", status: "warn", detail: "0.4 mm" },
        { id: "connectors", label: "connectors align with openings", status: "skip", detail: "no openings declared" },
      ],
    } as unknown as EnclosureFitReport;
    const unified = unifiedFromPcbReceipt(pcbReceipt(), "doc-1", { enclosureFit: fit });
    expect(
      unified.claims.find((c) => c.id === "pcb.enclosure_fit.board_outline")!.verdict,
    ).toBe("pass");
    const lid = unified.claims.find((c) => c.id === "pcb.enclosure_fit.lid_clearance")!;
    expect(lid.verdict).toBe("pass");
    expect(lid.details).toContain("warning:");
    const conn = unified.claims.find((c) => c.id === "pcb.enclosure_fit.connectors")!;
    expect(conn.verdict).toBe("unverifiable");
    // a skipped check keeps the whole receipt from reading verified-clean
    expect(overallVerdict(unified.claims)).toBe("unverifiable");
  });

  it("an enclosure-fit oracle that could not run is an unverifiable claim", () => {
    const unified = unifiedFromPcbReceipt(pcbReceipt(), undefined, {
      enclosureFitError: "enclosure-fit needs the kernel engine",
    });
    const fit = unified.claims.find((c) => c.id === "pcb.enclosure_fit")!;
    expect(fit.verdict).toBe("unverifiable");
    expect(fit.details).toContain("kernel engine");
    expect(overallVerdict(unified.claims)).toBe("unverifiable");
  });
});

describe("fail-closed rollup", () => {
  it("empty claim list is unverifiable, never clean", () => {
    expect(overallVerdict([])).toBe("unverifiable");
  });

  it("an unverifiable oracle never reads as pass", () => {
    const receipt = unverifiablePcbReceipt("ECAD engine unavailable", "doc-2");
    expect(receipt.claims[0]!.verdict).toBe("unverifiable");
    expect(receipt.claims[0]!.details).toBe("ECAD engine unavailable");
    expect(summarize(receipt).overall).toBe("unverifiable");
  });
});
