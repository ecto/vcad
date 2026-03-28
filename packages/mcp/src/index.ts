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
import { createServer } from "./server.js";

async function main() {
  const server = await createServer();
  const transport = new StdioServerTransport();
  await server.connect(transport);
}

main().catch((err) => {
  console.error("Fatal error:", err);
  process.exit(1);
});
