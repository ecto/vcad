import { describe, it, expect, beforeAll, beforeEach } from "vitest";
import { Engine } from "@vcad/engine";
import type { Document, Pcb } from "@vcad/ir";
import { createSchematic, placeComponents, setPlacement, buildReceipt } from "../tools/ecad.js";
import { checkEnclosureFit } from "../tools/enclosure.js";
import { documents, getSession, openDocument } from "../tools/session.js";
// Single source of truth for the showcase case geometry (untyped demo module;
// src/__tests__ is excluded from tsc so the JS import needs no declarations).
import {
  f405CaseDocument,
  boardHoleCenters,
  usbConnectorLocal,
} from "../../../../examples/f405-enclosure/geometry.mjs";

let engine: Engine;

beforeAll(async () => {
  engine = await Engine.init();
});

beforeEach(() => {
  documents.clear();
});

// eslint-disable-next-line @typescript-eslint/no-explicit-any
function out(result: { content: Array<{ type: string; text: string }> }): any {
  return JSON.parse(result.content[0].text);
}

function getPcbBoard(doc: Document): Pcb {
  const node = Object.values(doc.nodes).find((n) => (n.op as { type: string }).type === "PcbBoard");
  expect(node).toBeDefined();
  return (node!.op as unknown as { board: Pcb }).board;
}

/** Append four M3 NPTH mounting holes at the board-local 30.5mm pattern. */
function addMountingHoles(pcb: Pcb) {
  boardHoleCenters().forEach((h: { x: number; y: number }, i: number) => {
    pcb.footprints.push({
      ref: `H${i + 1}`,
      value: "M3",
      footprintName: "MountingHole_3.2mm_M3",
      position: { x: h.x, y: h.y },
      pads: [
        {
          number: "1",
          padType: "NPTH",
          shape: { type: "Circle", diameter: 3.2 },
          position: { x: 0, y: 0 },
          layers: ["FCu", "BCu"],
          drill: { diameter: 3.2 },
        },
      ],
    } as unknown as Pcb["footprints"][number]);
  });
}

/** Build the 36×36 F405 board: MCU + USB-C + mounting holes, placed. */
async function buildBoard(): Promise<string> {
  const created = out(
    await createSchematic({
      components: [
        {
          ref: "U1",
          value: "STM32F405",
          footprint: "QFP-48",
          x: 18,
          y: 18,
          pins: [
            { number: "1", name: "VDD", type: "PowerInput", x: 0, y: 0 },
            { number: "2", name: "PA11", type: "Bidirectional", x: 0, y: 1 },
            { number: "3", name: "PA12", type: "Bidirectional", x: 0, y: 2 },
          ],
        },
        {
          ref: "J1",
          value: "USB-C",
          footprint: "USB_C_Receptacle",
          x: 30,
          y: 18,
          pins: [
            { number: "A6", name: "DP", type: "Bidirectional", x: 0, y: 0 },
            { number: "A7", name: "DM", type: "Bidirectional", x: 0, y: 1 },
          ],
        },
      ],
      nets: { USB_DP: ["U1.2", "J1.A6"], USB_DM: ["U1.3", "J1.A7"] },
    }),
  );
  const id: string = created.document_id;
  out(await placeComponents({ document_id: id, board_width: 36, board_height: 36 }));
  // USB-C on the +X edge; MCU centered.
  const usb = usbConnectorLocal();
  out(
    await setPlacement({
      document_id: id,
      placements: [
        { ref: "J1", x: usb.x, y: usb.y },
        { ref: "U1", x: 18, y: 18 },
      ],
    }),
  );
  addMountingHoles(getPcbBoard(getSession(id)));
  return id;
}

describe("check_enclosure_fit (F405 in a 3D-printed case)", () => {
  it("extracts the case cavity/standoffs/cutout and verifies the board fits", async () => {
    const enc = out(openDocument({ initial: f405CaseDocument() as unknown as Document }));
    const boardId = await buildBoard();

    const res = out(
      await checkEnclosureFit(
        { document_id: boardId, enclosure_document_id: enc.document_id },
        engine,
      ),
    );

    // The kernel-evaluated case mesh yields the cavity, four standoffs, one cutout.
    expect(res.cavity).toBeTruthy();
    expect(res.standoffs_detected).toBe(4);
    expect(res.openings_detected).toBe(1);

    const byId = Object.fromEntries(res.checks.map((c: { id: string }) => [c.id, c]));
    expect(byId.board_fit.status).toBe("pass");
    expect(byId.mounting_holes.status).toBe("pass");
    expect(byId.mounting_holes.measurements.holes_matched).toBe(4);
    expect(byId.connector_cutouts.status).toBe("pass");
    // Lid clearance passes when kernel component bodies are available, else skips.
    expect(["pass", "skip"]).toContain(byId.lid_clearance.status);
    expect(res.ok).toBe(true);
  });

  it("derives a board outline + holes from the cavity on request", async () => {
    const enc = out(openDocument({ initial: f405CaseDocument() as unknown as Document }));
    const boardId = await buildBoard();
    const res = out(
      await checkEnclosureFit(
        { document_id: boardId, enclosure_document_id: enc.document_id, derive: true },
        engine,
      ),
    );
    expect(res.derived_board).toBeTruthy();
    expect(res.derived_board.mountingHoles.length).toBe(4);
    expect(res.derived_board.outline.vertices.length).toBe(4);
  });

  it("flags a board that overhangs the cavity", async () => {
    const enc = out(openDocument({ initial: f405CaseDocument() as unknown as Document }));
    const boardId = await buildBoard();
    // Shove the board 6mm off-center so it overhangs the +X wall.
    const res = out(
      await checkEnclosureFit(
        {
          document_id: boardId,
          enclosure_document_id: enc.document_id,
          board_offset: { x: 9, y: 3, z: 5 },
        },
        engine,
      ),
    );
    expect(res.checks.find((c: { id: string }) => c.id === "board_fit").status).toBe("fail");
    expect(res.ok).toBe(false);
  });

  it("errors clearly when the enclosure solid has no cavity", async () => {
    const block: Document = {
      version: "0.1",
      nodes: { "1": { id: 1, name: "block", op: { type: "Cube", size: { x: 20, y: 20, z: 10 } } } },
      materials: {},
      part_materials: {},
      roots: [{ root: 1, material: "default" }],
    } as unknown as Document;
    const enc = out(openDocument({ initial: block }));
    const boardId = await buildBoard();
    const res = await checkEnclosureFit(
      { document_id: boardId, enclosure_document_id: enc.document_id },
      engine,
    );
    expect(res.isError).toBe(true);
    expect(res.content[0].text).toMatch(/no interior cavity/i);
  });

  it("surfaces the enclosure-fit verdict through build_receipt", async () => {
    const enc = out(openDocument({ initial: f405CaseDocument() as unknown as Document }));
    const boardId = await buildBoard();
    const result = await buildReceipt(
      { document_id: boardId, enclosure_document_id: enc.document_id },
      engine,
    );
    const sc = (result as { structuredContent?: Record<string, unknown> }).structuredContent;
    expect(sc).toBeTruthy();
    expect(sc!.receipt).toBeTruthy();
    expect(sc!.enclosure_fit).toBeTruthy();
    const fit = sc!.enclosure_fit as { ok: boolean; checks: Array<{ id: string }> };
    expect(fit.checks.some((c) => c.id === "board_fit")).toBe(true);
  });
});
