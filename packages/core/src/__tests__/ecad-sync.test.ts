import { describe, it, expect } from "vitest";
import type { Pcb, SchematicSheet, SchematicComponent } from "@vcad/ir";
import { syncSchematicToPcbData, padsFromPins, type SyncNetlist } from "../stores/ecad-sync.js";

/** Minimal empty PCB — the sync only reads/writes `footprints` and `nets`. */
function makePcb(partial: Partial<Pcb> = {}): Pcb {
  return {
    outline: { vertices: [], thickness: 1.6 },
    stackup: {},
    nets: [],
    rules: {},
    footprints: [],
    traces: [],
    vias: [],
    zones: [],
    ...partial,
  } as unknown as Pcb;
}

function comp(
  ref: string,
  pins: { number: string; name: string }[],
  properties?: Record<string, string>,
): SchematicComponent {
  return {
    ref,
    value: ref,
    footprintId: "",
    position: { x: 0, y: 0 },
    pins: pins.map((p) => ({
      number: p.number,
      name: p.name,
      pin_type: "Passive",
      position: { x: 0, y: 0 },
    })),
    ...(properties ? { properties } : {}),
  } as unknown as SchematicComponent;
}

function sheet(components: SchematicComponent[]): SchematicSheet {
  return { components, wires: [], junctions: [], labels: [] } as unknown as SchematicSheet;
}

describe("padsFromPins", () => {
  it("makes one pad per pin, numbered to match, centered single row", () => {
    const pads = padsFromPins([{ number: "1" }, { number: "2" }, { number: "3" }]);
    expect(pads.map((p) => p.number)).toEqual(["1", "2", "3"]);
    // centered: middle pad at x=0, outer pads symmetric
    expect(pads[1]!.position.x).toBe(0);
    expect(pads[0]!.position.x).toBeCloseTo(-2.54);
    expect(pads[2]!.position.x).toBeCloseTo(2.54);
    expect(pads.every((p) => p.layers.includes("FCu"))).toBe(true);
  });
});

describe("syncSchematicToPcbData — placement", () => {
  it("places an unplaced component with pin-derived pads", () => {
    const pcb = makePcb();
    const sch = sheet([comp("R1", [{ number: "1", name: "A" }, { number: "2", name: "B" }])]);
    const { pcb: next, changed } = syncSchematicToPcbData(pcb, sch);
    expect(changed).toBe(true);
    expect(next.footprints).toHaveLength(1);
    expect(next.footprints[0]!.ref).toBe("R1");
    expect(next.footprints[0]!.pads.map((p) => p.number)).toEqual(["1", "2"]);
  });

  it("prefers an explicit footprintTemplate over pin-derived pads", () => {
    const template = JSON.stringify({
      pads: [
        { number: "1", padType: "THT", shape: { type: "Circle", diameter: 1 }, position: { x: -2, y: 0 }, layers: ["FCu"] },
        { number: "2", padType: "THT", shape: { type: "Circle", diameter: 1 }, position: { x: 2, y: 0 }, layers: ["FCu"] },
      ],
    });
    const sch = sheet([comp("D1", [{ number: "1", name: "A" }, { number: "2", name: "K" }], { footprintTemplate: template })]);
    const { pcb: next } = syncSchematicToPcbData(makePcb(), sch);
    expect(next.footprints[0]!.pads[0]!.padType).toBe("THT");
    expect(next.footprints[0]!.pads[0]!.position.x).toBe(-2);
  });

  it("does not duplicate an already-placed footprint and is idempotent", () => {
    const sch = sheet([comp("R1", [{ number: "1", name: "A" }, { number: "2", name: "B" }])]);
    const first = syncSchematicToPcbData(makePcb(), sch);
    expect(first.changed).toBe(true);
    const second = syncSchematicToPcbData(first.pcb, sch);
    expect(second.changed).toBe(false);
    expect(second.pcb.footprints).toHaveLength(1);
  });

  it("does not mutate the input pcb", () => {
    const pcb = makePcb();
    const sch = sheet([comp("R1", [{ number: "1", name: "A" }])]);
    syncSchematicToPcbData(pcb, sch);
    expect(pcb.footprints).toHaveLength(0);
  });
});

describe("syncSchematicToPcbData — net mapping", () => {
  const netlist: SyncNetlist = {
    nets: [
      { name: "NET-001", connections: [{ component_ref: "R1", pin_number: "2" }, { component_ref: "R2", pin_number: "1" }] },
      { name: "GND", connections: [{ component_ref: "R1", pin_number: "1" }] },
    ],
  };

  it("assigns pad.net from the netlist and unions net names into pcb.nets", () => {
    const sch = sheet([
      comp("R1", [{ number: "1", name: "A" }, { number: "2", name: "B" }]),
      comp("R2", [{ number: "1", name: "A" }, { number: "2", name: "B" }]),
    ]);
    const { pcb: next } = syncSchematicToPcbData(makePcb(), sch, netlist);
    const r1 = next.footprints.find((f) => f.ref === "R1")!;
    const r2 = next.footprints.find((f) => f.ref === "R2")!;
    expect(r1.pads.find((p) => p.number === "1")!.net).toBe("GND");
    expect(r1.pads.find((p) => p.number === "2")!.net).toBe("NET-001");
    expect(r2.pads.find((p) => p.number === "1")!.net).toBe("NET-001");
    // R2 pin 2 is on no net → unassigned
    expect(r2.pads.find((p) => p.number === "2")!.net).toBeUndefined();
    expect(next.nets.map((n) => n.name).sort()).toEqual(["GND", "NET-001"]);
  });

  it("clears a stale pad.net no longer present in the netlist", () => {
    const sch = sheet([comp("R1", [{ number: "1", name: "A" }, { number: "2", name: "B" }])]);
    // Place + assign nets, then re-sync with an empty netlist.
    const placed = syncSchematicToPcbData(makePcb(), sch, netlist);
    expect(placed.pcb.footprints[0]!.pads.find((p) => p.number === "1")!.net).toBe("GND");
    const cleared = syncSchematicToPcbData(placed.pcb, sch, { nets: [] });
    expect(cleared.changed).toBe(true);
    expect(cleared.pcb.footprints[0]!.pads.every((p) => p.net === undefined)).toBe(true);
  });

  it("preserves existing nets (additive union, never removes)", () => {
    const pcb = makePcb({ nets: [{ id: "VBAT", name: "VBAT" }] as Pcb["nets"] });
    const sch = sheet([comp("R1", [{ number: "1", name: "A" }, { number: "2", name: "B" }])]);
    const { pcb: next } = syncSchematicToPcbData(pcb, sch, netlist);
    expect(next.nets.map((n) => n.name).sort()).toEqual(["GND", "NET-001", "VBAT"]);
  });
});
