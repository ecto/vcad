import { describe, it, expect, beforeAll, beforeEach } from "vitest";
import { Engine } from "@vcad/engine";
import type { Document, Pcb, Vec2 } from "@vcad/ir";
import { createDocument } from "@vcad/ir";
import { addZone, solidFromBoard } from "../tools/ecad.js";
import { checkEnclosureFit } from "../tools/enclosure.js";
import { computeInspection } from "../tools/inspect.js";
import { documents, getSession, openDocument, registerSession } from "../tools/session.js";
// Proven showcase case geometry (untyped demo module; src/__tests__ is
// excluded from tsc so the JS import needs no declarations).
import { f405CaseDocument } from "../../../../examples/f405-enclosure/geometry.mjs";

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

/** CCW n-gon approximating a circle. */
function circleLoop(cx: number, cy: number, r: number, n: number): Vec2[] {
  const pts: Vec2[] = [];
  for (let i = 0; i < n; i++) {
    const t = (2 * Math.PI * i) / n;
    pts.push({ x: cx + r * Math.cos(t), y: cy + r * Math.sin(t) });
  }
  return pts;
}

/** Absolute shoelace area of a closed loop, mm². */
function loopArea(loop: Vec2[]): number {
  let area = 0;
  for (let i = 0; i < loop.length; i++) {
    const a = loop[i];
    const b = loop[(i + 1) % loop.length];
    area += a.x * b.y - b.x * a.y;
  }
  return Math.abs(area / 2);
}

// The rotor board under test: a 58mm disc with a center bore and four hub
// holes (a real PCB-motor rotor shape), 2oz copper pours on both faces.
const BOARD_R = 29;
const BORE_R = 4;
const HUB_R = 1.6;
const HUB_PITCH_R = 16;
const THICKNESS = 1.6;
const CU_2OZ_MM = 0.07;

const FR4_DENSITY_KG_M3 = 1850;
const COPPER_DENSITY_KG_M3 = 8960;

function rotorOutline(): { vertices: Vec2[]; cutouts: Vec2[][] } {
  return {
    vertices: circleLoop(0, 0, BOARD_R, 96),
    cutouts: [
      circleLoop(0, 0, BORE_R, 48),
      ...[0, 90, 180, 270].map((deg) => {
        const t = (deg * Math.PI) / 180;
        return circleLoop(
          HUB_PITCH_R * Math.cos(t),
          HUB_PITCH_R * Math.sin(t),
          HUB_R,
          32,
        );
      }),
    ],
  };
}

/** Polygonized board area (outline minus bore + hub cutouts), mm². */
function rotorPolygonArea(): number {
  const { vertices, cutouts } = rotorOutline();
  return loopArea(vertices) - cutouts.reduce((s, c) => s + loopArea(c), 0);
}

/** Register a PCB session holding the rotor board with 2oz pours both sides. */
function registerRotorBoard(): string {
  const { vertices, cutouts } = rotorOutline();
  const pcb = {
    outline: { vertices, cutouts, thickness: THICKNESS },
    stackup: {
      layers: [
        {
          layer: "FCu",
          copperThickness: CU_2OZ_MM,
          dielectricThickness: 1.46,
          dielectricEr: 4.5,
          material: "FR4",
        },
        { layer: "BCu", copperThickness: CU_2OZ_MM },
      ],
    },
    nets: [{ id: "GND", name: "GND" }],
    rules: {
      defaultRules: {
        name: "Default",
        traceWidth: 0.25,
        clearance: 0.2,
        viaDiameter: 0.8,
        viaDrill: 0.4,
      },
      classRules: [],
      netClassAssignments: {},
      edgeClearance: 0.5,
      holeToHole: 0.5,
      minAnnularRing: 0.15,
      minDrill: 0.2,
    },
    footprints: [],
    traces: [],
    vias: [],
    zones: [],
  } as unknown as Pcb;
  const doc = createDocument();
  (doc as Document & { pcb?: Pcb }).pcb = pcb;
  const id = registerSession(doc);
  // 2oz pours on both faces; fill_board carries the bore/hub cutouts as voids.
  expect(out(addZone({ document_id: id, net: "GND", layer: "FCu", fill_board: true })).success).toBe(true);
  expect(out(addZone({ document_id: id, net: "GND", layer: "BCu", fill_board: true })).success).toBe(true);
  return id;
}

describe("solid_from_board (58mm rotor board round trip)", () => {
  it("extrudes the outline with bore + hub cutouts intact", async () => {
    const id = registerRotorBoard();
    const res = out(await solidFromBoard({ document_id: id }));

    expect(res.success).toBe(true);
    expect(res.created_session).toBe(true);
    expect(res.source_document_id).toBe(id);
    expect(res.substrate.cutouts).toBe(5);

    const solidDoc = getSession(res.document_id);
    expect(solidDoc.roots.length).toBe(1);
    const insp = computeInspection(solidDoc, engine);

    // Volume ≈ board area × thickness within 2% (the task's round-trip gate).
    const expectedVolume = rotorPolygonArea() * THICKNESS;
    expect(Math.abs(insp.volume_mm3 - expectedVolume) / expectedVolume).toBeLessThan(0.02);

    // Cutouts actually removed: a hole-less disc is ~3.2% bigger, well outside
    // the 2% gate above — but assert the direction explicitly too.
    const noHoleVolume = loopArea(rotorOutline().vertices) * THICKNESS;
    const holeVolume = noHoleVolume - expectedVolume;
    expect(insp.volume_mm3).toBeLessThan(noHoleVolume - 0.5 * holeVolume);

    // Bbox: 58mm disc, board bottom at z = 0, top at thickness.
    expect(insp.bounding_box.min.x).toBeCloseTo(-BOARD_R, 0);
    expect(insp.bounding_box.max.x).toBeCloseTo(BOARD_R, 0);
    expect(insp.bounding_box.min.y).toBeCloseTo(-BOARD_R, 0);
    expect(insp.bounding_box.max.y).toBeCloseTo(BOARD_R, 0);
    expect(insp.bounding_box.min.z).toBeCloseTo(0, 3);
    expect(insp.bounding_box.max.z).toBeCloseTo(THICKNESS, 3);
  });

  it("mass metadata lands within 5% of hand-computed FR4 + 2oz copper", async () => {
    const id = registerRotorBoard();
    const res = out(await solidFromBoard({ document_id: id }));

    // Hand computation from ideal circles (datasheet style).
    const idealArea = Math.PI * (BOARD_R ** 2 - BORE_R ** 2 - 4 * HUB_R ** 2);
    const fr4G = (idealArea * THICKNESS * FR4_DENSITY_KG_M3) / 1e6;
    const copperG = (2 * idealArea * CU_2OZ_MM * COPPER_DENSITY_KG_M3) / 1e6;
    const totalG = fr4G + copperG;

    expect(Math.abs(res.mass.fr4_g - fr4G) / fr4G).toBeLessThan(0.05);
    expect(Math.abs(res.mass.copper_g - copperG) / copperG).toBeLessThan(0.05);
    expect(Math.abs(res.mass.total_g - totalG) / totalG).toBeLessThan(0.05);

    // Both pours were counted, each capped at the board area.
    expect(res.mass.copper_area_mm2_by_layer.FCu).toBeGreaterThan(0);
    expect(res.mass.copper_area_mm2_by_layer.BCu).toBeGreaterThan(0);

    // inspect_cad sees the same mass through the homogenized-density material —
    // this is what makes physics / mass-property queries realistic.
    const insp = computeInspection(getSession(res.document_id), engine);
    expect(insp.mass_g).toBeDefined();
    expect(Math.abs(insp.mass_g! - res.mass.total_g) / res.mass.total_g).toBeLessThan(0.05);
    const material = getSession(res.document_id).materials[res.mass.material];
    expect(material?.density).toBeGreaterThan(FR4_DENSITY_KG_M3);
  });

  it("emits simplified component keep-out volumes above the board", async () => {
    const id = registerRotorBoard();
    const pcb = (getSession(id) as Document & { pcb?: Pcb }).pcb!;
    pcb.footprints.push({
      ref: "R1",
      value: "10k",
      footprintName: "Resistor_SMD:R_0805",
      position: { x: 22, y: 0 },
      rotation: 0,
      front: true,
      pads: [
        {
          number: "1",
          padType: "SMD",
          shape: { type: "Rect", width: 1.0, height: 1.3 },
          position: { x: -0.95, y: 0 },
          layers: ["FCu"],
        },
        {
          number: "2",
          padType: "SMD",
          shape: { type: "Rect", width: 1.0, height: 1.3 },
          position: { x: 0.95, y: 0 },
          layers: ["FCu"],
        },
      ],
    } as unknown as Pcb["footprints"][number]);

    const res = out(await solidFromBoard({ document_id: id }));
    expect(res.components.count).toBe(1);
    expect(res.components.part_ids.length).toBe(1);
    const solidDoc = getSession(res.document_id);
    expect(solidDoc.roots.length).toBe(2); // substrate + R1 keep-out

    // The keep-out sits on the board top with a plausible chip height.
    const box = res.components.boxes[0];
    expect(box.ref).toBe("R1");
    expect(box.min.z).toBeGreaterThanOrEqual(THICKNESS - 0.2);
    expect(box.max.z).toBeGreaterThan(THICKNESS);
    expect(box.max.z).toBeLessThan(THICKNESS + 3);
    expect(box.min.x).toBeGreaterThan(19);
    expect(box.max.x).toBeLessThan(25);

    // Opt-out leaves just the substrate.
    const bare = out(await solidFromBoard({ document_id: id, include_components: false }));
    expect(bare.components.count).toBe(0);
    expect(getSession(bare.document_id).roots.length).toBe(1);
  });

  it("injects into an existing CAD session that still works for check_enclosure_fit", async () => {
    const enc = out(openDocument({ initial: f405CaseDocument() as unknown as Document }));
    const encDoc = getSession(enc.document_id);
    const caseRootId = encDoc.roots[0].root;
    const rootsBefore = encDoc.roots.length;

    const id = registerRotorBoard();
    const res = out(
      await solidFromBoard({
        document_id: id,
        document_id_target: enc.document_id,
        part_name: "rotor-board",
      }),
    );
    expect(res.success).toBe(true);
    expect(res.created_session).toBe(false);
    expect(res.document_id).toBe(enc.document_id);
    expect(getSession(enc.document_id).roots.length).toBe(rootsBefore + 1);
    const partNode = getSession(enc.document_id).nodes[res.part_id];
    expect(partNode?.name).toBe("rotor-board");
    expect((partNode?.op as { type?: string }).type).toBe("Extrude");

    // The enclosure session — now also holding the injected board solid —
    // still serves as check_enclosure_fit's enclosure input. The board solid
    // is vertex-heavier than the case, so select the case explicitly.
    const fit = out(
      await checkEnclosureFit(
        {
          document_id: id,
          enclosure_document_id: enc.document_id,
          enclosure_part_id: String(caseRootId),
        },
        engine,
      ),
    );
    expect(fit.success).toBe(true);
    expect(fit.cavity).toBeTruthy();
    expect(fit.checks.some((c: { id: string }) => c.id === "board_fit")).toBe(true);
  });

  it("errors cleanly without a PCB or with an unknown target session", async () => {
    const cad = out(openDocument({}));
    const noPcb = await solidFromBoard({ document_id: cad.document_id });
    expect(noPcb.isError).toBe(true);
    expect(noPcb.content[0].text).toMatch(/no PCB/i);

    const id = registerRotorBoard();
    const badTarget = await solidFromBoard({
      document_id: id,
      document_id_target: "doc_nope",
    });
    expect(badTarget.isError).toBe(true);
    expect(badTarget.content[0].text).toMatch(/unknown document_id/i);
  });
});
