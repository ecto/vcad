/**
 * length_match_traces — group length matching for timing-critical nets.
 *
 * Pins the contract: shorter nets in the match group grow clearance-checked
 * meanders until they reach the longest net (or an explicit target) within
 * tolerance; check_only measures without touching copper; untunable nets are
 * reported with a reason instead of guessed at; mutating runs replace the
 * net's copper in the session document and carry drc_delta.
 */
import { describe, it, expect, beforeAll, beforeEach } from "vitest";
import { Engine, matchTraceLengths } from "@vcad/engine";
import { getNodePcb, getPcbNodeIds } from "@vcad/core";
import type { Document, Pcb } from "@vcad/ir";
import {
  createSchematic,
  placeComponents,
  addTrace,
  lengthMatchTraces,
} from "../tools/ecad.js";
import { documents } from "../tools/session.js";

beforeAll(async () => {
  await Engine.init();
});
beforeEach(() => {
  documents.clear();
});

// The checked-in kernel WASM is only refreshed on main (wasm-refresh.yml), so a
// working tree carrying this feature but running the stale artifact can't
// exercise the binding. CI builds the WASM from source and runs the suite.
const kernelHasLengthMatch = await (async () => {
  try {
    const wasm = await import("@vcad/kernel-wasm");
    return typeof (wasm as Record<string, unknown>).ecadLengthMatch === "function";
  } catch {
    return false;
  }
})();

// eslint-disable-next-line @typescript-eslint/no-explicit-any
function out(result: { content: Array<{ type: string; text: string }> }): any {
  return JSON.parse(result.content[0]!.text);
}

function docPcb(id: string): Pcb {
  const doc = documents.get(id) as Document;
  return getNodePcb(doc, getPcbNodeIds(doc)[0]!)!;
}

const pin = (number: string, name: string) => ({ number, name, type: "Passive" });

/** A bare two-connector board with LONG/SHORT nets we hand-route. */
async function boardWithTwoNets(): Promise<string> {
  const id = out(
    await createSchematic({
      components: [
        {
          ref: "J1",
          value: "HDR",
          footprint: "Connector_PinHeader_2.54mm:PinHeader_1x02_P2.54mm_Vertical",
          x: 10,
          y: 10,
          pins: [pin("1", "1"), pin("2", "2")],
        },
        {
          ref: "J2",
          value: "HDR",
          footprint: "Connector_PinHeader_2.54mm:PinHeader_1x02_P2.54mm_Vertical",
          x: 60,
          y: 10,
          pins: [pin("1", "1"), pin("2", "2")],
        },
      ],
      nets: {
        LONG: ["J1.1", "J2.1"],
        SHORT: ["J1.2", "J2.2"],
      },
    }),
  ).document_id as string;
  out(await placeComponents({ document_id: id, board_width: 80, board_height: 50 }));
  // Hand-route both nets as clean straight chains of known lengths.
  out(
    await addTrace({
      document_id: id,
      net: "LONG",
      points: [
        { x: 10, y: 35 },
        { x: 60, y: 35 },
      ],
    }),
  );
  out(
    await addTrace({
      document_id: id,
      net: "SHORT",
      points: [
        { x: 15, y: 20 },
        { x: 45, y: 20 },
      ],
    }),
  );
  return id;
}

describe.skipIf(!kernelHasLengthMatch)("length_match_traces", () => {
  it("check_only reports lengths and deviations without touching copper", async () => {
    const id = await boardWithTwoNets();
    const before = docPcb(id).traces.length;

    const r = out(
      await lengthMatchTraces({
        document_id: id,
        nets: ["LONG", "SHORT"],
        check_only: true,
      }),
    );
    expect(r.success).toBe(true);
    expect(r.check_only).toBe(true);
    expect(r.target_length_mm).toBeCloseTo(50, 3);
    expect(r.all_matched).toBe(false);
    const short = r.nets.find((n: { net: string }) => n.net === "SHORT");
    expect(short.length_before_mm).toBeCloseTo(30, 3);
    expect(short.matched).toBe(false);
    expect(docPcb(id).traces.length).toBe(before);
  });

  it("meanders the short net up to the longest and commits the copper", async () => {
    const id = await boardWithTwoNets();

    const r = out(
      await lengthMatchTraces({
        document_id: id,
        nets: ["LONG", "SHORT"],
        tolerance: 0.5,
        max_amplitude: 3,
        spacing: 2,
      }),
    );
    expect(r.success).toBe(true);
    expect(r.all_matched).toBe(true);
    expect(r.nets_tuned).toBe(1);
    expect(r.drc_delta).toBeDefined();

    const short = r.nets.find((n: { net: string }) => n.net === "SHORT");
    expect(short.tuned).toBe(true);
    expect(short.length_after_mm).toBeGreaterThan(49);
    expect(short.length_after_mm).toBeLessThan(51);

    // The document's SHORT copper was replaced with the meandered polyline.
    const pcb = docPcb(id);
    const shortTraces = pcb.traces.filter((t) => t.net === "SHORT");
    expect(shortTraces.length).toBeGreaterThan(1);
    const total = shortTraces.reduce(
      (s, t) => s + Math.hypot(t.end.x - t.start.x, t.end.y - t.start.y),
      0,
    );
    expect(total).toBeCloseTo(50, 0);
    // LONG untouched.
    expect(pcb.traces.filter((t) => t.net === "LONG").length).toBe(1);
  });

  it("honors an explicit target_length", async () => {
    const id = await boardWithTwoNets();
    const r = out(
      await lengthMatchTraces({
        document_id: id,
        nets: ["SHORT"],
        target_length: 40,
        tolerance: 0.5,
        max_amplitude: 3,
        spacing: 2,
      }),
    );
    expect(r.target_length_mm).toBeCloseTo(40, 3);
    expect(r.all_matched).toBe(true);
  });

  it("reports unroutable/unknown nets with a reason", async () => {
    const id = await boardWithTwoNets();
    const r = out(
      await lengthMatchTraces({
        document_id: id,
        nets: ["LONG", "NOPE"],
      }),
    );
    expect(r.all_matched).toBe(false);
    const nope = r.nets.find((n: { net: string }) => n.net === "NOPE");
    expect(nope.tuned).toBe(false);
    expect(nope.skip_reason).toMatch(/no routed traces/);
  });

  it("rejects an empty net group and bad style", async () => {
    const id = await boardWithTwoNets();
    const empty = await lengthMatchTraces({ document_id: id, nets: [] });
    expect(empty.isError).toBe(true);
    const bad = await lengthMatchTraces({
      document_id: id,
      nets: ["LONG"],
      style: "squiggle",
    });
    expect(bad.isError).toBe(true);
  });

  it("engine wrapper returns per-net replacement traces as data", async () => {
    const id = await boardWithTwoNets();
    const pcb = docPcb(id);
    const before = JSON.stringify(pcb);
    const result = await matchTraceLengths(pcb, ["LONG", "SHORT"], {
      tolerance: 0.5,
      max_amplitude: 3,
      spacing: 2,
    });
    expect(result).not.toBeNull();
    expect(result!.all_matched).toBe(true);
    const short = result!.nets.find((n) => n.net === "SHORT")!;
    expect(short.tuned).toBe(true);
    expect(short.new_traces!.length).toBeGreaterThan(1);
    // Pure: the input board is untouched.
    expect(JSON.stringify(pcb)).toBe(before);
  });

  it("engine wrapper rejects an unrecognized style instead of defaulting", async () => {
    const id = await boardWithTwoNets();
    const pcb = docPcb(id);
    // A direct engine caller bypasses the MCP-layer style validation; the WASM
    // binding itself must refuse a typo rather than silently pick Trombone.
    const result = await matchTraceLengths(pcb, ["LONG", "SHORT"], {
      // @ts-expect-error deliberately invalid style
      style: "sawTooth",
    });
    expect(result).toBeNull();
  });
});
