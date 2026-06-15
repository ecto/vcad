/**
 * OAuth 2.1 authorization server for the hosted MCP endpoint.
 *
 * Implements the discovery + flow surface that MCP clients (claude.ai
 * connectors, ChatGPT apps, Codex `mcp login`, MCP Inspector) expect:
 *
 *  - RFC 8414  authorization-server metadata (/.well-known/oauth-authorization-server)
 *  - RFC 9728  protected-resource metadata  (/.well-known/oauth-protected-resource)
 *  - RFC 7591  dynamic client registration  (POST /oauth/register)
 *  - Authorization-code flow with mandatory PKCE (S256) — public clients only
 *  - Refresh tokens
 *
 * Identity is delegated to Supabase Auth (the same Google/GitHub OAuth the
 * vcad app uses): /oauth/authorize shows a provider picker, /oauth/start
 * runs Supabase's PKCE flow, /oauth/callback exchanges the Supabase code
 * for the user and mints our own tokens.
 *
 * Everything is **stateless**: client registrations, authorization codes,
 * and access/refresh tokens are HMAC-signed (HS256) blobs keyed by
 * MCP_OAUTH_SECRET, so a server restart invalidates nothing and no
 * database is needed. The one piece of in-memory state is a replay guard
 * for authorization codes (single machine — fine, sessions are
 * process-local anyway).
 *
 * Environment:
 *   MCP_OAUTH_SECRET     enables OAuth; HMAC key for all signed blobs
 *   MCP_PUBLIC_URL       issuer, e.g. https://mcp.vcad.io (required with OAuth)
 *   SUPABASE_URL         e.g. https://yteuhwciuxcbjwmabawj.supabase.co
 *   SUPABASE_ANON_KEY    Supabase anon (publishable) key
 *   MCP_OAUTH_PROVIDERS  comma list shown on the picker (default google,github)
 *
 * Supabase dashboard prerequisite: add `${MCP_PUBLIC_URL}/oauth/callback`
 * to Auth → URL Configuration → Redirect URLs.
 */

import { createHmac, createHash, randomBytes, timingSafeEqual } from "node:crypto";
import type { IncomingMessage, ServerResponse } from "node:http";

// ── Config ───────────────────────────────────────────────────────

export interface OAuthConfig {
  secret: string;
  issuer: string;
  supabaseUrl: string;
  supabaseAnonKey: string;
  providers: string[];
}

/** Read config from env at call time (tests mutate env). Returns null
 *  when OAuth is disabled (no secret). */
export function getOAuthConfig(): OAuthConfig | null {
  const secret = process.env.MCP_OAUTH_SECRET || "";
  if (!secret) return null;
  const issuer = (process.env.MCP_PUBLIC_URL || "").replace(/\/+$/, "");
  return {
    secret,
    issuer,
    supabaseUrl: (process.env.SUPABASE_URL || "").replace(/\/+$/, ""),
    supabaseAnonKey: process.env.SUPABASE_ANON_KEY || "",
    providers: (process.env.MCP_OAUTH_PROVIDERS || "google,github")
      .split(",")
      .map((s) => s.trim())
      .filter(Boolean),
  };
}

// ── Signed-blob (compact JWT, HS256) helpers ─────────────────────

function b64url(buf: Buffer): string {
  return buf.toString("base64url");
}

function hmac(data: string, secret: string): Buffer {
  return createHmac("sha256", secret).update(data).digest();
}

/** Sign a payload as a compact HS256 JWT. `ttlSec` sets `exp`. */
export function signToken(
  payload: Record<string, unknown>,
  secret: string,
  ttlSec: number,
): string {
  const header = b64url(Buffer.from(JSON.stringify({ alg: "HS256", typ: "JWT" })));
  const now = Math.floor(Date.now() / 1000);
  const body = b64url(
    Buffer.from(JSON.stringify({ ...payload, iat: now, exp: now + ttlSec })),
  );
  const sig = b64url(hmac(`${header}.${body}`, secret));
  return `${header}.${body}.${sig}`;
}

/** Verify signature + expiry; returns the payload or null. */
export function verifyToken(
  token: string,
  secret: string,
): Record<string, unknown> | null {
  const parts = token.split(".");
  if (parts.length !== 3) return null;
  const [header, body, sig] = parts;
  const expected = hmac(`${header}.${body}`, secret);
  let given: Buffer;
  try {
    given = Buffer.from(sig, "base64url");
  } catch {
    return null;
  }
  if (given.length !== expected.length || !timingSafeEqual(given, expected)) {
    return null;
  }
  try {
    const payload = JSON.parse(Buffer.from(body, "base64url").toString("utf8"));
    if (typeof payload.exp !== "number" || payload.exp < Date.now() / 1000) {
      return null;
    }
    return payload;
  } catch {
    return null;
  }
}

/** S256 PKCE: base64url(sha256(verifier)). */
function s256(verifier: string): string {
  return b64url(createHash("sha256").update(verifier).digest());
}

// ── Authorization-code replay guard ──────────────────────────────
// Codes are stateless JWTs; this in-memory jti set prevents replaying a
// code within its (short) lifetime. Single-process by design.

const usedCodeJtis = new Map<string, number>(); // jti → exp (ms)

function codeReplayed(jti: string, expSec: number): boolean {
  const now = Date.now();
  for (const [k, exp] of usedCodeJtis) {
    if (exp < now) usedCodeJtis.delete(k);
  }
  if (usedCodeJtis.has(jti)) return true;
  usedCodeJtis.set(jti, expSec * 1000);
  return false;
}

// ── Token lifetimes ──────────────────────────────────────────────

const CODE_TTL_SEC = 120;
const ACCESS_TTL_SEC = 60 * 60; // 1 h
const REFRESH_TTL_SEC = 30 * 24 * 60 * 60; // 30 d
const AUTH_REQUEST_TTL_SEC = 600; // provider picker → callback window

// ── HTTP plumbing ────────────────────────────────────────────────

function sendJson(res: ServerResponse, status: number, body: unknown): void {
  res.writeHead(status, {
    "Content-Type": "application/json",
    "Cache-Control": "no-store",
  });
  res.end(JSON.stringify(body));
}

function sendOAuthError(
  res: ServerResponse,
  status: number,
  error: string,
  description: string,
): void {
  sendJson(res, status, { error, error_description: description });
}

function sendHtml(res: ServerResponse, status: number, html: string): void {
  res.writeHead(status, { "Content-Type": "text/html; charset=utf-8" });
  res.end(html);
}

function redirect(res: ServerResponse, location: string, cookie?: string): void {
  const headers: Record<string, string> = { Location: location };
  if (cookie) headers["Set-Cookie"] = cookie;
  res.writeHead(302, headers);
  res.end();
}

/** Permissive CORS for the public OAuth endpoints (browser-based MCP
 *  clients hit /register and /token cross-origin). */
function oauthCors(res: ServerResponse): void {
  res.setHeader("Access-Control-Allow-Origin", "*");
  res.setHeader("Access-Control-Allow-Methods", "GET, POST, OPTIONS");
  res.setHeader("Access-Control-Allow-Headers", "Content-Type, Authorization, mcp-protocol-version");
}

async function readBody(req: IncomingMessage, maxBytes = 64 * 1024): Promise<string> {
  return new Promise((resolve, reject) => {
    const chunks: Buffer[] = [];
    let total = 0;
    req.on("data", (chunk: Buffer) => {
      total += chunk.length;
      if (total > maxBytes) {
        reject(new Error("body too large"));
        req.destroy();
        return;
      }
      chunks.push(chunk);
    });
    req.on("end", () => resolve(Buffer.concat(chunks).toString("utf8")));
    req.on("error", reject);
  });
}

/** Parse a token-endpoint body: form-encoded per spec, JSON tolerated. */
function parseParams(body: string, contentType: string): Record<string, string> {
  if (contentType.includes("application/json")) {
    try {
      const parsed = JSON.parse(body);
      const out: Record<string, string> = {};
      for (const [k, v] of Object.entries(parsed)) {
        if (typeof v === "string") out[k] = v;
      }
      return out;
    } catch {
      return {};
    }
  }
  return Object.fromEntries(new URLSearchParams(body));
}

function getCookie(req: IncomingMessage, name: string): string | null {
  const header = req.headers.cookie;
  if (!header) return null;
  for (const part of header.split(";")) {
    const [k, ...rest] = part.trim().split("=");
    if (k === name) return rest.join("=");
  }
  return null;
}

// ── Metadata documents ───────────────────────────────────────────

function authServerMetadata(cfg: OAuthConfig): Record<string, unknown> {
  return {
    issuer: cfg.issuer,
    authorization_endpoint: `${cfg.issuer}/oauth/authorize`,
    token_endpoint: `${cfg.issuer}/oauth/token`,
    registration_endpoint: `${cfg.issuer}/oauth/register`,
    response_types_supported: ["code"],
    grant_types_supported: ["authorization_code", "refresh_token"],
    code_challenge_methods_supported: ["S256"],
    token_endpoint_auth_methods_supported: ["none"],
    scopes_supported: ["vcad"],
  };
}

function protectedResourceMetadata(cfg: OAuthConfig): Record<string, unknown> {
  return {
    resource: `${cfg.issuer}/mcp`,
    authorization_servers: [cfg.issuer],
    bearer_methods_supported: ["header"],
    scopes_supported: ["vcad"],
  };
}

// ── Client registration (RFC 7591, stateless) ────────────────────

function validRedirectUri(uri: string): boolean {
  try {
    const u = new URL(uri);
    if (u.protocol === "https:") return true;
    // Loopback redirect URIs are allowed for native/dev clients (RFC 8252).
    return (
      u.protocol === "http:" &&
      (u.hostname === "localhost" || u.hostname === "127.0.0.1")
    );
  } catch {
    return false;
  }
}

function handleRegister(
  cfg: OAuthConfig,
  res: ServerResponse,
  body: string,
): void {
  let reg: { redirect_uris?: unknown; client_name?: unknown };
  try {
    reg = JSON.parse(body);
  } catch {
    sendOAuthError(res, 400, "invalid_client_metadata", "body must be JSON");
    return;
  }
  const uris = Array.isArray(reg.redirect_uris)
    ? reg.redirect_uris.filter((u): u is string => typeof u === "string")
    : [];
  if (uris.length === 0 || !uris.every(validRedirectUri)) {
    sendOAuthError(
      res,
      400,
      "invalid_redirect_uri",
      "redirect_uris must be a non-empty array of https (or loopback http) URLs",
    );
    return;
  }
  const name = typeof reg.client_name === "string" ? reg.client_name.slice(0, 120) : "";
  // The client_id IS the registration: a signed blob carrying the
  // redirect URIs. No registry to persist, nothing lost on restart.
  const clientId = signToken(
    { typ: "client", ru: uris, name },
    cfg.secret,
    10 * 365 * 24 * 60 * 60,
  );
  sendJson(res, 201, {
    client_id: clientId,
    client_name: name || undefined,
    redirect_uris: uris,
    token_endpoint_auth_method: "none",
    grant_types: ["authorization_code", "refresh_token"],
    response_types: ["code"],
  });
}

interface ClientInfo {
  redirectUris: string[];
  name: string;
}

function verifyClientId(cfg: OAuthConfig, clientId: string): ClientInfo | null {
  const payload = verifyToken(clientId, cfg.secret);
  if (!payload || payload.typ !== "client" || !Array.isArray(payload.ru)) {
    return null;
  }
  return {
    redirectUris: payload.ru as string[],
    name: typeof payload.name === "string" ? payload.name : "",
  };
}

// ── Authorize: provider picker ───────────────────────────────────

function escapeHtml(s: string): string {
  return s.replace(/[&<>"']/g, (c) =>
    ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!,
  );
}

const PROVIDER_LABELS: Record<string, string> = {
  google: "Continue with Google",
  github: "Continue with GitHub",
};

function handleAuthorize(
  cfg: OAuthConfig,
  url: URL,
  res: ServerResponse,
): void {
  const clientId = url.searchParams.get("client_id") || "";
  const redirectUri = url.searchParams.get("redirect_uri") || "";
  const state = url.searchParams.get("state") || "";
  const responseType = url.searchParams.get("response_type") || "";
  const codeChallenge = url.searchParams.get("code_challenge") || "";
  const challengeMethod = url.searchParams.get("code_challenge_method") || "";

  const client = verifyClientId(cfg, clientId);
  if (!client) {
    sendOAuthError(res, 400, "invalid_client", "unknown client_id — register first");
    return;
  }
  if (!client.redirectUris.includes(redirectUri)) {
    // Never redirect to an unregistered URI.
    sendOAuthError(res, 400, "invalid_redirect_uri", "redirect_uri not registered");
    return;
  }
  const fail = (error: string, description: string) => {
    const u = new URL(redirectUri);
    u.searchParams.set("error", error);
    u.searchParams.set("error_description", description);
    if (state) u.searchParams.set("state", state);
    redirect(res, u.toString());
  };
  if (responseType !== "code") {
    fail("unsupported_response_type", "only response_type=code is supported");
    return;
  }
  if (!codeChallenge || challengeMethod !== "S256") {
    // OAuth 2.1: PKCE with S256 is mandatory for public clients.
    fail("invalid_request", "PKCE with code_challenge_method=S256 is required");
    return;
  }

  // Carry the request through the provider hop as a signed blob.
  const request = signToken(
    { typ: "authreq", cid: sha256Hex(clientId), ru: redirectUri, st: state, cch: codeChallenge },
    cfg.secret,
    AUTH_REQUEST_TTL_SEC,
  );

  const buttons = cfg.providers
    .map((p) => {
      const label = PROVIDER_LABELS[p] ?? `Continue with ${p}`;
      const href = `/oauth/start?provider=${encodeURIComponent(p)}&request=${encodeURIComponent(request)}`;
      return `<a class="btn" href="${escapeHtml(href)}">${escapeHtml(label)}</a>`;
    })
    .join("\n");
  const appName = client.name ? escapeHtml(client.name) : "An MCP client";

  sendHtml(
    res,
    200,
    `<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Sign in — vcad</title>
<style>
body{font-family:-apple-system,system-ui,sans-serif;background:#101014;color:#e8e8ec;display:flex;align-items:center;justify-content:center;min-height:100vh;margin:0}
.card{background:#18181f;border:1px solid #2a2a33;border-radius:12px;padding:32px;max-width:380px;text-align:center}
h1{font-size:18px;margin:0 0 6px} p{color:#9a9aa5;font-size:14px;margin:0 0 24px}
.btn{display:block;background:#26262f;border:1px solid #3a3a45;border-radius:8px;color:#e8e8ec;text-decoration:none;padding:12px;margin:10px 0;font-size:14px}
.btn:hover{background:#30303b}
.logo{font-weight:700;letter-spacing:.5px;color:#f92672}
</style></head><body><div class="card">
<h1><span class="logo">vcad</span> — sign in</h1>
<p>${appName} is requesting access to your vcad MCP sessions.</p>
${buttons}
</div></body></html>`,
  );
}

function sha256Hex(s: string): string {
  return createHash("sha256").update(s).digest("hex");
}

// ── Start: hop to Supabase with its own PKCE ─────────────────────

function handleStart(cfg: OAuthConfig, url: URL, res: ServerResponse): void {
  const provider = url.searchParams.get("provider") || "";
  const request = url.searchParams.get("request") || "";
  const reqPayload = verifyToken(request, cfg.secret);
  if (!reqPayload || reqPayload.typ !== "authreq") {
    sendOAuthError(res, 400, "invalid_request", "authorization request expired — restart the flow");
    return;
  }
  if (!cfg.providers.includes(provider)) {
    sendOAuthError(res, 400, "invalid_request", `unknown provider "${provider}"`);
    return;
  }
  if (!cfg.supabaseUrl || !cfg.supabaseAnonKey) {
    sendOAuthError(res, 500, "server_error", "SUPABASE_URL / SUPABASE_ANON_KEY not configured");
    return;
  }

  // Our own PKCE verifier for the Supabase leg, carried in a signed
  // cookie so /oauth/callback can complete the exchange.
  const verifier = randomBytes(32).toString("base64url");
  const cookiePayload = signToken(
    { typ: "cb", req: request, ver: verifier },
    cfg.secret,
    AUTH_REQUEST_TTL_SEC,
  );
  const cookie = `vcad_oauth=${cookiePayload}; Path=/oauth; Max-Age=${AUTH_REQUEST_TTL_SEC}; HttpOnly; Secure; SameSite=Lax`;

  const supa = new URL(`${cfg.supabaseUrl}/auth/v1/authorize`);
  supa.searchParams.set("provider", provider);
  supa.searchParams.set("redirect_to", `${cfg.issuer}/oauth/callback`);
  supa.searchParams.set("code_challenge", s256(verifier));
  supa.searchParams.set("code_challenge_method", "s256");
  redirect(res, supa.toString(), cookie);
}

// ── Callback: Supabase code → our authorization code ─────────────

/** Injectable for tests. */
export let supabaseExchange = async (
  cfg: OAuthConfig,
  code: string,
  verifier: string,
): Promise<{ sub: string; email: string } | null> => {
  const resp = await fetch(`${cfg.supabaseUrl}/auth/v1/token?grant_type=pkce`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      apikey: cfg.supabaseAnonKey,
    },
    body: JSON.stringify({ auth_code: code, code_verifier: verifier }),
  });
  if (!resp.ok) {
    console.error("[oauth] supabase exchange failed:", resp.status, await resp.text());
    return null;
  }
  const data = (await resp.json()) as {
    user?: { id?: string; email?: string };
  };
  if (!data.user?.id) return null;
  return { sub: data.user.id, email: data.user.email ?? "" };
};

/** Test hook. */
export function setSupabaseExchange(fn: typeof supabaseExchange): void {
  supabaseExchange = fn;
}

async function handleCallback(
  cfg: OAuthConfig,
  url: URL,
  req: IncomingMessage,
  res: ServerResponse,
): Promise<void> {
  const supaCode = url.searchParams.get("code") || "";
  const cookieRaw = getCookie(req, "vcad_oauth") || "";
  const cookie = verifyToken(cookieRaw, cfg.secret);
  if (!cookie || cookie.typ !== "cb" || typeof cookie.req !== "string" || typeof cookie.ver !== "string") {
    sendHtml(res, 400, "<p>Sign-in session expired — close this tab and retry from your MCP client.</p>");
    return;
  }
  const reqPayload = verifyToken(cookie.req, cfg.secret);
  if (!reqPayload || reqPayload.typ !== "authreq") {
    sendHtml(res, 400, "<p>Authorization request expired — restart the flow.</p>");
    return;
  }
  if (!supaCode) {
    const desc = url.searchParams.get("error_description") || "provider sign-in failed";
    sendHtml(res, 400, `<p>${escapeHtml(desc)}</p>`);
    return;
  }

  const user = await supabaseExchange(cfg, supaCode, cookie.ver);
  if (!user) {
    sendHtml(res, 502, "<p>Could not verify your identity with the sign-in provider.</p>");
    return;
  }

  const code = signToken(
    {
      typ: "code",
      jti: randomBytes(8).toString("base64url"),
      sub: user.sub,
      email: user.email,
      cid: reqPayload.cid,
      ru: reqPayload.ru,
      cch: reqPayload.cch,
    },
    cfg.secret,
    CODE_TTL_SEC,
  );

  const target = new URL(String(reqPayload.ru));
  target.searchParams.set("code", code);
  if (reqPayload.st) target.searchParams.set("state", String(reqPayload.st));
  // Expire the flow cookie.
  redirect(res, target.toString(), "vcad_oauth=; Path=/oauth; Max-Age=0; HttpOnly; Secure; SameSite=Lax");
}

// ── Token endpoint ───────────────────────────────────────────────

function issueTokens(
  cfg: OAuthConfig,
  sub: string,
  email: string,
): Record<string, unknown> {
  return {
    access_token: signToken({ typ: "access", sub, email, scope: "vcad" }, cfg.secret, ACCESS_TTL_SEC),
    token_type: "Bearer",
    expires_in: ACCESS_TTL_SEC,
    refresh_token: signToken({ typ: "refresh", sub, email }, cfg.secret, REFRESH_TTL_SEC),
    scope: "vcad",
  };
}

function handleToken(
  cfg: OAuthConfig,
  req: IncomingMessage,
  res: ServerResponse,
  body: string,
): void {
  const params = parseParams(body, String(req.headers["content-type"] || ""));
  const grantType = params.grant_type;

  if (grantType === "authorization_code") {
    const code = verifyToken(params.code || "", cfg.secret);
    if (!code || code.typ !== "code") {
      sendOAuthError(res, 400, "invalid_grant", "authorization code is invalid or expired");
      return;
    }
    if (codeReplayed(String(code.jti), Number(code.exp))) {
      sendOAuthError(res, 400, "invalid_grant", "authorization code already used");
      return;
    }
    // PKCE binding — the whole point of the public-client flow.
    if (!params.code_verifier || s256(params.code_verifier) !== code.cch) {
      sendOAuthError(res, 400, "invalid_grant", "PKCE code_verifier mismatch");
      return;
    }
    // Bind to the requesting client + redirect URI when supplied.
    if (params.client_id && sha256Hex(params.client_id) !== code.cid) {
      sendOAuthError(res, 400, "invalid_grant", "client_id mismatch");
      return;
    }
    if (params.redirect_uri && params.redirect_uri !== code.ru) {
      sendOAuthError(res, 400, "invalid_grant", "redirect_uri mismatch");
      return;
    }
    sendJson(res, 200, issueTokens(cfg, String(code.sub), String(code.email ?? "")));
    return;
  }

  if (grantType === "refresh_token") {
    const refresh = verifyToken(params.refresh_token || "", cfg.secret);
    if (!refresh || refresh.typ !== "refresh") {
      sendOAuthError(res, 400, "invalid_grant", "refresh token is invalid or expired");
      return;
    }
    sendJson(res, 200, issueTokens(cfg, String(refresh.sub), String(refresh.email ?? "")));
    return;
  }

  sendOAuthError(res, 400, "unsupported_grant_type", "use authorization_code or refresh_token");
}

// ── Bearer validation for /mcp ───────────────────────────────────

export interface AuthUser {
  sub: string;
  email: string;
}

/** Validate an incoming Bearer access token. Null when missing/invalid. */
export function verifyAccessToken(req: IncomingMessage): AuthUser | null {
  const cfg = getOAuthConfig();
  if (!cfg) return null;
  const header = req.headers.authorization;
  if (typeof header !== "string") return null;
  const match = /^Bearer\s+(.+)$/i.exec(header);
  if (!match) return null;
  const payload = verifyToken(match[1], cfg.secret);
  if (!payload || payload.typ !== "access" || typeof payload.sub !== "string") {
    return null;
  }
  return { sub: payload.sub, email: String(payload.email ?? "") };
}

/** WWW-Authenticate header value pointing clients at discovery. */
export function wwwAuthenticate(cfg: OAuthConfig): string {
  return `Bearer realm="vcad-mcp", resource_metadata="${cfg.issuer}/.well-known/oauth-protected-resource"`;
}

// ── Router ───────────────────────────────────────────────────────

/**
 * Handle OAuth + discovery routes. Returns true when the request was
 * handled (response sent), false to fall through to other routes.
 */
export async function handleOAuthRoute(
  req: IncomingMessage,
  res: ServerResponse,
  url: URL,
): Promise<boolean> {
  const path = url.pathname;
  const isWellKnown =
    path.startsWith("/.well-known/oauth-authorization-server") ||
    path.startsWith("/.well-known/oauth-protected-resource");
  const isOAuth = path.startsWith("/oauth/");
  if (!isWellKnown && !isOAuth) return false;

  const cfg = getOAuthConfig();
  oauthCors(res);
  if (req.method === "OPTIONS") {
    res.writeHead(204);
    res.end();
    return true;
  }
  if (!cfg) {
    // OAuth disabled: 404 on discovery so clients treat the server as
    // no-auth instead of attempting a flow that can't complete.
    res.writeHead(404, { "Content-Type": "text/plain" });
    res.end("OAuth is not enabled on this server");
    return true;
  }
  if (!cfg.issuer) {
    sendOAuthError(res, 500, "server_error", "MCP_PUBLIC_URL is not configured");
    return true;
  }

  // RFC 8414 allows a path suffix on the well-known URLs (e.g.
  // /.well-known/oauth-authorization-server/mcp) — serve any suffix.
  if (path.startsWith("/.well-known/oauth-authorization-server")) {
    sendJson(res, 200, authServerMetadata(cfg));
    return true;
  }
  if (path.startsWith("/.well-known/oauth-protected-resource")) {
    sendJson(res, 200, protectedResourceMetadata(cfg));
    return true;
  }

  switch (path) {
    case "/oauth/register": {
      if (req.method !== "POST") break;
      handleRegister(cfg, res, await readBody(req));
      return true;
    }
    case "/oauth/authorize": {
      if (req.method !== "GET") break;
      handleAuthorize(cfg, url, res);
      return true;
    }
    case "/oauth/start": {
      if (req.method !== "GET") break;
      handleStart(cfg, url, res);
      return true;
    }
    case "/oauth/callback": {
      if (req.method !== "GET") break;
      await handleCallback(cfg, url, req, res);
      return true;
    }
    case "/oauth/token": {
      if (req.method !== "POST") break;
      handleToken(cfg, req, res, await readBody(req));
      return true;
    }
  }
  res.writeHead(405, { "Content-Type": "text/plain" });
  res.end("Method Not Allowed");
  return true;
}
