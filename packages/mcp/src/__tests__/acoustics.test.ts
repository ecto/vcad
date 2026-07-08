/**
 * simulate_strike — modal math, FEM, synthesis, and the verdict loop.
 *
 * The receipts here are physics: exact free-free eigenvalue roots, FEM
 * converging on the closed form for a uniform bar, non-harmonic overtone
 * ratios, and a synth→FFT round trip accurate to a cent.
 */
import { describe, expect, it } from "vitest";
import { Engine } from "@vcad/engine";
import {
  cents,
  closedFormHz,
  femHz,
  freeFreeBetaL,
  modeShape,
  noteToHz,
  simulateStrike,
  spectrumPeaks,
  strikeModes,
  synthesize,
  type BarSpec,
} from "../tools/acoustics.js";
import { sheetMetalCreate } from "../tools/sheet-metal.js";

const C6_BAR: BarSpec = {
  length_mm: 125.6,
  width_mm: 25,
  thickness_mm: 3.175,
  holes_mm: [28.16, 97.44],
  hole_dia_mm: 4.2,
  modulus_gpa: 69,
  density_kg_m3: 2700,
};
const uniform: BarSpec = { ...C6_BAR, holes_mm: [], hole_dia_mm: 0 };

describe("free-free beam modal math", () => {
  it("solves the cosh·cos = 1 eigenvalue roots", () => {
    const bl = freeFreeBetaL(4);
    expect(bl[0]).toBeCloseTo(4.73004074, 6);
    expect(bl[1]).toBeCloseTo(7.85320462, 6);
    expect(bl[2]).toBeCloseTo(10.99560784, 6);
    expect(bl[3]).toBeCloseTo(14.13716549, 6);
  });

  it("puts the mode-1 nodal line at 0.2242·L — where the cord holes go", () => {
    const bl = freeFreeBetaL(1)[0];
    // φ₁ changes sign across the node.
    expect(modeShape(bl, 0.2241) * modeShape(bl, 0.2243)).toBeLessThan(0);
  });

  it("matches the spec's f₁ ≈ 16.50/L² for 6061 at 3.175 mm", () => {
    const f1 = closedFormHz(uniform, 1)[0];
    expect(f1).toBeCloseTo(16.498 / 0.1256 ** 2, 0);
  });
});

describe("hole-aware FEM", () => {
  it("reproduces the closed form on a uniform bar to sub-cent accuracy", () => {
    const closed = closedFormHz(uniform, 3);
    const fem = femHz(uniform, 3);
    for (let i = 0; i < 3; i++) {
      expect(Math.abs(cents(fem[i], closed[i]))).toBeLessThan(0.5);
    }
  });

  it("recovers the non-harmonic free-free overtone ratios", () => {
    const fem = femHz(uniform, 3);
    expect(fem[1] / fem[0]).toBeCloseTo(2.7565, 3);
    expect(fem[2] / fem[0]).toBeCloseTo(5.4039, 3);
  });

  it("nodal cord holes flatten f₁ slightly — the detune the sim exists to catch", () => {
    const plain = femHz(uniform, 1)[0];
    const holed = femHz(C6_BAR, 1)[0];
    const shift = cents(holed, plain);
    expect(shift).toBeLessThan(-1); // flat, not sharp
    expect(shift).toBeGreaterThan(-20); // and small
  });
});

describe("strike synthesis and spectrum", () => {
  it("round-trips a strike through FFT peak extraction within a cent", () => {
    const fem = femHz(C6_BAR, 6);
    const modes = strikeModes(C6_BAR, fem, 0.5, 0.5);
    const samples = synthesize(modes.filter((m) => m.gain > 1e-4), 2.5, 44100);
    const peaks = spectrumPeaks(samples, 44100);
    const dominant = peaks.reduce((b, p) => (p.db > b.db ? p : b));
    expect(Math.abs(cents(dominant.hz, fem[0]))).toBeLessThan(1);
  });

  it("suppresses the antisymmetric 2.76·f₁ partial on a center strike", () => {
    const fem = femHz(C6_BAR, 3);
    const modes = strikeModes(C6_BAR, fem, 0.5, 0.5);
    expect(modes[1].gain).toBeLessThan(0.01); // node of mode 2 at center
    const offCenter = strikeModes(C6_BAR, fem, 0.3, 0.5);
    expect(offCenter[1].gain).toBeGreaterThan(0.05);
  });

  it("mode 1 rings long (cord at its nodes); higher partials die fast", () => {
    const fem = femHz(C6_BAR, 3);
    const modes = strikeModes(C6_BAR, fem, 0.5, 0.5);
    expect(modes[0].t60_s).toBeGreaterThan(2);
    expect(modes[2].t60_s).toBeLessThan(0.5);
  });

  it("parses note names", () => {
    expect(noteToHz("A4")).toBeCloseTo(440, 6);
    expect(noteToHz("C6")).toBeCloseTo(1046.502, 2);
    expect(noteToHz("F#4")).toBeCloseTo(369.994, 2);
    expect(() => noteToHz("H9")).toThrow();
  });
});

describe("simulate_strike tool", () => {
  it("verifies a C6 bar end-to-end from an explicit spec", async () => {
    const engine = await Engine.init();
    const result = simulateStrike(
      {
        bar: {
          length_mm: 125.6,
          width_mm: 25,
          thickness_mm: 3.175,
          material: "6061",
          holes_mm: [28.16, 97.44],
          hole_dia_mm: 4.2,
        },
        note: "C6",
        tolerance_cents: 10,
      },
      engine,
    );
    const r = JSON.parse(result.content[0].text);
    expect(r.bar.material).toBe("al-hard"); // registry, not this file
    expect(r.verdict.pass).toBe(true);
    // Rounding (−1.1¢) + hole shift (≈−5¢) — flat but inside tolerance.
    expect(r.verdict.cents_error).toBeLessThan(0);
    expect(r.verdict.cents_error).toBeGreaterThan(-10);
    expect(r.physics.hole_shift_cents).toBeLessThan(-1);
    // Holes shift mode 2 more than mode 1, so the ratio lands just under
    // the uniform-bar 2.7565.
    expect(r.physics.overtone_ratios[1]).toBeCloseTo(2.7565, 1);
  });

  it("reads bar geometry from a flat sheet-metal session and rejects bent parts", async () => {
    const engine = await Engine.init();
    const circle = (cx: number, cy: number, r: number) =>
      Array.from({ length: 24 }, (_, i) => {
        const a = -(2 * Math.PI * i) / 24;
        return { x: cx + r * Math.cos(a), y: cy + r * Math.sin(a) };
      });
    const created = JSON.parse(
      sheetMetalCreate(
        {
          outline: [
            { x: 0, y: 0 },
            { x: 125.6, y: 0 },
            { x: 125.6, y: 25 },
            { x: 0, y: 25 },
          ],
          holes: [circle(28.16, 12.5, 2.1), circle(97.44, 12.5, 2.1)],
          thickness: 3.175,
          material: "6061",
        },
        engine,
      ).content[0].text,
    );
    const r = JSON.parse(
      simulateStrike(
        { document_id: created.document_id, note: "C6" },
        engine,
      ).content[0].text,
    );
    expect(r.bar.length_mm).toBeCloseTo(125.6, 6);
    expect(r.bar.holes_mm).toHaveLength(2);
    expect(r.verdict.pass).toBe(true);

    const bent = JSON.parse(
      sheetMetalCreate(
        {
          width: 100,
          depth: 50,
          thickness: 3.175,
          material: "al-soft",
          flanges: [{ edge_index: 0, length: 20 }],
        },
        engine,
      ).content[0].text,
    );
    expect(() =>
      simulateStrike({ document_id: bent.document_id }, engine),
    ).toThrow(/flat free-free bars/);
  });
});
