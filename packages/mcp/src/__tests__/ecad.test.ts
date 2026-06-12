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
  addCoil,
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

    const drc = out(await runDrc({ document_id: id }));
    expect(drc.success).toBe(true);
    const clearanceErrors = (drc.details as Array<{ rule: string }>).filter(
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
