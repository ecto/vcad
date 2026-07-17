import { describe, it, expect, beforeAll } from "vitest";
import { Engine } from "@vcad/engine";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { createServer } from "../server.js";
import { documents } from "../tools/session.js";

/**
 * End-to-end coverage for the EM field-solver tool: the real server, the
 * real WASM kernel (CI builds it from source), coarse grids per class.
 *
 * Skips (rather than fails) when the loaded kernel WASM predates the em
 * bindings. Locally: `npm run build -w vcad-kernel-wasm`.
 */

const probeEngine = await Engine.init();
const wasmHasEm =
  typeof (
    probeEngine as unknown as {
      kernel?: { emSimulate?: unknown };
    }
  ).kernel?.emSimulate === "function";

type ToolText = { content: Array<{ type: string; text: string }> };

describe.skipIf(!wasmHasEm)("em field-solver tool", () => {
  let client: Client;

  beforeAll(async () => {
    documents.clear();
    const engine = await Engine.init();
    const server = await createServer(engine, { user: null });
    const [clientT, serverT] = InMemoryTransport.createLinkedPair();
    client = new Client(
      { name: "em-tools", version: "0.0.0" },
      { capabilities: {} },
    );
    await Promise.all([client.connect(clientT), server.connect(serverT)]);
  });

  it("axisym magnetostatics prices inductance with a cross-route residual", async () => {
    const res = (await client.callTool({
      name: "simulate_em",
      arguments: {
        spec: {
          problem: "axisym_magnetostatics",
          r_max_mm: 40,
          z_min_mm: 0,
          z_max_mm: 100,
          bc_r_outer: "neumann",
          bc_z_low: "neumann",
          bc_z_high: "neumann",
          coils: [
            {
              region: {
                x_min_mm: 20,
                x_max_mm: 22,
                y_min_mm: 0,
                y_max_mm: 100,
              },
              turns: 1000,
              current_a: "drive",
            },
          ],
        },
        parameters: { drive: 1.0 },
        options: { nx: 41, ny: 21 },
      },
    })) as ToolText;
    const out = JSON.parse(res.content[0].text);

    // A long 1000-turn solenoid: mu0 n^2 pi R^2 l ~ 17 mH.
    expect(out.qois.self_inductance_h).toBeGreaterThan(0.005);
    expect(out.qois.self_inductance_h).toBeLessThan(0.05);
    expect(out.claim_sets.length).toBe(1);
    expect(out.claim_sets[0].schema).toBe("vcad.em-claims/1");
    const names = out.claim_sets[0].claims.map(
      (c: { name: string }) => c.name,
    );
    expect(names).toContain("inductance_h");
    expect(names).toContain("stored_energy_j");
    expect(out.claim_sets[0].provenance.cross_route_residual).not.toBeNull();

    for (const c of out.receipt_claims) {
      expect(c.domain).toBe("em");
      expect(c.id.startsWith("em.")).toBe(true);
      expect(c.basis).toBe("predicted");
    }
  });

  it("electrostatics prices a two-terminal capacitance", async () => {
    const res = (await client.callTool({
      name: "simulate_em",
      arguments: {
        spec: {
          problem: "electrostatics",
          geometry: "axisymmetric",
          x_min_mm: 0,
          x_max_mm: 40,
          y_min_mm: 0,
          y_max_mm: 20,
          electrodes: [
            {
              shape: {
                type: "rect",
                x_min_mm: 0,
                x_max_mm: 10,
                y_min_mm: -1,
                y_max_mm: 21,
              },
              potential_v: 1,
            },
            {
              shape: {
                type: "rect",
                x_min_mm: 30,
                x_max_mm: 41,
                y_min_mm: -1,
                y_max_mm: 21,
              },
              potential_v: 0,
            },
          ],
        },
        options: { nx: 81, ny: 9, hot: 0 },
      },
    })) as ToolText;
    const out = JSON.parse(res.content[0].text);

    expect(out.qois.capacitance_f).toBeGreaterThan(0);
    // Two independent routes agree to a few percent on a coarse grid.
    expect(out.qois.capacitance_route_mismatch).toBeLessThan(0.1);
    const names = out.claim_sets[0].claims.map(
      (c: { name: string }) => c.name,
    );
    expect(names).toContain("capacitance_f");
    expect(out.receipt_claims.every((c: { domain: string }) => c.domain === "em")).toBe(
      true,
    );
  });

  it("planar magnetostatics without a torque block fails closed", async () => {
    const res = (await client.callTool({
      name: "simulate_em",
      arguments: {
        spec: {
          problem: "planar_magnetostatics",
          x_min_mm: 0,
          x_max_mm: 60,
          y_min_mm: 0,
          y_max_mm: 24,
          conductors: [
            {
              region: {
                x_min_mm: 3,
                x_max_mm: 9,
                y_min_mm: 4,
                y_max_mm: 7,
              },
              total_current_a: 4,
            },
          ],
        },
        options: { nx: 31, ny: 17 },
      },
    })) as { isError?: boolean; content: Array<{ text: string }> };
    expect(res.isError).toBe(true);
    expect(res.content[0].text).toContain("torque");
  });
});
