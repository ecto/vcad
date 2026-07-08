#!/usr/bin/env node
// HTTP <-> MCP-stdio bridge. Spawns the freshly-built vcad MCP server from the
// worktree and exposes POST /call {name, arguments} on localhost:8747.
// Responses: { ok, text, images: [savedPngPath...] } — image content is written
// to files so the caller can Read them.
import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { writeFileSync, mkdirSync } from "node:fs";

const WORKTREE = "/Users/cam/Developer/vcad/.claude/worktrees/wonderful-bardeen-ce469d";
const SERVER = `${WORKTREE}/packages/mcp/dist/index.js`;
const IMG_DIR = `${WORKTREE}/fab/renders`;
const PORT = 8747;
mkdirSync(IMG_DIR, { recursive: true });

const child = spawn("node", [SERVER], { cwd: WORKTREE, stdio: ["pipe", "pipe", "pipe"] });
child.stderr.on("data", (d) => process.stderr.write(`[srv] ${d}`));
child.on("exit", (c) => { console.error(`server exited ${c}`); process.exit(1); });

let buf = "";
const pending = new Map();
let nextId = 1;
child.stdout.on("data", (d) => {
  buf += d.toString();
  let nl;
  while ((nl = buf.indexOf("\n")) >= 0) {
    const line = buf.slice(0, nl); buf = buf.slice(nl + 1);
    if (!line.trim()) continue;
    let msg; try { msg = JSON.parse(line); } catch { continue; }
    if (msg.id != null && pending.has(msg.id)) {
      const { resolve } = pending.get(msg.id);
      pending.delete(msg.id);
      resolve(msg);
    }
  }
});

function rpc(method, params) {
  const id = nextId++;
  const p = new Promise((resolve, reject) => {
    pending.set(id, { resolve, reject });
    setTimeout(() => { if (pending.delete(id)) reject(new Error("timeout")); }, 120000);
  });
  child.stdin.write(JSON.stringify({ jsonrpc: "2.0", id, method, params }) + "\n");
  return p;
}

const init = await rpc("initialize", {
  protocolVersion: "2024-11-05",
  capabilities: {},
  clientInfo: { name: "bridge", version: "1.0" },
});
child.stdin.write(JSON.stringify({ jsonrpc: "2.0", method: "notifications/initialized" }) + "\n");
console.error(`initialized: ${init.result?.serverInfo?.name} ${init.result?.serverInfo?.version}`);

let imgSeq = 0;
createServer(async (req, res) => {
  const chunks = [];
  for await (const c of req.chunks ?? req) chunks.push(c);
  let body = {};
  try { body = JSON.parse(Buffer.concat(chunks).toString() || "{}"); } catch {}
  try {
    let out;
    if (req.url === "/list") {
      out = await rpc("tools/list", {});
      const names = (out.result?.tools ?? []).map((t) => t.name);
      res.end(JSON.stringify({ count: names.length, names }));
      return;
    }
    out = await rpc("tools/call", { name: body.name, arguments: body.arguments ?? {} });
    if (out.error) { res.end(JSON.stringify({ ok: false, error: out.error })); return; }
    const content = out.result?.content ?? [];
    const texts = [], images = [];
    for (const c of content) {
      if (c.type === "text") texts.push(c.text);
      else if (c.type === "image") {
        const f = `${IMG_DIR}/r${++imgSeq}-${body.name}.png`;
        writeFileSync(f, Buffer.from(c.data, "base64"));
        images.push(f);
      }
    }
    res.end(JSON.stringify({ ok: !out.result?.isError, isError: out.result?.isError ?? false, text: texts.join("\n"), images }));
  } catch (e) {
    res.statusCode = 500;
    res.end(JSON.stringify({ ok: false, error: String(e) }));
  }
}).listen(PORT, "127.0.0.1", () => console.error(`bridge on :${PORT}`));
