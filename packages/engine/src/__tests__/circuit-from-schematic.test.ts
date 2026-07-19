import { describe, it, expect, beforeAll } from "vitest";
import { getKernelWasm } from "../wasm-singleton.js";

/**
 * End-to-end exercise of the schematic→circuit seam over WASM
 * (`circuitFromSchematic`, kernel-side `vcad-ecad-sim::circuit::netlist`)
 * plus the stateless DC/AC analyses the schematic editor's Analyze flow
 * chains onto it. Uses explicit `nets` (connectivity as data), like the app's
 * netlist does after wire extraction.
 */

/* eslint-disable @typescript-eslint/no-explicit-any */
let wasm: any;

beforeAll(async () => {
  wasm = await getKernelWasm();
});

const pin = (n: string, x: number, y: number) => ({
  number: n,
  name: n,
  pin_type: "Passive",
  position: { x, y },
});

/** Voltage divider: VIN --R1(1k)-- OUT --R2(1k)-- GNDNET, driven by V1 = 10 V. */
function dividerSheet() {
  return {
    components: [
      {
        ref: "V1",
        value: "10",
        footprintId: "",
        position: { x: 0, y: 0 },
        pins: [pin("1", 0, 0), pin("2", 0, 10)],
      },
      {
        ref: "R1",
        value: "1k",
        footprintId: "",
        position: { x: 20, y: 0 },
        pins: [pin("1", 0, 0), pin("2", 10, 0)],
      },
      {
        ref: "R2",
        value: "1k",
        footprintId: "",
        position: { x: 40, y: 0 },
        pins: [pin("1", 0, 0), pin("2", 10, 0)],
      },
    ],
    wires: [],
    junctions: [],
    labels: [],
    nets: {
      VIN: ["V1.1", "R1.1"],
      OUT: ["R1.2", "R2.1"],
      GNDNET: ["R2.2", "V1.2"],
    },
  };
}

describe("circuitFromSchematic", () => {
  // The checked-in WASM artifacts are only refreshed on main
  // (wasm-refresh.yml); skip on a stale local build. CI builds from source
  // and always exercises these.
  const requireBinding = (ctx: { skip: () => void }) => {
    if (typeof wasm.circuitFromSchematic !== "function") ctx.skip();
  };

  it("maps a divider and solves the DC operating point", (ctx) => {
    requireBinding(ctx);
    const mapped = wasm.circuitFromSchematic(
      JSON.stringify(dividerSheet()),
      JSON.stringify({ groundNets: ["GNDNET"] }),
    );
    expect(mapped.ok).toBe(true);
    expect(mapped.nodeOfNet.GNDNET).toBe(0);
    expect(mapped.deviceOfRef.R1).toBeDefined();
    expect(mapped.devices).toHaveLength(3);

    const dc = wasm.circuitDcOperatingPoint(JSON.stringify({ devices: mapped.devices }));
    const outNode = mapped.nodeOfNet.OUT;
    expect(dc.nodeVoltages[outNode]).toBeCloseTo(5, 6);
    expect(Math.abs(dc.powerBalanceW)).toBeLessThan(1e-9);
  });

  it("runs an AC sweep through the mapped source", (ctx) => {
    requireBinding(ctx);
    const mapped = wasm.circuitFromSchematic(
      JSON.stringify(dividerSheet()),
      JSON.stringify({ groundNets: ["GNDNET"] }),
    );
    const source = mapped.deviceOfRef.V1;
    const ac = wasm.circuitAcResponse(
      JSON.stringify({ devices: mapped.devices }),
      source,
      new Float64Array([2 * Math.PI * 1000]),
    );
    const outNode = mapped.nodeOfNet.OUT;
    const p = ac.points[0];
    // Resistive divider: |H| = 0.5, no phase.
    expect(Math.hypot(p.nodeVoltagesRe[outNode], p.nodeVoltagesIm[outNode])).toBeCloseTo(0.5, 6);
  });

  it("injects supplies and stubs power symbols", (ctx) => {
    requireBinding(ctx);
    const sheet = {
      components: [
        {
          ref: "PWR1",
          value: "",
          footprintId: "",
          position: { x: 0, y: 0 },
          pins: [pin("1", 0, 0)],
        },
        {
          ref: "R1",
          value: "2k",
          footprintId: "",
          position: { x: 20, y: 0 },
          pins: [pin("1", 0, 0), pin("2", 10, 0)],
        },
      ],
      wires: [],
      junctions: [],
      labels: [],
      nets: { VCC: ["PWR1.1", "R1.1"], GND: ["R1.2"] },
    };
    const mapped = wasm.circuitFromSchematic(
      JSON.stringify(sheet),
      JSON.stringify({
        stubAsOpen: ["PWR1"],
        supplies: [{ net: "VCC", volts: 3.3 }, { net: "UNUSED", volts: 5 }],
      }),
    );
    expect(mapped.ok).toBe(true);
    expect(mapped.stubbed).toEqual(["PWR1"]);
    expect(mapped.supplySourceOfNet.VCC).toBeDefined();
    expect(mapped.unconnectedSupplies).toEqual(["UNUSED"]);
    const dc = wasm.circuitDcOperatingPoint(JSON.stringify({ devices: mapped.devices }));
    expect(dc.nodeVoltages[mapped.nodeOfNet.VCC]).toBeCloseTo(3.3, 9);
  });

  it("fails closed with per-component blockers", (ctx) => {
    requireBinding(ctx);
    const sheet = dividerSheet();
    sheet.components.push({
      ref: "U1",
      value: "ATmega328P",
      footprintId: "",
      position: { x: 60, y: 0 },
      pins: [pin("1", 0, 0)],
    });
    (sheet.nets as Record<string, string[]>).VIN.push("U1.1");
    sheet.components[1]!.value = "banana";
    const mapped = wasm.circuitFromSchematic(
      JSON.stringify(sheet),
      JSON.stringify({ groundNets: ["GNDNET"] }),
    );
    expect(mapped.ok).toBe(false);
    const refs = mapped.blockers.map((b: { reference: string }) => b.reference).sort();
    expect(refs).toEqual(["R1", "U1"]);
    expect(mapped.blockers.every((b: { message: string }) => b.message.length > 0)).toBe(true);
  });
});
