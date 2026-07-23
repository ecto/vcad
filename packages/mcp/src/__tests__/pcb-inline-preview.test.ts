import { describe, it, expect, beforeAll } from "vitest";
import { Engine } from "@vcad/engine";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { createServer } from "../server.js";

/**
 * A freshly placed PCB must actually preview in the inline viewer.
 *
 * Regression: Claude Code mounts the viewer widget WITHOUT declaring the
 * `io.modelcontextprotocol/ui` capability at initialize, and its widget →
 * server `get_preview_glb` round trip is not dependable. When the inline
 * `_meta` preview was gated on that capability, place_components results in
 * Claude Code rendered as "no geometry to preview" despite a fully placed
 * board. The inline preview must ride on mount-tool results for ALL clients
 * (it's `_meta` — never model-visible, ignored by UI-less hosts).
 */

async function connect(engine: Engine) {
  const server = await createServer(engine, { user: null });
  const [clientT, serverT] = InMemoryTransport.createLinkedPair();
  const client = new Client(
    { name: "test", version: "0.0.0" },
    { capabilities: {} }, // no UI extension declared — the Claude Code shape
  );
  await Promise.all([client.connect(clientT), server.connect(serverT)]);
  return { client, server };
}

function parse(result: unknown): Record<string, unknown> {
  const content = (result as { content: Array<{ type: string; text: string }> })
    .content;
  return JSON.parse(content[0].text) as Record<string, unknown>;
}

const SCHEMATIC_ARGS = {
  components: [
    {
      ref: "R1",
      value: "1k",
      footprint: "Resistor_SMD:R_0805",
      x: 0,
      y: 0,
      pins: [
        { number: "1", name: "~", type: "Passive", x: -5, y: 0 },
        { number: "2", name: "~", type: "Passive", x: 5, y: 0 },
      ],
    },
    {
      ref: "R2",
      value: "1k",
      footprint: "Resistor_SMD:R_0805",
      x: 20,
      y: 0,
      pins: [
        { number: "1", name: "~", type: "Passive", x: -5, y: 0 },
        { number: "2", name: "~", type: "Passive", x: 5, y: 0 },
      ],
    },
  ],
  nets: { MID: ["R1.2", "R2.1"] },
};

describe("place_components inline preview (no-UI-capability client)", () => {
  let engine: Engine;
  beforeAll(async () => {
    engine = await Engine.init();
  });

  it("rides a ready-to-render GLB in _meta and previews via get_preview_glb", async () => {
    const { client, server } = await connect(engine);
    const documentId = parse(
      await client.callTool({
        name: "create_schematic",
        arguments: SCHEMATIC_ARGS,
      }),
    ).document_id as string;

    const placed = (await client.callTool({
      name: "place_components",
      arguments: { document_id: documentId, board_width: 40, board_height: 30 },
    })) as { _meta?: Record<string, unknown> };

    const inline = placed._meta?.["vcad.io/preview"] as
      | { document_id?: string; glb?: string }
      | undefined;
    expect(inline?.document_id).toBe(documentId);
    expect(typeof inline?.glb).toBe("string");
    expect((inline!.glb as string).length).toBeGreaterThan(1000);

    // Non-mount PCB mutators must ride the inline GLB too: the mounted
    // widget's fetch fallback isn't dependable in Claude Code, and a
    // board-only document has no CAD part scene to fall back on — without
    // this, route_nets / add_zone results rendered "no geometry to preview".
    for (const call of [
      { name: "route_nets", arguments: { document_id: documentId } },
      {
        name: "add_zone",
        arguments: { document_id: documentId, net: "MID", layer: "FCu", fill_board: true },
      },
    ]) {
      const mutated = (await client.callTool(call)) as {
        _meta?: Record<string, unknown>;
      };
      const meta = mutated._meta?.["vcad.io/preview"] as
        | { document_id?: string; glb?: string }
        | undefined;
      expect(meta?.document_id, `${call.name} inline preview`).toBe(documentId);
      expect((meta?.glb ?? "").length, `${call.name} glb size`).toBeGreaterThan(1000);
    }

    // The widget's fetch path serves the same board.
    const glb = parse(
      await client.callTool({
        name: "get_preview_glb",
        arguments: { document_id: documentId },
      }),
    );
    expect(glb._vcad_glb).toBeTruthy();

    await client.close();
    await server.close();
  });
});
