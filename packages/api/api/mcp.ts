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

// Lazy imports — these are heavy (WASM engine), so we import dynamically
// to avoid loading them on every cold start for non-MCP routes.
let _createServer: typeof import("@vcad/mcp/server").createServer | undefined;
let _StreamableHTTPServerTransport: typeof import("@modelcontextprotocol/sdk/server/streamableHttp.js").StreamableHTTPServerTransport | undefined;
let _engine: Awaited<ReturnType<typeof import("@vcad/engine").Engine.init>> | undefined;

async function getServerDeps() {
  if (!_createServer) {
    const [mcpMod, sdkMod, engineMod] = await Promise.all([
      import("@vcad/mcp/server"),
      import("@modelcontextprotocol/sdk/server/streamableHttp.js"),
      import("@vcad/engine"),
    ]);
    _createServer = mcpMod.createServer;
    _StreamableHTTPServerTransport = sdkMod.StreamableHTTPServerTransport;
    _engine = await engineMod.Engine.init();
  }
  return {
    createServer: _createServer!,
    StreamableHTTPServerTransport: _StreamableHTTPServerTransport!,
    engine: _engine!,
  };
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
      await getServerDeps();

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
