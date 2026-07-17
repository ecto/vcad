import { describe, it, expect, beforeAll } from "vitest";
import { Engine } from "@vcad/engine";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { createServer } from "../server.js";
import { documents } from "../tools/session.js";

/**
 * End-to-end coverage for the antenna analysis tool: the real server, the
 * real WASM kernel (CI builds it from source), a coarse dipole sweep.
 *
 * Skips (rather than fails) when the loaded kernel WASM predates the
 * antenna bindings. Locally: `npm run build -w vcad-kernel-wasm`.
 */

const probeEngine = await Engine.init();
const wasmHasAntenna =
  typeof (
    probeEngine as unknown as {
      kernel?: { antennaAnalyze?: unknown };
    }
  ).kernel?.antennaAnalyze === "function";

const DIPOLE = {
  elements: [
    {
      type: "wire",
      start_mm: [0, 0, "neg_half_len"],
      end_mm: [0, 0, "half_len"],
      radius_mm: 1.0,
      segments: 20,
    },
  ],
  feed_mm: [0, 0, 0],
};

type ToolText = { content: Array<{ type: string; text: string }> };

describe.skipIf(!wasmHasAntenna)("antenna analysis tool", () => {
  let client: Client;

  beforeAll(async () => {
    documents.clear();
    const engine = await Engine.init();
    const server = await createServer(engine, { user: null });
    const [clientT, serverT] = InMemoryTransport.createLinkedPair();
    client = new Client(
      { name: "antenna-tools", version: "0.0.0" },
      { capabilities: {} },
    );
    await Promise.all([client.connect(clientT), server.connect(serverT)]);
  });

  it("analyze_antenna sweeps a dipole and finds its resonance", async () => {
    const res = (await client.callTool({
      name: "analyze_antenna",
      arguments: {
        spec: DIPOLE,
        parameters: { half_len: 500, neg_half_len: -500 },
        band: { f_lo_hz: 120e6, f_hi_hz: 165e6, points: 10 },
      },
    })) as ToolText;
    const out = JSON.parse(res.content[0].text);

    expect(out.segments).toBe(20);
    expect(out.sweep.length).toBe(10);
    for (const row of out.sweep) {
      expect(row.freq_hz).toBeGreaterThanOrEqual(120e6);
      expect(row.freq_hz).toBeLessThanOrEqual(165e6);
      expect(Number.isFinite(row.z_re_ohm)).toBe(true);
      expect(row.s11_db).toBeLessThanOrEqual(0);
    }

    expect(out.claim_set.schema).toBe("vcad.antenna-claims/1");
    const claims = Object.fromEntries(
      out.claim_set.claims.map((c: { name: string; value: number }) => [
        c.name,
        c.value,
      ]),
    );
    // Half-wave dipole: resonance near 143-145 MHz, R_in 60-80 ohm.
    expect(claims.resonance_in_band).toBe(1);
    expect(claims.resonant_frequency).toBeGreaterThan(135e6);
    expect(claims.resonant_frequency).toBeLessThan(155e6);
    expect(claims.gain_dbi).toBeGreaterThan(1.5);
    expect(claims.radiation_efficiency).toBeGreaterThan(0.95);

    expect(out.receipt_claims.length).toBe(out.claim_set.claims.length);
    for (const c of out.receipt_claims) {
      expect(c.domain).toBe("antenna");
      expect(c.id.startsWith("antenna.")).toBe(true);
      expect(c.basis).toBe("predicted");
    }
  });

  it("thin-wire validity gates fail closed", async () => {
    // 1 mm radius with 2 segments over 1 m: segment length fine, but at
    // 1.5 GHz the segment >> lambda/8 sampling gate must refuse.
    const res = (await client.callTool({
      name: "analyze_antenna",
      arguments: {
        spec: DIPOLE,
        parameters: { half_len: 500, neg_half_len: -500 },
        band: { f_lo_hz: 1.4e9, f_hi_hz: 1.6e9, points: 3 },
      },
    })) as { isError?: boolean; content: Array<{ text: string }> };
    expect(res.isError).toBe(true);
  });
});
