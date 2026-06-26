import {
  describe,
  it,
  expect,
  beforeAll,
  beforeEach,
  afterEach,
  vi,
} from "vitest";
import { Engine } from "@vcad/engine";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { createServer } from "../server.js";
import {
  setSessionFetch,
  isSessionStoreDurable,
  isProductionDeploy,
  sessionStoreInfo,
  warnIfSessionStoreNotDurable,
} from "../session-store.js";
import { documents } from "../tools/session.js";

/**
 * Deploy smoke test for session durability — the regression guard for the prod
 * incident where a production redeploy dropped an open design session because
 * SUPABASE_SERVICE_ROLE_KEY was unset and the store silently fell back to
 * in-memory.
 *
 * The e2e cases create a doc on one instance, simulate a redeploy (wipe every
 * warm cache + boot a fresh instance against the same durable backend), and
 * assert the doc still loads when durable — AND is lost when not (so the test
 * actually catches the condition it guards). The unit cases pin the observable
 * surfaces (server_info / /health `durable` flag, the loud boot warning).
 */

/** PostgREST stand-in whose `rows` map IS the durable backend: it's closed over
 *  here, so it survives a "redeploy" (a cleared cache + a new server). */
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
      return new Response(JSON.stringify({ ok: true }), {
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

async function connect(engine: Engine, user: { sub: string; email: string }) {
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

const USER = { sub: "user-smoke", email: "s@x.test" };

// ─── env helpers ─────────────────────────────────────────────────────────────

const ENV_KEYS = [
  "SUPABASE_URL",
  "SUPABASE_SERVICE_ROLE_KEY",
  "VERCEL_ENV",
  "NODE_ENV",
] as const;

describe("session durability self-report", () => {
  let saved: Record<string, string | undefined>;

  beforeEach(() => {
    saved = {};
    for (const k of ENV_KEYS) saved[k] = process.env[k];
    for (const k of ENV_KEYS) delete process.env[k];
  });
  afterEach(() => {
    for (const k of ENV_KEYS) {
      if (saved[k] === undefined) delete process.env[k];
      else process.env[k] = saved[k];
    }
    vi.restoreAllMocks();
  });

  it("isSessionStoreDurable mirrors the createSessionStore env check", () => {
    expect(isSessionStoreDurable()).toBe(false);
    process.env.SUPABASE_URL = "https://supa.test";
    expect(isSessionStoreDurable()).toBe(false); // url alone isn't enough
    process.env.SUPABASE_SERVICE_ROLE_KEY = "svc";
    expect(isSessionStoreDurable()).toBe(true);
  });

  it("isProductionDeploy detects Vercel prod and NODE_ENV=production", () => {
    expect(isProductionDeploy()).toBe(false);
    process.env.VERCEL_ENV = "production";
    expect(isProductionDeploy()).toBe(true);
    delete process.env.VERCEL_ENV;
    process.env.NODE_ENV = "production";
    expect(isProductionDeploy()).toBe(true);
    process.env.VERCEL_ENV = "preview";
    process.env.NODE_ENV = "test";
    expect(isProductionDeploy()).toBe(false);
  });

  it("sessionStoreInfo reports durable:false / in-memory without the key", () => {
    expect(sessionStoreInfo()).toEqual({
      durable: false,
      session_store: "in-memory",
      production: false,
    });
    process.env.SUPABASE_URL = "https://supa.test";
    process.env.SUPABASE_SERVICE_ROLE_KEY = "svc";
    expect(sessionStoreInfo()).toEqual({
      durable: true,
      session_store: "supabase",
      production: false,
    });
  });

  it("warnIfSessionStoreNotDurable fires loudly ONLY in prod without a key", () => {
    const spy = vi.spyOn(console, "error").mockImplementation(() => {});

    // Not prod → silent.
    expect(warnIfSessionStoreNotDurable()).toBe(false);
    expect(spy).not.toHaveBeenCalled();

    // Prod + durable → silent.
    process.env.VERCEL_ENV = "production";
    process.env.SUPABASE_URL = "https://supa.test";
    process.env.SUPABASE_SERVICE_ROLE_KEY = "svc";
    expect(warnIfSessionStoreNotDurable()).toBe(false);
    expect(spy).not.toHaveBeenCalled();

    // Prod + NO key → loud warning.
    delete process.env.SUPABASE_SERVICE_ROLE_KEY;
    expect(warnIfSessionStoreNotDurable()).toBe(true);
    expect(spy).toHaveBeenCalledOnce();
    expect(String(spy.mock.calls[0][0])).toMatch(/DURABILITY DEGRADED/);
  });
});

describe("deploy smoke: a session survives a redeploy", () => {
  let engine: Engine;
  let saved: Record<string, string | undefined>;

  beforeAll(async () => {
    engine = await Engine.init();
  });

  beforeEach(() => {
    saved = {};
    for (const k of ENV_KEYS) saved[k] = process.env[k];
    documents.clear();
  });
  afterEach(() => {
    for (const k of ENV_KEYS) {
      if (saved[k] === undefined) delete process.env[k];
      else process.env[k] = saved[k];
    }
    setSessionFetch(((...a: Parameters<typeof fetch>) =>
      fetch(...a)) as typeof fetch);
  });

  it("DURABLE: doc created on instance A still loads on a fresh instance B", async () => {
    process.env.SUPABASE_URL = "https://supa.test";
    process.env.SUPABASE_SERVICE_ROLE_KEY = "svc";
    installFake();

    // Instance A creates a document.
    const a = await connect(engine, USER);
    const open = parse(await a.client.callTool({ name: "open_document", arguments: {} }));
    const documentId = open.document_id as string;
    // server_info confirms the deploy is durable.
    expect(parse(await a.client.callTool({ name: "server_info", arguments: {} })).durable).toBe(true);
    await a.client.close();
    await a.server.close();

    // ── Redeploy: wipe every warm cache, boot a fresh instance against the
    // same durable backend.
    documents.clear();
    const b = await connect(engine, USER);

    // The doc still loads — hydrated from the durable store, not lost.
    const got = await b.client.callTool({
      name: "get_document",
      arguments: { document_id: documentId },
    });
    expect((got as { isError?: boolean }).isError ?? false).toBe(false);
    expect(parse(got).version).toBeDefined();

    await b.client.close();
    await b.server.close();
  });

  it("NEGATIVE CONTROL: without the service-role key the doc is LOST on redeploy", async () => {
    delete process.env.SUPABASE_SERVICE_ROLE_KEY; // the prod-incident condition
    process.env.SUPABASE_URL = "https://supa.test";
    installFake();

    const a = await connect(engine, USER);
    const documentId = parse(
      await a.client.callTool({ name: "open_document", arguments: {} }),
    ).document_id as string;
    // server_info / /health would advertise the hazard.
    expect(parse(await a.client.callTool({ name: "server_info", arguments: {} })).durable).toBe(false);
    await a.client.close();
    await a.server.close();

    // Redeploy.
    documents.clear();
    const b = await connect(engine, USER);

    // In-memory store can't rehydrate → the open board is gone. This is exactly
    // the regression the durable path fixes; the negative control proves the
    // smoke test detects it.
    const got = (await b.client.callTool({
      name: "get_document",
      arguments: { document_id: documentId },
    })) as { isError?: boolean; content: Array<{ text: string }> };
    expect(got.isError).toBe(true);
    expect(got.content[0].text).toMatch(/Unknown document_id/);

    await b.client.close();
    await b.server.close();
  });
});
