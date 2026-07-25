/**
 * EE layout lint heuristics + the fault-visibility fixes around them:
 *  - layoutLint: crystal/decoupling/connector/high-current warnings with refs
 *    and distances;
 *  - slimPreviewForInlineUi keeps placement_drc (and friends) in the slim stub;
 *  - enrichSuccessResult puts "fix placement before routing" FIRST when
 *    placement_drc is dirty.
 */
import { describe, it, expect } from "vitest";
import type { Pcb, Footprint, Pad, PcbLayer } from "@vcad/ir";
import { layoutLint } from "../tools/ecad.js";
import { slimPreviewForInlineUi } from "../server.js";
import { enrichSuccessResult } from "../tools/next-actions.js";

const smdPad = (number: string, x: number, y: number, net?: string): Pad => ({
  number,
  padType: "SMD",
  shape: { type: "Rect", width: 1, height: 1 },
  position: { x, y },
  ...(net ? { net } : {}),
  layers: ["FCu" as PcbLayer],
});

const fp = (ref: string, x: number, y: number, pads: Pad[]): Footprint => ({
  ref,
  value: ref,
  footprintName: "test",
  position: { x, y },
  pads,
});

const boardWith = (footprints: Footprint[], rules?: Pcb["rules"]): Pcb => ({
  outline: {
    vertices: [
      { x: 0, y: 0 },
      { x: 50, y: 0 },
      { x: 50, y: 50 },
      { x: 0, y: 50 },
    ],
  },
  stackup: { layers: [], totalThickness: 1.6 },
  nets: [],
  rules: rules ?? {
    defaultRules: {
      name: "default",
      traceWidth: 0.25,
      clearance: 0.2,
      viaDiameter: 0.8,
      viaDrill: 0.4,
    },
    edgeClearance: 0.3,
    holeToHole: 0.5,
    minAnnularRing: 0.15,
  },
  footprints,
  traces: [],
  vias: [],
  zones: [],
});

describe("layoutLint", () => {
  it("flags a crystal far from its MCU oscillator pins, naming refs and distance", () => {
    const pcb = boardWith([
      fp("U1", 20, 20, [
        smdPad("14", 0, 0, "XIN"),
        smdPad("15", 1, 0, "XOUT"),
        smdPad("1", -1, 0, "GND"),
      ]),
      // 11mm away — the RP2040 repro
      fp("Y1", 31, 20, [smdPad("1", 0, 0, "XIN"), smdPad("2", 1, 0, "XOUT")]),
    ]);
    const warnings = layoutLint(pcb);
    const w = warnings.find((x) => x.kind === "crystal_far_from_ic");
    expect(w).toBeDefined();
    expect(w!.refs).toContain("Y1");
    expect(w!.refs).toContain("U1");
    expect(w!.distance_mm).toBeGreaterThan(5);
    expect(w!.threshold_mm).toBe(5);
    expect(w!.message).toMatch(/Y1/);
    expect(w!.message).toMatch(/\dmm/);
  });

  it("stays quiet when the crystal is close", () => {
    const pcb = boardWith([
      fp("U1", 20, 20, [smdPad("14", 0, 0, "XIN"), smdPad("15", 1, 0, "XOUT")]),
      fp("Y1", 23, 20, [smdPad("1", 0, 0, "XIN"), smdPad("2", 1, 0, "XOUT")]),
    ]);
    expect(layoutLint(pcb).filter((w) => w.kind === "crystal_far_from_ic")).toHaveLength(0);
  });

  it("flags a decoupling cap far from the supply pin it decouples", () => {
    const pcb = boardWith([
      fp("U1", 20, 20, [smdPad("3", 0, 0, "VDD"), smdPad("4", 1, 0, "GND")]),
      fp("C1", 28, 20, [smdPad("1", 0, 0, "VDD"), smdPad("2", 1, 0, "GND")]),
    ]);
    const w = layoutLint(pcb).find((x) => x.kind === "decoupling_cap_far_from_pin");
    expect(w).toBeDefined();
    expect(w!.refs).toEqual(["C1", "U1"]);
    expect(w!.distance_mm).toBeGreaterThan(3);
  });

  it("ignores a non-decoupling cap (no ground pad)", () => {
    const pcb = boardWith([
      fp("U1", 20, 20, [smdPad("3", 0, 0, "VDD")]),
      fp("C9", 40, 40, [smdPad("1", 0, 0, "SIG_A"), smdPad("2", 1, 0, "SIG_B")]),
    ]);
    expect(layoutLint(pcb).filter((w) => w.kind === "decoupling_cap_far_from_pin")).toHaveLength(0);
  });

  it("flags a connector buried in the board interior", () => {
    const pcb = boardWith([fp("J1", 25, 25, [smdPad("1", 0, 0, "VBUS")])]);
    const w = layoutLint(pcb).find((x) => x.kind === "connector_not_on_edge");
    expect(w).toBeDefined();
    expect(w!.refs).toEqual(["J1"]);
    expect(w!.distance_mm).toBeGreaterThan(5);
  });

  it("accepts a connector at the board edge", () => {
    const pcb = boardWith([fp("J1", 1, 25, [smdPad("1", 0, 0, "VBUS")])]);
    expect(layoutLint(pcb).filter((w) => w.kind === "connector_not_on_edge")).toHaveLength(0);
  });

  it("flags a high-current-class pad crowding a USB pad", () => {
    const rules: Pcb["rules"] = {
      defaultRules: {
        name: "default",
        traceWidth: 0.25,
        clearance: 0.2,
        viaDiameter: 0.8,
        viaDrill: 0.4,
      },
      classRules: [
        {
          name: "power",
          traceWidth: 1.5,
          clearance: 0.3,
          viaDiameter: 1.0,
          viaDrill: 0.5,
        },
      ],
      netClassAssignments: { power: ["MOTOR_A"] },
      edgeClearance: 0.3,
      holeToHole: 0.5,
      minAnnularRing: 0.15,
    };
    const pcb = boardWith(
      [
        fp("Q1", 20, 20, [smdPad("1", 0, 0, "MOTOR_A")]),
        fp("U2", 21, 20, [smdPad("2", 0, 0, "USB_DP")]),
      ],
      rules,
    );
    const w = layoutLint(pcb).find((x) => x.kind === "high_current_near_sensitive");
    expect(w).toBeDefined();
    expect(w!.refs).toEqual(["Q1", "U2"]);
    expect(w!.distance_mm).toBeLessThan(2);
    expect(w!.message).toMatch(/MOTOR_A/);
    expect(w!.message).toMatch(/USB_DP/);
  });

  it("returns nothing for a clean small board", () => {
    const pcb = boardWith([
      fp("R1", 10, 10, [smdPad("1", 0, 0, "A"), smdPad("2", 1, 0, "B")]),
    ]);
    expect(layoutLint(pcb)).toHaveLength(0);
  });
});

describe("slimPreviewForInlineUi placement-fault preservation", () => {
  it("keeps placement_drc and warnings in the slim stub", () => {
    const body = {
      success: true,
      placement_drc: { clean: false, courtyard_overlaps: 7 },
      warnings: ["placement DRC found 7 courtyard overlap(s)"],
      nets: { X: Array.from({ length: 3000 }, (_, i) => `pad${i}`) }, // force >8192 chars
      document_id: "doc-1",
    };
    const result = { content: [{ type: "text", text: JSON.stringify(body) }] };
    slimPreviewForInlineUi(result, "doc-1", "place_components", true);
    const stub = JSON.parse(result.content[1]!.text) as Record<string, unknown>;
    expect(stub.document_id).toBe("doc-1");
    expect(stub.placement_drc).toEqual({ clean: false, courtyard_overlaps: 7 });
    expect(stub.warnings).toEqual(body.warnings);
    expect(stub.success).toBe(true);
    expect(stub.nets).toBeUndefined(); // bulk still slimmed
  });
});

describe("enrichSuccessResult placement-aware next_actions", () => {
  const bodyWith = (drc: Record<string, unknown>) =>
    JSON.stringify({ document_id: "doc-1", placement_drc: drc });

  it("puts 'fix placement before routing' first when placement_drc is dirty", () => {
    const result = {
      content: [{ type: "text", text: bodyWith({ clean: false, courtyard_overlaps: 7, shorts: [] }) }],
    };
    enrichSuccessResult(result, "place_components", {});
    const next = (result as { structuredContent?: { next_actions?: Array<{ action: string; tool?: string }> } })
      .structuredContent!.next_actions!;
    expect(next[0]!.tool).toBe("set_placement");
    expect(next[0]!.action).toMatch(/NOT clean/);
    expect(next[0]!.action).toMatch(/7 courtyard overlap/);
    expect(next[0]!.action).toMatch(/BEFORE routing/);
  });

  it("keeps the happy path when placement_drc is clean", () => {
    const result = {
      content: [{ type: "text", text: bodyWith({ clean: true }) }],
    };
    enrichSuccessResult(result, "place_components", {});
    const next = (result as { structuredContent?: { next_actions?: Array<{ tool?: string }> } })
      .structuredContent!.next_actions!;
    expect(next[0]!.tool).toBe("set_design_rules");
  });

  it("finds the JSON stub even when a prose summary block comes first (slimmed result)", () => {
    const result = {
      content: [
        { type: "text", text: "CAD document ready (doc-1). Geometry is available…" },
        { type: "text", text: bodyWith({ clean: false, off_board: ["J1"] }) },
      ],
    };
    enrichSuccessResult(result, "place_components", {});
    const next = (result as { structuredContent?: { next_actions?: Array<{ action: string }> } })
      .structuredContent!.next_actions!;
    expect(next[0]!.action).toMatch(/1 off-board/);
  });

  it("also fires for set_placement", () => {
    const result = {
      content: [{ type: "text", text: bodyWith({ clean: false, clearance_violations: 2 }) }],
    };
    enrichSuccessResult(result, "set_placement", {});
    const next = (result as { structuredContent?: { next_actions?: Array<{ tool?: string; action: string }> } })
      .structuredContent!.next_actions!;
    expect(next[0]!.tool).toBe("set_placement");
    expect(next[0]!.action).toMatch(/2 clearance/);
  });
});
