#!/usr/bin/env node
// "TS lines of document logic left outside Rust" -- daily metric.
// Counts non-blank, non-comment lines of every file in doc-logic-manifest.json,
// per package and overall. Whole files are counted; `partial: true` files are
// flagged and also reported as a separate subtotal.
//
//   node scripts/doc-logic-loc.mjs [--json]

import { readFileSync, existsSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, "..");
const PACKAGES = ["core", "app", "mcp", "engine"];
const json = process.argv.includes("--json");

/** Count lines that contain code outside comments (strings/templates aware). */
export function countCodeLines(src) {
  let count = 0;
  let inBlock = false;
  let inTemplate = false;
  for (const line of src.split("\n")) {
    let hasCode = false;
    let quote = null;
    for (let i = 0; i < line.length; i++) {
      const ch = line[i];
      const next = line[i + 1];
      if (inBlock) {
        if (ch === "*" && next === "/") { inBlock = false; i++; }
        continue;
      }
      if (quote || inTemplate) {
        hasCode = true;
        const q = inTemplate ? "`" : quote;
        if (ch === "\\") { i++; continue; }
        if (ch === q) { quote = null; inTemplate = false; }
        continue;
      }
      if (ch === "/" && next === "/") break;
      if (ch === "/" && next === "*") { inBlock = true; i++; continue; }
      if (/\s/.test(ch)) continue;
      hasCode = true;
      if (ch === '"' || ch === "'") quote = ch;
      else if (ch === "`") inTemplate = true;
    }
    if (hasCode) count++;
    quote = null; // single/double-quoted strings do not span lines
  }
  return count;
}

const manifest = JSON.parse(readFileSync(join(here, "doc-logic-manifest.json"), "utf8"));
const tracked = new Set(
  execFileSync("git", ["ls-files", "packages"], { cwd: root, encoding: "utf8", maxBuffer: 1 << 28 })
    .split("\n").filter(Boolean),
);

const problems = [];
const seen = new Set();
const files = [];
for (const entry of manifest.files) {
  const { path, reason, partial = false } = entry;
  const pkg = path.split("/")[1];
  if (seen.has(path)) problems.push(`duplicate: ${path}`);
  seen.add(path);
  if (!PACKAGES.includes(pkg) || !/\.tsx?$/.test(path)) problems.push(`out of scope: ${path}`);
  else if (/(\.test\.|\.spec\.|\/__tests__\/|generated|\.d\.ts$)/.test(path)) problems.push(`test/generated: ${path}`);
  else if (!tracked.has(path) || !existsSync(join(root, path))) problems.push(`not tracked or missing: ${path}`);
  else if (!reason) problems.push(`missing reason: ${path}`);
  else files.push({ path, pkg, partial, reason, loc: countCodeLines(readFileSync(join(root, path), "utf8")) });
}
if (problems.length) {
  console.error("doc-logic manifest errors:\n  " + problems.join("\n  "));
  process.exit(1);
}

const byPackage = Object.fromEntries(PACKAGES.map((p) => [p, { files: 0, loc: 0, partialLoc: 0 }]));
for (const f of files) {
  const b = byPackage[f.pkg];
  b.files++;
  b.loc += f.loc;
  if (f.partial) b.partialLoc += f.loc;
}
const total = { files: files.length, loc: 0, partialLoc: 0 };
for (const b of Object.values(byPackage)) { total.loc += b.loc; total.partialLoc += b.partialLoc; }

if (json) {
  console.log(JSON.stringify({ total, byPackage, files: files.map(({ pkg, ...f }) => f) }, null, 2));
} else {
  console.log("TS document-logic lines outside Rust (non-blank, non-comment)\n");
  for (const p of PACKAGES) {
    const b = byPackage[p];
    console.log(`  ${p.padEnd(8)} ${String(b.loc).padStart(7)}  (${b.files} files, ${b.partialLoc} in partial files)`);
  }
  console.log(`  ${"TOTAL".padEnd(8)} ${String(total.loc).padStart(7)}  (${total.files} files, ${total.partialLoc} in partial files)`);
}
