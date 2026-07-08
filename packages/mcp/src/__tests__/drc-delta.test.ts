/**
 * drc_delta — verify-on-write for the copper-mutating PCB tools.
 *
 * Field report: the old add_motor_winding returned a bare document_version for
 * a board it had just shorted in 3 places — the agent only learned from a
 * separate run_drc. These tests pin the contract that replaced it: every
 * copper mutator self-reports what it introduced, `clean` is the one-step
 * branch, and big boards get the bbox-scoped (incremental) snapshot path.
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
  addCoil,
  addNetTie,
  deleteNetTie,
  deleteTrace,
  setStackup,
  setBoardOutline,
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
 *  `edge` mm from the outline — placement is deterministic but not worth
 *  hardcoding into the test. */
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

describe("drc_delta — mutations that break the board say so", () => {
  it("add_trace bridging VCC to GND: clean=false with the short listed", async () => {
    const id = await placedBoard();
    out(await routeNets({ document_id: id }));
    const pcb = boardPcb(id);

    const res = out(
      await addTrace({
        document_id: id,
        net: "VCC",
        points: [padOf(pcb, "VCC"), padOf(pcb, "GND")],
      }),
    );
    expect(res.success).toBe(true);
    const delta = res.drc_delta;
    expect(delta).toBeDefined();
    expect(delta.clean).toBe(false);
    expect(delta.introduced).toBeGreaterThan(0);
    expect(delta.by_category.shorts).toBeGreaterThanOrEqual(1);
    // The sample is worst-first, so the short leads it — with a position.
    expect(delta.sample.length).toBeGreaterThan(0);
    expect(delta.sample[0].rule).toBe("Short");
    expect(delta.sample[0].position).toBeDefined();
    expect(delta.sample[0].message).toMatch(/VCC|GND/);
  });

  it("benign add_trace / add_via on empty board area: clean=true", async () => {
    const id = await placedBoard();
    const pcb = boardPcb(id);
    const spot = freeSpot(pcb);

    const t = out(
      await addTrace({
        document_id: id,
        net: "TEST",
        points: [spot, { x: spot.x + 1.5, y: spot.y }],
      }),
    );
    expect(t.success).toBe(true);
    expect(t.drc_delta.clean).toBe(true);
    expect(t.drc_delta.introduced).toBe(0);
    expect(t.drc_delta.by_category).toEqual({
      shorts: 0,
      clearance: 0,
      connectivity: 0,
      manufacturing: 0,
    });

    // A via ON the trace end (same net) is also benign.
    const v = out(
      await addVia({ document_id: id, net: "TEST", position: { x: spot.x + 1.5, y: spot.y } }),
    );
    expect(v.success).toBe(true);
    expect(v.drc_delta.clean).toBe(true);
  });

  it("delete_trace severing a routed net: connectivity break reported", async () => {
    const id = await placedBoard();
    out(await routeNets({ document_id: id }));
    const pcb = boardPcb(id);

    // The SIG2 segment farthest from any pad — removing it must split the
    // net's two pads into disjoint copper groups.
    const pads = padWorldPositions(pcb);
    let best = -1;
    let bestDist = -1;
    pcb.traces.forEach((t, i) => {
      if (t.net !== "SIG2") return;
      const mid = { x: (t.start.x + t.end.x) / 2, y: (t.start.y + t.end.y) / 2 };
      const d = Math.min(...pads.map((p) => Math.hypot(p.pos.x - mid.x, p.pos.y - mid.y)));
      if (d > bestDist) {
        bestDist = d;
        best = i;
      }
    });
    expect(best).toBeGreaterThanOrEqual(0);

    const res = out(await deleteTrace({ document_id: id, index: best, net: "SIG2" }));
    expect(res.success).toBe(true);
    const delta = res.drc_delta;
    expect(delta.clean).toBe(false);
    expect(delta.by_category.connectivity).toBeGreaterThanOrEqual(1);
    expect(
      delta.sample.some((v: { rule: string }) =>
        ["UnconnectedNet", "NetIslands"].includes(v.rule),
      ),
    ).toBe(true);
  });

  it("net ties: add_net_tie resolves a short, delete_net_tie re-convicts it", async () => {
    const id = await placedBoard();
    out(await routeNets({ document_id: id }));
    const pcb = boardPcb(id);

    // Deliberate VCC↔GND junction — a short until a tie declares it intended.
    out(
      await addTrace({
        document_id: id,
        net: "VCC",
        points: [padOf(pcb, "VCC"), padOf(pcb, "GND")],
      }),
    );

    // Declaring the junction intended EXEMPTS the short: resolved, clean.
    const tied = out(await addNetTie({ document_id: id, nets: ["VCC", "GND"] }));
    expect(tied.success).toBe(true);
    expect(tied.drc_delta.clean).toBe(true);
    expect(tied.drc_delta.resolved).toBeGreaterThanOrEqual(1);

    // Deleting the tie re-convicts the junction copper as a live short.
    const untied = out(await deleteNetTie({ document_id: id, nets: ["VCC", "GND"] }));
    expect(untied.success).toBe(true);
    expect(untied.drc_delta.clean).toBe(false);
    expect(untied.drc_delta.by_category.shorts).toBeGreaterThanOrEqual(1);
    expect(
      untied.drc_delta.sample.some((v: { rule: string }) => v.rule === "Short"),
    ).toBe(true);
  });

  it("set_stackup: verified no-op delta", async () => {
    const id = await placedBoard();
    const res = out(await setStackup({ document_id: id, copper_oz: 2 }));
    expect(res.success).toBe(true);
    expect(res.drc_delta.clean).toBe(true);
    expect(res.drc_delta.introduced).toBe(0);
    expect(res.drc_delta.resolved).toBe(0);
  });

  it("set_board_outline: full-board scope, growing the board is clean", async () => {
    const id = await placedBoard();
    out(await routeNets({ document_id: id }));
    const res = out(await setBoardOutline({ document_id: id, board_width: 60, board_height: 45 }));
    expect(res.success).toBe(true);
    expect(res.drc_delta).toBeDefined();
    // Outline changes re-judge every element — never region-scoped.
    expect(res.drc_delta.scope).toBe("full");
    expect(res.drc_delta.clean).toBe(true);
  });

  it("add_coil self-reports through the same contract", async () => {
    const id = await placedBoard();
    const pcb = boardPcb(id);
    // Keep the whole annulus (outer_radius 3mm + copper) clear of the edge.
    const spot = freeSpot(pcb, 8, 6);
    const res = out(
      await addCoil({
        document_id: id,
        center: spot,
        turns: 2,
        inner_radius: 1,
        outer_radius: 3,
        trace_width: 0.3,
        clearance: 0.3,
        net: "COIL",
      }),
    );
    expect(res.success).toBe(true);
    expect(res.drc_delta).toBeDefined();
    expect(res.drc_delta.clean).toBe(true);
  });
});

describe("drc_delta — incremental scope on big boards", () => {
  it(
    "past the element budget, snapshots are region-scoped and still catch shorts",
    { timeout: 120_000 },
    async () => {
      const id = await placedBoard();
      out(await routeNets({ document_id: id }));
      const pcb = boardPcb(id);

      // Inflate the board past the full-DRC element budget with a benign
      // chain of same-net filler segments on the back layer (one connected
      // island, no pads — introduces nothing).
      const y = 33;
      for (let i = 0; i < 2100; i++) {
        const x0 = 3 + i * 0.02;
        pcb.traces.push({
          start: { x: x0, y },
          end: { x: x0 + 0.02, y },
          width: 0.25,
          layer: "BCu",
          net: "FILL",
          source: "manual",
        });
      }

      // Benign via far from all copper → clean, via the region-scoped path.
      const spot = freeSpot(pcb);
      const benign = out(
        await addVia({ document_id: id, net: "T2", position: spot }),
      );
      expect(benign.success).toBe(true);
      expect(benign.drc_delta.scope).toBe("region");
      expect(benign.drc_delta.clean).toBe(true);

      // A VCC→GND bridge is a global connectivity fault — the scoped snapshot
      // must still convict it.
      const shorting = out(
        await addTrace({
          document_id: id,
          net: "VCC",
          points: [padOf(pcb, "VCC"), padOf(pcb, "GND")],
        }),
      );
      expect(shorting.success).toBe(true);
      expect(shorting.drc_delta.scope).toBe("region");
      expect(shorting.drc_delta.clean).toBe(false);
      expect(shorting.drc_delta.by_category.shorts).toBeGreaterThanOrEqual(1);
    },
  );
});
