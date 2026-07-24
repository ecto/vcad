import { describe, it, expect, beforeAll } from "vitest";
import { Engine } from "@vcad/engine";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { createServer } from "../server.js";
import { documents } from "../tools/session.js";

/**
 * End-to-end coverage for the lattice gauge theory tool: the real server,
 * the real WASM kernel, a small confined-phase SU(2) run.
 *
 * Skips (rather than fails) when the loaded kernel WASM predates the
 * qcd bindings. Locally: `npm run build -w vcad-kernel-wasm`.
 */

const probeEngine = await Engine.init();
const wasmHasQcd =
  typeof (
    probeEngine as unknown as {
      kernel?: { latticeGaugeSimulate?: unknown };
    }
  ).kernel?.latticeGaugeSimulate === "function";

const SPEC = {
  gauge: "Su2",
  dims: [4, 4, 4, 4],
  beta: 2.2,
  thermalization_sweeps: 20,
  measurement_sweeps: 60,
  overrelax_per_heatbath: 1,
  bin_size: 10,
  max_wilson_extent: 2,
  seed: 42,
  hot_start: false,
  measure_polyakov: true,
  snapshot_cooling: 10,
};

type ToolText = { content: Array<{ type: string; text: string }> };

describe.skipIf(!wasmHasQcd)("lattice gauge theory tool", () => {
  let client: Client;

  beforeAll(async () => {
    documents.clear();
    const engine = await Engine.init();
    const server = await createServer(engine, { user: null });
    const [clientT, serverT] = InMemoryTransport.createLinkedPair();
    client = new Client(
      { name: "qcd-tools", version: "0.0.0" },
      { capabilities: {} },
    );
    await Promise.all([client.connect(clientT), server.connect(serverT)]);
  });

  it("simulate_lattice_gauge returns jackknifed observables + claims", async () => {
    const res = (await client.callTool({
      name: "simulate_lattice_gauge",
      arguments: { spec: SPEC },
    })) as ToolText;
    const out = JSON.parse(res.content[0].text);

    // Plaquette with a real error bar, in the physical range.
    expect(out.result.plaquette.mean).toBeGreaterThan(0);
    expect(out.result.plaquette.mean).toBeLessThan(1);
    expect(out.result.plaquette.err).toBeGreaterThan(0);
    // Area law: W(2,2) < W(1,1).
    const loops = out.result.wilson_loops as Array<{
      r: number;
      t: number;
      value: { mean: number };
    }>;
    const w = (r: number, t: number) =>
      loops.find((l) => l.r === r && l.t === t)!.value.mean;
    expect(w(2, 2)).toBeLessThan(w(1, 1));
    // Polyakov order parameter measured; cooled snapshot + Q present.
    expect(out.result.polyakov_abs.mean).toBeGreaterThan(0);
    expect(out.result.snapshot.action_density.length).toBe(256);
    expect(typeof out.result.topological_charge).toBe("number");
    // Claims minted (6 bins >= MIN_BINS), fail-closed schema intact.
    expect(out.claims.schema).toBe("vcad.qcd-claims/1");
    const names = out.claims.claims.map((c: { name: string }) => c.name);
    expect(names).toContain("plaquette");
    expect(names).toContain("polyakov_abs");
    expect(out.claims.caveats.length).toBeGreaterThan(0);
  });

  it("starved statistics and oversized runs fail closed", async () => {
    const starved = (await client.callTool({
      name: "simulate_lattice_gauge",
      arguments: {
        spec: { ...SPEC, measurement_sweeps: 12, bin_size: 6 },
      },
    })) as ToolText;
    const out = JSON.parse(starved.content[0].text);
    // Runs (2 bins is a legal run) but refuses to mint claims.
    expect(out.claims).toBeNull();
    expect(out.claim_error).toContain("bins");

    const huge = (await client.callTool({
      name: "simulate_lattice_gauge",
      arguments: {
        spec: { ...SPEC, dims: [32, 32, 32, 32], measurement_sweeps: 5000 },
      },
    })) as { isError?: boolean; content: Array<{ text: string }> };
    expect(huge.isError).toBe(true);
    expect(huge.content[0].text).toContain("cap");
  });
});
