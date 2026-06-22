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
import { createSessionEventStore } from "./session-store.js";
import { appendOverlay, listEvents } from "./tools/live.js";

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
  const server = await createServer(eng, { user });

  const transport = new StreamableHTTPServerTransport({
    // Stateless: no session tracking
    sessionIdGenerator: undefined,
  });

  await server.connect(transport);

  try {
    // Parse body for POST requests
    if (req.method === "POST") {
      const body = await readBody(req);
      const parsed = JSON.parse(body);
      await transport.handleRequest(req, res, parsed);
    } else {
      await transport.handleRequest(req, res);
    }
  } finally {
    await transport.close();
    await server.close();
  }
}

/**
 * Handle a live-review-window request. Capability-keyed by the session id in the
 * path (`/live/<id>/<action>`) — possession of the unguessable id is the grant,
 * same model as mcp_sessions. Reads/appends go through the service-role event
 * store, so they work for both anon and signed-in sessions by session id alone.
 *
 *   GET  /live/<id>/events[?since=N]  → the session's spine events (replay)
 *   POST /live/<id>/annotate          → append a viewer overlay (pin/flag/…)
 *
 * The geometry stream (GLB / fold) and the browser viewer app are a separate,
 * env-gated slice — this is the data backbone the broadcast trigger feeds.
 */
async function handleLiveRequest(
  req: import("node:http").IncomingMessage,
  res: import("node:http").ServerResponse,
): Promise<void> {
  const url = new URL(req.url ?? "/", `http://${req.headers.host}`);
  const parts = url.pathname.split("/").filter(Boolean); // ["live", id, action]
  const sessionId = parts[1] ? decodeURIComponent(parts[1]) : "";
  const action = parts[2] ?? "";
  if (!sessionId) {
    res.writeHead(400, { "Content-Type": "text/plain" });
    res.end("missing session id");
    return;
  }

  // Viewers are usually anonymous (the link is the capability); a Bearer token,
  // if present, just attributes the actor. The event store reads/writes by
  // session id via the service role regardless.
  const user = verifyAccessToken(req);
  const eventStore = createSessionEventStore(user);

  if (req.method === "GET" && action === "events") {
    // Each call does a per-session fetch; throttle this public read like annotate.
    if (rateLimitExceeded(clientIp(req))) {
      res.writeHead(429, { "Content-Type": "text/plain" });
      res.end("Too Many Requests");
      return;
    }
    const sinceRaw = url.searchParams.get("since");
    const since = sinceRaw != null && sinceRaw !== "" ? Number(sinceRaw) : undefined;
    const events = await listEvents(
      eventStore,
      sessionId,
      Number.isFinite(since) ? since : undefined,
    );
    res.writeHead(200, { "Content-Type": "application/json" });
    res.end(JSON.stringify({ session_id: sessionId, events }));
    return;
  }

  if (req.method === "POST" && action === "annotate") {
    if (rateLimitExceeded(clientIp(req))) {
      res.writeHead(429, { "Content-Type": "text/plain" });
      res.end("Too Many Requests");
      return;
    }
    let parsed: unknown;
    try {
      // An overlay is tiny — a far smaller cap than the 10 MiB /mcp default.
      parsed = JSON.parse(await readBody(req, 16 * 1024));
    } catch {
      res.writeHead(400, { "Content-Type": "application/json" });
      res.end(JSON.stringify({ ok: false, error: "invalid or oversized json body" }));
      return;
    }
    // Tolerate null / array / scalar bodies — appendOverlay rejects the empty
    // overlay with a clean 400 instead of throwing a 500.
    const body =
      parsed && typeof parsed === "object" && !Array.isArray(parsed)
        ? (parsed as Record<string, unknown>)
        : {};
    const result = await appendOverlay(
      eventStore,
      sessionId,
      {
        type: typeof body.type === "string" ? body.type : "",
        payload: body.payload,
        author: typeof body.author === "string" ? body.author : undefined,
      },
      // The verified token identity is authoritative; a body author is only ever
      // honored (namespaced) for genuinely anonymous viewers.
      { trustedAuthor: user?.email || undefined },
    );
    res.writeHead(result.ok ? 200 : 400, { "Content-Type": "application/json" });
    res.end(JSON.stringify(result));
    return;
  }

  res.writeHead(404, { "Content-Type": "text/plain" });
  res.end("Not Found");
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
      "Authorization, Content-Type, mcp-session-id, Last-Event-ID, mcp-protocol-version",
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

    // Live review window — capability-keyed by the (unguessable) session id.
    // Flag-gated: a new public read/append surface, off until VCAD_LIVE_WINDOW=1.
    if (path.startsWith("/live/")) {
      if (process.env.VCAD_LIVE_WINDOW !== "1") {
        res.writeHead(404, { "Content-Type": "text/plain" });
        res.end("Not Found");
        return;
      }
      await handleLiveRequest(req, res);
      return;
    }

    // Health check
    if (path === "/health") {
      const eng = await getEngine();
      res.writeHead(200, { "Content-Type": "application/json" });
      res.end(JSON.stringify({
        status: "ok",
        engine: !!eng,
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
