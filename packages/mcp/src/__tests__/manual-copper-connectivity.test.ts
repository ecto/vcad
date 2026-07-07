/**
 * Regression tests for issue #378: copper laid by the manual routing tools
 * (add_trace / add_via) must be credited by the DRC connectivity checker.
 *
 * Root cause: the kernel connectivity graph (and several sibling DRC checks)
 * placed each pad at `footprint.position + pad.position` WITHOUT applying the
 * footprint rotation, while get_pad_positions / the ratsnest / the routers all
 * report the true rotated position. On any rotated footprint the connectivity
 * pad nodes sat at phantom locations no real copper could ever touch, so
 * UnconnectedNet never cleared for hand-routed nets.
 */
import { describe, it, expect, beforeAll, beforeEach } from "vitest";
import { Engine } from "@vcad/engine";
import {
  createSchematic,
  placeComponents,
  setPlacement,
  routeNets,
  runDrc,
  addTrace,
  addVia,
  getPadPositions,
} from "../tools/ecad.js";
import { documents } from "../tools/session.js";

beforeAll(async () => {
  await Engine.init();
});

beforeEach(() => {
  documents.clear();
});

/** Parse the single JSON text block of a tool result. */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
function out(result: { content: Array<{ type: string; text: string }> }): any {
  return JSON.parse(result.content[0].text);
}

const resistor = (ref: string, x: number) => ({
  ref,
  value: "1k",
  footprint: "Resistor_SMD:R_0805",
  x,
  y: 0,
  pins: [
    { number: "1", name: "~", type: "Passive", x: -5, y: 0 },
    { number: "2", name: "~", type: "Passive", x: 5, y: 0 },
  ],
});

const thtHeader = (ref: string, x: number) => ({
  ref,
  value: "CONN",
  footprint: "Connector_PinHeader_2.54mm:PinHeader_1x02_P2.54mm_Vertical",
  x,
  y: 0,
  pins: [
    { number: "1", name: "P1", type: "Passive", x: -5, y: 0 },
    { number: "2", name: "P2", type: "Passive", x: 5, y: 0 },
  ],
});

/** UnconnectedNet violations for `net` in a full-detail run_drc payload. */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
function unconnected(drc: any, net: string): unknown[] {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  return (drc.details ?? []).filter(
    (v: { rule: string; message: string }) =>
      v.rule === "UnconnectedNet" && v.message.includes(`'${net}'`),
  );
}

describe("manual copper is credited by connectivity (issue #378)", () => {
  it("add_trace between two pads clears UnconnectedNet (simple 2-pin net)", async () => {
    const created = out(
      await createSchematic({
        components: [resistor("R1", 0), resistor("R2", 20)],
        nets: { SIG: ["R1.2", "R2.1"] },
      }),
    );
    const id = created.document_id;
    await placeComponents({ document_id: id, board_width: 40, board_height: 20 });

    // Open before any copper.
    const before = out(await runDrc({ document_id: id, detail: "full" }));
    expect(unconnected(before, "SIG")).not.toEqual([]);

    // Hand-route with endpoints straight from get_pad_positions.
    const pads = out(await getPadPositions({ document_id: id, net: "SIG" })).pads;
    await addTrace({
      document_id: id,
      net: "SIG",
      layer: "FCu",
      points: pads.map((p: { x: number; y: number }) => ({ x: p.x, y: p.y })),
    });

    const after = out(await runDrc({ document_id: id, detail: "full" }));
    expect(unconnected(after, "SIG")).toEqual([]);
  });

  it("hand route on rotated THT footprints clears UnconnectedNet (trace on BCu + endpoint vias)", async () => {
    const created = out(
      await createSchematic({
        components: [thtHeader("J1", 0), thtHeader("J2", 30)],
        nets: { SIG: ["J1.2", "J2.2"], GND: ["J1.1", "J2.1"] },
      }),
    );
    const id = created.document_id;
    await placeComponents({ document_id: id, board_width: 50, board_height: 30 });
    // Rotation moves the off-origin pads — the #378 trigger. Before the fix
    // the connectivity graph kept these pads at their unrotated offsets.
    out(
      await setPlacement({
        document_id: id,
        placements: [
          { ref: "J1", x: 10, y: 15, rotation: 90 },
          { ref: "J2", x: 40, y: 15, rotation: 90 },
        ],
      }),
    );

    const pads = out(await getPadPositions({ document_id: id, net: "SIG" })).pads;
    expect(pads).toHaveLength(2);
    const p1 = { x: pads[0].x, y: pads[0].y };
    const p2 = { x: pads[1].x, y: pads[1].y };

    // The reporter's exact moves: a BCu trace pad-to-pad, then stitching vias
    // at both endpoints.
    await addTrace({ document_id: id, net: "SIG", layer: "BCu", points: [p1, p2] });
    await addVia({ document_id: id, net: "SIG", position: p1 });
    await addVia({ document_id: id, net: "SIG", position: p2 });

    const drc = out(await runDrc({ document_id: id, detail: "full" }));
    expect(unconnected(drc, "SIG")).toEqual([]);
  });

  it("autorouted copper is credited on rotated footprints too", async () => {
    const created = out(
      await createSchematic({
        components: [thtHeader("J1", 0), thtHeader("J2", 30)],
        nets: { SIG: ["J1.2", "J2.2"], GND: ["J1.1", "J2.1"] },
      }),
    );
    const id = created.document_id;
    await placeComponents({ document_id: id, board_width: 50, board_height: 30 });
    out(
      await setPlacement({
        document_id: id,
        placements: [
          { ref: "J1", x: 10, y: 15, rotation: 90 },
          { ref: "J2", x: 40, y: 15, rotation: 90 },
        ],
      }),
    );

    const routed = out(await routeNets({ document_id: id }));
    expect(routed.success).toBe(true);
    expect(routed.unrouted_nets ?? []).toEqual([]);

    const drc = out(await runDrc({ document_id: id, detail: "full" }));
    expect(unconnected(drc, "SIG")).toEqual([]);
    expect(unconnected(drc, "GND")).toEqual([]);
  });
});
