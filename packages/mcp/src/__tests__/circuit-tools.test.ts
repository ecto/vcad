import { describe, it, expect, beforeAll } from "vitest";
import { Engine } from "@vcad/engine";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { createServer } from "../server.js";
import { documents } from "../tools/session.js";

/**
 * End-to-end coverage for the circuit tools: the real server, the real WASM
 * kernel (CI builds it from source).
 *
 * Skips (rather than fails) when the loaded kernel WASM predates the circuit
 * analysis bindings. Locally: `npm run build -w vcad-kernel-wasm`.
 */

const probeEngine = await Engine.init();
const wasmHasCircuit =
  typeof (
    probeEngine as unknown as {
      kernel?: { circuitDcOperatingPoint?: unknown };
    }
  ).kernel?.circuitDcOperatingPoint === "function";

/** 12 V into a 3k/1k divider: out node sits at exactly 3 V. */
const DIVIDER = [
  { kind: "vsource", p: 1, n: 0, value: 12 },
  { kind: "resistor", p: 1, n: 2, value: 3000 },
  { kind: "resistor", p: 2, n: 0, value: 1000 },
];

/** Series RLC low-pass, detuned from the 10 kHz Butterworth target
 *  (f0 ≈ 15.9 kHz, Q = 0.5) — the filter_autotune example's start point.
 *  Topology: vin —R— mid —L— out —C— gnd. */
const RLC = [
  { kind: "vsource", p: 1, n: 0, value: 0 },
  { kind: "resistor", p: 1, n: 2, value: 200 },
  { kind: "inductor", p: 2, n: 3, value: 1e-3 },
  { kind: "capacitor", p: 3, n: 0, value: 1e-7 },
];

type ToolText = { content: Array<{ type: string; text: string }> };

describe.skipIf(!wasmHasCircuit)("circuit tools", () => {
  let client: Client;

  beforeAll(async () => {
    documents.clear();
    const engine = await Engine.init();
    const server = await createServer(engine, { user: null });
    const [clientT, serverT] = InMemoryTransport.createLinkedPair();
    client = new Client(
      { name: "circuit-tools", version: "0.0.0" },
      { capabilities: {} },
    );
    await Promise.all([client.connect(clientT), server.connect(serverT)]);
  });

  it("simulate_circuit dc: voltage divider hits 3 V with provisional claims", async () => {
    const res = (await client.callTool({
      name: "simulate_circuit",
      arguments: { devices: DIVIDER, analyses: ["dc"] },
    })) as ToolText;
    const out = JSON.parse(res.content[0].text);

    expect(out.dc.nodeVoltages[2]).toBeCloseTo(3.0, 9);
    // 12 V across 4k: 3 mA through both resistors.
    expect(out.dc.deviceCurrents[1]).toBeCloseTo(3e-3, 9);
    // The honesty signal: Tellegen residual is solver error only.
    expect(Math.abs(out.dc.powerBalanceW)).toBeLessThan(1e-9);

    expect(out.dc.claimSet.schema).toBe("vcad.spice-claims/1");
    const names = out.dc.claimSet.claims.map((c: { name: string }) => c.name);
    expect(names).toContain("dc_node_voltage_2");
    expect(names).toContain("power_balance_residual");
    expect(out.dc.receiptClaims.length).toBe(out.dc.claimSet.claims.length);
    for (const c of out.dc.receiptClaims) {
      expect(c.domain).toBe("circuit");
      expect(c.id.startsWith("circuit.")).toBe(true);
      expect(c.basis).toBe("predicted");
    }
  });

  it("simulate_circuit ac: RC low-pass Bode point at ω = 1/RC is −3 dB / −45°", async () => {
    const rc = [
      { kind: "vsource", p: 1, n: 0, value: 0 },
      { kind: "resistor", p: 1, n: 2, value: 1000 },
      { kind: "capacitor", p: 2, n: 0, value: 1e-6 },
    ];
    const fc = 1 / (2 * Math.PI * 1000 * 1e-6);
    const res = (await client.callTool({
      name: "simulate_circuit",
      arguments: {
        devices: rc,
        analyses: ["ac"],
        ac: { sourceId: 0, outNode: 2, frequenciesHz: [fc] },
      },
    })) as ToolText;
    const out = JSON.parse(res.content[0].text);
    expect(out.ac.bode.length).toBe(1);
    expect(out.ac.bode[0].magnitude).toBeCloseTo(Math.SQRT1_2, 9);
    expect(out.ac.bode[0].phaseDeg).toBeCloseTo(-45, 6);
    // Raw complex node voltages ride along.
    expect(out.ac.points[0].nodeVoltagesRe.length).toBe(3);
  });

  it("simulate_circuit transient: RC charges to the source voltage", async () => {
    const rc = [
      { kind: "vsource", p: 1, n: 0, value: 5 },
      { kind: "resistor", p: 1, n: 2, value: 1000 },
      { kind: "capacitor", p: 2, n: 0, value: 1e-6 },
    ];
    const res = (await client.callTool({
      name: "simulate_circuit",
      arguments: {
        devices: rc,
        analyses: ["transient"],
        transient: { dt: 1e-5, steps: 1000, sampleEvery: 100 },
      },
    })) as ToolText;
    const out = JSON.parse(res.content[0].text);
    const last = out.transient.nodeVoltages.at(-1);
    expect(last[2]).toBeGreaterThan(4.9); // ~10 τ
    expect(Math.abs(out.transient.powerBalanceW)).toBeLessThan(1e-9);
  });

  it("tune_circuit hits the 10 kHz / Q=0.707 Butterworth target from a detuned RLC", async () => {
    const res = (await client.callTool({
      name: "tune_circuit",
      arguments: {
        devices: RLC,
        target: {
          cutoffHz: 10_000,
          qFactor: Math.SQRT1_2,
          sourceId: 0,
          outNode: 3,
        },
        freeDevices: [{ device: 1 }, { device: 2 }, { device: 3 }],
      },
    })) as ToolText;
    const out = JSON.parse(res.content[0].text);

    expect(out.iterations).toBeGreaterThan(0);
    expect(out.objectiveAfter).toBeLessThan(out.objectiveBefore);
    // Achieved values are measured off the tuned response, not closed forms.
    expect(out.achievedCutoffHz).toBeGreaterThan(9_900);
    expect(out.achievedCutoffHz).toBeLessThan(10_100);
    expect(out.achievedQFactor).toBeGreaterThan(0.7);
    expect(out.achievedQFactor).toBeLessThan(0.715);
    expect(out.tunedValues.length).toBe(3);
    expect(out.response.length).toBe(25);

    expect(out.claimSet.schema).toBe("vcad.spice-claims/1");
    const names = out.claimSet.claims.map((c: { name: string }) => c.name);
    expect(names).toContain("cutoff_hz");
    expect(names).toContain("q_factor");
    for (const c of out.receiptClaims) {
      expect(c.basis).toBe("predicted");
    }
  });

  it("tune_circuit dc target: divider retuned to 4 V", async () => {
    const res = (await client.callTool({
      name: "tune_circuit",
      arguments: {
        devices: DIVIDER,
        target: { node: 2, dcVoltage: 4 },
        freeDevices: [{ device: 2, min: 100, max: 100_000 }],
      },
    })) as ToolText;
    const out = JSON.parse(res.content[0].text);
    expect(out.achievedDcVoltage).toBeCloseTo(4.0, 4);
    // 12·R2/(3000+R2) = 4 ⇒ R2 = 1500.
    expect(out.tunedValues[0].after).toBeCloseTo(1500, 0);
  });

  it("errors surface as typed tool errors, not crashes", async () => {
    // Floating node (no path to ground) → singular MNA → isError.
    const res = (await client.callTool({
      name: "simulate_circuit",
      arguments: {
        devices: [{ kind: "resistor", p: 1, n: 2, value: 1000 }],
        analyses: ["dc"],
      },
    })) as { isError?: boolean; content: Array<{ text: string }> };
    expect(res.isError).toBe(true);

    const res2 = (await client.callTool({
      name: "tune_circuit",
      arguments: {
        devices: RLC,
        target: { cutoffHz: 10_000, qFactor: 0.707, sourceId: 0, outNode: 3 },
        freeDevices: [{ device: 99 }],
      },
    })) as { isError?: boolean };
    expect(res2.isError).toBe(true);
  });
});
