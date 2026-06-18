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
import type { Order, OrderState, Quote, QuoteEconomics } from "./types.js";

export interface OrderFilter {
  status?: OrderState;
  limit?: number;
}

export interface FabricateStore {
  saveQuote(quote: Quote, econ: QuoteEconomics, userId: string): Promise<void>;
  saveOrder(order: Order, fabCostMinor: number, userId: string): Promise<void>;
  getOrder(orderId: string, userId: string): Promise<Order | null>;
  listOrders(userId: string, filter: OrderFilter): Promise<Order[]>;
}

// ── In-memory (module-global so it survives across calls in one process) ──

const memQuotes = new Map<string, { quote: Quote; econ: QuoteEconomics }>();
const memOrders = new Map<string, { order: Order; fabCostMinor: number }>();

const memKey = (userId: string, id: string): string => `${userId}::${id}`;

export class InMemoryFabricateStore implements FabricateStore {
  async saveQuote(quote: Quote, econ: QuoteEconomics, userId: string): Promise<void> {
    memQuotes.set(memKey(userId, quote.quote_id), { quote, econ });
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
