import { describe, it, expect, beforeAll } from "vitest";
import { Engine } from "@vcad/engine";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { createServer } from "../server.js";
import { documents } from "../tools/session.js";

/**
 * End-to-end coverage for the photonics FDTD tool: the real server, the
 * real WASM kernel (CI builds it from source), the crate's validated
 * straight-waveguide transmission case at test scale.
 *
 * Skips (rather than fails) when the loaded kernel WASM predates the
 * photonics bindings. Locally: `npm run build -w vcad-kernel-wasm`.
 */

const probeEngine = await Engine.init();
const wasmHasPhotonics =
  typeof (
    probeEngine as unknown as {
      kernel?: { photonicsSimulate?: unknown };
    }
  ).kernel?.photonicsSimulate === "function";

type ToolText = { content: Array<{ type: string; text: string }> };

describe.skipIf(!wasmHasPhotonics)("photonics FDTD tool", () => {
  let client: Client;

  beforeAll(async () => {
    documents.clear();
    const engine = await Engine.init();
    const server = await createServer(engine, { user: null });
    const [clientT, serverT] = InMemoryTransport.createLinkedPair();
    client = new Client(
      { name: "photonics-tools", version: "0.0.0" },
      { capabilities: {} },
    );
    await Promise.all([client.connect(clientT), server.connect(serverT)]);
  });

  it("simulate_photonics transmits a straight SOI waveguide", async () => {
    const res = (await client.callTool({
      name: "simulate_photonics",
      arguments: {
        spec: {
          wavelength_um: 1.55,
          n_core: 3.48,
          n_clad: 1.44,
          size_um: [5.6, 2.5],
          core_rects_um: [[-1, 1.14, 99, 1.36]],
          source: { x_um: 0.97, half_width_um: 0.11 },
          monitor_in_x_um: 1.94,
          outputs: [{ x_um: 4.85 }],
        },
        options: { resolution: 40, steps: 2500 },
      },
    })) as ToolText;
    const out = JSON.parse(res.content[0].text);

    // SOI slab mode: n_eff between cladding and core indices.
    expect(out.n_eff).toBeGreaterThan(1.44);
    expect(out.n_eff).toBeLessThan(3.48);

    expect(out.claim_set.schema).toBe("vcad.photonics-claims/1");
    const claims = Object.fromEntries(
      out.claim_set.claims.map((c: { name: string; value: number }) => [
        c.name,
        c.value,
      ]),
    );
    // A straight guide transmits nearly everything; arm B is empty.
    expect(claims.transmission_arm_a).toBeGreaterThan(0.8);
    expect(claims.transmission_arm_a).toBeLessThan(1.1);
    expect(claims.transmission_arm_b).toBe(0);
    expect(claims.insertion_loss_db).toBeLessThan(1);

    expect(out.claim_set.spectrum.length).toBe(1);
    expect(out.receipt_claims.length).toBe(out.claim_set.claims.length);
    for (const c of out.receipt_claims) {
      expect(c.domain).toBe("photonics");
      expect(c.id.startsWith("photonics.")).toBe(true);
      expect(c.basis).toBe("predicted");
    }
  });

  it("oversized grids fail closed with a cost hint", async () => {
    const res = (await client.callTool({
      name: "simulate_photonics",
      arguments: {
        spec: {
          wavelength_um: 1.55,
          n_core: 3.48,
          n_clad: 1.44,
          size_um: [4000, 4000],
          core_rects_um: [[0, 0, 4000, 1]],
          source: { x_um: 100, half_width_um: 0.11 },
          monitor_in_x_um: 200,
          outputs: [{ x_um: 3000 }],
        },
      },
    })) as { isError?: boolean; content: Array<{ text: string }> };
    expect(res.isError).toBe(true);
    expect(res.content[0].text).toContain("grid too large");
  });
});
