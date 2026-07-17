import { describe, it, expect, beforeAll } from "vitest";
import { Engine } from "@vcad/engine";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { createServer } from "../server.js";
import { documents } from "../tools/session.js";

/**
 * End-to-end coverage for the charged-particle optics tools: the real
 * server, the real WASM kernel (CI builds it from source, so this guards
 * the full vcad-kernel-particle → wasm-bindgen → engine → tool chain),
 * coarse grids so the suite stays fast.
 *
 * Skips (rather than fails) when the loaded kernel WASM predates the
 * particle bindings — the checked-in artifacts are refreshed only on main
 * by wasm-refresh.yml. Locally: `npm run build -w vcad-kernel-wasm` to
 * exercise these for real.
 */

const probeEngine = await Engine.init();
const wasmHasParticle =
  typeof (
    probeEngine as unknown as {
      kernel?: { particleSimulate?: unknown };
    }
  ).kernel?.particleSimulate === "function";

const SPEC = {
  chamber_radius_mm: 120,
  chamber_half_height_mm: 120,
  rings: [
    {
      ring_radius_mm: 40,
      z_mm: "ring_z",
      wire_radius_mm: 4,
      potential_v: "cathode_v",
      ampere_turns: "shield_at",
    },
    {
      ring_radius_mm: 40,
      z_mm: "ring_z_neg",
      wire_radius_mm: 4,
      potential_v: "cathode_v",
      ampere_turns: "shield_at_neg",
    },
  ],
};

const PARAMS = {
  ring_z: 22,
  ring_z_neg: -22,
  cathode_v: -20000,
  shield_at: 20000,
  shield_at_neg: -20000,
};

type ToolText = { content: Array<{ type: string; text: string }> };

describe.skipIf(!wasmHasParticle)("charged-particle optics tools", () => {
  let client: Client;

  beforeAll(async () => {
    documents.clear();
    const engine = await Engine.init();
    const server = await createServer(engine, { user: null });
    const [clientT, serverT] = InMemoryTransport.createLinkedPair();
    client = new Client(
      { name: "particle-tools", version: "0.0.0" },
      { capabilities: {} },
    );
    await Promise.all([client.connect(clientT), server.connect(serverT)]);
  });

  it("simulate_charged_particles returns stats + provisional claims", async () => {
    const res = (await client.callTool({
      name: "simulate_charged_particles",
      arguments: {
        spec: SPEC,
        parameters: PARAMS,
        options: { nr: 61, nz: 121, particles: 12, max_passes: 10 },
      },
    })) as ToolText;
    const out = JSON.parse(res.content[0].text);

    expect(out.stats.n).toBe(12);
    expect(out.stats.interception_fraction).toBeGreaterThanOrEqual(0);
    expect(out.stats.interception_fraction).toBeLessThanOrEqual(1);
    expect(out.geometric_transparency).toBeGreaterThan(0);
    expect(out.geometric_transparency).toBeLessThan(1);

    expect(out.claim_set.schema).toBe("vcad.particle-claims/1");
    const names = out.claim_set.claims.map((c: { name: string }) => c.name);
    expect(names).toContain("ddn_neutron_rate");
    expect(names).toContain("q_estimate");
    expect(names).toContain("distance_to_lawson");

    // Unified-receipt claims: particle domain, predicted basis (rolls up
    // Provisional by contract — never verified from a simulation alone).
    expect(out.receipt_claims.length).toBe(out.claim_set.claims.length);
    for (const c of out.receipt_claims) {
      expect(c.domain).toBe("particle");
      expect(c.id.startsWith("particle.")).toBe(true);
      expect(c.basis).toBe("predicted");
    }
  });

  it("optimize_electrodes climbs within bounds and reports starts", async () => {
    const res = (await client.callTool({
      name: "optimize_electrodes",
      arguments: {
        spec: SPEC,
        parameters: PARAMS,
        variables: [{ name: "shield_at", lo: 0, hi: 30000 }],
        options: {
          nr: 41,
          nz: 81,
          particles: 8,
          max_passes: 6,
          max_iters: 2,
          multi_start: false,
        },
      },
    })) as ToolText;
    const out = JSON.parse(res.content[0].text);

    expect(out.best_params.shield_at).toBeGreaterThanOrEqual(0);
    expect(out.best_params.shield_at).toBeLessThanOrEqual(30000);
    expect(out.best_sigma_v_m3).toBeGreaterThanOrEqual(0);
    expect(out.evals).toBeGreaterThanOrEqual(3);
    expect(out.starts.length).toBe(1);
    expect(out.history.length).toBeGreaterThanOrEqual(1);
  });

  it("unbound named parameters fail closed", async () => {
    const res = (await client.callTool({
      name: "simulate_charged_particles",
      arguments: {
        spec: SPEC,
        parameters: { ring_z: 22, ring_z_neg: -22 },
        options: { nr: 41, nz: 81, particles: 4, max_passes: 4 },
      },
    })) as { isError?: boolean; content: Array<{ text: string }> };
    expect(res.isError).toBe(true);
    expect(res.content[0].text).toContain("cathode_v");
  });
});
