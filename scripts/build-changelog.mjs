#!/usr/bin/env node

/**
 * Aggregate `changelog/entries/*.json` into a single `CHANGELOG.json` at the
 * repo root.
 *
 * Each entry lives in its own file so concurrent PRs don't fight over a
 * shared array. The rolled-up `CHANGELOG.json` is generated on demand and is
 * gitignored — it exists only so existing consumers (`@vcad/core` static
 * import, the docs site, release tooling) don't need to know about the
 * underlying directory layout.
 *
 * Sort order: newest first, by `date` descending then `id` descending so two
 * entries on the same day are stable.
 */

import { readFileSync, writeFileSync, readdirSync } from "fs";
import { join, dirname } from "path";
import { fileURLToPath } from "url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const root = join(__dirname, "..");
const entriesDir = join(root, "changelog", "entries");
const outFile = join(root, "CHANGELOG.json");

export function loadEntries() {
  const files = readdirSync(entriesDir).filter((f) => f.endsWith(".json"));
  const entries = files.map((f) => {
    const path = join(entriesDir, f);
    try {
      return JSON.parse(readFileSync(path, "utf8"));
    } catch (err) {
      throw new Error(`Failed to parse ${path}: ${err.message}`);
    }
  });
  entries.sort((a, b) => {
    if (a.date !== b.date) return a.date < b.date ? 1 : -1;
    return a.id < b.id ? 1 : -1;
  });
  return entries;
}

export function buildChangelog() {
  return {
    $schema: "./changelog.schema.json",
    entries: loadEntries(),
  };
}

// When run directly, write the rolled-up file.
if (import.meta.url === `file://${process.argv[1]}`) {
  const data = buildChangelog();
  writeFileSync(outFile, JSON.stringify(data, null, 2) + "\n");
  console.log(`[changelog] wrote ${data.entries.length} entries to CHANGELOG.json`);
}
