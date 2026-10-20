import { describe, it, expect, beforeAll } from "vitest";
import { Engine } from "@vcad/engine";
import {
  createSchematic,
  updateSchematic,
  inspectSchematic,
  runErc,
  placeComponents,
  exportKicad,
} from "../tools/ecad.js";
import { flattenSchematic, offsetRef } from "../tools/schematic-hierarchy.js";
import { getSession } from "../tools/session.js";

beforeAll(async () => {
  await Engine.init();
});

// eslint-disable-next-line @typescript-eslint/no-explicit-any
function out(result: { content: Array<{ type: string; text: string }>; isError?: boolean }): any {
  const parsed = JSON.parse(result.content[0].text);
  return parsed;
}

const pin = (number: string, type: string, x: number) => ({ number, name: number, type, x, y: 0 });

/** A CAN transceiver sheet: U1 (VCC, GND, TXD, CANH, CANL) + 120R termination R1. */
const canSheet = {
  name: "can",
  ports: ["TXD", { name: "CANH", direction: "Bidirectional" }],
  components: [
    {
      ref: "U1",
      value: "TCAN1044",
      footprint: "SOIC-8",
      x: 0,
      y: 0,
      pins: [
        pin("1", "PowerInput", 0),
        pin("2", "PowerInput", 10),
        pin("3", "Input", 20),
        pin("4", "Bidirectional", 30),
        pin("5", "Bidirectional", 40),
      ],
    },
    {
      ref: "R1",
      value: "120R",
      footprint: "Resistor_SMD:R_0603",
      x: 0,
      y: 50,
      pins: [pin("1", "Passive", 0), pin("2", "Passive", 10)],
    },
  ],
  nets: {
    VCC5: ["U1.1"],
    GND: ["U1.2"],
    TXD: ["U1.3"],
    CANH: ["U1.4", "R1.1"],
    CANL: ["U1.5", "R1.2"],
  },
};

/** Root: an MCU with two TX pins and a 5 V rail fed through a ferrite (flagged). */
const root = {
  title: "can x2",
  components: [
    {
      ref: "U10",
      value: "STM32",
      footprint: "LQFP-48",
      x: 0,
      y: 0,
      pins: [pin("1", "Output", 0), pin("2", "Output", 10), pin("3", "PowerInput", 20)],
    },
    {
      ref: "FB1",
      value: "ferrite",
      footprint: "Resistor_SMD:R_0603",
      x: 0,
      y: 40,
      pins: [pin("1", "Passive", 0), pin("2", "Passive", 10)],
    },
    {
      ref: "J1",
      value: "CAN bus",
      footprint: "PinHeader_1x02",
      x: 0,
      y: 80,
      pins: [pin("1", "Passive", 0), pin("2", "Passive", 10)],
    },
  ],
  nets: {
    TX_A: ["U10.1"],
    TX_B: ["U10.2"],
    GND: ["U10.3"],
    VCC5: ["FB1.2"],
    CAN_H_BUS: ["J1.1"],
  },
  global_nets: ["GND", "VCC5"],
  sheets: [canSheet],
  instances: [
    { name: "CAN1", sheet: "can", connections: { TXD: "TX_A", CANH: "CAN_H_BUS" } },
    { name: "CAN2", sheet: "can", connections: { TXD: "TX_B" }, ref_offset: 200 },
  ],
};

describe("hierarchical schematics", () => {
  it("offsetRef renumbers KiCad-style", () => {
    expect(offsetRef("U1", 100)).toBe("U101");
    expect(offsetRef("C12", 100)).toBe("C112");
    expect(offsetRef("J", 5)).toBe("J5");
  });

  it("flat sheets pass through flatten untouched", async () => {
    const sheet = { components: [], wires: [], junctions: [], labels: [] };
    const r = await flattenSchematic(sheet, async () => new Map());
    expect(r.sheet).toBe(sheet);
    expect(r.diagnostics).toEqual([]);
  });

  it("create_schematic flattens repeated instances with scoped nets and refs", async () => {
    const created = out(await createSchematic(root));
    expect(created.success).toBe(true);
    expect(created.components).toBe(3 + 2 + 2);
    const nets = created.nets as Record<string, string[]>;
    expect(nets.TX_A).toEqual(["U10.1", "U1_CAN1.3"]);
    expect(nets.TX_B).toEqual(["U10.2", "U201.3"]);
    expect(nets.CAN_H_BUS).toEqual(["J1.1", "R1_CAN1.1", "U1_CAN1.4"]);
    expect(nets["CAN1/CANL"]).toEqual(["R1_CAN1.2", "U1_CAN1.5"]);
    expect(nets["CAN2/CANH"]).toEqual(["R201.1", "U201.4"]);
    expect(nets.VCC5.sort()).toEqual(["FB1.2", "U1_CAN1.1", "U201.1"]);
    expect(created.hierarchy.instances.map((i: { path: string }) => i.path)).toEqual(["CAN1", "CAN2"]);
    // CAN2 leaves its CANH port open — reported, not fatal.
    expect(created.hierarchy.issues).toEqual([
      { severity: "Warning", message: "Hierarchical port 'CANH' of sheet instance 'CAN2' is not connected" },
    ]);
  });

  it("run_erc reports hierarchy issues and honours power flags", async () => {
    const created = out(await createSchematic(root));
    const before = out(await runErc({ document_id: created.document_id }));
    const msgs = (before.details as Array<{ message: string }>).map((d) => d.message);
    expect(msgs).toContain("Hierarchical port 'CANH' of sheet instance 'CAN2' is not connected");
    expect(msgs).toContain("Power input pin on net 'VCC5' has no power source");
    expect(msgs.some((m) => m.includes("Duplicate reference"))).toBe(false);

    // VCC5 is fed through FB1 — flag it as driven.
    out(
      await updateSchematic({
        document_id: created.document_id,
        power_flags: [{ net: "VCC5", note: "via FB1 from VBUS" }, "NOPE"],
      }),
    );
    const after = out(await runErc({ document_id: created.document_id }));
    const msgs2 = (after.details as Array<{ message: string }>).map((d) => d.message);
    expect(msgs2).not.toContain("Power input pin on net 'VCC5' has no power source");
    expect(msgs2).toContain("Power flag on net 'NOPE', which does not exist");
  });

  it("flags inside a child sheet are scoped to the instance", async () => {
    const created = out(
      await createSchematic({
        ...root,
        sheets: [{ ...canSheet, nets: { ...canSheet.nets, VLOC: ["U1.1"], VCC5: [] }, power_flags: ["VLOC"] }],
      }),
    );
    expect(created.power_flags).toEqual(["CAN1/VLOC", "CAN2/VLOC"]);
    const erc = out(await runErc({ document_id: created.document_id }));
    const msgs = (erc.details as Array<{ message: string }>).map((d) => d.message);
    expect(msgs.some((m) => m.includes("'CAN1/VLOC' has no power source"))).toBe(false);
  });

  it("reports scope collisions and unknown ports/sheets", async () => {
    const created = out(
      await createSchematic({
        ...root,
        nets: { ...root.nets, "CAN1/CANL": ["J1.2"] },
        global_nets: ["GND", "VCC5", "TXD"],
        instances: [
          ...root.instances,
          { name: "CAN3", sheet: "can", connections: { RXD: "X" } },
          { name: "ZZ", sheet: "missing" },
        ],
      }),
    );
    const issues = (created.hierarchy.issues as Array<{ message: string }>).map((d) => d.message);
    expect(issues).toContain("Net scope collision: port 'TXD' of sheet 'can' is also a global net");
    expect(issues.some((m) => m.includes("flattens to 'CAN1/CANL'"))).toBe(true);
    expect(issues).toContain("Sheet instance 'CAN3' connects 'RXD', which is not a port of sheet 'can'");
    expect(issues).toContain("Sheet instance 'ZZ' references unknown sheet 'missing'");
  });

  it("update_schematic edits instances; inspect_schematic reports tree, refs and BOM", async () => {
    const created = out(await createSchematic(root));
    const id = created.document_id;
    const upd = out(
      await updateSchematic({
        document_id: id,
        instances: [{ name: "CAN3", sheet: "can", connections: { TXD: "TX_A" }, ref_map: { U1: "U77" } }],
        remove_instances: ["CAN2"],
      }),
    );
    expect(upd.hierarchy.instances.map((i: { path: string }) => i.path)).toEqual(["CAN1", "CAN3"]);

    const info = out(await inspectSchematic({ document_id: id }));
    expect(info.hierarchical).toBe(true);
    expect(info.sheets[0].ports).toEqual([
      { name: "TXD", direction: "Passive" },
      { name: "CANH", direction: "Bidirectional" },
    ]);
    const can3 = info.instances.find((i: { path: string }) => i.path === "CAN3");
    expect(can3.refs).toEqual({ U1: "U77", R1: "R1_CAN3" });
    const transceivers = info.bom.find((l: { value: string }) => l.value === "TCAN1044");
    expect(transceivers.qty).toBe(2);
    expect(transceivers.refs).toEqual(["U1_CAN1", "U77"]);

    const one = out(await inspectSchematic({ document_id: id, instance: "CAN3" }));
    expect(Object.keys(one.nets)).toContain("CAN3/CANL");
    expect(Object.keys(one.nets)).not.toContain("CAN1/CANL");
  });

  it("place_components and export_kicad see the flattened design", async () => {
    const created = out(await createSchematic(root));
    const id = created.document_id;
    const placed = out(await placeComponents({ document_id: id, board_width: 120, board_height: 80 }));
    expect(placed.success).toBe(true);
    const kicad = out(await exportKicad({ document_id: id, filename: "can.kicad_sch" }));
    const text = JSON.stringify(kicad);
    expect(text).toContain("U201");
    expect(text).toContain("U1_CAN1");
  });

  it("stored document keeps the hierarchy as source (root sheet only at top level)", async () => {
    const created = out(await createSchematic(root));
    const doc = getSession(created.document_id) as unknown as {
      schematic: { components: unknown[]; hierarchy?: { sheets: unknown[] } };
    };
    expect(doc.schematic.components).toHaveLength(3);
    expect(doc.schematic.hierarchy?.sheets).toHaveLength(1);
  });
});
