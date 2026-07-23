#!/usr/bin/env node
// Self-validating MCP server launcher: builds exactly what the checkout
// contains before serving, so a stale dist can never be served.
//
// Point MCP configs here instead of dist/index.js:
//   { "command": "node", "args": ["<repo>/packages/mcp/scripts/serve.mjs"] }
//
// How it works:
//   1. Compute a source fingerprint: the git tree hash of packages/ + lib/ at
//      HEAD, plus a digest of any uncommitted changes under them.
//   2. Compare against dist/.build-stamp.json (written after a good build).
//   3. On mismatch, run the workspace build (VCAD_WASM_SKIP=1 — kernel WASM
//      artifacts are CI-owned and checked in), then stamp.
//   4. Best-effort: warn on stderr when the checkout is behind origin/main —
//      the launcher guarantees dist==src, but only the human can move HEAD.
//   5. exec dist/index.js with stdio passed through.

import { execFileSync, execSync, spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repo = join(here, "..", "..", "..");
const mcpDir = join(repo, "packages", "mcp");
const stampPath = join(mcpDir, "dist", ".build-stamp.json");
const log = (msg) => process.stderr.write(`[vcad-mcp serve] ${msg}\n`);

const git = (...args) =>
  execFileSync("git", ["-C", repo, ...args], { encoding: "utf8" }).trim();

function sourceFingerprint() {
  // Tree hash covers every committed source the server can depend on.
  const tracked = ["packages", "lib"].map((p) => {
    try {
      return git("rev-parse", `HEAD:${p}`);
    } catch {
      return "none";
    }
  });
  // Digest uncommitted changes (content, not just paths) under the same dirs.
  const dirty = git("status", "--porcelain", "--", "packages", "lib");
  let dirtyDigest = "clean";
  if (dirty) {
    const h = createHash("sha256").update(dirty);
    for (const line of dirty.split("\n")) {
      const file = line.slice(3).split(" -> ").pop();
      try {
        h.update(readFileSync(join(repo, file)));
      } catch {
        h.update(`missing:${file}`);
      }
    }
    dirtyDigest = h.digest("hex");
  }
  return `${tracked.join("+")}@${dirtyDigest}`;
}

function readStamp() {
  try {
    return JSON.parse(readFileSync(stampPath, "utf8")).fingerprint;
  } catch {
    return null;
  }
}

const fingerprint = sourceFingerprint();
const entry = join(mcpDir, "dist", "index.js");

if (readStamp() !== fingerprint || !existsSync(entry)) {
  log("dist is stale (or unstamped) — rebuilding workspace from source…");
  execSync("npm run build --workspaces --if-present", {
    cwd: repo,
    stdio: ["ignore", process.stderr, process.stderr], // keep stdout clean for MCP
    env: { ...process.env, VCAD_WASM_SKIP: "1" },
  });
  writeFileSync(
    stampPath,
    JSON.stringify({ fingerprint, builtAt: new Date().toISOString() }, null, 2),
  );
  log("rebuild complete.");
}

// Advisory only: dist now matches HEAD, but HEAD itself may be old.
try {
  execFileSync("git", ["-C", repo, "fetch", "-q", "--no-tags", "origin", "main"], {
    timeout: 5000,
  });
  const behind = git("rev-list", "--count", "HEAD..origin/main");
  const branch = git("rev-parse", "--abbrev-ref", "HEAD");
  if (behind !== "0")
    log(`WARNING: checkout '${branch}' is ${behind} commits behind origin/main — server is self-consistent but not latest.`);
} catch {
  // offline or no remote — fine, serve what we have
}

const child = spawn(process.execPath, [entry, ...process.argv.slice(2)], {
  stdio: "inherit",
});
child.on("exit", (code, signal) =>
  process.exit(code ?? (signal ? 1 : 0)),
);
// touch
