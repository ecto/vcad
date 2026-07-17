import { describe, it, expect, beforeAll } from "vitest";
import { Engine } from "@vcad/engine";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { createServer } from "../server.js";
import { documents } from "../tools/session.js";

/**
 * End-to-end coverage for the neutron shielding tool: the real server, the
 * real WASM kernel (CI builds it from source), a small-history HDPE stack.
 *
 * Skips (rather than fails) when the loaded kernel WASM predates the
 * neutronics bindings. Locally: `npm run build -w vcad-kernel-wasm`.
 */

const probeEngine = await Engine.init();
const wasmHasNeutronics =
  typeof (
    probeEngine as unknown as {
      kernel?: { neutronicsSimulate?: unknown };
    }
  ).kernel?.neutronicsSimulate === "function";

const SPEC = {
  layers: [
    { material: "air", thickness_mm: 100 },
    { material: "hdpe", thickness_mm: "shield_t" },
    { material: "air", thickness_mm: 400 },
  ],
  source: { rate_n_per_s: 1.0e6, energy_ev: 2.45e6 },
  detectors: [{ label: "operator", radius_mm: 400 }],
  run: { histories_per_batch: 500, batches: 4, seed: 7 },
};

type ToolText = { content: Array<{ type: string; text: string }> };

describe.skipIf(!wasmHasNeutronics)("neutron shielding tool", () => {
  let client: Client;

  beforeAll(async () => {
    documents.clear();
    const engine = await Engine.init();
    const server = await createServer(engine, { user: null });
    const [clientT, serverT] = InMemoryTransport.createLinkedPair();
    client = new Client(
      { name: "neutronics-tools", version: "0.0.0" },
      { capabilities: {} },
    );
    await Promise.all([client.connect(clientT), server.connect(serverT)]);
  });

  it("simulate_neutron_shield returns dose with error bars + provisional claims", async () => {
    const res = (await client.callTool({
      name: "simulate_neutron_shield",
      arguments: {
        spec: SPEC,
        parameters: { shield_t: 100 },
      },
    })) as ToolText;
    const out = JSON.parse(res.content[0].text);

    expect(out.detectors.length).toBe(1);
    expect(out.detectors[0].label).toBe("operator");
    expect(out.detectors[0].dose_usv_per_h).toBeGreaterThan(0);
    // Batch statistics produce a real, finite relative standard error.
    expect(out.detectors[0].rse).toBeGreaterThan(0);
    expect(Number.isFinite(out.detectors[0].rse)).toBe(true);
    expect(out.total_histories).toBe(2000);
    // Analog transport conserves neutrons to machine precision.
    expect(out.balance_max_dev).toBeLessThan(1e-9);

    expect(out.claim_set.schema).toBe("vcad.neutronics-claims/1");
    const names = out.claim_set.claims.map((c: { name: string }) => c.name);
    expect(names).toContain("dose_rate:operator");
    expect(names).toContain("attenuation_factor:operator");
    expect(names).toContain("absorbed_fraction");
    expect(out.claim_set.caveats.length).toBeGreaterThan(0);

    expect(out.receipt_claims.length).toBe(out.claim_set.claims.length);
    for (const c of out.receipt_claims) {
      expect(c.domain).toBe("neutronics");
      expect(c.id.startsWith("neutronics.")).toBe(true);
      expect(c.basis).toBe("predicted");
    }
  });

  it("unbound named parameters and D-T energies fail closed", async () => {
    const unbound = (await client.callTool({
      name: "simulate_neutron_shield",
      arguments: { spec: SPEC },
    })) as { isError?: boolean; content: Array<{ text: string }> };
    expect(unbound.isError).toBe(true);
    expect(unbound.content[0].text).toContain("shield_t");

    const dt = (await client.callTool({
      name: "simulate_neutron_shield",
      arguments: {
        spec: {
          ...SPEC,
          source: { rate_n_per_s: 1.0e6, energy_ev: 14.1e6 },
        },
        parameters: { shield_t: 100 },
      },
    })) as { isError?: boolean; content: Array<{ text: string }> };
    expect(dt.isError).toBe(true);
  });
});
