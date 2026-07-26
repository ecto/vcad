import { describe, it, expect, beforeAll, beforeEach } from "vitest";
import { Engine } from "@vcad/engine";
import type { Document } from "@vcad/ir";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { createServer } from "../server.js";
import { documents, registerSession } from "../tools/session.js";

/**
 * `flat_pattern_from_solid` closes the last gap between a verified assembly
 * and a cut order: parts modelled as ordinary solids must produce a DXF plus
 * a bend table without being re-authored through the sheet-metal ops.
 *
 * Driven through the real server dispatch (not the handler) because the DXF
 * is the deliverable, and a result-slimming pass that swallowed it would look
 * fine to a handler-level test — exactly how `sheet_metal_unfold` lost its
 * DXF to the preview handle in the 2026-07-25 field report.
 */

interface ToolCallResult {
  content: Array<{ type: string; text: string }>;
  structuredContent?: Record<string, unknown>;
  isError?: boolean;
}

async function connect(engine: Engine) {
  const server = await createServer(engine, { user: null });
  const [clientT, serverT] = InMemoryTransport.createLinkedPair();
  const client = new Client({ name: "test", version: "0.0.0" }, { capabilities: {} });
  await Promise.all([client.connect(clientT), server.connect(serverT)]);
  return { client, server };
}

function bodyOf(result: ToolCallResult): Record<string, unknown> {
  const merged: Record<string, unknown> = {};
  for (const block of result.content) {
    if (block.type !== "text") continue;
    try {
      const parsed = JSON.parse(block.text) as unknown;
      if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) {
        Object.assign(merged, parsed);
      }
    } catch {
      // prose block
    }
  }
  return merged;
}

/** Two identical 5 mm plates, one different plate, and a sphere that is not
 *  sheet metal at all. */
function plateDocument(): Document {
  return {
    version: "0.1",
    nodes: {
      "1": { id: 1, name: "femur-L", op: { type: "Cube", size: { x: 100, y: 50, z: 5 } } },
      "2": { id: 2, name: "femur-R", op: { type: "Cube", size: { x: 100, y: 50, z: 5 } } },
      "3": { id: 3, name: "crank", op: { type: "Cube", size: { x: 60, y: 40, z: 5 } } },
      "4": { id: 4, name: "ball", op: { type: "Sphere", radius: 12, segments: 32 } },
    },
    materials: {},
    part_materials: {},
    roots: [
      { root: 1, visible: true },
      { root: 2, visible: true },
      { root: 3, visible: true },
      { root: 4, visible: true },
    ],
    partDefs: {},
    instances: [],
    joints: [],
  } as unknown as Document;
}

describe("flat_pattern_from_solid", () => {
  let engine: Engine;
  beforeAll(async () => {
    engine = await Engine.init();
  });
  beforeEach(() => {
    documents.clear();
  });

  it("batches a document into unique patterns × quantity, with DXF and verification", async () => {
    const docId = registerSession(plateDocument());
    const { client, server } = await connect(engine);

    const result = (await client.callTool({
      name: "flat_pattern_from_solid",
      arguments: { document_id: docId },
    })) as ToolCallResult;
    expect(result.isError).toBeFalsy();

    const body = bodyOf(result);
    const patterns = body.patterns as Array<Record<string, unknown>>;
    expect(patterns).toHaveLength(2);

    const byQty = [...patterns].sort(
      (a, b) => (b.quantity as number) - (a.quantity as number),
    );
    // The two identical plates collapse into one pattern with quantity 2.
    expect(byQty[0]!.quantity).toBe(2);
    expect(byQty[1]!.quantity).toBe(1);

    for (const p of patterns) {
      expect(p.thickness_mm).toBeCloseTo(5, 6);
      // The DXF is the deliverable and must survive dispatch intact.
      const dxf = p.dxf as string;
      expect(typeof dxf).toBe("string");
      expect(dxf).toContain("CUT");
      expect(dxf).toContain("LWPOLYLINE");
      // Round-trip: profile × thickness reproduces the solid.
      const v = p.verification as Record<string, number>;
      expect(v.error_frac).toBeLessThan(1e-6);
      expect(v.recovered_volume_mm3).toBeCloseTo(v.solid_volume_mm3, 6);
      // Flat plates have no bends.
      expect(p.bend_table).toHaveLength(0);
    }

    const big = byQty[0]!.flat as Record<string, number>;
    expect(Math.max(big.width_mm, big.height_mm)).toBeCloseTo(100, 6);
    expect(Math.min(big.width_mm, big.height_mm)).toBeCloseTo(50, 6);

    // The sphere is reported as not-sheet-metal rather than silently skipped.
    const failed = body.not_sheet_metal as Array<Record<string, unknown>>;
    expect(failed).toHaveLength(1);
    expect(failed[0]!.name).toBe("ball");
    expect(String(failed[0]!.reason).length).toBeGreaterThan(0);

    // `nest_input` is ready to hand straight to sheet_metal_nest.
    const nested = (await client.callTool({
      name: "sheet_metal_nest",
      arguments: { parts: body.nest_input, params: {} },
    })) as ToolCallResult;
    expect(nested.isError).toBeFalsy();

    await client.close();
    await server.close();
  });

  it("flattens a single named part and fails loudly on a non-sheet part", async () => {
    const docId = registerSession(plateDocument());
    const { client, server } = await connect(engine);

    const one = (await client.callTool({
      name: "flat_pattern_from_solid",
      arguments: { document_id: docId, part_id: "3" },
    })) as ToolCallResult;
    const patterns = bodyOf(one).patterns as Array<Record<string, unknown>>;
    expect(patterns).toHaveLength(1);
    expect((patterns[0]!.parts as Array<Record<string, unknown>>)[0]!.name).toBe("crank");

    const sphere = (await client.callTool({
      name: "flat_pattern_from_solid",
      arguments: { document_id: docId, part_id: "4" },
    })) as ToolCallResult;
    expect(sphere.isError).toBe(true);

    await client.close();
    await server.close();
  });
});
