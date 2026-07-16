/**
 * render_view highlight params: `highlight: [ids]` spotlights explicit parts
 * (full material colour + brand-orange accent outline, everything else
 * ghosted), and `highlight_changed: true` defaults the set to the part ids
 * from the session's most recent mutation `changed` diff.
 */
import { describe, it, expect, beforeAll, beforeEach } from "vitest";
import { Engine } from "@vcad/engine";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { createServer } from "../server.js";
import { documents } from "../tools/session.js";

/** Brand orange (docs/brand-spec.md) — the accent-outline stroke colour. */
const ACCENT = "#f25c1f";

async function connect(engine: Engine) {
  const server = await createServer(engine, { user: null });
  const [clientT, serverT] = InMemoryTransport.createLinkedPair();
  const client = new Client(
    { name: "render-highlight-test", version: "0.0.0" },
    { capabilities: {} },
  );
  await Promise.all([client.connect(clientT), server.connect(serverT)]);
  return { client, server };
}

function firstText(result: unknown): string {
  const content = (result as { content: Array<{ type: string; text: string }> })
    .content;
  const block = content.find((c) => c.type === "text");
  if (!block) throw new Error("no text block in result");
  return block.text;
}

async function openDoc(client: Client): Promise<string> {
  const open = await client.callTool({ name: "open_document", arguments: {} });
  return JSON.parse(firstText(open)).document_id as string;
}

/** Create two disjoint cubes; returns the `changed` diffs of each mutation. */
async function seedTwoCubes(client: Client, id: string) {
  const first = await client.callTool({
    name: "create",
    arguments: {
      document_id: id,
      type: "cube",
      params: { size: { x: 20, y: 20, z: 10 } },
    },
  });
  const second = await client.callTool({
    name: "create",
    arguments: {
      document_id: id,
      type: "cube",
      params: {
        size: { x: 20, y: 20, z: 10 },
        position: { x: 40, y: 0, z: 0 },
      },
    },
  });
  return {
    first: JSON.parse(firstText(first)).changed,
    second: JSON.parse(firstText(second)).changed,
  };
}

describe("render_view highlight", () => {
  let engine: Engine;
  beforeAll(async () => {
    engine = await Engine.init();
  });
  beforeEach(() => {
    documents.clear();
  });

  it("renders with an explicit highlight set and echoes it back", async () => {
    const { client, server } = await connect(engine);
    const id = await openDoc(client);
    const { second } = await seedTwoCubes(client, id);
    expect(second?.added?.length).toBe(1);
    const partId = second.added[0].part_id as string;

    const res = await client.callTool({
      name: "render_view",
      arguments: { document_id: id, highlight: [partId] },
    });
    expect(res.isError ?? false).toBe(false);
    const meta = JSON.parse(firstText(res));
    expect(meta.highlight).toEqual([partId]);
    // When the rasterizer is unavailable the raw SVG comes back — assert the
    // accent stroke directly in that case.
    if (meta.format === "svg" && meta.svg) {
      expect(meta.svg).toContain(ACCENT);
    }

    await client.close();
    await server.close();
  });

  it("highlight_changed defaults to the last mutation's part ids", async () => {
    const { client, server } = await connect(engine);
    const id = await openDoc(client);
    const { second } = await seedTwoCubes(client, id);
    const lastPartId = second.added[0].part_id as string;

    const res = await client.callTool({
      name: "render_view",
      arguments: { document_id: id, highlight_changed: true },
    });
    expect(res.isError ?? false).toBe(false);
    const meta = JSON.parse(firstText(res));
    expect(meta.highlight).toEqual([lastPartId]);

    await client.close();
    await server.close();
  });

  it("highlight_changed with no recorded mutation is a loud error", async () => {
    const { client, server } = await connect(engine);
    const id = await openDoc(client);

    const res = await client.callTool({
      name: "render_view",
      arguments: { document_id: id, highlight_changed: true },
    });
    expect(res.isError).toBe(true);
    expect(firstText(res)).toContain("no recorded mutation diff");

    await client.close();
    await server.close();
  });

  it("an unmatched explicit highlight errors instead of rendering unhighlighted", async () => {
    const { client, server } = await connect(engine);
    const id = await openDoc(client);
    await seedTwoCubes(client, id);

    const res = await client.callTool({
      name: "render_view",
      arguments: { document_id: id, highlight: ["no-such-part"] },
    });
    expect(res.isError).toBe(true);
    expect(firstText(res)).toContain("highlight matched no parts");

    await client.close();
    await server.close();
  });
});
