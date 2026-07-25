// FLOOD_ZONE — the pcbeval villain.
//
// The connectivity-cheese hypothesis: "just pour one giant copper zone
// over the whole board and every net is connected." This solver commits
// that crime deliberately: a small board with six footprints on three
// declared nets, zero routed traces, and one full-board GND zone with
// zero clearance overlapping every pad.
//
// It is built to pass the *shape* checks (board_envelope, component_count)
// while the copper checks must kill it: the flood shorts GND against
// every other net (DRC Short / clearance), and the unzoned nets' pads are
// disconnected islands (NetIslands / UnconnectedNet). If any P-suite task
// accepts this output, a check is broken. Kept in CI as the standing
// proof, exactly like DEFAULT_CUBE for the mech suites.

import type { Solver, SolverOutput, ToolCall } from "../solver.js";

const rect = (w: number, h: number) => ({
  number: "1",
  padType: "SMD",
  shape: { type: "Rect", width: w, height: h },
  position: { x: 0, y: 0 },
  rotation: 0,
  layers: ["FCu", "FPaste", "FMask"],
});

/** A 30×20 board: 6 two-pad footprints on nets VCC/GND/SIG, no traces,
 *  one full-board GND flood at zero clearance. */
function floodZoneBoard(): unknown {
  const nets = [
    { id: "VCC", name: "VCC" },
    { id: "GND", name: "GND" },
    { id: "SIG", name: "SIG" },
  ];
  const footprints = [];
  const netOf = ["VCC", "GND", "SIG"];
  for (let i = 0; i < 6; i++) {
    const x = 5 + (i % 3) * 9;
    const y = 5 + Math.floor(i / 3) * 9;
    footprints.push({
      ref: `U${i + 1}`,
      value: "cheese",
      footprintName: "R_0805",
      position: { x, y },
      rotation: 0,
      front: true,
      pads: [
        { ...rect(1, 1.2), number: "1", position: { x: -1, y: 0 }, net: netOf[i % 3] },
        { ...rect(1, 1.2), number: "2", position: { x: 1, y: 0 }, net: netOf[(i + 1) % 3] },
      ],
    });
  }
  return {
    version: "0.1",
    nodes: {
      "1": {
        id: 1,
        name: "FLOOD_ZONE",
        op: {
          type: "PcbBoard",
          board: {
            outline: {
              vertices: [
                { x: 0, y: 0 },
                { x: 30, y: 0 },
                { x: 30, y: 20 },
                { x: 0, y: 20 },
              ],
              thickness: 1.6,
            },
            stackup: {
              layers: [
                {
                  layer: "FCu",
                  copperThickness: 0.035,
                  dielectricThickness: 1.53,
                  dielectricEr: 4.5,
                  material: "FR4",
                },
                { layer: "BCu", copperThickness: 0.035 },
              ],
            },
            nets,
            rules: {
              defaultRules: {
                name: "Default",
                traceWidth: 0.25,
                clearance: 0.2,
                viaDiameter: 0.8,
                viaDrill: 0.4,
              },
              edgeClearance: 0.5,
              holeToHole: 0.5,
              minAnnularRing: 0.15,
              minDrill: 0.2,
            },
            footprints,
            traces: [],
            vias: [],
            zones: [
              {
                outline: [
                  { x: 0, y: 0 },
                  { x: 30, y: 0 },
                  { x: 30, y: 20 },
                  { x: 0, y: 20 },
                ],
                net: "GND",
                layer: "FCu",
                clearance: 0,
                minArea: 0,
              },
            ],
          },
        },
      },
    },
    materials: {},
    part_materials: {},
    roots: [{ root: 1, material: "default" }],
  };
}

/** Returns the flood-zone board regardless of prompt. Must fail every
 *  pcbeval copper check. */
export const floodZoneSolver: Solver = {
  id: "flood-zone",
  name: "FLOOD_ZONE (pcbeval baseline villain)",
  provider: "stub",
  params: { crime: "one giant GND zone, zero clearance, zero traces" },
  async solve(_task, _prompt, _attachments?): Promise<SolverOutput> {
    const start = performance.now();
    const wallclockSec = (performance.now() - start) / 1000;
    const toolCall: ToolCall = {
      n: 0,
      tool: "flood_zone_stub",
      args: { stub: "FLOOD_ZONE" },
      result_kind: "ok",
      wallclock_ms: wallclockSec * 1000,
    };
    return {
      vcadJson: JSON.stringify(floodZoneBoard(), null, 2),
      controlPolicy: null,
      toolCalls: [toolCall],
      tokens: { input: 0, output: 0, total: 0 },
      wallclockSec,
    };
  },
};
