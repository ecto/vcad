import { describe, it, expect, beforeAll, beforeEach } from "vitest";
import { Engine } from "@vcad/engine";
import { createSchematic, placeComponents, routeNets } from "../tools/ecad.js";
import { documents } from "../tools/session.js";

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

async function placedBoard(): Promise<string> {
  const id = out(await createSchematic(demoBoard())).document_id as string;
  out(await placeComponents({ document_id: id, board_width: 50, board_height: 35 }));
  return id;
}

describe("route_nets receipt — the agent gets a verdict instead of {document_id}", () => {
  it("returns a receipt verdict, and a second route reports the silent short regression", async () => {
    const id = await placedBoard();

    const r1 = out(await routeNets({ document_id: id, receipt: true }));
    expect(r1.success).toBe(true);
    expect(r1.nets_routed).toBeGreaterThan(0);
    expect(r1.receipt).toBeDefined();
    expect(r1.receipt.tool).toBe("route_nets");
    expect(r1.receipt.credited).toBeGreaterThanOrEqual(5); // closed the unrouted nets
    expect(["improved", "improved-with-regressions"]).toContain(r1.receipt.verdict);
    expect(r1.receipt.headline).toBeTruthy();
    expect(r1.receipt.coverage).toBe("full");

    // A second route may or may not change the board (kernel-dependent: a clean
    // re-route is a no-op; a non-idempotent one stacks copper into shorts — see
    // issue #277). Either way the receipt must faithfully reflect what actually
    // happened: its verdict and shorts count must agree with its own DRC delta.
    // The catastrophic attribution itself is locked deterministically in
    // receipt.test.ts over captured fixtures.
    const r2 = out(await routeNets({ document_id: id, receipt: true }));
    const delta = r2.receipt.deltaByRule as Record<string, number>;
    expect(r2.receipt.shortsIntroduced).toBe(Math.max(0, delta.Short ?? 0));
    if (Object.values(delta).some((d) => d > 0)) {
      expect(["regression", "improved-with-regressions"]).toContain(r2.receipt.verdict);
    } else {
      expect(["no-op", "improved", "clean"]).toContain(r2.receipt.verdict);
    }
  });

  it("is opt-in — no receipt field unless requested", async () => {
    const id = await placedBoard();
    const r = out(await routeNets({ document_id: id }));
    expect(r.success).toBe(true);
    expect(r.receipt).toBeUndefined();
  });
});
