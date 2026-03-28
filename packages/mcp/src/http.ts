/**
 * HTTP entry point for the vcad MCP server.
 *
 * Runs as a standalone Node.js HTTP server with StreamableHTTPServerTransport.
 * Designed for deployment to Vercel, Fly.io, or any Node.js host.
 *
 * Stateless mode: each request creates a fresh transport + server.
 * No session persistence — every tool call is independent.
 */

// Redirect console.log to stderr (WASM init logs)
console.log = (...args: unknown[]) => console.error(...args);

import { createServer as createHttpServer } from "node:http";
import { StreamableHTTPServerTransport } from "@modelcontextprotocol/sdk/server/streamableHttp.js";
import { Engine } from "@vcad/engine";
import { createServer } from "./server.js";
import { getViewerHtml, MCP_APP_MIME_TYPE } from "./viewer.js";

const PORT = parseInt(process.env.PORT || "8080", 10);

// Pre-initialize the WASM engine once at startup, reuse for all requests
let engine: Engine | undefined;

async function getEngine(): Promise<Engine> {
  if (!engine) {
    engine = await Engine.init();
  }
  return engine;
}

/**
 * Handle an MCP request in stateless mode.
 * Creates a fresh Server + Transport per request, handles it, disposes.
 */
async function handleMcpRequest(
  req: import("node:http").IncomingMessage,
  res: import("node:http").ServerResponse,
): Promise<void> {
  const eng = await getEngine();
  const server = await createServer(eng);

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

/** Read request body as string. */
function readBody(req: import("node:http").IncomingMessage): Promise<string> {
  return new Promise((resolve, reject) => {
    const chunks: Buffer[] = [];
    req.on("data", (chunk: Buffer) => chunks.push(chunk));
    req.on("end", () => resolve(Buffer.concat(chunks).toString("utf-8")));
    req.on("error", reject);
  });
}

/** Set CORS headers for cross-origin MCP clients. */
function setCors(res: import("node:http").ServerResponse): void {
  res.setHeader("Access-Control-Allow-Origin", "*");
  res.setHeader("Access-Control-Allow-Methods", "GET, POST, DELETE, OPTIONS");
  res.setHeader(
    "Access-Control-Allow-Headers",
    "Content-Type, mcp-session-id, Last-Event-ID, mcp-protocol-version",
  );
  res.setHeader(
    "Access-Control-Expose-Headers",
    "mcp-session-id, mcp-protocol-version",
  );
}

// ── HTTP Server ──────────────────────────────────────────────────

const httpServer = createHttpServer(async (req, res) => {
  setCors(res);

  // CORS preflight
  if (req.method === "OPTIONS") {
    res.writeHead(204);
    res.end();
    return;
  }

  const url = new URL(req.url ?? "/", `http://${req.headers.host}`);
  const path = url.pathname;

  try {
    // MCP endpoint
    if (path === "/mcp") {
      await handleMcpRequest(req, res);
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
