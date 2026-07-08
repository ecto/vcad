import { describe, it, expect, beforeAll, beforeEach } from "vitest";
import { Engine } from "@vcad/engine";
import type { Document, Pcb } from "@vcad/ir";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { createServer } from "../server.js";
import {
  createSchematic,
  placeComponents,
  setPlacement,
} from "../tools/ecad.js";
import { documents, getSession, openDocument } from "../tools/session.js";
// Same showcase geometry the enclosure-fit test drives (untyped demo module;
// src/__tests__ is excluded from tsc so the JS import needs no declarations).
import {
  f405CaseDocument,
  boardHoleCenters,
  usbConnectorLocal,
} from "../../../../examples/f405-enclosure/geometry.mjs";

/**
 * Every tool that declares an `outputSchema` MUST, per the MCP spec, return
 * `structuredContent` on success — and that payload must match the schema. This
 * test drives one live success per outputSchema-declaring tool through the real
 * MCP client, which validates the returned `structuredContent` against the
 * advertised `outputSchema` (ajv, via the SDK) and throws on a mismatch or a
 * missing payload. So a green run is a per-tool schema-conformance proof.
 *
 * Setup docs are created via the module handlers directly; anonymous client
 * calls share the same process-wide session cache, so the driven tool sees them.
 */

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
  const node = Object.values(doc.nodes).find(
    (n) => (n.op as { type: string }).type === "PcbBoard",
  );
  return (node!.op as unknown as { board: Pcb }).board;
}

/** Two 10 mm cubes with a 10 mm gap along X (mirrors measure.test fixture). */
function twoCubesDocument(): Document {
  const nodes: Record<string, unknown> = {};
  let id = 0;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const add = (name: string, op: any): number => {
    id += 1;
    nodes[String(id)] = { id, name, op };
    return id;
  };
  const a = add("cube-a", { type: "Cube", size: { x: 10, y: 10, z: 10 } });
  const bSolid = add("cube-b-solid", { type: "Cube", size: { x: 10, y: 10, z: 10 } });
  const b = add("cube-b", { type: "Translate", child: bSolid, offset: { x: 20, y: 0, z: 0 } });
  return {
    version: "0.1",
    nodes,
    materials: {},
    part_materials: {},
    roots: [
      { root: a, material: "aluminum" },
      { root: b, material: "steel" },
    ],
  } as unknown as Document;
}

/** Rotor/stator with a 1 mm design air gap (mirrors clearance.test fixture). */
function rotorStatorDocument(): Document {
  const nodes: Record<string, unknown> = {};
  let id = 0;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const add = (name: string, op: any): number => {
    id += 1;
    nodes[String(id)] = { id, name, op };
    return id;
  };
  const rotorCyl = add("rotor-solid", { type: "Cylinder", radius: 5, height: 8, segments: 128 });
  const rotor = add("rotor", { type: "Translate", child: rotorCyl, offset: { x: 0, y: 0, z: 1 } });
  const statorOuter = add("stator-outer", { type: "Cylinder", radius: 10, height: 10, segments: 128 });
  const statorBoreCyl = add("stator-bore-solid", { type: "Cylinder", radius: 6, height: 12, segments: 128 });
  const statorBore = add("stator-bore", { type: "Translate", child: statorBoreCyl, offset: { x: 0, y: 0, z: -1 } });
  const stator = add("stator", { type: "Difference", left: statorOuter, right: statorBore });
  return {
    version: "0.1",
    nodes,
    materials: {},
    part_materials: {},
    roots: [
      { root: rotor, material: "steel" },
      { root: stator, material: "aluminum" },
    ],
  } as unknown as Document;
}

/** A cube document carrying a named parameter (for set_parameters). */
function parametricDocument(): Document {
  return {
    version: "0.1",
    nodes: { "1": { id: 1, name: "c", op: { type: "Cube", size: { x: 5, y: 5, z: 5 } } } },
    materials: {},
    part_materials: {},
    roots: [{ root: 1, material: "aluminum" }],
    parameters: { r: { value: 5 } },
  } as unknown as Document;
}

/** Append four M3 NPTH mounting holes at the board-local 30.5 mm pattern. */
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

/** Build the 36×36 F405 board (MCU + USB-C + mounting holes), placed. */
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

describe("outputSchema conformance (live result matches declared schema)", () => {
  let client: Client;

  beforeEach(async () => {
    const server = await createServer(engine, { user: null });
    const [clientT, serverT] = InMemoryTransport.createLinkedPair();
    client = new Client({ name: "output-schema", version: "0.0.0" }, { capabilities: {} });
    await Promise.all([client.connect(clientT), server.connect(serverT)]);
    // Caches the per-tool output validators the client applies on callTool.
    await client.listTools();
  });

  /** Call a tool and assert it returned schema-valid structuredContent. The SDK
   *  client throws if the payload is missing or fails outputSchema validation,
   *  so reaching the assertions means the live result conformed. */
  async function expectValidStructured(
    name: string,
    args: Record<string, unknown>,
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
  ): Promise<any> {
    const result = await client.callTool({ name, arguments: args });
    expect(result.isError, `${name} returned an error: ${JSON.stringify(result.content)}`).not.toBe(true);
    expect(result.structuredContent, `${name} declared outputSchema but returned no structuredContent`).toBeTruthy();
    return result.structuredContent;
  }

  it("measure", async () => {
    const doc = out(openDocument({ initial: twoCubesDocument() }));
    const sc = await expectValidStructured("measure", {
      document_id: doc.document_id,
      part_ids: ["cube-a"],
    });
    expect(sc.measure).toBeTruthy();
  });

  it("set_parameters", async () => {
    const doc = out(openDocument({ initial: parametricDocument() }));
    const sc = await expectValidStructured("set_parameters", {
      document_id: doc.document_id,
      parameters: { r: 10 },
    });
    expect(Array.isArray(sc.changed)).toBe(true);
  });

  it("check_clearance + build_receipt (mechanical-only receipt)", async () => {
    const doc = out(openDocument({ initial: rotorStatorDocument() }));
    const cl = await expectValidStructured("check_clearance", {
      document_id: doc.document_id,
      group_a: ["rotor"],
      group_b: ["stator"],
      min_mm: 0.9,
      label: "air-gap",
    });
    expect(cl.clearance).toBeTruthy();

    // The persisted clearance spec makes build_receipt take its mechanical-only
    // path, which unconditionally emits a `unified` receipt in structuredContent.
    const receipt = await expectValidStructured("build_receipt", {
      document_id: doc.document_id,
    });
    expect(receipt.unified).toBeTruthy();
  });

  it("solid_from_board + export_kicad + build_receipt + verify_receipt (PCB)", async () => {
    const boardId = await buildBoard();
    const solid = await expectValidStructured("solid_from_board", { document_id: boardId });
    expect(solid.solid_from_board).toBeTruthy();

    const kicad = await expectValidStructured("export_kicad", { document_id: boardId });
    expect(kicad.export_kicad).toBeTruthy();

    // PCB path: build_receipt emits both `unified` and a re-runnable `receipt`
    // (board_hash); verify_receipt re-runs that legacy receipt against the board
    // via the kernel (no engine needed on this path).
    const receipt = await expectValidStructured("build_receipt", { document_id: boardId });
    expect(receipt.receipt).toBeTruthy();

    const verified = await expectValidStructured("verify_receipt", {
      document_id: boardId,
      receipt: receipt.receipt,
    });
    expect(verified.verify_receipt).toBeTruthy();
  });

  it("check_enclosure_fit", async () => {
    const enc = out(openDocument({ initial: f405CaseDocument() as unknown as Document }));
    const boardId = await buildBoard();
    const sc = await expectValidStructured("check_enclosure_fit", {
      document_id: boardId,
      enclosure_document_id: enc.document_id,
    });
    expect(sc.enclosure_fit).toBeTruthy();
  });
});
