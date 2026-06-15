import { describe, it, expect, beforeEach, afterEach } from "vitest";
import { EventEmitter } from "node:events";
import type { IncomingMessage, ServerResponse } from "node:http";
import { createHash } from "node:crypto";
import {
  handleOAuthRoute,
  verifyAccessToken,
  setSupabaseExchange,
  signToken,
  getOAuthConfig,
} from "../oauth.js";

const SECRET = "test-secret";
const ISSUER = "https://mcp.example.com";

beforeEach(() => {
  process.env.MCP_OAUTH_SECRET = SECRET;
  process.env.MCP_PUBLIC_URL = ISSUER;
  process.env.SUPABASE_URL = "https://supa.example.com";
  process.env.SUPABASE_ANON_KEY = "anon";
  setSupabaseExchange(async () => ({ sub: "user-123", email: "a@b.c" }));
});

afterEach(() => {
  delete process.env.MCP_OAUTH_SECRET;
  delete process.env.MCP_PUBLIC_URL;
  delete process.env.SUPABASE_URL;
  delete process.env.SUPABASE_ANON_KEY;
});

// ── Minimal req/res fakes ─────────────────────────────────────────

function fakeReq(opts: {
  method?: string;
  body?: string;
  headers?: Record<string, string>;
}): IncomingMessage {
  const req = new EventEmitter() as unknown as IncomingMessage & EventEmitter;
  (req as { method?: string }).method = opts.method ?? "GET";
  (req as { headers: Record<string, string> }).headers = opts.headers ?? {};
  if (opts.body !== undefined) {
    setImmediate(() => {
      req.emit("data", Buffer.from(opts.body!));
      req.emit("end");
    });
  } else {
    setImmediate(() => req.emit("end"));
  }
  (req as { destroy: () => void }).destroy = () => {};
  return req;
}

interface FakeRes {
  status: number;
  headers: Record<string, string>;
  body: string;
  res: ServerResponse;
}

function fakeRes(): FakeRes {
  const out: FakeRes = { status: 0, headers: {}, body: "", res: null as unknown as ServerResponse };
  out.res = {
    setHeader(k: string, v: string) {
      out.headers[k.toLowerCase()] = v;
    },
    writeHead(status: number, headers?: Record<string, string>) {
      out.status = status;
      for (const [k, v] of Object.entries(headers ?? {})) {
        out.headers[k.toLowerCase()] = v;
      }
      return this;
    },
    end(chunk?: string | Buffer) {
      if (chunk) out.body += chunk.toString();
    },
  } as unknown as ServerResponse;
  return out;
}

async function route(
  path: string,
  opts: { method?: string; body?: string; headers?: Record<string, string> } = {},
): Promise<FakeRes> {
  const out = fakeRes();
  const handled = await handleOAuthRoute(
    fakeReq(opts),
    out.res,
    new URL(path, ISSUER),
  );
  expect(handled).toBe(true);
  return out;
}

function s256(verifier: string): string {
  return createHash("sha256").update(verifier).digest("base64url");
}

// ── Full flow helper ──────────────────────────────────────────────

async function register(redirectUri = "https://client.example.com/cb"): Promise<string> {
  const r = await route("/oauth/register", {
    method: "POST",
    body: JSON.stringify({ redirect_uris: [redirectUri], client_name: "Test" }),
  });
  expect(r.status).toBe(201);
  return JSON.parse(r.body).client_id as string;
}

/** register → authorize → start → callback; returns the auth code. */
async function runFlowToCode(verifier: string): Promise<{ code: string; clientId: string }> {
  const clientId = await register();
  const authz = await route(
    `/oauth/authorize?client_id=${encodeURIComponent(clientId)}&redirect_uri=${encodeURIComponent(
      "https://client.example.com/cb",
    )}&response_type=code&state=xyz&code_challenge=${s256(verifier)}&code_challenge_method=S256`,
  );
  expect(authz.status).toBe(200);
  const m = /href="([^"]*\/oauth\/start[^"]*)"/.exec(authz.body);
  expect(m).toBeTruthy();
  const startUrl = m![1].replace(/&amp;/g, "&");

  const start = await route(startUrl);
  expect(start.status).toBe(302);
  expect(start.headers["location"]).toContain("supa.example.com/auth/v1/authorize");
  const cookie = start.headers["set-cookie"].split(";")[0];

  const cb = await route("/oauth/callback?code=supacode", {
    headers: { cookie },
  });
  expect(cb.status).toBe(302);
  const target = new URL(cb.headers["location"]);
  expect(target.origin + target.pathname).toBe("https://client.example.com/cb");
  expect(target.searchParams.get("state")).toBe("xyz");
  return { code: target.searchParams.get("code")!, clientId };
}

// ── Tests ─────────────────────────────────────────────────────────

describe("discovery metadata", () => {
  it("serves RFC 8414 authorization-server metadata (with path suffix)", async () => {
    for (const p of [
      "/.well-known/oauth-authorization-server",
      "/.well-known/oauth-authorization-server/mcp",
    ]) {
      const r = await route(p);
      expect(r.status).toBe(200);
      const meta = JSON.parse(r.body);
      expect(meta.issuer).toBe(ISSUER);
      expect(meta.registration_endpoint).toBe(`${ISSUER}/oauth/register`);
      expect(meta.code_challenge_methods_supported).toEqual(["S256"]);
    }
  });

  it("serves RFC 9728 protected-resource metadata", async () => {
    const r = await route("/.well-known/oauth-protected-resource/mcp");
    const meta = JSON.parse(r.body);
    expect(meta.resource).toBe(`${ISSUER}/mcp`);
    expect(meta.authorization_servers).toEqual([ISSUER]);
  });

  it("404s discovery when OAuth is disabled", async () => {
    delete process.env.MCP_OAUTH_SECRET;
    const r = await route("/.well-known/oauth-authorization-server");
    expect(r.status).toBe(404);
  });

  it("does not claim unrelated routes", async () => {
    const out = fakeRes();
    const handled = await handleOAuthRoute(fakeReq({}), out.res, new URL("/mcp", ISSUER));
    expect(handled).toBe(false);
  });
});

describe("dynamic client registration", () => {
  it("registers a client and round-trips redirect_uris", async () => {
    const clientId = await register();
    expect(clientId.split(".").length).toBe(3);
  });

  it("rejects non-https redirect uris", async () => {
    const r = await route("/oauth/register", {
      method: "POST",
      body: JSON.stringify({ redirect_uris: ["http://evil.example.com/cb"] }),
    });
    expect(r.status).toBe(400);
  });

  it("allows loopback http redirect uris", async () => {
    const r = await route("/oauth/register", {
      method: "POST",
      body: JSON.stringify({ redirect_uris: ["http://localhost:3000/cb"] }),
    });
    expect(r.status).toBe(201);
  });
});

describe("authorize", () => {
  it("rejects unregistered redirect_uri without redirecting", async () => {
    const clientId = await register("https://client.example.com/cb");
    const r = await route(
      `/oauth/authorize?client_id=${encodeURIComponent(clientId)}&redirect_uri=${encodeURIComponent(
        "https://attacker.example.com/cb",
      )}&response_type=code&code_challenge=x&code_challenge_method=S256`,
    );
    expect(r.status).toBe(400);
  });

  it("requires PKCE S256", async () => {
    const clientId = await register();
    const r = await route(
      `/oauth/authorize?client_id=${encodeURIComponent(clientId)}&redirect_uri=${encodeURIComponent(
        "https://client.example.com/cb",
      )}&response_type=code`,
    );
    expect(r.status).toBe(302);
    expect(r.headers["location"]).toContain("error=invalid_request");
  });
});

describe("token exchange", () => {
  it("full flow: code + verifier → access/refresh tokens; token authenticates /mcp", async () => {
    const verifier = "a".repeat(43);
    const { code } = await runFlowToCode(verifier);

    const r = await route("/oauth/token", {
      method: "POST",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      body: new URLSearchParams({
        grant_type: "authorization_code",
        code,
        code_verifier: verifier,
      }).toString(),
    });
    expect(r.status).toBe(200);
    const tokens = JSON.parse(r.body);
    expect(tokens.token_type).toBe("Bearer");
    expect(tokens.refresh_token).toBeTruthy();

    const user = verifyAccessToken(
      fakeReq({ headers: { authorization: `Bearer ${tokens.access_token}` } }),
    );
    expect(user).toEqual({ sub: "user-123", email: "a@b.c" });
  });

  it("rejects a wrong PKCE verifier", async () => {
    const { code } = await runFlowToCode("b".repeat(43));
    const r = await route("/oauth/token", {
      method: "POST",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      body: new URLSearchParams({
        grant_type: "authorization_code",
        code,
        code_verifier: "wrong".repeat(10),
      }).toString(),
    });
    expect(r.status).toBe(400);
    expect(JSON.parse(r.body).error).toBe("invalid_grant");
  });

  it("rejects code replay", async () => {
    const verifier = "c".repeat(43);
    const { code } = await runFlowToCode(verifier);
    const body = new URLSearchParams({
      grant_type: "authorization_code",
      code,
      code_verifier: verifier,
    }).toString();
    const first = await route("/oauth/token", {
      method: "POST",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      body,
    });
    expect(first.status).toBe(200);
    const second = await route("/oauth/token", {
      method: "POST",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      body,
    });
    expect(second.status).toBe(400);
  });

  it("refresh_token grant issues new tokens", async () => {
    const verifier = "d".repeat(43);
    const { code } = await runFlowToCode(verifier);
    const r1 = await route("/oauth/token", {
      method: "POST",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      body: new URLSearchParams({
        grant_type: "authorization_code",
        code,
        code_verifier: verifier,
      }).toString(),
    });
    const { refresh_token } = JSON.parse(r1.body);
    const r2 = await route("/oauth/token", {
      method: "POST",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      body: new URLSearchParams({
        grant_type: "refresh_token",
        refresh_token,
      }).toString(),
    });
    expect(r2.status).toBe(200);
    expect(JSON.parse(r2.body).access_token).toBeTruthy();
  });
});

describe("verifyAccessToken", () => {
  it("rejects garbage, expired, and wrong-type tokens", () => {
    const cfg = getOAuthConfig()!;
    expect(
      verifyAccessToken(fakeReq({ headers: { authorization: "Bearer junk" } })),
    ).toBeNull();
    const expired = signToken({ typ: "access", sub: "u" }, cfg.secret, -10);
    expect(
      verifyAccessToken(fakeReq({ headers: { authorization: `Bearer ${expired}` } })),
    ).toBeNull();
    const refresh = signToken({ typ: "refresh", sub: "u" }, cfg.secret, 60);
    expect(
      verifyAccessToken(fakeReq({ headers: { authorization: `Bearer ${refresh}` } })),
    ).toBeNull();
  });
});
