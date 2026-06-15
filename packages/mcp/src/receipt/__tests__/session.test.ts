import { describe, it, expect, beforeAll, beforeEach } from "vitest";
import { Engine } from "@vcad/engine";
import { createSchematic, placeComponents, routeNets, runDrc } from "../../tools/ecad.js";
import { documents } from "../../tools/session.js";
import { ReceiptSession, type DrcSnapshot } from "../index.js";

beforeAll(async () => {
  await Engine.init();
});
beforeEach(() => {
  documents.clear();
});

// eslint-disable-next-line @typescript-eslint/no-explicit-any
function out(result: { content: Array<{ type: string; text: string }> }): any {
  return JSON.parse(result.content[0]!.text);
}

const pin = (number: string, name: string, type = "Passive") => ({ number, name, type });

function demoBoard() {
  return {
    title: "Receipt session board",
    components: [
      {
        ref: "U1",
        value: "MCU",
        footprint: "Package_SO:SOIC-8_3.9x4.9mm_P1.27mm",
        x: 40,
        y: 20,
        pins: [
          pin("1", "VCC", "PowerInput"),
          pin("2", "IO1"),
          pin("3", "IO2"),
          pin("4", "IO3"),
          pin("5", "IO4"),
          pin("6", "IO5"),
          pin("7", "IO6"),
          pin("8", "GND", "PowerInput"),
        ],
      },
      { ref: "C1", value: "100nF", footprint: "Capacitor_SMD:C_0805_2012Metric", x: 25, y: 25, pins: [pin("1", "1"), pin("2", "2")] },
      { ref: "C2", value: "1uF", footprint: "Capacitor_SMD:C_0805_2012Metric", x: 55, y: 25, pins: [pin("1", "1"), pin("2", "2")] },
      { ref: "J1", value: "HDR", footprint: "Connector_PinHeader_2.54mm:PinHeader_1x04_P2.54mm_Vertical", x: 70, y: 20, pins: [pin("1", "1"), pin("2", "2"), pin("3", "3"), pin("4", "4")] },
    ],
    nets: {
      VCC: ["U1.1", "C1.1", "J1.1"],
      GND: ["U1.8", "C1.2", "C2.2", "J1.2"],
      SIG1: ["U1.2", "C2.1"],
      SIG2: ["U1.3", "J1.3"],
      SIG3: ["U1.4", "J1.4"],
    },
  };
}

describe("ReceiptSession — auto-wraps mutations and hands the agent a verdict", () => {
  it("records the double-route, gives a verdict instead of {document_id}, and reverify detects drift", async () => {
    const board = demoBoard();
    const created = out(await createSchematic(board));
    const id = created.document_id as string;
    out(await placeComponents({ document_id: id, board_width: 50, board_height: 35 }));

    const drc = async (docId: string): Promise<DrcSnapshot> =>
      out(await runDrc({ document_id: docId, detail: "full", sample_size: 500 }));

    const session = new ReceiptSession(
      id,
      { title: board.title, components: board.components.length, nets: Object.keys(board.nets), unconnectedPins: created.unconnected_pins },
      { drc, build: { version: "0.9.4", sha: "test" } },
    );

    // First route: real work. The agent gets a verdict, not a bare id.
    const r1 = await session.record("route_nets", { document_id: id }, () => routeNets({ document_id: id }));
    expect(r1.view.document_id).toBe(id);
    expect(r1.view.credited).toBeGreaterThanOrEqual(5);
    expect(r1.view.verdict === "improved" || r1.view.verdict === "improved-with-regressions").toBe(true);
    // The wrapped mutator's own return is preserved alongside the verdict.
    expect(out(r1.result as never).document_id ?? id).toBeTruthy();

    // reverify immediately after recording: the board still matches the entry.
    const v1 = await session.reverify();
    expect(v1.ok).toBe(true);
    expect(v1.stored).toBe(v1.recomputed);

    // Second route: the silent catastrophe — surfaced loudly.
    const r2 = await session.record("route_nets", { document_id: id }, () => routeNets({ document_id: id }));
    expect(r2.view.verdict).toBe("regression");
    expect(r2.view.shortsIntroduced).toBeGreaterThan(0);
    expect(r2.view.headline).toMatch(/REGRESSION/);
    expect(r2.view.headline).toMatch(/short/i);
    // pre-existing footprint faults are reported as not-its-doing, never as blame.
    expect(r2.view.preExisting).toBeGreaterThan(0);
    expect(r2.view.headline).toMatch(/untouched/);

    // The ledger has both entries.
    const receipt = session.receipt();
    expect(receipt.entries.length).toBe(2);
    expect(receipt.board.title).toBe(board.title);

    // ---- reverify catches drift: mutate the board WITHOUT recording. ----
    await routeNets({ document_id: id }); // a third route, off the books
    const drift = await session.reverify();
    expect(drift.ok).toBe(false); // the live board no longer matches the last entry
    expect(drift.stored).not.toBe(drift.recomputed);
  });
});
