/**
 * vcad Fabricate — durable store for quotes + orders.
 *
 * Mirrors session-store.ts: an in-memory impl (local stdio / anonymous — the
 * source of truth in-process, perfect for running the loop locally) and a
 * cloud impl over raw PostgREST with the service-role key (hosted + signed-in
 * user). RLS is bypassed by the service role, so ownership is enforced in code:
 * every query filters user_id and every write sets it to the caller.
 */

import type { AuthUser } from "../oauth.js";
import type {
  AuthorizationStatus,
  DebitResult,
  Order,
  OrderReceiptStatus,
  OrderState,
  Quote,
  QuoteEconomics,
  SpendAuthorization,
} from "./types.js";

export interface OrderFilter {
  status?: OrderState;
  limit?: number;
}

/** Parameters for an atomic wallet debit (forwarded to the debit_wallet RPC). */
export interface DebitParams {
  userId: string;
  amountMinor: number;
  orderId: string;
  authorizationId: string;
  idempotencyKey: string;
}

/** Enrichment columns a state transition may carry alongside state+events
 *  (place_order records its receipt verdict; authorize_spend links the
 *  proposed authorization). Optional so plain transitions stay one-argument. */
export interface OrderStatePatch {
  receipt_status?: OrderReceiptStatus;
  authorization_id?: string | null;
}

export interface FabricateStore {
  saveQuote(quote: Quote, econ: QuoteEconomics, userId: string): Promise<void>;
  /** Read a persisted quote, ownership-scoped (BOM lines link quotes by id). */
  getQuote(quoteId: string, userId: string): Promise<Quote | null>;
  saveOrder(order: Order, fabCostMinor: number, userId: string): Promise<void>;
  getOrder(orderId: string, userId: string): Promise<Order | null>;
  listOrders(userId: string, filter: OrderFilter): Promise<Order[]>;
  // ── money plane (Phase 4, flag-gated) ──
  /** Persist a proposed spend authorization (status pending_human). */
  createAuthorization(authz: SpendAuthorization, userId: string): Promise<void>;
  /** Read an authorization, ownership-scoped. */
  getAuthorization(id: string, userId: string): Promise<SpendAuthorization | null>;
  /** Flip an authorization's status (e.g. elicitation decline → revoked).
   *  Never a substitute for the human approval flow — approval stays
   *  web-app/out-of-band; this records lifecycle outcomes. */
  setAuthorizationStatus(id: string, userId: string, status: AuthorizationStatus): Promise<void>;
  /** Atomic, balance-floored, idempotent debit. The ONLY way credits leave. */
  debit(p: DebitParams): Promise<DebitResult>;
  /** Transition an order's state, append a lifecycle event, and optionally
   *  patch enrichment fields (receipt_status / authorization_id). */
  setOrderState(
    orderId: string,
    userId: string,
    state: OrderState,
    note: string,
    patch?: OrderStatePatch,
  ): Promise<void>;
  /** Read the caller's prepaid wallet balance (minor units), or null when it
   *  can't be read (no wallet row, store outage) — callers must treat null as
   *  "unknown", never as zero. */
  getWalletBalance(userId: string): Promise<number | null>;
}

// ── In-memory (module-global so it survives across calls in one process) ──

const memQuotes = new Map<string, { quote: Quote; econ: QuoteEconomics }>();
const memOrders = new Map<string, { order: Order; fabCostMinor: number }>();
const memAuthz = new Map<string, SpendAuthorization>();
const memWallets = new Map<string, number>(); // userId → balance (minor)
// userId::key → the original request + its result, for replay-match idempotency.
const memDebits = new Map<
  string,
  { result: DebitResult; orderId: string; authorizationId: string; amountMinor: number }
>();

const memKey = (userId: string, id: string): string => `${userId}::${id}`;

export class InMemoryFabricateStore implements FabricateStore {
  async saveQuote(quote: Quote, econ: QuoteEconomics, userId: string): Promise<void> {
    memQuotes.set(memKey(userId, quote.quote_id), { quote, econ });
  }
  async getQuote(quoteId: string, userId: string): Promise<Quote | null> {
    return memQuotes.get(memKey(userId, quoteId))?.quote ?? null;
  }
  async saveOrder(order: Order, fabCostMinor: number, userId: string): Promise<void> {
    memOrders.set(memKey(userId, order.order_id), { order, fabCostMinor });
  }
  async getOrder(orderId: string, userId: string): Promise<Order | null> {
    return memOrders.get(memKey(userId, orderId))?.order ?? null;
  }
  async listOrders(userId: string, filter: OrderFilter): Promise<Order[]> {
    const prefix = `${userId}::`;
    const out: Order[] = [];
    for (const [key, val] of memOrders) {
      if (!key.startsWith(prefix)) continue;
      if (filter.status && val.order.state !== filter.status) continue;
      out.push(val.order);
    }
    out.sort((a, b) => (a.created_at < b.created_at ? 1 : -1));
    return typeof filter.limit === "number" ? out.slice(0, filter.limit) : out;
  }

  async createAuthorization(authz: SpendAuthorization, userId: string): Promise<void> {
    memAuthz.set(memKey(userId, authz.id), { ...authz, user_id: userId });
  }
  async getAuthorization(id: string, userId: string): Promise<SpendAuthorization | null> {
    return memAuthz.get(memKey(userId, id)) ?? null;
  }
  async setAuthorizationStatus(
    id: string,
    userId: string,
    status: AuthorizationStatus,
  ): Promise<void> {
    const a = memAuthz.get(memKey(userId, id));
    if (!a) return;
    memAuthz.set(memKey(userId, id), { ...a, status });
  }

  /**
   * Local mirror of the debit_wallet RPC contract — enough to exercise the
   * place_order logic in tests, enforcing the SAME guards the RPC does:
   * per-(user,key) idempotency WITH replay-match, authorized-only, NOT-expired,
   * amount ceiling, balance floor, one_time consume. The Supabase impl defers
   * to the real SECURITY DEFINER RPC, which is the source of truth.
   */
  async debit(p: DebitParams): Promise<DebitResult> {
    const idk = memKey(p.userId, p.idempotencyKey);
    const prior = memDebits.get(idk);
    if (prior) {
      // A reused key MUST match the original request, or it's an error — mirrors
      // the RPC's idempotency_key_reused guard (027 lines 185-190).
      if (
        prior.orderId !== p.orderId ||
        prior.authorizationId !== p.authorizationId ||
        prior.amountMinor !== p.amountMinor
      ) {
        return { ok: false, reason: "idempotency_key_reused" };
      }
      return { ...prior.result, idempotent: true };
    }

    const authz = memAuthz.get(memKey(p.userId, p.authorizationId));
    if (!authz) return { ok: false, reason: "authz_not_found" };
    if (authz.status === "revoked") return { ok: false, reason: "authz_revoked" };
    if (authz.status !== "authorized") return { ok: false, reason: "authz_not_authorized" };
    if (new Date(authz.expires_at).getTime() <= Date.now()) {
      return { ok: false, reason: "authz_expired" };
    }
    if (p.amountMinor <= 0) return { ok: false, reason: "invalid_amount" };
    if (p.amountMinor > authz.max_amount_minor) {
      return { ok: false, reason: "amount_exceeds_authz" };
    }
    const balance = memWallets.get(p.userId) ?? 0;
    if (balance < p.amountMinor) {
      return { ok: false, reason: "insufficient_funds", balance_minor: balance };
    }
    const after = balance - p.amountMinor;
    memWallets.set(p.userId, after);
    if (authz.kind === "one_time") {
      memAuthz.set(memKey(p.userId, authz.id), { ...authz, status: "consumed" });
    }
    const result: DebitResult = { ok: true, balance_minor: after };
    memDebits.set(idk, {
      result,
      orderId: p.orderId,
      authorizationId: p.authorizationId,
      amountMinor: p.amountMinor,
    });
    return result;
  }

  async setOrderState(
    orderId: string,
    userId: string,
    state: OrderState,
    note: string,
    patch?: OrderStatePatch,
  ): Promise<void> {
    const rec = memOrders.get(memKey(userId, orderId));
    if (!rec) return;
    rec.order.state = state;
    rec.order.events.push({ state, at: new Date().toISOString(), note });
    if (patch?.receipt_status !== undefined) rec.order.receipt_status = patch.receipt_status;
    if (patch?.authorization_id !== undefined) rec.order.authorization_id = patch.authorization_id;
    rec.order.updated_at = new Date().toISOString();
  }

  async getWalletBalance(userId: string): Promise<number | null> {
    return memWallets.get(userId) ?? null;
  }

  // ── test-only seams (NOT on the FabricateStore interface) ──
  /** Seed wallet credit for tests (production top-ups go via credit_wallet). */
  creditWalletForTest(userId: string, minor: number): void {
    memWallets.set(userId, (memWallets.get(userId) ?? 0) + minor);
  }
  /** Simulate the HUMAN web-app approval for tests (never an agent action). */
  approveAuthorizationForTest(id: string, userId: string): boolean {
    const a = memAuthz.get(memKey(userId, id));
    if (!a || a.status !== "pending_human") return false;
    memAuthz.set(memKey(userId, id), { ...a, status: "authorized" });
    return true;
  }
}

// ── Supabase (raw PostgREST + service role) ──

/** Injectable fetch seam (mirrors session-store's sessionFetch) for tests. */
export let fabricateFetch: typeof fetch = (...args) => fetch(...args);
export function setFabricateFetch(fn: typeof fetch): void {
  fabricateFetch = fn;
}

interface SupabaseCfg {
  supabaseUrl: string;
  serviceRoleKey: string;
}

export class SupabaseFabricateStore implements FabricateStore {
  constructor(private cfg: SupabaseCfg) {}

  private headers(extra: Record<string, string> = {}): Record<string, string> {
    return {
      apikey: this.cfg.serviceRoleKey,
      Authorization: `Bearer ${this.cfg.serviceRoleKey}`,
      "Content-Type": "application/json",
      ...extra,
    };
  }

  private url(table: string, query = ""): string {
    return `${this.cfg.supabaseUrl}/rest/v1/${table}${query}`;
  }

  async saveQuote(quote: Quote, econ: QuoteEconomics, userId: string): Promise<void> {
    const row = {
      id: quote.quote_id,
      user_id: userId,
      document_id: quote.document_id,
      doc_hash: quote.doc_hash,
      process: quote.process,
      material: quote.material,
      quantity: quote.quantity,
      fab_options: quote.fab_options,
      dfm: quote.dfm,
      fab_cost_minor: econ.fab_cost_minor,
      margin_minor: econ.margin_minor,
      landed_cost: quote.landed_cost,
      total_amount_minor: quote.total_amount_minor,
      currency: quote.currency,
      expires_at: quote.expires_at,
      // Migration-034 columns — stripped on retry if the DB predates them.
      kerf_intent_hash: quote.kerf_intent_hash ?? null,
      kerf_job_id: quote.kerf_job_id ?? null,
    };
    await this.insertTolerant("quotes", row, ["kerf_intent_hash", "kerf_job_id"]);
  }

  async getQuote(quoteId: string, userId: string): Promise<Quote | null> {
    try {
      const res = await fabricateFetch(
        this.url(
          "quotes",
          `?id=eq.${encodeURIComponent(quoteId)}&user_id=eq.${encodeURIComponent(userId)}&limit=1`,
        ),
        { method: "GET", headers: this.headers({ Accept: "application/vnd.pgrst.object+json" }) },
      );
      if (!res.ok) return null;
      return rowToQuote(await res.json());
    } catch (err) {
      console.error("[fabricate-store] getQuote failed:", err);
      return null;
    }
  }

  async saveOrder(order: Order, fabCostMinor: number, userId: string): Promise<void> {
    const row = {
      id: order.order_id,
      user_id: userId,
      document_id: order.document_id,
      quote_id: order.quote_id,
      state: order.state,
      fab: order.fab,
      fab_order_ref: order.fab_order_ref,
      amount_total_minor: order.amount_total_minor,
      fab_cost_minor: fabCostMinor,
      currency: order.currency,
      ship_to: order.ship_to,
      events: order.events,
      authorization_id: order.authorization_id, // column since migration 027
      // Migration-034 columns — stripped on retry if the DB predates them, so
      // a pre-migration deploy degrades to the 024/027 row instead of losing
      // the whole write. fab_artifact is the handle only, never bytes.
      fab_artifact: order.fab_artifact ?? null,
      receipt_status: order.receipt_status,
      kerf_intent_hash: order.kerf_intent_hash,
    };
    await this.insertTolerant(
      "orders",
      row,
      ["fab_artifact", "receipt_status", "kerf_intent_hash"],
      "id",
    );
  }

  async getOrder(orderId: string, userId: string): Promise<Order | null> {
    try {
      const res = await fabricateFetch(
        this.url(
          "orders",
          `?id=eq.${encodeURIComponent(orderId)}&user_id=eq.${encodeURIComponent(userId)}&limit=1`,
        ),
        { method: "GET", headers: this.headers({ Accept: "application/vnd.pgrst.object+json" }) },
      );
      if (!res.ok) return null;
      return rowToOrder(await res.json());
    } catch (err) {
      console.error("[fabricate-store] getOrder failed:", err);
      return null;
    }
  }

  async listOrders(userId: string, filter: OrderFilter): Promise<Order[]> {
    try {
      const limit = typeof filter.limit === "number" ? filter.limit : 50;
      let q = `?user_id=eq.${encodeURIComponent(userId)}&order=created_at.desc&limit=${limit}`;
      if (filter.status) q += `&state=eq.${encodeURIComponent(filter.status)}`;
      const res = await fabricateFetch(this.url("orders", q), {
        method: "GET",
        headers: this.headers(),
      });
      if (!res.ok) return [];
      const rows = (await res.json()) as unknown[];
      return rows.map(rowToOrder);
    } catch (err) {
      console.error("[fabricate-store] listOrders failed:", err);
      return [];
    }
  }

  async createAuthorization(authz: SpendAuthorization, userId: string): Promise<void> {
    await this.insert("spend_authorizations", {
      id: authz.id,
      user_id: userId,
      quote_id: authz.quote_id,
      kind: authz.kind,
      max_amount_minor: authz.max_amount_minor,
      daily_cap_minor: authz.daily_cap_minor,
      process_allowlist: authz.process_allowlist,
      fab_allowlist: authz.fab_allowlist,
      doc_hash: authz.doc_hash,
      status: authz.status,
      expires_at: authz.expires_at,
    });
  }

  async getAuthorization(id: string, userId: string): Promise<SpendAuthorization | null> {
    try {
      const res = await fabricateFetch(
        this.url(
          "spend_authorizations",
          `?id=eq.${encodeURIComponent(id)}&user_id=eq.${encodeURIComponent(userId)}&limit=1`,
        ),
        { method: "GET", headers: this.headers({ Accept: "application/vnd.pgrst.object+json" }) },
      );
      if (!res.ok) return null;
      return rowToAuthorization(await res.json());
    } catch (err) {
      console.error("[fabricate-store] getAuthorization failed:", err);
      return null;
    }
  }

  async debit(p: DebitParams): Promise<DebitResult> {
    try {
      const res = await fabricateFetch(this.url("rpc/debit_wallet"), {
        method: "POST",
        headers: this.headers(),
        body: JSON.stringify({
          p_user: p.userId,
          p_amount_minor: p.amountMinor,
          p_order_id: p.orderId,
          p_authorization_id: p.authorizationId,
          p_idempotency_key: p.idempotencyKey,
        }),
      });
      if (!res.ok) {
        return { ok: false, reason: `rpc_http_${res.status}` };
      }
      return (await res.json()) as DebitResult;
    } catch (err) {
      console.error("[fabricate-store] debit failed:", err);
      return { ok: false, reason: "rpc_unreachable" };
    }
  }

  async setOrderState(
    orderId: string,
    userId: string,
    state: OrderState,
    note: string,
    patch?: OrderStatePatch,
  ): Promise<void> {
    const order = await this.getOrder(orderId, userId);
    const base: Record<string, unknown> = { state };
    if (order) {
      base.events = [...order.events, { state, at: new Date().toISOString(), note }];
    } else {
      // The pre-read failed (transient network / non-2xx). Appending would
      // replace the whole events jsonb with a singleton — wiping the money
      // audit trail — so PATCH state (+patch fields) WITHOUT the events key
      // and keep whatever history the row already holds.
      console.error(
        `[fabricate-store] setOrderState ${orderId}: pre-read failed — patching without events to preserve history (dropped event: ${state} "${note}")`,
      );
    }
    if (patch?.authorization_id !== undefined) {
      base.authorization_id = patch.authorization_id; // column since 027
    }
    // receipt_status is a migration-034 column — patched tolerantly: if the
    // full PATCH is rejected specifically for column skew on a pre-migration
    // DB, retry without it so the state transition itself never fails there.
    const full =
      patch?.receipt_status !== undefined
        ? { ...base, receipt_status: patch.receipt_status }
        : base;
    const patchOnce = async (body: Record<string, unknown>) =>
      fabricateFetch(
        this.url(
          "orders",
          `?id=eq.${encodeURIComponent(orderId)}&user_id=eq.${encodeURIComponent(userId)}`,
        ),
        {
          method: "PATCH",
          headers: this.headers({ Prefer: "return=minimal" }),
          body: JSON.stringify(body),
        },
      );
    try {
      let res = await patchOnce(full);
      if (!res.ok && full !== base) {
        // Only strip the 034 column when the failure IS column skew — any
        // other failure (transient 5xx, timeout, rate limit) must not
        // silently drop the money-audit field from a then-successful retry.
        const bodyText = await res.text().catch(() => "");
        if (isColumnSkew(res.status, bodyText)) {
          res = await patchOnce(base);
        } else {
          console.error("[fabricate-store] setOrderState failed:", res.status, bodyText);
          return;
        }
      }
      if (!res.ok) {
        console.error(
          "[fabricate-store] setOrderState failed:",
          res.status,
          await res.text().catch(() => ""),
        );
      }
    } catch (err) {
      console.error("[fabricate-store] setOrderState failed:", err);
    }
  }

  async setAuthorizationStatus(
    id: string,
    userId: string,
    status: AuthorizationStatus,
  ): Promise<void> {
    const now = new Date().toISOString();
    const body: Record<string, unknown> = { status };
    if (status === "revoked") body.revoked_at = now;
    if (status === "consumed") body.consumed_at = now;
    try {
      const res = await fabricateFetch(
        this.url(
          "spend_authorizations",
          `?id=eq.${encodeURIComponent(id)}&user_id=eq.${encodeURIComponent(userId)}`,
        ),
        {
          method: "PATCH",
          headers: this.headers({ Prefer: "return=minimal" }),
          body: JSON.stringify(body),
        },
      );
      if (!res.ok) {
        console.error(
          "[fabricate-store] setAuthorizationStatus failed:",
          res.status,
          await res.text().catch(() => ""),
        );
      }
    } catch (err) {
      console.error("[fabricate-store] setAuthorizationStatus failed:", err);
    }
  }

  async getWalletBalance(userId: string): Promise<number | null> {
    try {
      const res = await fabricateFetch(
        this.url(
          "wallets",
          `?user_id=eq.${encodeURIComponent(userId)}&select=credit_balance_minor&limit=1`,
        ),
        { method: "GET", headers: this.headers({ Accept: "application/vnd.pgrst.object+json" }) },
      );
      if (!res.ok) return null;
      const row = (await res.json()) as Record<string, unknown>;
      const minor = Number(row?.credit_balance_minor);
      return Number.isFinite(minor) ? minor : null;
    } catch (err) {
      console.error("[fabricate-store] getWalletBalance failed:", err);
      return null;
    }
  }

  private async insert(
    table: string,
    row: Record<string, unknown>,
    onConflict?: string,
  ): Promise<void> {
    const res = await this.postRow(table, row, onConflict);
    if (!res.ok) {
      console.error(`[fabricate-store] insert ${table} failed:`, res.status, res.text);
    }
  }

  /**
   * Insert that tolerates column skew: if the full row is rejected BECAUSE the
   * migration-034 columns aren't deployed yet (PostgREST fails the WHOLE
   * insert on one unknown key), retry once WITHOUT `newerKeys` so a
   * pre-migration database degrades to the older row shape instead of losing
   * the write entirely. The stripped retry ONLY fires when the first failure
   * is actually column skew (see {@link isColumnSkew}) — any other failure
   * (transient 5xx, timeout, rate limit) keeps the original error path so a
   * flaky-then-successful retry can never silently drop money-audit fields.
   * Best-effort like every store write — never throws.
   */
  private async insertTolerant(
    table: string,
    row: Record<string, unknown>,
    newerKeys: readonly string[],
    onConflict?: string,
  ): Promise<void> {
    const first = await this.postRow(table, row, onConflict);
    if (first.ok) return;
    if (!isColumnSkew(first.status, first.text)) {
      console.error(`[fabricate-store] insert ${table} failed:`, first.status, first.text);
      return;
    }
    const stripped: Record<string, unknown> = { ...row };
    for (const k of newerKeys) delete stripped[k];
    const second = await this.postRow(table, stripped, onConflict);
    if (second.ok) {
      console.error(
        `[fabricate-store] insert ${table}: wrote without [${newerKeys.join(", ")}] ` +
          `(pre-migration schema? first attempt: ${first.status} ${first.text})`,
      );
      return;
    }
    console.error(
      `[fabricate-store] insert ${table} failed:`,
      first.status,
      first.text,
      "| retry without newer keys:",
      second.status,
      second.text,
    );
  }

  /** POST one row; reports the outcome instead of logging so tolerant callers
   *  can retry before deciding what to say. Never throws. */
  private async postRow(
    table: string,
    row: Record<string, unknown>,
    onConflict?: string,
  ): Promise<{ ok: boolean; status: number; text: string }> {
    try {
      const q = onConflict ? `?on_conflict=${onConflict}` : "";
      const res = await fabricateFetch(this.url(table, q), {
        method: "POST",
        headers: this.headers({
          Prefer: onConflict
            ? "resolution=merge-duplicates,return=minimal"
            : "return=minimal",
        }),
        body: JSON.stringify([row]),
      });
      if (res.ok) return { ok: true, status: res.status, text: "" };
      return {
        ok: false,
        status: res.status,
        text: await res.text().catch(() => ""),
      };
    } catch (err) {
      return { ok: false, status: 0, text: err instanceof Error ? err.message : String(err) };
    }
  }
}

/**
 * True when a PostgREST failure is an unknown-column rejection (schema skew:
 * the DB predates a migration that added the column). PostgREST reports this
 * as HTTP 400 with code PGRST204 ("Could not find the 'x' column of 'y' in
 * the schema cache") or a raw Postgres `column ... does not exist`. ONLY this
 * shape may trigger the stripped retry in the tolerant writers — anything
 * else keeps the original error path.
 */
function isColumnSkew(status: number, bodyText: string): boolean {
  return (
    status === 400 &&
    (bodyText.includes("PGRST204") ||
      /column .* does not exist|Could not find/i.test(bodyText))
  );
}

/** Map a PostgREST quotes row to a Quote (inverse of saveQuote's row shape).
 *  Server-only economics columns (fab_cost_minor, margin_minor) are never
 *  copied onto the returned Quote. */
function rowToQuote(row: unknown): Quote {
  const r = (row ?? {}) as Record<string, unknown>;
  return {
    quote_id: String(r.id ?? ""),
    document_id: String(r.document_id ?? ""),
    doc_hash: (r.doc_hash as string) ?? null,
    process: r.process as Quote["process"],
    material: (r.material as string) ?? null,
    quantity: Number(r.quantity ?? 1),
    dfm: (r.dfm as Quote["dfm"]) ?? { checked: false, passed: false, violations: [] },
    fab_options: Array.isArray(r.fab_options) ? (r.fab_options as Quote["fab_options"]) : [],
    landed_cost: (r.landed_cost as Quote["landed_cost"]) ?? {
      shipping_minor: 0,
      duty_minor: 0,
      basis: "unknown",
    },
    total_amount_minor: Number(r.total_amount_minor ?? 0),
    currency: String(r.currency ?? "USD"),
    margin_hidden: true,
    expires_at: String(r.expires_at ?? ""),
    created_at: String(r.created_at ?? ""),
    kerf_intent_hash: (r.kerf_intent_hash as string) ?? null,
    kerf_job_id: (r.kerf_job_id as string) ?? null,
    // Not a column — the recommended option leads the sorted fab_options.
    pricing_basis_best: Array.isArray(r.fab_options)
      ? (r.fab_options as Quote["fab_options"])[0]?.pricing_basis
      : undefined,
  };
}

/** Map a PostgREST orders row to an Order. */
function rowToOrder(row: unknown): Order {
  const r = (row ?? {}) as Record<string, unknown>;
  return {
    order_id: String(r.id ?? ""),
    document_id: String(r.document_id ?? ""),
    quote_id: String(r.quote_id ?? ""),
    state: (r.state as OrderState) ?? "QUOTED",
    fab: (r.fab as string) ?? null,
    fab_order_ref: (r.fab_order_ref as string) ?? null,
    amount_total_minor: Number(r.amount_total_minor ?? 0),
    currency: String(r.currency ?? "USD"),
    ship_to: r.ship_to ?? null,
    events: Array.isArray(r.events) ? (r.events as Order["events"]) : [],
    fab_artifact: (r.fab_artifact as Order["fab_artifact"]) ?? null,
    authorization_id: (r.authorization_id as string) ?? null,
    receipt_status: isReceiptStatus(r.receipt_status) ? r.receipt_status : null,
    kerf_intent_hash: (r.kerf_intent_hash as string) ?? null,
    created_at: String(r.created_at ?? ""),
    updated_at: String(r.updated_at ?? ""),
  };
}

function isReceiptStatus(v: unknown): v is OrderReceiptStatus {
  return v === "holds" || v === "stale" || v === "violated" || v === "unverified";
}

/** Map a PostgREST spend_authorizations row to a SpendAuthorization. */
function rowToAuthorization(row: unknown): SpendAuthorization {
  const r = (row ?? {}) as Record<string, unknown>;
  return {
    id: String(r.id ?? ""),
    user_id: String(r.user_id ?? ""),
    quote_id: (r.quote_id as string) ?? null,
    kind: (r.kind as SpendAuthorization["kind"]) ?? "one_time",
    max_amount_minor: Number(r.max_amount_minor ?? 0),
    daily_cap_minor: r.daily_cap_minor == null ? null : Number(r.daily_cap_minor),
    process_allowlist: (r.process_allowlist as string[] | null) ?? null,
    fab_allowlist: (r.fab_allowlist as string[] | null) ?? null,
    doc_hash: (r.doc_hash as string) ?? null,
    status: (r.status as SpendAuthorization["status"]) ?? "pending_human",
    expires_at: String(r.expires_at ?? ""),
    created_at: String(r.created_at ?? ""),
  };
}

/**
 * Choose the store impl: cloud-backed when a signed-in user AND Supabase
 * service-role env are present, else in-memory (preserves local/stdio behavior
 * and keeps the loop fully runnable offline).
 */
export function createFabricateStore(user: AuthUser | null): FabricateStore {
  const url = (process.env.SUPABASE_URL || "").replace(/\/+$/, "");
  const key = process.env.SUPABASE_SERVICE_ROLE_KEY || "";
  if (user && url && key) {
    return new SupabaseFabricateStore({ supabaseUrl: url, serviceRoleKey: key });
  }
  return new InMemoryFabricateStore();
}

/** The owner id used for store scoping — the authenticated user, or "local". */
export function ownerId(user: AuthUser | null): string {
  return user?.sub ?? "local";
}
