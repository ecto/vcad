/**
 * predict_physics: two-tier static FEA with basis-tagged receipt claims.
 * The contract under test: predict-tier passes are PROVISIONAL, verify-tier
 * passes are PASS, and both tiers agree on the physics ballpark.
 */
import { beforeAll, describe, expect, it } from "vitest";
import { Engine } from "@vcad/engine";
import { predictPhysicsTool } from "../tools/physics.js";

let engine: Engine;

beforeAll(async () => {
  engine = await Engine.init();
});

interface PhysicsOut {
  fidelity: string;
  basis: string;
  solve_ms: number;
  analysis: {
    max_displacement_mm: number;
    max_von_mises_mpa: number;
    converged: boolean;
  };
  receipt?: { claims: Array<Record<string, unknown>> };
  summary?: { verdict: string; predicted_basis: number; overall: string };
  note?: string;
}

// 80×10×10 mm aluminum cantilever, 100 N tip load, fixed root.
// Beam theory tip deflection ≈ 0.297 mm; root stress ≈ 48 MPa.
const cantilever = (extra: Record<string, unknown>) => ({
  domain_box: { min: [0, 0, 0], max: [80, 10, 10] },
  loads: [
    { region: { min: [80, 0, 0], max: [80, 10, 10] }, force: [0, 0, -100] },
  ],
  supports: [{ region: { min: [0, 0, 0], max: [0, 10, 10] } }],
  ...extra,
});

const run = (args: Record<string, unknown>): PhysicsOut =>
  JSON.parse(predictPhysicsTool(args, engine).content[0].text) as PhysicsOut;

describe("predict_physics", () => {
  it("predict tier: fast, ballpark-correct, provisional receipt", () => {
    const out = run(
      cantilever({ max_displacement_mm: 0.5, max_von_mises_mpa: 100 }),
    );
    expect(out.fidelity).toBe("predict");
    expect(out.basis).toBe("predicted");
    expect(out.analysis.converged).toBe(true);
    expect(out.analysis.max_displacement_mm).toBeGreaterThan(0.15);
    expect(out.analysis.max_displacement_mm).toBeLessThan(0.5);
    expect(out.receipt?.claims).toHaveLength(2);
    for (const c of out.receipt!.claims) {
      expect(c.basis).toBe("predicted");
      expect(c.verdict).toBe("pass");
    }
    // The load-bearing assertion: predicted passes are NOT a clean pass.
    expect(out.summary?.overall).toBe("pass");
    expect(out.summary?.verdict).toBe("provisional");
    expect(out.summary?.predicted_basis).toBe(2);
  });

  it("verify tier upgrades the same claims to a clean pass", () => {
    const out = run(
      cantilever({
        fidelity: "verify",
        max_displacement_mm: 0.5,
        max_von_mises_mpa: 100,
      }),
    );
    expect(out.basis).toBe("verified");
    expect(out.summary?.verdict).toBe("pass");
    expect(out.summary?.predicted_basis).toBe(0);
  });

  it("both tiers agree on the physics ballpark", () => {
    const p = run(cantilever({}));
    const v = run(cantilever({ fidelity: "verify" }));
    const rel =
      Math.abs(p.analysis.max_displacement_mm - v.analysis.max_displacement_mm) /
      v.analysis.max_displacement_mm;
    expect(rel).toBeLessThan(0.35);
    expect(p.note).toContain("No limits asserted");
  });

  it("violated limit fails the claim on either basis", () => {
    const out = run(cantilever({ max_displacement_mm: 0.01 }));
    expect(out.receipt?.claims[0].verdict).toBe("fail");
    expect(out.summary?.verdict).toBe("fail");
  });

  it("rejects malformed problems", () => {
    expect(() => run(cantilever({ part: "also-a-part" }))).toThrow(
      /exactly one/,
    );
    expect(() =>
      run({ domain_box: { min: [0, 0, 0], max: [10, 10, 10] }, loads: [] }),
    ).toThrow(/loads/);
  });
});
