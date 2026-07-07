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
import type { Document } from "@vcad/ir";
import { createServer } from "../server.js";
import {
  AnonSupabaseSessionStore,
  setSessionFetch,
} from "../session-store.js";
import { documents, saveDocument, loadDocument, openDocument } from "../tools/session.js";

/**
 * Durable save_document / load_document — the hosted-deploy path.
 *
 * WHY: in production save_document wrote to the serverless filesystem, which
 * is read-only — it failed on 100% of calls (4/4 in telemetry, across four
 * builds). These tests drive the fixed behavior through the real CallTool
 * handler with a fake PostgREST backend, exactly like checkpoint.test.ts:
 * a signed-in save is a durable, name-keyed row in the caller's own documents
 * table and survives a simulated redeploy; an anonymous save is keyed by an
 * unguessable id (a name would be guessable across capability-keyed tenants).
 */

/** PostgREST stand-in serving both the user-owned `documents` table and the
 *  anonymous `mcp_sessions` table. `rows` is the durable backend — it
 *  survives a "redeploy" (cleared `documents` cache + fresh server). */
function installFake() {
  const rows = new Map<string, Record<string, unknown>>();
  const eq = (sp: URLSearchParams, k: string): string | null => {
    const v = sp.get(k);
    return v && v.startsWith("eq.") ? v.slice(3) : null;
  };
  const keyOf = (url: URL): string =>
    url.pathname.endsWith("/mcp_sessions")
      ? `anon|${eq(url.searchParams, "document_id")}`
      : `${eq(url.searchParams, "user_id")}|${eq(url.searchParams, "local_id")}`;
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
      for (const r of body) {
        rows.set(
          url.pathname.endsWith("/mcp_sessions")
            ? `anon|${r.document_id}`
            : `${r.user_id}|${r.local_id}`,
          r,
        );
      }
      return new Response(null, { status: 201 });
    }
    if (method === "GET") {
      const row = rows.get(keyOf(url));
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

function makeCubeDoc(): Document {
  return {
    version: "0.1",
    nodes: {
      "1": {
        id: 1,
        name: "test_cube",
        op: { type: "Cube", size: { x: 10, y: 10, z: 10 } },
      },
    },
    materials: {},
    part_materials: {},
    roots: [{ root: 1, material: "default" }],
  };
}

const USER = { sub: "user-save", email: "s@x.test" };

describe("durable save_document / load_document (hosted deploy)", () => {
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

  it("signed-in save is durable, name-keyed, and survives a redeploy", async () => {
    const fake = installFake();

    // Instance A: open a doc and save it by a human name.
    const a = await connect(engine, USER);
    const documentId = parse(
      await a.client.callTool({
        name: "open_document",
        arguments: { initial: makeCubeDoc() },
      }),
    ).document_id as string;

    const saved = parse(
      await a.client.callTool({
        name: "save_document",
        arguments: { document_id: documentId, name: "My Part" },
      }),
    );
    expect(saved.saved).toBe(true);
    // Name normalizes to the deterministic slug used as the reopen handle.
    expect(saved.name).toBe("my-part");
    // No filesystem path in the durable result — the old fs write is exactly
    // what failed on the read-only serverless filesystem.
    expect(saved.path).toBeUndefined();
    // The row landed in the CALLER's documents table under the saved: key.
    expect(fake.rows.has(`${USER.sub}|mcp:saved:my-part`)).toBe(true);
    await a.client.close();
    await a.server.close();

    // ── Redeploy: wipe warm caches, boot a fresh instance B on the same
    // durable backend.
    documents.clear();
    const b = await connect(engine, USER);

    // Load by either the original name or the slug — both normalize the same.
    const loaded = parse(
      await b.client.callTool({
        name: "load_document",
        arguments: { name: "My Part" },
      }),
    );
    expect(loaded.document_id).toMatch(/^doc_/);
    expect(loaded.parts).toBe(1);

    const got = parse(
      await b.client.callTool({
        name: "get_document",
        arguments: { document_id: loaded.document_id },
      }),
    );
    expect(Object.keys(got.nodes as Record<string, unknown>)).toContain("1");

    await b.client.close();
    await b.server.close();
  });

  it("rejects an unusable save name instead of writing junk", async () => {
    installFake();
    const { client, server } = await connect(engine, USER);
    const documentId = parse(
      await client.callTool({
        name: "open_document",
        arguments: { initial: makeCubeDoc() },
      }),
    ).document_id as string;

    const res = (await client.callTool({
      name: "save_document",
      arguments: { document_id: documentId, name: "///" },
    })) as { isError?: boolean; content: Array<{ text: string }> };
    expect(res.isError).toBe(true);
    expect(res.content[0].text).toMatch(/Invalid save name/);

    await client.close();
    await server.close();
  });

  it("errors clearly when a named save doesn't exist", async () => {
    installFake();
    const { client, server } = await connect(engine, USER);
    const res = (await client.callTool({
      name: "load_document",
      arguments: { name: "never-saved" },
    })) as { isError?: boolean; content: Array<{ text: string }> };
    expect(res.isError).toBe(true);
    expect(res.content[0].text).toMatch(/No saved document named "never-saved"/);
    await client.close();
    await server.close();
  });

  it("anonymous save mints an unguessable key, not a name-derived row", async () => {
    const fake = installFake();
    const store = new AnonSupabaseSessionStore({
      supabaseUrl: "https://supa.test",
      serviceRoleKey: "svc",
    });

    const open = openDocument({ initial: makeCubeDoc() });
    const documentId = JSON.parse(open.content[0].text).document_id as string;

    const saved = JSON.parse(
      (await saveDocument({ document_id: documentId, name: "board" }, store))
        .content[0].text,
    ) as { saved: boolean; name: string };
    expect(saved.saved).toBe(true);
    // The reopen handle is saved_<slug>_<random> — never the bare name, which
    // would be guessable across capability-keyed tenants.
    expect(saved.name).toMatch(/^saved_board_[A-Za-z0-9_-]+$/);
    expect(fake.rows.has(`anon|${saved.name}`)).toBe(true);
    expect(fake.rows.has("anon|board")).toBe(false);

    // Cold-cache load by the returned key round-trips the document.
    documents.clear();
    const loaded = JSON.parse(
      (await loadDocument({ name: saved.name }, store)).content[0].text,
    ) as { document_id: string; parts: number };
    expect(loaded.document_id).toMatch(/^doc_/);
    expect(loaded.parts).toBe(1);

    // The bare name alone must NOT resolve for an anonymous caller.
    const miss = await loadDocument({ name: "board" }, store);
    expect(miss.isError).toBe(true);
  });
});
