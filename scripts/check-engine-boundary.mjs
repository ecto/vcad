#!/usr/bin/env node
/**
 * Guard the "one engine" boundary (docs: the engine roadmap / DocCommand).
 *
 * 1. Hard rule — in @vcad/core and @vcad/app, the Rust document engine is
 *    mutated only through `stores/engine-command.ts` (`applyCommand` /
 *    `applyToolCall`). A direct `engine.set_param(...)`-style call anywhere
 *    else bypasses fail-closed validation and the parity guarantees.
 * 2. Ratchet — geometry math (`Math.sin/cos/tan/atan2/sqrt/hypot/acos/asin`)
 *    in TypeScript packages may only go down. Domain logic belongs in Rust;
 *    when you port some, lower the baseline with `--update`.
 *
 * Usage: node scripts/check-engine-boundary.mjs [--update]
 */
import { readdirSync, readFileSync, statSync, writeFileSync, existsSync } from "node:fs";
import { join, relative, dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { execFileSync } from "node:child_process";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const baselinePath = join(root, "scripts", "engine-boundary-baseline.json");
const update = process.argv.includes("--update");

// Count only git-tracked sources. Gitignored build outputs (e.g.
// `*.generated.ts`) otherwise inflate the counts on any checkout that has
// built, so a local `--update` wrote a baseline CI could never reproduce.
let tracked = null;
try {
  tracked = new Set(
    execFileSync("git", ["ls-files", "-z"], { cwd: root, encoding: "utf8" })
      .split("\0")
      .filter(Boolean)
      .map((f) => join(root, f)),
  );
} catch {
  // Not a git checkout (e.g. a tarball): fall back to the filesystem walk.
}

function walk(dir, out = []) {
  if (!existsSync(dir)) return out;
  for (const name of readdirSync(dir)) {
    if (name === "node_modules" || name === "dist" || name === "__tests__") continue;
    const p = join(dir, name);
    let isDir;
    try {
      isDir = statSync(p).isDirectory();
    } catch {
      continue; // dangling symlink (e.g. core/src/CHANGELOG.json before the changelog build)
    }
    if (isDir) walk(p, out);
    else if (tracked && !tracked.has(p)) continue;
    else if (/\.(ts|tsx)$/.test(name) && !/\.(test|spec)\.tsx?$/.test(name) && !name.endsWith(".d.ts")) out.push(p);
  }
  return out;
}

const MUTATIONS =
  "add_feature|update_feature|delete_feature_by_id|delete_feature|set_translation|set_rotation|" +
  "set_scale|set_material|set_visible|rename_feature|set_joint_state|create_feature|set_param|" +
  "move_feature|import_ir|undo|redo";
const mutationCall = new RegExp(`\\b(?:engine|_crdtEngine|crdtEngine)!?\\.(?:${MUTATIONS})\\(`);
const ALLOWED = new Set(["packages/core/src/stores/engine-command.ts"]);

const violations = [];
for (const pkg of ["core", "app"]) {
  for (const file of walk(join(root, "packages", pkg, "src"))) {
    const rel = relative(root, file);
    if (ALLOWED.has(rel)) continue;
    readFileSync(file, "utf8")
      .split("\n")
      .forEach((line, i) => {
        if (mutationCall.test(line) && !line.trim().startsWith("//") && !line.trim().startsWith("*")) {
          violations.push(`${rel}:${i + 1}: ${line.trim()}`);
        }
      });
  }
}

const math = /\bMath\.(?:sin|cos|tan|atan2|sqrt|hypot|acos|asin)\(/g;
const counts = {};
for (const pkg of readdirSync(join(root, "packages"))) {
  let n = 0;
  for (const file of walk(join(root, "packages", pkg, "src"))) {
    n += (readFileSync(file, "utf8").match(math) ?? []).length;
  }
  if (n > 0) counts[pkg] = n;
}

if (update) {
  writeFileSync(baselinePath, JSON.stringify(counts, null, 2) + "\n");
  console.log(`[engine-boundary] baseline written: ${JSON.stringify(counts)}`);
}
const baseline = existsSync(baselinePath) ? JSON.parse(readFileSync(baselinePath, "utf8")) : {};
const grew = Object.entries(counts).filter(([pkg, n]) => n > (baseline[pkg] ?? 0));
const shrank = Object.entries(baseline).filter(([pkg, n]) => (counts[pkg] ?? 0) < n);

let failed = false;
if (violations.length) {
  failed = true;
  console.error(
    "[engine-boundary] direct engine mutations outside engine-command.ts — use applyCommand/applyToolCall:\n  " +
      violations.join("\n  "),
  );
}
if (grew.length) {
  failed = true;
  console.error(
    "[engine-boundary] geometry math grew in TypeScript (port it to Rust instead):\n  " +
      grew.map(([pkg, n]) => `${pkg}: ${baseline[pkg] ?? 0} → ${n}`).join("\n  "),
  );
}
if (shrank.length && !update) {
  console.log(
    "[engine-boundary] geometry math shrank — run `node scripts/check-engine-boundary.mjs --update` to lock it in: " +
      shrank.map(([pkg, n]) => `${pkg} ${n} → ${counts[pkg] ?? 0}`).join(", "),
  );
}
if (failed) process.exit(1);
console.log(`[engine-boundary] ok (${Object.entries(counts).map(([k, v]) => `${k} ${v}`).join(", ")})`);
