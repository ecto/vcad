/**
 * Orderable-parts DB + live LCSC/JLCPCB stock & price check.
 *
 * Two layers:
 *  1. The curated **orderable DB** (`lib/parts/orderable.json`, bundled by
 *     `scripts/gen-orderable-catalog.mjs`): real MPNs with manufacturer,
 *     vcad footprint, key parameters, datasheet, verified LCSC number and a
 *     verification record. Fully offline.
 *  2. A **live client** over two public, credential-free endpoints:
 *       - JLCPCB parts-library search (keyword → LCSC#, JLC assembly stock,
 *         basic/extended library, JLC price ladder)
 *       - LCSC product detail (LCSC# → distributor stock, price ladder,
 *         lifecycle)
 *     with a TTL cache, a serializing rate limiter, per-request timeouts, and
 *     an offline fallback to the DB when the network is unavailable.
 *
 * Stock and price are never stored in the DB — they go stale in days. Every
 * live answer carries `fetched_at` and its `source` (live / cache / offline).
 */
import { ORDERABLE_CATALOG_JSON } from "./orderable-catalog.generated.js";
import { behavior, type ToolDef } from "./tool-def.js";

type ToolResult = {
  content: Array<{ type: "text"; text: string }>;
  structuredContent?: Record<string, unknown>;
  isError?: boolean;
};

// ── Orderable DB ─────────────────────────────────────────────────────────────

/** A substitute recorded against a DB part. */
export interface OrderableAlternative {
  mpn: string;
  lcsc?: string | null;
  note: string;
}

/** One orderable part as authored in lib/parts/orderable.json. */
export interface OrderablePart {
  id: string;
  mpn: string | null;
  manufacturer: string;
  category: string;
  description: string;
  footprint: string;
  pins: number;
  /** Distinct electrical pad numbers on the land pattern, when the vendor
   *  footprint numbers by terminal (e.g. MOSFET G/S/D) rather than by lead. */
  pads?: number;
  /** `generated` = vcad derives a real land pattern; `placeholder` = not yet. */
  footprint_status: "generated" | "placeholder";
  params: Record<string, unknown>;
  datasheet_url: string | null;
  lcsc: string | null;
  jlc_library: "basic" | "extended" | null;
  aliases?: string[];
  bom_mpn?: string;
  bom_lcsc?: string;
  verification: { status: "verified" | "corrected" | "flagged"; source: string; note?: string };
  alternatives?: OrderableAlternative[];
}

interface OrderableDb {
  schema: string;
  checked: string;
  parts: OrderablePart[];
}

let dbCache: OrderableDb | null = null;
function db(): OrderableDb {
  if (!dbCache) dbCache = JSON.parse(ORDERABLE_CATALOG_JSON) as OrderableDb;
  return dbCache;
}

/** Every part in the orderable DB. */
export function orderableParts(): OrderablePart[] {
  return db().parts;
}

/** Date the DB's MPN/LCSC identities were last verified. */
export function orderableCheckedDate(): string {
  return db().checked;
}

/** Normalize an MPN for matching: uppercase, drop whitespace/dashes/slashes/parens. */
export function normMpn(s: string): string {
  return s.toUpperCase().replace(/[\s\-_/().,+=]/g, "");
}

const LCSC_RE = /^C\d{2,}$/i;

/**
 * Exact lookup by MPN, alias, the BOM's original (pre-correction) MPN, LCSC#,
 * or the BOM's original LCSC#. Also tolerates a manufacturer prefix
 * ("Molex 43045-0412") by trying the last whitespace token.
 */
export function lookupOrderable(query: string): OrderablePart | null {
  const q = query.trim();
  if (!q) return null;
  const parts = orderableParts();
  if (LCSC_RE.test(q)) {
    const c = q.toUpperCase();
    return parts.find((p) => p.lcsc === c || p.bom_lcsc === c) ?? null;
  }
  const keys = [normMpn(q)];
  const lastTok = q.split(/\s+/).pop();
  if (lastTok && lastTok !== q) keys.push(normMpn(lastTok));
  // "XT30PB(2+2)-M (AMASS, right angle)" → strip a trailing parenthetical note.
  const noNote = q.replace(/\s+\([^)]*\)\s*$/, "");
  if (noNote !== q) keys.push(normMpn(noNote));
  for (const key of keys) {
    const hit = parts.find((p) => {
      const names = [p.mpn, ...(p.aliases ?? []), p.bom_mpn].filter(Boolean) as string[];
      return names.some((n) => normMpn(n) === key);
    });
    if (hit) return hit;
  }
  return null;
}

/** Token search over MPN, aliases, manufacturer, category, description. */
export function searchOrderable(query: string, limit = 5): OrderablePart[] {
  const exact = lookupOrderable(query);
  const toks = query.toLowerCase().split(/[\s,]+/).filter((t) => t.length > 1);
  if (toks.length === 0) return exact ? [exact] : [];
  const scored = orderableParts()
    .map((p) => {
      const hay = [p.mpn ?? "", ...(p.aliases ?? []), p.manufacturer, p.category, p.description, p.footprint]
        .join(" ")
        .toLowerCase();
      const score = toks.reduce((s, t) => s + (hay.includes(t) ? 1 : 0), 0);
      return { p, score: p === exact ? 1e6 : score };
    })
    .filter((x) => x.score > 0)
    .sort((a, b) => b.score - a.score);
  return scored.slice(0, limit).map((x) => x.p);
}

// ── Live client ──────────────────────────────────────────────────────────────

/** Minimal fetch surface (global fetch satisfies it; tests inject fixtures). */
export type FetchLike = (
  url: string,
  init?: { method?: string; headers?: Record<string, string>; body?: string; signal?: AbortSignal },
) => Promise<{ ok: boolean; status: number; json(): Promise<unknown> }>;

/** One price break: unit price (USD) from `qty` upward. */
export interface PriceBreak {
  qty: number;
  unit_usd: number;
}

/** A live (or fallback) stock + price answer for one part. */
export interface StockQuote {
  query: string;
  found: boolean;
  lcsc: string | null;
  mpn: string | null;
  manufacturer: string | null;
  package: string | null;
  description: string | null;
  /** LCSC distributor stock (null when LCSC detail was unavailable). */
  stock_lcsc: number | null;
  /** JLCPCB assembly-library stock (null when JLC search was unavailable). */
  stock_jlc: number | null;
  /** JLC assembly library tier: basic parts carry no per-part setup fee. */
  jlc_library: "basic" | "extended" | null;
  /** LCSC minimum order quantity. */
  min_order_qty: number | null;
  /** LCSC distributor price ladder (USD) — buying loose parts. */
  price_breaks: PriceBreak[];
  /** JLCPCB assembly price ladder (USD) — parts placed by JLC, no LCSC MOQ. */
  jlc_price_breaks: PriceBreak[];
  /** LCSC `productCycle` ("normal", "sold_out", or a discontinued/NRND marker). */
  lifecycle: string | null;
  eol: boolean;
  source: "live" | "cache" | "offline";
  fetched_at: string | null;
  /** The DB record, when the query maps to one. */
  db: Pick<OrderablePart, "id" | "mpn" | "footprint" | "footprint_status" | "verification" | "alternatives"> | null;
  warnings: string[];
}

export interface StockClientOptions {
  fetch?: FetchLike;
  now?: () => number;
  /** Min spacing between outbound requests (ms). Default 400. */
  minIntervalMs?: number;
  /** Per-request timeout (ms). Default 10000. */
  timeoutMs?: number;
  /** Cache TTL (ms). Default 15 min. */
  ttlMs?: number;
  /** Never touch the network; answer from the DB only. */
  offline?: boolean;
  /** Injectable sleep (tests). */
  sleep?: (ms: number) => Promise<void>;
}

const JLC_SEARCH_URL =
  "https://jlcpcb.com/api/overseas-pcb-order/v1/shoppingCart/smtGood/selectSmtComponentList";
const LCSC_DETAIL_URL = "https://wmsc.lcsc.com/ftps/wm/product/detail?productCode=";
const UA = "vcad-mcp parts-stock (+https://vcad.io)";

interface JlcComponent {
  componentCode: string;
  componentModelEn: string;
  componentBrandEn: string;
  componentSpecificationEn?: string;
  describe?: string;
  stockCount: number;
  componentLibraryType: string;
  componentPrices?: Array<{ startNumber: number; productPrice: number }>;
}

interface LcscDetail {
  productCode: string;
  productModel: string;
  brandNameEn: string;
  encapStandard?: string;
  productIntroEn?: string;
  stockNumber: number;
  minBuyNumber?: number;
  productCycle?: string;
  productPriceList?: Array<{ ladder: number; usdPrice: number }>;
}

/** Is a JLC row a consignment placeholder rather than a real stocked part? */
function isPlaceholderRow(c: JlcComponent): boolean {
  return /^C99\d{8,}$/.test(c.componentCode) || c.componentBrandEn === "JLCPCB Assembly";
}

/** Unit price at `qty` from a ladder (the highest break ≤ qty; else the first). */
export function unitPriceAt(breaks: PriceBreak[], qty: number): number | null {
  if (breaks.length === 0) return null;
  const sorted = [...breaks].sort((a, b) => a.qty - b.qty);
  let price = sorted[0].unit_usd;
  for (const b of sorted) if (b.qty <= qty) price = b.unit_usd;
  return price;
}

/**
 * Turn a passive spec line ("RES 10k 0603", "CAP 2.2uF 100V X7S 1210") into a
 * JLC keyword and a value token that a matching row's description must carry.
 */
export function passiveKeyword(spec: string): { keyword: string; mustContain: string[] } | null {
  const m = spec.trim().match(/^(RES|CAP|IND|LED)\s+(.+)$/i);
  if (!m) return null;
  const kind = m[1].toUpperCase();
  const rest = m[2].trim();
  const toks = rest.split(/\s+/);
  const pkg = toks.find((t) => /^(0201|0402|0603|0805|1206|1210|1812|2010|2512)$/.test(t)) ?? "";
  if (kind === "RES") {
    let v = toks[0];
    if (/^\d+(\.\d+)?$/.test(v)) v = `${v}Ω`;
    else if (/^\d+(\.\d+)?[kKmM]$/.test(v)) v = `${v.slice(0, -1)}${v.slice(-1) === "k" || v.slice(-1) === "K" ? "k" : "M"}Ω`;
    return { keyword: `${v} ${pkg}`.trim(), mustContain: [v] };
  }
  if (kind === "CAP") {
    const v = toks[0];
    const volt = toks.find((t) => /^\d+(\.\d+)?V$/i.test(t));
    const kw = [v, volt, pkg].filter(Boolean).join(" ");
    return { keyword: kw, mustContain: [v, ...(volt ? [volt.toUpperCase()] : [])] };
  }
  if (kind === "LED") {
    const color = (toks.find((t) => /green|red|blue|yellow|white|orange/i.test(t)) ?? "").toLowerCase();
    // JLC's free-text search for "green LED" is swamped by consignment
    // placeholders; anchor common 0603 colours on known stocked parts.
    const KNOWN: Record<string, string> = { green: "KT-0603G", red: "KT-0603R" };
    if (pkg === "0603" && KNOWN[color]) return { keyword: KNOWN[color], mustContain: [] };
    return { keyword: `${color} LED ${pkg}`.trim(), mustContain: [] };
  }
  return { keyword: rest, mustContain: [] };
}

/** Rows' describe text uses "Ω", "uF", "nF", "pF", "V"; compare case-insensitively. */
function describeHas(c: JlcComponent, tok: string): boolean {
  const hay = `${c.describe ?? ""} ${c.componentSpecificationEn ?? ""}`.toLowerCase().replace(/µ/g, "u");
  const t = tok.toLowerCase().replace(/µ/g, "u");
  // Token boundary: the char before must not be a digit or '.', so "10kΩ"
  // doesn't match inside "110kΩ".
  const i = hay.indexOf(t);
  if (i < 0) return false;
  const before = i === 0 ? " " : hay[i - 1];
  return !/[\d.]/.test(before);
}

/** Live stock/price client: cache + rate limit + timeout + offline fallback. */
export class PartStockClient {
  private readonly fetchImpl: FetchLike | null;
  private readonly now: () => number;
  private readonly minIntervalMs: number;
  private readonly timeoutMs: number;
  private readonly ttlMs: number;
  private readonly offline: boolean;
  private readonly sleep: (ms: number) => Promise<void>;
  private readonly cache = new Map<string, { at: number; value: unknown }>();
  private queue: Promise<void> = Promise.resolve();
  private lastRequestAt = -Infinity;
  /** Outbound request count (observability + tests). */
  requests = 0;

  constructor(opts: StockClientOptions = {}) {
    const g = (globalThis as { fetch?: FetchLike }).fetch;
    this.fetchImpl = opts.fetch ?? (g ? g.bind(globalThis) : null);
    this.now = opts.now ?? Date.now;
    this.minIntervalMs = opts.minIntervalMs ?? 400;
    this.timeoutMs = opts.timeoutMs ?? 10_000;
    this.ttlMs = opts.ttlMs ?? 15 * 60_000;
    this.offline = opts.offline ?? false;
    this.sleep = opts.sleep ?? ((ms) => new Promise((r) => setTimeout(r, ms)));
  }

  /** Serialize + space requests; returns parsed JSON or throws. */
  private async request(url: string, init?: Parameters<FetchLike>[1]): Promise<unknown> {
    if (!this.fetchImpl || this.offline) throw new Error("offline");
    const run = async (): Promise<unknown> => {
      const wait = this.lastRequestAt + this.minIntervalMs - this.now();
      if (wait > 0) await this.sleep(wait);
      this.lastRequestAt = this.now();
      this.requests++;
      const ctl = typeof AbortController !== "undefined" ? new AbortController() : null;
      const timer = setTimeout(() => ctl?.abort(), this.timeoutMs);
      try {
        const timeout = new Promise<never>((_, rej) =>
          setTimeout(() => rej(new Error(`timeout after ${this.timeoutMs} ms`)), this.timeoutMs),
        );
        const res = await Promise.race([
          this.fetchImpl!(url, { ...init, signal: ctl?.signal }),
          timeout,
        ]);
        if (!res.ok) throw new Error(`HTTP ${res.status}`);
        return await res.json();
      } finally {
        clearTimeout(timer);
      }
    };
    const p = this.queue.then(run, run);
    this.queue = p.then(
      () => undefined,
      () => undefined,
    );
    return p;
  }

  private async cached<T>(key: string, fn: () => Promise<T>): Promise<{ value: T; hit: boolean }> {
    const c = this.cache.get(key);
    if (c && this.now() - c.at < this.ttlMs) return { value: c.value as T, hit: true };
    const value = await fn();
    this.cache.set(key, { at: this.now(), value });
    return { value, hit: false };
  }

  /** JLCPCB parts-library keyword search. */
  async jlcSearch(keyword: string): Promise<{ rows: JlcComponent[]; hit: boolean }> {
    const { value, hit } = await this.cached(`jlc:${keyword}`, async () => {
      const r = (await this.request(JLC_SEARCH_URL, {
        method: "POST",
        headers: { "Content-Type": "application/json", "User-Agent": UA },
        body: JSON.stringify({ keyword, currentPage: 1, pageSize: 10 }),
      })) as { code?: number; data?: { componentPageInfo?: { list?: JlcComponent[] } } };
      if (r.code !== 200) throw new Error(`JLC search code ${r.code}`);
      return r.data?.componentPageInfo?.list ?? [];
    });
    return { rows: value, hit };
  }

  /** LCSC product detail by LCSC#. Null when LCSC has no such product. */
  async lcscDetail(code: string): Promise<{ detail: LcscDetail | null; hit: boolean }> {
    const { value, hit } = await this.cached(`lcsc:${code}`, async () => {
      const r = (await this.request(LCSC_DETAIL_URL + encodeURIComponent(code), {
        headers: { "User-Agent": UA },
      })) as { code?: number; result?: LcscDetail | null };
      if (r.code !== 200) throw new Error(`LCSC detail code ${r.code}`);
      return r.result && r.result.productCode ? r.result : null;
    });
    return { detail: value, hit };
  }

  /**
   * Check one part. `query` is an LCSC#, an MPN (DB-known or not), or a
   * passive spec line ("RES 10k 0603"). `need` (default 1) picks the best
   * stocked candidate for spec queries.
   */
  async check(query: string, need = 1): Promise<StockQuote> {
    const q = query.trim();
    const dbPart = lookupOrderable(q);
    const base: StockQuote = {
      query: q,
      found: false,
      lcsc: dbPart?.lcsc ?? (LCSC_RE.test(q) ? q.toUpperCase() : null),
      mpn: dbPart?.mpn ?? null,
      manufacturer: dbPart?.manufacturer ?? null,
      package: dbPart?.footprint ?? null,
      description: dbPart?.description ?? null,
      stock_lcsc: null,
      stock_jlc: null,
      jlc_library: dbPart?.jlc_library ?? null,
      min_order_qty: null,
      price_breaks: [],
      jlc_price_breaks: [],
      lifecycle: null,
      eol: false,
      source: "live",
      fetched_at: null,
      db: dbPart
        ? {
            id: dbPart.id,
            mpn: dbPart.mpn,
            footprint: dbPart.footprint,
            footprint_status: dbPart.footprint_status,
            verification: dbPart.verification,
            alternatives: dbPart.alternatives,
          }
        : null,
      warnings: [],
    };
    if (dbPart?.verification.status === "flagged") {
      base.warnings.push(`DB flags this part: ${dbPart.verification.note ?? "unverified"}`);
    }
    if (dbPart?.verification.status === "corrected" && dbPart.bom_mpn && normMpn(dbPart.bom_mpn) === normMpn(q)) {
      base.warnings.push(`'${q}' was corrected to '${dbPart.mpn}': ${dbPart.verification.note ?? ""}`.trim());
    }
    if (dbPart?.bom_lcsc && dbPart.bom_lcsc === q.toUpperCase()) {
      base.warnings.push(`LCSC ${q} is NOT this part's number; the correct LCSC# is ${dbPart.lcsc}. ${dbPart.verification.note ?? ""}`.trim());
    }

    let anyHit = false;
    let anyMiss = false;
    try {
      // 1. Find the JLC row: by LCSC# when known, else by MPN / spec keyword.
      let row: JlcComponent | undefined;
      const spec = !dbPart && !LCSC_RE.test(q) ? passiveKeyword(q) : null;
      const keyword = base.lcsc ?? spec?.keyword ?? dbPart?.mpn ?? q;
      if (dbPart && !dbPart.mpn && !base.lcsc) {
        base.warnings.push("DB entry is a part class, not an orderable MPN — query a concrete value");
        return this.offlineFallback(base, "class entry");
      }
      const s = await this.jlcSearch(keyword);
      s.hit ? (anyHit = true) : (anyMiss = true);
      const rows = s.rows.filter((r) => !isPlaceholderRow(r));
      if (base.lcsc) {
        row = rows.find((r) => r.componentCode === base.lcsc) ?? s.rows.find((r) => r.componentCode === base.lcsc);
      } else if (spec) {
        const ok = rows.filter((r) => spec.mustContain.every((t) => describeHas(r, t)));
        const pkgTok = q.match(/\b(0201|0402|0603|0805|1206|1210|1812|2010|2512)\b/)?.[1];
        const pkgOk = pkgTok ? ok.filter((r) => (r.componentSpecificationEn ?? "").includes(pkgTok)) : ok;
        const rank = (r: JlcComponent) =>
          (r.stockCount >= need ? 0 : 2) + (r.componentLibraryType === "base" ? 0 : 1);
        row = [...pkgOk].sort((a, b) => rank(a) - rank(b) || b.stockCount - a.stockCount)[0];
        if (row) base.warnings.push(`spec match: picked ${row.componentCode} ${row.componentModelEn} for '${q}'`);
      } else {
        const key = normMpn(dbPart?.mpn ?? q);
        const exact = rows.filter((r) => normMpn(r.componentModelEn) === key);
        // Prefer an exact model in stock, then any exact, then a prefix match.
        row =
          exact.find((r) => r.stockCount > 0) ??
          exact[0] ??
          rows.find((r) => normMpn(r.componentModelEn).startsWith(key));
      }
      if (row) {
        base.found = true;
        base.lcsc = row.componentCode;
        base.mpn = base.mpn ?? row.componentModelEn;
        base.manufacturer = base.manufacturer ?? row.componentBrandEn;
        base.package = base.package ?? row.componentSpecificationEn ?? null;
        base.description = base.description ?? row.describe ?? null;
        base.stock_jlc = row.stockCount;
        base.jlc_library = row.componentLibraryType === "base" ? "basic" : "extended";
        base.jlc_price_breaks = (row.componentPrices ?? []).map((p) => ({ qty: p.startNumber, unit_usd: p.productPrice }));
        if (isPlaceholderRow(row)) base.warnings.push(`${row.componentCode} is a JLC consignment placeholder, not stocked`);
      }
      // 2. LCSC distributor detail.
      if (base.lcsc) {
        const d = await this.lcscDetail(base.lcsc);
        d.hit ? (anyHit = true) : (anyMiss = true);
        if (d.detail) {
          const det = d.detail;
          base.found = true;
          if (base.mpn && normMpn(det.productModel) !== normMpn(base.mpn) && !dbPart?.aliases?.some((a) => normMpn(a) === normMpn(det.productModel))) {
            base.warnings.push(`LCSC ${base.lcsc} is '${det.productModel}' (${det.brandNameEn}), not '${base.mpn}'`);
          }
          base.mpn = base.mpn ?? det.productModel;
          base.manufacturer = base.manufacturer ?? det.brandNameEn;
          base.stock_lcsc = det.stockNumber;
          base.min_order_qty = det.minBuyNumber ?? null;
          base.lifecycle = det.productCycle ?? null;
          // "normal" / "sold_out" are stock states; only end-of-life markers count.
          base.eol = /discontinu|obsolete|eol|nrnd|not.?recommend|end.?of.?life/i.test(det.productCycle ?? "");
          const ladder = (det.productPriceList ?? []).map((p) => ({ qty: p.ladder, unit_usd: p.usdPrice }));
          base.price_breaks = ladder;
        }
      }
      if (!base.found) base.warnings.push(`no LCSC/JLCPCB listing found for '${q}'`);
      base.source = anyHit && !anyMiss ? "cache" : "live";
      base.fetched_at = new Date(this.now()).toISOString();
      return base;
    } catch (e) {
      return this.offlineFallback(base, (e as Error).message);
    }
  }

  private offlineFallback(base: StockQuote, why: string): StockQuote {
    base.source = "offline";
    base.found = base.db !== null && base.lcsc !== null;
    base.warnings.push(
      `live check unavailable (${why}); identity from the offline DB (verified ${orderableCheckedDate()}), no stock/price`,
    );
    return base;
  }
}

// ── BOM annotation ───────────────────────────────────────────────────────────

/** A BOM line to annotate. `part` is an MPN, LCSC#, or passive spec line. */
export interface StockBomLine {
  name: string;
  part: string;
  qty: number;
  /** An LCSC# noted on the line (e.g. "LCSC C730194 (unverified)") — checked too. */
  noted_lcsc?: string | null;
}

/** Pricing basis: buying loose parts from LCSC, or parts placed by JLCPCB assembly. */
export type CostBasis = "lcsc" | "jlc_assembly";
const BASES: CostBasis[] = ["lcsc", "jlc_assembly"];

export type LineFlag =
  | "missing"
  | "out_of_stock"
  | "low_stock"
  | "eol"
  | "db_flagged"
  | "db_corrected"
  | "lcsc_mismatch"
  | "footprint_placeholder"
  | "unpriced";

export interface AnnotatedLine extends StockBomLine {
  quote: StockQuote;
  /** Extended cost (USD) per basis per build quantity; null when unpriced. */
  cost: Record<CostBasis, Record<string, number | null>>;
  flags: LineFlag[];
  alternatives: OrderableAlternative[];
}

export interface BomStockReport {
  builds: number[];
  lines: AnnotatedLine[];
  /** totals[basis][build] — per-board-set cost of all priced lines. */
  totals: Record<CostBasis, Record<string, { usd: number; unpriced_lines: number }>>;
  flagged: Array<{ name: string; part: string; flags: string[]; note: string }>;
  checked_at: string;
}

/**
 * Cost of `qtyTotal` pieces. LCSC honours its minimum order quantity; JLC
 * assembly pricing has none. Each basis falls back to the other ladder when
 * its own is empty.
 */
export function extCost(q: StockQuote, qtyTotal: number, basis: CostBasis): number | null {
  const own = basis === "lcsc" ? q.price_breaks : q.jlc_price_breaks;
  const ladder = own.length ? own : basis === "lcsc" ? q.jlc_price_breaks : q.price_breaks;
  const buy = basis === "lcsc" && own.length ? Math.max(qtyTotal, q.min_order_qty ?? 1) : qtyTotal;
  const unit = unitPriceAt(ladder, buy);
  return unit === null ? null : Math.round(unit * buy * 10000) / 10000;
}

/** Check every line, price it at each build quantity, and flag problems. */
export async function annotateBomStock(
  client: PartStockClient,
  lines: StockBomLine[],
  builds: number[] = [5, 100],
): Promise<BomStockReport> {
  const out: AnnotatedLine[] = [];
  for (const line of lines) {
    const need = line.qty * Math.max(...builds);
    const quote = await client.check(line.part, need);
    const flags: LineFlag[] = [];
    if (!quote.found) flags.push("missing");
    const stock = Math.max(quote.stock_lcsc ?? -1, quote.stock_jlc ?? -1);
    if (quote.found && quote.source !== "offline") {
      if (stock <= 0) flags.push("out_of_stock");
      else if (stock < need) flags.push("low_stock");
    }
    if (quote.eol) flags.push("eol");
    if (quote.db?.verification.status === "flagged") flags.push("db_flagged");
    if (quote.db?.verification.status === "corrected") flags.push("db_corrected");
    if (quote.db?.footprint_status === "placeholder") flags.push("footprint_placeholder");
    if (line.noted_lcsc && quote.lcsc && line.noted_lcsc.toUpperCase() !== quote.lcsc) {
      flags.push("lcsc_mismatch");
      quote.warnings.push(`line notes LCSC ${line.noted_lcsc}, actual part is ${quote.lcsc}`);
    }
    const cost = { lcsc: {}, jlc_assembly: {} } as AnnotatedLine["cost"];
    for (const basis of BASES) {
      for (const b of builds) cost[basis][String(b)] = extCost(quote, line.qty * b, basis);
    }
    if (quote.price_breaks.length === 0 && quote.jlc_price_breaks.length === 0) flags.push("unpriced");
    out.push({ ...line, quote, cost, flags, alternatives: quote.db?.alternatives ?? [] });
  }
  const totals = { lcsc: {}, jlc_assembly: {} } as BomStockReport["totals"];
  for (const basis of BASES) {
    for (const b of builds) {
      let usd = 0;
      let unpriced = 0;
      for (const l of out) {
        const c = l.cost[basis][String(b)];
        if (c === null) unpriced++;
        else usd += c;
      }
      totals[basis][String(b)] = { usd: Math.round(usd * 100) / 100, unpriced_lines: unpriced };
    }
  }
  const SERIOUS = new Set<LineFlag>(["missing", "out_of_stock", "low_stock", "eol", "db_flagged", "db_corrected", "lcsc_mismatch"]);
  const flagged = out
    .filter((l) => l.flags.some((f) => SERIOUS.has(f)))
    .map((l) => ({
      name: l.name,
      part: l.part,
      flags: l.flags,
      note: [l.quote.db?.verification.note, ...l.quote.warnings].filter(Boolean).join(" | "),
    }));
  return { builds, lines: out, totals, flagged, checked_at: new Date().toISOString() };
}


/** Minimal RFC-4180 CSV row splitter (quoted fields, doubled quotes). */
export function parseCsv(text: string): string[][] {
  const rows: string[][] = [];
  let row: string[] = [];
  let field = "";
  let inQ = false;
  for (let i = 0; i < text.length; i++) {
    const ch = text[i];
    if (inQ) {
      if (ch === '"' && text[i + 1] === '"') {
        field += '"';
        i++;
      } else if (ch === '"') inQ = false;
      else field += ch;
    } else if (ch === '"') inQ = true;
    else if (ch === ",") {
      row.push(field);
      field = "";
    } else if (ch === "\n" || ch === "\r") {
      if (ch === "\r" && text[i + 1] === "\n") i++;
      row.push(field);
      rows.push(row);
      row = [];
      field = "";
    } else field += ch;
  }
  if (field !== "" || row.length) {
    row.push(field);
    rows.push(row);
  }
  return rows.filter((r) => r.some((c) => c.trim() !== ""));
}

/**
 * BOM CSV (bom_export csv shape: kind,name,…,example_pn,…,qty,…,notes) →
 * stock lines. Manufactured lines and footprint-only lines are skipped.
 */
export function bomCsvToStockLines(csv: string): StockBomLine[] {
  const rows = parseCsv(csv);
  if (rows.length < 2) return [];
  const hdr = rows[0].map((h) => h.trim().toLowerCase());
  const col = (n: string) => hdr.indexOf(n);
  const iKind = col("kind");
  const iName = col("name");
  const iPn = col("example_pn") >= 0 ? col("example_pn") : col("mpn");
  const iQty = col("qty");
  const iNotes = col("notes");
  const lines: StockBomLine[] = [];
  for (const r of rows.slice(1)) {
    if (iKind >= 0 && r[iKind]?.trim() === "manufactured") continue;
    const pn = (r[iPn] ?? "").trim();
    if (!pn || /footprint only/i.test(pn)) continue;
    const noted = (r[iNotes] ?? "").match(/\bLCSC\s+(C\d+)/i)?.[1] ?? null;
    lines.push({
      name: (r[iName] ?? pn).trim(),
      part: pn,
      qty: Number(r[iQty]) || 1,
      noted_lcsc: noted,
    });
  }
  return lines;
}

// ── MCP tools ────────────────────────────────────────────────────────────────

let sharedClient: PartStockClient | null = null;
/** The process-wide client (shared cache + rate limiter). */
export function stockClient(): PartStockClient {
  if (!sharedClient) sharedClient = new PartStockClient({ offline: process.env.VCAD_PARTS_OFFLINE === "1" });
  return sharedClient;
}
/** Test hook: replace the shared client. */
export function setStockClient(c: PartStockClient | null): void {
  sharedClient = c;
}

const STOCK_NOTE =
  "Stock and prices are a live snapshot from public LCSC/JLCPCB listings (USD, excl. shipping/tax/JLC setup fees) — re-check before ordering.";

export const checkPartStockSchema = {
  type: "object",
  properties: {
    query: { type: "string", description: "MPN (e.g. 'TPS48110AQDGXRQ1'), LCSC# (e.g. 'C7519'), or a passive spec line ('RES 10k 0603', 'CAP 100nF 50V 0603')." },
    queries: { type: "array", items: { type: "string" }, description: "Several queries at once (rate-limited, cached)." },
    qty: { type: "integer", description: "Quantity needed — spec queries prefer a candidate with at least this much stock. Default 1." },
  },
} as const;

export const bomCheckStockSchema = {
  type: "object",
  properties: {
    bom_id: { type: "string", description: "A BOM from bom_create (its COTS lines are checked)." },
    csv: { type: "string", description: "BOM CSV text in the bom_export csv shape (kind,name,…,example_pn,…,qty,…,notes)." },
    lines: {
      type: "array",
      items: {
        type: "object",
        properties: { name: { type: "string" }, part: { type: "string" }, qty: { type: "number" }, noted_lcsc: { type: "string" } },
        required: ["part", "qty"],
      },
      description: "Inline lines: part = MPN / LCSC# / passive spec, qty per board.",
    },
    builds: { type: "array", items: { type: "integer" }, description: "Board build quantities to cost (default [5, 100])." },
  },
} as const;

/** `check_part_stock` handler. */
export async function checkPartStock(args: Record<string, unknown>): Promise<ToolResult> {
  const qs = Array.isArray(args.queries)
    ? (args.queries as unknown[]).map(String)
    : typeof args.query === "string"
      ? [args.query]
      : [];
  if (qs.length === 0) {
    return { content: [{ type: "text", text: JSON.stringify({ error: "pass `query` or `queries`" }) }], isError: true };
  }
  const need = typeof args.qty === "number" ? Math.max(1, Math.round(args.qty)) : 1;
  const client = stockClient();
  const results: StockQuote[] = [];
  for (const q of qs) results.push(await client.check(q, need));
  const payload = { count: results.length, results, note: STOCK_NOTE };
  return { content: [{ type: "text", text: JSON.stringify(payload) }], structuredContent: payload };
}

/** Resolver for bom_id → lines, injected by bom.ts to avoid an import cycle. */
export type BomLinesResolver = (bomId: string, ctx: unknown) => StockBomLine[] | null;
let bomResolver: BomLinesResolver | null = null;
export function setBomLinesResolver(r: BomLinesResolver): void {
  bomResolver = r;
}

/** `bom_check_stock` handler. */
export async function bomCheckStock(args: Record<string, unknown>, ctx?: unknown): Promise<ToolResult> {
  let lines: StockBomLine[] | null = null;
  if (typeof args.csv === "string") lines = bomCsvToStockLines(args.csv);
  else if (Array.isArray(args.lines)) {
    lines = (args.lines as Array<Record<string, unknown>>).map((l) => ({
      name: String(l.name ?? l.part),
      part: String(l.part),
      qty: Number(l.qty) || 1,
      noted_lcsc: typeof l.noted_lcsc === "string" ? l.noted_lcsc : null,
    }));
  } else if (typeof args.bom_id === "string") {
    lines = bomResolver ? bomResolver(args.bom_id, ctx) : null;
    if (!lines) {
      return { content: [{ type: "text", text: JSON.stringify({ error: `Unknown bom_id "${args.bom_id}"` }) }], isError: true };
    }
  }
  if (!lines || lines.length === 0) {
    return { content: [{ type: "text", text: JSON.stringify({ error: "pass bom_id, csv, or lines (no checkable lines found)" }) }], isError: true };
  }
  const builds =
    Array.isArray(args.builds) && args.builds.length
      ? (args.builds as unknown[]).map((n) => Math.max(1, Math.round(Number(n)))).filter(Boolean)
      : [5, 100];
  const report = await annotateBomStock(stockClient(), lines, builds);
  const payload = { ...report, note: STOCK_NOTE } as unknown as Record<string, unknown>;
  return { content: [{ type: "text", text: JSON.stringify(payload) }], structuredContent: payload };
}

export const toolDefs: ToolDef[] = [
  {
    name: "check_part_stock",
    pack: "bom",
    description:
      "Live stock + price check for electronic parts against public LCSC and JLCPCB listings (no credentials). Accepts an MPN, an LCSC#, or a passive spec line ('RES 10k 0603'). Returns LCSC and JLC-assembly stock, the USD price ladder, the JLC basic/extended library tier, lifecycle/EOL, and, when the part is in vcad's curated orderable DB, its verified LCSC#, footprint, verification record, and alternatives. Cached (15 min), rate-limited, and with timeouts. When offline it falls back to the DB's identity data and reports no stock.",
    inputSchema: checkPartStockSchema as unknown as Record<string, unknown>,
    handler: (a) => checkPartStock(a),
    behavior: behavior({}),
  },
  {
    name: "bom_check_stock",
    pack: "bom",
    description:
      "Annotate a BOM with live LCSC/JLCPCB stock and price. Takes a bom_id, BOM CSV text, or inline lines (MPN / LCSC# / passive spec + qty per board). Returns per-line stock, price ladder, and extended cost at each build quantity (default 5 and 100 boards, honouring LCSC minimum order), totals, and flags: missing, out_of_stock, low_stock, eol, db_flagged/db_corrected (the curated DB found the MPN wrong or unorderable), lcsc_mismatch (the line's noted LCSC# is a different part), footprint_placeholder. Flagged lines include alternatives.",
    inputSchema: bomCheckStockSchema as unknown as Record<string, unknown>,
    handler: (a, c) => bomCheckStock(a, c),
    behavior: behavior({}),
  },
];
