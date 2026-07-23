#!/usr/bin/env node
// Smoke test for the staged @vcad/mcp npm bundle: spawn it as a stdio MCP
// server, complete an `initialize` handshake, and require a serverInfo
// version. Proves the bundle boots and the co-located kernel WASM loads
// (kernel init failures crash the process before it can answer).
//
// Usage: node scripts/mcp-npm-smoke.mjs packages/mcp/npm-dist/index.mjs

import { spawn } from "node:child_process";

const entry = process.argv[2];
if (!entry) {
  console.error("usage: mcp-npm-smoke.mjs <path/to/index.mjs>");
  process.exit(2);
}

const child = spawn(process.execPath, [entry], {
  stdio: ["pipe", "pipe", "inherit"],
});

const timeout = setTimeout(() => {
  console.error("SMOKE FAIL: no initialize response within 60s");
  child.kill();
  process.exit(1);
}, 60_000);

let buf = "";
child.stdout.on("data", (d) => {
  buf += d.toString();
  for (const line of buf.split("\n")) {
    if (!line.trim()) continue;
    let msg;
    try {
      msg = JSON.parse(line);
    } catch {
      continue; // partial line
    }
    if (msg.id === 1) {
      clearTimeout(timeout);
      const info = msg.result?.serverInfo;
      if (!info?.version) {
        console.error("SMOKE FAIL: initialize returned no serverInfo.version", msg);
        child.kill();
        process.exit(1);
      }
      console.log(`SMOKE OK: ${info.name}@${info.version}`);
      child.kill();
      process.exit(0);
    }
  }
});

child.on("exit", (code) => {
  clearTimeout(timeout);
  console.error(`SMOKE FAIL: server exited early (code ${code})`);
  process.exit(1);
});

child.stdin.write(
  JSON.stringify({
    jsonrpc: "2.0",
    id: 1,
    method: "initialize",
    params: {
      protocolVersion: "2025-06-18",
      capabilities: {},
      clientInfo: { name: "mcp-npm-smoke", version: "0" },
    },
  }) + "\n",
);
