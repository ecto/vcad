import { describe, it, expect, beforeAll } from "vitest";
import { Engine } from "@vcad/engine";
import type { Document } from "@vcad/ir";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { createServer } from "../server.js";
import { documents } from "../tools/session.js";

/**
 * End-to-end coverage for analyze_structure: the real server, the real
 * WASM kernel (CI builds it from source), an aluminum cantilever beam
 * part analyzed with fail-closed mesh-convergence gating.
 *
 * Skips (rather than fails) when the loaded kernel WASM predates the
 * FEA bindings. Locally: `npm run build -w vcad-kernel-wasm`.
 */

const probeEngine = await Engine.init();
const wasmHasFea =
  typeof (
    probeEngine as unknown as {
      kernel?: { feaAnalyzeMesh?: unknown };
    }
  ).kernel?.feaAnalyzeMesh === "function";

function beamDoc(): Document {
  return {
    version: "0.1",
    nodes: {
      "1": {
        id: 1,
        name: "beam",
        op: { type: "Cube", size: { x: 80, y: 10, z: 10 } },
      },
    },
    materials: {},
    part_materials: {},
    roots: [{ root: 1, material: "default" }],
  } as unknown as Document;
}

type ToolText = { content: Array<{ type: string; text: string }> };

const CANTILEVER = {
  document_id: "doc_beam",
  part: "beam",
  loads: [
    { region: { min: [80, 0, 0], max: [80, 10, 10] }, force: [0, 0, -100] },
  ],
  supports: [{ region: { min: [0, 0, 0], max: [0, 10, 10] } }],
  yield_strength_mpa: 276,
  resolution: 24,
};

describe.skipIf(!wasmHasFea)("analyze_structure tool", () => {
  let client: Client;

  beforeAll(async () => {
    documents.clear();
    documents.set("doc_beam", beamDoc());
    const engine = await Engine.init();
    const server = await createServer(engine, { user: null });
    const [clientT, serverT] = InMemoryTransport.createLinkedPair();
    client = new Client(
      { name: "structure-tools", version: "0.0.0" },
      { capabilities: {} },
    );
    await Promise.all([client.connect(clientT), server.connect(serverT)]);
  });

  it("converged cantilever: beam-theory ballpark, safety factor, provisional claims", async () => {
    const res = (await client.callTool({
      name: "analyze_structure",
      arguments: {
        ...CANTILEVER,
        // Loose gates for a fast CI-friendly two-level 24→48 study.
        displacement_tol: 0.4,
        stress_tol: 0.6,
      },
    })) as ToolText;
    const out = JSON.parse(res.content[0].text);

    expect(out.part).toBe("beam");
    expect(out.study.verdict.verdict).toBe("Converged");
    expect(out.study.levels).toHaveLength(2);
    // Timoshenko tip deflection ≈ 0.30 mm; linear tets approach from below.
    const fine = out.study.levels[1];
    expect(fine.max_displacement_mm).toBeGreaterThan(0.15);
    expect(fine.max_displacement_mm).toBeLessThan(0.4);
    // Peak stress near the fixed root, deflection at the tip.
    expect(fine.max_stress_at[0]).toBeLessThan(20);
    expect(fine.max_displacement_at[0]).toBeGreaterThan(70);
    // Safety factor claimed, aluminum does not yield at 100 N.
    expect(out.study.safety_factor).toBeGreaterThan(1);
    // Claims: fea-claims/1 set + unified receipt claims, all predicted.
    expect(out.claim_set.schema).toBe("vcad.fea-claims/1");
    const names = out.claim_set.claims.map((c: { name: string }) => c.name);
    expect(names).toContain("max_von_mises_mpa");
    expect(names).toContain("safety_factor");
    for (const c of out.receipt_claims) {
      expect(c.domain).toBe("structure");
      expect(c.basis).toBe("predicted");
    }
  });

  it("unconverged study is Unverifiable and emits no QoI claims", async () => {
    const res = (await client.callTool({
      name: "analyze_structure",
      arguments: {
        ...CANTILEVER,
        resolution: 8,
        displacement_tol: 1e-6,
        stress_tol: 1e-6,
      },
    })) as ToolText;
    const out = JSON.parse(res.content[0].text);
    expect(out.study.verdict.verdict).toBe("Unverifiable");
    expect(out.study.safety_factor).toBeNull();
    expect(out.claim_set).toBeNull();
    expect(out.receipt_claims).toHaveLength(1);
    expect(out.receipt_claims[0].verdict).toBe("unverifiable");
  });

  it("a load region off the part fails closed", async () => {
    await expect(
      client.callTool({
        name: "analyze_structure",
        arguments: {
          ...CANTILEVER,
          loads: [
            {
              region: { min: [500, 500, 500], max: [501, 501, 501] },
              force: [0, 0, -1],
            },
          ],
        },
      }),
    ).resolves.toMatchObject({ isError: true });
  });
});
