/**
 * `search_mechanical_parts` — spec-search the curated mechanical COTS catalog.
 *
 * The mechanical sibling of `search_electronic_parts`: bearings, precision
 * shafts, shaft collars, flange couplings, standoffs, machine screws, and
 * ferrite/ceramic magnets, searchable by type + dimension + free text. The
 * data lives in `lib/parts/mechanical.json` (same source-of-truth-in-lib/
 * pattern as the DFM rule packs) and is bundled at build time by
 * `scripts/gen-mech-catalog.mjs`.
 *
 * Honesty rule: `price_band_usd` is a rough street-price ESTIMATE, never a
 * live quote — flagged in every result, same as Phase-0 fab quotes.
 */

import { MECH_CATALOG_JSON } from "./mech-catalog.generated.js";

type ToolResult = { content: Array<{ type: "text"; text: string }>; isError?: boolean };

/** One catalog entry. `spec` is an open bag of dimension/material fields. */
export interface MechPart {
  id: string;
  type: string;
  name: string;
  spec: Record<string, unknown>;
  example_pn: string;
  synonyms: string[];
  /** [low, high] rough per-unit street price in USD — an estimate. */
  price_band_usd: [number, number];
  notes: string;
}

export const MECH_PART_TYPES = [
  "bearing",
  "shaft",
  "shaft_collar",
  "flange_coupling",
  "standoff",
  "screw",
  "magnet",
] as const;

const PRICE_DISCLAIMER =
  "price_band_usd is a rough per-unit street-price ESTIMATE (small quantities, typical online vendors) — not a live quote. Same honesty rule as Phase-0 fab quotes.";

let cachedParts: MechPart[] | null = null;

/** Parsed catalog entries (parsed once per process). */
export function mechCatalog(): MechPart[] {
  if (!cachedParts) {
    const parsed = JSON.parse(MECH_CATALOG_JSON) as { parts: MechPart[] };
    cachedParts = parsed.parts;
  }
  return cachedParts;
}

export const searchMechanicalPartsSchema = {
  type: "object" as const,
  properties: {
    query: {
      type: "string" as const,
      description:
        "Free-text query matched against name, synonyms, part number, type, and " +
        "grade (e.g. '608 bearing', 'M3 standoff', 'Y30 ring magnet').",
    },
    type: {
      type: "string" as const,
      enum: [...MECH_PART_TYPES],
      description:
        "Part type filter: bearing | shaft | shaft_collar | flange_coupling | standoff | screw | magnet.",
    },
    bore_mm: { type: "number" as const, description: "Bore / inner working diameter in mm (bearings, collars, couplings)." },
    od_mm: { type: "number" as const, description: "Outer diameter in mm." },
    id_mm: { type: "number" as const, description: "Inner diameter in mm (ring magnets)." },
    diameter_mm: {
      type: "number" as const,
      description: "Generic diameter in mm — matches shaft diameter or a disc/ring OD.",
    },
    width_mm: { type: "number" as const, description: "Width in mm (also matches magnet thickness)." },
    thickness_mm: { type: "number" as const, description: "Thickness in mm (also matches bearing/collar width)." },
    thread: { type: "string" as const, description: "Thread size, e.g. 'M2', 'M2.5', 'M3' (standoffs, screws)." },
    length_mm: {
      type: "number" as const,
      description: "Length in mm — matched against the entry's available lengths (screws, standoffs, shafts).",
    },
    tolerance_mm: {
      type: "number" as const,
      description: "Dimension match tolerance in mm (default 0.25).",
    },
    limit: { type: "integer" as const, minimum: 1, description: "Max results (default 10)." },
  },
  required: [],
};

const num = (v: unknown): number | null =>
  typeof v === "number" && Number.isFinite(v) ? v : null;

/** Does `wanted` match any of the numeric spec fields named in `keys`? */
function dimMatches(
  spec: Record<string, unknown>,
  keys: string[],
  wanted: number,
  tol: number,
): boolean {
  for (const key of keys) {
    const v = num(spec[key]);
    if (v !== null && Math.abs(v - wanted) <= tol) return true;
  }
  return false;
}

/** Does `wanted` match one of the entry's available lengths (or its length field)? */
function lengthMatches(spec: Record<string, unknown>, wanted: number, tol: number): boolean {
  const lengths = spec.lengths_mm;
  if (Array.isArray(lengths)) {
    return lengths.some((l) => num(l) !== null && Math.abs((l as number) - wanted) <= tol);
  }
  return dimMatches(spec, ["length_mm"], wanted, tol);
}

const normThread = (t: string): string => t.trim().toUpperCase().replace(/\s+/g, "");

export interface MechSearchFilters {
  query?: string;
  type?: string;
  bore_mm?: number;
  od_mm?: number;
  id_mm?: number;
  diameter_mm?: number;
  width_mm?: number;
  thickness_mm?: number;
  thread?: string;
  length_mm?: number;
  tolerance_mm?: number;
  limit?: number;
}

/** Pure search over the catalog — exported for tests and reuse. */
export function searchMechCatalog(filters: MechSearchFilters): MechPart[] {
  const tol = num(filters.tolerance_mm) ?? 0.25;
  const limit = Math.max(1, Math.round(num(filters.limit) ?? 10));
  const tokens = (filters.query ?? "")
    .toLowerCase()
    .split(/[\s,/]+/)
    .filter(Boolean);

  const scored: Array<{ part: MechPart; score: number }> = [];
  for (const part of mechCatalog()) {
    if (filters.type && part.type !== filters.type) continue;
    const spec = part.spec;

    if (num(filters.bore_mm) !== null && !dimMatches(spec, ["bore_mm"], filters.bore_mm as number, tol)) continue;
    if (num(filters.od_mm) !== null && !dimMatches(spec, ["od_mm", "flange_dia_mm"], filters.od_mm as number, tol)) continue;
    if (num(filters.id_mm) !== null && !dimMatches(spec, ["id_mm"], filters.id_mm as number, tol)) continue;
    if (
      num(filters.diameter_mm) !== null &&
      !dimMatches(spec, ["diameter_mm", "od_mm", "bore_mm"], filters.diameter_mm as number, tol)
    )
      continue;
    if (
      num(filters.width_mm) !== null &&
      !dimMatches(spec, ["width_mm", "thickness_mm"], filters.width_mm as number, tol)
    )
      continue;
    if (
      num(filters.thickness_mm) !== null &&
      !dimMatches(spec, ["thickness_mm", "width_mm"], filters.thickness_mm as number, tol)
    )
      continue;
    if (filters.thread) {
      const specThread = typeof spec.thread === "string" ? normThread(spec.thread) : "";
      if (specThread !== normThread(filters.thread)) continue;
    }
    if (num(filters.length_mm) !== null && !lengthMatches(spec, filters.length_mm as number, tol)) continue;

    // Free-text ranking over the surviving candidates. With a query, every
    // token must hit somewhere (AND semantics); name/PN hits outrank spec
    // hits, and a whole-query synonym match outranks both.
    let score = 0;
    if (tokens.length > 0) {
      const whole = tokens.join(" ");
      if (part.synonyms.some((s) => s.toLowerCase() === whole)) score += 5;
      const hay = [
        part.id,
        part.name,
        part.type,
        part.example_pn,
        ...part.synonyms,
        ...Object.values(part.spec).map((v) => String(v)),
      ]
        .join(" ")
        .toLowerCase();
      let all = true;
      for (const t of tokens) {
        if (!hay.includes(t)) {
          all = false;
          break;
        }
        score += part.name.toLowerCase().includes(t) || part.example_pn.toLowerCase().includes(t) ? 2 : 1;
      }
      if (!all) continue;
    }
    scored.push({ part, score });
  }

  scored.sort((a, b) => b.score - a.score || a.part.id.localeCompare(b.part.id));
  return scored.slice(0, limit).map((s) => s.part);
}

/** `search_mechanical_parts` MCP handler. */
export function searchMechanicalParts(input: unknown): ToolResult {
  const args = (input ?? {}) as Record<string, unknown>;
  if (args.type !== undefined && !MECH_PART_TYPES.includes(args.type as (typeof MECH_PART_TYPES)[number])) {
    return {
      content: [
        {
          type: "text",
          text: JSON.stringify({
            error: `Unknown type "${String(args.type)}". Use one of: ${MECH_PART_TYPES.join(", ")}.`,
          }),
        },
      ],
      isError: true,
    };
  }

  const results = searchMechCatalog(args as MechSearchFilters);
  return {
    content: [
      {
        type: "text",
        text: JSON.stringify(
          {
            count: results.length,
            results: results.map((p) => ({
              id: p.id,
              type: p.type,
              name: p.name,
              spec: p.spec,
              example_pn: p.example_pn,
              synonyms: p.synonyms,
              price_band_usd: { low: p.price_band_usd[0], high: p.price_band_usd[1], basis: "estimate" },
              ...(p.notes ? { notes: p.notes } : {}),
            })),
            catalog: "vcad-mech-catalog/1",
            pricing_note: PRICE_DISCLAIMER,
            ...(results.length === 0
              ? {
                  hint:
                    "No match. Try loosening filters (tolerance_mm), dropping the query, or a nearby standard size — the catalog carries common 608/625/688/600x bearings, 3-12 mm ground shafts, M2/M3 standoffs and screws, and Y30/Y35/C8 ferrite magnets.",
                }
              : {}),
          },
          null,
          2,
        ),
      },
    ],
  };
}
