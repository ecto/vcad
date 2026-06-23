import { describe, it, expect, beforeEach, afterEach } from "vitest";
import {
  SupabaseShareStore,
  NoopShareStore,
  createShareStore,
  resolveSessionIr,
  setSessionFetch,
  type ShareStore,
  type ShareRecord,
} from "../session-store.js";
import { shareSession, unshareSession } from "../tools/live-share.js";

const CFG = { supabaseUrl: "https://supa.test", serviceRoleKey: "svc" };
const json = (r: { content: Array<{ text: string }> }) => JSON.parse(r.content[0].text);
const jsonResp = (body: unknown) =>
  new Response(JSON.stringify(body), { status: 200, headers: { "Content-Type": "application/json" } });

afterEach(() => {
  setSessionFetch(((...a: Parameters<typeof fetch>) => fetch(...a)) as typeof fetch);
});

/** In-memory live_shares fake (stores shared_by) routed by method. */
function makeSharesFake() {
  const rows = new Map<string, string | null>();
  const fetchImpl = (async (input: unknown, init: RequestInit = {}) => {
    const url = new URL(String(input));
    const method = (init.method ?? "GET").toUpperCase();
    const eq = url.searchParams.get("session_id");
    const id = eq && eq.startsWith("eq.") ? eq.slice(3) : null;
    if (method === "POST") {
      const body = JSON.parse(String(init.body)) as Array<{ session_id: string; shared_by?: string | null }>;
      for (const r of body) rows.set(r.session_id, r.shared_by ?? null);
      return new Response(null, { status: 201 });
    }
    if (method === "DELETE") {
      if (id) rows.delete(id);
      return new Response(null, { status: 204 });
    }
    return jsonResp(id && rows.has(id) ? [{ session_id: id, shared_by: rows.get(id) }] : []);
  }) as unknown as typeof fetch;
  return { rows, fetchImpl };
}

describe("SupabaseShareStore", () => {
  it("share → getShare returns the owner; unshare → null", async () => {
    setSessionFetch(makeSharesFake().fetchImpl);
    const store = new SupabaseShareStore(CFG);
    expect(await store.getShare("doc_x")).toBeNull();
    await store.share("doc_x", "user-1");
    expect(await store.getShare("doc_x")).toEqual({ shared_by: "user-1" });
    await store.unshare("doc_x");
    expect(await store.getShare("doc_x")).toBeNull();
  });

  it("never throws on a transport error (degrades to not-shared)", async () => {
    setSessionFetch((() => {
      throw new Error("down");
    }) as unknown as typeof fetch);
    const store = new SupabaseShareStore(CFG);
    expect(await store.getShare("doc_x")).toBeNull();
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

  it("resolves from mcp_sessions (anon capability session)", async () => {
    setSessionFetch((async (input: unknown) => {
      const url = new URL(String(input));
      return jsonResp(url.pathname.endsWith("/mcp_sessions") ? [{ content: DOC }] : []);
    }) as unknown as typeof fetch);
    const ir = await resolveSessionIr("doc_anon");
    expect(ir).not.toBeNull();
    expect(Object.keys(ir!.nodes)).toContain("1");
  });

  it("does NOT fall back to documents without an owner (anti-spoof)", async () => {
    let documentsQueried = false;
    setSessionFetch((async (input: unknown) => {
      const url = new URL(String(input));
      if (url.pathname.endsWith("/documents")) documentsQueried = true;
      return jsonResp([]); // mcp_sessions misses
    }) as unknown as typeof fetch);
    expect(await resolveSessionIr("doc_signed")).toBeNull();
    expect(documentsQueried).toBe(false);
  });

  it("scopes the documents fallback to the owner's user_id", async () => {
    let docSearch = "";
    setSessionFetch((async (input: unknown) => {
      const url = new URL(String(input));
      if (url.pathname.endsWith("/documents")) {
        docSearch = url.search;
        return jsonResp([{ content: DOC }]);
      }
      return jsonResp([]); // mcp_sessions misses
    }) as unknown as typeof fetch);
    const ir = await resolveSessionIr("doc_signed", "owner-1");
    expect(ir).not.toBeNull();
    expect(docSearch).toContain("local_id=eq.mcp%3Adoc_signed");
    expect(docSearch).toContain("user_id=eq.owner-1");
  });

  it("returns null with no Supabase env", async () => {
    delete process.env.SUPABASE_URL;
    expect(await resolveSessionIr("doc_anon", "owner-1")).toBeNull();
  });
});

class FakeShareStore implements ShareStore {
  shared = new Map<string, string | null>();
  async getShare(id: string): Promise<ShareRecord | null> {
    return this.shared.has(id) ? { shared_by: this.shared.get(id) ?? null } : null;
  }
  async share(id: string, by: string | null) {
    this.shared.set(id, by);
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
    expect(await store.getShare("doc_1")).toEqual({ shared_by: "u1" });
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
    await store.share("doc_1", null);
    const res = await unshareSession({ document_id: "doc_1" }, store);
    expect(json(res).shared).toBe(false);
    expect(await store.getShare("doc_1")).toBeNull();
  });
});
