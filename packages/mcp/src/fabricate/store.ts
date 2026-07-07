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
  DebitResult,
  Order,
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
  /** Atomic, balance-floored, idempotent debit. The ONLY way credits leave. */
  debit(p: DebitParams): Promise<DebitResult>;
  /** Transition an order's state and append a lifecycle event. */
  setOrderState(orderId: string, userId: string, state: OrderState, note: string): Promise<void>;
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

  async setOrderState(orderId: string, userId: string, state: OrderState, note: string): Promise<void> {
    const rec = memOrders.get(memKey(userId, orderId));
    if (!rec) return;
    rec.order.state = state;
    rec.order.events.push({ state, at: new Date().toISOString(), note });
    rec.order.updated_at = new Date().toISOString();
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
    };
    await this.insert("quotes", row);
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
    // `fab_artifact` is intentionally NOT written here — there is no orders
    // column for it yet, and including an unknown key would fail the whole
    // PostgREST insert. The durable cloud record of the fab bundle is the
    // `order_placed` session-event (place_order); the handle is also re-suppliable
    // at place_order time. A dedicated orders.fab_artifact column is the follow-up.
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
    };
    await this.insert("orders", row, "id");
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

  async setOrderState(orderId: string, userId: string, state: OrderState, note: string): Promise<void> {
    const order = await this.getOrder(orderId, userId);
    const events = [
      ...(order?.events ?? []),
      { state, at: new Date().toISOString(), note },
    ];
    try {
      const res = await fabricateFetch(
        this.url(
          "orders",
          `?id=eq.${encodeURIComponent(orderId)}&user_id=eq.${encodeURIComponent(userId)}`,
        ),
        {
          method: "PATCH",
          headers: this.headers({ Prefer: "return=minimal" }),
          body: JSON.stringify({ state, events }),
        },
      );
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

  private async insert(
    table: string,
    row: Record<string, unknown>,
    onConflict?: string,
  ): Promise<void> {
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
      if (!res.ok) {
        console.error(
          `[fabricate-store] insert ${table} failed:`,
          res.status,
          await res.text().catch(() => ""),
        );
      }
    } catch (err) {
      console.error(`[fabricate-store] insert ${table} failed:`, err);
    }
  }
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
    created_at: String(r.created_at ?? ""),
    updated_at: String(r.updated_at ?? ""),
  };
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
