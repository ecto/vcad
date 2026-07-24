/**
 * simulate_strike — marshalling and the end-to-end verdict loop.
 *
 * The numerics (free-free modal math, hole-aware FEM, synthesis, FFT) live
 * in the kernel now — `crates/vcad-kernel-acoustics/src/strike.rs` carries
 * the pinned physics tests (eigenvalue roots, FEM-vs-closed-form, overtone
 * ratios, the synth→FFT round trip). What's left to test here is the seam:
 * bar geometry out of a session document, material properties out of the
 * kernel registry, kernel results into the tool payload.
 */
import { describe, expect, it } from "vitest";
import { Engine } from "@vcad/engine";
import { cents, simulateStrike } from "../tools/acoustics.js";
import { sheetMetalCreate } from "../tools/sheet-metal.js";

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
    // Center strike: the antisymmetric 2.76·f₁ partial is suppressed, so
    // mode 2 doesn't survive the audible-gain filter.
    expect(r.modes.map((m: { n: number }) => m.n)).not.toContain(2);
    // Mode 1 rings long (cord at its nodes).
    expect(r.modes[0].t60_s).toBeGreaterThan(2);
  });

  it("parses note names through the kernel", async () => {
    const engine = await Engine.init();
    expect(engine.noteToHz("A4")).toBeCloseTo(440, 6);
    expect(engine.noteToHz("C6")).toBeCloseTo(1046.502, 2);
    expect(engine.noteToHz("F#4")).toBeCloseTo(369.994, 2);
    expect(() => engine.noteToHz("H9")).toThrow();
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

  it("returns WAV bytes and a spectrum whose dominant peak matches f₁", async () => {
    const engine = await Engine.init();
    const { mkdtempSync } = await import("node:fs");
    const { tmpdir } = await import("node:os");
    const { join } = await import("node:path");
    const prev = process.env.VCAD_MCP_EXPORT_DIR;
    process.env.VCAD_MCP_EXPORT_DIR = mkdtempSync(join(tmpdir(), "vcad-strike-"));
    try {
      runWavCase(engine);
    } finally {
      if (prev === undefined) delete process.env.VCAD_MCP_EXPORT_DIR;
      else process.env.VCAD_MCP_EXPORT_DIR = prev;
    }
  });

  function runWavCase(engine: Engine) {
    const result = simulateStrike(
      {
        bar: { length_mm: 125.6, width_mm: 25, thickness_mm: 3.175, material: "6061" },
        duration_s: 1,
        wav_filename: "strike-test.wav",
      },
      engine,
    );
    const r = JSON.parse(result.content[0].text);
    expect(r.wav.bytes).toBe(44 + 2 * 44100);
    const dominant = r.spectrum_peaks.reduce(
      (b: { hz: number; db: number }, p: { hz: number; db: number }) =>
        p.db > b.db ? p : b,
    );
    expect(Math.abs(cents(dominant.hz, r.physics.f1_fem_with_holes_hz))).toBeLessThan(2);
  }
});
