/**
 * Smoke-test the stdio MCP server for MCP Apps metadata and lean geometry results.
 * Run: node scripts/test-local-mcp-apps.mjs
 */
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import path from "node:path";

const root = path.dirname(fileURLToPath(import.meta.url));
const serverPath = path.join(root, "../dist/index.js");

function send(proc, msg) {
  proc.stdin.write(JSON.stringify(msg) + "\n");
}

function readMessages(proc, onMessage) {
  let buf = "";
  proc.stdout.on("data", (chunk) => {
    buf += chunk.toString();
    let idx;
    while ((idx = buf.indexOf("\n")) !== -1) {
      const line = buf.slice(0, idx).trim();
      buf = buf.slice(idx + 1);
      if (!line) continue;
      try {
        onMessage(JSON.parse(line));
      } catch (e) {
        console.error("bad json:", line.slice(0, 200));
      }
    }
  });
}

const proc = spawn("node", [serverPath], {
  cwd: path.join(root, ".."),
  env: { ...process.env, VCAD_WASM_SKIP: "1" },
  stdio: ["pipe", "pipe", "inherit"],
});

const pending = new Map();
let nextId = 1;

readMessages(proc, (msg) => {
  if (msg.id != null && pending.has(msg.id)) {
    const { resolve, reject } = pending.get(msg.id);
    pending.delete(msg.id);
    if (msg.error) reject(msg.error);
    else resolve(msg.result);
  }
});

function request(method, params) {
  const id = nextId++;
  return new Promise((resolve, reject) => {
    pending.set(id, { resolve, reject });
    send(proc, { jsonrpc: "2.0", id, method, params });
    setTimeout(() => {
      if (pending.has(id)) {
        pending.delete(id);
        reject(new Error(`timeout: ${method}`));
      }
    }, 120000);
  });
}

async function main() {
  await request("initialize", {
    protocolVersion: "2024-11-05",
    capabilities: {
      extensions: {
        "io.modelcontextprotocol/ui": {
          mimeTypes: ["text/html;profile=mcp-app"],
        },
      },
    },
    clientInfo: { name: "local-test", version: "1.0" },
  });
  send(proc, { jsonrpc: "2.0", method: "notifications/initialized", params: {} });

  const tools = await request("tools/list", {});
  const loon = tools.tools.find((t) => t.name === "create_cad_loon");
  if (!loon?._meta?.ui?.resourceUri) {
    throw new Error("create_cad_loon missing _meta.ui.resourceUri");
  }
  console.log("OK tools/list _meta:", loon._meta.ui.resourceUri);

  const resources = await request("resources/list", {});
  const viewer = resources.resources.find((r) => r.uri === "ui://vcad/viewer");
  const csp = viewer?._meta?.ui?.csp?.resourceDomains ?? [];
  if (!csp.includes("blob:")) {
    throw new Error(`viewer CSP missing blob: — got ${JSON.stringify(csp)}`);
  }
  console.log("OK resources/list CSP:", csp);

  const read = await request("resources/read", { uri: "ui://vcad/viewer" });
  const html = read.contents[0];
  if (html.mimeType !== "text/html;profile=mcp-app") {
    throw new Error("wrong viewer mimeType");
  }
  if (!html.text?.includes("vcad-viewer")) {
    throw new Error("viewer HTML missing expected bundle marker");
  }
  const readCsp = html._meta?.ui?.csp?.resourceDomains ?? [];
  if (!readCsp.includes("blob:")) {
    throw new Error(`resources/read CSP missing blob: — got ${JSON.stringify(readCsp)}`);
  }
  console.log("OK resources/read html bytes:", html.text.length, "csp:", readCsp);

  const call = await request("tools/call", {
    name: "create_cad_loon",
    arguments: {
      source: "[root [pipe [cube 60 40 10] [fillet 3]] \"aluminum\"]",
    },
  });
  const totalChars = call.content.reduce((n, c) => n + (c.text?.length ?? 0), 0);
  if (totalChars > 2048) {
    throw new Error(`create_cad_loon result too large (${totalChars} chars) — inline UI may be suppressed`);
  }
  if (!call.structuredContent?.document_id) {
    throw new Error("create_cad_loon missing structuredContent.document_id");
  }
  console.log(
    "OK create_cad_loon lean result:",
    totalChars,
    "chars, document_id:",
    call.structuredContent.document_id,
  );

  const preview = await request("tools/call", {
    name: "get_preview_glb",
    arguments: { document_id: call.structuredContent.document_id },
  });
  const glbBlock = preview.content[0]?.text ?? "";
  if (!glbBlock.includes("_vcad_glb")) {
    throw new Error("get_preview_glb missing _vcad_glb envelope");
  }
  console.log("OK get_preview_glb bytes:", glbBlock.length);

  proc.kill();
  console.log("\nAll local MCP Apps checks passed.");
}

main().catch((e) => {
  console.error("FAIL:", e.message ?? e);
  proc.kill();
  process.exit(1);
});
