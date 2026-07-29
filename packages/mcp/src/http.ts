/**
 * HTTP entry point for the vcad MCP server.
 *
 * Runs as a standalone Node.js HTTP server with StreamableHTTPServerTransport.
 * Designed for deployment to Vercel, Fly.io, or any Node.js host.
 *
 * Stateless mode: each request creates a fresh transport + server.
 * No session persistence — every tool call is independent.
 *
 * Security posture for public deployments is gated on environment variables:
 *   MCP_API_KEY               required Bearer token for /mcp; unset = anon (dev only)
 *   MCP_ALLOWED_ORIGINS       comma-separated CORS allowlist; unset = no CORS headers
 *   MCP_RATE_LIMIT_PER_MINUTE per-IP cap for /mcp (default 60)
 *   MCP_MAX_BODY_BYTES        cap on POST body size (default 10 MiB)
 */

// Redirect console.log to stderr (WASM init logs)
console.log = (...args: unknown[]) => console.error(...args);

import { createServer as createHttpServer } from "node:http";
import { StreamableHTTPServerTransport } from "@modelcontextprotocol/sdk/server/streamableHttp.js";
import { Engine } from "@vcad/engine";
import { createServer } from "./server.js";
import { verifyAccessToken } from "./oauth.js";
import { getViewerHtml, MCP_APP_MIME_TYPE } from "./viewer.js";
import { flushTelemetry } from "./telemetry.js";
import { handleLiveRequest } from "./live-route.js";
import { handleArtifactRequest } from "./artifact-route.js";
import { sessionStoreInfo } from "./session-store.js";
import {
  isModernMessage,
  handleModernRequest,
  handleModernListen,
} from "./protocol-2026.js";

const PORT = parseInt(process.env.PORT || "8080", 10);

const API_KEY = process.env.MCP_API_KEY || "";
const ALLOWED_ORIGINS = (process.env.MCP_ALLOWED_ORIGINS || "")
  .split(",")
  .map((s) => s.trim())
  .filter(Boolean);
const RATE_LIMIT_PER_MINUTE = parseInt(
  process.env.MCP_RATE_LIMIT_PER_MINUTE || "60",
  10,
);
const MAX_BODY_BYTES = parseInt(
  process.env.MCP_MAX_BODY_BYTES || String(10 * 1024 * 1024),
  10,
);

// Pre-initialize the WASM engine once at startup, reuse for all requests
let engine: Engine | undefined;

async function getEngine(): Promise<Engine> {
  if (!engine) {
    engine = await Engine.init();
  }
  return engine;
}

// ── Rate limiter (in-memory, per process) ────────────────────────
// Sliding-window counter keyed by client IP. Good enough to stop casual
// abuse from a single source; production deployments behind a real
// gateway should still front this with their own rate limiting.

interface RateEntry {
  windowStart: number;
  count: number;
}
const rateMap = new Map<string, RateEntry>();
const WINDOW_MS = 60_000;

function clientIp(req: import("node:http").IncomingMessage): string {
  const fwd = req.headers["x-forwarded-for"];
  if (typeof fwd === "string" && fwd.length > 0) {
    return fwd.split(",")[0].trim();
  }
  return req.socket.remoteAddress ?? "unknown";
}

function rateLimitExceeded(ip: string): boolean {
  if (RATE_LIMIT_PER_MINUTE <= 0) return false;
  const now = Date.now();
  const entry = rateMap.get(ip);
  if (!entry || now - entry.windowStart >= WINDOW_MS) {
    rateMap.set(ip, { windowStart: now, count: 1 });
    return false;
  }
  entry.count += 1;
  return entry.count > RATE_LIMIT_PER_MINUTE;
}

// Periodically drop stale rate-limiter entries so the map doesn't leak
// under sustained traffic from many distinct IPs.
setInterval(() => {
  const now = Date.now();
  for (const [ip, entry] of rateMap) {
    if (now - entry.windowStart >= WINDOW_MS * 2) rateMap.delete(ip);
  }
}, WINDOW_MS).unref();

/**
 * Handle an MCP request in stateless mode.
 * Creates a fresh Server + Transport per request, handles it, disposes.
 */
async function handleMcpRequest(
  req: import("node:http").IncomingMessage,
  res: import("node:http").ServerResponse,
): Promise<void> {
  const eng = await getEngine();
  const user = verifyAccessToken(req);
  // assumeUiClient: this stateless path builds a fresh Server per request,
  // so the UI-extension capability from `initialize` is never visible here —
  // attach the inline `_meta` preview unconditionally (see ServerContext).
  const makeServer = () => createServer(eng, { user, assumeUiClient: true });

  // Dual-era: a request that declares its own protocol version (or asks for
  // `server/discover`) speaks MCP 2026-07-28 and is served by the modern
  // handler; `initialize`-based traffic falls through to the SDK transport.
  if (req.method === "POST") {
    const body = await readBody(req);
    let parsed: unknown;
    try {
      parsed = JSON.parse(body);
    } catch {
      res.writeHead(400, { "Content-Type": "application/json" });
      res.end(
        JSON.stringify({
          jsonrpc: "2.0",
          id: null,
          error: { code: -32700, message: "Parse error" },
        }),
      );
      return;
    }

    if (isModernMessage(parsed, req.headers)) {
      await handleModernHttp(req, res, parsed, makeServer);
      return;
    }

    await handleLegacyMcpRequest(req, res, makeServer, parsed);
    return;
  }

  await handleLegacyMcpRequest(req, res, makeServer);
}

/** Write one JSON-RPC message as an SSE event. */
function sseWrite(
  res: import("node:http").ServerResponse,
  message: unknown,
): void {
  res.write(`event: message\ndata: ${JSON.stringify(message)}\n\n`);
}

/** Open an SSE response stream (headers only; events follow). */
function sseOpen(res: import("node:http").ServerResponse): void {
  res.writeHead(200, {
    "Content-Type": "text/event-stream",
    "Cache-Control": "no-cache, no-transform",
    Connection: "keep-alive",
    // Tell reverse proxies (nginx) not to buffer, or events arrive in bursts.
    "X-Accel-Buffering": "no",
  });
}

/**
 * Serve one modern (MCP 2026-07-28) request over HTTP.
 *
 * Response shape by method:
 *  - `subscriptions/listen` → a long-lived SSE stream: acknowledgment first,
 *    then opted-in change notifications, kept alive with SSE comments until
 *    the client closes the stream (which is the cancellation signal).
 *  - `tools/call` → a single JSON object, upgraded to an SSE response stream
 *    the moment the tool emits a request-scoped notification
 *    (`notifications/progress`, `notifications/message`) — the notification(s)
 *    then precede the final response on the stream.
 *  - everything else → a single `application/json` object.
 */
async function handleModernHttp(
  req: import("node:http").IncomingMessage,
  res: import("node:http").ServerResponse,
  parsed: unknown,
  makeServer: () => Promise<Awaited<ReturnType<typeof createServer>>>,
): Promise<void> {
  const method = (parsed as { method?: string } | null)?.method;

  if (method === "subscriptions/listen") {
    sseOpen(res);
    const handle = handleModernListen(parsed, {
      send: (m) => sseWrite(res, m),
    });
    // SSE comment keep-alive so intermediaries don't reap the quiet stream.
    const keepAlive = setInterval(() => res.write(":\n\n"), 25_000);
    keepAlive.unref();
    req.on("close", () => {
      clearInterval(keepAlive);
      handle.close();
      res.end();
    });
    return;
  }

  // The stream is opened lazily, on the first request-scoped notification:
  // that keeps validation failures (400 with a JSON-RPC error body, as the
  // spec requires) on the plain-JSON path, while a tool that emits progress
  // gets a real SSE response stream. Both response shapes are ones the client
  // MUST support.
  let streamOpen = false;
  const { status, body: out } = await handleModernRequest(parsed, {
    createServer: makeServer,
    headers: req.headers,
    onNotification:
      method === "tools/call"
        ? (n) => {
            if (!streamOpen) {
              sseOpen(res);
              streamOpen = true;
            }
            sseWrite(res, n);
          }
        : undefined,
  });

  if (streamOpen) {
    if (out !== null) sseWrite(res, out);
    res.end();
    return;
  }

  if (out === null) {
    res.writeHead(status);
    res.end();
    return;
  }
  res.writeHead(status, { "Content-Type": "application/json" });
  res.end(JSON.stringify(out));
}

/** The pre-2026 (`initialize`-handshake) path, on the SDK's own transport. */
async function handleLegacyMcpRequest(
  req: import("node:http").IncomingMessage,
  res: import("node:http").ServerResponse,
  makeServer: () => Promise<Awaited<ReturnType<typeof createServer>>>,
  parsed?: unknown,
): Promise<void> {
  const server = await makeServer();

  const transport = new StreamableHTTPServerTransport({
    // Stateless: no session tracking
    sessionIdGenerator: undefined,
  });

  await server.connect(transport);

  try {
    if (parsed !== undefined) {
      await transport.handleRequest(req, res, parsed);
    } else {
      await transport.handleRequest(req, res);
    }
  } finally {
    await transport.close();
    await server.close();
  }
}

/** Read request body as string, enforcing a max size (default MAX_BODY_BYTES;
 *  callers like the live annotate route pass a much smaller cap). */
function readBody(
  req: import("node:http").IncomingMessage,
  maxBytes: number = MAX_BODY_BYTES,
): Promise<string> {
  return new Promise((resolve, reject) => {
    const chunks: Buffer[] = [];
    let total = 0;
    req.on("data", (chunk: Buffer) => {
      total += chunk.length;
      if (total > maxBytes) {
        reject(new Error("Request body exceeds limit"));
        req.destroy();
        return;
      }
      chunks.push(chunk);
    });
    req.on("end", () => resolve(Buffer.concat(chunks).toString("utf-8")));
    req.on("error", reject);
  });
}

/** Echo the Origin header iff it's on the allowlist. */
function setCors(
  req: import("node:http").IncomingMessage,
  res: import("node:http").ServerResponse,
): void {
  const origin = req.headers.origin;
  if (typeof origin === "string" && ALLOWED_ORIGINS.includes(origin)) {
    res.setHeader("Access-Control-Allow-Origin", origin);
    res.setHeader("Vary", "Origin");
    res.setHeader("Access-Control-Allow-Methods", "GET, POST, DELETE, OPTIONS");
    res.setHeader(
      "Access-Control-Allow-Headers",
      "Authorization, Content-Type, mcp-session-id, Last-Event-ID, mcp-protocol-version, mcp-method, mcp-name",
    );
    res.setHeader(
      "Access-Control-Expose-Headers",
      "mcp-session-id, mcp-protocol-version",
    );
  }
}

/** Return true if the request presents a valid Bearer token (or API key is disabled). */
function isAuthorized(req: import("node:http").IncomingMessage): boolean {
  if (!API_KEY) return true; // auth disabled — startup logged a warning
  const header = req.headers.authorization;
  if (typeof header !== "string") return false;
  const match = /^Bearer\s+(.+)$/i.exec(header);
  if (!match) return false;
  const provided = match[1];
  // Constant-time compare to avoid timing oracles.
  if (provided.length !== API_KEY.length) return false;
  let diff = 0;
  for (let i = 0; i < provided.length; i++) {
    diff |= provided.charCodeAt(i) ^ API_KEY.charCodeAt(i);
  }
  return diff === 0;
}

// ── HTTP Server ──────────────────────────────────────────────────

const httpServer = createHttpServer(async (req, res) => {
  setCors(req, res);

  // CORS preflight
  if (req.method === "OPTIONS") {
    res.writeHead(204);
    res.end();
    return;
  }

  const url = new URL(req.url ?? "/", `http://${req.headers.host}`);
  const path = url.pathname;

  try {
    // MCP endpoint — auth + rate limit
    if (path === "/mcp") {
      if (!isAuthorized(req)) {
        res.writeHead(401, { "Content-Type": "text/plain" });
        res.end("Unauthorized");
        return;
      }
      if (rateLimitExceeded(clientIp(req))) {
        res.writeHead(429, { "Content-Type": "text/plain" });
        res.end("Too Many Requests");
        return;
      }
      await handleMcpRequest(req, res);
      await flushTelemetry();
      return;
    }

    // Viewer HTML (for MCP Apps and debugging)
    if (path === "/viewer.html") {
      res.writeHead(200, {
        "Content-Type": MCP_APP_MIME_TYPE,
        "Cache-Control": "public, max-age=300",
      });
      res.end(getViewerHtml());
      return;
    }

    // Artifact channel (/artifacts/*) — large export/fab bundles offloaded out
    // of model context. Capability-keyed by the unguessable id in the path.
    if (await handleArtifactRequest(req, res)) {
      return;
    }

    // Live review window (/live/*) — shared, flag-gated handler. Returns true
    // once it has written a response.
    if (await handleLiveRequest(req, res, { user: verifyAccessToken(req), getEngine })) {
      return;
    }

    // Health check. `durable` / `session_store` make the session-durability
    // state observable with a plain curl — durable:false means a redeploy will
    // drop open sessions (service-role key unset).
    if (path === "/health") {
      const eng = await getEngine();
      res.writeHead(200, { "Content-Type": "application/json" });
      res.end(JSON.stringify({
        status: "ok",
        engine: !!eng,
        ...sessionStoreInfo(),
        timestamp: new Date().toISOString(),
      }));
      return;
    }

    // 404
    res.writeHead(404, { "Content-Type": "text/plain" });
    res.end("Not Found");
  } catch (err) {
    console.error("Request error:", err);
    if (!res.headersSent) {
      res.writeHead(500, { "Content-Type": "text/plain" });
      res.end("Internal Server Error");
    }
  }
});

// ── Start ────────────────────────────────────────────────────────

async function main() {
  // Pre-warm the engine
  await getEngine();
  console.error(`[vcad-mcp] Engine initialized`);

  if (!API_KEY) {
    console.error(
      "[vcad-mcp] WARNING: MCP_API_KEY is not set — /mcp is open to anonymous callers. " +
        "Do not use this configuration on a public network.",
    );
  }
  if (ALLOWED_ORIGINS.length === 0) {
    console.error(
      "[vcad-mcp] MCP_ALLOWED_ORIGINS is empty — no cross-origin browsers will be accepted.",
    );
  }

  httpServer.listen(PORT, () => {
    console.error(`[vcad-mcp] HTTP server listening on port ${PORT}`);
    console.error(`[vcad-mcp] MCP endpoint: http://localhost:${PORT}/mcp`);
    console.error(`[vcad-mcp] Viewer: http://localhost:${PORT}/viewer.html`);
    console.error(`[vcad-mcp] Health: http://localhost:${PORT}/health`);
  });
}

main().catch((err) => {
  console.error("Fatal error:", err);
  process.exit(1);
});
