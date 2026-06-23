import { describe, it, expect, afterEach, beforeEach } from "vitest";
import { gzipSync } from "node:zlib";
import { resolveShareToken, setSessionFetch } from "../session-store.js";
import { continueDocument } from "../tools/continue-doc.js";
import { documents } from "../tools/session.js";

/** gzip + base64url, mirroring the web app's inline-doc encoder. */
function encodeInline(obj: unknown): string {
  return gzipSync(Buffer.from(JSON.stringify(obj), "utf-8"))
    .toString("base64")
    .replace(/\+/g, "-")
    .replace(/\//g, "_")
    .replace(/=+$/, "");
}

const UUID = "11111111-2222-3333-4444-555555555555";
const json = (r: { content: Array<{ text: string }> }) =>
  JSON.parse(r.content[0].text);
const jsonResp = (body: unknown) =>
  new Response(JSON.stringify(body), {
    status: 200,
    headers: { "Content-Type": "application/json" },
  });

const origUrl = process.env.SUPABASE_URL;
const origKey = process.env.SUPABASE_SERVICE_ROLE_KEY;

beforeEach(() => {
  process.env.SUPABASE_URL = "https://supa.test";
  process.env.SUPABASE_SERVICE_ROLE_KEY = "svc";
  documents.clear();
});

afterEach(() => {
  setSessionFetch(((...a: Parameters<typeof fetch>) => fetch(...a)) as typeof fetch);
  if (origUrl === undefined) delete process.env.SUPABASE_URL;
  else process.env.SUPABASE_URL = origUrl;
  if (origKey === undefined) delete process.env.SUPABASE_SERVICE_ROLE_KEY;
  else process.env.SUPABASE_SERVICE_ROLE_KEY = origKey;
});

describe("resolveShareToken", () => {
  it("returns null without Supabase env", async () => {
    delete process.env.SUPABASE_URL;
    expect(await resolveShareToken(UUID)).toBeNull();
  });

  it("returns null for a non-uuid token (never hits the network)", async () => {
    let called = false;
    setSessionFetch((() => {
      called = true;
      throw new Error("should not fetch");
    }) as unknown as typeof fetch);
    expect(await resolveShareToken("not-a-uuid")).toBeNull();
    expect(called).toBe(false);
  });

  it("parses the get_shared_document RPC row", async () => {
    setSessionFetch((async (input: unknown, init: RequestInit = {}) => {
      expect(String(input)).toContain("/rest/v1/rpc/get_shared_document");
      expect(JSON.parse(String(init.body))).toEqual({ p_token: UUID });
      return jsonResp([{ name: "Bracket", content: { nodes: {}, roots: ["n1"] } }]);
    }) as unknown as typeof fetch);
    const r = await resolveShareToken(UUID);
    expect(r?.name).toBe("Bracket");
    expect(r?.content).toEqual({ nodes: {}, roots: ["n1"] });
  });

  it("returns null when the token resolves to no rows", async () => {
    setSessionFetch((async () => jsonResp([])) as unknown as typeof fetch);
    expect(await resolveShareToken(UUID)).toBeNull();
  });
});

describe("continueDocument", () => {
  it("errors on a missing token", async () => {
    const r = await continueDocument({});
    expect(r.isError).toBe(true);
    expect(r.content[0].text).toMatch(/requires a `token`/);
  });

  it("errors with a re-share hint on an unknown token", async () => {
    setSessionFetch((async () => jsonResp([])) as unknown as typeof fetch);
    const r = await continueDocument({ token: UUID });
    expect(r.isError).toBe(true);
    expect(r.content[0].text).toMatch(/Continue in Claude/);
  });

  it("opens a session from raw-IR share content (no WASM needed)", async () => {
    setSessionFetch((async () =>
      jsonResp([
        { name: "Bracket", content: { version: 1, nodes: {}, roots: ["n1"] } },
      ])) as unknown as typeof fetch);
    const r = await continueDocument({ token: UUID });
    expect(r.isError).toBeFalsy();
    const out = json(r);
    expect(typeof out.document_id).toBe("string");
    expect(out.parts).toBe(1);
    expect(out.name).toBe("Bracket");
    // The session is registered and resolvable for subsequent tool calls.
    expect(documents.has(out.document_id)).toBe(true);
  });

  it("opens a session from an inline `doc` handoff (anon path, no network)", async () => {
    let fetched = false;
    setSessionFetch((() => {
      fetched = true;
      throw new Error("inline path must not hit the network");
    }) as unknown as typeof fetch);
    const blob = encodeInline({ version: 1, nodes: {}, roots: ["a", "b"] });
    const r = await continueDocument({ doc: blob });
    expect(r.isError).toBeFalsy();
    const out = json(r);
    expect(out.parts).toBe(2);
    expect(documents.has(out.document_id)).toBe(true);
    expect(fetched).toBe(false);
  });

  it("errors with a re-share hint on a corrupt inline `doc`", async () => {
    const r = await continueDocument({ doc: "not-valid-gzip-base64" });
    expect(r.isError).toBe(true);
    expect(r.content[0].text).toMatch(/Continue in Claude/);
  });

  it("is idempotent on the token: re-open reuses the same session (one fetch)", async () => {
    let fetches = 0;
    setSessionFetch((async () => {
      fetches++;
      return jsonResp([
        { name: "Bracket", content: { version: 1, nodes: {}, roots: ["n1"] } },
      ]);
    }) as unknown as typeof fetch);
    const r1 = await continueDocument({ token: UUID });
    const r2 = await continueDocument({ token: UUID });
    const id1 = json(r1).document_id;
    const id2 = json(r2).document_id;
    expect(id1).toBe(`cont_${UUID}`); // deterministic rendezvous key
    expect(id2).toBe(id1);
    expect(fetches).toBe(1); // the re-open is served from the warm cache
  });

  it("rehydrates a durable session before re-fetching the share (cold instance)", async () => {
    let fetched = false;
    setSessionFetch((() => {
      fetched = true;
      throw new Error("cold re-open must prefer the durable session");
    }) as unknown as typeof fetch);
    const store = {
      load: async (id: string) =>
        id === `cont_${UUID}`
          ? ({ version: 1, nodes: {}, roots: ["a", "b", "c"] } as never)
          : null,
      save: async () => {},
      drop: async () => {},
    };
    const r = await continueDocument({ token: UUID }, store);
    expect(r.isError).toBeFalsy();
    const out = json(r);
    expect(out.document_id).toBe(`cont_${UUID}`);
    expect(out.parts).toBe(3); // came from the durable session, not the share
    expect(fetched).toBe(false);
  });
});
