import { describe, it, expect, beforeAll, beforeEach } from "vitest";
import { Engine } from "@vcad/engine";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { createServer } from "../server.js";
import { documents } from "../tools/session.js";

/**
 * End-to-end coverage of `apply_edits` through the real CallTool pipeline (in-
 * memory MCP client, anonymous connection). Exercises atomic multi-op batches,
 * all-or-nothing rollback, single-entry undo, and dry_run — the acceptance
 * criteria from issue #435.
 */

async function connect(engine: Engine) {
  const server = await createServer(engine, { user: null });
  const [clientT, serverT] = InMemoryTransport.createLinkedPair();
  const client = new Client(
    { name: "batch-test", version: "0.0.0" },
    { capabilities: {} },
  );
  await Promise.all([client.connect(clientT), server.connect(serverT)]);
  return { client, server };
}

function firstText(result: unknown): string {
  const content = (result as { content: Array<{ type: string; text: string }> })
    .content;
  return content[0].text;
}

async function openDoc(client: Client): Promise<string> {
  const open = await client.callTool({ name: "open_document", arguments: {} });
  return JSON.parse(firstText(open)).document_id as string;
}

async function getDoc(
  client: Client,
  id: string,
): Promise<{ nodes: Record<string, unknown>; roots: unknown[] }> {
  const got = await client.callTool({
    name: "get_document",
    arguments: { document_id: id },
  });
  return JSON.parse(firstText(got));
}

describe("apply_edits (atomic multi-op batch)", () => {
  let engine: Engine;
  beforeAll(async () => {
    engine = await Engine.init();
  });
  beforeEach(() => {
    documents.clear();
  });

  it("applies a multi-op batch with one changed diff and one integrity block", async () => {
    const { client, server } = await connect(engine);
    const id = await openDoc(client);

    const res = await client.callTool({
      name: "apply_edits",
      arguments: {
        document_id: id,
        ops: [
          { op: "create", type: "cube", params: { size: { x: 10, y: 10, z: 10 } } },
          { op: "create", type: "cube", params: { size: { x: 4, y: 4, z: 4 } } },
        ],
      },
    });
    expect(res.isError ?? false).toBe(false);
    const parsed = JSON.parse(firstText(res));
    expect(parsed.applied).toBe(2);
    expect(parsed.ops).toHaveLength(2);
    // One aggregated changed diff covering both ops (two new parts).
    expect(parsed.changed).toBeDefined();
    expect(parsed.changed.added).toHaveLength(2);
    // One integrity certificate over the final document.
    expect(parsed.integrity).toBeDefined();
    expect(parsed.integrity.parts).toBe(2);

    // Document state is correct: two root parts.
    const doc = await getDoc(client, id);
    expect(doc.roots).toHaveLength(2);

    await client.close();
    await server.close();
  });

  it("rolls back the whole batch on a mid-list failure, leaving the doc byte-identical", async () => {
    const { client, server } = await connect(engine);
    const id = await openDoc(client);

    // Seed one valid part so there is prior state to preserve.
    await client.callTool({
      name: "apply_edits",
      arguments: {
        document_id: id,
        ops: [{ op: "create", type: "cube", params: { size: { x: 5, y: 5, z: 5 } } }],
      },
    });
    const before = JSON.stringify(await getDoc(client, id));

    // Batch: valid create, then a malformed create (size missing z) the planner
    // rejects → the batch fails at op index 1.
    const res = await client.callTool({
      name: "apply_edits",
      arguments: {
        document_id: id,
        ops: [
          { op: "create", type: "cube", params: { size: { x: 2, y: 2, z: 2 } } },
          { op: "create", type: "cube", params: { size: { x: 2, y: 2 } } },
        ],
      },
    });
    expect(res.isError).toBe(true);
    // Error names the failing op index (index 1).
    expect(firstText(res)).toMatch(/op 1/);

    // Document is byte-identical to its pre-call state.
    const after = JSON.stringify(await getDoc(client, id));
    expect(after).toBe(before);

    await client.close();
    await server.close();
  });

  it("undo after a batch rewinds the entire batch", async () => {
    const { client, server } = await connect(engine);
    const id = await openDoc(client);

    await client.callTool({
      name: "apply_edits",
      arguments: {
        document_id: id,
        ops: [
          { op: "create", type: "cube", params: { size: { x: 3, y: 3, z: 3 } } },
          { op: "create", type: "cube", params: { size: { x: 3, y: 3, z: 3 } } },
          { op: "create", type: "sphere", params: { radius: 2 } },
        ],
      },
    });
    expect((await getDoc(client, id)).roots).toHaveLength(3);

    // A single undo rewinds all three creates at once.
    const u = await client.callTool({
      name: "undo",
      arguments: { document_id: id },
    });
    expect(JSON.parse(firstText(u)).success).toBe(true);
    expect((await getDoc(client, id)).roots).toHaveLength(0);

    await client.close();
    await server.close();
  });

  it("dry_run reports the per-op plan and mutates nothing", async () => {
    const { client, server } = await connect(engine);
    const id = await openDoc(client);
    const before = JSON.stringify(await getDoc(client, id));

    const res = await client.callTool({
      name: "apply_edits",
      arguments: {
        document_id: id,
        dry_run: true,
        ops: [
          { op: "create", type: "cube", params: { size: { x: 1, y: 1, z: 1 } } },
          { op: "create", type: "sphere", params: { radius: 1 } },
        ],
      },
    });
    expect(res.isError ?? false).toBe(false);
    const parsed = JSON.parse(firstText(res));
    expect(parsed.dry_run).toBe(true);
    expect(parsed.planned).toBe(2);
    expect(parsed.ops).toHaveLength(2);
    // Nothing committed: no changed / integrity block, document untouched.
    expect(parsed.changed).toBeUndefined();

    const after = JSON.stringify(await getDoc(client, id));
    expect(after).toBe(before);

    await client.close();
    await server.close();
  });

  it("rejects an over-cap batch with a clear error", async () => {
    const { client, server } = await connect(engine);
    const id = await openDoc(client);

    const ops = Array.from({ length: 51 }, () => ({
      op: "create",
      type: "cube",
      params: { size: { x: 1, y: 1, z: 1 } },
    }));
    const res = await client.callTool({
      name: "apply_edits",
      arguments: { document_id: id, ops },
    });
    expect(res.isError).toBe(true);
    expect(firstText(res)).toMatch(/cap/);

    await client.close();
    await server.close();
  });
});
