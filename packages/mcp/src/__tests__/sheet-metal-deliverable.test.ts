import { describe, it, expect, beforeAll } from "vitest";
import { Engine } from "@vcad/engine";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { createServer } from "../server.js";
import { documents } from "../tools/session.js";

/**
 * `sheet_metal_unfold`'s deliverable — the flat pattern and the fab-ready DXF
 * — must survive the FULL server dispatch path, in BOTH renderings a host can
 * choose: the text `content` blocks and `structuredContent`.
 *
 * Field report (2026-07-25): the handler returned everything, but the only
 * structured payload was the `{document_id, document_version}` preview handle,
 * and a host that renders structuredContent in preference to the text blocks
 * showed the caller exactly that stub — no flat pattern, no DXF, no bbox. A
 * handler-level test would have passed while that shipped, so this drives the
 * real ListTools/CallTool handlers end to end and asserts both fields.
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

/** The JSON object a text-only host would parse out of the result. */
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

// 2mm Al-soft on the sendcutsend profile: fixed inside radius 0.97, K 0.48.
// Bend allowance = (pi/2) * (R + K*t) = (pi/2) * (0.97 + 0.96).
const ALLOWANCE = (Math.PI / 2) * (0.97 + 0.48 * 2);
const BASE_WIDTH = 76;
const FLANGE = 55;
// Developed width: base + two flanges, each joined by one bend allowance.
const DEVELOPED_WIDTH = BASE_WIDTH + 2 * (FLANGE + ALLOWANCE);

describe("sheet_metal_unfold returns its deliverable through full dispatch", () => {
  let engine: Engine;
  beforeAll(async () => {
    engine = await Engine.init();
  });

  it("returns flat pattern + layered DXF in content AND structuredContent", async () => {
    documents.clear();
    const { client, server } = await connect(engine);

    const created = (await client.callTool({
      name: "sheet_metal_create",
      arguments: {
        width: BASE_WIDTH,
        depth: 200,
        thickness: 2,
        material: "Al-soft",
        shop_profile: "sendcutsend",
        bend_relief: true,
        flanges: [
          { edge_index: 1, length: FLANGE, direction: "Up" },
          { edge_index: 3, length: FLANGE, direction: "Up" },
        ],
      },
    })) as ToolCallResult;
    expect(created.isError).toBeFalsy();

    // sheet_metal_create's own promised summary must survive too — same
    // behavior flags, same hazard.
    const createBody = bodyOf(created);
    expect(createBody.model).toBeDefined();
    expect(createBody.violations).toBeDefined();
    expect(created.structuredContent?.model).toBeDefined();

    const docId = created.structuredContent?.document_id as string;
    expect(typeof docId).toBe("string");

    const unfolded = (await client.callTool({
      name: "sheet_metal_unfold",
      arguments: { document_id: docId, include_dxf: true },
    })) as ToolCallResult;
    expect(unfolded.isError).toBeFalsy();

    // Both renderings carry the deliverable — neither may be a bare handle.
    for (const [label, view] of [
      ["content", bodyOf(unfolded)],
      ["structuredContent", unfolded.structuredContent ?? {}],
    ] as Array<[string, Record<string, unknown>]>) {
      const flat = view.flat_pattern as Record<string, unknown> | undefined;
      expect(flat, `${label}: flat_pattern missing`).toBeDefined();

      const dxf = view.dxf;
      expect(typeof dxf, `${label}: dxf missing`).toBe("string");
      const text = dxf as string;
      expect(text.length).toBeGreaterThan(0);
      // The documented layers, all four declared in the LAYER table.
      for (const layer of ["CUT", "BEND_UP", "BEND_DOWN", "ENGRAVE"]) {
        expect(text, `${label}: DXF missing ${layer}`).toContain(layer);
      }
      // Bend centerlines are DASHED, and real cut geometry was emitted.
      expect(text).toContain("DASHED");
      expect(text).toContain("LWPOLYLINE");

      // Flat bbox matches the developed length for this flange/K-factor case.
      const bbox = flat!.bbox as number[];
      expect(bbox).toHaveLength(4);
      expect(bbox[2] - bbox[0]).toBeCloseTo(DEVELOPED_WIDTH, 6);
      expect(bbox[3] - bbox[1]).toBeCloseTo(200, 6);

      // Two 90-degree Up creases, one per flange, on the allowance midline.
      const creases = flat!.creases as Array<Record<string, unknown>>;
      expect(creases).toHaveLength(2);
      for (const crease of creases) {
        expect(crease.direction).toBe("Up");
        expect(crease.k_factor).toBeCloseTo(0.48, 6);
      }
      expect(flat!.area_mm2 as number).toBeGreaterThan(0);
    }

    await client.close();
    await server.close();
  });

  // The same part authored in loon must unfold to the same flat pattern as
  // the one authored through sheet_metal_create. If it doesn't, the loon
  // bindings are a lossy detour and a part written that way would be cut
  // wrong — which is the whole failure the bindings exist to prevent.
  it("a loon-authored chain unfolds identically to sheet_metal_create", async () => {
    documents.clear();
    const { client, server } = await connect(engine);

    const created = (await client.callTool({
      name: "create_cad_loon",
      arguments: {
        source: [
          `[pipe [sheet-base-flange-rect-shop ${BASE_WIDTH}.0 200.0 2.0 "Al-soft" "sendcutsend"]`,
          `      [sheet-edge-flange "east" ${FLANGE}.0 90.0]`,
          `      [sheet-edge-flange "west" ${FLANGE}.0 90.0]`,
          `      [sheet-bend-relief]]`,
        ].join("\n"),
      },
    })) as ToolCallResult;
    expect(created.isError, JSON.stringify(created.content)).toBeFalsy();

    const docId = created.structuredContent?.document_id as string;
    expect(typeof docId).toBe("string");

    const unfolded = (await client.callTool({
      name: "sheet_metal_unfold",
      arguments: { document_id: docId, include_dxf: true },
    })) as ToolCallResult;
    expect(unfolded.isError, JSON.stringify(unfolded.content)).toBeFalsy();

    const flat = (unfolded.structuredContent?.flat_pattern ?? {}) as Record<string, unknown>;
    const bbox = flat.bbox as number[];
    expect(bbox, "loon chain produced no flat pattern").toHaveLength(4);
    expect(bbox[2] - bbox[0]).toBeCloseTo(DEVELOPED_WIDTH, 6);
    expect(bbox[3] - bbox[1]).toBeCloseTo(200, 6);

    // Both bends resolved through the shop profile's table, not a default.
    const creases = flat.creases as Array<Record<string, unknown>>;
    expect(creases).toHaveLength(2);
    for (const crease of creases) {
      expect(crease.direction).toBe("Up");
      expect(crease.k_factor).toBeCloseTo(0.48, 6);
    }

    await client.close();
    await server.close();
  });
});
