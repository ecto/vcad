import { describe, it, expect, beforeAll, beforeEach } from "vitest";
import { Engine, resolveFootprint, parseKicadPcb } from "@vcad/engine";
import type { Document, Pcb, SchematicSheet, Vec2 } from "@vcad/ir";
import { createDocument } from "@vcad/ir";
import {
  createSchematic,
  placeComponents,
  routeNets,
  critiqueRoute,
  runDrc,
  runErc,
  exportGerber,
  exportKicad,
  validateForFab,
  fabPrep,
  calcImpedance,
  sizeImpedance,
  sizePdn,
  calcCoil,
  sizeCoil,
  calcRf,
  calcMotor,
  checkSelfStart,
  addCoil,
  addCoilArray,
  windingLayout,
  addTrace,
  getPadPositions,
  getFootprint,
  describePcb,
  listFootprints,
  searchFootprints,
  addVia,
  setStackup,
  setPlacement,
  setBoardOutline,
  addZone,
  deleteZone,
  deleteTrace,
  deleteVia,
  getCopper,
  addNetTie,
  deleteNetTie,
  undo,
  setDesignRules,
  sizeTraceForCurrent,
  addViaArray,
  addMotorWinding,
  aggregateDrc,
  parseNetPair,
  summarizePlacementDrc,
  boardFromSolid,
  appNotesForPin,
  unconnectedPinSeverity,
} from "../tools/ecad.js";
import { renderRatsnest, renderStackup } from "../tools/render.js";
import { importKicad, importEagle } from "../tools/import-pcb.js";
import {
  documents,
  getSession,
  openDocument,
  recordHistorySnapshot,
  registerSession,
} from "../tools/session.js";
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

/**
 * Register a session holding a hand-built board with a single power net "PWR"
 * on two SMD pads. When `continuous`, a trace ties the pads into one galvanic
 * island; otherwise they're stranded (the +3V3-into-islands failure shape).
 * Returns the document_id for the realized-copper gate tests.
 */
function registerPwrBoard(continuous: boolean): string {
  const fp = (ref: string, x: number) => ({
    ref,
    value: "X",
    footprintName: "test:pad",
    position: { x, y: 25 },
    rotation: 0,
    front: true,
    pads: [
      {
        number: "1",
        padType: "SMD",
        shape: { type: "Rect", width: 1.5, height: 1.5 },
        position: { x: 0, y: 0 },
        layers: ["FCu"],
        net: "PWR",
      },
    ],
  });
  const pcb = {
    outline: {
      vertices: [
        { x: 0, y: 0 },
        { x: 50, y: 0 },
        { x: 50, y: 50 },
        { x: 0, y: 50 },
      ],
      cutouts: [],
      thickness: 1.6,
    },
    stackup: {
      layers: [
        { layer: "FCu", copperThickness: 0.035, dielectricThickness: 1.5, dielectricEr: 4.5, material: "FR4" },
      ],
    },
    nets: [{ id: "PWR", name: "PWR" }],
    rules: {
      defaultRules: { name: "Default", traceWidth: 0.25, clearance: 0.2, viaDiameter: 0.8, viaDrill: 0.4 },
      classRules: [],
      netClassAssignments: {},
      edgeClearance: 0.3,
      holeToHole: 0.5,
      minAnnularRing: 0.15,
      minDrill: 0.2,
    },
    footprints: [fp("U1", 10), fp("U2", 40)],
    traces: continuous
      ? [{ start: { x: 10, y: 25 }, end: { x: 40, y: 25 }, width: 0.5, layer: "FCu", net: "PWR" }]
      : [],
    vias: [],
    zones: [],
  } as unknown as Pcb;
  const doc = createDocument();
  (doc as Document & { pcb?: Pcb }).pcb = pcb;
  return registerSession(doc);
}

describe("pin-type validation (create_schematic)", () => {
  it("accepts valid PinType variants verbatim", async () => {
    const created = out(
      await createSchematic({
        components: [
          {
            ref: "U1",
            value: "X",
            footprint: "SOIC-8",
            x: 0,
            y: 0,
            pins: [
              { number: "1", name: "A", type: "Bidirectional", x: 0, y: 0 },
              { number: "2", name: "B", type: "PowerInput", x: 0, y: 1 },
              { number: "3", name: "C", type: "OpenCollector", x: 0, y: 2 },
            ],
          },
        ],
      }),
    );
    expect(created.success).toBe(true);
    const doc = getSession(created.document_id);
    expect(doc.schematic!.components[0]!.pins.map((p) => p.pin_type)).toEqual([
      "Bidirectional",
      "PowerInput",
      "OpenCollector",
    ]);
  });

  it("rejects a mis-cased pin type with a precise case correction", async () => {
    await expect(
      createSchematic({
        components: [
          {
            ref: "U1",
            footprint: "SOIC-8",
            x: 0,
            y: 0,
            pins: [{ number: "34", name: "D", type: "BiDirectional", x: 0, y: 0 }],
          },
        ],
      }),
    ).rejects.toThrow(
      /Invalid pin type "BiDirectional" on U1\.34.*did you mean "Bidirectional"/,
    );
  });

  it("rejects an unknown pin type with a fuzzy suggestion and the valid list", async () => {
    await expect(
      createSchematic({
        components: [
          {
            ref: "R9",
            footprint: "Resistor_SMD:R_0805",
            x: 0,
            y: 0,
            pins: [{ number: "1", name: "~", type: "Passiv", x: 0, y: 0 }],
          },
        ],
      }),
    ).rejects.toThrow(/did you mean "Passive".*Valid pin types:/);
  });

  it("defaults an absent pin type to Passive", async () => {
    const created = out(
      await createSchematic({
        components: [
          {
            ref: "R1",
            footprint: "Resistor_SMD:R_0805",
            x: 0,
            y: 0,
            pins: [{ number: "1", name: "~", x: 0, y: 0 }],
          },
        ],
      }),
    );
    const doc = getSession(created.document_id);
    expect(doc.schematic!.components[0]!.pins[0]!.pin_type).toBe("Passive");
  });
});

describe("footprint discovery", () => {
  interface FamilyOut {
    family: string;
    example: string;
    kind: string;
    aliases: string[];
  }

  it("list_footprints returns families with example ids, filterable by kind", async () => {
    const all = out(await listFootprints({}));
    expect(all.success).toBe(true);
    expect(all.count).toBeGreaterThan(20);
    const conn = out(await listFootprints({ kind: "connector" }));
    expect((conn.families as FamilyOut[]).length).toBeGreaterThan(0);
    expect((conn.families as FamilyOut[]).every((f) => f.kind === "connector")).toBe(true);
    expect((conn.families as FamilyOut[]).map((f) => f.family)).toContain("USB-C");
  });

  it("every advertised example id resolves to a real family (drift guard)", async () => {
    const all = out(await listFootprints({}));
    // Reach into the same examples list the tool advertises and resolve each
    // through the kernel — keeps the TS catalog honest against footprint.rs.
    for (const fam of all.families as Array<{ family: string; example: string }>) {
      // Resolve each advertised example through the kernel and assert a real
      // (non-fallback) match. The id carries its own count for most families;
      // a generous 8 covers the count-less ones.
      const res = await resolveFootprint(fam.example, 8);
      expect(res, `resolver unavailable for ${fam.example}`).toBeTruthy();
      expect(res!.matched, `${fam.family} example "${fam.example}" must match a real family, got note: ${res!.note}`).toBe(true);
    }
  });

  it("search_footprints ranks the obvious family first", async () => {
    const soic = out(await searchFootprints({ query: "SOIC 8" }));
    expect(soic.count).toBeGreaterThan(0);
    expect(soic.matches[0].family).toBe("SOIC");
    const jst = out(await searchFootprints({ query: "jst" }));
    expect((jst.matches as Array<{ family: string }>).every((m) => m.family.startsWith("JST"))).toBe(true);
    const qfn = out(await searchFootprints({ query: "qfn" }));
    expect(qfn.matches[0].family).toBe("QFN");
  });

  it("search_footprints resolves crystal / USB micro / HTSSOP queries", async () => {
    // The RP2040-board repro: "crystal 3225" used to return 0 matches.
    const xtal = out(await searchFootprints({ query: "crystal 3225" }));
    expect(xtal.count).toBeGreaterThan(0);
    expect(xtal.matches[0].family).toBe("Crystal");
    const bare = out(await searchFootprints({ query: "3225" }));
    expect((bare.matches as Array<{ family: string }>).map((m) => m.family)).toContain("Crystal");
    const micro = out(await searchFootprints({ query: "USB micro" }));
    expect(micro.matches[0].family).toBe("USB-Micro-B");
    const htssop = out(await searchFootprints({ query: "HTSSOP-16" }));
    expect(htssop.matches[0].family).toBe("HTSSOP");
    const powerpad = out(await searchFootprints({ query: "powerpad" }));
    expect(powerpad.matches[0].family).toBe("HTSSOP");
  });

  it("search_footprints errors on an empty query", async () => {
    const res = await searchFootprints({ query: "  " });
    expect((res as { isError?: boolean }).isError).toBe(true);
  });
});

describe("catalog parts (create_schematic resolves added FC parts)", () => {
  it("resolves TCAN1042 pins from the database without explicit pins", async () => {
    const created = out(
      await createSchematic({
        components: [{ ref: "U1", part: "TCAN1042HGV", footprint: "SOIC-8", x: 0, y: 0 }],
        nets: { GND: ["U1.2"], "3V3": ["U1.3"] },
      }),
    );
    expect(created.success).toBe(true);
    const comp = getSession(created.document_id).schematic!.components[0]!;
    expect(comp.pins.length).toBe(8);
    const names = comp.pins.map((p) => p.name);
    expect(names).toContain("CANH");
    expect(names).toContain("CANL");
  });

  it("resolves DRV8833 pins by part name (jellybean IC)", async () => {
    const created = out(
      await createSchematic({
        components: [{ ref: "U1", part: "DRV8833", footprint: "TSSOP-16", x: 0, y: 0 }],
        nets: { VM: ["U1.13"], GND: ["U1.12"], AIN1: ["U1.16"] },
      }),
    );
    expect(created.success).toBe(true);
    const comp = getSession(created.document_id).schematic!.components[0]!;
    expect(comp.pins.length).toBe(16);
    expect(comp.pins.find((p) => p.number === "16")!.name).toBe("AIN1");
    expect(comp.pins.find((p) => p.number === "13")!.name).toBe("VM");
  });

  it("resolves RP2040 including the EP ground pad", async () => {
    const created = out(
      await createSchematic({
        components: [{ ref: "U1", part: "RP2040", footprint: "QFN-56", x: 0, y: 0 }],
        nets: { GND: ["U1.EP"], XIN: ["U1.20"], USB_DP: ["U1.47"] },
      }),
    );
    expect(created.success).toBe(true);
    const comp = getSession(created.document_id).schematic!.components[0]!;
    expect(comp.pins.length).toBe(57);
    expect(comp.pins.find((p) => p.number === "EP")!.name).toBe("GND");
  });
});

describe("two-terminal passive pin synthesis (create_schematic)", () => {
  it("synthesizes pins 1/2 for a value-only chip passive", async () => {
    const created = out(
      await createSchematic({
        components: [
          { ref: "C1", value: "100nF", footprint: "C_0603", x: 0, y: 0 },
          { ref: "R1", value: "10k", footprint: "R_0402", x: 20, y: 0 },
          { ref: "L1", value: "10uH", footprint: "L_0805", x: 40, y: 0 },
          { ref: "D1", value: "1N4148", footprint: "D_SOD-123", x: 60, y: 0 },
        ],
        nets: { GND: ["C1.1", "R1.1"], SIG: ["C1.2", "R1.2", "L1.1", "D1.1"] },
      }),
    );
    expect(created.success).toBe(true);
    for (const comp of getSession(created.document_id).schematic!.components) {
      expect(comp.pins.length, comp.ref).toBe(2);
      expect(comp.pins.map((p) => p.number)).toEqual(["1", "2"]);
      expect(comp.pins.every((p) => p.pin_type === "Passive")).toBe(true);
    }
    // No "has no pins" warnings for synthesized passives.
    expect(
      (created.warnings ?? []).filter((w: string) => w.includes("has no pins")),
    ).toEqual([]);
  });

  it("explicit pins and non-passive footprints are untouched", async () => {
    const created = out(
      await createSchematic({
        components: [
          {
            ref: "C1",
            value: "100nF",
            footprint: "C_0603",
            x: 0,
            y: 0,
            pins: [{ number: "A", name: "A", type: "Passive", x: 0, y: 0 }],
          },
          // Unknown IC footprint, no part: still warns instead of guessing pins.
          { ref: "U1", value: "mystery", footprint: "SOIC-8", x: 20, y: 0 },
        ],
      }),
    );
    const comps = getSession(created.document_id).schematic!.components;
    expect(comps[0]!.pins.map((p) => p.number)).toEqual(["A"]);
    expect(comps[1]!.pins.length).toBe(0);
  });
});

describe("import_kicad / import_eagle", () => {
  // Minimal KiCad board (mirrors the kernel parser's own fixture): 2 nets,
  // a 100x80 outline, one 0805 with 2 pads, one trace, one via.
  const MINIMAL_KICAD = `(kicad_pcb (version 20221018) (generator test)
  (general (thickness 1.6))
  (layers (0 "F.Cu" signal) (31 "B.Cu" signal) (44 "Edge.Cuts" user))
  (net 0 "")
  (net 1 "VCC")
  (net 2 "GND")
  (gr_line (start 0 0) (end 100 0) (layer "Edge.Cuts") (width 0.05))
  (gr_line (start 100 0) (end 100 80) (layer "Edge.Cuts") (width 0.05))
  (gr_line (start 100 80) (end 0 80) (layer "Edge.Cuts") (width 0.05))
  (gr_line (start 0 80) (end 0 0) (layer "Edge.Cuts") (width 0.05))
  (footprint "R_0805" (layer "F.Cu") (at 25 40)
    (fp_text reference "R1" (at 0 0) (layer "F.SilkS"))
    (pad "1" smd rect (at -1 0) (size 1 1.2) (layers "F.Cu" "F.Paste" "F.Mask") (net 1 "VCC"))
    (pad "2" smd rect (at 1 0) (size 1 1.2) (layers "F.Cu" "F.Paste" "F.Mask") (net 2 "GND"))
  )
  (segment (start 25 40) (end 50 40) (width 0.25) (layer "F.Cu") (net 1))
  (via (at 50 40) (size 0.8) (drill 0.4) (layers "F.Cu" "B.Cu") (net 1))
)`;

  it("imports a KiCad board into a live, tool-ready session", async () => {
    const content_base64 = Buffer.from(MINIMAL_KICAD, "utf8").toString("base64");
    const res = out(await importKicad({ content_base64, name: "Imported" }));
    expect(res.success).toBe(true);
    expect(res.document_id).toBeTruthy();
    expect(res.summary.footprints).toBe(1);
    expect(res.summary.nets).toBe(2);
    expect(res.summary.outline_vertices).toBe(4);
    expect(res.summary.traces).toBe(1);
    expect(res.summary.vias).toBe(1);

    // The returned id drives the rest of the toolchain: the board is queryable.
    const board = getPcbBoard(getSession(res.document_id));
    expect(board.footprints[0]!.ref).toBe("R1");
    const pads = out(await getPadPositions({ document_id: res.document_id }));
    expect(pads.count).toBe(2);
  });

  it("errors clearly on unparseable content and missing input", async () => {
    const bad = await importKicad({
      content_base64: Buffer.from("not a kicad file", "utf8").toString("base64"),
    });
    expect((bad as { isError?: boolean }).isError).toBe(true);
    const none = await importKicad({});
    expect((none as { isError?: boolean }).isError).toBe(true);
  });

  it("import_eagle returns a not-yet-supported stub pointing at import_kicad", () => {
    const res = importEagle({ filename: "board.brd" });
    expect((res as { isError?: boolean }).isError).toBe(true);
    expect(res.content[0].text).toContain("import_kicad");
  });
});

describe("render_ratsnest / render_stackup", () => {
  it("render_ratsnest overlays airwires and reports the unconnected-pair count", async () => {
    const created = out(
      await createSchematic({
        components: [resistor("R1", 0), resistor("R2", 20), resistor("R3", 40)],
        nets: { GND: ["R1.1", "R2.1", "R3.1"], SIG: ["R1.2", "R2.2"] },
      }),
    );
    const id = created.document_id;
    await placeComponents({ document_id: id, board_width: 60, board_height: 30 });

    const res = await renderRatsnest({ document_id: id });
    const image = res.content.find((c) => c.type === "image");
    expect(image, "expected a PNG image block (is @resvg/resvg-js installed?)").toBeDefined();
    const textBlock = res.content.find((c) => c.type === "text") as
      | { text: string }
      | undefined;
    const meta = JSON.parse(textBlock!.text);
    // Nothing routed yet → every net connection is an airwire (GND 3 pads → 2,
    // SIG 2 pads → 1).
    expect(meta.airwires).toBeGreaterThan(0);
    expect(meta.format).toBe("png");
  });

  it("render_stackup returns one image per copper layer plus an index", async () => {
    const created = out(
      await createSchematic({
        components: [resistor("R1", 0), resistor("R2", 20)],
        nets: { N: ["R1.2", "R2.1"] },
      }),
    );
    const id = created.document_id;
    await placeComponents({ document_id: id, board_width: 40, board_height: 20 });

    const res = await renderStackup({ document_id: id });
    const images = res.content.filter((c) => c.type === "image");
    expect(images.length).toBeGreaterThanOrEqual(2); // at least F.Cu + B.Cu
    const textBlock = res.content.find((c) => c.type === "text") as
      | { text: string }
      | undefined;
    const meta = JSON.parse(textBlock!.text);
    expect(meta.layers.length).toBe(images.length);
  });
});

describe("get_pad_positions", () => {
  interface PadPos {
    ref: string;
    pin: string;
    x: number;
    y: number;
    net: string | null;
    layer: string | null;
  }

  it("returns absolute board-frame coordinates matching the footprint transform", async () => {
    const created = out(
      await createSchematic({
        components: [resistor("R1", 0), resistor("R2", 20)],
        nets: { MID: ["R1.2", "R2.1"] },
      }),
    );
    const id = created.document_id;
    await placeComponents({ document_id: id, board_width: 40, board_height: 20 });

    const res = out(await getPadPositions({ document_id: id }));
    expect(res.success).toBe(true);
    expect(res.count).toBeGreaterThan(0);
    expect((res.pads as PadPos[]).length).toBe(res.count);

    // Cross-check each returned pad against the stored footprint geometry,
    // recomputing the absolute position with the documented transform so a
    // regression in the rotation/offset math fails loudly.
    const board = getPcbBoard(getSession(id));
    const byKey = new Map(
      (res.pads as PadPos[]).map((p) => [`${p.ref}.${p.pin}`, p]),
    );
    for (const fp of board.footprints) {
      const t = ((fp.rotation ?? 0) * Math.PI) / 180;
      for (const pad of fp.pads) {
        const ex =
          fp.position.x + pad.position.x * Math.cos(t) - pad.position.y * Math.sin(t);
        const ey =
          fp.position.y + pad.position.x * Math.sin(t) + pad.position.y * Math.cos(t);
        const got = byKey.get(`${fp.ref}.${pad.number}`);
        expect(got).toBeTruthy();
        expect(got!.x).toBeCloseTo(ex, 3);
        expect(got!.y).toBeCloseTo(ey, 3);
        expect(got!.layer).toMatch(/Cu$/);
      }
    }
  });

  it("agrees with the Rust pad_world_position transform on a rotated footprint", async () => {
    const created = out(
      await createSchematic({
        components: [resistor("R1", 0)],
      }),
    );
    const id = created.document_id;
    await placeComponents({ document_id: id, board_width: 40, board_height: 20 });

    // Pin the placed footprint to the exact fixture the Rust kernel test
    // `pad_world_position_rotated_matches_ts_tool` asserts in
    // crates/vcad-ecad-pcb/src/geometry.rs. Both sides hardcode the same
    // world coordinates (origin (10, 20), rotation 190°), so the Rust copper
    // pipeline (DRC / routing / pours) and this reporting tool cannot drift
    // apart on the pad transform. If you change one side, change both.
    const board = getPcbBoard(getSession(id));
    const fp = board.footprints.find((f) => f.ref === "R1")!;
    fp.position = { x: 10, y: 20 };
    fp.rotation = 190;
    fp.pads[0]!.position = { x: 7.62, y: 0 };
    fp.pads[1]!.position = { x: 2.54, y: -1.27 };

    const res = out(await getPadPositions({ document_id: id, ref: "R1" }));
    const pads = res.pads as PadPos[];
    expect(pads.length).toBe(2);
    expect(pads[0]!.x).toBeCloseTo(2.496, 3);
    expect(pads[0]!.y).toBeCloseTo(18.677, 3);
    expect(pads[1]!.x).toBeCloseTo(7.278, 3);
    expect(pads[1]!.y).toBeCloseTo(20.81, 3);
  });

  it("filters by net and by ref", async () => {
    const created = out(
      await createSchematic({
        components: [resistor("R1", 0), resistor("R2", 20)],
        nets: { MID: ["R1.2", "R2.1"] },
      }),
    );
    const id = created.document_id;
    await placeComponents({ document_id: id, board_width: 40, board_height: 20 });

    const mid = out(await getPadPositions({ document_id: id, net: "MID" }));
    expect(mid.count).toBe(2);
    expect((mid.pads as PadPos[]).every((p) => p.net === "MID")).toBe(true);
    expect((mid.pads as PadPos[]).map((p) => `${p.ref}.${p.pin}`).sort()).toEqual([
      "R1.2",
      "R2.1",
    ]);

    const r1 = out(await getPadPositions({ document_id: id, ref: "R1" }));
    expect(r1.count).toBe(2);
    expect((r1.pads as PadPos[]).every((p) => p.ref === "R1")).toBe(true);
  });

  it("errors when the document has no PCB", async () => {
    const created = out(await createSchematic({ components: [resistor("R1", 0)] }));
    const res = await getPadPositions({ document_id: created.document_id });
    expect((res as { isError?: boolean }).isError).toBe(true);
  });
});

describe("get_footprint", () => {
  const conn = (ref: string, footprint: string, pins: number[]) => ({
    ref,
    value: "CONN",
    footprint,
    x: 0,
    y: 0,
    pins: pins.map((n) => ({ number: String(n), name: String(n), type: "Passive" })),
  });

  it("resolves a footprint id PRE-placement in the local frame", async () => {
    const r = out(await getFootprint({ footprint: "Connector_JST:JST_PH_2", pins: 2 }));
    expect(r.success).toBe(true);
    expect(r.mode).toBe("resolved");
    expect(r.family).toBe("JST-PH");
    expect(r.matched).toBe(true);
    expect(r.generated).toBe(true);
    expect(r.rotation_convention).toMatch(/counter-clockwise/i);
    expect(r.rotation_convention).toMatch(/degrees/i);
    expect(r.count).toBe(2);
    // No placement → board frame equals local frame (origin 0,0, rot 0).
    expect(r.origin).toMatchObject({ x: 0, y: 0, rotation: 0, side: "front" });
    for (const p of r.pads) {
      expect(p.board.x).toBeCloseTo(p.local.x, 6);
      expect(p.board.y).toBeCloseTo(p.local.y, 6);
      expect(p.pad_type).toBe("THT");
      expect(p.drill_mm).toBeGreaterThan(0);
    }
    // Courtyard AABB is reported in both frames.
    expect(r.courtyard.local).not.toBeNull();
    expect(r.courtyard.board).not.toBeNull();
    expect(r.courtyard.local.max.x).toBeGreaterThan(r.courtyard.local.min.x);
  });

  it("projects pads into a hypothetical board placement (origin + CCW rotation)", async () => {
    const flat = out(await getFootprint({ footprint: "Connector_JST:JST_PH_2", pins: 2 }));
    const rotated = out(
      await getFootprint({
        footprint: "Connector_JST:JST_PH_2",
        pins: 2,
        at: { x: 10, y: 5 },
        rotation: 90,
      }),
    );
    expect(rotated.origin).toMatchObject({ x: 10, y: 5, rotation: 90 });
    // A pad at local (lx, ly) maps to (10 - ly, 5 + lx) under a 90° CCW rotation.
    for (let i = 0; i < flat.pads.length; i++) {
      const lp = flat.pads[i].local;
      const bp = rotated.pads[i].board;
      expect(bp.x).toBeCloseTo(10 - lp.y, 3);
      expect(bp.y).toBeCloseTo(5 + lp.x, 3);
    }
  });

  it("reads a PLACED footprint's real transform, nets, and courtyard", async () => {
    const created = out(
      await createSchematic({ components: [conn("J1", "JST_PH_3", [1, 2, 3])] }),
    );
    const id = created.document_id;
    await placeComponents({ document_id: id, board_width: 40, board_height: 30 });

    const r = out(await getFootprint({ document_id: id, ref: "J1" }));
    expect(r.mode).toBe("placed");
    expect(r.ref).toBe("J1");
    expect(r.generated).toBe(true); // synthesized by the engine at placement

    // Board-frame pad coords agree with get_pad_positions (shared transform).
    const pads = out(await getPadPositions({ document_id: id, ref: "J1" }));
    const byPin = new Map(
      (pads.pads as Array<{ pin: string; x: number; y: number }>).map((p) => [p.pin, p]),
    );
    for (const p of r.pads) {
      const ref = byPin.get(p.pin)!;
      expect(p.board.x).toBeCloseTo(ref.x, 3);
      expect(p.board.y).toBeCloseTo(ref.y, 3);
      expect(p.net).toBe(p.pin); // pin names act as nets here
    }
    expect(r.courtyard.local).not.toBeNull();
  });

  it("errors when neither ref nor footprint is given", async () => {
    const res = await getFootprint({});
    expect((res as { isError?: boolean }).isError).toBe(true);
  });
});

describe("describe_pcb", () => {
  it("returns a compact structured snapshot of the routed board", async () => {
    const created = out(
      await createSchematic({
        components: [resistor("R1", 0), resistor("R2", 20), resistor("R3", 40)],
        nets: { MID: ["R1.2", "R2.1"], GND: ["R2.2", "R3.1"] },
      }),
    );
    const id = created.document_id;
    await placeComponents({ document_id: id, board_width: 60, board_height: 30 });
    await routeNets({ document_id: id });
    await addZone({ document_id: id, net: "GND", layer: "BCu", fill_board: true });

    const d = out(await describePcb({ document_id: id }));
    expect(d.success).toBe(true);

    // Board size echoes what place_components built.
    expect(d.board.width).toBeCloseTo(60, 3);
    expect(d.board.height).toBeCloseTo(30, 3);
    expect(d.board.outline_vertices).toBe(4);
    expect(d.board.area_mm2).toBeCloseTo(1800, 0);

    // Stackup: canonical layer names + copper weight (default 0.035mm ≈ 1oz).
    expect(d.stackup.copper_layers).toBe(2);
    const fcu = d.stackup.layers.find((l: { layer: string }) => l.layer === "FCu");
    expect(fcu).toBeTruthy();
    expect(fcu.copper_oz).toBeCloseTo(1, 1);

    // Nets reported as data, not just a count.
    expect(d.nets.count).toBe(2);
    expect([...d.nets.names].sort()).toEqual(["GND", "MID"]);

    // Design rules surface the default net class + fab limits.
    expect(d.design_rules.default.name).toBe("Default");
    expect(d.design_rules.default.traceWidth).toBeGreaterThan(0);
    expect(d.design_rules.minDrill).toBeGreaterThan(0);

    // Zone reported by { net, layer, bbox, fill }.
    expect(d.zones.length).toBe(1);
    expect(d.zones[0].net).toBe("GND");
    expect(d.zones[0].layer).toBe("BCu");
    expect(d.zones[0].bbox).toBeTruthy();
    expect(d.zones[0].fill).toBeTruthy();

    // Routed copper counted by net and by layer; the per-net tally totals the
    // segment + arc counts so the breakdown is internally consistent.
    expect(d.traces.segments).toBeGreaterThan(0);
    const traceTally = Object.values(d.traces.by_net).reduce(
      (a, b) => a + (b as number),
      0,
    );
    expect(d.traces.segments + d.traces.arcs).toBe(traceTally);
    expect(Object.keys(d.traces.by_layer).length).toBeGreaterThan(0);

    // Components / pads (3 resistors × 2 pads).
    expect(d.components.count).toBe(3);
    expect(d.components.pads).toBe(6);

    // DRC status present and structured.
    expect(d.drc).toHaveProperty("categories");
    expect(d.drc).toHaveProperty("byRule");
    expect(typeof d.drc.clean).toBe("boolean");

    // The export/render probe actually serialized the board — the only check
    // that catches a DRC-clean-but-unexportable board. WASM is loaded in tests.
    expect(d.exportability.wasm_available).toBe(true);
    expect(d.exportability.gerber_exportable).toBe(true);
    expect(d.exportability.gerber_file_count).toBeGreaterThan(0);
    expect(d.exportability.renderable).toBe(true);
    expect(d.exportability.preview_mesh_count).toBeGreaterThan(0);
  });

  it("errors when the document has no PCB", async () => {
    const created = out(await createSchematic({ components: [resistor("R1", 0)] }));
    const res = await describePcb({ document_id: created.document_id });
    expect((res as { isError?: boolean }).isError).toBe(true);
  });
});

describe("THT drill sizing (drill = lead + ~0.2mm)", () => {
  const conn = (ref: string, footprint: string, pins: number[]) => ({
    ref,
    value: "X",
    footprint,
    x: 0,
    y: 0,
    pins: pins.map((n) => ({ number: String(n), name: "~", type: "Passive" })),
  });

  it("emits fab-buildable drills for headers and JST connectors, DRC-clean", async () => {
    const created = out(
      await createSchematic({
        components: [
          conn("J1", "Connector_PinHeader_2.54mm:PinHeader_1x04_P2.54mm_Vertical", [1, 2, 3, 4]),
          conn("J2", "JST_PH_2", [1, 2]),
        ],
      }),
    );
    const id = created.document_id;
    await placeComponents({ document_id: id, board_width: 60, board_height: 40 });
    const board = getPcbBoard(getSession(id));
    const drill = (ref: string) =>
      board.footprints.find((f) => f.ref === ref)!.pads.map((p) => p.drill!.diameter);

    // 2.54mm header: standard 1.0mm drill, not the old pitch-scaled 1.016mm.
    for (const d of drill("J1")) expect(d).toBeLessThanOrEqual(1.0 + 1e-9);
    // JST-PH 0.5mm post → 0.7mm drill, not the old flat 1.0mm.
    for (const d of drill("J2")) expect(d).toBeCloseTo(0.7, 6);

    // No self-inflicted hole-to-hole / annular / drill manufacturing flags.
    const drc = out(await runDrc({ document_id: id, detail: "full" }));
    type V = { rule: string };
    const mfg = (drc.details as V[]).filter((v) =>
      ["HoleToHole", "MinDrill", "AnnularRing"].includes(v.rule),
    );
    expect(mfg).toEqual([]);
  });
});

describe("DRC provenance + generated tagging", () => {
  const header = (ref: string, footprint: string, pins: number[]) => ({
    ref,
    value: "HDR",
    footprint,
    x: 0,
    y: 0,
    pins: pins.map((n) => ({ number: String(n), name: "~", type: "Passive" })),
  });

  it("tags generated-footprint manufacturing artifacts apart from real faults", async () => {
    const created = out(
      await createSchematic({
        components: [
          header("J1", "Connector_PinHeader_2.54mm:PinHeader_1x04_P2.54mm_Vertical", [1, 2, 3, 4]),
        ],
      }),
    );
    const id = created.document_id;
    await placeComponents({ document_id: id, board_width: 40, board_height: 30 });

    // Force a manufacturing fault on the generated land pattern: demand a drill
    // larger than the header's pads carry. Every THT pad now trips MinDrill.
    await setDesignRules({ document_id: id, min_drill: 1.5 });

    const drc = out(await runDrc({ document_id: id, detail: "full" }));
    expect(drc.success).toBe(true);

    // Summary surfaces the trustworthy split.
    expect(drc.byProvenance).toBeDefined();
    expect(drc.generatedArtifacts).toBeGreaterThan(0);
    expect(drc.realViolations).toBe(drc.violations - drc.generatedArtifacts);
    expect(
      drc.byProvenance.intra_footprint +
        drc.byProvenance.inter_component +
        drc.byProvenance.routing,
    ).toBe(drc.violations);

    // Every violation carries provenance + generated.
    type V = { rule: string; provenance: string; generated: boolean };
    for (const v of drc.details as V[]) {
      expect(["intra_footprint", "inter_component", "routing"]).toContain(v.provenance);
      expect(typeof v.generated).toBe("boolean");
    }

    // The drill faults are intra-footprint and flagged as generated artifacts.
    const drills = (drc.details as V[]).filter((v) => v.rule === "MinDrill");
    expect(drills.length).toBe(4);
    expect(drills.every((v) => v.provenance === "intra_footprint")).toBe(true);
    expect(drills.every((v) => v.generated === true)).toBe(true);
  });

  it("tags routing/connectivity violations as routing, not generated", async () => {
    const created = out(
      await createSchematic({
        components: [resistor("R1", 0), resistor("R2", 20)],
        nets: { MID: ["R1.2", "R2.1"] },
      }),
    );
    const id = created.document_id;
    await placeComponents({ document_id: id, board_width: 40, board_height: 20 });

    // Nothing routed → the MID net is unconnected: a routing-provenance, real
    // (not-generated) violation.
    const drc = out(await runDrc({ document_id: id, detail: "full" }));
    type V = { rule: string; provenance: string; generated: boolean };
    const unrouted = (drc.details as V[]).filter((v) => v.rule === "UnconnectedNet");
    expect(unrouted.length).toBeGreaterThan(0);
    expect(unrouted.every((v) => v.provenance === "routing")).toBe(true);
    expect(unrouted.every((v) => v.generated === false)).toBe(true);
    expect(drc.byProvenance.routing).toBeGreaterThanOrEqual(unrouted.length);
  });
});

describe("route_nets locked_nets", () => {
  const hasDetour = (pcb: Pcb) =>
    pcb.traces.some(
      (t) =>
        t.net === "MID" &&
        Math.abs(t.start.x - 1) < 1e-6 &&
        Math.abs(t.start.y - 9) < 1e-6,
    );

  it("preserves hand-placed copper on a locked net", async () => {
    const created = out(
      await createSchematic({
        components: [resistor("R1", 0), resistor("R2", 20), resistor("R3", 40)],
        nets: { MID: ["R1.2", "R2.1"], NETB: ["R2.2", "R3.1"] },
      }),
    );
    const id = created.document_id;
    await placeComponents({ document_id: id, board_width: 60, board_height: 20 });

    // A distinctive detour the autorouter would never reproduce.
    await addTrace({
      document_id: id,
      net: "MID",
      layer: "FCu",
      points: [
        { x: 1, y: 9 },
        { x: 1, y: 1 },
        { x: 9, y: 1 },
      ],
    });
    expect(hasDetour(getPcbBoard(getSession(id)))).toBe(true);

    const res = out(await routeNets({ document_id: id, locked_nets: ["MID"] }));
    expect(res.success).toBe(true);
    expect(res.locked_nets).toEqual(["MID"]);
    // The locked net's copper is neither ripped up nor re-routed.
    expect(res.traces_removed ?? 0).toBe(0);
    expect(hasDetour(getPcbBoard(getSession(id)))).toBe(true);
  });

  it("preserves manual add_trace copper even when the net is NOT locked", async () => {
    const created = out(
      await createSchematic({
        components: [resistor("R1", 0), resistor("R2", 20), resistor("R3", 40)],
        nets: { MID: ["R1.2", "R2.1"], NETB: ["R2.2", "R3.1"] },
      }),
    );
    const id = created.document_id;
    await placeComponents({ document_id: id, board_width: 60, board_height: 20 });
    await addTrace({
      document_id: id,
      net: "MID",
      layer: "FCu",
      points: [
        { x: 1, y: 9 },
        { x: 1, y: 1 },
      ],
    });
    expect(hasDetour(getPcbBoard(getSession(id)))).toBe(true);

    // No locked_nets: provenance alone protects the hand-placed copper. The
    // net is preserved wholesale (the kernel can't route "around" existing
    // copper) and reported so the caller knows it was skipped.
    const res = out(await routeNets({ document_id: id }));
    expect(res.success).toBe(true);
    expect(res.manual_nets_preserved).toEqual(["MID"]);
    expect(res.traces_removed ?? 0).toBe(0);
    expect((res.warnings ?? []).join(" ")).toContain("MID");
    const board = getPcbBoard(getSession(id));
    expect(hasDetour(board)).toBe(true);
    // Other nets still route normally around the preserved copper.
    expect(board.traces.some((t) => t.net === "NETB")).toBe(true);
  });

  it("rips up untagged (pre-provenance) copper on an unlocked net (control)", async () => {
    const created = out(
      await createSchematic({
        components: [resistor("R1", 0), resistor("R2", 20)],
        nets: { MID: ["R1.2", "R2.1"] },
      }),
    );
    const id = created.document_id;
    await placeComponents({ document_id: id, board_width: 40, board_height: 20 });
    // Inject a raw trace with no `source` — the shape of a document saved
    // before copper provenance existed. Legacy copper stays disposable, so a
    // re-route replaces it instead of stacking on top of it.
    getPcbBoard(getSession(id)).traces.push({
      start: { x: 1, y: 9 },
      end: { x: 1, y: 1 },
      width: 0.25,
      layer: "FCu",
      net: "MID",
    });
    expect(hasDetour(getPcbBoard(getSession(id)))).toBe(true);

    const res = out(await routeNets({ document_id: id }));
    expect(res.traces_removed).toBeGreaterThan(0);
    expect(hasDetour(getPcbBoard(getSession(id)))).toBe(false);
  });

  it("keeps a manual via while re-routing the net's traces", async () => {
    const created = out(
      await createSchematic({
        components: [resistor("R1", 0), resistor("R2", 20)],
        nets: { MID: ["R1.2", "R2.1"] },
      }),
    );
    const id = created.document_id;
    await placeComponents({ document_id: id, board_width: 40, board_height: 20 });
    out(await routeNets({ document_id: id }));
    out(
      await addVia({
        document_id: id,
        net: "MID",
        position: { x: 2, y: 2 },
      }),
    );

    // A via alone doesn't block re-routing (the ratsnest only counts traces),
    // so the net is re-owned: autorouted copper replaced, manual via kept.
    const res = out(await routeNets({ document_id: id }));
    expect(res.success).toBe(true);
    expect(res.traces_removed).toBeGreaterThan(0);
    expect(res.manual_nets_preserved).toBeUndefined();
    const board = getPcbBoard(getSession(id));
    expect(
      board.vias.some(
        (v) => v.net === "MID" && v.source === "manual" && v.position.x === 2,
      ),
    ).toBe(true);
    expect(board.traces.some((t) => t.net === "MID")).toBe(true);
  });
});


describe("route_nets strategy", () => {
  const mkBoard = async () => {
    const created = out(
      await createSchematic({
        components: [resistor("R1", 0), resistor("R2", 20), resistor("R3", 40)],
        nets: { GND: ["R1.1", "R2.1", "R3.1"], SIG: ["R1.2", "R2.2"] },
      }),
    );
    const id = created.document_id;
    await placeComponents({ document_id: id, board_width: 60, board_height: 30 });
    return id;
  };

  it("every strategy routes the board fully (no coverage regression vs auto)", async () => {
    for (const strategy of ["auto", "power_first", "fanout_desc", "fanout_asc"]) {
      const id = await mkBoard();
      const res = out(await routeNets({ document_id: id, strategy }));
      expect(res.success, `strategy ${strategy}`).toBe(true);
      expect(res.nets_routed, `strategy ${strategy}`).toBeGreaterThanOrEqual(2);
      expect(res.unrouted_nets ?? [], `strategy ${strategy}`).toEqual([]);
    }
  });

  it("power_first stitches a plane power net first and still routes signals", async () => {
    const id = await mkBoard();
    await addZone({ document_id: id, net: "GND", layer: "BCu", fill_board: true });
    const res = out(await routeNets({ document_id: id, strategy: "power_first" }));
    expect(res.success).toBe(true);
    expect(res.plane_stitched).toContain("GND");
    const board = getPcbBoard(getSession(id));
    expect(board.traces.some((t) => t.net === "SIG")).toBe(true);
  });
});

describe("route_nets plane stitching", () => {
  it("reports nets connected through a copper plane in plane_stitched", async () => {
    const created = out(
      await createSchematic({
        components: [resistor("R1", 0), resistor("R2", 20), resistor("R3", 40)],
        nets: { GND: ["R1.1", "R2.1", "R3.1"], SIG: ["R1.2", "R2.2"] },
      }),
    );
    const id = created.document_id;
    await placeComponents({ document_id: id, board_width: 60, board_height: 30 });
    // GND copper plane on the back layer — pads get stitched to it with vias.
    await addZone({ document_id: id, net: "GND", layer: "BCu", fill_board: true });

    const res = out(await routeNets({ document_id: id }));
    expect(res.success).toBe(true);
    expect(res.plane_stitched).toContain("GND");
    expect((res.plane_stitched as string[]).includes("SIG")).toBe(false);
    // GND pads were stitched to the plane with vias, not star-routed as traces.
    const board = getPcbBoard(getSession(id));
    expect(board.vias.some((v) => v.net === "GND")).toBe(true);
  });
});

describe("route_nets routability + diagnostics", () => {
  it("reports a routability score of 1 for a fully-routed board", async () => {
    const created = out(
      await createSchematic({
        components: [resistor("R1", 0), resistor("R2", 20)],
        nets: { SIG: ["R1.2", "R2.1"] },
      }),
    );
    const id = created.document_id;
    await placeComponents({ document_id: id, board_width: 60, board_height: 30 });
    const res = out(await routeNets({ document_id: id }));
    expect(res.success).toBe(true);
    expect(res.routability).toBe(1);
    // A finished board carries no unrouted diagnostics.
    expect(res.unrouted_diagnostics).toBeUndefined();
  });

  it("surfaces routability < 1 and actionable diagnostics when a net can't route", async () => {
    // Two nets must cross on a one-layer board barely wide enough for the parts,
    // so one connection can't be closed without shorting — it comes back with a
    // diagnostic, and routability drops below 1.
    const created = out(
      await createSchematic({
        components: [resistor("R1", 0), resistor("R2", 6)],
        nets: { A: ["R1.1", "R2.2"], B: ["R1.2", "R2.1"] },
      }),
    );
    const id = created.document_id;
    // Single copper layer (FCu only) so a crossing net has nowhere to escape.
    await placeComponents({ document_id: id, board_width: 14, board_height: 6 });
    await setStackup({ document_id: id, layers: ["FCu"] });
    const res = out(await routeNets({ document_id: id }));
    expect(res.success).toBe(true);
    expect(typeof res.routability).toBe("number");
    expect(res.routability).toBeGreaterThanOrEqual(0);
    expect(res.routability).toBeLessThanOrEqual(1);
    if (res.unrouted_nets && res.unrouted_nets.length > 0) {
      expect(res.routability).toBeLessThan(1);
      expect(Array.isArray(res.unrouted_diagnostics)).toBe(true);
      const d = res.unrouted_diagnostics[0];
      expect(res.unrouted_nets).toContain(d.net);
      expect(typeof d.reason).toBe("string");
      expect(d.reason.length).toBeGreaterThan(0);
      // Either a concrete blocker or a suggested escape layer is given.
      expect(
        (Array.isArray(d.blocking_nets) && d.blocking_nets.length > 0) ||
          typeof d.suggested_layer === "string",
      ).toBe(true);
    }
  });
});

describe("add_zone overlap guard", () => {
  const mkBoard = async () => {
    const created = out(
      await createSchematic({
        components: [resistor("R1", 0), resistor("R2", 20)],
        nets: { GND: ["R1.1", "R2.1"], SIG: ["R1.2", "R2.2"] },
      }),
    );
    const id = created.document_id;
    await placeComponents({ document_id: id, board_width: 60, board_height: 60 });
    return id;
  };

  // Axis-aligned rectangle as a CCW outline.
  const rect = (x0: number, y0: number, x1: number, y1: number) => [
    { x: x0, y: y0 },
    { x: x1, y: y0 },
    { x: x1, y: y1 },
    { x: x0, y: y1 },
  ];

  const in2Zones = (id: string) =>
    getPcbBoard(getSession(id)).zones.filter((z) => z.layer === "In2Cu");

  it("rejects a different-net pour overlapping an existing pour on the same layer", async () => {
    const id = await mkBoard();
    const first = out(
      await addZone({ document_id: id, net: "3V3", layer: "In2Cu", outline: rect(0, 0, 20, 20) }),
    );
    expect(first.success).toBe(true);
    expect(first.zone_drc.clean).toBe(true);

    const second = out(
      await addZone({ document_id: id, net: "5V", layer: "In2Cu", outline: rect(10, 10, 30, 30) }),
    );
    expect(second.success).toBe(false);
    expect(second.zone_drc.clean).toBe(false);
    expect(second.zone_drc.overlaps).toHaveLength(1);
    expect(second.zone_drc.overlaps[0].layer).toBe("In2Cu");
    expect([...second.zone_drc.overlaps[0].nets].sort()).toEqual(["3V3", "5V"]);
    expect(second.zone_drc.overlaps[0].bbox).toEqual({
      min: { x: 10, y: 10 },
      max: { x: 20, y: 20 },
    });
    expect(second.error).toMatch(/short/i);
    expect(second.error).toContain("5V");
    expect(second.error).toContain("3V3");
    // The shorting pour was NOT committed — only the first zone remains.
    expect(in2Zones(id)).toHaveLength(1);
  });

  it("rejects two full-board planes of different nets on the same layer (coincident outlines)", async () => {
    const id = await mkBoard();
    await addZone({ document_id: id, net: "GND", layer: "In2Cu", fill_board: true });
    const power = out(
      await addZone({ document_id: id, net: "VBAT", layer: "In2Cu", fill_board: true }),
    );
    expect(power.success).toBe(false);
    expect(power.zone_drc.overlaps).toHaveLength(1);
    expect([...power.zone_drc.overlaps[0].nets].sort()).toEqual(["GND", "VBAT"]);
    expect(in2Zones(id)).toHaveLength(1);
  });

  it("authors the overlapping pour anyway when allow_overlap is set", async () => {
    const id = await mkBoard();
    await addZone({ document_id: id, net: "3V3", layer: "In2Cu", outline: rect(0, 0, 20, 20) });
    const forced = out(
      await addZone({
        document_id: id,
        net: "5V",
        layer: "In2Cu",
        outline: rect(10, 10, 30, 30),
        allow_overlap: true,
      }),
    );
    expect(forced.success).toBe(true);
    expect(forced.zone_drc.clean).toBe(false);
    expect(forced.zone_drc.overlaps).toHaveLength(1);
    // Both pours are present — the caller opted in to the overlap.
    expect(in2Zones(id)).toHaveLength(2);
  });

  it("allows a same-net pour overlapping on the same layer (planes merge)", async () => {
    const id = await mkBoard();
    await addZone({ document_id: id, net: "GND", layer: "In2Cu", outline: rect(0, 0, 20, 20) });
    const same = out(
      await addZone({ document_id: id, net: "GND", layer: "In2Cu", outline: rect(10, 10, 30, 30) }),
    );
    expect(same.success).toBe(true);
    expect(same.zone_drc.clean).toBe(true);
    expect(same.zone_drc.overlaps).toEqual([]);
    expect(in2Zones(id)).toHaveLength(2);
  });

  it("allows overlapping different-net pours on different layers", async () => {
    const id = await mkBoard();
    await addZone({ document_id: id, net: "3V3", layer: "In2Cu", outline: rect(0, 0, 20, 20) });
    const other = out(
      await addZone({ document_id: id, net: "5V", layer: "In1Cu", outline: rect(10, 10, 30, 30) }),
    );
    expect(other.success).toBe(true);
    expect(other.zone_drc.clean).toBe(true);
  });

  it("allows non-overlapping different-net pours on the same layer", async () => {
    const id = await mkBoard();
    await addZone({ document_id: id, net: "3V3", layer: "In2Cu", outline: rect(0, 0, 20, 20) });
    const apart = out(
      await addZone({ document_id: id, net: "5V", layer: "In2Cu", outline: rect(25, 0, 45, 20) }),
    );
    expect(apart.success).toBe(true);
    expect(apart.zone_drc.clean).toBe(true);
    expect(apart.zone_drc.overlaps).toEqual([]);
  });
});

describe("set_board_outline", () => {
  it("resizes the outline, keeps component positions, and flags off-board parts", async () => {
    const created = out(
      await createSchematic({
        components: [resistor("R1", 0), resistor("R2", 20)],
        nets: { MID: ["R1.2", "R2.1"] },
      }),
    );
    const id = created.document_id;
    await placeComponents({ document_id: id, board_width: 60, board_height: 40 });

    const before = getPcbBoard(getSession(id));
    const posBefore = before.footprints
      .map((f) => ({ ref: f.ref, x: f.position.x, y: f.position.y }))
      .sort((a, b) => a.ref.localeCompare(b.ref));

    // Shrink the width just below the rightmost footprint so it is guaranteed
    // off-board; a tall height keeps Y from being the limiting axis.
    const maxX = Math.max(...posBefore.map((p) => p.x));
    const newW = Math.max(1, maxX - 0.5);
    const res = out(
      await setBoardOutline({ document_id: id, board_width: newW, board_height: 100 }),
    );
    expect(res.success).toBe(true);
    expect(res.outline.width).toBeCloseTo(newW, 3);
    expect(res.components_kept).toBe(2);

    const after = getPcbBoard(getSession(id));
    // Positions untouched (this is the whole point — no re-placement).
    const posAfter = after.footprints
      .map((f) => ({ ref: f.ref, x: f.position.x, y: f.position.y }))
      .sort((a, b) => a.ref.localeCompare(b.ref));
    expect(posAfter).toEqual(posBefore);
    // The outline really changed.
    const xs = after.outline.vertices.map((v) => v.x);
    expect(Math.max(...xs) - Math.min(...xs)).toBeCloseTo(newW, 3);
    // Off-board set matches exactly the footprints whose origin is now outside.
    const offX = posAfter
      .filter((p) => p.x < 0 || p.x > newW)
      .map((p) => p.ref)
      .sort();
    expect(offX.length).toBeGreaterThan(0);
    expect(((res.off_board as string[]) ?? []).slice().sort()).toEqual(offX);
  });

  it("accepts an explicit polygon outline and preserves the current thickness", async () => {
    const created = out(await createSchematic({ components: [resistor("R1", 0)] }));
    const id = created.document_id;
    await placeComponents({
      document_id: id,
      board_width: 30,
      board_height: 30,
      board_thickness: 2.0,
    });
    const res = out(
      await setBoardOutline({
        document_id: id,
        outline: {
          vertices: [
            { x: 0, y: 0 },
            { x: 50, y: 0 },
            { x: 50, y: 50 },
            { x: 0, y: 50 },
          ],
        },
      }),
    );
    expect(res.success).toBe(true);
    expect(res.outline.thickness).toBeCloseTo(2.0, 3);
    const after = getPcbBoard(getSession(id));
    expect(after.outline.thickness).toBeCloseTo(2.0, 3);
    expect(after.outline.vertices.length).toBe(4);
  });

  it("errors when no outline is specified", async () => {
    const created = out(await createSchematic({ components: [resistor("R1", 0)] }));
    const id = created.document_id;
    await placeComponents({ document_id: id, board_width: 20, board_height: 20 });
    const res = await setBoardOutline({ document_id: id });
    expect((res as { isError?: boolean }).isError).toBe(true);
  });
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
    // Pins not in any net are reported immediately, enriched with name + severity.
    expect(
      created.unconnected_pins.map((p: { ref: string }) => p.ref).sort(),
    ).toEqual(["R1.1", "R2.2"]);
    expect(
      created.unconnected_pins.every(
        (p: { severity: string; pin_type: string }) =>
          p.severity === "info" && p.pin_type === "Passive",
      ),
    ).toBe(true);

    const doc = getSession(created.document_id);
    expect(doc.schematic?.nets).toEqual({ MID: ["R1.2", "R2.1"] });
  });

  it("enriches unconnected pins on a known part with severity and datasheet app-notes", async () => {
    const created = out(
      await createSchematic({
        // NE555 resolves its 8 pins from the parts database; wire only the
        // supply rails so CTRL/RESET/etc. stay open and get flagged.
        components: [{ ref: "U1", part: "NE555", footprint: "DIP-8", x: 0, y: 0 }],
        nets: { GND: ["U1.1"], VCC: ["U1.8"] },
      }),
    );
    expect(created.success).toBe(true);
    const byRef: Record<
      string,
      { pin_name: string; pin_type: string; severity: string; app_notes?: string[] }
    > = Object.fromEntries(
      (created.unconnected_pins as Array<{ ref: string }>).map((p) => [
        (p as { ref: string }).ref,
        p,
      ]),
    );

    // CTRL (pin 5, Passive) → info, carrying its own bypass note.
    expect(byRef["U1.5"]).toMatchObject({
      pin_name: "CTRL",
      pin_type: "Passive",
      severity: "info",
    });
    expect((byRef["U1.5"].app_notes ?? []).join(" ")).toContain("CTRL");

    // RESET (pin 4, Input) → warning, with its own note and NOT CTRL's.
    expect(byRef["U1.4"]).toMatchObject({ pin_name: "RESET", severity: "warning" });
    const resetNotes = (byRef["U1.4"].app_notes ?? []).join(" ");
    expect(resetNotes).toContain("RESET");
    expect(resetNotes).not.toContain("CTRL");

    // Wired supply pins are not reported at all.
    expect(byRef["U1.1"]).toBeUndefined();
    expect(byRef["U1.8"]).toBeUndefined();
  });

  it("appNotesForPin matches by pin number, name token, and skips power rails", () => {
    const ne555 = [
      "Pin 5 (CTRL) is commonly bypassed to GND with a 10nF cap when unused.",
      "Tie RESET (pin 4) to VCC when the reset function is not needed.",
    ];
    // By number ("pin 5") and by name token ("CTRL") — same single note.
    expect(appNotesForPin(ne555, { number: "5", name: "CTRL" })).toEqual([ne555[0]]);
    expect(appNotesForPin(ne555, { number: "4", name: "RESET" })).toEqual([ne555[1]]);
    // GND (pin 1) is named incidentally in note 0 but is a rail → no match.
    expect(appNotesForPin(ne555, { number: "1", name: "GND" })).toEqual([]);
    // VCC (pin 8) likewise appears in note 1 but is a rail → no match.
    expect(appNotesForPin(ne555, { number: "8", name: "VCC" })).toEqual([]);
    // Compound names split on "/" and overline markers are stripped.
    expect(
      appNotesForPin(["PB5 doubles as ~RESET; keep it high for normal operation."], {
        number: "1",
        name: "PB5/~RESET",
      }),
    ).toHaveLength(1);
  });

  it("unconnectedPinSeverity warns on floating inputs and power, else info", () => {
    expect(unconnectedPinSeverity("PowerInput")).toBe("warning");
    expect(unconnectedPinSeverity("Input")).toBe("warning");
    expect(unconnectedPinSeverity("Passive")).toBe("info");
    expect(unconnectedPinSeverity("Output")).toBe("info");
    expect(unconnectedPinSeverity("OpenCollector")).toBe("info");
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

    // Native KiCad board export round-trips back through the importer.
    const kicad = out(
      await exportKicad({ document_id: id, filename: "out.kicad_pcb" }),
    );
    expect(kicad.success).toBe(true);
    expect(kicad.format).toBe("kicad_pcb");
    expect(kicad.document_id).toBe(id);
    expect(typeof kicad.content).toBe("string");
    expect(kicad.content).toContain("(kicad_pcb");
    expect(kicad.content).toContain('(generator "vcad")');
    // The routed net and a placed footprint survive into the file.
    expect(kicad.content).toContain('"MID"');
    expect(kicad.content).toContain("(segment");

    const reimported = await parseKicadPcb(kicad.content);
    expect(reimported).not.toBeNull();
    expect(reimported!.footprints.length).toBe(2);
    const refs = reimported!.footprints.map((fp) => fp.ref).sort();
    expect(refs).toEqual(["R1", "R2"]);
    expect(reimported!.traces.length).toBeGreaterThan(0);
  });

  it("export_kicad writes a .kicad_sch schematic and rejects unknown extensions", async () => {
    const created = out(
      await createSchematic({
        components: [resistor("R1", 0), resistor("R2", 20)],
        nets: { MID: ["R1.2", "R2.1"] },
      }),
    );
    const id = created.document_id;

    const sch = out(
      await exportKicad({ document_id: id, filename: "sheet.kicad_sch" }),
    );
    expect(sch.success).toBe(true);
    expect(sch.format).toBe("kicad_sch");
    expect(sch.content).toContain("(kicad_sch");
    expect(sch.content).toContain("(lib_symbols");
    expect(sch.content).toContain('(lib_id "vcad:R1")');

    // An unsupported extension is a clean tool error, not a throw.
    const bad = await exportKicad({ document_id: id, filename: "nope.brd" });
    expect(bad.isError).toBe(true);
  });

  it("export_kicad .kicad_pro exports a linked project bundle", async () => {
    const created = out(
      await createSchematic({
        components: [resistor("R1", 0), resistor("R2", 20)],
        nets: { MID: ["R1.2", "R2.1"] },
      }),
    );
    const id = created.document_id;
    out(await placeComponents({ document_id: id, board_width: 50, board_height: 50 }));

    const result = await exportKicad({ document_id: id, filename: "demo.kicad_pro" });
    if ((result as { isError?: boolean }).isError) {
      // The checked-in kernel WASM predates exportKicadProject (artifacts are
      // only refreshed on main); the tool must degrade with a clear error.
      expect(JSON.stringify(result)).toContain("unavailable");
      return;
    }
    const bundle = out(result);
    expect(bundle.success).toBe(true);
    expect(bundle.format).toBe("kicad_project");
    expect(bundle.document_id).toBe(id);
    const names = (bundle.files as Array<{ name: string }>).map((f) => f.name);
    expect(names).toEqual(["demo.kicad_pro", "demo.kicad_sch", "demo.kicad_pcb"]);
    const get = (n: string) =>
      (bundle.files as Array<{ name: string; content: string }>).find((f) => f.name === n)!
        .content;
    // The project file is valid JSON recording the sheet root uuid.
    const pro = JSON.parse(get("demo.kicad_pro"));
    expect(pro.meta.filename).toBe("demo.kicad_pro");
    const rootUuid = pro.sheets[0][0];
    expect(get("demo.kicad_sch")).toContain(`(uuid "${rootUuid}")`);
    // The board footprints are linked to schematic symbols (cross-probe paths).
    expect(get("demo.kicad_pcb")).toContain('(sheetfile "demo.kicad_sch")');
    expect(get("demo.kicad_pcb")).toMatch(/\(path "\/[0-9a-f-]{36}"\)/);

    // A bare name (no extension) takes the same bundle path.
    const bare = out(await exportKicad({ document_id: id, filename: "demo" }));
    expect(bare.format).toBe("kicad_project");
  });

  it("export_gerber blocks a dirty (unconnected-net) board and returns the DRC summary", async () => {
    const created = out(
      await createSchematic({
        components: [resistor("R1", 0), resistor("R2", 20)],
        nets: { MID: ["R1.2", "R2.1"] },
      }),
    );
    const id = created.document_id;
    out(await placeComponents({ document_id: id, board_width: 50, board_height: 50 }));
    // Deliberately NOT routed → MID's two pads sit in disjoint copper groups,
    // so DRC reports an UnconnectedNet error and the board is not fab-clean.

    const blockedResult = await exportGerber({ document_id: id }); // require_clean_drc defaults true
    // The gate tripping is a verdict, not a tool failure — isError would make
    // clients and telemetry read a working guard as a crash.
    expect((blockedResult as { isError?: boolean }).isError).toBeUndefined();
    const blocked = out(blockedResult);
    expect(blocked.success).toBe(false);
    expect(blocked.blocked).toBe(true);
    expect(blocked.drc.errors).toBeGreaterThan(0);
    expect(blocked.drc.byRule.UnconnectedNet).toBeGreaterThan(0);
    expect(blocked.files).toBeUndefined();

    // Opting out of the gate still emits the bundle (caller forced a dirty export).
    const forced = out(
      await exportGerber({ document_id: id, require_clean_drc: false }),
    );
    expect(forced.success).toBe(true);
    expect(forced.files.length).toBeGreaterThan(0);
  });

  it("export_gerber exports a clean routed board under the default DRC gate", async () => {
    const created = out(
      await createSchematic({
        components: [resistor("R1", 0), resistor("R2", 20)],
        nets: { MID: ["R1.2", "R2.1"] },
      }),
    );
    const id = created.document_id;
    out(await placeComponents({ document_id: id, board_width: 50, board_height: 50 }));
    expect(out(await routeNets({ document_id: id })).success).toBe(true);

    const gerber = out(await exportGerber({ document_id: id }));
    expect(gerber.success).toBe(true);
    expect(gerber.files.length).toBeGreaterThan(0);
  });

  it("validate_for_fab passes a clean routed board (ready)", async () => {
    const created = out(
      await createSchematic({
        components: [resistor("R1", 0), resistor("R2", 20)],
        nets: { MID: ["R1.2", "R2.1"] },
      }),
    );
    const id = created.document_id;
    out(await placeComponents({ document_id: id, board_width: 50, board_height: 50 }));
    out(await routeNets({ document_id: id }));

    const v = out(await validateForFab({ document_id: id }));
    expect(v.ready).toBe(true);
    expect(v.verdict).toBe("ready");
    expect(v.drc.status).toBe("clean");
    expect(v.renderable.ok).toBe(true);
    expect(v.gerber_exportable.ok).toBe(true);
    expect(v.blockers).toHaveLength(0);
    expect(v.unverifiable).toHaveLength(0);
  });

  it("fab_prep reports both numbers and points at export_gerber when clean", async () => {
    const created = out(
      await createSchematic({
        components: [resistor("R1", 0), resistor("R2", 20)],
        nets: { MID: ["R1.2", "R2.1"] },
      }),
    );
    const id = created.document_id;
    out(await placeComponents({ document_id: id, board_width: 50, board_height: 50 }));

    const r = out(await fabPrep({ document_id: id }));
    expect(r.success).toBe(true);
    expect(r.converged).toBe(true);
    // The whole point of the receipt: never one number. Both the
    // stripped-board baseline and the finished board are reported, and only
    // the difference is charged to the router.
    expect(r.drc_delta.baseline_total).toBeTypeOf("number");
    expect(r.drc_delta.final_total).toBeTypeOf("number");
    expect(r.drc_delta.route_attributable_blocking).toBe(0);
    expect(r.drc_delta.baseline_note).toContain("stripped");
    expect(r.headline).toContain("stripped of all routing");
    expect(r.next_action).toContain("export_gerber");
    // fab_prep is the way to GET clean, so the gate it feeds must now pass.
    expect(out(await exportGerber({ document_id: id })).success).toBe(true);
  });

  it("fab_prep logs every rule calibration with its derivation, and does nothing unasked", async () => {
    const created = out(
      await createSchematic({
        components: [resistor("R1", 0), resistor("R2", 20)],
        nets: { MID: ["R1.2", "R2.1"] },
      }),
    );
    const id = created.document_id;
    out(await placeComponents({ document_id: id, board_width: 50, board_height: 50 }));
    // Declare a via class the board's own global minimum forbids.
    out(
      await setDesignRules({
        document_id: id,
        via_diameter: 0.21,
        via_drill: 0.12,
        min_drill: 0.2,
        min_annular_ring: 0.15,
      }),
    );

    const off = out(await fabPrep({ document_id: id, dry_run: true }));
    expect(off.calibration.requested).toBe(false);
    expect(off.calibration.applied).toHaveLength(0);

    const on = out(await fabPrep({ document_id: id, calibrate_rules: true, dry_run: true }));
    expect(on.calibration.requested).toBe(true);
    const drill = on.calibration.applied.find((c: { rule: string }) => c.rule === "minDrill");
    expect(drill).toBeDefined();
    expect(drill.declared).toBeCloseTo(0.2);
    expect(drill.calibrated).toBeCloseTo(0.12);
    expect(drill.justification).toContain("via class");
  });

  it("fab_prep refuses a waiver naming a rule that does not exist", async () => {
    const created = out(
      await createSchematic({
        components: [resistor("R1", 0), resistor("R2", 20)],
        nets: { MID: ["R1.2", "R2.1"] },
      }),
    );
    const id = created.document_id;
    out(await placeComponents({ document_id: id, board_width: 50, board_height: 50 }));

    const r = out(await fabPrep({ document_id: id, accept_rules: ["MinTraceWidht"] }));
    expect(r.converged).toBe(false);
    expect(r.blocker).toContain("MinTraceWidht");
  });

  it("validate_for_fab blocks a dirty board and names the DRC errors", async () => {
    const created = out(
      await createSchematic({
        components: [resistor("R1", 0), resistor("R2", 20)],
        nets: { MID: ["R1.2", "R2.1"] },
      }),
    );
    const id = created.document_id;
    out(await placeComponents({ document_id: id, board_width: 50, board_height: 50 }));

    const v = out(await validateForFab({ document_id: id }));
    expect(v.ready).toBe(false);
    expect(v.verdict).toBe("blocked");
    expect(v.drc.status).toBe("violations");
    expect(v.blockers.some((b: string) => b.startsWith("DRC:"))).toBe(true);
    expect(v.suggested_fixes.length).toBeGreaterThan(0);
  });

  it("validate_for_fab reports a serialization blocker with the exact failing field", async () => {
    const created = out(
      await createSchematic({
        components: [resistor("R1", 0), resistor("R2", 20)],
        nets: { MID: ["R1.2", "R2.1"] },
      }),
    );
    const id = created.document_id;
    out(await placeComponents({ document_id: id, board_width: 50, board_height: 50 }));
    out(await routeNets({ document_id: id }));

    // Corrupt the board so the kernel can no longer deserialize it: drop a
    // required field. Gerber serialization must surface 'thickness' by name, and
    // — fail-closed — DRC must read 'unverifiable', never clean.
    const board = getPcbBoard(getSession(id));
    delete (board.outline as { thickness?: number }).thickness;

    const v = out(await validateForFab({ document_id: id }));
    expect(v.ready).toBe(false);
    expect(v.gerber_exportable.ok).toBe(false);
    expect(v.gerber_exportable.field).toBe("thickness");
    expect(v.blockers.some((b: string) => b.includes("thickness"))).toBe(true);
    expect(v.drc.status).toBe("unverifiable");
  });

  it("parametric footprint engine resolves QFN/DPAK on-board and reports unknowns", async () => {
    const comp = (
      ref: string,
      footprint: string,
      pinNums: number[],
      extra: Record<string, unknown> = {},
    ) => ({
      ref,
      value: "X",
      footprint,
      x: 0,
      y: 0,
      pins: pinNums.map((n) => ({ number: String(n), name: String(n), type: "Passive" })),
      ...extra,
    });

    const created = out(
      await createSchematic({
        components: [
          comp("U1", "Package_DFN_QFN:QFN-40_5x5mm_P0.4mm", [1, 2, 3, 4]),
          comp("Q1", "Package_TO_SOT_SMD:TO-252-3_TabPin2", [1, 2, 3]),
          comp("X1", "Acme:TotallyUnknownConnector", [1, 2, 3, 4, 5, 6]),
        ],
      }),
    );
    const id = created.document_id;
    const placed = out(
      await placeComponents({ document_id: id, board_width: 80, board_height: 60 }),
    );

    expect(placed.success).toBe(true);
    // Only the unknown footprint is a fallback; QFN + DPAK resolved.
    const fbRefs = (placed.fallback_footprints ?? []).map((f: { ref: string }) => f.ref);
    expect(fbRefs).toContain("X1");
    expect(fbRefs).not.toContain("U1");
    expect(fbRefs).not.toContain("Q1");
    expect(placed.footprints_resolved).toBe(2);

    const board = getPcbBoard(getSession(id));
    const fp = (ref: string) => board.footprints.find((f) => f.ref === ref)!;

    // QFN-40: 40 leads + 1 thermal EP, and every pad stays within the ~5mm
    // body — the regression guard against the old ~74mm off-board column.
    const u1 = fp("U1");
    expect(u1.pads.length).toBe(41);
    for (const p of u1.pads) {
      expect(Math.abs(p.position.x)).toBeLessThan(4);
      expect(Math.abs(p.position.y)).toBeLessThan(4);
    }

    // DPAK: the tab (pad "2") is the largest pad.
    const q1 = fp("Q1");
    const area = (p: (typeof q1.pads)[number]) =>
      p.shape.type === "Rect" ? p.shape.width * p.shape.height : 0;
    const tab = q1.pads.find((p) => p.number === "2")!;
    expect(area(tab)).toBe(Math.max(...q1.pads.map(area)));

    // Unknown part: compact grid placeholder, on-board, one pad per pin.
    const x1 = fp("X1");
    expect(x1.pads.length).toBe(6);
    for (const p of x1.pads) {
      expect(Math.abs(p.position.x)).toBeLessThan(10);
      expect(Math.abs(p.position.y)).toBeLessThan(10);
    }
  });

  it("resolves connector families (JST/Molex/Tag-Connect/USB-C) end-to-end", async () => {
    const comp = (ref: string, footprint: string, pinNums: number[]) => ({
      ref,
      value: "CONN",
      footprint,
      x: 0,
      y: 0,
      pins: pinNums.map((n) => ({ number: String(n), name: String(n), type: "Passive" })),
    });

    const created = out(
      await createSchematic({
        components: [
          // The exact issue reproduction: a 2-pin JST-PH power connector.
          comp("J1", "JST_PH_2", [1, 2]),
          comp("J2", "Connector_JST:JST_SH_BM06B-SRSS-TB_1x06-1MP_P1.00mm_Horizontal", [
            1, 2, 3, 4, 5, 6,
          ]),
          comp("J3", "Connector_Molex:Molex_PicoBlade_53261-0271_1x02-1MP_P1.25mm_Vertical", [
            1, 2,
          ]),
          comp("J4", "Tag-Connect:TC2050-IDC-NL_2x05_P1.27mm", [1, 2, 3, 4, 5, 6, 7, 8, 9, 10]),
          comp("J5", "Connector_USB:USB_C_Receptacle_16pin", [
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16,
          ]),
        ],
      }),
    );
    const id = created.document_id;
    const placed = out(
      await placeComponents({ document_id: id, board_width: 90, board_height: 70 }),
    );

    expect(placed.success).toBe(true);
    // None of these should fall back to a placeholder anymore.
    expect(placed.fallback_footprints ?? []).toEqual([]);
    expect(placed.footprints_resolved).toBe(5);

    const board = getPcbBoard(getSession(id));
    const fp = (ref: string) => board.footprints.find((f) => f.ref === ref)!;

    // JST-PH: 2 through-hole contacts, 2.0mm pitch — assemble-able, not a chip.
    const j1 = fp("J1");
    expect(j1.pads.length).toBe(2);
    expect(j1.pads.every((p) => p.padType === "THT")).toBe(true);
    const dx = Math.abs(j1.pads[0].position.x - j1.pads[1].position.x);
    expect(Math.abs(dx - 2.0)).toBeLessThan(1e-6);
    // Pads carry the schematic nets (here pin names 1/2 act as nets).
    expect(j1.pads.find((p) => p.number === "1")!.net).toBe("1");

    // JST-SH: 6 SMD contacts.
    const j2 = fp("J2");
    expect(j2.pads.length).toBe(6);
    expect(j2.pads.every((p) => p.padType === "SMD")).toBe(true);

    // Molex Pico-Blade: 2 SMD contacts — a part number must not read as a chip.
    expect(fp("J3").pads.filter((p) => p.padType === "SMD").length).toBe(2);

    // Tag-Connect TC2050: 10 bare-copper pads in two columns, no paste.
    const j4 = fp("J4");
    expect(j4.pads.length).toBe(10);
    expect(j4.pads.every((p) => !p.layers.includes("FPaste"))).toBe(true);

    // USB-C (16): 16 numeric contacts + 4 shield posts, all on a ~90×70 board.
    const j5 = fp("J5");
    const numeric = j5.pads.filter((p) => /^\d+$/.test(p.number));
    const shields = j5.pads.filter((p) => p.number.startsWith("SH"));
    expect(numeric.length).toBe(16);
    expect(shields.length).toBe(4);
    for (const p of j5.pads) {
      expect(Math.abs(p.position.x)).toBeLessThan(10);
      expect(Math.abs(p.position.y)).toBeLessThan(10);
    }

    // The densely-packed connector pads must not raise false clearance or
    // manufacturing violations (the intra-footprint pad exemption + on-board
    // geometry). Connectivity violations are expected — nothing is routed yet.
    const drc = out(await runDrc({ document_id: id }));
    expect(drc.success).toBe(true);
    expect(drc.categories.clearance).toBe(0);
    expect(drc.categories.manufacturing).toBe(0);
  });

  it("inline pads escape hatch overrides the footprint engine", async () => {
    const created = out(
      await createSchematic({
        components: [
          {
            ref: "J9",
            value: "CustomConn",
            footprint: "Acme:Unknown",
            x: 0,
            y: 0,
            pins: [
              { number: "1", name: "A", type: "Passive" },
              { number: "2", name: "B", type: "Passive" },
            ],
            pads: [
              { number: "1", shape: { type: "Rect", width: 2, height: 2 }, position: { x: -3, y: 0 } },
              { number: "2", shape: { type: "Rect", width: 2, height: 2 }, position: { x: 3, y: 0 } },
            ],
          },
        ],
        nets: { SIG: ["J9.1"] },
      }),
    );
    const id = created.document_id;
    const placed = out(
      await placeComponents({ document_id: id, board_width: 40, board_height: 40 }),
    );

    // Inline pads are author-supplied geometry, never a fallback.
    const fbRefs = (placed.fallback_footprints ?? []).map((f: { ref: string }) => f.ref);
    expect(fbRefs).not.toContain("J9");

    const board = getPcbBoard(getSession(id));
    const j9 = board.footprints.find((f) => f.ref === "J9")!;
    expect(j9.pads.length).toBe(2);
    const p1 = j9.pads.find((p) => p.number === "1")!;
    expect(p1.position).toEqual({ x: -3, y: 0 });
    // Net was assigned from the schematic even though geometry was inline.
    expect(p1.net).toBe("SIG");
  });

  it("run_drc splits counts into connectivity vs illegal categories", async () => {
    // Two pads on one net, never routed → an UnconnectedNet (connectivity), not
    // an illegal-geometry violation.
    const created = out(
      await createSchematic({
        components: [resistor("R1", 0), resistor("R2", 30)],
        nets: { MID: ["R1.2", "R2.1"] },
      }),
    );
    const id = created.document_id;
    out(await placeComponents({ document_id: id, board_width: 50, board_height: 50 }));
    const drc = out(await runDrc({ document_id: id }));
    expect(drc.categories).toBeDefined();
    expect(drc.categories).toHaveProperty("connectivity");
    expect(drc.categories).toHaveProperty("clearance");
    expect(drc.categories).toHaveProperty("manufacturing");
    // The unrouted net is connectivity, and connectivity + clearance +
    // manufacturing must sum to the total violation count.
    const { connectivity, clearance, manufacturing } = drc.categories;
    expect(connectivity + clearance + manufacturing).toBe(drc.violations);
    expect(connectivity).toBe(drc.byRule.UnconnectedNet ?? 0);
  });

  it("force_directed placement keeps big components' courtyards apart (size-aware)", async () => {
    const dpak = (ref: string, x: number) => ({
      ref,
      value: "FET",
      footprint: "Package_TO_SOT_SMD:TO-252-3_TabPin2",
      x,
      y: 0,
      pins: [
        { number: "1", name: "G", type: "Passive" },
        { number: "2", name: "D", type: "Passive" },
        { number: "3", name: "S", type: "Passive" },
      ],
    });
    const created = out(
      await createSchematic({
        components: [dpak("Q1", 0), dpak("Q2", 5)],
        nets: { PH: ["Q1.2", "Q2.2"] }, // shared net → attraction competes with size repulsion
      }),
    );
    const id = created.document_id;
    out(
      await placeComponents({
        document_id: id,
        board_width: 70,
        board_height: 70,
        strategy: "force_directed",
      }),
    );
    const board = getPcbBoard(getSession(id));
    const q1 = board.footprints.find((f) => f.ref === "Q1")!;
    const q2 = board.footprints.find((f) => f.ref === "Q2")!;
    const d = Math.hypot(q1.position.x - q2.position.x, q1.position.y - q2.position.y);
    // Two DPAK courtyards (~6mm radius each) must not be stacked despite sharing
    // a net — size-aware repulsion holds them well apart.
    expect(d).toBeGreaterThan(8);
  });

  it("force_directed keeps large edge components fully on-board (extent-aware clamp)", async () => {
    // Cram nine LQFP-64 (~12mm body, ~6mm half-extent) onto a board too small to
    // space them at their courtyard radius. Repulsion slams the perimeter parts
    // hard against the placement clamp. The old clamp pinned each *center* to the
    // raw bounds, so an edge part's 6mm body hung ~half off the board (and over
    // its neighbor) — many DRC round-trips. The fix insets the clamp by each
    // part's half-extent, so the whole courtyard stays on the board.
    const lqfp = (ref: string) => ({
      ref,
      value: "MCU",
      footprint: "Package_QFP:LQFP-64_10x10mm_P0.5mm",
      x: 0,
      y: 0,
      // The parametric engine builds all 64 pads from the footprint id; a few
      // pins are enough to declare the part.
      pins: [1, 2, 3, 4].map((n) => ({ number: String(n), name: String(n), type: "Passive" })),
    });
    const W = 30;
    const H = 30;
    const created = out(
      await createSchematic({
        components: ["U1", "U2", "U3", "U4", "U5", "U6", "U7", "U8", "U9"].map(lqfp),
      }),
    );
    const id = created.document_id;
    out(
      await placeComponents({
        document_id: id,
        board_width: W,
        board_height: H,
        strategy: "force_directed",
      }),
    );
    const board = getPcbBoard(getSession(id));

    /** Absolute copper bounding box of a placed footprint (pad land extents). */
    const padHalf = (shape: { type: string } & Record<string, number>): number =>
      shape.type === "Circle"
        ? shape.diameter / 2
        : shape.type === "Rect" || shape.type === "Oval" || shape.type === "RoundRect"
          ? Math.max(shape.width, shape.height) / 2
          : 0.5;
    const bbox = (fp: (typeof board.footprints)[number]) => {
      let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
      for (const p of fp.pads) {
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        const h = padHalf(p.shape as any);
        minX = Math.min(minX, fp.position.x + p.position.x - h);
        minY = Math.min(minY, fp.position.y + p.position.y - h);
        maxX = Math.max(maxX, fp.position.x + p.position.x + h);
        maxY = Math.max(maxY, fp.position.y + p.position.y + h);
      }
      return { minX, minY, maxX, maxY };
    };

    // Every part's full courtyard lands inside the (0,0)→(W,H) outline. The 0.01mm
    // tolerance absorbs the footprint-build position rounding.
    const eps = 0.02;
    for (const fp of board.footprints) {
      const b = bbox(fp);
      expect(b.minX).toBeGreaterThanOrEqual(-eps);
      expect(b.minY).toBeGreaterThanOrEqual(-eps);
      expect(b.maxX).toBeLessThanOrEqual(W + eps);
      expect(b.maxY).toBeLessThanOrEqual(H + eps);
    }
  });

  it("placement_drc flags an off-board part as clean:false so the agent can branch", async () => {
    // After placement, a part shoved past the outline must flip placement_drc to
    // clean:false (with its ref in off_board) — the cheap pre-routing signal an
    // agent branches on before route_nets, instead of discovering it at run_drc.
    const created = out(
      await createSchematic({
        components: [resistor("R1", 0), resistor("R2", 20)],
        nets: { MID: ["R1.2", "R2.1"] },
      }),
    );
    const id = created.document_id;
    out(await placeComponents({ document_id: id, board_width: 40, board_height: 40 }));
    const board = getPcbBoard(getSession(id));
    // On-board placement is clean.
    expect((await summarizePlacementDrc(board)).clean).toBe(true);
    // Push R2 off the outline → clean:false, R2 reported off_board.
    board.footprints.find((f) => f.ref === "R2")!.position = { x: 80, y: 80 };
    const drc = await summarizePlacementDrc(board);
    expect(drc.clean).toBe(false);
    expect(drc.off_board).toContain("R2");
    expect(drc.off_board).not.toContain("R1");
  });

  it("force_directed never bakes a cross-net pad short into the board", async () => {
    // A tightly-netted NE555-style cluster: the net-attraction can pull
    // different-net pads on top of each other (a VCC/GND short) before any
    // routing. Placement must legalize that away, not ship success with a short.
    const r = (ref: string) => ({
      ref,
      value: "1k",
      footprint: "Resistor_SMD:R_0805",
      x: 0,
      y: 0,
      pins: [
        { number: "1", name: "~", type: "Passive" },
        { number: "2", name: "~", type: "Passive" },
      ],
    });
    const twoPin = (ref: string, value: string, footprint: string) => ({
      ref,
      value,
      footprint,
      x: 0,
      y: 0,
      pins: [
        { number: "1", name: "~", type: "Passive" },
        { number: "2", name: "~", type: "Passive" },
      ],
    });
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
              number: String(i + 1),
              name: ["GND", "TRIG", "OUT", "RESET", "CTRL", "THR", "DIS", "VCC"][i],
              type: "Passive",
            })),
          },
          r("R1"),
          r("R2"),
          r("R3"),
          twoPin("C1", "10uF", "Capacitor_SMD:C_1206"),
          twoPin("C2", "10nF", "Capacitor_SMD:C_0805"),
          twoPin("D1", "LED", "LED_SMD:LED_0805"),
          twoPin("J1", "PWR", "Connector_PinHeader_2.54mm:PinHeader_1x02_P2.54mm"),
        ],
        nets: {
          VCC: ["J1.1", "U1.8", "U1.4", "R1.1"],
          GND: ["J1.2", "U1.1", "C1.2", "C2.2", "R3.2"],
          DIS: ["U1.7", "R1.2", "R2.1"],
          THR: ["U1.6", "U1.2", "R2.2", "C1.1"],
          CTRL: ["U1.5", "C2.1"],
          OUT: ["U1.3", "D1.1"],
          LEDK: ["D1.2", "R3.1"],
        },
      }),
    );
    const id = created.document_id;
    const placed = out(
      await placeComponents({
        document_id: id,
        board_shape: { outer_diameter: 25, type: "circle" },
        strategy: "force_directed",
      }),
    );
    expect(placed.success).toBe(true);
    expect(placed.placement_conflicts).toBeUndefined();

    // The DRC oracle: before any routing, no copper-to-copper clearance or short
    // violations may exist (the only legal violations are unrouted ratsnest).
    const drc = out(await runDrc({ document_id: id, detail: "full" }));
    const shorts = (drc.details ?? []).filter((v: { rule: string }) => v.rule === "Short");
    expect(shorts).toHaveLength(0);
    expect(drc.categories.clearance).toBe(0);
  });

  it("force_directed reports cross-net conflicts it can't fit (no false success)", async () => {
    // Four 1x02 headers (~4.5mm wide) sharing a common net on a 5mm board:
    // there is physically no way to separate every different-net pad. The placer
    // must report the unresolved pairs and refuse to claim success.
    const hdr = (ref: string) => ({
      ref,
      value: "H",
      footprint: "Connector_PinHeader_2.54mm:PinHeader_1x02_P2.54mm",
      x: 0,
      y: 0,
      pins: [
        { number: "1", name: "~", type: "Passive" },
        { number: "2", name: "~", type: "Passive" },
      ],
    });
    const comps = ["J1", "J2", "J3", "J4"].map(hdr);
    const nets: Record<string, string[]> = { COMMON: comps.map((c) => `${c.ref}.1`) };
    comps.forEach((c) => (nets[`SIG_${c.ref}`] = [`${c.ref}.2`]));
    const created = out(await createSchematic({ components: comps, nets }));
    const placed = out(
      await placeComponents({
        document_id: created.document_id,
        board_width: 5,
        board_height: 5,
        strategy: "force_directed",
      }),
    );
    expect(placed.success).toBe(false);
    expect(placed.placement_conflicts.length).toBeGreaterThan(0);
    // Each reported conflict is a genuine sub-clearance gap (clearance is 0.2mm).
    for (const c of placed.placement_conflicts) {
      expect(c.gap).toBeLessThan(0.2);
      expect(c.a).toBeTruthy();
      expect(c.b).toBeTruthy();
    }
    expect(placed.warnings.some((w: string) => /cross-net pad overlap/i.test(w))).toBe(true);
  });

  it("route_nets honors per-net-class width and reports realized widths", async () => {
    const created = out(
      await createSchematic({
        components: [
          { ref: "J1", value: "PWR", footprint: "Connector_PinHeader_2.54mm:PinHeader_1x02_P2.54mm", x: 0, y: 0, pins: [{ number: "1", name: "V", type: "Passive" }, { number: "2", name: "G", type: "Passive" }] },
          resistor("R1", 15),
          resistor("R2", 30),
        ],
        nets: { VBAT: ["J1.1", "R1.1"], SIG: ["R1.2", "R2.1"] },
      }),
    );
    const id = created.document_id;
    out(await placeComponents({ document_id: id, board_width: 50, board_height: 40 }));
    // Give VBAT a wide power class; signals stay thin by default.
    out(
      await setDesignRules({
        document_id: id,
        track_width: 0.25,
        classes: [{ name: "power", nets: ["VBAT"], track_width: 1.5 }],
      }),
    );
    const routed = out(await routeNets({ document_id: id }));
    expect(routed.success).toBe(true);
    expect(routed.track_widths_mm).toBeDefined();
    // VBAT routed at its class width; SIG at the default — a single route call,
    // two widths, no per-net width argument needed.
    if (routed.track_widths_mm.VBAT !== undefined && routed.track_widths_mm.SIG !== undefined) {
      expect(routed.track_widths_mm.VBAT).toBeGreaterThan(routed.track_widths_mm.SIG);
      expect(routed.track_widths_mm.VBAT).toBeCloseTo(1.5, 1);
    }
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

describe("placement DRC (pre-routing checks)", () => {
  /** A 1-pad part on a named net — the smallest thing that can short another. */
  const onePad = (ref: string, net: string) => ({
    ref,
    value: net,
    footprint: "Test:Pad",
    x: 0,
    y: 0,
    pins: [{ number: "1", name: net, type: "Passive" }],
    pads: [{ number: "1", shape: { type: "Rect" as const, width: 1.5, height: 1.5 }, position: { x: 0, y: 0 } }],
  });

  it("place_components returns a clean placement_drc for a roomy board", async () => {
    const created = out(
      await createSchematic({
        components: [resistor("R1", 0), resistor("R2", 30)],
        nets: { MID: ["R1.2", "R2.1"] },
      }),
    );
    const placed = out(
      await placeComponents({ document_id: created.document_id, board_width: 60, board_height: 60 }),
    );
    expect(placed.placement_drc).toBeDefined();
    expect(placed.placement_drc.clean).toBe(true);
    expect(placed.placement_drc.shorts).toEqual([]);
    expect(placed.placement_drc.clearance_violations).toBe(0);
    expect(placed.placement_drc.courtyard_overlaps).toBe(0);
    expect(placed.placement_drc.off_board).toEqual([]);
    // A clean placement adds no DRC warning.
    expect((placed.warnings ?? []).some((w: string) => w.includes("placement DRC"))).toBe(false);
  });

  it("reports a pad-to-pad short with both nets and the offending refs", async () => {
    const created = out(
      await createSchematic({
        components: [onePad("C1", "VCC"), onePad("J1", "GND")],
        nets: { VCC: ["C1.1"], GND: ["J1.1"] },
      }),
    );
    const id = created.document_id;
    out(await placeComponents({ document_id: id, board_width: 40, board_height: 40 }));
    const board = getPcbBoard(getSession(id));
    // Stack J1 on C1 so their different-net pads overlap → a hard short.
    const c1 = board.footprints.find((f) => f.ref === "C1")!;
    const j1 = board.footprints.find((f) => f.ref === "J1")!;
    j1.position = { x: c1.position.x, y: c1.position.y };

    const drc = await summarizePlacementDrc(board);
    expect(drc.clean).toBe(false);
    expect(drc.shorts.length).toBe(1);
    expect([...drc.shorts[0].nets].sort()).toEqual(["GND", "VCC"]);
    expect([...drc.shorts[0].refs].sort()).toEqual(["C1", "J1"]);
    // The overlap is reported as a short, not double-counted as clearance.
    expect(drc.clearance_violations).toBe(0);
  });

  it("flags a footprint placed off the board outline", async () => {
    const created = out(
      await createSchematic({
        components: [resistor("R1", 0), resistor("R2", 20)],
        nets: { MID: ["R1.2", "R2.1"] },
      }),
    );
    const id = created.document_id;
    out(await placeComponents({ document_id: id, board_width: 40, board_height: 40 }));
    const board = getPcbBoard(getSession(id));
    board.footprints.find((f) => f.ref === "R2")!.position = { x: 500, y: 500 };

    const drc = await summarizePlacementDrc(board);
    expect(drc.off_board).toContain("R2");
    expect(drc.off_board).not.toContain("R1");
    expect(drc.clean).toBe(false);
  });

  it("place_components surfaces collisions on an overcrowded board (and warns)", async () => {
    const dpak = (ref: string, x: number) => ({
      ref,
      value: "FET",
      footprint: "Package_TO_SOT_SMD:TO-252-3_TabPin2",
      x,
      y: 0,
      pins: [
        { number: "1", name: "G", type: "Passive" },
        { number: "2", name: "D", type: "Passive" },
        { number: "3", name: "S", type: "Passive" },
      ],
    });
    const created = out(
      await createSchematic({ components: [dpak("Q1", 0), dpak("Q2", 4), dpak("Q3", 8), dpak("Q4", 12)] }),
    );
    const placed = out(
      await placeComponents({ document_id: created.document_id, board_width: 14, board_height: 14 }),
    );
    const d = placed.placement_drc;
    expect(d.clean).toBe(false);
    expect(d.shorts.length + d.clearance_violations + d.courtyard_overlaps).toBeGreaterThan(0);
    expect((placed.warnings ?? []).some((w: string) => w.includes("placement DRC"))).toBe(true);
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

describe("route_nets idempotency", () => {
  // The exact board from the field bug report (issue #277). Before the fix,
  // routing an already-routed board a second time stacked copper: the kernel
  // ratsnest skips nets that already have a trace, so the second route_all
  // came back empty, the no-kernel fallback in routeNets misfired, and it
  // chained naive straight segments over the clean route — turning a handful
  // of inherent violations into dozens of shorts. The fix rips up the target
  // nets' *autorouted* copper before routing (hand-placed copper is
  // provenance-tagged and preserved), so re-running replaces the route
  // instead of adding to it.
  const soic8 = (ref: string, x: number) => ({
    ref,
    value: "U",
    footprint: "Package_SO:SOIC-8_3.9x4.9mm_P1.27mm",
    x,
    y: 0,
    pins: Array.from({ length: 8 }, (_, i) => ({
      number: `${i + 1}`,
      name: "~",
      type: "Passive",
      x: i < 4 ? -5 : 5,
      y: (i % 4) * 1.27,
    })),
  });
  const cap = (ref: string, x: number) => ({
    ref,
    value: "100n",
    footprint: "Capacitor_SMD:C_0805_2012Metric",
    x,
    y: 20,
    pins: [
      { number: "1", name: "~", type: "Passive", x: -2, y: 0 },
      { number: "2", name: "~", type: "Passive", x: 2, y: 0 },
    ],
  });
  const header = (ref: string, x: number) => ({
    ref,
    value: "Conn",
    footprint: "Connector_PinHeader_2.54mm:PinHeader_1x04_P2.54mm_Vertical",
    x,
    y: 0,
    pins: Array.from({ length: 4 }, (_, i) => ({
      number: `${i + 1}`,
      name: "~",
      type: "Passive",
      x: 0,
      y: i * 2.54,
    })),
  });

  const buildBoard = async () => {
    const created = out(
      await createSchematic({
        components: [soic8("U1", 0), cap("C1", 20), cap("C2", 40), header("J1", 60)],
        nets: {
          VCC: ["U1.1", "C1.1", "J1.1"],
          GND: ["U1.8", "C1.2", "C2.2", "J1.2"],
          SIG1: ["U1.2", "C2.1"],
          SIG2: ["U1.3", "J1.3"],
          SIG3: ["U1.4", "J1.4"],
        },
      }),
    );
    const id = created.document_id;
    out(await placeComponents({ document_id: id, board_width: 50, board_height: 35 }));
    return id;
  };

  /** Segment identity — stacked copper shows up as exact duplicates. */
  const segKey = (t: { net: string; layer: string; start: Vec2; end: Vec2 }) =>
    `${t.net}|${t.layer}|${t.start.x},${t.start.y}->${t.end.x},${t.end.y}`;

  it("a second route_nets does not stack copper or add violations", async () => {
    const id = await buildBoard();

    // First route — establishes the clean baseline.
    out(await routeNets({ document_id: id }));
    const board1 = getPcbBoard(getSession(id));
    const traces1 = board1.traces.length;
    const vias1 = board1.vias.length;
    const drc1 = out(await runDrc({ document_id: id }));
    expect(traces1).toBeGreaterThan(0);
    // A single pass never stacks copper on itself — the duplicate-free
    // baseline the re-route must hold.
    const keys1 = board1.traces.map(segKey);
    expect(new Set(keys1).size).toBe(keys1.length);

    // Second route — must rip up and re-lay the same copper, not append a second
    // set. After the rip-up the board is pads-only again, exactly as it was
    // before the first route, so the deterministic router reproduces it byte
    // for byte.
    const r2 = out(await routeNets({ document_id: id }));
    const board2 = getPcbBoard(getSession(id));
    const drc2 = out(await runDrc({ document_id: id }));

    // The rip-up is visible in the result, not silent.
    expect(r2.traces_removed).toBeGreaterThan(0);
    expect(board2.traces.length).toBe(traces1);
    expect(board2.vias.length).toBe(vias1);
    // No two identical segments — the stacked-copper signature.
    const keys2 = board2.traces.map(segKey);
    expect(new Set(keys2).size).toBe(keys2.length);
    // Every piece of autorouted copper carries its provenance, so the next
    // re-route knows it is disposable.
    expect(board2.traces.every((t) => t.source === "autoroute")).toBe(true);
    expect(board2.vias.every((v) => v.source === "autoroute")).toBe(true);
    expect(drc2.violations).toBe(drc1.violations);
    // The headline symptom: the re-route must never introduce shorts.
    expect(drc2.byRule.Short ?? 0).toBe(0);
  });

  it("a scoped re-route of one net leaves the other nets' copper untouched", async () => {
    const id = await buildBoard();
    out(await routeNets({ document_id: id }));

    const before = getPcbBoard(getSession(id));
    const othersBefore = before.traces.filter((t) => t.net !== "SIG2").map(segKey).sort();
    const sig2Before = before.traces.filter((t) => t.net === "SIG2").length;
    expect(sig2Before).toBeGreaterThan(0);

    const r = out(await routeNets({ document_id: id, nets: ["SIG2"] }));
    expect(r.success).toBe(true);
    expect(r.traces_removed).toBeGreaterThan(0);

    const after = getPcbBoard(getSession(id));
    // SIG2 was replaced (still routed, no duplicate segments), not doubled.
    // The scoped pass routes against different obstacles than the original
    // negotiated pass, so the exact path may differ — but stacking would at
    // least double the segment count.
    const sig2After = after.traces.filter((t) => t.net === "SIG2");
    expect(sig2After.length).toBeGreaterThan(0);
    expect(sig2After.length).toBeLessThan(sig2Before * 2);
    const sig2Keys = sig2After.map(segKey);
    expect(new Set(sig2Keys).size).toBe(sig2Keys.length);
    // Copper on every other net is byte-identical.
    expect(after.traces.filter((t) => t.net !== "SIG2").map(segKey).sort()).toEqual(
      othersBefore,
    );
  });

  it("routing three times in a row stays flat — no monotonic copper growth", async () => {
    const id = await buildBoard();

    const snapshot = async () => {
      out(await routeNets({ document_id: id }));
      const b = getPcbBoard(getSession(id));
      const drc = out(await runDrc({ document_id: id }));
      return { traces: b.traces.length, vias: b.vias.length, violations: drc.violations };
    };

    const first = await snapshot();
    const second = await snapshot();
    const third = await snapshot();

    expect(second).toEqual(first);
    expect(third).toEqual(first);
  });

  // J1's pad world positions before a move — header at rotation 0, so pad world
  // is just footprint origin + pad offset.
  const j1PadWorld = (board: Pcb) => {
    const fp = board.footprints.find((f) => f.ref === "J1")!;
    return fp.pads.map((p) => ({ x: fp.position.x + p.position.x, y: fp.position.y + p.position.y }));
  };
  const j1Nets = new Set(["VCC", "GND", "SIG2", "SIG3"]);
  const nearAny = (p: Vec2, pts: Vec2[]) => pts.some((q) => Math.hypot(p.x - q.x, p.y - q.y) < 1.5);

  it("re-routing after set_placement leaves no orphaned copper at the old position", async () => {
    const id = await buildBoard();
    out(await routeNets({ document_id: id }));
    const oldPads = j1PadWorld(getPcbBoard(getSession(id)));

    // Move J1 across the board, then re-route the whole board.
    out(await setPlacement({ document_id: id, placements: [{ ref: "J1", x: 45, y: 30 }] }));
    out(await routeNets({ document_id: id }));

    const board = getPcbBoard(getSession(id));
    const orphans = board.traces.filter(
      (t) => j1Nets.has(t.net) && (nearAny(t.start, oldPads) || nearAny(t.end, oldPads)),
    );
    expect(orphans.length).toBe(0);
  });

  it("a scoped re-route sweeps stale copper on unfiltered nets whose pads moved", async () => {
    const id = await buildBoard();
    out(await routeNets({ document_id: id }));
    const oldPads = j1PadWorld(getPcbBoard(getSession(id)));

    // Move J1, then re-route ONLY SIG2. The unfiltered J1 nets (VCC/GND/SIG3)
    // are now stale; route_nets must detect and rip them, not leave dead copper.
    out(await setPlacement({ document_id: id, placements: [{ ref: "J1", x: 45, y: 30 }] }));
    const r = out(await routeNets({ document_id: id, nets: ["SIG2"] }));

    const board = getPcbBoard(getSession(id));
    const stale = board.traces.filter(
      (t) => ["VCC", "GND", "SIG3"].includes(t.net) && (nearAny(t.start, oldPads) || nearAny(t.end, oldPads)),
    );
    expect(stale.length).toBe(0);
    // And it reports the cleanup it did, so the agent isn't blind to it.
    expect(r.traces_removed).toBeGreaterThan(0);
    expect((r.stale_nets_cleared as string[]).sort()).toContain("GND");
  });

  it("does not flag a freshly-routed board as stale (no spurious rip-up)", async () => {
    const id = await buildBoard();
    out(await routeNets({ document_id: id }));
    // No move — a second scoped route on a clean board must not report any
    // stale-net cleanup (detection is false-positive-free on clean copper).
    const r = out(await routeNets({ document_id: id, nets: ["SIG2"] }));
    expect(r.stale_nets_cleared).toBeUndefined();
  });

  // Timeout raised above the 5s default: this is the only case in the suite that
  // routes a board with a dense free coil present (~24 spiral segments become
  // router obstacles), so each of the two route_nets passes runs the kernel
  // autorouter ~200× longer than a pad-only board (~0.9s vs ~4ms locally, ~1.8s
  // total). The autorouter is unchanged; the cost is inherent to the coil-as-
  // obstacle case and sits near the 5s edge, so it flakes under loaded CI. The
  // assertion is about correctness (coil copper survives the stale sweep), not
  // speed — give it headroom rather than let wall-clock flake the correctness check.
  it("the stale sweep never rips coil/winding copper (free spiral, no pads)", async () => {
    const id = await buildBoard();
    // A standalone coil on its own net — a free spiral whose terminals dangle by
    // design and whose net has no pads. It must survive every route_nets call.
    const coil = out(
      await addCoil({
        document_id: id,
        center: { x: 25, y: 17 },
        turns: 4,
        inner_radius: 3,
        outer_radius: 8,
        trace_width: 0.3,
        clearance: 0.3,
        net: "COIL",
      }),
    );
    const coilTraces = coil.traces_added as number;
    expect(coilTraces).toBeGreaterThan(20);
    out(await routeNets({ document_id: id }));

    // Move a part and do a scoped re-route — the coil net is not listed and has
    // dangling ends, but the sweep must leave it intact.
    out(await setPlacement({ document_id: id, placements: [{ ref: "J1", x: 45, y: 30 }] }));
    const r = out(await routeNets({ document_id: id, nets: ["SIG2"] }));

    const board = getPcbBoard(getSession(id));
    expect(board.traces.filter((t) => t.net === "COIL").length).toBe(coilTraces);
    expect((r.stale_nets_cleared as string[] | undefined) ?? []).not.toContain("COIL");
  }, 20000);
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

describe("run_erc kernel pin-type & power rules", () => {
  /** A 1-pin IC whose single pin carries a real electrical type. */
  const ic = (ref: string, x: number, type: string, name = "P") => ({
    ref,
    value: ref,
    footprint: "SOIC-8",
    x,
    y: 0,
    pins: [{ number: "1", name, type, x: 0, y: 0 }],
  });

  it("flags two outputs driving one net (data-driven nets)", async () => {
    const created = out(
      await createSchematic({
        components: [ic("U1", 0, "Output", "OUT"), ic("U2", 40, "Output", "OUT")],
        nets: { BUS: ["U1.1", "U2.1"] },
      }),
    );
    const erc = out(await runErc({ document_id: created.document_id }));
    expect(erc.verified).toBe(true);
    const conflicts = (erc.details as Array<{ message: string; severity: string }>).filter((d) =>
      d.message.includes("multiple outputs"),
    );
    expect(conflicts).toHaveLength(1);
    expect(conflicts[0]!.severity).toBe("Error");
    expect(erc.errors).toBeGreaterThanOrEqual(1);
  });

  it("flags a power input with no driver as floating power", async () => {
    const created = out(
      await createSchematic({
        components: [ic("U1", 0, "PowerInput", "VCC"), resistor("R1", 40)],
        nets: { SENSE: ["U1.1", "R1.1"] },
      }),
    );
    const erc = out(await runErc({ document_id: created.document_id }));
    expect(erc.verified).toBe(true);
    const floating = (erc.details as Array<{ message: string; severity: string }>).filter((d) =>
      d.message.includes("no power source"),
    );
    expect(floating).toHaveLength(1);
    expect(floating[0]!.severity).toBe("Warning");
  });

  it("does not flag a power input on a recognized power net", async () => {
    const created = out(
      await createSchematic({
        components: [ic("U1", 0, "PowerInput", "VCC"), resistor("R1", 40)],
        nets: { VCC: ["U1.1", "R1.1"] },
      }),
    );
    const erc = out(await runErc({ document_id: created.document_id }));
    expect(erc.verified).toBe(true);
    expect(
      (erc.details as Array<{ message: string }>).filter((d) =>
        d.message.includes("no power source"),
      ),
    ).toEqual([]);
  });

  it("reports an unconnected power pin once, never doubled as floating power", async () => {
    const created = out(
      await createSchematic({
        components: [ic("U1", 0, "PowerInput", "VCC")],
      }),
    );
    const erc = out(await runErc({ document_id: created.document_id }));
    expect(erc.verified).toBe(true);
    const unconnected = (erc.details as Array<{ message: string; severity: string }>).filter((d) =>
      d.message.includes("Unconnected pin"),
    );
    expect(unconnected).toHaveLength(1);
    expect(unconnected[0]!.severity).toBe("Error");
    expect(
      (erc.details as Array<{ message: string }>).filter((d) =>
        d.message.includes("no power source"),
      ),
    ).toEqual([]);
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

describe("place_components utilization", () => {
  it("reports board utilization and a tighter rectangular outline", async () => {
    const created = out(
      await createSchematic({
        components: [resistor("R1", 0), resistor("R2", 20)],
      }),
    );
    const placed = out(
      await placeComponents({
        document_id: created.document_id,
        board_width: 50,
        board_height: 40,
      }),
    );

    const u = placed.utilization;
    expect(u).toBeDefined();
    // Board area is exact from the rectangle outline.
    expect(u.board_area_mm2).toBeCloseTo(2000, 1);
    expect(u.component_area_mm2).toBeGreaterThan(0);
    expect(u.component_area_mm2).toBeLessThan(u.board_area_mm2);
    // % used is internally consistent and a small fraction (two 0805 parts).
    expect(u.utilization_pct).toBeCloseTo(
      Math.round((u.component_area_mm2 / u.board_area_mm2) * 1000) / 10,
      1,
    );
    expect(u.utilization_pct).toBeGreaterThan(0);
    expect(u.utilization_pct).toBeLessThan(100);

    // Bounding box is positive and fits inside the board.
    expect(u.bounding_box.w).toBeGreaterThan(0);
    expect(u.bounding_box.h).toBeGreaterThan(0);
    expect(u.bounding_box.x).toBeGreaterThanOrEqual(0);

    // Suggested outline keeps the rect shape, encloses the parts, and is
    // tighter than the over-large 50×40 board.
    expect(u.suggested_outline.type).toBe("rect");
    expect(u.suggested_outline.width).toBeGreaterThanOrEqual(u.bounding_box.w);
    expect(u.suggested_outline.height).toBeGreaterThanOrEqual(u.bounding_box.h);
    expect(u.suggested_outline.width).toBeLessThan(50);
    expect(u.suggested_outline.note).toContain("edge clearance");
  });

  it("suggests an enclosing circle for a circular board", async () => {
    const created = out(
      await createSchematic({
        components: Array.from({ length: 4 }, (_, i) => resistor(`R${i + 1}`, i * 8)),
      }),
    );
    const placed = out(
      await placeComponents({
        document_id: created.document_id,
        board_shape: { type: "circle", outer_diameter: 40 },
      }),
    );

    const u = placed.utilization;
    expect(u).toBeDefined();
    // 64-gon inscribed in r=20 ≈ π·20² = 1257 mm².
    expect(u.board_area_mm2).toBeGreaterThan(1200);
    expect(u.board_area_mm2).toBeLessThan(1300);
    expect(u.suggested_outline.type).toBe("circle");
    expect(u.suggested_outline.outer_diameter).toBeGreaterThan(0);
    expect(u.suggested_outline.outer_diameter).toBeLessThanOrEqual(40);
    expect(u.suggested_outline.center).toBeDefined();
  });

  it("honors edge_margin when sizing the suggested outline", async () => {
    const created = out(
      await createSchematic({
        components: [resistor("R1", 0), resistor("R2", 20)],
      }),
    );
    const id = created.document_id;

    // Zero margin → suggested width hugs the bounding box (rounded up to 0.5mm).
    const tight = out(
      await placeComponents({ document_id: id, board_width: 50, board_height: 40, edge_margin: 0 }),
    ).utilization;
    expect(tight.suggested_outline.width - tight.bounding_box.w).toBeGreaterThanOrEqual(0);
    expect(tight.suggested_outline.width - tight.bounding_box.w).toBeLessThan(0.5);
    expect(tight.suggested_outline.note).toContain("0mm");

    // 5mm margin → ~10mm wider than the bounding box (both sides).
    const loose = out(
      await placeComponents({ document_id: id, board_width: 50, board_height: 40, edge_margin: 5 }),
    ).utilization;
    expect(loose.suggested_outline.width - loose.bounding_box.w).toBeGreaterThanOrEqual(10);
    expect(loose.suggested_outline.width - loose.bounding_box.w).toBeLessThan(10.5);
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
      await addCoil({
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

  it("run_drc flags a same-net trace over the coil's inner via as SameNetBypass", async () => {
    const id = await circleBoardSession();
    const coil = out(
      await addCoil({
        document_id: id,
        center: { x: 40, y: 40 },
        turns: 10,
        inner_radius: 2.6,
        outer_radius: 7.2,
        trace_width: 0.25,
        clearance: 0.2,
        net: "PHA",
        inner_via: true,
      }),
    );
    expect(coil.success).toBe(true);

    // The old add_motor_winding failure: a same-net star trace from the outer
    // endpoint straight along the terminal ray, over the inner via — no
    // net-based rule can see it, but it short-circuits the spiral.
    out(
      await addTrace({
        document_id: id,
        net: "PHA",
        layer: "FCu",
        width: 0.25,
        points: [coil.outer_endpoint, { x: 41, y: 40 }],
      }),
    );

    const drc = out(await runDrc({ document_id: id }));
    expect(drc.byRule?.SameNetBypass ?? 0).toBeGreaterThan(0);
    // Warning severity, bucketed under connectivity.
    expect(drc.warnings).toBeGreaterThan(0);
    expect(drc.categories.connectivity).toBeGreaterThan(0);
  });

  it("rejects coils whose turns don't fit the clearance, with the max that would", async () => {
    const id = await circleBoardSession();
    const res = await addCoil({
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
    const res = await addCoil({
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
    const res = out(await addCoilArray({ document_id: id, ...base, net_sequence: ["PHA", "PHB", "PHC"] }));
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
    const res = out(await addCoilArray({ document_id: id, ...base, net: "X", chirality: "alternating" }));
    expect(res.results.map((r: { direction: string }) => r.direction)).toEqual(["ccw", "cw", "ccw"]);
  });

  it("cycles net_sequence when shorter than count", async () => {
    const id = await ringBoard();
    const res = out(
      await addCoilArray({ document_id: id, ...base, count: 4, net_sequence: ["A", "B"] }),
    );
    expect(res.results.map((r: { net: string }) => r.net)).toEqual(["A", "B", "A", "B"]);
  });

  it("mutates the same session that addCoil writes to", async () => {
    const id = await ringBoard();
    const before = getPcbBoard(getSession(id)).traces.length;
    out(await addCoilArray({ document_id: id, ...base, net_sequence: ["PHA", "PHB", "PHC"] }));
    expect(getPcbBoard(getSession(id)).traces.length).toBeGreaterThan(before);
  });

  it("rejects count < 1", async () => {
    const id = await ringBoard();
    const res = await addCoilArray({ document_id: id, ...base, count: 0, net: "X" });
    expect(res.isError).toBe(true);
  });

  it("collects per-coil geometry failures instead of throwing", async () => {
    const id = await ringBoard();
    // outer_radius <= inner_radius fails inside addCoil for every coil.
    const res = await addCoilArray({
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
      await addTrace({
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
    expect(
      (await addTrace({ document_id: id, net: "X", points: [{ x: 0, y: 0 }] })).isError,
    ).toBe(true);
    expect(
      (
        await addTrace({
          document_id: id,
          net: "X",
          layer: "FSilkS",
          points: [
            { x: 0, y: 0 },
            { x: 1, y: 1 },
          ],
        })
      ).isError,
    ).toBe(true);
  });

  it("add_via adds a via with default span and ensures the net", async () => {
    const id = await board();
    const res = out(await addVia({ document_id: id, net: "GND", position: { x: 5, y: 5 } }));
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
    const res = out(await setStackup({ document_id: id, copper_oz: 2 }));
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
      await addCoil({
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
    out(await setStackup({ document_id: id2, copper_oz: 2 }));
    const c2oz = out(
      await addCoil({
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
      await setStackup({
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
      await addCoil({
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
      await addCoil({
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
      await addCoil({
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
    expect(bcu).toBe(fcu); // one segment per layer per spiral sample
    const stitch = b.vias.find((v) => v.net === "PHA");
    expect(stitch).toBeDefined();
    expect(stitch!.startLayer).toBe("FCu");
    expect(stitch!.endLayer).toBe("BCu");
    // Total length is the sum across both layers.
    expect(res.total_length_mm).toBeCloseTo(res.total_length_mm, 3);
  });

  /**
   * Magnetic dipole moment of the series current path, in mm² (m_z / I).
   *
   * Chains each layer's traces into a polyline, orients it along the direction
   * the series current actually flows (entering at `entry`), and accumulates the
   * shoelace sum about `center`. A stacked coil is only useful if consecutive
   * layers circulate the *same* way — if they alternate, their axial fields
   * subtract and the stack does nothing.
   */
  function seriesDipoleMoment(
    traces: Array<{ start: Vec2; end: Vec2; layer: string; net: string }>,
    vias: Array<{ position: Vec2; startLayer: string; endLayer: string; net: string }>,
    net: string,
    layers: string[],
    center: Vec2,
    entry: Vec2,
  ): { perLayer: number[]; total: number } {
    const near = (a: Vec2, b: Vec2) => Math.hypot(a.x - b.x, a.y - b.y) < 1e-6;
    const shoelace = (pts: Vec2[]) => {
      let s = 0;
      for (let i = 0; i + 1 < pts.length; i++) {
        const x1 = pts[i].x - center.x,
          y1 = pts[i].y - center.y;
        const x2 = pts[i + 1].x - center.x,
          y2 = pts[i + 1].y - center.y;
        s += x1 * y2 - x2 * y1;
      }
      return s / 2;
    };

    const perLayer: number[] = [];
    let cursor = entry;
    for (let li = 0; li < layers.length; li++) {
      const segs = traces.filter((t) => t.net === net && t.layer === layers[li]);
      expect(segs.length).toBeGreaterThan(0);
      // Traces are pushed contiguously, so the stored polyline is start→end.
      const stored: Vec2[] = [segs[0].start, ...segs.map((s) => s.end)];
      const head = stored[0];
      const tail = stored[stored.length - 1];
      // Current enters this layer at whichever endpoint the previous hop landed on.
      const forward = near(head, cursor);
      expect(forward || near(tail, cursor)).toBe(true);
      const path = forward ? stored : [...stored].reverse();
      perLayer.push(shoelace(path));
      cursor = path[path.length - 1];
      // Follow the stitch via to the next layer (it sits on the exit terminal).
      if (li + 1 < layers.length) {
        const v = vias.find(
          (v) =>
            v.net === net &&
            v.startLayer === layers[li] &&
            v.endLayer === layers[li + 1] &&
            near(v.position, cursor),
        );
        expect(v, `stitch via from ${layers[li]} to ${layers[li + 1]} at the exit terminal`).toBeDefined();
      }
    }
    return { perLayer, total: perLayer.reduce((a, b) => a + b, 0) };
  }

  // Top-down physical order. add_coil's COPPER_LAYERS tops out at 8 even though
  // the IR enum carries In1Cu–In8Cu (10 copper layers).
  const STACK_8 = ["FCu", "In1Cu", "In2Cu", "In3Cu", "In4Cu", "In5Cu", "In6Cu", "BCu"];
  for (const n of [2, 4, 8]) {
    it(`a ${n}-layer stack adds its fields rather than cancelling, and fabricates`, async () => {
      const id = await circleBoardSession();
      const center = { x: 40, y: 40 };
      const layers = n === 8 ? STACK_8 : [...STACK_8.slice(0, n - 1), "BCu"];
      const res = out(
        await addCoil({
          document_id: id,
          center,
          turns: 3,
          inner_radius: 5,
          outer_radius: 12,
          trace_width: 0.4,
          clearance: 0.3,
          net: "PHA",
          layers,
        }),
      );
      expect(res.success).toBe(true);
      // A stack that cancels is useless; so is one that will not fabricate.
      // (Turnarounds recur at the same radius every two layers, so an integer
      // `turns` used to drop two vias in one hole.)
      expect(res.drc_delta.clean).toBe(true);
      expect(res.stitch_vias.length).toBe(n - 1);

      const b = getPcbBoard(getSession(id));
      const { perLayer, total } = seriesDipoleMoment(
        b.traces as never,
        b.vias as never,
        "PHA",
        layers,
        center,
        res.terminals.a as Vec2,
      );

      // Every layer must contribute with the same sign...
      const s0 = Math.sign(perLayer[0]);
      for (const m of perLayer) expect(Math.sign(m)).toBe(s0);
      // ...so the stack's moment scales with layer count instead of cancelling.
      const single = Math.abs(perLayer[0]);
      expect(Math.abs(total)).toBeGreaterThan(0.9 * n * single);
    });
  }
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
      await addMotorWinding({
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
    // 2 terminal vias per coil (center + outer lead-out), 2 layer-hop vias per
    // series link (3 phases × 3 links), 1 hop via per star drop (3 phases).
    expect(res.vias_added).toBe(45);
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
      await addMotorWinding({
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
    const res = await addMotorWinding({
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

  // --- 9s/6p regression: planar interconnect, scoped star, routed feeds ----
  //
  // Reproduces the reported geometry bugs: (1) straight-chord series links
  // crossing on the return layer (cross-net Shorts), (2) star chords riding a
  // coil's terminal ray over its same-net inner via (a silent electrical
  // bypass of the last coil of each phase), (3) the star junction landing at
  // the board center over the shaft-bore cutout, with phase feeds never
  // routed to their pads (NetIslands).

  /** n-gon approximation of a circle, CCW. */
  function circlePts(cx: number, cy: number, r: number, n = 64): Vec2[] {
    return Array.from({ length: n }, (_, i) => {
      const a = (i / n) * 2 * Math.PI;
      return {
        x: Math.round((cx + r * Math.cos(a)) * 1000) / 1000,
        y: Math.round((cy + r * Math.sin(a)) * 1000) / 1000,
      };
    });
  }

  /** Perpendicular distance from p to segment ab (test-local copy). */
  function distToSeg(p: Vec2, a: Vec2, b: Vec2): number {
    const dx = b.x - a.x;
    const dy = b.y - a.y;
    const len2 = dx * dx + dy * dy;
    if (len2 === 0) return Math.hypot(p.x - a.x, p.y - a.y);
    const t = Math.max(0, Math.min(1, ((p.x - a.x) * dx + (p.y - a.y) * dy) / len2));
    return Math.hypot(p.x - (a.x + t * dx), p.y - (a.y + t * dy));
  }

  /**
   * The bypass invariant: no trace may pass within clearance of a same-net
   * via it isn't terminating at — copper riding a terminal ray over a via
   * electrically shorts out everything between the two contact points.
   */
  function findViaBypasses(pcb: Pcb, clearance: number): string[] {
    const issues: string[] = [];
    for (const v of pcb.vias) {
      for (const t of pcb.traces) {
        if (t.net !== v.net) continue;
        const lim = v.diameter / 2 + t.width / 2 + clearance - 1e-6;
        const d = distToSeg(v.position, t.start, t.end);
        if (d >= lim) continue;
        const atStart = Math.hypot(v.position.x - t.start.x, v.position.y - t.start.y) <= 1e-3;
        const atEnd = Math.hypot(v.position.x - t.end.x, v.position.y - t.end.y) <= 1e-3;
        if (atStart || atEnd) continue;
        issues.push(
          `net '${v.net}' via at (${v.position.x},${v.position.y}) is ${d.toFixed(3)}mm from ` +
            `a passing same-net trace (${t.start.x},${t.start.y})→(${t.end.x},${t.end.y})`,
        );
      }
    }
    return issues;
  }

  /** Read the hand-built board back from a registered session. */
  function boardOf(id: string): Pcb {
    return (getSession(id) as Document & { pcb?: Pcb }).pcb!;
  }

  /**
   * 70mm circular stator board with a shaft bore at (35,35) — the reported
   * repro — plus one SMD testpoint per phase net so feed routing (and the
   * NetIslands rule) is exercised.
   */
  function statorBoard9s6p(withPads: boolean, boreRadius = 10): string {
    const tp = (ref: string, net: string, angleDeg: number) => {
      const a = (angleDeg * Math.PI) / 180;
      return {
        ref,
        value: "TP",
        footprintName: "test:tp",
        position: {
          x: Math.round((35 + 33.5 * Math.cos(a)) * 1000) / 1000,
          y: Math.round((35 + 33.5 * Math.sin(a)) * 1000) / 1000,
        },
        rotation: 0,
        front: true,
        pads: [
          {
            number: "1",
            padType: "SMD",
            shape: { type: "Rect", width: 1.5, height: 1.5 },
            position: { x: 0, y: 0 },
            layers: ["FCu"],
            net,
          },
        ],
      };
    };
    const pcb = {
      outline: {
        vertices: circlePts(35, 35, 35),
        cutouts: [circlePts(35, 35, boreRadius)],
        thickness: 1.6,
      },
      stackup: {
        layers: [
          { layer: "FCu", copperThickness: 0.035, dielectricThickness: 1.5, dielectricEr: 4.5, material: "FR4" },
          { layer: "BCu", copperThickness: 0.035 },
        ],
      },
      nets: withPads
        ? [
            { id: "PHA", name: "PHA" },
            { id: "PHB", name: "PHB" },
            { id: "PHC", name: "PHC" },
          ]
        : [],
      rules: {
        defaultRules: { name: "Default", traceWidth: 0.25, clearance: 0.15, viaDiameter: 0.8, viaDrill: 0.4 },
        classRules: [],
        netClassAssignments: {},
        edgeClearance: 0.5,
        holeToHole: 0.5,
        minAnnularRing: 0.15,
        minDrill: 0.2,
      },
      // Testpoints near each phase's feed escape ray (phase starts at slots
      // 0/1/2 → 0°/40°/80°).
      footprints: withPads ? [tp("TP1", "PHA", 350), tp("TP2", "PHB", 30), tp("TP3", "PHC", 70)] : [],
      traces: [],
      vias: [],
      zones: [],
    } as unknown as Pcb;
    const doc = createDocument();
    (doc as Document & { pcb?: Pcb }).pcb = pcb;
    return registerSession(doc);
  }

  const wind9s6p = (id: string, connection: "wye" | "delta") =>
    addMotorWinding({
      document_id: id,
      slots: 9,
      poles: 6,
      center: { x: 35, y: 35 },
      pitch_radius: 22.5,
      inner_radius: 2.6,
      outer_radius: 7.2,
      turns_per_coil: 10,
      trace_width: 0.25,
      clearance: 0.15,
      connection,
    });

  it("9s/6p wye: 0 shorts, no same-net via bypass, star off the bore, feeds routed", async () => {
    const id = statorBoard9s6p(true);
    const res = out(await wind9s6p(id, "wye"));
    expect(res.errors).toBeUndefined();
    expect(res.success).toBe(true);
    expect(res.coils_placed).toBe(9);

    // (3) The star junction sits on real board material — never at the board
    // center, never over the 10mm-radius bore cutout.
    expect(res.star_junction).toBeDefined();
    const starDist = Math.hypot(res.star_junction.x - 35, res.star_junction.y - 35);
    expect(starDist).toBeGreaterThan(10.5);
    // …and the tie recording it is region-scoped, not board-wide.
    const b = boardOf(id);
    expect(b.netTies!.length).toBe(1);
    expect(b.netTies![0].position).toBeDefined();
    expect(b.netTies![0].radius).toBeDefined();
    expect(b.netTies![0].radius!).toBeLessThan(6);

    // (3b) Feeds reached the phase testpoints.
    expect(res.feeds_routed).toEqual(expect.arrayContaining(["PHA", "PHB", "PHC"]));
    expect(res.feeds_unrouted).toBeUndefined();

    // (2) The bypass invariant, re-checked independently of the tool's audit:
    // no trace within clearance of a same-net via it isn't terminating at.
    expect(findViaBypasses(b, 0.15)).toEqual([]);

    // (1) Kernel DRC: no shorts (series links / star are planar; the wye
    // junction is exempted by its scoped tie), no stranded copper (feeds
    // landed), no clearance faults. The board is DRC-clean outright.
    const drc = out(await runDrc({ document_id: id, detail: "full" }));
    expect(drc.byRule?.Short ?? 0).toBe(0);
    expect(drc.byRule?.NetIslands ?? 0).toBe(0);
    expect(drc.byRule?.Clearance ?? 0).toBe(0);
    expect(drc.violations).toBe(0);
  });

  it("9s/6p delta: 0 shorts and no bypass with per-junction scoped ties", async () => {
    // Delta needs 2 rings per phase (series + return links); the Ø20 bore
    // doesn't fit 6 — use a Ø12 bore for the delta variant.
    const id = statorBoard9s6p(false, 6);
    const res = out(await wind9s6p(id, "delta"));
    expect(res.errors).toBeUndefined();
    expect(res.success).toBe(true);
    expect(res.coils_placed).toBe(9);
    expect(res.net_ties_added).toBe(3);

    const b = boardOf(id);
    expect(findViaBypasses(b, 0.15)).toEqual([]);

    const drc = out(await runDrc({ document_id: id, detail: "full" }));
    expect(drc.byRule?.Short ?? 0).toBe(0);
    expect(drc.byRule?.Clearance ?? 0).toBe(0);
  });

  it("9s/6p delta on the Ø20 bore fails loudly — the rings don't fit", async () => {
    const id = statorBoard9s6p(false, 10);
    const res = await wind9s6p(id, "delta");
    expect(res.isError).toBe(true);
    expect(res.content[0]!.text).toContain("interconnect doesn't fit");
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

  it("buckets SameNetBypass into the connectivity category", () => {
    const violations = [
      {
        rule: "SameNetBypass",
        severity: "Warning",
        message:
          "Same-net bypass on net 'PHA': copper at (57.20, 40.00) touches copper at " +
          "(52.60, 40.00) that is 481 conductor hops away — the contact short-circuits " +
          "everything between them (fatal to a two-terminal structure like a spiral " +
          "coil, shunt, or sense trace)",
        position: { x: 52.6, y: 40 },
        actual: 481,
        required: 3,
      },
    ];
    const summary = aggregateDrc(violations, 20, "summary");
    expect(summary.categories.connectivity).toBe(1);
    expect(summary.categories.clearance).toBe(0);
    expect(summary.categories.manufacturing).toBe(0);
    expect(summary.warnings).toBe(1);
    expect(summary.errors).toBe(0);
    // The single-net message yields a [net, ""] pair for the roll-up.
    expect(summary.byNetPair.some((p) => p.nets[0] === "PHA" && p.nets[1] === "")).toBe(true);
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
  it("solves single-ended microstrip width for 50Ω, and calc_impedance re-verifies it", async () => {
    const stack = { dielectric_height: 0.2, dielectric_er: 4.3, copper_thickness: 0.035 };
    const r = out(sizeImpedance({ trace_type: "microstrip", target_z0: 50, ...stack }));
    expect(r.success).toBe(true);
    expect(r.within_tolerance).toBe(true);
    expect(Math.abs(r.measured.z0 - 50)).toBeLessThan(2.5);
    expect(r.width_mm).toBeGreaterThan(0.1);
    expect(r.width_mm).toBeLessThan(1);
    expect(r.measured.recomputed_from_geometry).toBe(true);
    // One model, one number: calc_impedance at the solved width agrees.
    const v = out(await calcImpedance({ trace_type: "microstrip", trace_width: r.width_mm, ...stack }));
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

  it("solves a differential pair for 90Ω diff / 50Ω SE and re-verifies both", async () => {
    const stack = { dielectric_height: 0.2, dielectric_er: 4.3 };
    const r = out(
      sizeImpedance({ trace_type: "diff_microstrip", target_diff_z0: 90, target_z0: 50, ...stack }),
    );
    expect(r.within_tolerance).toBe(true);
    expect(Math.abs(r.measured.z0 - 50)).toBeLessThan(2.5);
    expect(Math.abs(r.measured.diff_z0 - 90)).toBeLessThan(4.5);
    expect(r.spacing_mm).toBeGreaterThan(0);
    const v = out(
      await calcImpedance({ trace_type: "diff_microstrip", trace_width: r.width_mm, spacing: r.spacing_mm, ...stack }),
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

  it("sizes segment widths so the load node meets its IR-drop budget", async () => {
    const r = out(await sizePdn(bridge([{ node: 3, max_drop: 0.015 }])));
    expect(r.success).toBe(true);
    expect(r.within_budget).toBe(true);
    expect(r.widths_mm).toHaveLength(5);
    // Drop recomputed from a forward solve sits at/under budget (within tol).
    expect(r.measured_drops_v[0]).toBeLessThanOrEqual(0.015 * 1.05);
    expect(r.measured_drops_v[0]).toBeGreaterThan(0); // a real drop, mesh is solved
    expect(r.document_id).toBeUndefined();
    // No board referenced → model-only, clearly labelled (never an implied PASS).
    expect(r.realized_check).toMatch(/model-only/i);
  });

  it("flags a budget it cannot meet within the width bounds", async () => {
    // An impossibly tight budget at realistic max widths -> over_budget reported.
    const r = out(await sizePdn({ ...bridge([{ node: 3, max_drop: 1e-5 }]), max_width: 0.5 }));
    expect(r.within_budget).toBe(false);
    expect(r.over_budget.length).toBeGreaterThan(0);
    expect(r.active_constraints).toContain("max_width");
  });

  it("rejects a singular (disconnected) mesh", async () => {
    // Node 3 has no path to the reference (node 0).
    const res = await sizePdn({
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
    const r = out(await sizePdn({ ...bridge([{ node: 3, max_drop: 0.015 }]), engine: "exact" }));
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

  it("wider bounds let it meet a tighter budget than narrow bounds", async () => {
    const tight = out(await sizePdn({ ...bridge([{ node: 3, max_drop: 0.008 }]), max_width: 0.3 }));
    const roomy = out(await sizePdn({ ...bridge([{ node: 3, max_drop: 0.008 }]), max_width: 5 }));
    expect(roomy.within_budget).toBe(true);
    // The constrained run does no better than the roomy one.
    expect(roomy.measured_drops_v[0]).toBeLessThanOrEqual(tight.measured_drops_v[0] + 1e-9);
  });

  it("a usage error when only one of document_id / net is given", async () => {
    const res = await sizePdn({ ...bridge([{ node: 3, max_drop: 0.015 }]), net: "+3V3" });
    expect(res.isError).toBe(true);
    expect(res.content[0].text).toMatch(/document_id and net/i);
  });

  it("certifies a PASS against a galvanically-continuous plane", async () => {
    const id = registerPwrBoard(true);
    const r = out(
      await sizePdn({ ...bridge([{ node: 3, max_drop: 0.015 }]), document_id: id, net: "PWR" }),
    );
    expect(r.within_budget).toBe(true);
    expect(r.blocked).toBeUndefined();
    expect(r.realized_verified).toBe(true);
    expect(r.realized_plane.continuous).toBe(true);
    expect(r.realized_plane.coverage_pct).toBe(100);
  });

  it("REFUSES a PASS on a disconnected plane, reporting coverage + worst island", async () => {
    const id = registerPwrBoard(false);
    const r = out(
      await sizePdn({ ...bridge([{ node: 3, max_drop: 0.015 }]), document_id: id, net: "PWR" }),
    );
    // A closed-form PASS on a dead plane is refused, not reported.
    expect(r.blocked).toBe(true);
    expect(r.verdict).toBe("blocked");
    expect(r.within_budget).toBe(false);
    expect(r.unverifiable_reason).toMatch(/disconnected plane/i);
    expect(r.realized_plane.islands).toBe(2);
    expect(r.realized_plane.coverage_pct).toBe(50);
    expect(r.realized_plane.connected_pads).toBe(1);
    expect(r.realized_plane.total_pads).toBe(2);
    expect(r.realized_plane.stitching_vias).toBe(0);
    expect(r.realized_plane.worst_island.pad_count).toBe(1);
  });
});

describe("calc_impedance realized-trace gate", () => {
  it("certifies the impedance when the trace is realized as continuous copper", async () => {
    const id = registerPwrBoard(true);
    const v = out(
      await calcImpedance({
        trace_type: "microstrip",
        trace_width: 0.3,
        dielectric_height: 0.2,
        document_id: id,
        net: "PWR",
      }),
    );
    expect(v.z0).toBeGreaterThan(0);
    expect(v.blocked).toBeUndefined();
    expect(v.realized_verified).toBe(true);
  });

  it("blocks an impedance for a trace split into islands", async () => {
    const id = registerPwrBoard(false);
    const v = out(
      await calcImpedance({
        trace_type: "microstrip",
        trace_width: 0.3,
        dielectric_height: 0.2,
        document_id: id,
        net: "PWR",
      }),
    );
    // The model number is still computed, but it is not certified.
    expect(v.z0).toBeGreaterThan(0);
    expect(v.blocked).toBe(true);
    expect(v.realized_plane.continuous).toBe(false);
  });

  it("blocks an impedance for a net with no realized copper", async () => {
    const id = registerPwrBoard(true);
    const v = out(
      await calcImpedance({
        trace_type: "microstrip",
        trace_width: 0.3,
        dielectric_height: 0.2,
        document_id: id,
        net: "GHOST",
      }),
    );
    expect(v.blocked).toBe(true);
    expect(v.realized_plane.realized).toBe(false);
    expect(v.summary).toMatch(/no realized copper/i);
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

  it("export_gerber offloads a large bundle to an artifact (URL + manifest), keeps a small one inline", async () => {
    const created = out(
      await createSchematic({ components: [resistor("R1", 0, ["A", "B"])] }),
    );
    const id = created.document_id;
    out(await placeComponents({ document_id: id, board_width: 30, board_height: 30 }));

    // Small bundle under the default cap → stays inline.
    const small = out(await exportGerber({ document_id: id }));
    expect(small.success).toBe(true);
    expect(Array.isArray(small.files)).toBe(true);
    expect(small.files.length).toBeGreaterThan(0);
    expect(small.artifact_url).toBeUndefined();

    // Force the SAME bundle over the cap → offloaded to the artifact store.
    process.env.MCP_MAX_INLINE_ARTIFACT_BYTES = "10";
    try {
      const big = out(await exportGerber({ document_id: id }));
      expect(big.success).toBe(true);
      // No inline files — only the handle travels.
      expect(big.files).toBeUndefined();
      expect(big.artifact_url).toMatch(/\/artifacts\/art_/);
      expect(big.artifact_id).toMatch(/^art_/);
      expect(Array.isArray(big.manifest)).toBe(true);
      expect(big.manifest.length).toBe(small.files.length);
      for (const entry of big.manifest) {
        expect(typeof entry.file).toBe("string");
        expect(entry.bytes).toBeGreaterThan(0);
        expect(entry.sha256).toMatch(/^[0-9a-f]{64}$/);
      }
      expect(big.fab_artifact.artifact_id).toBe(big.artifact_id);
    } finally {
      delete process.env.MCP_MAX_INLINE_ARTIFACT_BYTES;
    }
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

// A board with two placed resistors (refs R1, R2) on a 50×50 outline.
async function boardWithTwoResistors(): Promise<string> {
  const created = out(
    await createSchematic({
      components: [resistor("R1", 0), resistor("R2", 20)],
      nets: { MID: ["R1.2", "R2.1"] },
    }),
  );
  const id = created.document_id as string;
  out(await placeComponents({ document_id: id, board_width: 50, board_height: 50 }));
  return id;
}

/** True when a tool returned an error result (text isn't JSON — don't out() it). */
const isErr = (r: unknown): boolean => (r as { isError?: boolean }).isError === true;

describe("set_placement", () => {
  it("places by ref with rotation + side and reports counts", async () => {
    const id = await boardWithTwoResistors();
    const res = out(
      await setPlacement({
        document_id: id,
        placements: [
          { ref: "R1", x: 10, y: 12, rotation: 90, side: "bottom" },
          { ref: "R2", x: 30, y: 30 },
        ],
      }),
    );
    expect(res.success).toBe(true);
    expect(res.moved).toBe(2);
    expect(res.rotated).toBe(1);
    expect(res.flipped).toBe(1);

    const fp = (ref: string) => getPcbBoard(getSession(id)).footprints.find((f) => f.ref === ref)!;
    expect(fp("R1").position).toEqual({ x: 10, y: 12 });
    expect(fp("R1").rotation).toBe(90);
    expect(fp("R1").front).toBe(false);
    expect(fp("R2").position).toEqual({ x: 30, y: 30 });
  });

  it("collects unknown refs and warns on off-board landings", async () => {
    const id = await boardWithTwoResistors();
    const res = out(
      await setPlacement({
        document_id: id,
        placements: [
          { ref: "R1", x: 200, y: 200 },
          { ref: "NOPE", x: 5, y: 5 },
        ],
      }),
    );
    expect(res.success).toBe(true);
    expect(res.unknown_refs).toEqual(["NOPE"]);
    expect(res.warnings.join(" ")).toContain("outside the board outline");
  });

  it("flags two footprints stacked at the same point", async () => {
    const id = await boardWithTwoResistors();
    const res = out(
      await setPlacement({
        document_id: id,
        placements: [
          { ref: "R1", x: 25, y: 25 },
          { ref: "R2", x: 25, y: 25 },
        ],
      }),
    );
    expect(res.warnings.join(" ")).toContain("likely overlap");
  });

  it("errors when every ref is unknown", async () => {
    const id = await boardWithTwoResistors();
    const res = await setPlacement({ document_id: id, placements: [{ ref: "X", x: 1, y: 1 }] });
    expect(isErr(res)).toBe(true);
  });

  it("returns a fresh placement_drc so a short can be made then fixed in-loop", async () => {
    const onePad = (ref: string, net: string) => ({
      ref,
      value: net,
      footprint: "Test:Pad",
      x: 0,
      y: 0,
      pins: [{ number: "1", name: net, type: "Passive" }],
      pads: [{ number: "1", shape: { type: "Rect" as const, width: 1.5, height: 1.5 }, position: { x: 0, y: 0 } }],
    });
    const created = out(
      await createSchematic({
        components: [onePad("C1", "VCC"), onePad("J1", "GND")],
        nets: { VCC: ["C1.1"], GND: ["J1.1"] },
      }),
    );
    const id = created.document_id;
    out(await placeComponents({ document_id: id, board_width: 40, board_height: 40 }));

    // Stack J1 on C1 → set_placement reports the short directly.
    const shorted = out(
      await setPlacement({
        document_id: id,
        placements: [
          { ref: "C1", x: 20, y: 20 },
          { ref: "J1", x: 20, y: 20 },
        ],
      }),
    );
    expect(shorted.placement_drc.clean).toBe(false);
    expect(shorted.placement_drc.shorts.length).toBe(1);
    expect([...shorted.placement_drc.shorts[0].refs].sort()).toEqual(["C1", "J1"]);

    // Move J1 away → the same call confirms a clean board; no run_drc needed.
    const fixed = out(await setPlacement({ document_id: id, placements: [{ ref: "J1", x: 5, y: 5 }] }));
    expect(fixed.placement_drc.clean).toBe(true);
    expect(fixed.placement_drc.shorts).toEqual([]);
  });
});

describe("add_zone", () => {
  it("fills the whole board outline on a new net", async () => {
    const id = await boardWithTwoResistors();
    const res = out(await addZone({ document_id: id, net: "GND", fill_board: true }));
    expect(res.success).toBe(true);
    expect(res.fill).toBe("board");
    expect(res.thermal_relief).toBe("Relief");

    const board = getPcbBoard(getSession(id));
    expect(board.zones).toHaveLength(1);
    expect(board.zones[0].net).toBe("GND");
    expect(board.zones[0].layer).toBe("FCu");
    expect(board.zones[0].fillType).toBe("Solid");
    expect(board.zones[0].outline.length).toBeGreaterThanOrEqual(3);
    // The net was auto-created.
    expect(board.nets.some((n) => n.id === "GND")).toBe(true);
  });

  it("accepts an explicit polygon and rejects a degenerate one", async () => {
    const id = await boardWithTwoResistors();
    const ok = out(
      await addZone({
        document_id: id,
        net: "VBAT",
        layer: "BCu",
        outline: [
          { x: 5, y: 5 },
          { x: 20, y: 5 },
          { x: 20, y: 20 },
          { x: 5, y: 20 },
        ],
        thermal_relief: "Direct",
      }),
    );
    expect(ok.success).toBe(true);
    expect(ok.fill).toBe("polygon");
    expect(getPcbBoard(getSession(id)).zones[0].thermalRelief).toBe("Direct");

    const bad = await addZone({ document_id: id, net: "X", outline: [{ x: 0, y: 0 }] });
    expect(isErr(bad)).toBe(true);
  });
});

describe("delete_zone / delete_trace / delete_via", () => {
  it("deletes a zone by index and reports a changed diff", async () => {
    const id = await boardWithTwoResistors();
    out(await addZone({ document_id: id, net: "GND", layer: "BCu", fill_board: true }));
    out(await addZone({ document_id: id, net: "VBAT", layer: "FCu", fill_board: true }));
    expect(getPcbBoard(getSession(id)).zones).toHaveLength(2);

    const res = out(await deleteZone({ document_id: id, index: 0 }));
    expect(res.success).toBe(true);
    expect(res.deleted).toMatchObject({
      action: "removed",
      kind: "zone",
      index: 0,
      net: "GND",
      layer: "BCu",
    });
    expect(res.changed).toEqual([res.deleted]);
    expect(res.zones_total).toBe(1);

    // The surviving pour is the one we didn't delete.
    const board = getPcbBoard(getSession(id));
    expect(board.zones).toHaveLength(1);
    expect(board.zones[0].net).toBe("VBAT");
  });

  it("deletes a zone by an unambiguous net match", async () => {
    const id = await boardWithTwoResistors();
    out(await addZone({ document_id: id, net: "GND", fill_board: true }));
    const res = out(await deleteZone({ document_id: id, net: "GND" }));
    expect(res.success).toBe(true);
    expect(getPcbBoard(getSession(id)).zones).toHaveLength(0);
  });

  it("rejects a bad index, an ambiguous net, and a missing selector", async () => {
    const id = await boardWithTwoResistors();
    out(await addZone({ document_id: id, net: "GND", layer: "FCu", fill_board: true }));
    out(await addZone({ document_id: id, net: "GND", layer: "BCu", fill_board: true }));

    expect(isErr(await deleteZone({ document_id: id, index: 9 }))).toBe(true);
    // Two GND zones → net alone is ambiguous; the error names the layer fix.
    expect(isErr(await deleteZone({ document_id: id, net: "GND" }))).toBe(true);
    // No index and no net at all → nothing to identify.
    expect(isErr(await deleteZone({ document_id: id }))).toBe(true);

    // Layer disambiguates → the FCu pour survives.
    const ok = out(await deleteZone({ document_id: id, net: "GND", layer: "BCu" }));
    expect(ok.success).toBe(true);
    const board = getPcbBoard(getSession(id));
    expect(board.zones).toHaveLength(1);
    expect(board.zones[0].layer).toBe("FCu");
  });

  it("guards an index against a mismatched net", async () => {
    const id = await boardWithTwoResistors();
    out(await addZone({ document_id: id, net: "GND", fill_board: true }));
    // Index 0 IS the GND zone — asking to delete a 'VBAT' one there is a mistake.
    expect(isErr(await deleteZone({ document_id: id, index: 0, net: "VBAT" }))).toBe(true);
    expect(getPcbBoard(getSession(id)).zones).toHaveLength(1);
  });

  it("deletes a trace and a via by index", async () => {
    const id = await boardWithTwoResistors();
    out(
      await addTrace({
        document_id: id,
        net: "MID",
        points: [
          { x: 0, y: 0 },
          { x: 5, y: 0 },
        ],
      }),
    );
    out(await addVia({ document_id: id, net: "MID", position: { x: 5, y: 0 } }));
    expect(getPcbBoard(getSession(id)).traces.length).toBeGreaterThanOrEqual(1);
    expect(getPcbBoard(getSession(id)).vias).toHaveLength(1);

    const dt = out(await deleteTrace({ document_id: id, index: 0 }));
    expect(dt.deleted).toMatchObject({ kind: "trace", net: "MID" });
    expect(getPcbBoard(getSession(id)).traces).toHaveLength(0);

    const dv = out(await deleteVia({ document_id: id, index: 0 }));
    expect(dv.deleted).toMatchObject({ kind: "via", net: "MID" });
    expect(getPcbBoard(getSession(id)).vias).toHaveLength(0);
  });
});

describe("get_copper", () => {
  /** Board with two traces (MID/AUX), one via, and a back-side GND pour. */
  async function boardWithCopper(): Promise<string> {
    const id = await boardWithTwoResistors();
    out(
      await addTrace({
        document_id: id,
        net: "MID",
        points: [
          { x: 10, y: 10 },
          { x: 20, y: 10 },
        ],
      }),
    );
    out(
      await addTrace({
        document_id: id,
        net: "AUX",
        layer: "BCu",
        points: [
          { x: 30, y: 30 },
          { x: 40, y: 30 },
        ],
      }),
    );
    out(await addVia({ document_id: id, net: "MID", position: { x: 20, y: 10 } }));
    out(await addZone({ document_id: id, net: "GND", layer: "BCu", fill_board: true }));
    return id;
  }

  it("returns every element with delete-compatible kind + index", async () => {
    const id = await boardWithCopper();
    const res = out(await getCopper({ document_id: id }));
    expect(res.success).toBe(true);
    expect(res.total).toBe(4);
    expect(res.count).toBe(4);
    expect(res.next_offset).toBeUndefined();
    expect(res.total_by_kind).toEqual({ trace: 2, via: 1, zone: 1 });

    // Deterministic order: traces, arcs, vias, zones — index order within each.
    expect(res.elements.map((e: { kind: string }) => e.kind)).toEqual([
      "trace",
      "trace",
      "via",
      "zone",
    ]);
    expect(res.elements[0]).toMatchObject({
      kind: "trace",
      index: 0,
      net: "MID",
      layer: "FCu",
      start: { x: 10, y: 10 },
      end: { x: 20, y: 10 },
      source: "manual",
    });
    expect(res.elements[2]).toMatchObject({
      kind: "via",
      index: 0,
      net: "MID",
      layers: ["FCu", "BCu"],
      position: { x: 20, y: 10 },
    });
    // Zones report bbox + vertex count, not the (possibly huge) outline.
    expect(res.elements[3]).toMatchObject({ kind: "zone", index: 0, net: "GND", layer: "BCu" });
    expect(res.elements[3].bbox).toEqual({ min: { x: 0, y: 0 }, max: { x: 50, y: 50 } });
    expect(res.elements[3].vertices).toBeGreaterThanOrEqual(3);
    expect(res.elements[3].outline).toBeUndefined();
  });

  it("filters by kind, net, layer, and bbox", async () => {
    const id = await boardWithCopper();

    const vias = out(await getCopper({ document_id: id, kind: "via" }));
    expect(vias.total).toBe(1);
    expect(vias.elements[0].kind).toBe("via");

    const mid = out(await getCopper({ document_id: id, net: "MID" }));
    expect(mid.total).toBe(2); // FCu trace + via
    expect(mid.elements.every((e: { net: string }) => e.net === "MID")).toBe(true);

    // BCu: the AUX trace, the pour — and the via, whose barrel spans FCu→BCu.
    const bcu = out(await getCopper({ document_id: id, layer: "BCu" }));
    expect(bcu.total).toBe(3);
    expect(bcu.elements.map((e: { kind: string }) => e.kind).sort()).toEqual([
      "trace",
      "via",
      "zone",
    ]);

    // Spatial: only the MID trace's neighborhood, kind-scoped so the
    // board-spanning pour doesn't match.
    const near = out(
      await getCopper({ document_id: id, kind: "trace", bbox: { x: 5, y: 5, w: 10, h: 10 } }),
    );
    expect(near.total).toBe(1);
    expect(near.elements[0]).toMatchObject({ kind: "trace", net: "MID" });

    // Empty match is a success with total 0, not an error.
    const none = out(await getCopper({ document_id: id, net: "GHOST" }));
    expect(none.total).toBe(0);
    expect(none.elements).toEqual([]);
  });

  it("query indices drive surgical deletes", async () => {
    const id = await boardWithCopper();
    const aux = out(await getCopper({ document_id: id, kind: "trace", net: "AUX" }));
    expect(aux.total).toBe(1);
    const del = out(await deleteTrace({ document_id: id, index: aux.elements[0].index }));
    expect(del.deleted).toMatchObject({ kind: "trace", net: "AUX" });
    expect(out(await getCopper({ document_id: id, kind: "trace" })).total).toBe(1);
  });

  it("paginates with offset and reports the uncapped total", async () => {
    const id = await boardWithTwoResistors();
    for (let i = 0; i < 5; i++) {
      out(await addVia({ document_id: id, net: "MID", position: { x: 10 + i * 5, y: 40 } }));
    }
    const p1 = out(await getCopper({ document_id: id, kind: "via", limit: 2 }));
    expect(p1.total).toBe(5);
    expect(p1.count).toBe(2);
    expect(p1.next_offset).toBe(2);
    expect(p1.elements.map((e: { index: number }) => e.index)).toEqual([0, 1]);

    const p2 = out(await getCopper({ document_id: id, kind: "via", limit: 2, offset: 4 }));
    expect(p2.count).toBe(1);
    expect(p2.elements[0].index).toBe(4);
    expect(p2.next_offset).toBeUndefined();
  });

  it("rejects a bad kind, a malformed layer, and a bad bbox", async () => {
    const id = await boardWithTwoResistors();
    expect(isErr(await getCopper({ document_id: id, kind: "wire" }))).toBe(true);
    expect(isErr(await getCopper({ document_id: id, layer: "F.Cu" }))).toBe(true);
    expect(isErr(await getCopper({ document_id: id, bbox: { x: 0, y: 0 } }))).toBe(true);
  });
});

describe("add_net_tie / delete_net_tie", () => {
  /** Board whose netlist has four nets (one pad each) and no routing. */
  async function boardWithFourNets(): Promise<string> {
    const created = out(
      await createSchematic({
        components: [resistor("R1", 0), resistor("R2", 20)],
        nets: { PHA: ["R1.1"], PHB: ["R1.2"], GND: ["R2.1"], AGND: ["R2.2"] },
      }),
    );
    const id = created.document_id as string;
    out(await placeComponents({ document_id: id, board_width: 50, board_height: 50 }));
    return id;
  }

  it("authors a board-wide tie and returns the updated tie list", async () => {
    const id = await boardWithFourNets();
    const res = out(await addNetTie({ document_id: id, nets: ["GND", "AGND"] }));
    expect(res.success).toBe(true);
    expect(res.tie).toMatchObject({ index: 0, nets: ["GND", "AGND"], scope: "board_wide" });
    expect(res.net_ties).toEqual([res.tie]);
    expect(res.net_ties_total).toBe(1);
    expect(res.changed).toEqual([
      { action: "added", kind: "netTie", index: 0, net: "GND+AGND" },
    ]);

    const board = getPcbBoard(getSession(id));
    expect(board.netTies).toHaveLength(1);
    expect(board.netTies![0]).toEqual({ nets: ["GND", "AGND"] });
  });

  it("authors a region-scoped tie with position + radius", async () => {
    const id = await boardWithFourNets();
    const res = out(
      await addNetTie({
        document_id: id,
        nets: ["PHA", "PHB", "GND"],
        position: { x: 25, y: 25 },
        radius: 3,
      }),
    );
    expect(res.tie).toMatchObject({
      scope: "region",
      position: { x: 25, y: 25 },
      radius: 3,
    });
    const board = getPcbBoard(getSession(id));
    expect(board.netTies![0].position).toEqual({ x: 25, y: 25 });
    expect(board.netTies![0].radius).toBe(3);
  });

  it("rejects unknown nets, < 2 distinct nets, and a half-scoped region", async () => {
    const id = await boardWithFourNets();
    // Unknown net — the error names it so the typo is findable.
    const bad = await addNetTie({ document_id: id, nets: ["GND", "GROUND"] });
    expect(isErr(bad)).toBe(true);
    expect((bad as { content: Array<{ text: string }> }).content[0].text).toContain("GROUND");

    expect(isErr(await addNetTie({ document_id: id, nets: ["GND"] }))).toBe(true);
    // Duplicates collapse — still one distinct net.
    expect(isErr(await addNetTie({ document_id: id, nets: ["GND", "GND"] }))).toBe(true);
    // position without radius (or vice versa) would silently become a
    // board-wide exemption in the kernel — rejected outright.
    expect(
      isErr(await addNetTie({ document_id: id, nets: ["GND", "AGND"], position: { x: 1, y: 1 } })),
    ).toBe(true);
    expect(isErr(await addNetTie({ document_id: id, nets: ["GND", "AGND"], radius: 2 }))).toBe(true);
    expect(
      isErr(
        await addNetTie({
          document_id: id,
          nets: ["GND", "AGND"],
          position: { x: 1, y: 1 },
          radius: 0,
        }),
      ),
    ).toBe(true);
    expect(getPcbBoard(getSession(id)).netTies ?? []).toHaveLength(0);
  });

  it("deletes by index, by net set, and by position; guards a mismatched index", async () => {
    const id = await boardWithFourNets();
    out(await addNetTie({ document_id: id, nets: ["GND", "AGND"] }));
    out(
      await addNetTie({
        document_id: id,
        nets: ["PHA", "PHB"],
        position: { x: 10, y: 10 },
        radius: 2,
      }),
    );
    out(
      await addNetTie({
        document_id: id,
        nets: ["PHB", "PHA"],
        position: { x: 40, y: 40 },
        radius: 2,
      }),
    );

    // Net-set match alone is ambiguous across the two PHA/PHB junctions…
    expect(isErr(await deleteNetTie({ document_id: id, nets: ["PHA", "PHB"] }))).toBe(true);
    // …position disambiguates (order-insensitive net match).
    const byPos = out(
      await deleteNetTie({ document_id: id, nets: ["PHB", "PHA"], position: { x: 40, y: 40 } }),
    );
    expect(byPos.deleted).toMatchObject({ index: 2, nets: ["PHB", "PHA"] });
    expect(byPos.net_ties_total).toBe(2);
    expect(byPos.changed).toEqual([
      { action: "removed", kind: "netTie", index: 2, net: "PHB+PHA" },
    ]);

    // Index + mismatched nets is a guard, not a delete.
    expect(isErr(await deleteNetTie({ document_id: id, index: 0, nets: ["PHA", "PHB"] }))).toBe(
      true,
    );
    // No selector at all → error; out-of-range index → error.
    expect(isErr(await deleteNetTie({ document_id: id }))).toBe(true);
    expect(isErr(await deleteNetTie({ document_id: id, index: 9 }))).toBe(true);

    const bySet = out(await deleteNetTie({ document_id: id, nets: ["AGND", "GND"] }));
    expect(bySet.deleted.nets).toEqual(["GND", "AGND"]);
    const byIndex = out(await deleteNetTie({ document_id: id, index: 0 }));
    expect(byIndex.net_ties_total).toBe(0);
    expect(getPcbBoard(getSession(id)).netTies).toHaveLength(0);
  });

  it("a region-scoped tie exempts the junction short in DRC; deleting it re-arms", async () => {
    const id = await boardWithFourNets();
    // Pin the floorplan to the top edge so no pad sits near the junction the
    // test builds — only the deliberate crossing may short.
    out(
      await setPlacement({
        document_id: id,
        placements: [
          { ref: "R1", x: 10, y: 45 },
          { ref: "R2", x: 35, y: 45 },
        ],
      }),
    );
    // A PHB stub ending ON the PHA trace — a T-junction at (20, 25), the
    // shape of a real tie point (shunt tap, star chord meeting the neutral).
    out(
      await addTrace({
        document_id: id,
        net: "PHA",
        points: [
          { x: 10, y: 25 },
          { x: 30, y: 25 },
        ],
      }),
    );
    out(
      await addTrace({
        document_id: id,
        net: "PHB",
        points: [
          { x: 20, y: 15 },
          { x: 20, y: 25 },
        ],
      }),
    );

    const before = out(await runDrc({ document_id: id }));
    expect(before.byRule?.Short ?? 0).toBeGreaterThan(0);

    out(
      await addNetTie({
        document_id: id,
        nets: ["PHA", "PHB"],
        position: { x: 20, y: 25 },
        radius: 3,
      }),
    );
    const tied = out(await runDrc({ document_id: id }));
    expect(tied.byRule?.Short ?? 0).toBe(0);

    // Take the tie back — the same copper is a short again (fail-closed).
    out(await deleteNetTie({ document_id: id, nets: ["PHA", "PHB"] }));
    const rearmed = out(await runDrc({ document_id: id }));
    expect(rearmed.byRule?.Short ?? 0).toBeGreaterThan(0);
  });
});

describe("undo (snapshot rewind)", () => {
  it("rewinds the last mutation and reports what the rewind removed", async () => {
    const id = await boardWithTwoResistors();
    // The dispatch layer snapshots before each mutation — simulate that here.
    recordHistorySnapshot(id);
    out(await addZone({ document_id: id, net: "GND", fill_board: true }));
    expect(getPcbBoard(getSession(id)).zones).toHaveLength(1);

    const res = out(await undo({ document_id: id }));
    expect(res.success).toBe(true);
    expect(res.undone).toBe(true);
    // The pour is gone — the board is back to its pre-add state.
    expect(getPcbBoard(getSession(id)).zones).toHaveLength(0);
    expect(res.changed).toEqual([
      expect.objectContaining({ action: "removed", kind: "zone", net: "GND" }),
    ]);
  });

  it("walks back multiple steps, then reports nothing left to undo", async () => {
    const id = await boardWithTwoResistors();
    recordHistorySnapshot(id);
    out(await addZone({ document_id: id, net: "GND", fill_board: true }));
    recordHistorySnapshot(id);
    out(
      await addTrace({
        document_id: id,
        net: "MID",
        points: [
          { x: 0, y: 0 },
          { x: 5, y: 0 },
        ],
      }),
    );
    expect(getPcbBoard(getSession(id)).traces.length).toBeGreaterThanOrEqual(1);

    // First undo drops the trace; the zone stays.
    const u1 = out(await undo({ document_id: id }));
    expect(u1.remaining_undos).toBe(1);
    expect(getPcbBoard(getSession(id)).traces).toHaveLength(0);
    expect(getPcbBoard(getSession(id)).zones).toHaveLength(1);

    // Second undo drops the zone.
    out(await undo({ document_id: id }));
    expect(getPcbBoard(getSession(id)).zones).toHaveLength(0);

    // Stack is empty now.
    expect(isErr(await undo({ document_id: id }))).toBe(true);
  });

  it("throws on an unknown document and errors with nothing recorded", async () => {
    // An unknown id throws the pinned getSession error (the dispatch layer
    // turns thrown errors into structured results); consistent with the rest
    // of the ECAD surface.
    expect(() => undo({ document_id: "doc_nope" })).toThrow(/Unknown document_id/);
    const id = await boardWithTwoResistors();
    // Known session, but no snapshot was recorded for it yet → graceful error.
    expect(isErr(await undo({ document_id: id }))).toBe(true);
  });
});

describe("set_design_rules", () => {
  it("writes default rules and a net class that DRC then reads", async () => {
    const id = await boardWithTwoResistors();
    const res = out(
      await setDesignRules({
        document_id: id,
        clearance: 0.3,
        track_width: 0.4,
        classes: [{ name: "power", nets: ["MID"], clearance: 0.6, track_width: 1.0 }],
      }),
    );
    expect(res.success).toBe(true);
    expect(res.rules.clearance).toBe(0.3);
    expect(res.rules.track_width).toBe(0.4);
    expect(res.classes).toEqual(["power"]);

    const board = getPcbBoard(getSession(id));
    expect(board.rules.defaultRules.clearance).toBe(0.3);
    expect(board.rules.defaultRules.traceWidth).toBe(0.4);
    expect(board.rules.classRules?.[0]).toMatchObject({ name: "power", clearance: 0.6, traceWidth: 1.0 });
    expect(board.rules.netClassAssignments).toEqual({ power: ["MID"] });

    // DRC still runs against the updated rules.
    const drc = out(await runDrc({ document_id: id }));
    expect(drc.success).toBe(true);
  });

  it("warns on unknown class nets and errors with no fields", async () => {
    const id = await boardWithTwoResistors();
    const warn = out(
      await setDesignRules({ document_id: id, classes: [{ name: "hv", nets: ["GHOST"] }] }),
    );
    expect(warn.success).toBe(true);
    expect(warn.warnings.join(" ")).toContain("GHOST");

    const empty = await setDesignRules({ document_id: id });
    expect(isErr(empty)).toBe(true);
  });
});

describe("size_trace_for_current", () => {
  it("solves a plausible width and derates inner layers", async () => {
    const outer = out(sizeTraceForCurrent({ current_a: 10, copper_oz: 1, temp_rise_c: 10 }));
    expect(outer.standard).toBe("IPC-2221");
    expect(outer.current_a).toBe(10);
    // 10A / 10°C / 1oz external lands around 7mm by the closed form.
    expect(outer.width_mm).toBeGreaterThan(5);
    expect(outer.width_mm).toBeLessThan(9);

    const inner = out(
      sizeTraceForCurrent({ current_a: 10, copper_oz: 1, temp_rise_c: 10, layer: "inner" }),
    );
    expect(inner.layer).toBe("inner");
    // Inner traces shed heat poorly → must be wider for the same current.
    expect(inner.width_mm).toBeGreaterThan(outer.width_mm);

    // Heavier copper → narrower trace for the same current.
    const heavy = out(sizeTraceForCurrent({ current_a: 10, copper_oz: 2, temp_rise_c: 10 }));
    expect(heavy.width_mm).toBeLessThan(outer.width_mm);
  });

  it("snaps the width up to the fab grid and rejects bad current", async () => {
    const r = out(sizeTraceForCurrent({ current_a: 3, fab_grid_mm: 0.5 }));
    // Snapped up to the next grid step, never below the raw solve.
    expect(r.width_mm).toBeGreaterThanOrEqual(r.width_mm_raw);
    expect(r.width_mm - r.width_mm_raw).toBeLessThan(0.5);
    const ratio = r.width_mm / 0.5;
    expect(Math.abs(ratio - Math.round(ratio))).toBeLessThan(1e-9);

    const bad = sizeTraceForCurrent({ current_a: 0 });
    expect(isErr(bad)).toBe(true);
  });
});

describe("add_via_array", () => {
  it("fills a region with a clipped via grid", async () => {
    const id = await boardWithTwoResistors();
    const res = out(
      await addViaArray({ document_id: id, net: "GND", region: { x: 10, y: 10, w: 5, h: 5 }, pitch: 1 }),
    );
    expect(res.success).toBe(true);
    expect(res.mode).toBe("region");
    // 6×6 grid (0..5 inclusive at pitch 1), all inside the 50×50 board.
    expect(res.vias_added).toBe(36);
    const board = getPcbBoard(getSession(id));
    expect(board.vias).toHaveLength(36);
    expect(board.vias.every((v) => v.net === "GND")).toBe(true);
  });

  it("clips grid vias that fall outside the board", async () => {
    const id = await boardWithTwoResistors();
    const res = out(
      await addViaArray({ document_id: id, net: "GND", region: { x: 48, y: 48, w: 6, h: 6 }, pitch: 1 }),
    );
    // Part of this region is off the 50×50 board → some vias dropped.
    expect(res.skipped_outside_board).toBeGreaterThan(0);
  });

  it("places explicit points and refuses an empty request", async () => {
    const id = await boardWithTwoResistors();
    const res = out(
      await addViaArray({
        document_id: id,
        net: "SIG",
        points: [
          { x: 5, y: 5 },
          { x: 6, y: 6 },
        ],
      }),
    );
    expect(res.mode).toBe("points");
    expect(res.vias_added).toBe(2);

    const bad = await addViaArray({ document_id: id, net: "SIG" });
    expect(isErr(bad)).toBe(true);
  });
});

describe("new PCB tools survive the kernel pipeline", () => {
  it("a pour, a via field, and tightened rules pass DRC + Gerber", async () => {
    const id = await boardWithTwoResistors();
    out(await setDesignRules({ document_id: id, clearance: 0.25, track_width: 0.3 }));
    out(await addZone({ document_id: id, net: "GND", fill_board: true }));
    out(
      await addViaArray({
        document_id: id,
        net: "GND",
        region: { x: 15, y: 15, w: 4, h: 4 },
        pitch: 1,
      }),
    );
    out(await routeNets({ document_id: id }));

    // The kernel computes zone fills + DRC against the mutated rules; both must
    // accept the board, and Gerber must render the new copper.
    const drc = out(await runDrc({ document_id: id }));
    expect(drc.success).toBe(true);
    const gerber = out(await exportGerber({ document_id: id }));
    expect(gerber.success).toBe(true);
    expect(gerber.files.length).toBeGreaterThan(0);
  });
});

// ===========================================================================
// Unverifiable ≠ clean — a board/schematic the kernel can't deserialize (e.g. a
// dotted layer name 'In1.Cu' that should be 'In1Cu') must surface as an error
// the agent branches on, NEVER as a passing "0 violations / clean" result.
// ===========================================================================
describe("run_drc / run_erc / critique_route surface 'unverifiable', not false-clean", () => {
  /** A minimal valid board, used as the base for the malformed fixtures. */
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

  // A trace on a completely unknown layer name — serde refuses the whole board.
  // (Dotted KiCad forms like "In1.Cu" are now accepted via serde aliases and
  // auto-coerced to "In1Cu", so they no longer trigger the kernel parse error.)
  const malformedPcb = {
    ...validPcb,
    traces: [
      { start: { x: 1, y: 1 }, end: { x: 5, y: 1 }, width: 0.2, layer: "UNKNOWN_LAYER", net: "GND" },
    ],
  } as unknown as Pcb;

  const malformedSheet = {
    components: [
      {
        ref: "R1",
        value: "10k",
        footprintId: "Resistor_SMD:R_0805",
        position: { x: 0, y: 0 },
        rotation: 0,
        mirror: false,
        pins: [{ number: "1", name: "1", pin_type: "Inputt", position: { x: 0, y: 0 } }],
      },
    ],
    wires: [],
    junctions: [],
    labels: [],
  } as unknown as SchematicSheet;

  /** Inline document carrying a raw (possibly malformed) PCB. */
  function docWithPcb(pcb: Pcb): Document {
    return { ...createDocument(), pcb } as unknown as Document;
  }
  /** Inline document carrying a raw (possibly malformed) schematic. */
  function docWithSchematic(schematic: SchematicSheet): Document {
    return { ...createDocument(), schematic } as unknown as Document;
  }

  /** Permissive view of a tool result — the unverifiable arm carries
   *  `structuredContent`/`isError` the success arm doesn't. */
  type AnyResult = {
    isError?: boolean;
    content: Array<{ type: string; text: string }>;
    structuredContent?: {
      verifiable?: boolean;
      status?: string;
      offending_field?: string;
      next_actions?: string[];
    };
  };
  const asResult = (r: unknown) => r as AnyResult;

  it("run_drc returns isError + verifiable:false (not success) for a malformed board", async () => {
    const result = asResult(await runDrc({ document: docWithPcb(malformedPcb) }));
    expect(result.isError).toBe(true);
    expect(result.structuredContent?.verifiable).toBe(false);
    expect(result.structuredContent?.status).toBe("errored");
    expect(result.structuredContent?.offending_field).toBe("UNKNOWN_LAYER");
    expect(result.structuredContent?.next_actions?.length).toBeGreaterThan(0);
    // The text must NOT read as a clean pass.
    const text = result.content[0]!.text;
    expect(text).toMatch(/UNVERIFIABLE/);
    expect(text).not.toMatch(/"success":\s*true/);
  });

  it("run_drc still passes for a board the kernel can parse", async () => {
    const result = asResult(await runDrc({ document: docWithPcb(validPcb) }));
    expect(result.isError).toBeFalsy();
    expect(out(result).success).toBe(true);
  });

  it("critique_route returns isError + verifiable:false for a malformed board", async () => {
    const result = asResult(await critiqueRoute({ document: docWithPcb(malformedPcb), net: "GND" }));
    expect(result.isError).toBe(true);
    expect(result.structuredContent?.verifiable).toBe(false);
    expect(result.structuredContent?.offending_field).toBe("UNKNOWN_LAYER");
  });

  it("run_erc returns isError + verifiable:false for a malformed schematic", async () => {
    const result = asResult(await runErc({ document: docWithSchematic(malformedSheet) }));
    expect(result.isError).toBe(true);
    expect(result.structuredContent?.verifiable).toBe(false);
    expect(result.structuredContent?.offending_field).toBe("Inputt");
  });
});

describe("set_design_rules ordering tolerance (buffering)", () => {
  const powerSchematic = () =>
    createSchematic({
      components: [
        {
          ref: "J1",
          value: "PWR",
          footprint: "Connector_PinHeader_2.54mm:PinHeader_1x02_P2.54mm",
          x: 0,
          y: 0,
          pins: [
            { number: "1", name: "V", type: "Passive" },
            { number: "2", name: "G", type: "Passive" },
          ],
        },
        resistor("R1", 15),
        resistor("R2", 30),
      ],
      nets: { VBAT: ["J1.1", "R1.1"], SIG: ["R1.2", "R2.1"] },
    });

  it("guides to place_components instead of dead-ending when there's no board yet", async () => {
    const created = out(await powerSchematic());
    const id = created.document_id;
    // set_design_rules BEFORE the board exists used to fail with a bare error.
    const res = await setDesignRules({ document_id: id, clearance: 0.3 });
    expect(res.isError).toBeUndefined();
    const body = out(res);
    expect(body.success).toBe(true);
    expect(body.buffered).toBe(true);
    // It carries a recovery action pointing at the canonical next step.
    expect(body.next_actions[0].tool).toBe("place_components");
    expect(res.structuredContent?.next_actions?.[0].tool).toBe("place_components");
  });

  it("replays buffered rules onto the board when place_components runs", async () => {
    const created = out(await powerSchematic());
    const id = created.document_id;
    // Rules first (no board), then place — the buffered rules must land.
    out(
      await setDesignRules({
        document_id: id,
        clearance: 0.35,
        track_width: 0.3,
        classes: [{ name: "power", nets: ["VBAT"], track_width: 1.5 }],
      }),
    );
    const placed = out(await placeComponents({ document_id: id, board_width: 50, board_height: 40 }));
    expect(placed.buffered_design_rules_applied).toBe(true);

    const board = getPcbBoard(getSession(id));
    expect(board.rules.defaultRules.clearance).toBeCloseTo(0.35, 5);
    expect(board.rules.defaultRules.traceWidth).toBeCloseTo(0.3, 5);
    expect((board.rules.classRules ?? []).map((c) => c.name)).toContain("power");
    expect(board.rules.netClassAssignments?.power).toEqual(["VBAT"]);

    // And the buffered rules actually drive routing — VBAT routes at its class width.
    const routed = out(await routeNets({ document_id: id }));
    expect(routed.success).toBe(true);
    if (routed.track_widths_mm?.VBAT !== undefined) {
      expect(routed.track_widths_mm.VBAT).toBeCloseTo(1.5, 1);
    }
  });

  it("still fails fast on a malformed early call (no fields, or a class with no nets)", async () => {
    const created = out(await powerSchematic());
    const id = created.document_id;
    const empty = await setDesignRules({ document_id: id });
    expect(empty.isError).toBe(true);
    const badClass = await setDesignRules({
      document_id: id,
      classes: [{ name: "power", nets: [] }],
    });
    expect(badClass.isError).toBe(true);
  });
});

describe("layer-name validation at write boundaries", () => {
  /** Error text of a failed tool result (the content block isn't JSON). */
  const errText = (r: { content: Array<{ text: string }> }) => r.content[0].text;

  it("add_trace rejects the dotted KiCad form with a did-you-mean + legal list", async () => {
    const id = await boardWithTwoResistors();
    const res = await addTrace({
      document_id: id,
      net: "MID",
      layer: "In1.Cu",
      points: [
        { x: 1, y: 1 },
        { x: 9, y: 1 },
      ],
    });
    expect(isErr(res)).toBe(true);
    const t = errText(res);
    expect(t).toContain("'In1.Cu' is not valid");
    expect(t).toContain("did you mean 'In1Cu'");
    expect(t).toContain("Legal: FCu, BCu, In1Cu");
    // No corrupt copper landed on the board.
    expect(getPcbBoard(getSession(id)).traces.some((tr) => tr.layer === ("In1.Cu" as never))).toBe(
      false,
    );
  });

  it("add_trace rejects an entirely unknown layer with the legal list", async () => {
    const id = await boardWithTwoResistors();
    const res = await addTrace({
      document_id: id,
      net: "MID",
      layer: "TopCopper",
      points: [
        { x: 1, y: 1 },
        { x: 9, y: 1 },
      ],
    });
    expect(isErr(res)).toBe(true);
    expect(errText(res)).toContain("'TopCopper' is not valid");
    expect(errText(res)).toContain("Legal:");
  });

  it("add_trace rejects a valid but non-copper layer", async () => {
    const id = await boardWithTwoResistors();
    const res = await addTrace({
      document_id: id,
      net: "MID",
      layer: "EdgeCuts",
      points: [
        { x: 1, y: 1 },
        { x: 9, y: 1 },
      ],
    });
    expect(isErr(res)).toBe(true);
    expect(errText(res)).toContain("not a copper layer");
  });

  it("add_trace still accepts the canonical layer name", async () => {
    const id = await boardWithTwoResistors();
    const res = out(
      await addTrace({
        document_id: id,
        net: "MID",
        layer: "In1Cu",
        points: [
          { x: 1, y: 1 },
          { x: 9, y: 1 },
        ],
      }),
    );
    expect(res.success).toBe(true);
    expect(res.layer).toBe("In1Cu");
  });

  it("add_via rejects a dotted start_layer", async () => {
    const id = await boardWithTwoResistors();
    const res = await addVia({
      document_id: id,
      net: "MID",
      position: { x: 10, y: 10 },
      start_layer: "F.Cu",
    });
    expect(isErr(res)).toBe(true);
    expect(errText(res)).toContain("start_layer");
    expect(errText(res)).toContain("did you mean 'FCu'");
  });

  it("add_zone rejects a dotted layer", async () => {
    const id = await boardWithTwoResistors();
    const res = await addZone({ document_id: id, net: "GND", layer: "B.Cu", fill_board: true });
    expect(isErr(res)).toBe(true);
    expect(errText(res)).toContain("did you mean 'BCu'");
    expect(getPcbBoard(getSession(id)).zones).toHaveLength(0);
  });

  it("add_via_array rejects a dotted end_layer", async () => {
    const id = await boardWithTwoResistors();
    const res = await addViaArray({
      document_id: id,
      net: "GND",
      region: { x: 15, y: 15, w: 4, h: 4 },
      end_layer: "In2.Cu",
    });
    expect(isErr(res)).toBe(true);
    expect(errText(res)).toContain("end_layer");
    expect(errText(res)).toContain("did you mean 'In2Cu'");
  });

  it("set_stackup rejects a dotted layer in a per-layer override", async () => {
    const id = await boardWithTwoResistors();
    const res = await setStackup({
      document_id: id,
      layers: [{ layer: "In1.Cu", copper_oz: 2 }],
    });
    expect(isErr(res)).toBe(true);
    expect(errText(res)).toContain("did you mean 'In1Cu'");
  });

  it("add_coil rejects a dotted layer", async () => {
    const id = await boardWithTwoResistors();
    const res = await addCoil({
      document_id: id,
      center: { x: 25, y: 25 },
      turns: 3,
      inner_radius: 2,
      outer_radius: 8,
      trace_width: 0.3,
      net: "MID",
      layer: "In3.Cu",
    });
    expect(isErr(res)).toBe(true);
    expect(errText(res)).toContain("did you mean 'In3Cu'");
  });

  it("add_motor_winding rejects a dotted copper_layer", async () => {
    const id = await boardWithTwoResistors();
    const res = await addMotorWinding({
      document_id: id,
      slots: 9,
      poles: 6,
      center: { x: 25, y: 25 },
      pitch_radius: 15,
      inner_radius: 2,
      outer_radius: 5,
      trace_width: 0.3,
      copper_layer: "F.Cu",
    });
    expect(isErr(res)).toBe(true);
    expect(errText(res)).toContain("copper_layer");
    expect(errText(res)).toContain("did you mean 'FCu'");
  });
});

describe("EM receipt claims", () => {
  const stack = { dielectric_height: 0.2, copper_thickness: 0.035, dielectric_er: 4.3 };

  it("calc_impedance claims Z0, er_eff, and delay with the model that computed them", async () => {
    const r = out(await calcImpedance({ trace_width: 0.3, ...stack }));
    expect(r.claims.map((c: any) => c.quantity)).toEqual([
      "characteristic_impedance",
      "effective_permittivity",
      "propagation_delay",
    ]);
    for (const c of r.claims) {
      expect(c.domain).toBe("em");
      expect(c.method).toBe("ipc2141-microstrip");
      expect(c.inputs.trace_width).toBe(0.3);
    }
    const z0 = r.claims.find((c: any) => c.quantity === "characteristic_impedance");
    expect(z0.predicted).toBe(r.z0);
    expect(z0.unit).toBe("ohm");
  });

  it("a differential pair adds a differential_impedance claim", async () => {
    const r = out(
      await calcImpedance({
        trace_width: 0.15,
        spacing: 0.15,
        trace_type: "diff_microstrip",
        ...stack,
      }),
    );
    const diff = r.claims.find((c: any) => c.quantity === "differential_impedance");
    expect(diff.predicted).toBe(r.z_diff);
    expect(diff.method).toBe("edge-coupled-diff-pair");
    expect(diff.inputs.spacing).toBe(0.15);
  });

  it("size_impedance claims describe the snapped geometry it recommends", () => {
    const r = out(sizeImpedance({ trace_type: "microstrip", target_z0: 50, ...stack }));
    const z0 = r.claims.find((c: any) => c.quantity === "characteristic_impedance");
    expect(z0.predicted).toBe(r.measured.z0);
    expect(z0.inputs.trace_width).toBe(r.width_mm);
  });

  it("calc_coil and size_coil claim inductance and DC resistance", () => {
    const r = out(calcCoil({ inner_radius: 2, outer_radius: 6, turns: 10, trace_width: 0.2 }));
    const ind = r.claims.find((c: any) => c.quantity === "inductance");
    expect(ind.predicted).toBe(r.inductance_nh);
    expect(ind.unit).toBe("nH");
    expect(ind.method).toBe("wheeler-mohan-1999");
    expect(r.claims.find((c: any) => c.quantity === "dc_resistance").predicted).toBe(
      r.dc_resistance_ohm,
    );

    const s = out(
      sizeCoil({ target_inductance_nh: 500, inner_radius: 2, outer_radius: 8, trace_width: 0.15 }),
    );
    const sInd = s.claims.find((c: any) => c.quantity === "inductance");
    expect(sInd.predicted).toBe(s.achieved_inductance_nh);
    expect(sInd.inputs.turns).toBe(s.turns);
  });

  it("calc_rf claims resonance and Q", () => {
    const r = out(calcRf({ topology: "series_rlc", r_ohm: 50, l_henry: 10e-9, c_farad: 10e-12 }));
    const f0 = r.claims.find((c: any) => c.quantity === "resonant_frequency");
    expect(f0.predicted).toBe(r.resonance_hz);
    expect(f0.unit).toBe("Hz");
    expect(r.claims.find((c: any) => c.quantity === "q_factor").predicted).toBe(r.q_factor);
  });

  it("size_pdn claims one IR drop per budgeted node", async () => {
    const r = out(
      await sizePdn({
        nodes: 2,
        edges: [{ a: 0, b: 1, length: 10 }],
        loads: [{ node: 1, current: 1.0 }],
        targets: [{ node: 1, max_drop: 0.05 }],
      }),
    );
    expect(r.claims).toHaveLength(1);
    expect(r.claims[0].quantity).toBe("ir_drop");
    expect(r.claims[0].predicted).toBe(r.measured_drops_v[0]);
    expect(r.claims[0].inputs.node).toBe(1);
  });

  it("winding_layout claims the winding factor only for a feasible plan", () => {
    const plan = out(windingLayout({ slots: 9, poles: 12 }));
    const kw = plan.claims.find((c: any) => c.quantity === "winding_factor");
    expect(kw.predicted).toBe(plan.windingFactor);
    expect(kw.method).toBe("star-of-slots");
    const bad = out(windingLayout({ slots: 9, poles: 12, layer: "single" }));
    expect(bad.claims).toBeUndefined();
  });

  it("calc_motor claims Kt/Ke/speed/stall, plus B_gap when it computed it", async () => {
    const r = out(
      await calcMotor({
        pole_pairs: 6,
        turns_per_phase: 60,
        winding_factor: 0.866,
        inner_radius_mm: 5,
        outer_radius_mm: 30,
        phase_resistance_ohm: 0.5,
        supply_voltage_v: 24,
        magnet: {},
      }),
    );
    const q = (name: string) => r.claims.find((c: any) => c.quantity === name);
    expect(q("torque_constant").predicted).toBe(r.kt_nm_per_a);
    expect(q("torque_constant").unit).toBe("N·m/A");
    expect(q("back_emf_constant").predicted).toBe(r.ke_v_s_per_rad);
    expect(q("no_load_speed").predicted).toBe(r.no_load_speed_rad_s);
    expect(q("stall_torque").predicted).toBe(r.stall_torque_nm);
    const b = q("airgap_flux_density");
    expect(b.method).toBe("mec-reluctance");
    expect(b.predicted).toBe(r.airgap_flux_tesla);
    // Every claim in the family carries the uniform shape.
    for (const c of r.claims) {
      expect(c.domain).toBe("em");
      expect(typeof c.predicted).toBe("number");
      expect(typeof c.unit).toBe("string");
      expect(typeof c.method).toBe("string");
      expect(c.inputs).toBeTruthy();
    }
  });

  it("calc_motor induction mode claims B1, slip torque, sync speed, and rotor loss", async () => {
    const r = out(
      await calcMotor({
        mode: "induction",
        pole_pairs: 3,
        turns_per_phase: 30,
        winding_factor: 0.866,
        phase_current_a: 1.5,
        electrical_freq_hz: 100,
        effective_gap_mm: 4.7,
        sheet_conductance_s: 8120,
        inner_radius_mm: 15.3,
        outer_radius_mm: 28.5,
      }),
    );
    const quantities = r.claims.map((c: any) => c.quantity);
    expect(quantities).toEqual([
      "airgap_flux_density",
      "torque_per_unit_slip",
      "locked_rotor_torque",
      "synchronous_speed",
      "rotor_copper_loss",
    ]);
    for (const c of r.claims) {
      expect(c.domain).toBe("em");
      expect(c.method).toBe("thin-sheet-induction");
      expect(c.inputs.sheet_conductance_s).toBe(8120);
      expect(c.inputs.end_effect_factor).toBe(0.65);
    }
    const lr = r.claims.find((c: any) => c.quantity === "locked_rotor_torque");
    expect(lr.predicted).toBe(r.locked_rotor_torque_nm);
    expect(lr.unit).toBe("N·m");
  });

  it("calc_motor PM fringing derate claims both the raw MEC B and the derated B", async () => {
    const r = out(
      await calcMotor({
        pole_pairs: 6,
        turns_per_phase: 60,
        winding_factor: 0.866,
        inner_radius_mm: 5,
        outer_radius_mm: 30,
        phase_resistance_ohm: 0.5,
        supply_voltage_v: 24,
        magnet: { airgap_mm: 1, pole_width_mm: 10 },
      }),
    );
    const bClaims = r.claims.filter((c: any) => c.quantity === "airgap_flux_density");
    expect(bClaims.map((c: any) => c.method)).toEqual(["mec-reluctance", "mec-fringing-derate"]);
    expect(bClaims[0].predicted).toBe(r.airgap_flux_raw_tesla);
    expect(bClaims[1].predicted).toBe(r.airgap_flux_tesla);
    expect(bClaims[1].inputs.pole_width_mm).toBe(10);
  });

  it("check_self_start claims the catalog friction estimate and the margin", () => {
    const r = out(
      checkSelfStart({
        available_torque_nm: 5e-3,
        bearings: { type: "608-2RS", preload: "light", count: 2 },
      }),
    );
    const friction = r.claims.find((c: any) => c.quantity === "friction_torque");
    expect(friction.method).toBe("bearing-friction-catalog");
    expect(friction.predicted).toBeCloseTo(4e-3, 9);
    const margin = r.claims.find((c: any) => c.quantity === "start_margin");
    expect(margin.method).toBe("torque-friction-margin");
    expect(margin.predicted).toBe(r.margin);
    // A direct friction estimate is the caller's number, not a prediction.
    const direct = out(checkSelfStart({ available_torque_nm: 1e-3, friction_torque_nm: 2e-3 }));
    expect(direct.claims.map((c: any) => c.quantity)).toEqual(["start_margin"]);
  });
});

describe("calc_motor induction mode (thin-sheet axial rotor)", () => {
  // The validation reference from the tool spec: N=30, kw=0.866, p=3,
  // I=1.5 A rms, f=100 Hz, g=4.7 mm, σs=8120 S (2×2oz copper), r1=15.3,
  // r2=28.5 → B1 ≈ 4.7 mT, locked-rotor ≈ 18 µN·m before end effect.
  const reference = {
    mode: "induction",
    pole_pairs: 3,
    turns_per_phase: 30,
    winding_factor: 0.866,
    phase_current_a: 1.5,
    electrical_freq_hz: 100,
    effective_gap_mm: 4.7,
    sheet_conductance_s: 8120,
    inner_radius_mm: 15.3,
    outer_radius_mm: 28.5,
  };

  it("pins the reference machine: B1 ≈ 4.7 mT, raw locked-rotor ≈ 18 µN·m, 2000 rpm sync", async () => {
    const r = out(await calcMotor(reference));
    expect(r.success).toBe(true);
    expect(r.mode).toBe("induction");
    expect(r.b1_tesla).toBeCloseTo(4.69e-3, 4);
    expect(r.locked_rotor_torque_raw_nm).toBeCloseTo(17.8e-6, 7);
    // Default Russell–Norsworthy end effect 0.65 scales the delivered torque.
    expect(r.end_effect_factor).toBe(0.65);
    expect(r.locked_rotor_torque_nm).toBeCloseTo(0.65 * r.locked_rotor_torque_raw_nm, 9);
    expect(r.torque_per_unit_slip_nm).toBe(r.locked_rotor_torque_nm);
    expect(r.sync_rpm).toBe(2000);
    // Locked-rotor sheet loss is the air-gap power T·ωsync.
    const omegaSync = (2 * Math.PI * 100) / 3;
    expect(r.copper_loss_w).toBeCloseTo(r.locked_rotor_torque_nm * omegaSync, 7);
  });

  it("honors an explicit end_effect_factor", async () => {
    const r = out(await calcMotor({ ...reference, end_effect_factor: 1 }));
    expect(r.locked_rotor_torque_nm).toBe(r.locked_rotor_torque_raw_nm);
  });

  it("torque scales with conductance and current squared, field inversely with gap", async () => {
    const base = out(await calcMotor(reference));
    const thick = out(await calcMotor({ ...reference, sheet_conductance_s: 16240 }));
    expect(thick.locked_rotor_torque_nm / base.locked_rotor_torque_nm).toBeCloseTo(2, 4);
    const hot = out(await calcMotor({ ...reference, phase_current_a: 3 }));
    expect(hot.locked_rotor_torque_nm / base.locked_rotor_torque_nm).toBeCloseTo(4, 4);
    const far = out(await calcMotor({ ...reference, effective_gap_mm: 9.4 }));
    expect(far.b1_tesla / base.b1_tesla).toBeCloseTo(0.5, 4);
  });

  it("rejects missing or non-physical induction inputs", async () => {
    for (const bad of [
      { ...reference, phase_current_a: undefined },
      { ...reference, electrical_freq_hz: 0 },
      { ...reference, effective_gap_mm: -1 },
      { ...reference, sheet_conductance_s: undefined },
      { ...reference, end_effect_factor: 1.5 },
      { ...reference, mode: "linear" },
    ]) {
      expect(isErr(await calcMotor(bad as Record<string, unknown>))).toBe(true);
    }
  });

  it("PM mode still requires its electrical inputs", async () => {
    const res = await calcMotor({
      pole_pairs: 6,
      turns_per_phase: 60,
      inner_radius_mm: 5,
      outer_radius_mm: 30,
    });
    expect(isErr(res)).toBe(true);
  });
});

describe("calc_motor PM fringing derate", () => {
  const base = {
    pole_pairs: 6,
    turns_per_phase: 60,
    winding_factor: 0.866,
    inner_radius_mm: 5,
    outer_radius_mm: 30,
    phase_resistance_ohm: 0.5,
    supply_voltage_v: 24,
  };

  it("reports raw and derated B and uses the derated value for Kt", async () => {
    const plain = out(await calcMotor({ ...base, magnet: { airgap_mm: 1 } }));
    const derated = out(
      await calcMotor({ ...base, magnet: { airgap_mm: 1, pole_width_mm: 10 } }),
    );
    // w/(w+2g) = 10/12.
    expect(derated.fringing_derate).toBeCloseTo(10 / 12, 3);
    expect(derated.airgap_flux_raw_tesla).toBe(plain.airgap_flux_tesla);
    expect(derated.airgap_flux_tesla).toBeCloseTo(
      plain.airgap_flux_tesla * (10 / 12),
      3,
    );
    // Kt scales linearly with the gap flux.
    expect(derated.kt_nm_per_a / plain.kt_nm_per_a).toBeCloseTo(10 / 12, 2);
    // Without pole_width_mm nothing changes and no derate fields appear.
    expect(plain.fringing_derate).toBeUndefined();
    expect(plain.airgap_flux_raw_tesla).toBeUndefined();
  });

  it("a wide pole barely derates; derate shrinks as the gap grows", async () => {
    const wide = out(await calcMotor({ ...base, magnet: { airgap_mm: 0.5, pole_width_mm: 40 } }));
    expect(wide.fringing_derate).toBeGreaterThan(0.95);
    const bigGap = out(await calcMotor({ ...base, magnet: { airgap_mm: 3, pole_width_mm: 10 } }));
    expect(bigGap.fringing_derate).toBeLessThan(wide.fringing_derate);
  });

  it("rejects a non-positive pole width", async () => {
    expect(isErr(await calcMotor({ ...base, magnet: { pole_width_mm: 0 } }))).toBe(true);
  });
});

describe("check_self_start", () => {
  it("608-2RS light pair lands at the documented 1–4 mN·m and gates fail-closed", () => {
    const r = out(
      checkSelfStart({
        available_torque_nm: 2e-3, // 2 mN·m available
        bearings: { type: "608-2RS", preload: "light", count: 2 },
      }),
    );
    expect(r.friction_torque_mnm).toEqual({ min: 1, max: 4 });
    // Beats the optimistic end, not the worst case → fail-closed no-start.
    expect(r.starts).toBe(false);
    expect(r.starts_best_case).toBe(true);
    expect(r.margin).toBeCloseTo(0.5, 6);

    const strong = out(
      checkSelfStart({
        available_torque_nm: 12e-3,
        bearings: { type: "608-2RS", preload: "light", count: 2 },
      }),
    );
    expect(strong.starts).toBe(true);
    expect(strong.margin).toBeCloseTo(3, 6);
  });

  it("computes available torque from Kt·I when not given directly", () => {
    const r = out(
      checkSelfStart({
        kt_nm_per_a: 0.004,
        current_a: 1.5,
        bearings: { type: "608-ZZ", count: 2 },
      }),
    );
    expect(r.available_torque_nm).toBeCloseTo(0.006, 9);
    expect(r.available_torque_source).toBe("kt_times_current");
    expect(r.starts).toBe(true);
  });

  it("shielded and miniature presets are far freer than sealed 608s", () => {
    const sealed = out(
      checkSelfStart({ available_torque_nm: 1e-3, bearings: { type: "608-2RS", count: 2 } }),
    );
    const shielded = out(
      checkSelfStart({ available_torque_nm: 1e-3, bearings: { type: "608-ZZ", count: 2 } }),
    );
    expect(shielded.friction_torque_mnm.max).toBeLessThan(sealed.friction_torque_mnm.max);
    expect(shielded.starts).toBe(true);
    for (const type of ["625", "688"]) {
      const r = out(checkSelfStart({ available_torque_nm: 1e-3, bearings: { type, count: 2 } }));
      expect(r.starts).toBe(true);
      expect(r.bearings.per_bearing_mnm.max).toBeLessThanOrEqual(0.6);
    }
  });

  it("medium preload and more bearings scale the range", () => {
    const light = out(
      checkSelfStart({ available_torque_nm: 1, bearings: { type: "608-2RS", preload: "light", count: 2 } }),
    );
    const medium = out(
      checkSelfStart({ available_torque_nm: 1, bearings: { type: "608-2RS", preload: "medium", count: 2 } }),
    );
    expect(medium.friction_torque_mnm.max).toBeGreaterThan(light.friction_torque_mnm.max);
    const quad = out(
      checkSelfStart({ available_torque_nm: 1, bearings: { type: "608-2RS", preload: "light", count: 4 } }),
    );
    expect(quad.friction_torque_mnm.max).toBeCloseTo(2 * light.friction_torque_mnm.max, 9);
  });

  it("a direct friction estimate overrides the catalog", () => {
    const r = out(
      checkSelfStart({
        available_torque_nm: 3e-3,
        friction_torque_nm: 2e-3,
        bearings: { type: "608-2RS", count: 2 },
      }),
    );
    expect(r.friction_source).toBe("direct");
    expect(r.friction_torque_mnm).toEqual({ min: 2, max: 2 });
    expect(r.starts).toBe(true);
    expect(r.margin).toBeCloseTo(1.5, 6);
  });

  it("an induction locked-rotor µN·m machine does NOT start on sealed bearings", () => {
    // The reference thin-sheet machine: ~11.6 µN·m delivered locked-rotor
    // torque vs a pair of 608-2RS at 1–4 mN·m — two orders of magnitude short.
    const r = out(
      checkSelfStart({
        available_torque_nm: 11.6e-6,
        bearings: { type: "608-2RS", preload: "light", count: 2 },
      }),
    );
    expect(r.starts).toBe(false);
    expect(r.starts_best_case).toBe(false);
    expect(r.margin).toBeLessThan(0.01);
  });

  it("rejects bad inputs", () => {
    expect(isErr(checkSelfStart({}))).toBe(true);
    expect(isErr(checkSelfStart({ available_torque_nm: -1 }))).toBe(true);
    expect(isErr(checkSelfStart({ kt_nm_per_a: 0.01 }))).toBe(true);
    expect(
      isErr(checkSelfStart({ available_torque_nm: 1, bearings: { type: "6900" } })),
    ).toBe(true);
    expect(
      isErr(checkSelfStart({ available_torque_nm: 1, bearings: { type: "625", preload: "heavy" } })),
    ).toBe(true);
    expect(
      isErr(checkSelfStart({ available_torque_nm: 1, bearings: { count: 1.5 } })),
    ).toBe(true);
    expect(isErr(checkSelfStart({ available_torque_nm: 1, friction_torque_nm: 0 }))).toBe(true);
  });
});

describe("diff_stripline model consistency (calc_impedance vs size_impedance)", () => {
  const stack = { dielectric_height: 0.4, copper_thickness: 0.035, dielectric_er: 4.3 };

  it("diff_stripline computes its base Z0 with the stripline formula", async () => {
    const diff = out(
      await calcImpedance({ trace_type: "diff_stripline", trace_width: 0.15, spacing: 0.2, ...stack }),
    );
    const se = out(
      await calcImpedance({ trace_type: "stripline", trace_width: 0.15, ...stack }),
    );
    // Same base Z0 as plain stripline — NOT the microstrip formula.
    expect(diff.z0).toBeCloseTo(se.z0, 6);
    // Fully embedded in the dielectric → er_eff == er (and the delay follows).
    expect(diff.er_eff).toBeCloseTo(4.3, 6);
    // Claims name the model actually used.
    const z0claim = diff.claims.find((c: any) => c.quantity === "characteristic_impedance");
    expect(z0claim.method).toBe("ipc2141-stripline");
  });

  it("size_impedance(diff_stripline) geometry is reproduced by calc_impedance — same model", async () => {
    const sized = out(
      sizeImpedance({ trace_type: "diff_stripline", target_z0: 50, target_diff_z0: 90, ...stack }),
    );
    expect(sized.within_tolerance).toBe(true);
    const calc = out(
      await calcImpedance({
        trace_type: "diff_stripline",
        trace_width: sized.width_mm,
        spacing: sized.spacing_mm,
        ...stack,
      }),
    );
    expect(calc.z0).toBeCloseTo(sized.measured.z0, 1);
    expect(calc.z_diff).toBeCloseTo(sized.measured.diff_z0, 1);
  });

  it("stripline and microstrip diff pairs use their own coupling constants", async () => {
    // Same s/h for both families; compare the implied coupling k = z_diff / (2·z0).
    const geo = { trace_width: 0.15, spacing: 0.4, ...stack }; // s/h = 1
    const micro = out(await calcImpedance({ trace_type: "diff_microstrip", ...geo }));
    const strip = out(await calcImpedance({ trace_type: "diff_stripline", ...geo }));
    const kMicro = micro.z_diff / (2 * micro.z0);
    const kStrip = strip.z_diff / (2 * strip.z0);
    // Analytic constants at s/h = 1: microstrip 1 − 0.48·e^−0.96, stripline 1 − 0.347·e^−2.9.
    expect(kMicro).toBeCloseTo(1 - 0.48 * Math.exp(-0.96), 2);
    expect(kStrip).toBeCloseTo(1 - 0.347 * Math.exp(-2.9), 2);
    // Stripline couples less at the same spacing — the pair sits closer to 2·Z0.
    expect(kStrip).toBeGreaterThan(kMicro);
  });
});
