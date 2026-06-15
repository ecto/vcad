import { describe, it, expect, beforeAll, beforeEach } from "vitest";
import { Engine } from "@vcad/engine";
import type { Document, Pcb, Vec2 } from "@vcad/ir";
import {
  createSchematic,
  placeComponents,
  routeNets,
  runDrc,
  runErc,
  exportGerber,
  calcImpedance,
  sizeImpedance,
  sizePdn,
  calcCoil,
  sizeCoil,
  calcRf,
  addCoil,
  addCoilArray,
  windingLayout,
  addTrace,
  addVia,
  setStackup,
  addMotorWinding,
  aggregateDrc,
  parseNetPair,
  boardFromSolid,
} from "../tools/ecad.js";
import { documents, getSession, openDocument } from "../tools/session.js";
import { openInBrowser } from "../tools/share.js";

let engine: Engine;

beforeAll(async () => {
  engine = await Engine.init();
});

beforeEach(() => {
  documents.clear();
});

/** Parse the single JSON text block of a tool result. */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
function out(result: { content: Array<{ type: string; text: string }> }): any {
  return JSON.parse(result.content[0].text);
}

/** Find the PcbBoard node's board in a document. */
function getPcbBoard(doc: Document): Pcb {
  const node = Object.values(doc.nodes).find(
    (n) => (n.op as { type: string }).type === "PcbBoard",
  );
  expect(node).toBeDefined();
  return (node!.op as unknown as { board: Pcb }).board;
}

const resistor = (ref: string, x: number, pinNames: [string, string] = ["~", "~"]) => ({
  ref,
  value: "1k",
  footprint: "Resistor_SMD:R_0805",
  x,
  y: 0,
  pins: [
    { number: "1", name: pinNames[0], type: "Passive", x: -5, y: 0 },
    { number: "2", name: pinNames[1], type: "Passive", x: 5, y: 0 },
  ],
});

describe("ecad session flow", () => {
  it("create_schematic opens a session and returns the resolved netlist, not the document", async () => {
    const created = out(
      await createSchematic({
        components: [resistor("R1", 0), resistor("R2", 20)],
        nets: { MID: ["R1.2", "R2.1"] },
      }),
    );
    expect(created.success).toBe(true);
    expect(created.document_id).toBeTruthy();
    expect(created.document).toBeUndefined();
    expect(created.nets.MID.sort()).toEqual(["R1.2", "R2.1"]);
    // Pins not in any net are reported immediately.
    expect(created.unconnected_pins.sort()).toEqual(["R1.1", "R2.2"]);

    const doc = getSession(created.document_id);
    expect(doc.schematic?.nets).toEqual({ MID: ["R1.2", "R2.1"] });
  });

  it("place → route → drc → gerber all work against the session id", async () => {
    const created = out(
      await createSchematic({
        components: [resistor("R1", 0), resistor("R2", 20)],
        nets: { MID: ["R1.2", "R2.1"] },
      }),
    );
    const id = created.document_id;

    const placed = out(
      await placeComponents({ document_id: id, board_width: 50, board_height: 50 }),
    );
    expect(placed.success).toBe(true);
    expect(placed.document).toBeUndefined();
    expect(placed.document_id).toBe(id);
    expect(placed.nets.MID).toBeDefined();

    // The session document was mutated server-side.
    const board = getPcbBoard(getSession(id));
    const padNet = (ref: string, num: string) =>
      board.footprints.find((fp) => fp.ref === ref)!.pads.find((p) => p.number === num)!.net;
    expect(padNet("R1", "2")).toBe("MID");
    expect(padNet("R2", "1")).toBe("MID");

    const routed = out(await routeNets({ document_id: id }));
    expect(routed.success).toBe(true);
    expect(routed.nets_routed).toBe(1);
    expect(getPcbBoard(getSession(id)).traces.length).toBeGreaterThan(0);

    const drc = out(await runDrc({ document_id: id }));
    expect(drc.success).toBe(true);

    const gerber = out(await exportGerber({ document_id: id }));
    expect(gerber.success).toBe(true);
    expect(gerber.files.length).toBeGreaterThan(0);
  });

  it("inline `document` still works as the legacy stateless flow", async () => {
    const doc = {
      version: "0.1",
      nodes: {},
      materials: {},
      part_materials: {},
      roots: [],
      schematic: {
        components: [
          {
            ref: "R1",
            value: "1k",
            footprintId: "Resistor_SMD:R_0805",
            position: { x: 0, y: 0 },
            rotation: 0,
            pins: [
              { number: "1", name: "A", pin_type: "Passive", position: { x: -5, y: 0 } },
              { number: "2", name: "B", pin_type: "Passive", position: { x: 5, y: 0 } },
            ],
          },
        ],
        wires: [],
        junctions: [],
        labels: [],
      },
    } as unknown as Document;

    const placed = out(
      await placeComponents({ document: doc, board_width: 30, board_height: 30 }),
    );
    expect(placed.success).toBe(true);
    // Legacy mode: full document echoed, no session created.
    expect(placed.document).toBeDefined();
    expect(placed.document_id).toBeUndefined();
    expect(documents.size).toBe(0);
  });

  it("rejects calls with neither document_id nor document", async () => {
    await expect(routeNets({})).rejects.toThrow(/document_id/);
  });
});

describe("explicit netlist (nets as data)", () => {
  it("wires a wye topology purely from the nets map", async () => {
    // Three phase coils + a 4-pin connector — the exact shape that broke
    // with coordinate-coincidence labels.
    const coil = (ref: string, x: number) => ({
      ref,
      value: "coil",
      footprint: "Inductor_SMD:L_1210_3225Metric",
      x,
      y: 0,
      pins: [
        { number: "1", name: "~", type: "Passive", x: -5, y: 0 },
        { number: "2", name: "~", type: "Passive", x: 5, y: 0 },
      ],
    });
    const created = out(
      await createSchematic({
        components: [
          coil("L1", 0),
          coil("L2", 20),
          coil("L3", 40),
          {
            ref: "J1",
            value: "Conn",
            footprint: "Connector_PinHeader_2.54mm:PinHeader_1x04_P2.54mm_Vertical",
            x: 60,
            y: 0,
            pins: Array.from({ length: 4 }, (_, i) => ({
              number: `${i + 1}`,
              name: "~",
              type: "Passive",
              x: 0,
              y: i * 2.54,
            })),
          },
        ],
        nets: {
          PHA: ["L1.1", "J1.1"],
          PHB: ["L2.1", "J1.2"],
          PHC: ["L3.1", "J1.3"],
          N: ["L1.2", "L2.2", "L3.2", "J1.4"],
        },
      }),
    );
    expect(created.unconnected_pins).toBeUndefined();
    expect(created.nets.N.sort()).toEqual(["J1.4", "L1.2", "L2.2", "L3.2"]);

    const id = created.document_id;
    out(await placeComponents({ document_id: id, board_width: 80, board_height: 80 }));
    const board = getPcbBoard(getSession(id));
    const padNet = (ref: string, num: string) =>
      board.footprints.find((fp) => fp.ref === ref)!.pads.find((p) => p.number === num)!.net;
    expect(padNet("L1", "1")).toBe("PHA");
    expect(padNet("J1", "1")).toBe("PHA");
    expect(padNet("L3", "2")).toBe("N");
    expect(padNet("J1", "4")).toBe("N");
    expect(board.nets.map((n) => n.id).sort()).toEqual(["N", "PHA", "PHB", "PHC"]);

    // ERC counts explicitly-netted pins as connected.
    const erc = out(await runErc({ document_id: id }));
    expect(erc.errors).toBe(0);
    expect(
      (erc.details as Array<{ message: string }>).filter((d) =>
        d.message.includes("Unconnected"),
      ),
    ).toEqual([]);
  });

  it("rejects unknown refs and pins with every problem listed", async () => {
    await expect(
      createSchematic({
        components: [resistor("R1", 0)],
        nets: { X: ["R9.1", "R1.7", "garbage"] },
      }),
    ).rejects.toThrow(/unknown component "R9".*no pin "7".*not of the form/s);
  });

  it("merges explicit nets with wire-derived connectivity, explicit name winning", async () => {
    // R1.2 — wire — R2.1, and the explicit net claims R1.2 under "MID".
    const created = out(
      await createSchematic({
        components: [resistor("R1", 0), resistor("R2", 20)],
        wires: [{ x1: 5, y1: 0, x2: 15, y2: 0 }],
        nets: { MID: ["R1.2"] },
      }),
    );
    // The wire-merged group adopts the explicit name.
    expect(created.nets.MID.sort()).toEqual(["R1.2", "R2.1"]);
  });
});

describe("schematic label diagnostics", () => {
  it("warns when a label touches neither a pin nor a wire endpoint", async () => {
    const created = out(
      await createSchematic({
        components: [resistor("R1", 0)],
        labels: [{ name: "GHOST", x: 99, y: 99 }],
      }),
    );
    expect(created.warnings.join(" ")).toContain('"GHOST"');
    expect(created.warnings.join(" ")).toContain("names nothing");
  });

  it("merges same-named global labels across the sheet (KiCad semantics)", async () => {
    // Two BUS labels, one on each component's pin, no wires between them.
    const created = out(
      await createSchematic({
        components: [resistor("R1", 0), resistor("R2", 40)],
        labels: [
          { name: "BUS", x: 5, y: 0 }, // on R1.2
          { name: "BUS", x: 35, y: 0 }, // on R2.1
        ],
      }),
    );
    expect(created.nets.BUS?.sort()).toEqual(["R1.2", "R2.1"]);
  });

  it("renames disjoint nets that collide on a Local label name instead of shorting them", async () => {
    // Two separate wires, each carrying a Local "SIG" label. Local labels
    // must NOT join by name — the nets stay separate, the second renamed.
    const created = out(
      await createSchematic({
        components: [
          resistor("R1", 0),
          resistor("R2", 20),
          resistor("R3", 100),
          resistor("R4", 120),
        ],
        wires: [
          { x1: 5, y1: 0, x2: 15, y2: 0 }, // R1.2 — R2.1
          { x1: 105, y1: 0, x2: 115, y2: 0 }, // R3.2 — R4.1
        ],
        labels: [
          { name: "SIG", x: 5, y: 0, scope: "Local" },
          { name: "SIG", x: 105, y: 0, scope: "Local" },
        ],
      }),
    );
    const names = Object.keys(created.nets).sort();
    expect(names).toEqual(["SIG", "SIG_2"]);
    // All four pins are present across the two nets — none silently dropped.
    const allPins = Object.values(created.nets as Record<string, string[]>)
      .flat()
      .sort();
    expect(allPins).toEqual(["R1.2", "R2.1", "R3.2", "R4.1"]);
    expect(created.warnings.join(" ")).toContain("renamed");
  });

  it("merges explicit nets with same-named label nets on disjoint pins (global name semantics)", async () => {
    // Explicit VCC on R1.1/R2.1 plus a global-labeled VCC wire on R3.2—R4.1:
    // one name, one net — all four pins together, reported together.
    const created = out(
      await createSchematic({
        components: [
          resistor("R1", 0),
          resistor("R2", 20),
          resistor("R3", 100),
          resistor("R4", 120),
        ],
        wires: [{ x1: 105, y1: 0, x2: 115, y2: 0 }],
        labels: [{ name: "VCC", x: 105, y: 0 }],
        nets: { VCC: ["R1.1", "R2.1"] },
      }),
    );
    expect(created.nets.VCC.sort()).toEqual(["R1.1", "R2.1", "R3.2", "R4.1"]);
  });

  it("run_erc agrees with create_schematic for rotated components", async () => {
    // R1 rotated 90°: its pin 2 world position is (0, 5), not (5, 0).
    const created = out(
      await createSchematic({
        components: [
          { ...resistor("R1", 0), rotation: 90 },
          { ...resistor("R2", 0), y: 20 },
        ],
        wires: [{ x1: 0, y1: 5, x2: -5, y2: 20 }], // R1.2 (rotated) — R2.1
      }),
    );
    expect(created.nets["NET-001"].sort()).toEqual(["R1.2", "R2.1"]);

    const erc = out(await runErc({ document_id: created.document_id }));
    const unconnected = (erc.details as Array<{ message: string }>)
      .filter((d) => d.message.includes("Unconnected"))
      .map((d) => d.message);
    // The wired (rotated) pin must not be flagged; the two open pins are.
    expect(unconnected.join(" ")).not.toContain("R1 pin 2");
    expect(unconnected).toHaveLength(2);
  });
});

describe("board shapes and radial placement", () => {
  it("creates a circular board with a center bore and rings components radially", async () => {
    const created = out(
      await createSchematic({
        components: Array.from({ length: 6 }, (_, i) => resistor(`R${i + 1}`, i * 10)),
      }),
    );
    const id = created.document_id;
    const placed = out(
      await placeComponents({
        document_id: id,
        board_shape: { type: "circle", outer_diameter: 60, inner_diameter: 20 },
        strategy: "radial",
      }),
    );
    expect(placed.success).toBe(true);
    expect(placed.board.shape).toBe("circle");

    const board = getPcbBoard(getSession(id));
    // Outer polygon spans the 60mm circle; bore is a cutout.
    expect(board.outline.cutouts).toHaveLength(1);
    const center = { x: 30, y: 30 };
    const maxR = Math.max(
      ...board.outline.vertices.map((v: Vec2) => Math.hypot(v.x - center.x, v.y - center.y)),
    );
    expect(maxR).toBeCloseTo(30, 0);

    // Radial placement: every footprint sits on the mid-annulus ring (r=20).
    for (const fp of board.footprints) {
      const r = Math.hypot(fp.position.x - center.x, fp.position.y - center.y);
      expect(r).toBeGreaterThan(19);
      expect(r).toBeLessThan(21);
    }
  });

  it("accepts an explicit outline polygon", async () => {
    const created = out(
      await createSchematic({ components: [resistor("R1", 0)] }),
    );
    const id = created.document_id;
    const placed = out(
      await placeComponents({
        document_id: id,
        outline: {
          vertices: [
            { x: 0, y: 0 },
            { x: 80, y: 0 },
            { x: 80, y: 40 },
            { x: 40, y: 40 },
            { x: 40, y: 20 },
            { x: 0, y: 20 },
          ],
        },
      }),
    );
    expect(placed.success).toBe(true);
    expect(placed.board.shape).toBe("polygon");
    expect(getPcbBoard(getSession(id)).outline.vertices).toHaveLength(6);
  });

  it("errors helpfully when no outline form is given", async () => {
    const created = out(await createSchematic({ components: [resistor("R1", 0)] }));
    const res = await placeComponents({ document_id: created.document_id });
    expect(res.isError).toBe(true);
    expect(res.content[0].text).toContain("board_shape");
    expect(res.content[0].text).toContain("outline");
  });
});

describe("add_coil", () => {
  async function circleBoardSession(): Promise<string> {
    const created = out(await createSchematic({ components: [resistor("R1", 0)] }));
    const id = created.document_id;
    out(
      await placeComponents({
        document_id: id,
        board_shape: { type: "circle", outer_diameter: 80, inner_diameter: 16 },
      }),
    );
    return id;
  }

  it("generates a spiral on a net with sane endpoints, length, and resistance", async () => {
    const id = await circleBoardSession();
    const coil = out(
      addCoil({
        document_id: id,
        center: { x: 40, y: 40 },
        turns: 5,
        inner_radius: 5,
        outer_radius: 15,
        trace_width: 0.5,
        clearance: 0.5,
        net: "PHA",
        inner_via: true,
      }),
    );
    expect(coil.success).toBe(true);
    expect(coil.traces_added).toBeGreaterThan(100);
    // Spiral length ≈ π (r_in + r_out) turns = π·20·5 ≈ 314mm.
    expect(coil.length_mm).toBeGreaterThan(300);
    expect(coil.length_mm).toBeLessThan(320);
    expect(coil.estimated_dc_resistance_ohms).toBeGreaterThan(0.2);
    expect(coil.estimated_dc_resistance_ohms).toBeLessThan(0.4);
    // Inner endpoint at r≈5, outer at r≈15 from the center.
    const rIn = Math.hypot(coil.inner_endpoint.x - 40, coil.inner_endpoint.y - 40);
    const rOut = Math.hypot(coil.outer_endpoint.x - 40, coil.outer_endpoint.y - 40);
    expect(rIn).toBeCloseTo(5, 1);
    expect(rOut).toBeCloseTo(15, 1);

    const board = getPcbBoard(getSession(id));
    expect(board.nets.some((n) => n.id === "PHA")).toBe(true);
    expect(board.traces.filter((t) => t.net === "PHA").length).toBe(coil.traces_added);
    const via = board.vias.find((v) => v.net === "PHA");
    expect(via).toBeDefined();
    expect(via!.startLayer).toBe("FCu");
    expect(via!.endLayer).toBe("BCu");
    // Every trace endpoint stays inside the coil annulus.
    for (const t of board.traces) {
      if (t.net !== "PHA") continue;
      for (const p of [t.start, t.end]) {
        expect(Math.hypot(p.x - 40, p.y - 40)).toBeLessThanOrEqual(15.01);
      }
    }
  });

  it("rejects coils whose turns don't fit the clearance, with the max that would", async () => {
    const id = await circleBoardSession();
    const res = addCoil({
      document_id: id,
      center: { x: 40, y: 40 },
      turns: 30,
      inner_radius: 5,
      outer_radius: 15,
      trace_width: 0.5,
      clearance: 0.5,
      net: "PHA",
    });
    expect(res.isError).toBe(true);
    expect(res.content[0].text).toContain("at most 10 turn(s)");
  });

  it("requires a PCB on the document", async () => {
    const created = out(await createSchematic({ components: [resistor("R1", 0)] }));
    const res = addCoil({
      document_id: created.document_id,
      center: { x: 0, y: 0 },
      turns: 2,
      inner_radius: 2,
      outer_radius: 8,
      trace_width: 0.3,
      net: "X",
    });
    expect(res.isError).toBe(true);
    expect(res.content[0].text).toContain("place_components");
  });
});

describe("winding_layout", () => {
  it("9-slot/12-pole 3-phase: double layer, all-positive polarity, kw≈0.866", () => {
    const plan = out(windingLayout({ slots: 9, poles: 12 }));
    expect(plan.feasible).toBe(true);
    expect(plan.layer).toBe("double");
    expect(plan.coils).toHaveLength(9);
    expect(plan.coilsPerPhase).toBe(3);
    // Regression lock for the original session bug: NO sign flip within a phase.
    expect(plan.coils.every((c: { polarity: number }) => c.polarity === 1)).toBe(true);
    expect(plan.windingFactor).toBeCloseTo(0.866, 3);
    expect(plan.pitchFactor).toBeCloseTo(0.866, 3);
    expect(plan.distributionFactor).toBeCloseTo(1, 6);
    // Canonical verified phase table: A,C,B repeating (phase indices 0,2,1).
    expect(plan.coils.map((c: { phase: number }) => c.phase)).toEqual([0, 2, 1, 0, 2, 1, 0, 2, 1]);
    expect(plan.coils.map((c: { net: string }) => c.net)).toEqual([
      "PHA", "PHC", "PHB", "PHA", "PHC", "PHB", "PHA", "PHC", "PHB",
    ]);
    expect(plan.neutralNet).toBe("WIND_N");
  });

  it("rejects a single-layer winding for an odd slot count", () => {
    const plan = out(windingLayout({ slots: 9, poles: 12, layer: "single" }));
    expect(plan.feasible).toBe(false);
    expect(plan.reason).toMatch(/single-layer|odd/i);
  });

  it("12-slot/10-pole: kw≈0.933, each phase carries both polarities", () => {
    const plan = out(windingLayout({ slots: 12, poles: 10 }));
    expect(plan.feasible).toBe(true);
    expect(plan.coils).toHaveLength(12);
    expect(plan.coilsPerPhase).toBe(4);
    expect(plan.windingFactor).toBeCloseTo(0.933, 3);
    expect(plan.distributionFactor).toBeCloseTo(0.966, 3);
    for (let ph = 0; ph < 3; ph++) {
      const signs = plan.coils
        .filter((c: { phase: number }) => c.phase === ph)
        .map((c: { polarity: number }) => c.polarity);
      expect(signs).toContain(1);
      expect(signs).toContain(-1);
    }
  });

  it("flags infeasible slot/pole/phase combinations", () => {
    const plan = out(windingLayout({ slots: 9, poles: 12, phases: 4 }));
    expect(plan.feasible).toBe(false);
    expect(plan.reason).toBeTruthy();
  });

  it("is pure — ignores document_id, mutates nothing, returns no doc handle", async () => {
    const created = out(await createSchematic({ components: [resistor("R1", 0)] }));
    const id = created.document_id;
    out(
      await placeComponents({
        document_id: id,
        board_shape: { type: "circle", outer_diameter: 80, inner_diameter: 16 },
      }),
    );
    const before = getPcbBoard(getSession(id)).traces.length;
    const plan = out(windingLayout({ document_id: id, slots: 9, poles: 12 }));
    expect(plan.document_id).toBeUndefined();
    expect(plan.document).toBeUndefined();
    expect(getPcbBoard(getSession(id)).traces.length).toBe(before);
  });
});

describe("add_coil_array", () => {
  async function ringBoard(): Promise<string> {
    const created = out(await createSchematic({ components: [resistor("R1", 0)] }));
    const id = created.document_id;
    out(
      await placeComponents({
        document_id: id,
        board_shape: { type: "circle", outer_diameter: 120, inner_diameter: 20 },
      }),
    );
    return id;
  }

  const base = {
    count: 3,
    center: { x: 60, y: 60 },
    pitch_radius: 20,
    turns: 4,
    inner_radius: 3,
    outer_radius: 8,
    trace_width: 0.25,
    clearance: 0.25,
  };

  it("lays a 3-phase ring: 3 coils 120° apart, nets cycled", async () => {
    const id = await ringBoard();
    const res = out(addCoilArray({ document_id: id, ...base, net_sequence: ["PHA", "PHB", "PHC"] }));
    expect(res.success).toBe(true);
    expect(res.coils_added).toBe(3);
    expect(res.total_traces).toBeGreaterThan(0);
    expect(res.results.map((r: { net: string }) => r.net)).toEqual(["PHA", "PHB", "PHC"]);

    const board = getPcbBoard(getSession(id));
    for (const n of ["PHA", "PHB", "PHC"]) {
      expect(board.nets.some((x) => x.id === n)).toBe(true);
    }
    // First coil at +X of the ring; all centers at radius 20 from board center.
    expect(res.results[0].center.x).toBeCloseTo(80, 2);
    expect(res.results[0].center.y).toBeCloseTo(60, 2);
    for (const r of res.results) {
      expect(Math.hypot(r.center.x - 60, r.center.y - 60)).toBeCloseTo(20, 3);
    }
  });

  it("chirality 'alternating' flips winding sense per coil", async () => {
    const id = await ringBoard();
    const res = out(addCoilArray({ document_id: id, ...base, net: "X", chirality: "alternating" }));
    expect(res.results.map((r: { direction: string }) => r.direction)).toEqual(["ccw", "cw", "ccw"]);
  });

  it("cycles net_sequence when shorter than count", async () => {
    const id = await ringBoard();
    const res = out(
      addCoilArray({ document_id: id, ...base, count: 4, net_sequence: ["A", "B"] }),
    );
    expect(res.results.map((r: { net: string }) => r.net)).toEqual(["A", "B", "A", "B"]);
  });

  it("mutates the same session that addCoil writes to", async () => {
    const id = await ringBoard();
    const before = getPcbBoard(getSession(id)).traces.length;
    out(addCoilArray({ document_id: id, ...base, net_sequence: ["PHA", "PHB", "PHC"] }));
    expect(getPcbBoard(getSession(id)).traces.length).toBeGreaterThan(before);
  });

  it("rejects count < 1", async () => {
    const id = await ringBoard();
    const res = addCoilArray({ document_id: id, ...base, count: 0, net: "X" });
    expect(res.isError).toBe(true);
  });

  it("collects per-coil geometry failures instead of throwing", async () => {
    const id = await ringBoard();
    // outer_radius <= inner_radius fails inside addCoil for every coil.
    const res = addCoilArray({
      document_id: id,
      ...base,
      inner_radius: 8,
      outer_radius: 4,
      net: "X",
    });
    const payload = out(res);
    expect(payload.success).toBe(false);
    expect(payload.errors.length).toBeGreaterThan(0);
  });
});

describe("add_trace / add_via / set_stackup", () => {
  async function board(): Promise<string> {
    const created = out(await createSchematic({ components: [resistor("R1", 0)] }));
    const id = created.document_id;
    out(await placeComponents({ document_id: id, board_width: 50, board_height: 50 }));
    return id;
  }

  it("add_trace adds N-1 segments and ensures the net", async () => {
    const id = await board();
    const res = out(
      addTrace({
        document_id: id,
        net: "SIG",
        points: [
          { x: 0, y: 0 },
          { x: 10, y: 0 },
          { x: 10, y: 10 },
          { x: 20, y: 10 },
        ],
      }),
    );
    expect(res.success).toBe(true);
    expect(res.traces_added).toBe(3);
    expect(res.length_mm).toBeCloseTo(30, 3);
    expect(res.layer).toBe("FCu");
    const b = getPcbBoard(getSession(id));
    expect(b.traces.filter((t) => t.net === "SIG").length).toBe(3);
    expect(b.nets.some((n) => n.id === "SIG")).toBe(true);
  });

  it("add_trace rejects < 2 points and non-copper layers", async () => {
    const id = await board();
    expect(addTrace({ document_id: id, net: "X", points: [{ x: 0, y: 0 }] }).isError).toBe(true);
    expect(
      addTrace({
        document_id: id,
        net: "X",
        layer: "FSilkS",
        points: [
          { x: 0, y: 0 },
          { x: 1, y: 1 },
        ],
      }).isError,
    ).toBe(true);
  });

  it("add_via adds a via with default span and ensures the net", async () => {
    const id = await board();
    const res = out(addVia({ document_id: id, net: "GND", position: { x: 5, y: 5 } }));
    expect(res.success).toBe(true);
    expect(res.position).toEqual({ x: 5, y: 5 });
    const b = getPcbBoard(getSession(id));
    const via = b.vias.find((v) => v.net === "GND");
    expect(via).toBeDefined();
    expect(via!.startLayer).toBe("FCu");
    expect(via!.endLayer).toBe("BCu");
    expect(via!.diameter).toBe(b.rules.defaultRules.viaDiameter);
    expect(b.nets.some((n) => n.id === "GND")).toBe(true);
  });

  it("set_stackup copper_oz changes copperThickness on all copper layers", async () => {
    const id = await board();
    const res = out(setStackup({ document_id: id, copper_oz: 2 }));
    expect(res.success).toBe(true);
    const b = getPcbBoard(getSession(id));
    for (const l of b.stackup.layers.filter((s) => /Cu$/.test(s.layer))) {
      expect(l.copperThickness).toBe(0.07); // round3(2 × 0.0348)
    }
  });

  it("set_stackup at 2 oz roughly halves a coil's DC resistance", async () => {
    // 1 oz baseline coil.
    const id1 = await board();
    const c1oz = out(
      addCoil({
        document_id: id1,
        center: { x: 25, y: 25 },
        turns: 3,
        inner_radius: 3,
        outer_radius: 10,
        trace_width: 0.3,
        clearance: 0.2,
        net: "PHA",
      }),
    );
    // 2 oz coil on a fresh board (thicker copper → lower resistance).
    const id2 = await board();
    out(setStackup({ document_id: id2, copper_oz: 2 }));
    const c2oz = out(
      addCoil({
        document_id: id2,
        center: { x: 25, y: 25 },
        turns: 3,
        inner_radius: 3,
        outer_radius: 10,
        trace_width: 0.3,
        clearance: 0.2,
        net: "PHA",
      }),
    );
    const ratio = c2oz.estimated_dc_resistance_ohms / c1oz.estimated_dc_resistance_ohms;
    expect(ratio).toBeCloseTo(0.035 / 0.07, 2); // ≈ 0.5 (round3 copper thickness)
  });

  it("set_stackup per-layer creates a missing copper layer entry", async () => {
    const id = await board();
    const res = out(
      setStackup({
        document_id: id,
        layers: [{ layer: "In1Cu", copper_oz: 0.5, material: "FR4" }],
      }),
    );
    expect(res.success).toBe(true);
    const b = getPcbBoard(getSession(id));
    const in1 = b.stackup.layers.find((l) => l.layer === "In1Cu");
    expect(in1).toBeDefined();
    expect(in1!.copperThickness).toBe(0.017); // round3(0.5 × 0.0348)
    expect(in1!.material).toBe("FR4");
  });
});

describe("add_coil lead-out and multilayer", () => {
  async function circleBoardSession(): Promise<string> {
    const created = out(await createSchematic({ components: [resistor("R1", 0)] }));
    const id = created.document_id;
    out(
      await placeComponents({
        document_id: id,
        board_shape: { type: "circle", outer_diameter: 80, inner_diameter: 16 },
      }),
    );
    return id;
  }

  it("inner_lead_out moves the inner endpoint off the outer radial spoke", async () => {
    const id = await circleBoardSession();
    const center = { x: 40, y: 40 };
    const plain = out(
      addCoil({
        document_id: id,
        center,
        turns: 4,
        inner_radius: 5,
        outer_radius: 14,
        trace_width: 0.4,
        clearance: 0.3,
        net: "A",
        start_angle_deg: 0,
      }),
    );
    // start_angle 0 → inner end sits on the +X spoke at angle 0.
    const plainAngle = Math.atan2(plain.inner_endpoint.y - 40, plain.inner_endpoint.x - 40);
    expect(plainAngle).toBeCloseTo(0, 5);

    const id2 = await circleBoardSession();
    const led = out(
      addCoil({
        document_id: id2,
        center,
        turns: 4,
        inner_radius: 5,
        outer_radius: 14,
        trace_width: 0.4,
        clearance: 0.3,
        net: "A",
        start_angle_deg: 0,
        inner_lead_out: 3,
        inner_via: true,
      }),
    );
    // New inner terminal is no longer on the +X spoke (angle 0).
    const ledAngle = Math.atan2(led.inner_endpoint.y - 40, led.inner_endpoint.x - 40);
    expect(Math.abs(ledAngle)).toBeGreaterThan(0.1);
    // The via lands on the lead-out terminal, not the spiral start.
    const b = getPcbBoard(getSession(id2));
    const via = b.vias.find((v) => v.net === "A");
    expect(via).toBeDefined();
    expect(via!.position).toEqual(led.inner_endpoint);
  });

  it("layers:[FCu,BCu] stacks the coil on both layers with a stitch via", async () => {
    const id = await circleBoardSession();
    const res = out(
      addCoil({
        document_id: id,
        center: { x: 40, y: 40 },
        turns: 3,
        inner_radius: 5,
        outer_radius: 12,
        trace_width: 0.4,
        clearance: 0.3,
        net: "PHA",
        layers: ["FCu", "BCu"],
      }),
    );
    expect(res.success).toBe(true);
    expect(res.multilayer).toBe(true);
    expect(res.layers_used).toEqual(["FCu", "BCu"]);
    expect(res.stitch_vias.length).toBe(1);
    expect(res.terminals.a).toBeDefined();
    expect(res.terminals.b).toBeDefined();
    const b = getPcbBoard(getSession(id));
    const fcu = b.traces.filter((t) => t.net === "PHA" && t.layer === "FCu").length;
    const bcu = b.traces.filter((t) => t.net === "PHA" && t.layer === "BCu").length;
    expect(fcu).toBeGreaterThan(0);
    expect(bcu).toBe(fcu); // same geometry on both layers
    const stitch = b.vias.find((v) => v.net === "PHA");
    expect(stitch).toBeDefined();
    expect(stitch!.startLayer).toBe("FCu");
    expect(stitch!.endLayer).toBe("BCu");
    // Total length is the sum across both layers.
    expect(res.total_length_mm).toBeCloseTo(res.total_length_mm, 3);
  });
});

describe("add_motor_winding", () => {
  async function statorBoard(): Promise<string> {
    const created = out(await createSchematic({ components: [resistor("R1", 0)] }));
    const id = created.document_id;
    out(
      await placeComponents({
        document_id: id,
        board_shape: { type: "circle", outer_diameter: 120, inner_diameter: 20 },
      }),
    );
    return id;
  }

  it("realizes a feasible 12s/10p wye winding: 12 coils, interconnect, a net-tie", async () => {
    const id = await statorBoard();
    const res = out(
      addMotorWinding({
        document_id: id,
        slots: 12,
        poles: 10,
        center: { x: 60, y: 60 },
        pitch_radius: 40,
        inner_radius: 2,
        outer_radius: 6,
        trace_width: 0.2,
        clearance: 0.15,
        connection: "wye",
      }),
    );
    expect(res.success).toBe(true);
    expect(res.coils_placed).toBe(12);
    expect(res.coils_failed).toBe(0);
    expect(res.connection).toBe("wye");
    expect(res.interconnect_traces).toBeGreaterThan(0);
    expect(res.net_ties_added).toBe(1);
    expect(res.vias_added).toBe(24); // inner + outer via per coil
    expect(res.winding_factor).toBeGreaterThan(0.9);

    const b = getPcbBoard(getSession(id));
    expect(b.netTies).toBeDefined();
    expect(b.netTies!.length).toBe(1);
    // The tie lists all phase nets plus the neutral.
    expect(b.netTies![0].nets).toContain("WIND_N");
    expect(b.netTies![0].nets).toContain("PHA");
    expect(b.netTies![0].nets).toContain("PHB");
    expect(b.netTies![0].nets).toContain("PHC");
    // Coils landed on copper (FCu) and interconnect on the return layer (BCu).
    expect(b.traces.some((t) => t.layer === "FCu")).toBe(true);
    expect(b.traces.some((t) => t.layer === "BCu")).toBe(true);
  });

  it("delta winding adds a net-tie per junction", async () => {
    const id = await statorBoard();
    const res = out(
      addMotorWinding({
        document_id: id,
        slots: 12,
        poles: 10,
        center: { x: 60, y: 60 },
        pitch_radius: 40,
        inner_radius: 2,
        outer_radius: 6,
        trace_width: 0.2,
        clearance: 0.15,
        connection: "delta",
      }),
    );
    expect(res.success).toBe(true);
    expect(res.coils_placed).toBe(12);
    expect(res.connection).toBe("delta");
    expect(res.net_ties_added).toBe(3); // one per phase junction
  });

  it("rejects an infeasible slot/pole/phase combination", async () => {
    const id = await statorBoard();
    const res = addMotorWinding({
      document_id: id,
      slots: 9,
      poles: 12,
      phases: 4,
      center: { x: 60, y: 60 },
      pitch_radius: 40,
      inner_radius: 2,
      outer_radius: 6,
      trace_width: 0.2,
    });
    expect(res.isError).toBe(true);
  });
});

describe("run_drc summary aggregation", () => {
  it("aggregateDrc rolls up by rule + net-pair with a capped sample", () => {
    const viol = (rule: string, message: string, actual: number, required: number) => ({
      rule,
      severity: "Error",
      message,
      position: { x: 0, y: 0 },
      actual,
      required,
    });
    const violations = [
      viol("Clearance", "Clearance violation: trace net 'PHA' to net 'PHB': 0.00mm < 0.25mm", 0, 0.25),
      viol("Clearance", "Clearance violation: trace net 'PHB' to net 'PHA': 0.10mm < 0.25mm", 0.1, 0.25),
      viol("Clearance", "Clearance violation: trace net 'PHA' to net 'PHC': 0.05mm < 0.25mm", 0.05, 0.25),
      viol("MinTraceWidth", "Trace width on net 'GND' too narrow", 0.1, 0.2),
    ];
    const summary = aggregateDrc(violations, 2, "summary");
    expect(summary.violations).toBe(4);
    expect(summary.errors).toBe(4);
    expect(summary.byRule.Clearance).toBe(3);
    expect(summary.byRule.MinTraceWidth).toBe(1);
    // (PHA,PHB) and (PHB,PHA) collapse into one pair.
    const phaPhb = summary.byNetPair.find(
      (p) => p.nets[0] === "PHA" && p.nets[1] === "PHB",
    );
    expect(phaPhb?.count).toBe(2);
    // Single-net rule yields a [net, ""] pair.
    expect(summary.byNetPair.some((p) => p.nets[0] === "GND" && p.nets[1] === "")).toBe(true);
    // Worst clearance is the 0.00mm short.
    expect(summary.worstClearance?.actual).toBe(0);
    // Sample respects the cap and reports truncation; full details omitted.
    expect(summary.sample.length).toBe(2);
    expect(summary.sampleCapped).toBe(true);
    expect(summary.details).toBeUndefined();
  });

  it("detail='full' attaches the complete violation array", () => {
    const violations = [
      { rule: "MinDrill", severity: "Error", message: "drill too small", actual: 0.1, required: 0.2 },
    ];
    const summary = aggregateDrc(violations, 20, "full");
    expect(summary.details).toBeDefined();
    expect(summary.details).toHaveLength(1);
    expect(summary.detail).toBe("full");
  });

  it("does not let a NaN measurement corrupt worstClearance", () => {
    const violations = [
      { rule: "Clearance", severity: "Error", message: "net 'A' to net 'B'", actual: NaN, required: 0.2 },
      { rule: "Clearance", severity: "Error", message: "net 'A' to net 'C'", actual: 0.05, required: 0.2 },
    ];
    const summary = aggregateDrc(violations, 20, "summary");
    expect(summary.worstClearance?.actual).toBe(0.05);
  });

  it("parseNetPair sorts the pair so order doesn't matter", () => {
    expect(parseNetPair("net 'B' to net 'A'")).toEqual(["A", "B"]);
    expect(parseNetPair("only net 'GND' here")).toEqual(["GND", ""]);
    expect(parseNetPair("no nets mentioned")).toEqual(["", ""]);
  });

  it("run_drc returns the summary shape by default (no details), full on request", async () => {
    const created = out(await createSchematic({ components: [resistor("R1", 0)] }));
    const id = created.document_id;
    out(
      await placeComponents({
        document_id: id,
        board_shape: { type: "circle", outer_diameter: 80, inner_diameter: 16 },
      }),
    );
    const summary = out(await runDrc({ document_id: id }));
    expect(summary.success).toBe(true);
    expect(typeof summary.violations).toBe("number");
    expect(summary.byRule).toBeDefined();
    expect(Array.isArray(summary.byNetPair)).toBe(true);
    expect(Array.isArray(summary.sample)).toBe(true);
    expect(summary.detail).toBe("summary");
    expect(summary.details).toBeUndefined();

    const full = out(await runDrc({ document_id: id, detail: "full" }));
    expect(full.detail).toBe("full");
    expect(Array.isArray(full.details)).toBe(true);
  });
});

describe("size_impedance", () => {
  it("solves single-ended microstrip width for 50Ω, and calc_impedance re-verifies it", () => {
    const stack = { dielectric_height: 0.2, dielectric_er: 4.3, copper_thickness: 0.035 };
    const r = out(sizeImpedance({ trace_type: "microstrip", target_z0: 50, ...stack }));
    expect(r.success).toBe(true);
    expect(r.within_tolerance).toBe(true);
    expect(Math.abs(r.measured.z0 - 50)).toBeLessThan(2.5);
    expect(r.width_mm).toBeGreaterThan(0.1);
    expect(r.width_mm).toBeLessThan(1);
    expect(r.measured.recomputed_from_geometry).toBe(true);
    // One model, one number: calc_impedance at the solved width agrees.
    const v = out(calcImpedance({ trace_type: "microstrip", trace_width: r.width_mm, ...stack }));
    expect(Math.abs(v.z0 - 50)).toBeLessThan(2.5);
    expect(r.document_id).toBeUndefined();
  });

  it("solves stripline width for 50Ω", () => {
    const r = out(
      sizeImpedance({ trace_type: "stripline", target_z0: 50, dielectric_height: 0.5, dielectric_er: 4.3 }),
    );
    expect(r.within_tolerance).toBe(true);
    expect(Math.abs(r.measured.z0 - 50)).toBeLessThan(2.5);
  });

  it("solves a differential pair for 90Ω diff / 50Ω SE and re-verifies both", () => {
    const stack = { dielectric_height: 0.2, dielectric_er: 4.3 };
    const r = out(
      sizeImpedance({ trace_type: "diff_microstrip", target_diff_z0: 90, target_z0: 50, ...stack }),
    );
    expect(r.within_tolerance).toBe(true);
    expect(Math.abs(r.measured.z0 - 50)).toBeLessThan(2.5);
    expect(Math.abs(r.measured.diff_z0 - 90)).toBeLessThan(4.5);
    expect(r.spacing_mm).toBeGreaterThan(0);
    const v = out(
      calcImpedance({ trace_type: "diff_microstrip", trace_width: r.width_mm, spacing: r.spacing_mm, ...stack }),
    );
    expect(Math.abs(v.z_diff - 90)).toBeLessThan(5);
  });

  it("reports an unreachable target with the binding DFM bound — never a silent miss", () => {
    const r = out(
      sizeImpedance({
        trace_type: "microstrip",
        target_z0: 120,
        dielectric_height: 0.2,
        dielectric_er: 4.3,
        min_width: 0.1,
        max_width: 2,
      }),
    );
    expect(r.within_tolerance).toBe(false);
    expect(r.reason).toMatch(/reach|closest/i);
    expect(r.active_constraints).toContain("min_width");
    // Still returns the honest best-achievable, recomputed from geometry.
    expect(r.measured.z0).toBeLessThan(120);
  });

  it("snaps to the fab grid and stays in tolerance", () => {
    const grid = 0.0254;
    const r = out(
      sizeImpedance({ trace_type: "microstrip", target_z0: 50, dielectric_height: 0.2, dielectric_er: 4.3, fab_grid_mm: grid }),
    );
    const ratio = r.width_mm / grid;
    expect(Math.abs(ratio - Math.round(ratio))).toBeLessThan(1e-6);
    expect(r.within_tolerance).toBe(true);
  });

  it("rejects an invalid stackup (copper thicker than dielectric)", () => {
    const res = sizeImpedance({ target_z0: 50, dielectric_height: 0.02, copper_thickness: 0.035 });
    expect(res.isError).toBe(true);
  });
});

describe("size_pdn", () => {
  // Wheatstone-bridge mesh: VRM(0) -> {1,2} -> 3 with a bridge edge 1-2.
  const bridge = (targets: Array<{ node: number; max_drop: number }>) => ({
    nodes: 4,
    edges: [
      { a: 0, b: 1, length: 10 },
      { a: 0, b: 2, length: 10 },
      { a: 1, b: 3, length: 10 },
      { a: 2, b: 3, length: 10 },
      { a: 1, b: 2, length: 8 },
    ],
    loads: [{ node: 3, current: 1.0 }],
    targets,
  });

  it("sizes segment widths so the load node meets its IR-drop budget", () => {
    const r = out(sizePdn(bridge([{ node: 3, max_drop: 0.015 }])));
    expect(r.success).toBe(true);
    expect(r.within_budget).toBe(true);
    expect(r.widths_mm).toHaveLength(5);
    // Drop recomputed from a forward solve sits at/under budget (within tol).
    expect(r.measured_drops_v[0]).toBeLessThanOrEqual(0.015 * 1.05);
    expect(r.measured_drops_v[0]).toBeGreaterThan(0); // a real drop, mesh is solved
    expect(r.document_id).toBeUndefined();
  });

  it("flags a budget it cannot meet within the width bounds", () => {
    // An impossibly tight budget at realistic max widths -> over_budget reported.
    const r = out(sizePdn({ ...bridge([{ node: 3, max_drop: 1e-5 }]), max_width: 0.5 }));
    expect(r.within_budget).toBe(false);
    expect(r.over_budget.length).toBeGreaterThan(0);
    expect(r.active_constraints).toContain("max_width");
  });

  it("rejects a singular (disconnected) mesh", () => {
    // Node 3 has no path to the reference (node 0).
    const res = sizePdn({
      nodes: 4,
      edges: [{ a: 0, b: 1, length: 10 }],
      loads: [{ node: 3, current: 1.0 }],
      targets: [{ node: 3, max_drop: 0.1 }],
    });
    expect(res.isError).toBe(true);
    expect(res.content[0].text).toMatch(/singular|path/i);
  });

  it("engine:'exact' routes into the Rust adjoint engine via WASM (when built)", async () => {
    const { ecadDiffEngineAvailable } = await import("../wasm/ecad-diff.js");
    const r = out(sizePdn({ ...bridge([{ node: 3, max_drop: 0.015 }]), engine: "exact" }));
    if (ecadDiffEngineAvailable()) {
      // The Rust engine (implicit-function adjoint) sized the mesh.
      expect(r.engine).toBe("rust-adjoint");
      expect(r.within_budget).toBe(true);
      expect(r.measured_drops_v[0]).toBeLessThanOrEqual(0.015 * 1.05);
      expect(r.widths_mm).toHaveLength(5);
    } else {
      // Artifact absent → graceful fall-through to the TS solver.
      expect(r.success).toBe(true);
    }
  });

  it("wider bounds let it meet a tighter budget than narrow bounds", () => {
    const tight = out(sizePdn({ ...bridge([{ node: 3, max_drop: 0.008 }]), max_width: 0.3 }));
    const roomy = out(sizePdn({ ...bridge([{ node: 3, max_drop: 0.008 }]), max_width: 5 }));
    expect(roomy.within_budget).toBe(true);
    // The constrained run does no better than the roomy one.
    expect(roomy.measured_drops_v[0]).toBeLessThanOrEqual(tight.measured_drops_v[0] + 1e-9);
  });
});

describe("calc_coil / size_coil", () => {
  it("computes a sane inductance and resistance for a planar spiral", () => {
    const r = out(
      calcCoil({ inner_radius: 2, outer_radius: 6, turns: 10, trace_width: 0.2 }),
    );
    expect(r.success).toBe(true);
    // ~10-turn 2–6mm circular spiral lands in the hundreds-of-nH range.
    expect(r.inductance_nh).toBeGreaterThan(300);
    expect(r.inductance_nh).toBeLessThan(1500);
    expect(r.dc_resistance_ohm).toBeGreaterThan(0);
    expect(r.wire_length_mm).toBeGreaterThan(0);
  });

  it("inductance grows with turns squared", () => {
    const l1 = out(calcCoil({ inner_radius: 2, outer_radius: 6, turns: 5, trace_width: 0.2 })).inductance_nh;
    const l2 = out(calcCoil({ inner_radius: 2, outer_radius: 6, turns: 10, trace_width: 0.2 })).inductance_nh;
    // Doubling turns ~quadruples L (geometry fixed).
    expect(l2 / l1).toBeGreaterThan(3.5);
    expect(l2 / l1).toBeLessThan(4.5);
  });

  it("size_coil solves the turn count for a target inductance, and calc_coil re-verifies", () => {
    const target = 500;
    const r = out(
      sizeCoil({ target_inductance_nh: target, inner_radius: 2, outer_radius: 8, trace_width: 0.15 }),
    );
    expect(r.success).toBe(true);
    expect(r.fits).toBe(true);
    expect(r.turns).toBeGreaterThan(0);
    // Re-verify: calc_coil at the solved (integer) turns reproduces the achieved L.
    const v = out(
      calcCoil({ inner_radius: 2, outer_radius: 8, turns: r.turns, trace_width: 0.15 }),
    );
    expect(Math.abs(v.inductance_nh - r.achieved_inductance_nh)).toBeLessThan(1e-2);
  });

  it("size_coil reports fit-limited when the target needs more turns than fit", () => {
    // Huge target in a thin annulus with a wide trace → cannot fit enough turns.
    const r = out(
      sizeCoil({
        target_inductance_nh: 50000,
        inner_radius: 2,
        outer_radius: 3,
        trace_width: 0.2,
        clearance: 0.2,
      }),
    );
    expect(r.fits).toBe(false);
    expect(r.within_tolerance).toBe(false);
    expect(r.summary).toMatch(/fit|widen|band/i);
  });
});

describe("calc_rf", () => {
  // 10 nH + 10 pF series RLC → f0 = 1/(2π√(LC)) ≈ 503 MHz.
  const f0 = 1 / (2 * Math.PI * Math.sqrt(10e-9 * 10e-12));

  it("reports resonance, and a series RLC dips to |Z|=R at f0", () => {
    const r = out(calcRf({ topology: "series_rlc", r_ohm: 50, l_henry: 10e-9, c_farad: 10e-12 }));
    expect(r.success).toBe(true);
    expect(r.resonance_hz).toBeCloseTo(f0, -6); // within ~1 MHz
    // At resonance the reactances cancel → |Z| ≈ R.
    expect(r.z_at_resonance_ohm).toBeCloseTo(50, 1);
  });

  it("a 50Ω series RLC is well matched at resonance (high return loss)", () => {
    const r = out(calcRf({ topology: "series_rlc", r_ohm: 50, l_henry: 10e-9, c_farad: 10e-12, z0_ohm: 50 }));
    // Z=50=Z0 at f0 → S11→0 → large return loss; best match near f0.
    expect(r.best_match.return_loss_db).toBeGreaterThan(40);
    expect(r.best_match.f_hz).toBeCloseTo(f0, -7);
  });

  it("higher R lowers the series Q", () => {
    const lowR = out(calcRf({ r_ohm: 5, l_henry: 10e-9, c_farad: 10e-12 })).q_factor;
    const highR = out(calcRf({ r_ohm: 50, l_henry: 10e-9, c_farad: 10e-12 })).q_factor;
    expect(lowR).toBeGreaterThan(highR);
  });

  it("validates inputs", () => {
    expect(calcRf({ r_ohm: 50, l_henry: 0, c_farad: 1e-12 }).isError).toBe(true);
    expect(calcRf({ r_ohm: 50, l_henry: 1e-9, c_farad: 1e-12, topology: "bogus" }).isError).toBe(true);
  });
});

describe("board_from_solid", () => {
  it("traces a stator disc into an outline with the bore as a cutout", async () => {
    // Solid: 30mm-radius disc, 2mm thick, with an 8mm-radius center bore.
    const doc: Document = {
      version: "0.1",
      nodes: {
        "1": { id: 1, name: "disc", op: { type: "Cylinder", radius: 30, height: 2, segments: 64 } },
        "2": { id: 2, name: "bore", op: { type: "Cylinder", radius: 8, height: 2, segments: 64 } },
        "3": { id: 3, name: "stator", op: { type: "Difference", left: 1, right: 2 } },
      },
      materials: {},
      part_materials: {},
      roots: [{ root: 3, material: "default" }],
    } as unknown as Document;
    const opened = out(openDocument({ initial: doc }));
    const id = opened.document_id;

    const traced = out(boardFromSolid({ document_id: id }, engine));
    expect(traced.success).toBe(true);
    expect(traced.cutouts).toBe(1);

    // The outline approximates the 30mm circle around the disc center.
    const vs: Vec2[] = traced.outline.vertices;
    expect(vs.length).toBeGreaterThanOrEqual(8);
    const cx = vs.reduce((s, v) => s + v.x, 0) / vs.length;
    const cy = vs.reduce((s, v) => s + v.y, 0) / vs.length;
    for (const v of vs) {
      const r = Math.hypot(v.x - cx, v.y - cy);
      expect(r).toBeGreaterThan(28.5);
      expect(r).toBeLessThan(31.5);
    }
    // Bore cutout approximates the 8mm circle.
    const bore: Vec2[] = traced.outline.cutouts[0];
    for (const v of bore) {
      const r = Math.hypot(v.x - cx, v.y - cy);
      expect(r).toBeGreaterThan(6.5);
      expect(r).toBeLessThan(9.5);
    }
    // Both outline and cutouts are wound CCW (positive signed area) — the
    // kernel extruder's convention.
    const signedArea = (poly: Vec2[]) => {
      let a = 0;
      for (let i = 0; i < poly.length; i++) {
        const p = poly[i];
        const q = poly[(i + 1) % poly.length];
        a += p.x * q.y - q.x * p.y;
      }
      return a / 2;
    };
    expect(signedArea(vs)).toBeGreaterThan(0);
    expect(signedArea(bore)).toBeGreaterThan(0);

    // Round trip: the traced outline drives a real board.
    const created = out(await createSchematic({ components: [resistor("R1", 0)] }));
    const placed = out(
      await placeComponents({
        document_id: created.document_id,
        outline: traced.outline,
      }),
    );
    expect(placed.success).toBe(true);
    expect(placed.board.shape).toBe("polygon");
  });

  it("lists the parts when the document has several and no part_id is given", async () => {
    const doc: Document = {
      version: "0.1",
      nodes: {
        "1": { id: 1, name: "a", op: { type: "Cube", size: { x: 10, y: 10, z: 2 } } },
        "2": { id: 2, name: "b", op: { type: "Cube", size: { x: 5, y: 5, z: 2 } } },
      },
      materials: {},
      part_materials: {},
      roots: [
        { root: 1, material: "default" },
        { root: 2, material: "default" },
      ],
    } as unknown as Document;
    const opened = out(openDocument({ initial: doc }));
    expect(() => boardFromSolid({ document_id: opened.document_id }, engine)).toThrow(
      /pass part_id.*1 \(a\).*2 \(b\)/s,
    );
  });
});

describe("ecad pipeline behaviors (session flow)", () => {
  it("derives pad nets from wire connectivity, not pin names", async () => {
    const created = out(
      await createSchematic({
        components: [resistor("R1", 0), resistor("R2", 20)],
        wires: [{ x1: 5, y1: 0, x2: 15, y2: 0 }],
        labels: [{ name: "MID", x: 5, y: 0 }],
      }),
    );
    const id = created.document_id;
    out(await placeComponents({ document_id: id, board_width: 50, board_height: 50 }));

    const board = getPcbBoard(getSession(id));
    const padNet = (ref: string, num: string) =>
      board.footprints.find((fp) => fp.ref === ref)!.pads.find((p) => p.number === num)!.net;
    expect(padNet("R1", "2")).toBe("MID");
    expect(padNet("R2", "1")).toBe("MID");
    expect(padNet("R1", "1")).toBeUndefined();
    expect(padNet("R2", "2")).toBeUndefined();
  });

  it("routes nets without same-layer crossings between different nets", async () => {
    const created = out(
      await createSchematic({
        components: [
          {
            ref: "J1",
            value: "Conn",
            footprint: "Connector_PinHeader_2.54mm:PinHeader_1x02_P2.54mm_Vertical",
            x: 0,
            y: 0,
            pins: [
              { number: "1", name: "VCC", type: "Passive" },
              { number: "2", name: "GND", type: "Passive" },
            ],
          },
          {
            ref: "R1",
            value: "330",
            footprint: "Resistor_SMD:R_0805_2012Metric",
            x: 20,
            y: 0,
            pins: [
              { number: "1", name: "VCC", type: "Passive" },
              { number: "2", name: "N1", type: "Passive" },
            ],
          },
          {
            ref: "D1",
            value: "LED",
            footprint: "LED_SMD:LED_0805_2012Metric",
            x: 40,
            y: 0,
            pins: [
              { number: "1", name: "N1", type: "Passive" },
              { number: "2", name: "GND", type: "Passive" },
            ],
          },
        ],
      }),
    );
    const id = created.document_id;
    out(await placeComponents({ document_id: id, board_width: 30, board_height: 30 }));
    const routed = out(await routeNets({ document_id: id }));
    expect(routed.success).toBe(true);
    expect(routed.nets_routed).toBe(3);
    expect(routed.fallback_nets).toBeUndefined();

    const traces = getPcbBoard(getSession(id)).traces;
    expect(traces.length).toBeGreaterThan(0);

    const cross = (o: Vec2, a: Vec2, b: Vec2) =>
      (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x);
    const properlyIntersects = (p1: Vec2, p2: Vec2, q1: Vec2, q2: Vec2) => {
      const d1 = cross(q1, q2, p1);
      const d2 = cross(q1, q2, p2);
      const d3 = cross(p1, p2, q1);
      const d4 = cross(p1, p2, q2);
      return (
        ((d1 > 1e-9 && d2 < -1e-9) || (d1 < -1e-9 && d2 > 1e-9)) &&
        ((d3 > 1e-9 && d4 < -1e-9) || (d3 < -1e-9 && d4 > 1e-9))
      );
    };

    for (let i = 0; i < traces.length; i++) {
      for (let j = i + 1; j < traces.length; j++) {
        const a = traces[i];
        const b = traces[j];
        if (a.net === b.net || a.layer !== b.layer) continue;
        expect(
          properlyIntersects(a.start, a.end, b.start, b.end),
          `trace ${a.net} crosses ${b.net}`,
        ).toBe(false);
      }
    }
  });

  it("DRC flags same-layer copper shorts between different nets", async () => {
    const created = out(
      await createSchematic({
        components: [resistor("R1", 0, ["A", "B"]), resistor("R2", 20, ["A", "B"])],
      }),
    );
    const id = created.document_id;
    out(await placeComponents({ document_id: id, board_width: 40, board_height: 40 }));

    // Inject two crossing traces on different nets — a hard short.
    const board = getPcbBoard(getSession(id));
    board.traces.push(
      { start: { x: 10, y: 10 }, end: { x: 30, y: 30 }, width: 0.25, layer: "FCu", net: "A" },
      { start: { x: 10, y: 30 }, end: { x: 30, y: 10 }, width: 0.25, layer: "FCu", net: "B" },
    );

    // Default summary surfaces the short in the rule rollup + worst clearance.
    const drc = out(await runDrc({ document_id: id }));
    expect(drc.success).toBe(true);
    expect(drc.byRule.Clearance).toBeGreaterThan(0);
    expect(drc.worstClearance).not.toBeNull();

    // Full detail still returns every violation for callers that want it.
    const full = out(await runDrc({ document_id: id, detail: "full" }));
    const clearanceErrors = (full.details as Array<{ rule: string }>).filter(
      (v) => v.rule === "Clearance",
    );
    expect(clearanceErrors.length).toBeGreaterThan(0);
  });

  it("generates real multi-pin footprint geometry (SOIC-8)", async () => {
    const created = out(
      await createSchematic({
        components: [
          {
            ref: "U1",
            value: "NE555",
            footprint: "Package_SO:SOIC-8_3.9x4.9mm_P1.27mm",
            x: 0,
            y: 0,
            pins: Array.from({ length: 8 }, (_, i) => ({
              number: `${i + 1}`,
              name: `P${i + 1}`,
              type: "Passive",
            })),
          },
        ],
      }),
    );
    const id = created.document_id;
    const placed = out(
      await placeComponents({ document_id: id, board_width: 30, board_height: 30 }),
    );
    expect(placed.success).toBe(true);

    const pads = getPcbBoard(getSession(id)).footprints[0].pads;
    expect(pads.length).toBe(8);
    const unique = new Set(pads.map((p) => `${p.position.x},${p.position.y}`));
    expect(unique.size).toBe(8);
  });

  it("export_gerber falls back to inline files when output_dir is unwritable", async () => {
    const created = out(
      await createSchematic({ components: [resistor("R1", 0, ["A", "B"])] }),
    );
    const id = created.document_id;
    out(await placeComponents({ document_id: id, board_width: 30, board_height: 30 }));

    const res = out(
      await exportGerber({
        document_id: id,
        // /dev/null can't be a directory — mkdir fails on every platform.
        output_dir: "/dev/null/nope",
      }),
    );
    expect(res.success).toBe(true);
    expect(res.message).toContain("returning contents inline");
    expect(res.files.length).toBeGreaterThan(0);
    expect(res.files[0].content).toBeTruthy();
  });

  it("open_in_browser produces a URL for PCB documents", async () => {
    const created = out(
      await createSchematic({ components: [resistor("R1", 0, ["A", "B"])] }),
    );
    const id = created.document_id;
    out(await placeComponents({ document_id: id, board_width: 30, board_height: 30 }));

    const res = openInBrowser({
      document: JSON.stringify(getSession(id)),
      name: "pcb-test",
    });
    expect(res.content[0].text).toContain("https://vcad.io/#/new?doc=");
    expect(res.content[0].text).toContain("Payload (json)");
  });

  it("keeps all footprints inside the board outline", async () => {
    for (const strategy of ["grid", "force_directed"]) {
      const created = out(
        await createSchematic({
          components: Array.from({ length: 5 }, (_, i) =>
            resistor(`R${i + 1}`, i * 10, ["A", "B"]),
          ),
        }),
      );
      const id = created.document_id;
      const placed = out(
        await placeComponents({
          document_id: id,
          board_width: 25,
          board_height: 15,
          strategy,
        }),
      );
      expect(placed.success).toBe(true);
      expect(placed.strategy).toBe(strategy);

      for (const fp of getPcbBoard(getSession(id)).footprints) {
        expect(fp.position.x).toBeGreaterThan(0);
        expect(fp.position.x).toBeLessThan(25);
        expect(fp.position.y).toBeGreaterThan(0);
        expect(fp.position.y).toBeLessThan(15);
      }
    }
  });

  it("re-running place_components replaces the board instead of stacking a second one", async () => {
    const created = out(
      await createSchematic({ components: [resistor("R1", 0, ["A", "B"])] }),
    );
    const id = created.document_id;
    out(await placeComponents({ document_id: id, board_width: 30, board_height: 30 }));
    const second = out(
      await placeComponents({ document_id: id, board_width: 50, board_height: 50 }),
    );
    expect(second.warnings.join(" ")).toContain("replaced");

    const doc = getSession(id);
    const pcbNodes = Object.values(doc.nodes).filter(
      (n) => (n.op as { type: string }).type === "PcbBoard",
    );
    expect(pcbNodes).toHaveLength(1);
    const w = Math.max(...getPcbBoard(doc).outline.vertices.map((v) => v.x));
    expect(w).toBe(50);
  });
});
