import { describe, it, expect } from "vitest";
import type { SketchSegment2D } from "@vcad/ir";
import { computeSketchBounds } from "../sketch-math.js";

describe("computeSketchBounds", () => {
  it("returns null for empty segment list", () => {
    expect(computeSketchBounds([])).toBeNull();
  });

  it("computes bounds for line segments", () => {
    const segs: SketchSegment2D[] = [
      { type: "Line", start: { x: 0, y: 0 }, end: { x: 10, y: 0 } },
      { type: "Line", start: { x: 10, y: 0 }, end: { x: 10, y: 5 } },
      { type: "Line", start: { x: 10, y: 5 }, end: { x: -2, y: 5 } },
      { type: "Line", start: { x: -2, y: 5 }, end: { x: 0, y: 0 } },
    ];
    expect(computeSketchBounds(segs)).toEqual({
      minU: -2,
      maxU: 10,
      minV: 0,
      maxV: 5,
    });
  });

  it("includes the full enclosing circle for a closed loop of arcs", () => {
    // Full circle radius 7 around (3, 4), split into 4 quarter arcs.
    const cx = 3;
    const cy = 4;
    const r = 7;
    const segs: SketchSegment2D[] = [
      {
        type: "Arc",
        start: { x: cx + r, y: cy },
        end: { x: cx, y: cy + r },
        center: { x: cx, y: cy },
        ccw: true,
      },
      {
        type: "Arc",
        start: { x: cx, y: cy + r },
        end: { x: cx - r, y: cy },
        center: { x: cx, y: cy },
        ccw: true,
      },
      {
        type: "Arc",
        start: { x: cx - r, y: cy },
        end: { x: cx, y: cy - r },
        center: { x: cx, y: cy },
        ccw: true,
      },
      {
        type: "Arc",
        start: { x: cx, y: cy - r },
        end: { x: cx + r, y: cy },
        center: { x: cx, y: cy },
        ccw: true,
      },
    ];
    const b = computeSketchBounds(segs)!;
    expect(b.minU).toBeCloseTo(cx - r);
    expect(b.maxU).toBeCloseTo(cx + r);
    expect(b.minV).toBeCloseTo(cy - r);
    expect(b.maxV).toBeCloseTo(cy + r);
  });

  it("only extends to cardinals the arc actually sweeps through", () => {
    // Quarter arc from (+r, 0) CCW to (0, +r), centered at origin, r=5.
    // It crosses the +V cardinal (90°) but not -U, -V, or +U (start).
    const segs: SketchSegment2D[] = [
      {
        type: "Arc",
        start: { x: 5, y: 0 },
        end: { x: 0, y: 5 },
        center: { x: 0, y: 0 },
        ccw: true,
      },
    ];
    const b = computeSketchBounds(segs)!;
    expect(b.minU).toBeCloseTo(0);
    expect(b.maxU).toBeCloseTo(5);
    expect(b.minV).toBeCloseTo(0);
    expect(b.maxV).toBeCloseTo(5);
  });
});
