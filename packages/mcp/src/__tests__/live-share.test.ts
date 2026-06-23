import { describe, it, expect, beforeEach, afterEach } from "vitest";
import {
  SupabaseShareStore,
  NoopShareStore,
  createShareStore,
  resolveSessionIr,
  setSessionFetch,
  type ShareStore,
} from "../session-store.js";
import { shareSession, unshareSession } from "../tools/live-share.js";

const CFG = { supabaseUrl: "https://supa.test", serviceRoleKey: "svc" };
const json = (r: { content: Array<{ text: string }> }) => JSON.parse(r.content[0].text);

afterEach(() => {
  setSessionFetch(((...a: Parameters<typeof fetch>) => fetch(...a)) as typeof fetch);
});

/** In-memory live_shares fake routed by method. */
function makeSharesFake() {
  const rows = new Set<string>();
  const fetchImpl = (async (input: unknown, init: RequestInit = {}) => {
    const url = new URL(String(input));
    const method = (init.method ?? "GET").toUpperCase();
    const eq = url.searchParams.get("session_id");
    const id = eq && eq.startsWith("eq.") ? eq.slice(3) : null;
    if (method === "POST") {
      const body = JSON.parse(String(init.body)) as Array<{ session_id: string }>;
      for (const r of body) rows.add(r.session_id);
      return new Response(null, { status: 201 });
    }
    if (method === "DELETE") {
      if (id) rows.delete(id);
      return new Response(null, { status: 204 });
    }
    // GET
    return new Response(JSON.stringify(id && rows.has(id) ? [{ session_id: id }] : []), {
      status: 200,
      headers: { "Content-Type": "application/json" },
    });
  }) as unknown as typeof fetch;
  return { rows, fetchImpl };
}

describe("SupabaseShareStore", () => {
  it("share → isShared true; unshare → isShared false", async () => {
    setSessionFetch(makeSharesFake().fetchImpl);
    const store = new SupabaseShareStore(CFG);
    expect(await store.isShared("doc_x")).toBe(false);
    await store.share("doc_x", "user-1");
    expect(await store.isShared("doc_x")).toBe(true);
    await store.unshare("doc_x");
    expect(await store.isShared("doc_x")).toBe(false);
  });

  it("never throws on a transport error (degrades to not-shared)", async () => {
    setSessionFetch((() => {
      throw new Error("down");
    }) as unknown as typeof fetch);
    const store = new SupabaseShareStore(CFG);
    expect(await store.isShared("doc_x")).toBe(false);
    await expect(store.share("doc_x", null)).resolves.toBeUndefined();
  });
});

describe("createShareStore factory", () => {
  let prev: Record<string, string | undefined>;
  beforeEach(() => {
    prev = { u: process.env.SUPABASE_URL, k: process.env.SUPABASE_SERVICE_ROLE_KEY };
  });
  afterEach(() => {
    if (prev.u === undefined) delete process.env.SUPABASE_URL;
    else process.env.SUPABASE_URL = prev.u;
    if (prev.k === undefined) delete process.env.SUPABASE_SERVICE_ROLE_KEY;
    else process.env.SUPABASE_SERVICE_ROLE_KEY = prev.k;
  });
  it("Supabase store with env, no-op without", () => {
    process.env.SUPABASE_URL = "https://supa.test";
    process.env.SUPABASE_SERVICE_ROLE_KEY = "k";
    expect(createShareStore()).toBeInstanceOf(SupabaseShareStore);
    delete process.env.SUPABASE_URL;
    delete process.env.SUPABASE_SERVICE_ROLE_KEY;
    expect(createShareStore()).toBeInstanceOf(NoopShareStore);
  });
});

describe("resolveSessionIr", () => {
  const DOC = { nodes: { "1": { id: 1, op: { type: "Cube" } } }, roots: [{ root: 1 }] };
  let prev: Record<string, string | undefined>;
  beforeEach(() => {
    prev = { u: process.env.SUPABASE_URL, k: process.env.SUPABASE_SERVICE_ROLE_KEY };
    process.env.SUPABASE_URL = "https://supa.test";
    process.env.SUPABASE_SERVICE_ROLE_KEY = "svc";
  });
  afterEach(() => {
    if (prev.u === undefined) delete process.env.SUPABASE_URL;
    else process.env.SUPABASE_URL = prev.u;
    if (prev.k === undefined) delete process.env.SUPABASE_SERVICE_ROLE_KEY;
    else process.env.SUPABASE_SERVICE_ROLE_KEY = prev.k;
  });

  const fake = (handler: (table: string, id: string | null) => unknown[]) =>
    setSessionFetch((async (input: unknown) => {
      const url = new URL(String(input));
      const table = url.pathname.endsWith("/mcp_sessions")
        ? "mcp_sessions"
        : url.pathname.endsWith("/documents")
          ? "documents"
          : "?";
      const idP = url.searchParams.get(table === "mcp_sessions" ? "document_id" : "local_id");
      const id = idP && idP.startsWith("eq.") ? idP.slice(3) : null;
      return new Response(JSON.stringify(handler(table, id)), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      });
    }) as unknown as typeof fetch);

  it("resolves from mcp_sessions (anon capability session)", async () => {
    fake((t) => (t === "mcp_sessions" ? [{ content: DOC }] : []));
    const ir = await resolveSessionIr("doc_anon");
    expect(ir).not.toBeNull();
    expect(Object.keys(ir!.nodes)).toContain("1");
  });

  it("falls back to documents by mcp:<id> when mcp_sessions misses", async () => {
    fake((t, id) => (t === "documents" && id === "mcp:doc_signed" ? [{ content: DOC }] : []));
    const ir = await resolveSessionIr("doc_signed");
    expect(ir).not.toBeNull();
  });

  it("returns null when neither table has it", async () => {
    fake(() => []);
    expect(await resolveSessionIr("nope")).toBeNull();
  });

  it("returns null with no Supabase env", async () => {
    delete process.env.SUPABASE_URL;
    expect(await resolveSessionIr("doc_anon")).toBeNull();
  });
});

class FakeShareStore implements ShareStore {
  shared = new Set<string>();
  async isShared(id: string) {
    return this.shared.has(id);
  }
  async share(id: string) {
    this.shared.add(id);
  }
  async unshare(id: string) {
    this.shared.delete(id);
  }
}

describe("share_session / unshare_session tools", () => {
  let prevFlag: string | undefined;
  beforeEach(() => {
    prevFlag = process.env.VCAD_LIVE_WINDOW;
    process.env.VCAD_LIVE_WINDOW = "1";
  });
  afterEach(() => {
    if (prevFlag === undefined) delete process.env.VCAD_LIVE_WINDOW;
    else process.env.VCAD_LIVE_WINDOW = prevFlag;
  });

  it("share_session marks shared and returns a link + explicit public warning", async () => {
    const store = new FakeShareStore();
    const res = await shareSession({ document_id: "doc_1" }, store, { sub: "u1", email: "a@b.c" });
    expect(res.isError).toBeFalsy();
    const out = json(res);
    expect(out.shared).toBe(true);
    expect(out.link).toContain("/live/doc_1");
    expect(out.warning.toUpperCase()).toContain("PUBLIC");
    expect(await store.isShared("doc_1")).toBe(true);
  });

  it("share_session refuses when the live window is disabled", async () => {
    delete process.env.VCAD_LIVE_WINDOW;
    const res = await shareSession({ document_id: "doc_1" }, new FakeShareStore(), null);
    expect(res.isError).toBe(true);
  });

  it("share_session requires document_id", async () => {
    const res = await shareSession({}, new FakeShareStore(), null);
    expect(res.isError).toBe(true);
  });

  it("unshare_session revokes the share", async () => {
    const store = new FakeShareStore();
    await store.share("doc_1");
    const res = await unshareSession({ document_id: "doc_1" }, store);
    expect(json(res).shared).toBe(false);
    expect(await store.isShared("doc_1")).toBe(false);
  });
});
