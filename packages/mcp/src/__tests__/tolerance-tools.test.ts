import { describe, it, expect, beforeAll } from "vitest";
import { Engine } from "@vcad/engine";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { createServer } from "../server.js";
import { documents } from "../tools/session.js";

/**
 * End-to-end coverage for the tolerance stackup tool: the real server, the
 * real WASM kernel (CI builds it from source), fast MC sample counts.
 *
 * Skips (rather than fails) when the loaded kernel WASM predates the
 * tolerance bindings — the checked-in artifacts are refreshed only on main
 * by wasm-refresh.yml. Locally: `npm run build -w vcad-kernel-wasm` to
 * exercise these for real.
 */

const probeEngine = await Engine.init();
const wasmHasTolerance =
  typeof (
    probeEngine as unknown as {
      kernel?: { toleranceAnalyze?: unknown };
    }
  ).kernel?.toleranceAnalyze === "function";

const SPEC = {
  name: "receipt-chain",
  contributors: [
    {
      name: "pocket",
      coeff: 1.0,
      nominal: 20.0,
      tol_minus: 0.15,
      tol_plus: 0.15,
      dist: { type: "normal_from_tol", convention: { type: "three_sigma" } },
    },
    {
      name: "bushing",
      coeff: -1.0,
      nominal: 12.0,
      tol_minus: 0.1,
      tol_plus: 0.0,
      dist: { type: "uniform", lo: -0.1, hi: 0.0 },
    },
    {
      name: "shim",
      coeff: -1.0,
      nominal: "shim_nominal",
      tol_minus: 0.05,
      tol_plus: 0.05,
      dist: { type: "normal_from_tol", convention: { type: "three_sigma" } },
    },
  ],
  requirement: { name: "protrusion", lower_mm: 0.35, upper_mm: 0.75 },
};

type ToolText = { content: Array<{ type: string; text: string }> };

describe.skipIf(!wasmHasTolerance)("tolerance stackup tools", () => {
  let client: Client;

  beforeAll(async () => {
    documents.clear();
    const engine = await Engine.init();
    const server = await createServer(engine, { user: null });
    const [clientT, serverT] = InMemoryTransport.createLinkedPair();
    client = new Client(
      { name: "tolerance-tools", version: "0.0.0" },
      { capabilities: {} },
    );
    await Promise.all([client.connect(clientT), server.connect(serverT)]);
  });

  it("analyze_tolerance_stackup returns all three analyses + provisional claims", async () => {
    const res = (await client.callTool({
      name: "analyze_tolerance_stackup",
      arguments: {
        spec: SPEC,
        parameters: { shim_nominal: 7.5 },
        options: { n: 20000, seed: 314, batches: 8 },
      },
    })) as ToolText;
    const out = JSON.parse(res.content[0].text);

    // Nominal gap = 20 - 12 - 7.5 = 0.5 mm, inside [0.35, 0.75] — but the
    // worst-case extremes (0.30, 0.80) violate both limits while the
    // statistical fit stays high: the classic WC-vs-RSS story.
    expect(out.worst_case.passes).toBe(false);
    expect(out.worst_case.min_gap).toBeCloseTo(0.3, 6);
    expect(out.worst_case.max_gap).toBeCloseTo(0.8, 6);
    expect(out.rss.mean_gap).toBeGreaterThan(0.4);
    expect(out.rss.mean_gap).toBeLessThan(0.6);
    expect(out.monte_carlo.n).toBe(20000);
    expect(out.monte_carlo.fit_probability).toBeGreaterThan(0.9);
    expect(out.monte_carlo.fit_standard_error).toBeGreaterThan(0);

    // Sensitivities rank by variance share and sum to ~1.
    expect(out.sensitivities.length).toBe(3);
    const shares = out.sensitivities.map(
      (s: { variance_share: number }) => s.variance_share,
    );
    expect(shares.reduce((a: number, b: number) => a + b, 0)).toBeCloseTo(
      1,
      6,
    );

    expect(out.claim_set.schema).toBe("vcad.tolerance-claims/1");
    const names = out.claim_set.claims.map((c: { name: string }) => c.name);
    expect(names).toContain("fit_probability");
    expect(names).toContain("worst_case_margin_mm");

    expect(out.receipt_claims.length).toBe(out.claim_set.claims.length);
    for (const c of out.receipt_claims) {
      expect(c.domain).toBe("tolerance");
      expect(c.id.startsWith("tolerance.")).toBe(true);
      expect(c.basis).toBe("predicted");
    }
  });

  it("unbound named parameters fail closed", async () => {
    const res = (await client.callTool({
      name: "analyze_tolerance_stackup",
      arguments: {
        spec: SPEC,
        options: { n: 1000, batches: 4 },
      },
    })) as { isError?: boolean; content: Array<{ text: string }> };
    expect(res.isError).toBe(true);
    expect(res.content[0].text).toContain("shim_nominal");
  });
});
