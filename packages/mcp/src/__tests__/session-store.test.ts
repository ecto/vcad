import { describe, it, expect, beforeEach, afterEach } from "vitest";
import type { Document } from "@vcad/ir";
import {
  SupabaseSessionStore,
  AnonSupabaseSessionStore,
  InMemorySessionStore,
  createSessionStore,
  setSessionFetch,
} from "../session-store.js";
import {
  documents,
  hydrateSession,
  persistSession,
  getSession,
  getDocumentTool,
  runInSessionScope,
} from "../tools/session.js";

/** Minimal one-cube Document, same shape used across tools.test.ts. */
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
  } as unknown as Document;
}

/**
 * In-memory PostgREST stand-in for the `documents` table. Routes by method +
 * `user_id` / `local_id` query filters, so it exercises the real
 * SupabaseSessionStore request shaping (URLs, headers, body) without a network.
 */
function makePostgrestFake() {
  const rows = new Map<string, Record<string, unknown>>();
  const seenUrls: string[] = [];
  const key = (userId: string | null, localId: string | null) =>
    `${userId}|${localId}`;
  const eqParam = (sp: URLSearchParams, k: string): string | null => {
    const v = sp.get(k);
    return v && v.startsWith("eq.") ? v.slice(3) : null;
  };

  const fetchImpl = (async (input: unknown, init: RequestInit = {}) => {
    const urlStr = String(input);
    seenUrls.push(urlStr);
    const url = new URL(urlStr);
    const sp = url.searchParams;
    const method = (init.method ?? "GET").toUpperCase();

    if (method === "POST") {
      const body = JSON.parse(String(init.body)) as Array<
        Record<string, unknown>
      >;
      for (const r of body) {
        rows.set(key(String(r.user_id), String(r.local_id)), r);
      }
      return new Response(null, { status: 201 });
    }

    const k = key(eqParam(sp, "user_id"), eqParam(sp, "local_id"));
    if (method === "GET") {
      const row = rows.get(k);
      if (!row) return new Response("", { status: 406 }); // PostgREST: ≠1 row
      return new Response(JSON.stringify({ content: row.content }), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      });
    }
    if (method === "DELETE") {
      rows.delete(k);
      return new Response(null, { status: 204 });
    }
    return new Response("unexpected", { status: 400 });
  }) as unknown as typeof fetch;

  return { rows, seenUrls, fetchImpl };
}

const CFG = {
  supabaseUrl: "https://supa.test",
  serviceRoleKey: "service-role-key",
  userId: "user-me",
};

afterEach(() => {
  documents.clear();
  setSessionFetch(((...args: Parameters<typeof fetch>) =>
    fetch(...args)) as typeof fetch);
});

describe("SupabaseSessionStore", () => {
  it("round-trips a document through save → load", async () => {
    const fake = makePostgrestFake();
    setSessionFetch(fake.fetchImpl);
    const store = new SupabaseSessionStore(CFG);

    const doc = makeCubeDoc();
    await store.save("doc_a", doc);
    const loaded = await store.load("doc_a");

    expect(loaded).toEqual(doc);
    // Defensive copy — mutating the load result must not touch the backend.
    (loaded as Document).roots.push({ root: 999, material: "x" } as never);
    const again = await store.load("doc_a");
    expect((again as Document).roots).toHaveLength(1);
  });

  it("returns null on a miss (never throws)", async () => {
    const fake = makePostgrestFake();
    setSessionFetch(fake.fetchImpl);
    const store = new SupabaseSessionStore(CFG);
    expect(await store.load("doc_absent")).toBeNull();
  });

  it("scopes reads by user_id — no cross-user leak", async () => {
    const fake = makePostgrestFake();
    setSessionFetch(fake.fetchImpl);

    const mallory = new SupabaseSessionStore({ ...CFG, userId: "user-other" });
    await mallory.save("doc_secret", makeCubeDoc());

    // Same document_id, different caller → ownership filter excludes it.
    const me = new SupabaseSessionStore(CFG);
    expect(await me.load("doc_secret")).toBeNull();
    // The owner can still read it.
    expect(await mallory.load("doc_secret")).not.toBeNull();
  });

  it("keys rows by mcp:<documentId> and the caller's user_id", async () => {
    const fake = makePostgrestFake();
    setSessionFetch(fake.fetchImpl);
    await new SupabaseSessionStore(CFG).save("doc_x", makeCubeDoc());

    expect(fake.rows.has("user-me|mcp:doc_x")).toBe(true);
    // Every request carries the ownership filter / conflict target.
    expect(fake.seenUrls.some((u) => u.includes("on_conflict=user_id"))).toBe(
      true,
    );
  });

  it("drops a row by (user_id, local_id)", async () => {
    const fake = makePostgrestFake();
    setSessionFetch(fake.fetchImpl);
    const store = new SupabaseSessionStore(CFG);
    await store.save("doc_d", makeCubeDoc());
    expect(fake.rows.size).toBe(1);
    await store.drop("doc_d");
    expect(fake.rows.size).toBe(0);
  });
});

describe("cold-instance recovery (hydrate on cache miss)", () => {
  it("rehydrates a cleared cache from the durable store", async () => {
    const fake = makePostgrestFake();
    setSessionFetch(fake.fetchImpl);
    const store = new SupabaseSessionStore(CFG);

    // Seed the durable store, then simulate a cold serverless instance: the
    // warm cache is empty even though the row exists.
    await store.save("doc_cold", makeCubeDoc());
    documents.clear();
    expect(documents.has("doc_cold")).toBe(false);

    const hydrated = await hydrateSession(store, "doc_cold");
    expect(hydrated).toBe(true);
    expect(documents.has("doc_cold")).toBe(true);

    // The reader path now succeeds instead of throwing "Unknown document_id".
    const fetched = JSON.parse(
      getDocumentTool({ document_id: "doc_cold" }).content[0].text,
    ) as Document;
    expect(fetched.roots).toHaveLength(1);
    expect(Object.keys(fetched.nodes)).toContain("1");
  });
});

describe("per-tenant cache isolation (runInSessionScope)", () => {
  const userA = { sub: "user-A", email: "a@x.test" };
  const userB = { sub: "user-B", email: "b@x.test" };

  it("user B cannot read user A's warm-cached document by id", async () => {
    const fake = makePostgrestFake();
    setSessionFetch(fake.fetchImpl);
    const storeA = new SupabaseSessionStore({ ...CFG, userId: userA.sub });
    const storeB = new SupabaseSessionStore({ ...CFG, userId: userB.sub });

    // A, inside A's scope, creates + persists a session and can read it back.
    await runInSessionScope(userA, async () => {
      documents.set("doc_shared", makeCubeDoc());
      await persistSession(storeA, "doc_shared");
      expect(documents.has("doc_shared")).toBe(true);
    });

    // B, inside B's own (fresh, isolated) scope, must NOT see A's session —
    // not from the cache (per-request Map is empty) and not via hydrate (B's
    // store is scoped to B's user_id, so A's row is invisible).
    await runInSessionScope(userB, async () => {
      expect(documents.has("doc_shared")).toBe(false);
      await hydrateSession(storeB, "doc_shared");
      expect(documents.has("doc_shared")).toBe(false);
      expect(() => getSession("doc_shared")).toThrow(/Unknown document_id/);
    });

    // And it never leaked into the process-wide fallback either.
    expect(documents.has("doc_shared")).toBe(false);
  });

  it("the same user sees their session across requests via the durable store", async () => {
    const fake = makePostgrestFake();
    setSessionFetch(fake.fetchImpl);
    const storeA = new SupabaseSessionStore({ ...CFG, userId: userA.sub });

    await runInSessionScope(userA, async () => {
      documents.set("doc_mine", makeCubeDoc());
      await persistSession(storeA, "doc_mine");
    });
    // A later request: fresh empty scope, but hydrate pulls it from A's store.
    await runInSessionScope(userA, async () => {
      expect(documents.has("doc_mine")).toBe(false);
      expect(await hydrateSession(storeA, "doc_mine")).toBe(true);
      expect(getSession("doc_mine").roots).toHaveLength(1);
    });
  });
});

describe("in-memory fallback (no user / no service-role key)", () => {
  it("load always misses and getSession still throws the pinned error", async () => {
    const store = new InMemorySessionStore();
    expect(await store.load()).toBeNull();
    expect(await hydrateSession(store, "doc_missing")).toBe(false);
    expect(() => getSession("doc_missing")).toThrow(/Unknown document_id/);
  });
});

/** PostgREST stand-in for the `mcp_sessions` table — routes by document_id. */
function makeMcpSessionsFake() {
  const rows = new Map<string, Record<string, unknown>>();
  const eqParam = (sp: URLSearchParams, k: string): string | null => {
    const v = sp.get(k);
    return v && v.startsWith("eq.") ? v.slice(3) : null;
  };
  const fetchImpl = (async (input: unknown, init: RequestInit = {}) => {
    const url = new URL(String(input));
    const method = (init.method ?? "GET").toUpperCase();
    if (method === "POST") {
      const body = JSON.parse(String(init.body)) as Array<Record<string, unknown>>;
      for (const r of body) rows.set(String(r.document_id), r);
      return new Response(null, { status: 201 });
    }
    const id = eqParam(url.searchParams, "document_id");
    if (method === "GET") {
      const row = id ? rows.get(id) : undefined;
      if (!row) return new Response("", { status: 406 });
      return new Response(JSON.stringify({ content: row.content }), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      });
    }
    if (method === "DELETE") {
      if (id) rows.delete(id);
      return new Response(null, { status: 204 });
    }
    return new Response("unexpected", { status: 400 });
  }) as unknown as typeof fetch;
  return { rows, fetchImpl };
}

describe("AnonSupabaseSessionStore (capability-keyed, anonymous)", () => {
  const ACFG = { supabaseUrl: "https://supa.test", serviceRoleKey: "service-role-key" };

  it("round-trips an anonymous document by id", async () => {
    setSessionFetch(makeMcpSessionsFake().fetchImpl);
    const store = new AnonSupabaseSessionStore(ACFG);
    await store.save("doc_anon_1", makeCubeDoc());
    const loaded = await store.load("doc_anon_1");
    expect(loaded).not.toBeNull();
    expect(JSON.stringify(loaded)).toContain("test_cube");
  });

  it("misses (null) for an unknown id", async () => {
    setSessionFetch(makeMcpSessionsFake().fetchImpl);
    expect(await new AnonSupabaseSessionStore(ACFG).load("nope")).toBeNull();
  });

  it("drops a session", async () => {
    setSessionFetch(makeMcpSessionsFake().fetchImpl);
    const store = new AnonSupabaseSessionStore(ACFG);
    await store.save("doc_d", makeCubeDoc());
    await store.drop("doc_d");
    expect(await store.load("doc_d")).toBeNull();
  });

  it("hydrates a cold instance from mcp_sessions (cross-instance recovery)", async () => {
    setSessionFetch(makeMcpSessionsFake().fetchImpl);
    await new AnonSupabaseSessionStore(ACFG).save("doc_cold", makeCubeDoc());
    documents.clear(); // simulate a fresh instance with an empty cache
    const ok = await hydrateSession(new AnonSupabaseSessionStore(ACFG), "doc_cold");
    expect(ok).toBe(true);
    expect(() => getSession("doc_cold")).not.toThrow();
  });
});

describe("createSessionStore factory", () => {
  let prevUrl: string | undefined;
  let prevKey: string | undefined;

  beforeEach(() => {
    prevUrl = process.env.SUPABASE_URL;
    prevKey = process.env.SUPABASE_SERVICE_ROLE_KEY;
  });
  afterEach(() => {
    if (prevUrl === undefined) delete process.env.SUPABASE_URL;
    else process.env.SUPABASE_URL = prevUrl;
    if (prevKey === undefined) delete process.env.SUPABASE_SERVICE_ROLE_KEY;
    else process.env.SUPABASE_SERVICE_ROLE_KEY = prevKey;
  });

  it("returns the anonymous capability store without a user but with keys", () => {
    process.env.SUPABASE_URL = "https://supa.test";
    process.env.SUPABASE_SERVICE_ROLE_KEY = "k";
    expect(createSessionStore(null)).toBeInstanceOf(AnonSupabaseSessionStore);
  });

  it("returns in-memory without a user AND no keys (stdio/local)", () => {
    delete process.env.SUPABASE_URL;
    delete process.env.SUPABASE_SERVICE_ROLE_KEY;
    expect(createSessionStore(null)).toBeInstanceOf(InMemorySessionStore);
  });

  it("returns in-memory with a user but no service-role key", () => {
    process.env.SUPABASE_URL = "https://supa.test";
    delete process.env.SUPABASE_SERVICE_ROLE_KEY;
    expect(
      createSessionStore({ sub: "user-me", email: "a@b.c" }),
    ).toBeInstanceOf(InMemorySessionStore);
  });

  it("returns the Supabase store with a user + url + key", () => {
    process.env.SUPABASE_URL = "https://supa.test/";
    process.env.SUPABASE_SERVICE_ROLE_KEY = "k";
    expect(
      createSessionStore({ sub: "user-me", email: "a@b.c" }),
    ).toBeInstanceOf(SupabaseSessionStore);
  });
});
