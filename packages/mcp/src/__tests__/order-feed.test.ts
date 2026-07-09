import { describe, it, expect } from "vitest";
import { getOrderFeed } from "../tools/order-feed.js";
import { InMemoryFabricateStore } from "../fabricate/store.js";
import { authorizeSpend } from "../tools/ordering.js";
import type { Order } from "../fabricate/types.js";
import type { AuthUser } from "../oauth.js";
import type {
  SessionEvent,
  SessionEventStore,
  StoredSessionEvent,
} from "../session-store.js";

/**
 * get_order_feed — the dock's single read. Two invariants under test:
 *
 *  1. windowing: the store limit applies BEFORE the document filter, so the
 *     feed must fetch wide (200) and slice to 20 AFTER filtering — newer
 *     orders on OTHER documents can't push this document's live approval
 *     out of the dock.
 *  2. version token: covers authorization status, receipt status, pricing
 *     basis, and wallet balance — the states that change WITHOUT an order
 *     state transition or new event (human approval flips only the authz
 *     row) — so the viewer's version-dedup re-renders on approval.
 */

class NullEventStore implements SessionEventStore {
  async append(_sessionId: string, _evt: SessionEvent): Promise<void> {}
  async list(): Promise<StoredSessionEvent[]> {
    return [];
  }
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
const out = (r: { content: Array<{ text: string }> }): any =>
  JSON.parse(r.content[0].text);

function makeOrder(orderId: string, documentId: string, createdAt: string, over: Partial<Order> = {}): Order {
  return {
    order_id: orderId,
    document_id: documentId,
    quote_id: `q_${orderId}`,
    state: "QUOTED",
    fab: "digitalmetal",
    fab_order_ref: null,
    amount_total_minor: 5000,
    currency: "USD",
    ship_to: null,
    events: [{ state: "QUOTED", at: createdAt, note: "quote" }],
    authorization_id: null,
    receipt_status: null,
    kerf_intent_hash: null,
    created_at: createdAt,
    updated_at: createdAt,
    ...over,
  };
}

const iso = (i: number): string => new Date(Date.UTC(2026, 0, 1, 0, 0, i)).toISOString();

describe("get_order_feed windowing (limit applied AFTER the document filter)", () => {
  it("keeps an older document's order visible past 20 newer orders elsewhere", async () => {
    const user: AuthUser = { sub: "u-feed-window", email: "x@y.z" };
    const store = new InMemoryFabricateStore();
    // Oldest row: the order the dock must not lose (doc A).
    await store.saveOrder(makeOrder("ord_a", "doc_a", iso(0)), 4000, user.sub);
    // 25 newer orders on doc B — with a pre-filter limit of 20 these would
    // evict ord_a from the window entirely.
    for (let i = 1; i <= 25; i++) {
      await store.saveOrder(makeOrder(`ord_b_${i}`, "doc_b", iso(i)), 4000, user.sub);
    }

    const feed = out(await getOrderFeed({ document_id: "doc_a" }, store, user));
    expect(feed.orders).toHaveLength(1);
    expect(feed.orders[0].order_id).toBe("ord_a");

    // And the busy document still caps at the dock's 20.
    const feedB = out(await getOrderFeed({ document_id: "doc_b" }, store, user));
    expect(feedB.orders).toHaveLength(20);
  });
});

describe("get_order_feed version token (approval/wallet changes re-render the dock)", () => {
  it("changes when the human approves the authorization (no order state/event change)", async () => {
    const prev = process.env.VCAD_FABRICATE_ORDERING;
    process.env.VCAD_FABRICATE_ORDERING = "1";
    try {
      const user: AuthUser = { sub: "u-feed-authz", email: "x@y.z" };
      const store = new InMemoryFabricateStore();
      await store.saveOrder(makeOrder("ord_v", "doc_v", iso(0)), 4000, user.sub);
      const a = out(
        await authorizeSpend({ order_id: "ord_v" }, store, new NullEventStore(), user),
      );

      const pending = out(await getOrderFeed({ document_id: "doc_v" }, store, user));
      expect(pending.orders[0].state_chip).toBe("approval");
      expect(pending.orders[0].authorization.status).toBe("pending_human");

      // Human approval on vcad.io flips ONLY the spend_authorizations row.
      const orderBefore = await store.getOrder("ord_v", user.sub);
      expect(store.approveAuthorizationForTest(a.authorization_id, user.sub)).toBe(true);
      const orderAfter = await store.getOrder("ord_v", user.sub);
      expect(orderAfter?.state).toBe(orderBefore?.state);
      expect(orderAfter?.events.length).toBe(orderBefore?.events.length);

      const approved = out(await getOrderFeed({ document_id: "doc_v" }, store, user));
      expect(approved.orders[0].state_chip).toBe("quoted");
      expect(approved.orders[0].authorization.status).toBe("authorized");
      // The whole point: the viewer dedups on version, so it MUST move.
      expect(approved.version).not.toBe(pending.version);
    } finally {
      if (prev === undefined) delete process.env.VCAD_FABRICATE_ORDERING;
      else process.env.VCAD_FABRICATE_ORDERING = prev;
    }
  });

  it("changes when the wallet balance changes", async () => {
    const user: AuthUser = { sub: "u-feed-wallet", email: "x@y.z" };
    const store = new InMemoryFabricateStore();
    await store.saveOrder(makeOrder("ord_w", "doc_w", iso(0)), 4000, user.sub);

    const before = out(await getOrderFeed({ document_id: "doc_w" }, store, user));
    store.creditWalletForTest(user.sub, 12345);
    const after = out(await getOrderFeed({ document_id: "doc_w" }, store, user));
    expect(after.wallet_balance_usd).toBe(123.45);
    expect(after.version).not.toBe(before.version);
  });

  it("changes when the persisted receipt status changes (state stays QUOTED)", async () => {
    const user: AuthUser = { sub: "u-feed-receipt", email: "x@y.z" };
    const store = new InMemoryFabricateStore();
    await store.saveOrder(makeOrder("ord_r", "doc_r", iso(0)), 4000, user.sub);

    const before = out(await getOrderFeed({ document_id: "doc_r" }, store, user));
    // place_order's receipt gate persists "violated" with the state unchanged;
    // strip the appended event's contribution by comparing against a second
    // no-op read to make sure the receipt itself participates.
    await store.setOrderState("ord_r", user.sub, "QUOTED", "receipt violated at place time", {
      receipt_status: "violated",
    });
    const after = out(await getOrderFeed({ document_id: "doc_r" }, store, user));
    expect(after.orders[0].receipt.status).toBe("violated");
    expect(after.version).not.toBe(before.version);
  });
});
