/**
 * Vercel serverless function entry point for the vcad MCP server.
 *
 * Solves the WASM path problem: after esbuild bundles everything into a
 * single file, the wasm-singleton's source-relative path to the .wasm file
 * breaks (it resolves to /kernel-wasm/... outside the function sandbox).
 * We read the .wasm co-located with the bundle and prime it into
 * Engine.init(), so the engine AND every other consumer of the shared
 * wasm-singleton (e.g. the commandRegistry bootstrap behind the registry
 * CRUD tools) use the same initialized module.
 */

import type { IncomingMessage, ServerResponse } from "node:http";
import { StreamableHTTPServerTransport } from "@modelcontextprotocol/sdk/server/streamableHttp.js";
import {
  createServer,
  getBuildInfo,
  sessionStoreInfo,
  flushTelemetry,
  handleLiveRequest,
  handleArtifactRequest,
  isModernMessage,
  handleModernRequest,
  listenClosureResponse,
  flushArtifacts,
  artifactStoreInfo,
} from "@vcad/mcp/server";
import {
  getOAuthConfig,
  handleOAuthRoute,
  verifyAccessToken,
  wwwAuthenticate,
} from "@vcad/mcp/oauth";
import { Engine } from "@vcad/engine";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { readFileSync } from "node:fs";
import {
  isGeoBlocked,
  GEO_BLOCK_BODY,
  GEO_BLOCK_STATUS,
} from "../../shared/geo-block.js";

// The serverless function filesystem is read-only (/var/task), so the
// filesystem-touching tools (export_cad, import_step, export_gerber)
// must return inline base64 payloads instead of writing to disk — a
// writeFileSync there throws EROFS and the bytes would be invisible to
// the caller anyway. The tools read this flag at call time
// (isRemoteDeployment()), so setting it at module scope is sufficient.
if (process.env.VCAD_MCP_REMOTE === undefined) {
  process.env.VCAD_MCP_REMOTE = "1";
}

// Locate WASM file next to this bundle
const __bundleDir = dirname(fileURLToPath(import.meta.url));
const WASM_PATH = join(__bundleDir, "vcad_kernel_wasm_bg.wasm");

// Module-scoped engine — survives warm invocations (Fluid Compute)
let _engine: Engine | undefined;

async function getEngine(): Promise<Engine> {
  if (_engine) return _engine;
  _engine = await Engine.init({ wasmInput: readFileSync(WASM_PATH) });
  return _engine;
}

/** Send a JSON response using raw Node.js API (Build Output API
 *  serves raw ServerResponse, not Vercel's enhanced VercelResponse). */
function sendJson(res: ServerResponse, status: number, data: unknown): void {
  res.writeHead(status, { "Content-Type": "application/json" });
  res.end(JSON.stringify(data));
}

export default async function handler(
  req: IncomingMessage,
  res: ServerResponse,
): Promise<void> {
  // CORS — `Authorization` is allowed so signed-in clients can present a
  // Bearer access token on /mcp once the OAuth flow has run.
  res.setHeader("Access-Control-Allow-Origin", "*");
  res.setHeader("Access-Control-Allow-Methods", "GET, POST, DELETE, OPTIONS");
  res.setHeader(
    "Access-Control-Allow-Headers",
    "Content-Type, Authorization, mcp-session-id, Last-Event-ID, mcp-protocol-version",
  );
  res.setHeader(
    "Access-Control-Expose-Headers",
    "mcp-session-id, mcp-protocol-version",
  );

  // U.S. export-control / sanctions geo-block (see shared/geo-block.ts).
  // Defense-in-depth behind the edge middleware in .vercel/output — every
  // route dests to this one function, so this check alone covers the whole
  // surface even if a request reaches it without traversing the middleware.
  const ipCountry = req.headers["x-vercel-ip-country"];
  const ipRegion = req.headers["x-vercel-ip-country-region"];
  if (
    isGeoBlocked(
      Array.isArray(ipCountry) ? ipCountry[0] : ipCountry,
      Array.isArray(ipRegion) ? ipRegion[0] : ipRegion,
    )
  ) {
    res.writeHead(GEO_BLOCK_STATUS, { "Content-Type": "application/json" });
    res.end(GEO_BLOCK_BODY);
    return;
  }

  const url = new URL(req.url ?? "/", `https://${req.headers.host ?? "mcp.vcad.io"}`);

  // OAuth 2.1 discovery + flow endpoints (/.well-known/oauth-* and
  // /oauth/*). Active only when MCP_OAUTH_SECRET is set; otherwise these
  // paths 404 and the server behaves as an open, no-auth endpoint.
  // Returns true once it has written a response (incl. OPTIONS preflight
  // for those paths), so it must run before the generic OPTIONS handler.
  if (await handleOAuthRoute(req, res, url)) {
    return;
  }

  if (req.method === "OPTIONS") {
    res.writeHead(204);
    res.end();
    return;
  }

  // Health check — public, no auth. Carries full build identity so prod can be
  // verified with a plain `curl https://mcp.vcad.io/health` (no MCP handshake):
  // diff build_sha against `git rev-parse HEAD`, and watch instance_id to see
  // stale serverless instances draining behind a fresh deployment.
  if (req.method === "GET" && url.pathname === "/health") {
    try {
      const engine = await getEngine();
      sendJson(res, 200, {
        status: "ok",
        ...getBuildInfo(),
        // durable:false here means SUPABASE_SERVICE_ROLE_KEY is unset on this
        // prod deploy → open sessions are in-memory only and a redeploy drops
        // them. Verifiable with `curl https://mcp.vcad.io/health`.
        ...sessionStoreInfo(),
        ...artifactStoreInfo(),
        engine: !!engine,
        timestamp: new Date().toISOString(),
      });
    } catch (err) {
      sendJson(res, 500, {
        status: "error",
        error: err instanceof Error ? err.message : String(err),
      });
    }
    return;
  }

  // Identify the caller (null when OAuth is off or no Bearer token was sent).
  // Computed unconditionally so an authenticated session persists to the
  // user's account even while /mcp stays open during the MCP_REQUIRE_AUTH
  // transition. verifyAccessToken is a local HMAC verify — no network.
  const user = verifyAccessToken(req);

  // Artifact channel (/artifacts/*) — large export/fab bundles offloaded out of
  // model context, fetched here by their unguessable id. Public, no auth (the
  // id is the capability), like /live geometry.
  if (await handleArtifactRequest(req, res)) {
    return;
  }

  // Live review window (/live/*) — shared, flag-gated handler (VCAD_LIVE_WINDOW).
  // Returns true once it has written a response; falls through otherwise.
  if (await handleLiveRequest(req, res, { user, getEngine })) {
    return;
  }

  // Optional hard gate on /mcp. Off by default so existing anonymous
  // clients keep working while the OAuth flow rolls out; set
  // MCP_REQUIRE_AUTH=1 (alongside MCP_OAUTH_SECRET) to require a valid
  // signed-in access token once every client has migrated.
  if (process.env.MCP_REQUIRE_AUTH && url.pathname === "/mcp") {
    const cfg = getOAuthConfig();
    if (cfg && !user) {
      res.writeHead(401, {
        "Content-Type": "text/plain",
        "WWW-Authenticate": wwwAuthenticate(cfg),
      });
      res.end("Unauthorized");
      return;
    }
  }

  // MCP endpoint — parse body for POST, then delegate to transport
  try {
    const engine = await getEngine();
    // assumeUiClient: stateless per-request Server — the UI-extension
    // capability from `initialize` never reaches tools/call, so attach the
    // inline `_meta` preview unconditionally (see ServerContext in @vcad/mcp).
    const makeServer = () =>
      createServer(engine, { user, assumeUiClient: true });

    // ── MCP 2026-07-28 (dual-era) ─────────────────────────────────
    // A request carrying modern per-request `_meta` (or a modern-only
    // method) is served by the modern handler; legacy `initialize` traffic
    // falls through to the SDK transport below. Mirrors http.ts, minus SSE:
    // Vercel buffers responses (see enableJsonResponse below), so replies
    // are always single JSON objects here. That also means
    // `subscriptions/listen` cannot hold a stream open on this deployment —
    // answer it with the spec's graceful-closure response (the empty result
    // that tells the client the subscription ended cleanly, deliverable
    // over stdio or the standalone HTTP server instead).
    if (req.method === "POST") {
      const rawBody = await readRequestBody(req);
      let parsed: unknown;
      try {
        parsed = JSON.parse(rawBody);
      } catch {
        sendJson(res, 400, {
          jsonrpc: "2.0",
          id: null,
          error: { code: -32700, message: "Parse error" },
        });
        return;
      }

      if (isModernMessage(parsed, req.headers)) {
        try {
          const method = (parsed as { method?: string } | null)?.method;
          if (method === "subscriptions/listen") {
            sendJson(res, 200, listenClosureResponse(parsed));
            return;
          }
          const { status, body } = await handleModernRequest(parsed, {
            createServer: makeServer,
            headers: req.headers,
          });
          if (body === null) {
            res.writeHead(status);
            res.end();
          } else {
            sendJson(res, status, body);
          }
        } finally {
          await Promise.all([flushTelemetry(), flushArtifacts()]);
        }
        return;
      }

      await handleLegacyMcp(req, res, makeServer, parsed);
      return;
    }

    await handleLegacyMcp(req, res, makeServer);
  } catch (err) {
    console.error("[vcad-mcp] Error:", err);
    if (!res.headersSent) {
      sendJson(res, 500, { error: "Internal server error" });
    }
  }
}

/** Read a POST body as a UTF-8 string (bounded by Vercel's own body limits). */
function readRequestBody(req: IncomingMessage): Promise<string> {
  return new Promise((resolve, reject) => {
    const chunks: Buffer[] = [];
    req.on("data", (c: Buffer) => chunks.push(c));
    req.on("end", () => resolve(Buffer.concat(chunks).toString("utf-8")));
    req.on("error", reject);
  });
}

/** The pre-2026 (`initialize`-handshake) path on the SDK's own transport. */
async function handleLegacyMcp(
  req: IncomingMessage,
  res: ServerResponse,
  makeServer: () => ReturnType<typeof createServer>,
  parsedBody?: unknown,
): Promise<void> {
  try {
    const server = await makeServer();
    const transport = new StreamableHTTPServerTransport({
      sessionIdGenerator: undefined, // stateless
      enableJsonResponse: true, // return JSON instead of SSE — Vercel buffers responses and adds Content-Length which breaks SSE streaming
    });

    await server.connect(transport);

    try {
      // The POST body stream was already consumed by the dual-era router, so
      // hand the parsed body through — the SDK skips its own read when given
      // one. GET/DELETE pass no body.
      if (parsedBody !== undefined) {
        await transport.handleRequest(req, res, parsedBody);
      } else {
        await transport.handleRequest(req, res);
      }
    } finally {
      // Drain PostHog captures AND pending durable artifact writes before
      // the serverless instance can freeze — an in-flight fetch is killed
      // the instant the function returns, and an unflushed artifact would
      // reintroduce the 404-from-another-instance bug.
      await Promise.all([flushTelemetry(), flushArtifacts()]);
      await transport.close();
      await server.close();
    }
  } catch (err) {
    console.error("[vcad-mcp] Error:", err);
    if (!res.headersSent) {
      sendJson(res, 500, { error: "Internal server error" });
    }
  }
}
