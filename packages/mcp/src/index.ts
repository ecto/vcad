#!/usr/bin/env node
/**
 * @vcad/mcp — MCP server for CAD operations.
 *
 * Provides tools for creating, exporting, and inspecting CAD geometry
 * via the Model Context Protocol.
 */

// Redirect console.log to stderr so WASM init messages don't
// corrupt the JSON-RPC stdio transport (which uses stdout).
const _origLog = console.log;
console.log = (...args: unknown[]) => console.error(...args);

import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import type { JSONRPCMessage } from "@modelcontextprotocol/sdk/types.js";
import { createServer } from "./server.js";
import { isModernMessage, handleModernRequest } from "./protocol-2026.js";

async function main() {
  // stdio is a single trusted local process — no HTTP request, no auth — so
  // the session store stays in-memory (the documents Map is durable for the
  // process lifetime).
  const server = await createServer(undefined, { user: null });
  const transport = new StdioServerTransport();
  await server.connect(transport);

  // Dual-era stdio. `connect` installed the SDK's legacy (`initialize`-based)
  // message handler; wrap it so a client speaking MCP 2026-07-28 — which sends
  // `server/discover` as its opening probe and never sends `initialize` — is
  // served by the modern handler instead of getting a
  // "received request before initialization" error and falling back.
  //
  // The modern path builds a short-lived server per request, which is safe
  // here for the same reason stateless HTTP already is: the session cache
  // (`fallbackDocuments` in tools/session-core.ts) is process-global, and an
  // anonymous stdio caller resolves to it rather than to a per-connection
  // scope. `document_id` handles therefore survive across modern requests —
  // which is exactly the pattern 2026-07-28 prescribes now that the protocol
  // itself carries no session.
  const legacyOnMessage = transport.onmessage?.bind(transport);
  transport.onmessage = (msg: JSONRPCMessage) => {
    if (!isModernMessage(msg)) {
      legacyOnMessage?.(msg);
      return;
    }
    void handleModernRequest(msg, {
      createServer: () => createServer(undefined, { user: null }),
    }).then(({ body }) => {
      if (body !== null) {
        void transport.send(body as JSONRPCMessage);
      }
    });
  };
}

main().catch((err) => {
  console.error("Fatal error:", err);
  process.exit(1);
});
