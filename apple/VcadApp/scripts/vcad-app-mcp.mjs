#!/usr/bin/env node
// stdio ↔ HTTP shim for the vcad desktop app's agent bridge.
//
// The app serves MCP on http://127.0.0.1:<port>/mcp and writes the port to
// ~/.vcad/app.json. MCP clients that only speak stdio point here:
//
//   { "command": "node", "args": ["/path/to/vcad/apple/VcadApp/scripts/vcad-app-mcp.mjs"] }
//
// Each newline-delimited JSON-RPC message on stdin is POSTed to the app; the
// JSON response (if any — notifications get 202) is written to stdout. The
// port file is re-read on every request, so relaunching the app needs no
// client restart.
import { readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { createInterface } from "node:readline";

const discovery = join(homedir(), ".vcad", "app.json");

function endpoint() {
  if (process.env.VCAD_APP_URL) return process.env.VCAD_APP_URL;
  const info = JSON.parse(readFileSync(discovery, "utf8"));
  return info.url ?? `http://127.0.0.1:${info.port}/mcp`;
}

function reply(obj) {
  process.stdout.write(JSON.stringify(obj) + "\n");
}

const rl = createInterface({ input: process.stdin });
rl.on("line", async (line) => {
  if (!line.trim()) return;
  let msg;
  try {
    msg = JSON.parse(line);
  } catch {
    return reply({ jsonrpc: "2.0", id: null, error: { code: -32700, message: "parse error" } });
  }
  try {
    const res = await fetch(endpoint(), {
      method: "POST",
      headers: { "content-type": "application/json", accept: "application/json, text/event-stream" },
      body: line,
    });
    if (res.status === 202 || msg.id === undefined) return;
    reply(await res.json());
  } catch (e) {
    if (msg.id === undefined) return;
    reply({
      jsonrpc: "2.0",
      id: msg.id,
      error: { code: -32000, message: `vcad app not reachable (${e.message}). Is vcad.app running? (${discovery})` },
    });
  }
});
