#!/usr/bin/env node
/**
 * Bundle the per-type ts-rs output of the source crates (crates/vcad-ir and
 * crates/vcad-receipt, each under <crate>/bindings/bindings/*.ts) into a
 * single packages/ir/src/generated.ts.
 *
 * Deterministic by construction: types are emitted sorted by name, ts-rs's
 * per-file banners are stripped, and the cross-file `import type` lines are
 * dropped (every type lives in this one module, so intra-file references
 * resolve without imports). That determinism is what lets `ir:check` assert
 * the committed file is byte-identical to a fresh generation.
 *
 * Refresh the bindings first (once per source crate):
 *   cargo test -p vcad-ir --features ts-rs export_bindings -- --ignored
 *   cargo test -p vcad-receipt --features ts-rs export_bindings -- --ignored
 * then run this (or `npm run ir:gen`).
 *
 * Pass `--out <path>` to write elsewhere (used by `ir:check` to diff against
 * the committed file without clobbering it).
 */
import { readdirSync, readFileSync, writeFileSync, existsSync } from "node:fs";
import { join, dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
// Crates whose ts-rs bindings feed generated.ts. Each has its own
// `export_bindings` test; type names must be unique across all of them.
const SOURCE_CRATES = ["vcad-ir", "vcad-receipt"];
const outIdx = process.argv.indexOf("--out");
const outPath =
  outIdx !== -1 && process.argv[outIdx + 1]
    ? resolve(process.argv[outIdx + 1])
    : join(root, "packages", "ir", "src", "generated.ts");

// (typeName, dir) pairs from every source crate, sorted by type name so the
// bundle stays deterministic regardless of which crate a type lives in.
const files = [];
const seen = new Map();
for (const crate of SOURCE_CRATES) {
  const dir = join(root, "crates", crate, "bindings", "bindings");
  if (!existsSync(dir)) {
    console.error(
      `[ir:gen] bindings dir not found: ${dir}\n` +
        `Run: cargo test -p ${crate} --features ts-rs export_bindings -- --ignored`,
    );
    process.exit(1);
  }
  for (const f of readdirSync(dir).filter((f) => f.endsWith(".ts"))) {
    if (seen.has(f)) {
      console.error(
        `[ir:gen] type name collision: ${f} exported by both ` +
          `${seen.get(f)} and ${crate} — rename one of the Rust types.`,
      );
      process.exit(1);
    }
    seen.set(f, crate);
    files.push({ name: f, dir });
  }
}
files.sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0));

if (files.length === 0) {
  console.error(`[ir:gen] no .ts files found in any bindings dir`);
  process.exit(1);
}

// ts-rs v10 only honors `#[ts(optional)]` on `Option<T>`. Collection fields
// (`Vec<T>` / `Map<K,V>`) carrying `#[serde(default, skip_serializing_if =
// "…is_empty")]` are omitted from JSON when empty, so they are semantically
// optional on the wire — but ts-rs renders them as required. We can't express
// that in Rust, so force these specific fields optional here. Keyed by type
// name (= file name) so a field name can't collide across types.
// Two ts-rs gaps, both keyed by type name (= file name) so a field name can't
// collide across types:
//   (a) Vec<T>/Map<K,V> + `skip_serializing_if = "…is_empty"` — omitted when
//       empty, so optional on the wire, but ts-rs can't mark them optional.
//   (b) scalar fields with `#[serde(default)]` (no skip) — Rust accepts JSON
//       without them, so they're optional on input; ts-rs renders them required
//       because they're always written. The hand-written contract (and the
//       reader code, e.g. `front !== false`) treats them as optional.
const FORCE_OPTIONAL = {
  Instance: ["tags"],
  Document: ["parameters", "bindings", "clearance_specs", "analysis_studies", "constraints"],
  DesignRules: ["classRules", "netClassAssignments"],
  BoardOutline: ["cutouts"],
  Zone: ["holes", "priority", "minArea", "fillType", "thermalRelief"],
  Footprint: ["graphics", "properties", "rotation", "front"],
  Pcb: ["traceArcs", "keepouts", "netTies"],
  SchematicComponent: ["properties", "rotation", "mirror"],
  SchematicLabel: ["rotation"],
  DrillSpec: ["oval"],
  Pad: ["rotation"],
  // CsgOp variant fields with `#[serde(default)]`/Vec+skip that ts-rs can't
  // mark optional. Each name is unique within the CsgOp union (verified —
  // radius/depth/width are NOT listed because they also name required fields
  // on other variants like Cylinder).
  CsgOp: ["holes", "alignment", "kind", "gap"],
  // Molecular domain: Vec+skip fields (velocities/bonds) and scalar
  // `#[serde(default)]` fields (charge/order/periodic) are optional on the
  // wire but ts-rs renders them required.
  MoleculeSystem: ["velocities", "bonds"],
  Species: ["charge"],
  Bond: ["order"],
  Cell: ["periodic"],
};

const blocks = [];
for (const { name, dir } of files) {
  const typeName = name.replace(/\.ts$/, "");
  let src = readFileSync(join(dir, name), "utf8");
  // Drop ts-rs's per-file "This file was generated by [ts-rs]" banner.
  src = src.replace(/^\/\/ This file was generated by \[ts-rs\].*\r?\n/m, "");
  // Drop cross-file imports — all types are bundled into one module.
  src = src.replace(/^import type .*;\r?\n/gm, "");
  // ts-rs renders `HashMap<K,V>` as `{ [key in string]?: V }` (optional
  // values); the hand-written contract uses `Record<string, V>` (present
  // values), which the consuming code relies on. Normalize to Record.
  src = src.replace(
    /\{ \[key in string\]\?: ([^{}]+?) \}/g,
    "Record<string, $1>",
  );
  // Force the collection fields ts-rs can't mark optional (see FORCE_OPTIONAL).
  for (const field of FORCE_OPTIONAL[typeName] ?? []) {
    const re = new RegExp(`(^|\\n)(${field}): `, "g");
    const before = src;
    src = src.replace(re, `$1$2?: `);
    if (src === before) {
      console.error(
        `[ir:gen] FORCE_OPTIONAL miss: ${typeName}.${field} not found — ` +
          `field renamed or removed in Rust? update FORCE_OPTIONAL.`,
      );
      process.exit(1);
    }
  }
  blocks.push(src.trim());
}

const banner = `// @generated by scripts/gen-ir-types.mjs from crates/vcad-ir + crates/vcad-receipt — DO NOT EDIT.
//
// Source of truth: the Rust types in crates/vcad-ir and crates/vcad-receipt
// (serde + #[derive(ts_rs::TS)]).
// Regenerate with \`npm run ir:gen\`; CI runs \`npm run ir:check\` to fail builds
// where this file has drifted from the Rust definitions.
/* eslint-disable */`;

const content = `${banner}\n\n${blocks.join("\n\n")}\n`;

if (process.argv.includes("--check")) {
  // Drift guard: assert the committed file matches a fresh generation. Fails
  // CI when the Rust IR types changed without running `npm run ir:gen`.
  const existing = existsSync(outPath) ? readFileSync(outPath, "utf8") : "";
  if (existing !== content) {
    console.error(
      `[ir:check] ${outPath} is STALE — the Rust IR types changed without ` +
        `regenerating. Run \`npm run ir:gen\` and commit the result.`,
    );
    process.exit(1);
  }
  console.log(`[ir:check] ${outPath} is up to date (${files.length} types).`);
} else {
  writeFileSync(outPath, content);
  console.log(`[ir:gen] wrote ${outPath} (${files.length} types)`);
}
