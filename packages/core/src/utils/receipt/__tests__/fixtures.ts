/** Deterministic DRC snapshot fixtures, shaped from real captured run_drc output
 *  of the demo board (MCU + 2 caps + 1×4 header). They reproduce the narrative
 *  — clean first route, then an accidental re-route that stacks copper into
 *  shorts — independent of any kernel build's routing behavior. Not a test file
 *  (no .test suffix), so vitest does not collect it. */

import type { DrcSnapshot, DrcViolation } from "../types.js";

export const mk = (rule: string, message: string, x: number, y: number): DrcViolation => ({
  rule,
  severity: "Error",
  message,
  position: { x, y },
});

export const snap = (viols: DrcViolation[], byRule: Record<string, number>): DrcSnapshot => ({
  violations: viols.length,
  errors: viols.length,
  warnings: 0,
  byRule,
  details: viols,
  sample: viols,
  sampleCapped: false,
});

// 3 connector pad-to-pad faults — the part's own pin pitch, present at every stage.
const footprint = [
  mk("Clearance", "Clearance violation: pad J1.1 net 'VCC' to pad J1.2 net 'GND': 0.040mm < 0.200mm", 11.25, 21.52),
  mk("Clearance", "Clearance violation: pad J1.2 net 'GND' to pad J1.3 net 'SIG2': 0.040mm < 0.200mm", 11.25, 24.06),
  mk("Clearance", "Clearance violation: pad J1.3 net 'SIG2' to pad J1.4 net 'SIG3': 0.040mm < 0.200mm", 11.25, 26.6),
];

// 5 unrouted nets, pre-route.
const unconnected = ["VCC", "GND", "SIG1", "SIG2", "SIG3"].map((n, i) =>
  mk("UnconnectedNet", `Unconnected net '${n}': pads split across 2 disjoint copper groups`, 8.55, 9 + i),
);

// 1 via the first route drops near the connector holes.
const via = mk("HoleToHole", "Hole-to-hole spacing -0.700mm < 0.500mm", 11.25, 20.25);

// 10 hard shorts + 8 trace-clearance faults the re-route stacks on (copper on copper).
const shortPairs: Array<[string, string]> = [
  ["SIG1", "GND"], ["SIG1", "SIG3"], ["SIG1", "VCC"], ["SIG1", "SIG2"], ["GND", "SIG3"],
  ["GND", "VCC"], ["GND", "SIG2"], ["SIG3", "VCC"], ["SIG3", "SIG2"], ["VCC", "SIG2"],
];
const shorts = shortPairs.map(([a, b], i) =>
  mk("Short", `Short: nets '${a}' and '${b}' are connected by copper`, 9.675 + i * 0.05, 10.305),
);
const traceClears = Array.from({ length: 8 }, (_, i) =>
  mk("Clearance", `Clearance violation: trace net 'SIG${(i % 3) + 1}' to net 'VCC': 0.000mm < 0.200mm`, 14 + i, 9.5),
);

/** Pre-route: 3 footprint faults + 5 unrouted nets. */
export const S0 = snap([...footprint, ...unconnected], { Clearance: 3, UnconnectedNet: 5 });
/** After one route: nets connected, footprint faults remain, one via dropped. */
export const S1 = snap([...footprint, via], { Clearance: 3, HoleToHole: 1 });
/** After an accidental re-route: 10 shorts + 8 trace clearances stacked on. */
export const S2 = snap(
  [...footprint, via, ...shorts, ...traceClears],
  { Clearance: 11, HoleToHole: 1, Short: 10 },
);
/** S2 plus one more short — an off-the-books mutation, for drift detection. */
export const S2_drifted = snap(
  [...footprint, via, ...shorts, ...traceClears, mk("Short", "Short: nets 'VCC' and 'IO1' are connected by copper", 30, 30)],
  { Clearance: 11, HoleToHole: 1, Short: 11 },
);
