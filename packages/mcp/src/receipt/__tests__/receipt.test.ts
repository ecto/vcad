import { describe, it, expect, beforeAll, beforeEach } from "vitest";
import { writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { Engine } from "@vcad/engine";
import { createSchematic, placeComponents, routeNets, runDrc } from "../../tools/ecad.js";
import { documents } from "../../tools/session.js";
import {
  buildReceipt,
  fingerprintSnapshot,
  renderReceiptHtml,
  renderReceiptText,
  type DrcSnapshot,
  type MutationStep,
} from "../index.js";

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

/** The demo board: an MCU + two caps + a 4-pin header. The connector's pads sit
 *  0.04mm apart (a pre-existing footprint fault no routing can fix) — the perfect
 *  control for testing that attribution never blames the router for layout. */
function demoBoard() {
  return {
    title: "Receipt demo board",
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

const drcFull = async (id: string): Promise<DrcSnapshot> =>
  out(await runDrc({ document_id: id, detail: "full", sample_size: 500 }));

describe("Receipt — wraps PCB mutations in before/after DRC and attributes blame", () => {
  it("builds a ledger that catches the silent double-route regression", async () => {
    const board = demoBoard();
    const created = out(await createSchematic(board));
    const id = created.document_id as string;

    out(await placeComponents({ document_id: id, board_width: 50, board_height: 35 }));

    // S0: pre-route baseline. S1: after one route. S2: after an accidental re-route.
    const s0 = await drcFull(id);
    out(await routeNets({ document_id: id }));
    const s1 = await drcFull(id);
    out(await routeNets({ document_id: id }));
    const s2 = await drcFull(id);

    const steps: MutationStep[] = [
      { tool: "route_nets", args: { document_id: id }, before: s0, after: s1 },
      { tool: "route_nets", args: { document_id: id }, before: s1, after: s2 },
    ];

    const receipt = buildReceipt({
      board: { title: board.title, components: board.components.length, nets: Object.keys(board.nets) },
      preflight: { unconnectedPins: created.unconnected_pins },
      build: { version: "0.9.4", sha: "test" },
      steps,
    });

    const [e1, e2] = receipt.entries;

    // ---- Entry 1: the first route does real work, with a small via cost. ----
    expect(e1!.tally.credited).toBeGreaterThanOrEqual(5); // closed the 5 unconnected nets
    expect(e1!.fixed.some((g) => g.rule === "UnconnectedNet")).toBe(true);
    expect(e1!.verdict === "improved" || e1!.verdict === "improved-with-regressions").toBe(true);

    // ---- Entry 2: the re-route is a silent catastrophe. ----
    expect(e2!.tally.shortsIntroduced).toBeGreaterThan(0); // copper-on-copper shorts
    expect(e2!.regression).toBe(true);
    expect(e2!.introduced.some((g) => g.cause === "routing")).toBe(true);
    expect(e2!.deltaTotal).toBeGreaterThan(0); // got much worse

    // ---- Attribution invariant: a footprint fault is NEVER blamed on the router. ----
    for (const e of receipt.entries) {
      for (const g of e.introduced) {
        expect(g.cause).not.toBe("footprint");
        expect(g.cause).not.toBe("placement");
      }
      // the 3 connector pad-pitch faults persist and are tagged pre-existing
      const padFaults = e.persisted.filter((g) => g.cause === "footprint");
      for (const g of padFaults) expect(g.blame).toBe("pre-existing");
    }
    expect(e2!.persisted.some((g) => g.cause === "footprint")).toBe(true);

    // ---- Coverage + deterministic fingerprint (the "didn't cheat" token). ----
    expect(e2!.coverage).toBe("full");
    const s2Again = await drcFull(id);
    expect(fingerprintSnapshot(s2Again)).toBe(e2!.fingerprint); // byte-identical re-run

    // ---- Emit the rendered artifacts and echo the text view. ----
    const html = renderReceiptHtml(receipt);
    expect(html).toContain("hard short");
    expect(html).toContain("PRE-EXISTING");
    writeFileSync(fileURLToPath(new URL("../../../receipt-demo.html", import.meta.url)), html);
    const text = renderReceiptText(receipt);
    writeFileSync(fileURLToPath(new URL("../../../receipt-demo.txt", import.meta.url)), text);
    // eslint-disable-next-line no-console
    console.log("\n" + text);
  });
});
