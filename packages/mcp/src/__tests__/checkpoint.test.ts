import {
  describe,
  it,
  expect,
  beforeAll,
  beforeEach,
  afterEach,
} from "vitest";
import { Engine } from "@vcad/engine";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { createServer } from "../server.js";
import { setSessionFetch } from "../session-store.js";
import { documents } from "../tools/session.js";

/**
 * End-to-end coverage of checkpoint_document / branch_from, driven through the
 * real CallTool handler in createServer over an in-memory MCP client, with a
 * fake PostgREST backend so checkpoints persist (and survive a simulated
 * redeploy) exactly as in production.
 */

/** PostgREST + append_session_event stand-in. The `rows` map is the durable
 *  backend — it survives a "redeploy" (a cleared `documents` cache + a fresh
 *  server) because it's closed over here, not in instance memory. */
function installFake() {
  const rows = new Map<string, Record<string, unknown>>();
  const eq = (sp: URLSearchParams, k: string): string | null => {
    const v = sp.get(k);
    return v && v.startsWith("eq.") ? v.slice(3) : null;
  };
  setSessionFetch((async (input: unknown, init: RequestInit = {}) => {
    const url = new URL(String(input));
    const method = (init.method ?? "GET").toUpperCase();
    if (method === "POST" && url.pathname.endsWith("/rpc/append_session_event")) {
      return new Response(JSON.stringify({ ok: true, id: 1, seq: 1 }), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      });
    }
    if (method === "POST") {
      const body = JSON.parse(String(init.body)) as Array<
        Record<string, unknown>
      >;
      for (const r of body) rows.set(`${r.user_id}|${r.local_id}`, r);
      return new Response(null, { status: 201 });
    }
    const key = `${eq(url.searchParams, "user_id")}|${eq(url.searchParams, "local_id")}`;
    if (method === "GET") {
      const row = rows.get(key);
      if (!row) return new Response("", { status: 406 });
      return new Response(JSON.stringify({ content: row.content }), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      });
    }
    return new Response(null, { status: 204 });
  }) as unknown as typeof fetch);
  return { rows };
}

async function connect(
  engine: Engine,
  user: { sub: string; email: string } | null,
) {
  const server = await createServer(engine, { user });
  const [clientT, serverT] = InMemoryTransport.createLinkedPair();
  const client = new Client(
    { name: "test", version: "0.0.0" },
    { capabilities: {} },
  );
  await Promise.all([client.connect(clientT), server.connect(serverT)]);
  return { client, server };
}

function parse(result: unknown): Record<string, unknown> {
  const content = (result as { content: Array<{ type: string; text: string }> })
    .content;
  return JSON.parse(content[0].text) as Record<string, unknown>;
}

/** A two-resistor schematic with one named net — the netlist anchor. */
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

const USER = { sub: "user-ckpt", email: "c@x.test" };

describe("checkpoint_document / branch_from", () => {
  let engine: Engine;
  let prevUrl: string | undefined;
  let prevKey: string | undefined;

  beforeAll(async () => {
    engine = await Engine.init();
  });

  beforeEach(() => {
    prevUrl = process.env.SUPABASE_URL;
    prevKey = process.env.SUPABASE_SERVICE_ROLE_KEY;
    process.env.SUPABASE_URL = "https://supa.test";
    process.env.SUPABASE_SERVICE_ROLE_KEY = "svc";
    documents.clear();
  });

  afterEach(() => {
    if (prevUrl === undefined) delete process.env.SUPABASE_URL;
    else process.env.SUPABASE_URL = prevUrl;
    if (prevKey === undefined) delete process.env.SUPABASE_SERVICE_ROLE_KEY;
    else process.env.SUPABASE_SERVICE_ROLE_KEY = prevKey;
    setSessionFetch(((...a: Parameters<typeof fetch>) =>
      fetch(...a)) as typeof fetch);
  });

  it("snapshots a session and reports the netlist anchor in the summary", async () => {
    installFake();
    const { client, server } = await connect(engine, USER);

    const created = parse(
      await client.callTool({
        name: "create_schematic",
        arguments: SCHEMATIC_ARGS,
      }),
    );
    const documentId = created.document_id as string;

    const ckpt = parse(
      await client.callTool({
        name: "checkpoint_document",
        arguments: { document_id: documentId, label: "post-schematic" },
      }),
    );

    expect(typeof ckpt.checkpoint_id).toBe("string");
    expect(ckpt.checkpoint_id).not.toBe(documentId);
    expect(ckpt.of).toBe(documentId);
    expect(ckpt.label).toBe("post-schematic");
    const summary = ckpt.summary as Record<string, number>;
    // The netlist is the anchor — the summary surfaces it so the agent can see
    // it was captured.
    expect(summary.schematic_nets).toBe(1);
    expect(summary.schematic_components).toBe(2);

    await client.close();
    await server.close();
  });

  it("branches into a NEW session id with the netlist preserved", async () => {
    installFake();
    const { client, server } = await connect(engine, USER);

    const documentId = (
      parse(
        await client.callTool({
          name: "create_schematic",
          arguments: SCHEMATIC_ARGS,
        }),
      ).document_id as string
    );
    const checkpointId = parse(
      await client.callTool({
        name: "checkpoint_document",
        arguments: { document_id: documentId },
      }),
    ).checkpoint_id as string;

    const branch = parse(
      await client.callTool({
        name: "branch_from",
        arguments: { checkpoint_id: checkpointId },
      }),
    );

    expect(branch.branched_from).toBe(checkpointId);
    const branchId = branch.document_id as string;
    expect(branchId).not.toBe(documentId);
    expect(branchId).not.toBe(checkpointId);
    expect((branch.summary as Record<string, number>).schematic_nets).toBe(1);

    // The branch is a real, readable session with the same netlist.
    const got = parse(
      await client.callTool({
        name: "get_document",
        arguments: { document_id: branchId },
      }),
    );
    expect((got.schematic as { nets: Record<string, unknown> }).nets.MID).toBeDefined();

    await client.close();
    await server.close();
  });

  it("restores a checkpoint in place when given `into` (same id)", async () => {
    installFake();
    const { client, server } = await connect(engine, USER);

    const documentId = parse(
      await client.callTool({
        name: "create_schematic",
        arguments: SCHEMATIC_ARGS,
      }),
    ).document_id as string;
    const checkpointId = parse(
      await client.callTool({
        name: "checkpoint_document",
        arguments: { document_id: documentId },
      }),
    ).checkpoint_id as string;

    const restored = parse(
      await client.callTool({
        name: "branch_from",
        arguments: { checkpoint_id: checkpointId, into: documentId },
      }),
    );
    // Restore keeps the original id so existing handles keep working.
    expect(restored.document_id).toBe(documentId);
    expect(restored.restored_from).toBe(checkpointId);

    await client.close();
    await server.close();
  });

  it("a checkpoint survives a simulated redeploy (branch from a cold instance)", async () => {
    const fake = installFake();
    // Instance A: build a netlist and checkpoint it.
    const a = await connect(engine, USER);
    const documentId = parse(
      await a.client.callTool({
        name: "create_schematic",
        arguments: SCHEMATIC_ARGS,
      }),
    ).document_id as string;
    const checkpointId = parse(
      await a.client.callTool({
        name: "checkpoint_document",
        arguments: { document_id: documentId },
      }),
    ).checkpoint_id as string;
    // The checkpoint reached the durable backend.
    expect(fake.rows.has(`${USER.sub}|mcp:${checkpointId}`)).toBe(true);
    await a.client.close();
    await a.server.close();

    // ── Redeploy: wipe every warm cache and boot a fresh instance B against
    // the SAME durable backend.
    documents.clear();
    const b = await connect(engine, USER);

    // Branch_from on the cold instance must rehydrate the checkpoint from the
    // store — not "Unknown checkpoint_id".
    const branch = parse(
      await b.client.callTool({
        name: "branch_from",
        arguments: { checkpoint_id: checkpointId },
      }),
    );
    expect(branch.isError ?? false).toBe(false);
    expect(branch.branched_from).toBe(checkpointId);
    expect((branch.summary as Record<string, number>).schematic_nets).toBe(1);

    await b.client.close();
    await b.server.close();
  });

  it("errors clearly on an unknown checkpoint_id", async () => {
    installFake();
    const { client, server } = await connect(engine, USER);
    const res = (await client.callTool({
      name: "branch_from",
      arguments: { checkpoint_id: "ckpt_nope" },
    })) as { isError?: boolean; content: Array<{ text: string }> };
    expect(res.isError).toBe(true);
    expect(res.content[0].text).toMatch(/Unknown checkpoint_id/);
    await client.close();
    await server.close();
  });
});
