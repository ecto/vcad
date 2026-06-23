import { describe, it, expect, beforeEach, afterEach } from "vitest";
import { Readable } from "node:stream";
import type { IncomingMessage, ServerResponse } from "node:http";
import { handleLiveRequest } from "../live-route.js";
import { setSessionFetch } from "../session-store.js";

function makeReq(method: string, path: string, body?: string): IncomingMessage {
  const req = body != null ? Readable.from([Buffer.from(body)]) : Readable.from([]);
  Object.assign(req, {
    method,
    url: path,
    headers: { host: "mcp.vcad.io" },
    socket: { remoteAddress: "127.0.0.1" },
  });
  return req as unknown as IncomingMessage;
}

interface CapRes {
  statusCode: number;
  headers: Record<string, string>;
  body: string;
  ended: boolean;
  writeHead(s: number, h?: Record<string, string>): CapRes;
  end(b?: string | Buffer): void;
}
function makeRes(): CapRes {
  return {
    statusCode: 0,
    headers: {},
    body: "",
    ended: false,
    writeHead(s, h) {
      this.statusCode = s;
      if (h) this.headers = h;
      return this;
    },
    end(b) {
      this.body = typeof b === "string" ? b : b ? `<${b.length} bytes>` : "";
      this.ended = true;
    },
  };
}
const res = () => makeRes() as unknown as ServerResponse & CapRes;

/**
 * PostgREST/RPC fake routed by pathname. `shared` = ids with a live_shares row;
 * `docs` = session ids resolvable to geometry (mcp_sessions).
 */
function installFake(opts: { shared?: string[]; docs?: Record<string, unknown> } = {}) {
  const sharedSet = new Set(opts.shared ?? []);
  const docs = opts.docs ?? {};
  const eq = (sp: URLSearchParams, k: string) => {
    const v = sp.get(k);
    return v && v.startsWith("eq.") ? v.slice(3) : null;
  };
  setSessionFetch((async (input: unknown, init: RequestInit = {}) => {
    void init;
    const url = new URL(String(input));
    const p = url.pathname;
    const j = (body: unknown, status = 200) =>
      new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });

    if (p.endsWith("/live_shares")) {
      const id = eq(url.searchParams, "session_id");
      return j(id && sharedSet.has(id) ? [{ session_id: id }] : []);
    }
    if (p.endsWith("/session_events")) return j([]);
    if (p.endsWith("/rpc/append_session_event")) return j({ ok: true, id: 1, seq: 1 });
    if (p.endsWith("/mcp_sessions")) {
      const id = eq(url.searchParams, "document_id");
      return j(id && docs[id] ? [{ content: docs[id] }] : []);
    }
    if (p.endsWith("/documents")) return j([]);
    return j([], 404);
  }) as unknown as typeof fetch);
}

const SHARED = "doc_shared";

let saved: Record<string, string | undefined>;
beforeEach(() => {
  saved = {
    url: process.env.SUPABASE_URL,
    key: process.env.SUPABASE_SERVICE_ROLE_KEY,
    flag: process.env.VCAD_LIVE_WINDOW,
  };
  process.env.SUPABASE_URL = "https://supa.test";
  process.env.SUPABASE_SERVICE_ROLE_KEY = "svc";
  process.env.VCAD_LIVE_WINDOW = "1";
  installFake({ shared: [SHARED] });
});
afterEach(() => {
  for (const [k, v] of [
    ["SUPABASE_URL", saved.url],
    ["SUPABASE_SERVICE_ROLE_KEY", saved.key],
    ["VCAD_LIVE_WINDOW", saved.flag],
  ] as const) {
    if (v === undefined) delete process.env[k];
    else process.env[k] = v;
  }
  setSessionFetch(((...a: Parameters<typeof fetch>) => fetch(...a)) as typeof fetch);
});

describe("handleLiveRequest — flag + share gate", () => {
  it("returns false for a non-/live path", async () => {
    const r = res();
    expect(await handleLiveRequest(makeReq("GET", "/mcp"), r, { user: null })).toBe(false);
    expect(r.ended).toBe(false);
  });

  it("404s every /live path when the flag is off", async () => {
    process.env.VCAD_LIVE_WINDOW = "0";
    const r = res();
    await handleLiveRequest(makeReq("GET", `/live/${SHARED}/events`), r, { user: null });
    expect(r.statusCode).toBe(404);
  });

  it("404s a session that was never shared (private by default)", async () => {
    const r = res();
    await handleLiveRequest(makeReq("GET", "/live/doc_private/events"), r, { user: null });
    expect(r.statusCode).toBe(404);
  });

  it("serves events for a SHARED session", async () => {
    const r = res();
    await handleLiveRequest(makeReq("GET", `/live/${SHARED}/events`), r, { user: null });
    expect(r.statusCode).toBe(200);
    const parsed = JSON.parse(r.body) as { session_id: string; events: unknown[] };
    expect(parsed.session_id).toBe(SHARED);
    expect(Array.isArray(parsed.events)).toBe(true);
  });

  it("accepts an annotation on a shared session", async () => {
    const r = res();
    await handleLiveRequest(
      makeReq("POST", `/live/${SHARED}/annotate`, JSON.stringify({ type: "pin", payload: {} })),
      r,
      { user: null },
    );
    expect(r.statusCode).toBe(200);
    expect(JSON.parse(r.body).ok).toBe(true);
  });

  it("rejects an invalid overlay type (400) on a shared session", async () => {
    const r = res();
    await handleLiveRequest(
      makeReq("POST", `/live/${SHARED}/annotate`, JSON.stringify({ type: "mutation" })),
      r,
      { user: null },
    );
    expect(r.statusCode).toBe(400);
  });

  it("400s a missing session id before the gate", async () => {
    const r = res();
    await handleLiveRequest(makeReq("GET", "/live/"), r, { user: null });
    expect(r.statusCode).toBe(400);
  });
});

describe("handleLiveRequest — glb route", () => {
  it("404s glb when the session isn't shared", async () => {
    const r = res();
    await handleLiveRequest(makeReq("GET", "/live/doc_private/glb"), r, { user: null });
    expect(r.statusCode).toBe(404);
  });

  it("404s glb for a shared session with no resolvable geometry", async () => {
    const r = res();
    await handleLiveRequest(makeReq("GET", `/live/${SHARED}/glb`), r, { user: null });
    expect(r.statusCode).toBe(404); // resolveSessionIr returns null (no doc seeded)
  });

  it("503s glb when geometry resolves but no engine is provided", async () => {
    installFake({ shared: [SHARED], docs: { [SHARED]: { nodes: { "1": {} }, roots: [{ root: 1 }] } } });
    const r = res();
    await handleLiveRequest(makeReq("GET", `/live/${SHARED}/glb`), r, { user: null });
    expect(r.statusCode).toBe(503);
  });
});
