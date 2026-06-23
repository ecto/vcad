import { describe, it, expect, beforeEach, afterEach } from "vitest";
import { Readable } from "node:stream";
import type { IncomingMessage, ServerResponse } from "node:http";
import { handleLiveRequest } from "../live-route.js";

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
  body: string;
  ended: boolean;
  writeHead(s: number, h?: unknown): CapRes;
  end(b?: string): void;
}
function makeRes(): CapRes {
  return {
    statusCode: 0,
    body: "",
    ended: false,
    writeHead(s) {
      this.statusCode = s;
      return this;
    },
    end(b) {
      this.body = b ?? "";
      this.ended = true;
    },
  };
}
const res = () => makeRes() as unknown as ServerResponse & CapRes;

// No Supabase env → createSessionEventStore returns the no-op store, so reads
// are [] and appends are accepted but discarded — enough to test routing.
let saved: Record<string, string | undefined>;
beforeEach(() => {
  saved = {
    url: process.env.SUPABASE_URL,
    key: process.env.SUPABASE_SERVICE_ROLE_KEY,
    flag: process.env.VCAD_LIVE_WINDOW,
  };
  delete process.env.SUPABASE_URL;
  delete process.env.SUPABASE_SERVICE_ROLE_KEY;
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
});

describe("handleLiveRequest routing", () => {
  it("returns false for a non-/live path (caller continues routing)", async () => {
    process.env.VCAD_LIVE_WINDOW = "1";
    const r = res();
    const handled = await handleLiveRequest(makeReq("GET", "/mcp"), r, { user: null });
    expect(handled).toBe(false);
    expect(r.ended).toBe(false);
  });

  it("404s every /live path when the flag is off", async () => {
    delete process.env.VCAD_LIVE_WINDOW;
    const r = res();
    const handled = await handleLiveRequest(
      makeReq("GET", "/live/doc_x/events"),
      r,
      { user: null },
    );
    expect(handled).toBe(true);
    expect(r.statusCode).toBe(404);
  });

  it("serves events as JSON when the flag is on", async () => {
    process.env.VCAD_LIVE_WINDOW = "1";
    const r = res();
    await handleLiveRequest(makeReq("GET", "/live/doc_x/events"), r, { user: null });
    expect(r.statusCode).toBe(200);
    const parsed = JSON.parse(r.body) as { session_id: string; events: unknown[] };
    expect(parsed.session_id).toBe("doc_x");
    expect(Array.isArray(parsed.events)).toBe(true);
  });

  it("accepts a valid annotation (200 ok)", async () => {
    process.env.VCAD_LIVE_WINDOW = "1";
    const r = res();
    await handleLiveRequest(
      makeReq("POST", "/live/doc_x/annotate", JSON.stringify({ type: "pin", payload: { text: "hi" } })),
      r,
      { user: null },
    );
    expect(r.statusCode).toBe(200);
    expect(JSON.parse(r.body).ok).toBe(true);
  });

  it("rejects an invalid overlay type (400)", async () => {
    process.env.VCAD_LIVE_WINDOW = "1";
    const r = res();
    await handleLiveRequest(
      makeReq("POST", "/live/doc_x/annotate", JSON.stringify({ type: "mutation", payload: {} })),
      r,
      { user: null },
    );
    expect(r.statusCode).toBe(400);
    expect(JSON.parse(r.body).ok).toBe(false);
  });

  it("400s a missing session id", async () => {
    process.env.VCAD_LIVE_WINDOW = "1";
    const r = res();
    await handleLiveRequest(makeReq("GET", "/live/"), r, { user: null });
    expect(r.statusCode).toBe(400);
  });

  it("404s an unknown /live action", async () => {
    process.env.VCAD_LIVE_WINDOW = "1";
    const r = res();
    await handleLiveRequest(makeReq("GET", "/live/doc_x/bogus"), r, { user: null });
    expect(r.statusCode).toBe(404);
  });
});
