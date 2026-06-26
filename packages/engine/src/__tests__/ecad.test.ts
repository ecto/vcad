import { describe, it, expect, beforeAll } from "vitest";
import type { Pcb, SchematicSheet } from "@vcad/ir";
import { Engine } from "../index.js";
import { runDrc, runErc, critiqueRoute } from "../ecad.js";

// The ecad wrappers call the kernel WASM (a direct `import("@vcad/kernel-wasm")`)
// that `Engine.init()` initializes. Without this, a malformed board still
// reports `errored` — but for the wrong reason (kernel not loaded), so init it
// here to exercise the real serde deserialize path.
beforeAll(async () => {
  await Engine.init();
});

/** A minimal but fully-valid board the kernel deserializes cleanly. */
const validPcb: Pcb = {
  outline: {
    vertices: [
      { x: 0, y: 0 },
      { x: 10, y: 0 },
      { x: 10, y: 10 },
      { x: 0, y: 10 },
    ],
    thickness: 1.6,
  },
  stackup: { layers: [{ layer: "FCu" }, { layer: "BCu" }] },
  nets: [],
  rules: {
    defaultRules: {
      name: "default",
      traceWidth: 0.2,
      clearance: 0.2,
      viaDiameter: 0.6,
      viaDrill: 0.3,
    },
    edgeClearance: 0.2,
    holeToHole: 0.25,
    minAnnularRing: 0.05,
    minDrill: 0.2,
  },
  footprints: [],
  traces: [],
  vias: [],
  zones: [],
};

// Same board, but with a trace on a dotted layer name (`In1.Cu`). The kernel's
// `PcbLayer` enum only knows `In1Cu`, so serde refuses the whole board — the
// exact bug that used to deserialize-fail and get swallowed into a false-clean.
const malformedPcb = {
  ...validPcb,
  traces: [{ start: { x: 1, y: 1 }, end: { x: 5, y: 1 }, width: 0.2, layer: "In1.Cu", net: "GND" }],
} as unknown as Pcb;

/** A minimal valid schematic the kernel deserializes cleanly. */
const validSheet: SchematicSheet = {
  components: [
    {
      ref: "R1",
      value: "10k",
      footprintId: "Resistor_SMD:R_0805",
      position: { x: 0, y: 0 },
      rotation: 0,
      mirror: false,
      pins: [
        { number: "1", name: "1", pin_type: "Passive", position: { x: -1, y: 0 } },
        { number: "2", name: "2", pin_type: "Passive", position: { x: 1, y: 0 } },
      ],
    },
  ],
  wires: [],
  junctions: [],
  labels: [],
};

// Same schematic with a bogus pin electrical type — serde rejects the sheet.
const malformedSheet = {
  ...validSheet,
  components: [
    {
      ...validSheet.components[0],
      pins: [{ number: "1", name: "1", pin_type: "Inputt", position: { x: 0, y: 0 } }],
    },
  ],
} as unknown as SchematicSheet;

describe("ecad verification wrappers — three-state outcome", () => {
  it("runDrc reports `errored` (NOT clean) when the kernel can't parse the board", async () => {
    const outcome = await runDrc(malformedPcb);
    // The critical regression: a deserialize failure must NEVER read as a clean
    // empty violation list.
    expect(outcome.status).toBe("errored");
    if (outcome.status === "errored") {
      expect(outcome.offending_field).toBe("In1.Cu");
      expect(outcome.reason).toMatch(/In1\.Cu/);
    }
  });

  it("runDrc reports `ok` for a board the kernel can parse", async () => {
    const outcome = await runDrc(validPcb);
    expect(outcome.status).toBe("ok");
    if (outcome.status === "ok") expect(Array.isArray(outcome.value)).toBe(true);
  });

  it("critiqueRoute reports `errored` (NOT null) when the kernel can't parse the board", async () => {
    const outcome = await critiqueRoute(malformedPcb, "GND");
    expect(outcome.status).toBe("errored");
    if (outcome.status === "errored") expect(outcome.offending_field).toBe("In1.Cu");
  });

  it("runErc reports `errored` (NOT clean) when the kernel can't parse the schematic", async () => {
    const outcome = await runErc(malformedSheet);
    expect(outcome.status).toBe("errored");
    if (outcome.status === "errored") {
      expect(outcome.offending_field).toBe("Inputt");
      expect(outcome.reason).toMatch(/Inputt/);
    }
  });

  it("runErc reports `ok` for a schematic the kernel can parse", async () => {
    const outcome = await runErc(validSheet);
    expect(outcome.status).toBe("ok");
    if (outcome.status === "ok") expect(Array.isArray(outcome.value)).toBe(true);
  });
});
