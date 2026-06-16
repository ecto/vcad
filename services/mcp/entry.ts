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
import { createServer } from "@vcad/mcp/server";
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

  // Health check
  if (req.method === "GET" && req.url === "/health") {
    try {
      const engine = await getEngine();
      sendJson(res, 200, {
        status: "ok",
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
    const server = await createServer(engine, { user });
    const transport = new StreamableHTTPServerTransport({
      sessionIdGenerator: undefined, // stateless
      enableJsonResponse: true, // return JSON instead of SSE — Vercel buffers responses and adds Content-Length which breaks SSE streaming
    });

    await server.connect(transport);

    try {
      await transport.handleRequest(req, res);
    } finally {
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
