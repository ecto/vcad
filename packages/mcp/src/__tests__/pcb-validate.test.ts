import { describe, it, expect } from "vitest";
import { validatePcb, pcbValidationError, VALID_LAYERS } from "../tools/pcb-validate.js";
import { PCB_LAYERS } from "../tools/pcb-layers.js";
import type { Pcb } from "@vcad/ir";

function minimalPcb(overrides: Partial<Pcb> = {}): Pcb {
  return {
    outline: { vertices: [{ x: 0, y: 0 }, { x: 10, y: 0 }, { x: 10, y: 10 }, { x: 0, y: 10 }], thickness: 1.6 },
    stackup: { layers: [{ layer: "FCu", copperThickness: 0.035 }, { layer: "BCu", copperThickness: 0.035 }] },
    nets: [],
    rules: { defaultRules: { traceWidth: 0.2, clearance: 0.2 }, minDrill: 0.3, minAnnularRing: 0.15, edgeClearance: 0.25 },
    footprints: [],
    traces: [],
    vias: [],
    zones: [],
    ...overrides,
  } as Pcb;
}

describe("PCB layer list (single source)", () => {
  it("is the 21-name canonical list shared with the validator", () => {
    expect(PCB_LAYERS).toHaveLength(21);
    // pcb-validate's VALID_LAYERS must be derived from the shared PCB_LAYERS —
    // same names, no drift between the two modules.
    expect(new Set(VALID_LAYERS)).toEqual(new Set(PCB_LAYERS));
    for (const layer of PCB_LAYERS) {
      expect(VALID_LAYERS.has(layer)).toBe(true);
    }
  });
});

describe("validatePcb", () => {
  it("passes a well-formed board", () => {
    const result = validatePcb(minimalPcb());
    expect(result.valid).toBe(true);
    expect(result.diagnostics).toHaveLength(0);
    expect(result.documentSafe).toBe(true);
  });

  it("rejects a dotted layer name like F.Cu", () => {
    const pcb = minimalPcb({
      stackup: {
        layers: [
          { layer: "F.Cu" as any, copperThickness: 0.035 },
          { layer: "BCu", copperThickness: 0.035 },
        ],
      },
    });
    const result = validatePcb(pcb);
    expect(result.valid).toBe(false);
    expect(result.diagnostics).toHaveLength(1);
    expect(result.diagnostics[0].subsystem).toBe("layer_parse");
    expect(result.diagnostics[0].field).toBe("pcb.stackup.layers[0].layer");
    expect(result.diagnostics[0].value).toBe("F.Cu");
    expect(result.diagnostics[0].accepted).toContain("FCu");
    expect(result.diagnostics[0].message).toContain("F.Cu");
    expect(result.diagnostics[0].message).toContain("not a valid PcbLayer");
    expect(result.documentSafe).toBe(true);
  });

  it("rejects In1.Cu in middle of stackup", () => {
    const pcb = minimalPcb({
      stackup: {
        layers: [
          { layer: "FCu", copperThickness: 0.035 },
          { layer: "In1.Cu" as any, copperThickness: 0.035 },
          { layer: "BCu", copperThickness: 0.035 },
        ],
      },
    });
    const result = validatePcb(pcb);
    expect(result.valid).toBe(false);
    expect(result.diagnostics[0].field).toBe("pcb.stackup.layers[1].layer");
    expect(result.diagnostics[0].value).toBe("In1.Cu");
  });

  it("catches invalid layer on a trace", () => {
    const pcb = minimalPcb({
      traces: [{ start: { x: 0, y: 0 }, end: { x: 1, y: 1 }, width: 0.2, layer: "F.Cu" as any, net: "GND" }],
    });
    const result = validatePcb(pcb);
    expect(result.valid).toBe(false);
    expect(result.diagnostics[0].field).toBe("pcb.traces[].layer");
    expect(result.diagnostics[0].value).toBe("F.Cu");
  });

  it("catches invalid layer on a zone", () => {
    const pcb = minimalPcb({
      zones: [{ net: "GND", layer: "B.Cu" as any, vertices: [{ x: 0, y: 0 }], priority: 0 }],
    });
    const result = validatePcb(pcb);
    expect(result.valid).toBe(false);
    expect(result.diagnostics[0].field).toBe("pcb.zones[].layer");
  });

  it("reports all valid layers in accepted list", () => {
    const pcb = minimalPcb({
      stackup: { layers: [{ layer: "BOGUS" as any }] },
    });
    const result = validatePcb(pcb);
    expect(result.diagnostics[0].accepted).toEqual([...VALID_LAYERS]);
  });

  it("catches missing stackup.layers", () => {
    const pcb = minimalPcb();
    (pcb as any).stackup = {};
    const result = validatePcb(pcb);
    expect(result.valid).toBe(false);
    expect(result.diagnostics[0].subsystem).toBe("serde");
    expect(result.diagnostics[0].field).toBe("pcb.stackup.layers");
  });
});

describe("pcbValidationError", () => {
  it("produces a structured error naming the field and tool", () => {
    const pcb = minimalPcb({
      stackup: { layers: [{ layer: "F.Cu" as any }] },
    });
    const validity = validatePcb(pcb);
    const err = pcbValidationError("render_pcb", validity, "doc123");
    expect(err.isError).toBe(true);
    const body = JSON.parse(err.content[0].text);
    expect(body.field).toBe("pcb.stackup.layers[0].layer");
    expect(body.value).toBe("F.Cu");
    expect(body.subsystem).toBe("layer_parse");
    expect(body.tool).toBe("render_pcb");
    expect(body.document_id).toBe("doc123");
    expect(body.document_safe).toBe(true);
    expect(body.accepted).toContain("FCu");
  });

  it("includes other_issues when multiple diagnostics exist", () => {
    const pcb = minimalPcb({
      stackup: { layers: [{ layer: "F.Cu" as any }, { layer: "B.Cu" as any }] },
    });
    const validity = validatePcb(pcb);
    expect(validity.diagnostics.length).toBeGreaterThan(1);
    const err = pcbValidationError("export_gerber", validity);
    const body = JSON.parse(err.content[0].text);
    expect(body.other_issues).toBeDefined();
    expect(body.other_issues.length).toBeGreaterThan(0);
  });
});
