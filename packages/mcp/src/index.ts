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
import {
  isModernMessage,
  handleModernRequest,
  handleModernListen,
  type ListenHandle,
} from "./protocol-2026.js";

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
  const listens = new Map<string | number, ListenHandle>();
  transport.onmessage = (msg: JSONRPCMessage) => {
    if (!isModernMessage(msg)) {
      // A modern client cancels a stdio subscription by referencing the
      // listen request's id in notifications/cancelled — intercept that
      // before the legacy path (which knows nothing about it).
      const m = msg as unknown as {
        method?: string;
        params?: { requestId?: string | number };
      };
      const cancelId = m.params?.requestId;
      if (
        m.method === "notifications/cancelled" &&
        cancelId !== undefined &&
        listens.has(cancelId)
      ) {
        listens.get(cancelId)!.close();
        listens.delete(cancelId);
        return;
      }
      legacyOnMessage?.(msg);
      return;
    }

    const method = (msg as unknown as { method?: string }).method;
    if (method === "subscriptions/listen") {
      // Long-lived subscription on the shared channel: acknowledgment first,
      // then opted-in notifications, each tagged with the subscription id so
      // the client can demultiplex. Ends on notifications/cancelled (above)
      // or process exit.
      const id = (msg as unknown as { id?: string | number }).id;
      const handle = handleModernListen(msg, {
        send: (m) => void transport.send(m as JSONRPCMessage),
      });
      if (id !== undefined) listens.set(id, handle);
      return;
    }

    void handleModernRequest(msg, {
      createServer: () => createServer(undefined, { user: null }),
      // Request-scoped notifications share the stdio channel; the SDK
      // client correlates progress via its progressToken.
      onNotification: (n) => void transport.send(n as JSONRPCMessage),
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
