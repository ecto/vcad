/**
 * Parts library — thin TS facade over the WASM kernel's `getPartsManifest`
 * and `buildPart` exports.
 *
 * The manifest powers the palette's Parts tab, the Cmd+K part entries, and
 * the chat + MCP `search_parts` tool. The `buildPart` path is used by the
 * engine when it encounters a `PartInstance` node, and by `place_part` for
 * direct insertion.
 */

import type { Document } from "@vcad/ir";

/** One parameter declaration for a part. */
export type PartParam =
  | {
      kind: "length";
      name: string;
      min: number;
      max: number;
      default: number;
      unit: string;
    }
  | {
      kind: "number";
      name: string;
      min: number;
      max: number;
      default: number;
    }
  | {
      kind: "integer";
      name: string;
      min: number;
      max: number;
      default: number;
    }
  | {
      kind: "enum";
      name: string;
      values: string[];
      default: string;
    }
  | {
      kind: "boolean";
      name: string;
      default: boolean;
    };

/** A single catalog xref row. */
export interface PartXref {
  params: Record<string, string>;
  mcmaster?: string;
  iso?: string;
  din?: string;
}

/** One entry in the parts manifest. */
export interface PartManifestEntry {
  id: string;
  name: string;
  category: string;
  description?: string;
  version: string;
  synonyms: string[];
  params: PartParam[];
  xrefs: PartXref[];
  thumb_svg?: string;
  search_tokens: string[];
}

interface PartsKernelApi {
  getPartsManifest?: () => string;
  buildPart?: (path: string, paramsJson: string) => string;
}

let cachedManifest: PartManifestEntry[] | null = null;

/** Read the full built-in parts manifest from the WASM kernel. */
export function loadPartsManifest(
  kernel: PartsKernelApi,
): PartManifestEntry[] {
  if (cachedManifest) return cachedManifest;
  if (typeof kernel.getPartsManifest !== "function") return [];
  try {
    const json = kernel.getPartsManifest();
    const parsed = JSON.parse(json) as PartManifestEntry[];
    cachedManifest = parsed;
    return parsed;
  } catch (err) {
    console.warn("[parts] failed to load manifest:", err);
    return [];
  }
}

/** Clear the manifest cache (useful for tests / hot reload). */
export function clearPartsManifestCache(): void {
  cachedManifest = null;
}

/** Get the default parameter map for a part. */
export function defaultParamsFor(
  entry: PartManifestEntry,
): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const p of entry.params) {
    out[p.name] = p.default;
  }
  return out;
}

/**
 * Search the manifest for parts matching a free-text query.
 *
 * Tokens come from the part's name, category, synonyms, xref numbers, and
 * id. Matching is case-insensitive substring. Exact token hits rank above
 * substring hits; within a rank, category-filtered + original order.
 */
export function searchParts(
  manifest: PartManifestEntry[],
  opts: { query?: string; category?: string; limit?: number } = {},
): PartManifestEntry[] {
  const query = (opts.query ?? "").trim().toLowerCase();
  const category = opts.category?.toLowerCase();
  const limit = opts.limit ?? 10;

  let pool = manifest;
  if (category) {
    pool = pool.filter((p) => p.category.toLowerCase() === category);
  }
  if (!query) return pool.slice(0, limit);

  const scored = pool
    .map((p) => {
      const haystack = p.search_tokens.map((t) => t.toLowerCase());
      let score = 0;
      for (const tok of haystack) {
        if (tok === query) {
          score += 100;
        } else if (tok.includes(query)) {
          score += 20;
        }
      }
      // Also check the id and name for loose partial matches.
      if (p.name.toLowerCase().includes(query)) score += 10;
      if (p.id.toLowerCase().includes(query)) score += 5;
      return { entry: p, score };
    })
    .filter((x) => x.score > 0)
    .sort((a, b) => b.score - a.score);

  return scored.slice(0, limit).map((x) => x.entry);
}

/** Invoke the WASM kernel to build a part's sub-document. */
export function buildPartDocument(
  kernel: PartsKernelApi,
  path: string,
  params: Record<string, unknown>,
): Document {
  if (typeof kernel.buildPart !== "function") {
    throw new Error("kernel does not support buildPart — rebuild WASM kernel");
  }
  const json = kernel.buildPart(path, JSON.stringify(params));
  return JSON.parse(json) as Document;
}
