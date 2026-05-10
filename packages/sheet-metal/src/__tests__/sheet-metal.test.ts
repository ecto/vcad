import { describe, it, expect } from "vitest";
import {
  baseFlangeRect,
  addEdgeFlange,
  builtinBendTable,
  bendAllowance,
  bendDeduction,
  lookupKFactor,
  unfold,
  refold,
  flatPatternFromModel,
  tessellate,
  type EdgeFlangeParams,
  type Frame,
} from "../index.js";
import { newModel, pushPanel, pushBend, bfs, identityFrame } from "../model.js";

const FRAC_PI_2 = Math.PI / 2;

function defaultParams(panel = 0, edgeIndex = 0): EdgeFlangeParams {
  return {
    panel,
    edgeIndex,
    length: 25,
    angle: FRAC_PI_2,
    radius: 1.0,
    direction: "Up",
    position: "MaterialInside",
    material: "Al-soft",
  };
}

describe("bend math", () => {
  it("BA at 90° matches classic formula", () => {
    const ba = bendAllowance(FRAC_PI_2, 1.0, 0.4, 1.0);
    expect(ba).toBeCloseTo(FRAC_PI_2 * 1.4, 12);
  });

  it("BA is zero for zero angle", () => {
    expect(bendAllowance(0, 1, 0.4, 1)).toBeCloseTo(0, 15);
  });

  it("BD consistent with setback formula", () => {
    const r = 1.5,
      t = 1.0,
      k = 0.42;
    const bd = bendDeduction(FRAC_PI_2, r, k, t);
    const ba = bendAllowance(FRAC_PI_2, r, k, t);
    const ossb2 = 2 * (r + t) * Math.tan(FRAC_PI_2 / 2);
    expect(bd).toBeCloseTo(ossb2 - ba, 12);
  });
});

describe("bend table", () => {
  it("builtin has expected materials", () => {
    const t = builtinBendTable();
    for (const m of ["Al-soft", "Al-hard", "Steel-mild", "SS-304"]) {
      expect(t.rows.some((r) => r.material === m)).toBe(true);
    }
  });

  it("lookup returns provenance", () => {
    const t = builtinBendTable();
    const got = lookupKFactor(t, "Al-soft", 1.0, 1.0);
    expect(got).not.toBeNull();
    expect(got!.kFactor).toBeCloseTo(0.35, 12);
    expect(got!.source.kind).toBe("Builtin");
  });

  it("lookup unknown material returns null", () => {
    expect(lookupKFactor(builtinBendTable(), "Unobtanium", 1, 1)).toBeNull();
  });

  it("lookup falls back to closest R/t", () => {
    const got = lookupKFactor(builtinBendTable(), "Al-soft", 1.0, 1.7);
    expect(got).not.toBeNull();
    expect(got!.kFactor).toBeGreaterThan(0.34);
    expect(got!.kFactor).toBeLessThan(0.4);
  });
});

describe("base flange", () => {
  it("rect creates single panel", () => {
    const m = baseFlangeRect(100, 50, 1);
    expect(m.panels.length).toBe(1);
    expect(m.bends.length).toBe(0);
    expect(m.thickness).toBe(1);
    expect(m.panels[0]!.outline.length).toBe(4);
  });

  it("rect rejects non-positive dims", () => {
    expect(() => baseFlangeRect(-1, 50, 1)).toThrow();
    expect(() => baseFlangeRect(100, 0, 1)).toThrow();
    expect(() => baseFlangeRect(100, 50, 0)).toThrow();
  });
});

describe("edge flange", () => {
  it("adds panel and bend", () => {
    const m = baseFlangeRect(100, 50, 1);
    const t = builtinBendTable();
    const [childId, bendId] = addEdgeFlange(m, t, defaultParams());
    expect(childId).toBe(1);
    expect(bendId).toBe(0);
    expect(m.panels.length).toBe(2);
    expect(m.bends.length).toBe(1);
    expect(m.panels[0]!.incidentBends).toEqual([0]);
    expect(m.panels[1]!.incidentBends).toEqual([0]);
  });

  it("up flange at 90° lifts above parent", () => {
    const m = baseFlangeRect(100, 50, 1);
    const t = builtinBendTable();
    const [childId] = addEdgeFlange(m, t, defaultParams());
    const child = m.panels[childId]!;
    // tip at panel-local (50, 25) — distance from hinge axis (the x-axis) = 25.
    const fp = child.frameBent;
    const tip = {
      x: fp.origin.x + fp.xDir.x * 50 + fp.yDir.x * 25,
      y: fp.origin.y + fp.xDir.y * 50 + fp.yDir.y * 25,
      z: fp.origin.z + fp.xDir.z * 50 + fp.yDir.z * 25,
    };
    expect(Math.abs(tip.z)).toBeGreaterThan(1e-6);
    const distYZ = Math.hypot(tip.y, tip.z);
    expect(distYZ).toBeCloseTo(25, 9);
  });

  it("down flange mirrors up", () => {
    const t = builtinBendTable();
    const mu = baseFlangeRect(100, 50, 1);
    const md = baseFlangeRect(100, 50, 1);
    addEdgeFlange(mu, t, { ...defaultParams(), direction: "Up" });
    addEdgeFlange(md, t, { ...defaultParams(), direction: "Down" });
    const tipZ = (m: typeof mu) => {
      const f = m.panels[1]!.frameBent;
      return f.origin.z + f.xDir.z * 50 + f.yDir.z * 25;
    };
    expect(tipZ(mu) + tipZ(md)).toBeCloseTo(0, 9);
  });

  it("manual K overrides table", () => {
    const m = baseFlangeRect(100, 50, 1);
    const t = builtinBendTable();
    const [, bendId] = addEdgeFlange(m, t, { ...defaultParams(), manualK: 0.123 });
    expect(m.bends[bendId]!.kFactor).toBeCloseTo(0.123, 12);
    expect(m.bends[bendId]!.kFactorSource).toBe("manual");
  });

  it("rejects invalid inputs", () => {
    const m = baseFlangeRect(100, 50, 1);
    const t = builtinBendTable();
    expect(() => addEdgeFlange(m, t, { ...defaultParams(), panel: 99 })).toThrow();
    expect(() => addEdgeFlange(m, t, { ...defaultParams(), edgeIndex: 99 })).toThrow();
    expect(() => addEdgeFlange(m, t, { ...defaultParams(), length: 0 })).toThrow();
    expect(() => addEdgeFlange(m, t, { ...defaultParams(), angle: 4 })).toThrow();
    expect(() => addEdgeFlange(m, t, { ...defaultParams(), material: "Unobtanium" })).toThrow();
  });
});

describe("unfold/refold (the legendary involution)", () => {
  function makeUChannel() {
    const m = baseFlangeRect(100, 50, 1);
    const t = builtinBendTable();
    addEdgeFlange(m, t, defaultParams(0, 0));
    addEdgeFlange(m, t, defaultParams(0, 2));
    return m;
  }

  function frameClose(a: Frame, b: Frame, tol: number): boolean {
    const d = (u: { x: number; y: number; z: number }, v: { x: number; y: number; z: number }) =>
      Math.hypot(u.x - v.x, u.y - v.y, u.z - v.z);
    return (
      d(a.origin, b.origin) < tol &&
      d(a.xDir, b.xDir) < tol &&
      d(a.yDir, b.yDir) < tol
    );
  }

  it("round trip is identity on bent frames", () => {
    const m = makeUChannel();
    const originals = m.panels.map((p) => ({ ...p.frameBent }));
    unfold(m);
    // Garbage bent frames so refold actually rebuilds them.
    for (let i = 1; i < m.panels.length; i++) {
      m.panels[i]!.frameBent = identityFrame();
    }
    refold(m);
    for (let i = 0; i < m.panels.length; i++) {
      expect(frameClose(m.panels[i]!.frameBent, originals[i]!, 1e-9)).toBe(true);
    }
  });

  it("flat round trip is identity", () => {
    const m = makeUChannel();
    const originals = m.panels.map((p) => ({ ...p.frameFlat }));
    for (let i = 1; i < m.panels.length; i++) {
      m.panels[i]!.frameFlat = identityFrame();
    }
    unfold(m);
    for (let i = 0; i < m.panels.length; i++) {
      expect(frameClose(m.panels[i]!.frameFlat, originals[i]!, 1e-9)).toBe(true);
    }
  });

  it("no drift under repeated round-trip", () => {
    const m = makeUChannel();
    const originals = m.panels.map((p) => ({ ...p.frameBent }));
    for (let i = 0; i < 10; i++) {
      unfold(m);
      refold(m);
    }
    for (let i = 0; i < m.panels.length; i++) {
      expect(frameClose(m.panels[i]!.frameBent, originals[i]!, 1e-9)).toBe(true);
    }
  });
});

describe("flat pattern projection", () => {
  it("root sits at origin", () => {
    const m = baseFlangeRect(100, 50, 1);
    addEdgeFlange(m, builtinBendTable(), defaultParams());
    const fp = flatPatternFromModel(m);
    expect(fp.panelOutlines2D[0]![0]!.x).toBeCloseTo(0, 12);
    expect(fp.panelOutlines2D[0]![0]!.y).toBeCloseTo(0, 12);
  });

  it("child is offset by bend allowance", () => {
    const m = baseFlangeRect(100, 50, 1);
    addEdgeFlange(m, builtinBendTable(), defaultParams());
    const ba = bendAllowance(
      m.bends[0]!.angle,
      m.bends[0]!.radius,
      m.bends[0]!.kFactor,
      m.thickness,
    );
    const fp = flatPatternFromModel(m);
    expect(fp.panelOutlines2D[1]![0]!.y).toBeCloseTo(-ba, 9);
  });

  it("creases carry provenance", () => {
    const m = baseFlangeRect(100, 50, 1);
    addEdgeFlange(m, builtinBendTable(), defaultParams(0, 0));
    addEdgeFlange(m, builtinBendTable(), defaultParams(0, 2));
    const fp = flatPatternFromModel(m);
    expect(fp.creases.length).toBe(2);
    for (const c of fp.creases) expect(c.kFactorSource).not.toBeNull();
  });

  it("area includes bend strips", () => {
    const m = baseFlangeRect(100, 50, 1);
    addEdgeFlange(m, builtinBendTable(), defaultParams());
    const ba = bendAllowance(
      m.bends[0]!.angle,
      m.bends[0]!.radius,
      m.bends[0]!.kFactor,
      m.thickness,
    );
    const fp = flatPatternFromModel(m);
    const expected = 100 * 50 + 100 * 25 + 100 * ba;
    expect(fp.areaMm2).toBeCloseTo(expected, 6);
  });
});

describe("tessellation", () => {
  it("produces non-empty mesh for a base flange", () => {
    const m = baseFlangeRect(100, 50, 1);
    const mesh = tessellate(m);
    expect(mesh.positions.length).toBeGreaterThan(0);
    expect(mesh.indices.length).toBeGreaterThan(0);
    expect(mesh.normals.length).toBe(mesh.positions.length);
  });

  it("produces more triangles for a bent part than a flat one", () => {
    const flat = baseFlangeRect(100, 50, 1);
    const bent = baseFlangeRect(100, 50, 1);
    addEdgeFlange(bent, builtinBendTable(), defaultParams());
    expect(tessellate(bent).indices.length).toBeGreaterThan(
      tessellate(flat).indices.length,
    );
  });
});

describe("model graph", () => {
  it("bfs visits all panels in a tree", () => {
    const m = newModel(1);
    const mk = () => ({
      outline: [],
      holes: [],
      frameBent: identityFrame(),
      frameFlat: identityFrame(),
      incidentBends: [],
    });
    const p0 = pushPanel(m, mk());
    const p1 = pushPanel(m, mk());
    const p2 = pushPanel(m, mk());
    m.root = p0;
    pushBend(m, {
      parent: p0,
      child: p1,
      edgeParent: [
        { x: 0, y: 0 },
        { x: 1, y: 0 },
      ],
      radius: 1,
      angle: FRAC_PI_2,
      direction: "Up",
      kFactor: 0.42,
      kFactorSource: null,
    });
    pushBend(m, {
      parent: p1,
      child: p2,
      edgeParent: [
        { x: 0, y: 0 },
        { x: 1, y: 0 },
      ],
      radius: 1,
      angle: FRAC_PI_2,
      direction: "Up",
      kFactor: 0.42,
      kFactorSource: null,
    });
    const order = [...bfs(m)].map(([p]) => p);
    expect(order).toEqual([p0, p1, p2]);
  });
});
