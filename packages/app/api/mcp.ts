/**
 * Vercel API route for the vcad MCP server.
 *
 * Stateless: each request creates a fresh Server + Transport,
 * handles the request, and disposes. The WASM engine is initialized
 * once at module scope and reused across warm invocations.
 */

import type { VercelRequest, VercelResponse } from "@vercel/node";
import { StreamableHTTPServerTransport } from "@modelcontextprotocol/sdk/server/streamableHttp.js";

// Use workspace package imports (Vercel resolves these via node_modules)
import { createServer } from "@vcad/mcp/server";
import { Engine } from "@vcad/engine";

// Module-scope engine — survives warm invocations
let _engine: Engine | undefined;

async function getEngine(): Promise<Engine> {
  if (!_engine) {
    _engine = await Engine.init();
  }
  return _engine;
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
    const engine = await getEngine();
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
