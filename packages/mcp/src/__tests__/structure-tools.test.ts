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
const probeKernel = (
  probeEngine as unknown as {
    kernel?: { feaAnalyzeMesh?: unknown; feaCheckBeam?: unknown };
  }
).kernel;
const wasmHasFea = typeof probeKernel?.feaAnalyzeMesh === "function";
/** The closed-form prismatic route and the thin-wall diagnosis ship
 *  together; both need a kernel newer than the FEA bindings alone. */
const wasmHasSection = typeof probeKernel?.feaCheckBeam === "function";

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

  it.skipIf(!wasmHasSection)("a thin-walled plate refuses WITH the cell arithmetic and a route forward", async () => {
    // A 300x60x2 mm plate: one cell through the wall at any resolution the
    // tier allows. The old behavior was a bare Unverifiable (or a
    // staircased answer); the caller had to derive the cell arithmetic to
    // find out why. Now the refusal carries it, plus where to go instead.
    documents.set("doc_plate", {
      version: "0.1",
      nodes: {
        "1": {
          id: 1,
          name: "plate",
          op: { type: "Cube", size: { x: 300, y: 60, z: 2 } },
        },
      },
      materials: {},
      part_materials: {},
      roots: [{ root: 1, material: "default" }],
    } as unknown as Document);
    const res = (await client.callTool({
      name: "analyze_structure",
      arguments: {
        document_id: "doc_plate",
        part: "plate",
        loads: [
          { region: { min: [300, 0, 0], max: [300, 60, 2] }, force: [0, 0, -50] },
        ],
        supports: [{ region: { min: [0, 0, 0], max: [0, 60, 2] } }],
        yield_strength_mpa: 276,
        resolution: 40,
        displacement_tol: 10,
        stress_tol: 10,
      },
    })) as ToolText & { isError?: boolean };
    const text = res.content[0].text;
    expect(text).toContain("THIN-WALLED");
    // The arithmetic the caller would otherwise redo by hand.
    expect(text).toMatch(/thinnest load-bearing section measures 2\.0/);
    expect(text).toMatch(/cell\(s\) sit through/);
    expect(text).toMatch(/resolution \d{3,}/);
    // And a named route forward, not just a refusal.
    expect(text).toContain("beam_check");
  });

  it.skipIf(!wasmHasSection)("beam_check prices the thin-walled torsion tube the lattice cannot", async () => {
    // 40x40x2 mm aluminum tube, 312 mm long, 40 N·m: the exact member
    // analyze_structure has to refuse.
    const res = (await client.callTool({
      name: "beam_check",
      arguments: {
        profile: { type: "rect_tube", width_mm: 40, height_mm: 40, wall_mm: 2 },
        length_mm: 312,
        end_condition: "cantilever_tip",
        torque_nmm: 40_000,
        yield_strength_mpa: 276,
      },
    })) as ToolText;
    const out = JSON.parse(res.content[0].text);
    expect(out.check.verdict.verdict).toBe("Applicable");
    // Bredt: J = 4·A_m²·t/s with A_m = 38² mm², s = 152 mm.
    expect(out.check.section.torsion_constant_mm4).toBeCloseTo(109_744, 0);
    // tau = T/(2·A_m·t) = 40000/5776.
    expect(out.check.torsional_shear_mpa).toBeCloseTo(6.925, 2);
    expect(out.check.twist_deg).toBeLessThan(0.3);
    expect(out.check.safety_factor).toBeGreaterThan(20);
    expect(out.claim_set.schema).toBe("vcad.fea-claims/1");
    for (const c of out.receipt_claims) {
      expect(c.id.startsWith("structure.beam.")).toBe(true);
      expect(c.domain).toBe("structure");
      expect(c.basis).toBe("predicted");
    }
  });

  it.skipIf(!wasmHasSection)("beam_check fails closed on a member beam theory cannot price", async () => {
    const res = (await client.callTool({
      name: "beam_check",
      arguments: {
        profile: { type: "rect", width_mm: 30, height_mm: 30 },
        length_mm: 60,
        end_condition: "cantilever_tip",
        transverse_force_n: 500,
        yield_strength_mpa: 276,
      },
    })) as ToolText;
    const out = JSON.parse(res.content[0].text);
    expect(out.check.verdict.verdict).toBe("Unverifiable");
    expect(out.check.safety_factor).toBeNull();
    expect(out.claim_set).toBeNull();
    expect(out.receipt_claims).toHaveLength(1);
    expect(out.receipt_claims[0].verdict).toBe("unverifiable");
    // The refusal names the other route, both directions being covered.
    expect(out.check.verdict.reasons.join(" ")).toContain("analyze_structure");
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
