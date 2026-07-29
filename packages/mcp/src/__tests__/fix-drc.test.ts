/**
 * fix_drc — run DRC and auto-apply the mechanically-safe subset of fixes.
 *
 * Contract under test: each safe fixer (via dedupe, plane stitching, edge
 * nudge, per-net reroute) resolves its seeded violation; every fix is
 * delta-verified (a fix that would introduce a violation is reverted); and
 * design-decision rules (Short, …) are never touched — they come back in
 * `skipped` with a reason. The receipt always carries before/after counts.
 */
import { describe, it, expect, beforeAll, beforeEach } from "vitest";
import { Engine } from "@vcad/engine";
import { getNodePcb, getPcbNodeIds } from "@vcad/core";
import type { Pcb, Vec2 } from "@vcad/ir";
import {
  createSchematic,
  placeComponents,
  routeNets,
  addTrace,
  addVia,
  addZone,
  fixDrc,
  runDrc,
} from "../tools/ecad.js";
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

function boardPcb(id: string): Pcb {
  const doc = documents.get(id)!;
  const nodeIds = getPcbNodeIds(doc);
  const pcb = nodeIds.length > 0 ? getNodePcb(doc, nodeIds[0]!) : null;
  expect(pcb).not.toBeNull();
  return pcb!;
}

/** World positions of every netted pad on the board. */
function padWorldPositions(pcb: Pcb): Array<{ net: string; pos: Vec2 }> {
  return pcb.footprints.flatMap((fp) =>
    fp.pads
      .filter((p) => p.net)
      .map((p) => ({
        net: p.net!,
        pos: { x: fp.position.x + p.position.x, y: fp.position.y + p.position.y },
      })),
  );
}

/** First pad position on a net. */
function padOf(pcb: Pcb, net: string): Vec2 {
  const hit = padWorldPositions(pcb).find((p) => p.net === net);
  expect(hit, `no pad on net ${net}`).toBeDefined();
  return hit!.pos;
}

/** A spot on the board at least `margin` mm from every pad and trace, and
 *  `edge` mm from the outline. */
function freeSpot(pcb: Pcb, margin = 4, edge = 3): Vec2 {
  const pads = padWorldPositions(pcb);
  const candidates: Vec2[] = [];
  for (let x = edge; x <= 50 - edge; x += 2) {
    for (let y = edge; y <= 35 - edge; y += 2) {
      candidates.push({ x, y });
    }
  }
  const farEnough = (c: Vec2): boolean =>
    pads.every((p) => Math.hypot(p.pos.x - c.x, p.pos.y - c.y) > margin) &&
    pcb.traces.every(
      (t) =>
        Math.hypot(t.start.x - c.x, t.start.y - c.y) > margin &&
        Math.hypot(t.end.x - c.x, t.end.y - c.y) > margin,
    );
  const spot = candidates.find(farEnough);
  expect(spot, "no free spot on the board").toBeDefined();
  return spot!;
}

describe("fix_drc — safe fixers resolve their seeded violations", () => {
  it("HoleToHole: dedupes overlapping same-net vias, keeps one", async () => {
    const id = await placedBoard();
    out(await routeNets({ document_id: id }));
    const pcb = boardPcb(id);
    const spot = freeSpot(pcb);

    // Two same-net vias whose drills overlap (0.1mm apart, 0.4mm drills).
    out(await addVia({ document_id: id, net: "DUP", position: spot }));
    out(await addVia({ document_id: id, net: "DUP", position: { x: spot.x + 0.1, y: spot.y } }));

    const seeded = out(await runDrc({ document_id: id, detail: "full" }));
    expect(seeded.byRule.HoleToHole).toBeGreaterThanOrEqual(1);
    const viasBefore = boardPcb(id).vias.length;

    const res = out(await fixDrc({ document_id: id }));
    expect(res.success).toBe(true);
    expect(res.verified).toBe(true);
    expect(res.fixed.some((f: { rule: string; action: string }) => f.rule === "HoleToHole" && f.action === "delete_via")).toBe(true);
    expect((res.after.by_rule.HoleToHole ?? 0)).toBeLessThan(res.before.by_rule.HoleToHole);
    expect(boardPcb(id).vias.length).toBe(viasBefore - 1);
  });

  it("HoleToHole: different-net overlap is skipped, not touched", async () => {
    const id = await placedBoard();
    out(await routeNets({ document_id: id }));
    const pcb = boardPcb(id);
    const spot = freeSpot(pcb);

    out(await addVia({ document_id: id, net: "NA", position: spot }));
    out(await addVia({ document_id: id, net: "NB", position: { x: spot.x + 0.1, y: spot.y } }));
    const viasBefore = boardPcb(id).vias.length;

    const res = out(await fixDrc({ document_id: id }));
    expect(res.success).toBe(true);
    expect(res.fixed.filter((f: { rule: string }) => f.rule === "HoleToHole")).toHaveLength(0);
    expect(
      res.skipped.some(
        (s: { rule: string; reason: string }) =>
          s.rule === "HoleToHole" && /different nets/.test(s.reason),
      ),
    ).toBe(true);
    expect(boardPcb(id).vias.length).toBe(viasBefore);
  });

  it("UnstitchedPad: drops stitching vias for pads on a plane net", async () => {
    // All-SMD board: no THT drill can implicitly connect the pour, so a
    // back-layer plane with no vias leaves every SIG1 pad unstitched.
    const id = out(
      await createSchematic({
        components: [
          { ref: "C1", value: "c", footprint: "Capacitor_SMD:C_0805_2012Metric", x: 25, y: 25, pins: [pin("1", "1"), pin("2", "2")] },
          { ref: "C2", value: "c", footprint: "Capacitor_SMD:C_0805_2012Metric", x: 35, y: 25, pins: [pin("1", "1"), pin("2", "2")] },
        ],
        nets: { SIG1: ["C1.1", "C2.1"], SIG2: ["C1.2", "C2.2"] },
      }),
    ).document_id as string;
    out(await placeComponents({ document_id: id, board_width: 50, board_height: 35 }));
    // Route first (front-layer traces), THEN pour the SIG1 plane on the back
    // layer — no via reaches it, so every SIG1 SMD pad is an UnstitchedPad.
    out(await routeNets({ document_id: id }));
    out(await addZone({ document_id: id, net: "SIG1", layer: "BCu", fill_board: true }));

    const seeded = out(await runDrc({ document_id: id, detail: "full" }));
    expect(seeded.byRule.UnstitchedPad).toBeGreaterThanOrEqual(1);

    const res = out(await fixDrc({ document_id: id }));
    expect(res.success).toBe(true);
    expect(res.verified).toBe(true);
    const stitches = res.fixed.filter(
      (f: { rule: string; action: string }) => f.rule === "UnstitchedPad" && f.action === "add_via",
    );
    expect(stitches.length).toBeGreaterThanOrEqual(1);
    expect((res.after.by_rule.UnstitchedPad ?? 0)).toBeLessThan(res.before.by_rule.UnstitchedPad);
    // Stitching vias are tagged manual so route_nets never rips them.
    const pcb = boardPcb(id);
    expect(pcb.vias.some((v) => v.net === "SIG1" && v.source === "manual")).toBe(true);
  });

  it("EdgeClearance: nudges a free trace endpoint inward when a corridor exists", async () => {
    const id = await placedBoard();
    const pcb = boardPcb(id);
    // A short filler trace hugging the bottom edge (y=0.3 < 0.5 edge
    // clearance + half-width), endpoints on no pad.
    const spot = freeSpot(pcb);
    out(
      await addTrace({
        document_id: id,
        net: "EDGE",
        points: [
          { x: spot.x, y: 0.3 },
          { x: spot.x + 2, y: 0.3 },
        ],
      }),
    );
    const seeded = out(await runDrc({ document_id: id, detail: "full" }));
    expect(seeded.byRule.EdgeClearance).toBeGreaterThanOrEqual(1);

    const res = out(await fixDrc({ document_id: id }));
    expect(res.success).toBe(true);
    expect(res.verified).toBe(true);
    expect(
      res.fixed.some(
        (f: { rule: string; action: string }) => f.rule === "EdgeClearance" && f.action === "nudge_trace",
      ),
    ).toBe(true);
    expect((res.after.by_rule.EdgeClearance ?? 0)).toBeLessThan(res.before.by_rule.EdgeClearance);
    // Connectivity of the moved copper: both endpoints moved together.
    const after = boardPcb(id);
    const t = after.traces.find((x) => x.net === "EDGE")!;
    expect(t.start.y).toBeGreaterThan(0.3);
    expect(t.end.y).toBeGreaterThan(0.3);
  });

  it("UnconnectedNet: reroutes flagged nets at higher effort", async () => {
    const id = await placedBoard();
    // No routing at all — every multi-pad net is UnconnectedNet.
    const seeded = out(await runDrc({ document_id: id, detail: "full" }));
    expect(seeded.byRule.UnconnectedNet).toBeGreaterThanOrEqual(1);

    const res = out(await fixDrc({ document_id: id }));
    expect(res.success).toBe(true);
    expect(res.verified).toBe(true);
    expect(res.fixed.some((f: { action: string }) => f.action === "reroute_net")).toBe(true);
    expect((res.after.by_rule.UnconnectedNet ?? 0)).toBeLessThan(res.before.by_rule.UnconnectedNet);
  });

  it("never touches a different-net short — skipped with a reason, board unchanged", async () => {
    const id = await placedBoard();
    out(await routeNets({ document_id: id }));
    const pcb = boardPcb(id);
    out(
      await addTrace({
        document_id: id,
        net: "VCC",
        points: [padOf(pcb, "VCC"), padOf(pcb, "GND")],
      }),
    );
    const seeded = out(await runDrc({ document_id: id, detail: "full" }));
    expect(seeded.byRule.Short).toBeGreaterThanOrEqual(1);

    const res = out(await fixDrc({ document_id: id }));
    expect(res.success).toBe(true);
    expect(res.fixed.filter((f: { rule: string }) => f.rule === "Short")).toHaveLength(0);
    expect(
      res.skipped.some((s: { rule: string; reason: string }) => s.rule === "Short" && /design intent/.test(s.reason)),
    ).toBe(true);
    // The short survives — fix_drc must not silently resolve it.
    expect(res.after.by_rule.Short).toBeGreaterThanOrEqual(1);
  });

  it("dry_run plans without mutating", async () => {
    const id = await placedBoard();
    out(await routeNets({ document_id: id }));
    const pcb = boardPcb(id);
    const spot = freeSpot(pcb);
    out(await addVia({ document_id: id, net: "DUP", position: spot }));
    out(await addVia({ document_id: id, net: "DUP", position: { x: spot.x + 0.1, y: spot.y } }));
    const viasBefore = boardPcb(id).vias.length;
    const tracesBefore = boardPcb(id).traces.length;

    const res = out(await fixDrc({ document_id: id, dry_run: true }));
    expect(res.success).toBe(true);
    expect(res.dry_run).toBe(true);
    expect(res.planned.length).toBeGreaterThanOrEqual(1);
    expect(res.fixed).toHaveLength(0);
    expect(boardPcb(id).vias.length).toBe(viasBefore);
    expect(boardPcb(id).traces.length).toBe(tracesBefore);
    // Dry run: after === before (nothing was applied).
    expect(res.after.violations).toBe(res.before.violations);
  });

  it("clean board: no-op receipt with matching before/after", async () => {
    const id = await placedBoard();
    out(await routeNets({ document_id: id }));
    const res = out(await fixDrc({ document_id: id }));
    expect(res.success).toBe(true);
    expect(res.verified).toBe(true);
    expect(res.fix_count).toBe(0);
    expect(res.after.violations).toBe(res.before.violations);
  });

  it("reports progress through ctx.progress when wired", async () => {
    const id = await placedBoard();
    out(await routeNets({ document_id: id }));
    const pcb = boardPcb(id);
    const spot = freeSpot(pcb);
    out(await addVia({ document_id: id, net: "DUP", position: spot }));
    out(await addVia({ document_id: id, net: "DUP", position: { x: spot.x + 0.1, y: spot.y } }));

    const messages: string[] = [];
    const stubCtx = {
      progress: (_c: number, _t: number | undefined, message?: string) => {
        if (message) messages.push(message);
      },
    } as unknown as import("../tools/tool-def.js").ToolContext;
    const res = out(await fixDrc({ document_id: id }, stubCtx));
    expect(res.success).toBe(true);
    expect(messages.some((m) => m.includes("running initial DRC"))).toBe(true);
    expect(messages.some((m) => m.includes("verifying fix"))).toBe(true);
    expect(messages.some((m) => m.includes("running final DRC"))).toBe(true);
  });
});
