/**
 * Vercel API route for the vcad MCP server.
 *
 * Stateless: each request creates a fresh Server + Transport,
 * handles the request, and disposes. The WASM engine is initialized
 * once at module scope and reused across warm invocations.
 *
 * Endpoint: POST /api/mcp (MCP protocol)
 *           GET  /api/mcp (SSE stream, if needed)
 *           DELETE /api/mcp (session close, no-op in stateless mode)
 */

import type { VercelRequest, VercelResponse } from "@vercel/node";

// Lazy-load heavy deps on first request (WASM engine ~8MB).
// Module-scope caching keeps them warm across Vercel invocations.
let _deps: {
  createServer: typeof import("../../mcp/src/server.js").createServer;
  StreamableHTTPServerTransport: typeof import("@modelcontextprotocol/sdk/server/streamableHttp.js").StreamableHTTPServerTransport;
  engine: unknown;
} | undefined;

async function getDeps() {
  if (!_deps) {
    const [mcpMod, sdkMod, engineMod] = await Promise.all([
      import("../../mcp/src/server.js"),
      import("@modelcontextprotocol/sdk/server/streamableHttp.js"),
      import("@vcad/engine"),
    ]);
    const engine = await engineMod.Engine.init();
    _deps = {
      createServer: mcpMod.createServer,
      StreamableHTTPServerTransport: sdkMod.StreamableHTTPServerTransport,
      engine,
    };
  }
  return _deps;
}

export default async function handler(req: VercelRequest, res: VercelResponse) {
  // CORS
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

  if (req.method === "OPTIONS") {
    res.status(204).end();
    return;
  }

  try {
    const { createServer, StreamableHTTPServerTransport, engine } =
      await getDeps();

    const server = await createServer(engine);
    const transport = new StreamableHTTPServerTransport({
      sessionIdGenerator: undefined, // stateless
    });

    await server.connect(transport);

    try {
      await transport.handleRequest(req, res, req.body);
    } finally {
      await transport.close();
      await server.close();
    }
  } catch (err) {
    console.error("[vcad-mcp] Error:", err);
    if (!res.headersSent) {
      res.status(500).json({ error: "Internal server error" });
    }
  }
}
