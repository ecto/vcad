import { describe, it, expect } from "vitest";
import { snapshotFromViolations } from "../adapter.js";
import { buildEntry } from "../engine.js";
import type { DrcViolation } from "../types.js";

const v = (rule: string, message: string, severity = "Error"): DrcViolation => ({
  rule,
  severity,
  message,
  position: { x: 1, y: 1 },
  actual: 0,
  required: 0.2,
});

describe("snapshotFromViolations — flat DRC list → DrcSnapshot", () => {
  it("aggregates byRule and splits errors/warnings", () => {
    const snap = snapshotFromViolations([
      v("Clearance", "a"),
      v("Clearance", "b"),
      v("Short", "c"),
      v("MinTraceWidth", "d", "Warning"),
    ]);
    expect(snap.violations).toBe(4);
    expect(snap.errors).toBe(3);
    expect(snap.warnings).toBe(1);
    expect(snap.byRule).toEqual({ Clearance: 2, Short: 1, MinTraceWidth: 1 });
    expect(snap.details?.length).toBe(4); // full coverage in-browser
  });

  it("feeds buildEntry to a correct verdict (the in-app recorder path)", () => {
    const before = snapshotFromViolations([]);
    const after = snapshotFromViolations([
      v("Short", "Short: nets 'A' and 'B' are connected by copper"),
    ]);
    const e = buildEntry({ tool: "autoroute", args: {}, before, after }, 0);
    expect(e.verdict).toBe("regression");
    expect(e.tally.shortsIntroduced).toBe(1);
    expect(e.coverage).toBe("full");
  });
});
