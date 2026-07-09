/**
 * kerf `intentHash` — reproduced EXACTLY from kerf packages/engine/src/hash.ts
 * (keep byte-identical; drift is checked by kerf-contract.test).
 *
 * The hash is the identity a VendorQuote and a spend mandate bind to: the same
 * design + config + quantity at the same vendor always collides to the same
 * hash, and any change to what would actually be manufactured produces a new
 * one (quote is dead ⇒ re-quote — never silent re-pricing).
 */

import { createHash } from "node:crypto";
import type { ConfiguratorIntent } from "./contract.js";

/** Recursively sort object keys (default String sort); arrays keep order. */
function sortKeys(v: unknown): unknown {
  if (Array.isArray(v)) return v.map(sortKeys);
  if (v !== null && typeof v === "object") {
    const rec = v as Record<string, unknown>;
    const out: Record<string, unknown> = {};
    for (const k of Object.keys(rec).sort()) out[k] = sortKeys(rec[k]);
    return out;
  }
  return v;
}

/** Canonical JSON: recursively key-sorted objects, arrays in order. */
export function canonicalJson(v: unknown): string {
  return JSON.stringify(sortKeys(v));
}

/** Lowercase-hex SHA-256 over the UTF-8 canonical JSON of `v`. */
export function sha256Hex(v: unknown): string {
  return createHash("sha256").update(canonicalJson(v), "utf8").digest("hex");
}

/**
 * Hash a ConfiguratorIntent the way kerf does.
 *
 * Deliberately EXCLUDES `idempotency_key`, `ship_to`, `budget_cap`,
 * `deadline`, `kind`, and file names/sizes — a re-quote of the same design
 * collides intentionally; only what would actually be manufactured (vendor,
 * process, exact file bytes via sha256, vendor-native config, quantity)
 * participates. After canonicalization the hashed JSON has top-level key
 * order `config, files, process, quantity, vendor`.
 */
export function intentHash(intent: ConfiguratorIntent): string {
  return sha256Hex({
    vendor: intent.vendor,
    process: intent.process ?? null,
    files: (intent.files ?? []).map((f) => f.sha256),
    config: intent.config ?? {},
    quantity: intent.quantity ?? null,
  });
}
