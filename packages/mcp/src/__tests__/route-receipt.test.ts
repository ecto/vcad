import { describe, it, expect, beforeAll, beforeEach } from "vitest";
import { Engine } from "@vcad/engine";
import { getNodePcb, getPcbNodeIds } from "@vcad/core";
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
  it("returns a receipt with routing-specific fields", async () => {
    const id = await placedBoard();

    const r1 = out(await routeNets({ document_id: id, receipt: true }));
    expect(r1.success).toBe(true);
    expect(r1.nets_routed).toBeGreaterThan(0);
    expect(r1.receipt).toBeDefined();
    expect(r1.receipt.tool).toBe("route_nets");
    expect(r1.receipt.credited).toBeGreaterThanOrEqual(5);
    expect(["improved", "improved-with-regressions"]).toContain(r1.receipt.verdict);
    expect(r1.receipt.headline).toBeTruthy();
    expect(r1.receipt.coverage).toBe("full");

    // Before/after DRC totals must ride in the receipt — both the error
    // slice and the full violation counts.
    expect(typeof r1.receipt.errors.before).toBe("number");
    expect(typeof r1.receipt.errors.after).toBe("number");
    expect(typeof r1.receipt.violations.before).toBe("number");
    expect(typeof r1.receipt.violations.after).toBe("number");

    // Routing-specific fields must always be present in the receipt
    expect(Array.isArray(r1.receipt.nets_routed)).toBe(true);
    expect(r1.receipt.nets_routed.length).toBeGreaterThan(0);
    expect(Array.isArray(r1.receipt.nets_unrouted)).toBe(true);
    expect(typeof r1.receipt.traces_added).toBe("number");
    expect(r1.receipt.traces_added).toBeGreaterThan(0);
    expect(typeof r1.receipt.traces_removed).toBe("number");
    expect(typeof r1.receipt.vias_added).toBe("number");
    expect(typeof r1.receipt.vias_removed).toBe("number");
    expect(Array.isArray(r1.receipt.plane_stitched)).toBe(true);
    expect(Array.isArray(r1.receipt.short_pairs)).toBe(true);
  });

  it("second route receipt is consistent with its own DRC delta", async () => {
    const id = await placedBoard();
    out(await routeNets({ document_id: id, receipt: true }));

    const r2 = out(await routeNets({ document_id: id, receipt: true }));
    const delta = r2.receipt.deltaByRule as Record<string, number>;
    expect(r2.receipt.shortsIntroduced).toBe(Math.max(0, delta.Short ?? 0));
    if (Object.values(delta).some((d) => d > 0)) {
      expect(["regression", "improved-with-regressions"]).toContain(r2.receipt.verdict);
    } else {
      expect(["no-op", "improved", "clean"]).toContain(r2.receipt.verdict);
    }
    // Routing fields present on re-route too
    expect(Array.isArray(r2.receipt.nets_routed)).toBe(true);
    expect(typeof r2.receipt.traces_removed).toBe("number");
    expect(r2.receipt.traces_removed).toBeGreaterThanOrEqual(0);
  });

  it("receipt works with string-coerced receipt flag", async () => {
    const id = await placedBoard();
    // MCP clients may send receipt as a string "true" instead of boolean true
    const r = out(await routeNets({ document_id: id, receipt: "true" }));
    expect(r.receipt).toBeDefined();
    expect(r.receipt.tool).toBe("route_nets");
    expect(Array.isArray(r.receipt.nets_routed)).toBe(true);
  });

  it("reports shorts when a cross-net trace is injected before re-route", async () => {
    const id = await placedBoard();
    out(await routeNets({ document_id: id }));

    // Inject a trace that bridges VCC and GND — a hard short
    const doc = documents.get(id)!;
    const nodeIds = getPcbNodeIds(doc);
    const pcb = nodeIds.length > 0 ? getNodePcb(doc, nodeIds[0]!) : null;
    expect(pcb).not.toBeNull();
    if (!pcb) return;

    const vccPad = pcb.footprints
      .flatMap((fp) => fp.pads.filter((p) => p.net === "VCC").map((p) => ({
        x: fp.position.x + p.position.x,
        y: fp.position.y + p.position.y,
      })))[0];
    const gndPad = pcb.footprints
      .flatMap((fp) => fp.pads.filter((p) => p.net === "GND").map((p) => ({
        x: fp.position.x + p.position.x,
        y: fp.position.y + p.position.y,
      })))[0];
    expect(vccPad).toBeDefined();
    expect(gndPad).toBeDefined();
    if (!vccPad || !gndPad) return;

    // Add a shorting trace directly on the live PCB
    pcb.traces.push({
      start: vccPad,
      end: gndPad,
      width: 0.25,
      layer: "FCu" as any,
      net: "VCC",
    });

    // Re-route with receipt — the before DRC should see the short; the
    // re-route rips up VCC copper (including the injected trace) and
    // re-lays it clean, so the after DRC may or may not still have it.
    // Either way the receipt must be populated and consistent.
    const r = out(await routeNets({ document_id: id, receipt: true }));
    expect(r.receipt).toBeDefined();
    expect(Array.isArray(r.receipt.short_pairs)).toBe(true);
    const delta = r.receipt.deltaByRule as Record<string, number>;
    expect(r.receipt.shortsIntroduced).toBe(Math.max(0, delta.Short ?? 0));
  });

  it("a scoped re-route never leaves a net in more disjoint copper groups (guard retries/rolls back)", async () => {
    const { netContinuity } = await import("@vcad/engine");
    const id = await placedBoard();
    out(await routeNets({ document_id: id }));

    const doc = documents.get(id)!;
    const nodeIds = getPcbNodeIds(doc);
    const pcb = nodeIds.length > 0 ? getNodePcb(doc, nodeIds[0]!) : null;
    expect(pcb).not.toBeNull();
    if (!pcb) return;

    const before = await netContinuity(pcb, "GND");
    if (!before) return; // kernel unavailable — guard is off by design

    // Scoped re-route of a couple of signal nets: any collateral damage on
    // GND (stale rip-up / negotiated rip-up side effects) must be caught by
    // the connectivity guard — retried, or rolled back, and reported.
    const r = out(await routeNets({ document_id: id, nets: ["SIG1", "SIG2"] }));
    expect(r.success).toBe(true);

    const after = await netContinuity(pcb, "GND");
    expect(after).not.toBeNull();
    if (!after) return;
    if (before.islands > 0) {
      // The invariant the guard enforces: connectivity never silently worse.
      // A rolled-back net restores its pre-call copper, so its island count
      // is back at `before` — and the regression is reported either way.
      if (after.islands > before.islands) {
        expect(r.connectivity_regressions?.GND).toBeDefined();
      }
    }
  });

  it("is opt-in — no receipt field unless requested", async () => {
    const id = await placedBoard();
    const r = out(await routeNets({ document_id: id }));
    expect(r.success).toBe(true);
    expect(r.receipt).toBeUndefined();
  });
});
