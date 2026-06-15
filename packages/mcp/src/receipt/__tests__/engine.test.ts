import { describe, it, expect } from "vitest";
import { buildEntry, classifyCause, fingerprintSnapshot } from "../engine.js";
import type { DrcSnapshot, DrcViolation } from "../types.js";

const v = (rule: string, message: string, x = 0, y = 0): DrcViolation => ({
  rule,
  severity: "Error",
  message,
  position: { x, y },
});

const snap = (violations: DrcViolation[], byRule: Record<string, number>, opts: Partial<DrcSnapshot> = {}): DrcSnapshot => ({
  violations: opts.violations ?? violations.length,
  errors: opts.violations ?? violations.length,
  warnings: 0,
  byRule,
  details: opts.details === undefined && opts.sample === undefined ? violations : opts.details,
  sample: opts.sample,
  sampleCapped: opts.sampleCapped,
});

describe("classifyCause — every real run_drc message format", () => {
  const cases: Array<[string, string]> = [
    ["Clearance violation: pad J1.1 net 'VCC' to pad J1.2 net 'GND': 0.040mm < 0.200mm", "footprint"],
    ["Clearance violation: pad J1.1 net 'VCC' to pad J2.2 net 'GND': 0.040mm < 0.200mm", "placement"],
    ["Clearance violation: trace net 'SIG1' to net 'VCC': 0.000mm < 0.200mm", "routing"],
    ["Short: nets 'SIG1' and 'GND' are connected by copper", "routing"],
    ["Hole-to-hole spacing -0.700mm < 0.500mm", "via"],
    ["Unconnected net 'GND': pads split across 4 disjoint copper groups", "connectivity"],
    ["Pad 1 drill 0.150mm below minimum 0.200mm on J1", "footprint"],
    ["Pad 1 on J1 annular ring 0.050mm < 0.150mm", "footprint"],
    // a net literally named "trace" must NOT make a footprint fault read as routing
    ["Clearance violation: pad U1.1 net 'trace' to pad U1.2 net 'GND': 0.040mm < 0.200mm", "footprint"],
  ];
  for (const [msg, cause] of cases) {
    it(`${cause}: ${msg.slice(0, 40)}…`, () => {
      expect(classifyCause(v("X", msg)).cause).toBe(cause);
    });
  }
});

describe("buildEntry — partial coverage cannot read 'clean' on a shorted board", () => {
  it("derives regression from byRule even when the sample hides the shorts", () => {
    const pad = (n: string) => v("Clearance", `Clearance violation: pad J1.${n} net 'A' to pad J1.${Number(n) + 1} net 'B': 0.040mm < 0.200mm`, Number(n), 0);
    const before = snap([], { Clearance: 3 }, { violations: 3, sample: [pad("1"), pad("2"), pad("3")], sampleCapped: true });
    // after: 10 shorts appear in byRule, but the capped sample shows only the 3 footprint faults
    const after = snap([], { Clearance: 3, Short: 10 }, { violations: 13, sample: [pad("1"), pad("2"), pad("3")], sampleCapped: true });

    const e = buildEntry({ tool: "route_nets", args: {}, before, after }, 0);
    expect(e.verdict).toBe("regression");
    expect(e.regression).toBe(true);
    expect(e.tally.shortsIntroduced).toBe(10); // authoritative from byRule, not the sample
    expect(e.coverage).toBe("partial");
  });
});

describe("buildEntry — attribution edge cases the review caught", () => {
  it("a persisted hole-to-hole whose magnitude drifts is persisted, not credit+blame", () => {
    const hole = (mag: string) => v("HoleToHole", `Hole-to-hole spacing ${mag} < 0.500mm`, 9.675, 10.305);
    const before = snap([hole("-0.700mm")], { HoleToHole: 1 });
    const after = snap([hole("-0.690mm")], { HoleToHole: 1 });
    const e = buildEntry({ tool: "route_nets", args: {}, before, after }, 0);
    expect(e.introduced.length).toBe(0);
    expect(e.fixed.length).toBe(0);
    expect(e.persisted.reduce((n, g) => n + g.count, 0)).toBe(1);
    expect(e.verdict).toBe("no-op");
  });

  it("a persisted single-pad drill fault is pre-existing, never carried-over/blamed", () => {
    const drill = v("MinDrill", "Pad 1 drill 0.150mm below minimum 0.200mm on J1", 5, 5);
    const before = snap([drill], { MinDrill: 1 });
    const after = snap([drill], { MinDrill: 1 });
    const e = buildEntry({ tool: "route_nets", args: {}, before, after }, 0);
    expect(e.persisted[0]!.blame).toBe("pre-existing");
    expect(e.tally.preExisting).toBe(1);
    expect(e.tally.blamed).toBe(0);
  });
});

describe("fingerprintSnapshot — order-independent, board-content only", () => {
  it("ignores the emission order of the details array", () => {
    const a = v("Short", "Short: nets 'A' and 'B' are connected by copper", 1, 1);
    const b = v("Short", "Short: nets 'C' and 'D' are connected by copper", 2, 2);
    const s1 = snap([a, b], { Short: 2 });
    const s2 = snap([b, a], { Short: 2 });
    expect(fingerprintSnapshot(s1)).toBe(fingerprintSnapshot(s2));
  });

  it("differs when the board content differs", () => {
    const a = v("Short", "Short: nets 'A' and 'B' are connected by copper", 1, 1);
    const c = v("Short", "Short: nets 'A' and 'C' are connected by copper", 1, 1);
    expect(fingerprintSnapshot(snap([a], { Short: 1 }))).not.toBe(fingerprintSnapshot(snap([c], { Short: 1 })));
  });
});
