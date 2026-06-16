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
 * End-to-end coverage of the dispatch persist/hydrate wrapper: drives the real
 * CallTool handler in createServer through an in-memory MCP client, with a fake
 * PostgREST backend, so the per-request session scope + persist gating are
 * exercised exactly as in production. (The persist wrapper was previously only
 * typecheck-verified.)
 */

interface Fake {
  rows: Map<string, Record<string, unknown>>;
  posts: number;
  gets: number;
}

function installFake(): Fake {
  const rows = new Map<string, Record<string, unknown>>();
  const fake: Fake = { rows, posts: 0, gets: 0 };
  const eq = (sp: URLSearchParams, k: string): string | null => {
    const v = sp.get(k);
    return v && v.startsWith("eq.") ? v.slice(3) : null;
  };
  setSessionFetch((async (input: unknown, init: RequestInit = {}) => {
    const url = new URL(String(input));
    const method = (init.method ?? "GET").toUpperCase();
    if (method === "POST") {
      fake.posts++;
      const body = JSON.parse(String(init.body)) as Array<
        Record<string, unknown>
      >;
      for (const r of body) rows.set(`${r.user_id}|${r.local_id}`, r);
      return new Response(null, { status: 201 });
    }
    const key = `${eq(url.searchParams, "user_id")}|${eq(url.searchParams, "local_id")}`;
    if (method === "GET") {
      fake.gets++;
      const row = rows.get(key);
      if (!row) return new Response("", { status: 406 });
      return new Response(JSON.stringify({ content: row.content }), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      });
    }
    return new Response(null, { status: 204 }); // DELETE
  }) as unknown as typeof fetch);
  return fake;
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

function firstText(result: unknown): string {
  const content = (result as { content: Array<{ type: string; text: string }> })
    .content;
  return content[0].text;
}

describe("dispatch persist wrapper (end-to-end through createServer)", () => {
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

  it("persists a creator, hydrates a reader from the store, and never persists a reader", async () => {
    const fake = installFake();
    const { client, server } = await connect(engine, {
      sub: "user-1",
      email: "u@x.test",
    });

    // Creator: open_document → persisted to the store on the way out.
    const open = await client.callTool({ name: "open_document", arguments: {} });
    const documentId = JSON.parse(firstText(open)).document_id as string;
    expect(fake.posts).toBe(1);
    expect(fake.rows.has(`user-1|mcp:${documentId}`)).toBe(true);

    const postsAfterOpen = fake.posts;

    // Reader: get_document runs in a fresh per-request scope, so it can only
    // succeed by hydrating from the durable store — and it must NOT persist.
    const got = await client.callTool({
      name: "get_document",
      arguments: { document_id: documentId },
    });
    expect(JSON.parse(firstText(got)).version).toBeDefined(); // IR round-tripped
    expect(fake.gets).toBeGreaterThanOrEqual(1); // hydrate read happened
    expect(fake.posts).toBe(postsAfterOpen); // reader did not write

    await client.close();
    await server.close();
  });

  it("persists a switch-path writer whose id is only in result text (create_schematic)", async () => {
    const fake = installFake();
    const { client, server } = await connect(engine, {
      sub: "user-2",
      email: "v@x.test",
    });

    // create_schematic is NOT a uiTool, so it never gets structuredContent —
    // the persist site must fall back to scanning the result text for the id.
    const res = await client.callTool({
      name: "create_schematic",
      arguments: {},
    });
    expect(res.isError ?? false).toBe(false);
    const documentId = JSON.parse(firstText(res)).document_id as string;
    expect(fake.posts).toBe(1);
    expect(fake.rows.has(`user-2|mcp:${documentId}`)).toBe(true);

    await client.close();
    await server.close();
  });
});
