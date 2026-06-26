import { describe, it, expect, beforeEach, afterEach } from "vitest";
import { authorizeSpend, placeOrder } from "../tools/ordering.js";
import { InMemoryFabricateStore } from "../fabricate/store.js";
import { storeArtifact, clearArtifacts } from "../tools/artifact-store.js";
import type { Order, SpendAuthorization } from "../fabricate/types.js";
import type { AuthUser } from "../oauth.js";
import type { SessionEvent, SessionEventStore, StoredSessionEvent } from "../session-store.js";

/** Records control events so we can assert the spine got them. */
class RecordingEventStore implements SessionEventStore {
  public events: Array<SessionEvent & { sessionId: string }> = [];
  async append(sessionId: string, evt: SessionEvent): Promise<void> {
    this.events.push({ sessionId, ...evt });
  }
  async list(): Promise<StoredSessionEvent[]> {
    return [];
  }
  types(): string[] {
    return this.events.map((e) => e.type);
  }
}

function makeOrder(over: Partial<Order> = {}): Order {
  const now = new Date().toISOString();
  return {
    order_id: "ord_1",
    document_id: "doc_x",
    quote_id: "q_1",
    state: "QUOTED",
    fab: "digitalmetal",
    fab_order_ref: null,
    amount_total_minor: 5000,
    currency: "USD",
    ship_to: null,
    events: [{ state: "QUOTED", at: now, note: "quote" }],
    created_at: now,
    updated_at: now,
    ...over,
  };
}

const text = (r: { content: Array<{ text: string }> }) => r.content[0].text;
const json = (r: { content: Array<{ text: string }> }) => JSON.parse(text(r));

describe("Fabricate ordering — disabled by default (flag gate)", () => {
  beforeEach(() => delete process.env.VCAD_FABRICATE_ORDERING);

  it("authorize_spend refuses unless VCAD_FABRICATE_ORDERING=1", async () => {
    const res = await authorizeSpend({ order_id: "ord_1" }, new InMemoryFabricateStore(), new RecordingEventStore(), null);
    expect(res.isError).toBe(true);
    expect(text(res)).toContain("disabled");
  });

  it("place_order refuses unless VCAD_FABRICATE_ORDERING=1", async () => {
    const res = await placeOrder(
      { order_id: "ord_1", authorization_id: "a" },
      new InMemoryFabricateStore(),
      new RecordingEventStore(),
      null,
    );
    expect(res.isError).toBe(true);
    expect(text(res)).toContain("disabled");
  });
});

describe("Fabricate ordering — enabled (test-mode)", () => {
  let prev: string | undefined;
  beforeEach(() => {
    prev = process.env.VCAD_FABRICATE_ORDERING;
    process.env.VCAD_FABRICATE_ORDERING = "1";
  });
  afterEach(() => {
    if (prev === undefined) delete process.env.VCAD_FABRICATE_ORDERING;
    else process.env.VCAD_FABRICATE_ORDERING = prev;
    clearArtifacts();
  });

  it("binds a fab artifact handle to the placed order (handle on the spine, not bytes)", async () => {
    const user: AuthUser = { sub: "u-artifact", email: "x@y.z" };
    const store = new InMemoryFabricateStore();
    const es = new RecordingEventStore();
    await store.saveOrder(makeOrder(), 4000, user.sub);
    const handle = storeArtifact([
      { name: "top.gbr", content: "G04 fab copper*" },
      { name: "out.drl", content: "M48\n" },
    ]);

    const a = json(await authorizeSpend({ order_id: "ord_1" }, store, es, user));
    store.approveAuthorizationForTest(a.authorization_id, user.sub);
    store.creditWalletForTest(user.sub, 10000);

    const res = await placeOrder(
      { order_id: "ord_1", authorization_id: a.authorization_id, fab_artifact_id: handle.artifact_id },
      store,
      es,
      user,
    );
    expect(res.isError).toBeFalsy();
    const out = json(res);
    expect(out.state).toBe("PAID");
    expect(out.fab_artifact.artifact_id).toBe(handle.artifact_id);
    expect(out.fab_artifact.files).toBe(2);
    // The fab bytes never appear in the tool result.
    expect(text(res)).not.toContain("G04 fab copper");

    // The durable spine event carries the handle (for the fab-submission worker).
    const placed = es.events.find((e) => e.type === "order_placed");
    expect(placed).toBeTruthy();
    expect(
      (placed!.payload as { fab_artifact?: { artifact_id?: string } }).fab_artifact?.artifact_id,
    ).toBe(handle.artifact_id);
  });

  it("rejects a place_order with an unknown fab artifact handle before any debit", async () => {
    const user: AuthUser = { sub: "u-badart", email: "x@y.z" };
    const store = new InMemoryFabricateStore();
    await store.saveOrder(makeOrder(), 4000, user.sub);
    const a = json(await authorizeSpend({ order_id: "ord_1" }, store, new RecordingEventStore(), user));
    store.approveAuthorizationForTest(a.authorization_id, user.sub);
    store.creditWalletForTest(user.sub, 10000);

    const res = await placeOrder(
      { order_id: "ord_1", authorization_id: a.authorization_id, fab_artifact_id: "art_missing" },
      store,
      new RecordingEventStore(),
      user,
    );
    expect(res.isError).toBe(true);
    expect(text(res)).toContain("Unknown or expired fab artifact");
    // No money moved — the order is still QUOTED.
    expect((await store.getOrder("ord_1", user.sub))?.state).toBe("QUOTED");
  });

  it("runs propose → (human approve) → place: debits once, moves to PAID, emits spine events", async () => {
    const user: AuthUser = { sub: "u-happy", email: "x@y.z" };
    const store = new InMemoryFabricateStore();
    const es = new RecordingEventStore();
    await store.saveOrder(makeOrder(), 4000, user.sub);

    // 1. Agent proposes — pending_human, propose_order on the spine, no money moved.
    const aRes = await authorizeSpend({ order_id: "ord_1" }, store, es, user);
    const a = json(aRes);
    expect(a.status).toBe("pending_human");
    expect(es.types()).toContain("propose_order");
    const authId = a.authorization_id as string;

    // 2. Placing BEFORE human approval is refused.
    const early = await placeOrder({ order_id: "ord_1", authorization_id: authId }, store, es, user);
    expect(early.isError).toBe(true);
    expect(text(early)).toContain("pending human approval");

    // 3. The human approves (web app, simulated) and the wallet has credit.
    expect(store.approveAuthorizationForTest(authId, user.sub)).toBe(true);
    store.creditWalletForTest(user.sub, 10000);

    // 4. Now place succeeds: PAID, order_placed emitted.
    const pRes = await placeOrder({ order_id: "ord_1", authorization_id: authId }, store, es, user);
    expect(pRes.isError).toBeFalsy();
    expect(json(pRes).state).toBe("PAID");
    expect(es.types()).toContain("order_placed");
    expect((await store.getOrder("ord_1", user.sub))?.state).toBe("PAID");
  });

  it("refuses place_order with an unknown authorization", async () => {
    const user: AuthUser = { sub: "u-noauth", email: "x@y.z" };
    const store = new InMemoryFabricateStore();
    await store.saveOrder(makeOrder(), 4000, user.sub);
    const res = await placeOrder({ order_id: "ord_1", authorization_id: "nope" }, store, new RecordingEventStore(), user);
    expect(res.isError).toBe(true);
    expect(text(res)).toContain("Unknown authorization_id");
  });

  it("fails payment on insufficient funds and leaves the order QUOTED", async () => {
    const user: AuthUser = { sub: "u-broke", email: "x@y.z" };
    const store = new InMemoryFabricateStore();
    const es = new RecordingEventStore();
    await store.saveOrder(makeOrder(), 4000, user.sub);
    const a = json(await authorizeSpend({ order_id: "ord_1" }, store, es, user));
    store.approveAuthorizationForTest(a.authorization_id, user.sub);
    // No credit added → balance 0 < 5000.

    const res = await placeOrder({ order_id: "ord_1", authorization_id: a.authorization_id }, store, es, user);
    expect(res.isError).toBe(true);
    expect(text(res)).toContain("insufficient_funds");
    expect((await store.getOrder("ord_1", user.sub))?.state).toBe("QUOTED");
    expect(es.types()).toContain("order_payment_failed");
  });

  it("debits at most once across a retry (idempotent)", async () => {
    const user: AuthUser = { sub: "u-idem", email: "x@y.z" };
    const store = new InMemoryFabricateStore();
    const a = { authorization_id: "" } as { authorization_id: string };
    await store.saveOrder(makeOrder(), 4000, user.sub);
    a.authorization_id = json(await authorizeSpend({ order_id: "ord_1" }, store, new RecordingEventStore(), user))
      .authorization_id;
    store.approveAuthorizationForTest(a.authorization_id, user.sub);
    store.creditWalletForTest(user.sub, 10000);

    const first = await store.debit({
      userId: user.sub,
      amountMinor: 5000,
      orderId: "ord_1",
      authorizationId: a.authorization_id,
      idempotencyKey: "ord_1:debit",
    });
    const second = await store.debit({
      userId: user.sub,
      amountMinor: 5000,
      orderId: "ord_1",
      authorizationId: a.authorization_id,
      idempotencyKey: "ord_1:debit",
    });
    expect(first.ok).toBe(true);
    expect(first.balance_minor).toBe(5000); // 10000 - 5000
    expect(second.ok).toBe(true);
    expect(second.idempotent).toBe(true);
    expect(second.balance_minor).toBe(5000); // not double-debited
  });

  it("won't authorize a non-QUOTED order", async () => {
    const user: AuthUser = { sub: "u-paid", email: "x@y.z" };
    const store = new InMemoryFabricateStore();
    await store.saveOrder(makeOrder({ state: "PAID" }), 4000, user.sub);
    const res = await authorizeSpend({ order_id: "ord_1" }, store, new RecordingEventStore(), user);
    expect(res.isError).toBe(true);
    expect(text(res)).toContain("not QUOTED");
  });

  it("rejects a spend ceiling below the order total", async () => {
    const user: AuthUser = { sub: "u-low", email: "x@y.z" };
    const store = new InMemoryFabricateStore();
    await store.saveOrder(makeOrder(), 4000, user.sub);
    const res = await authorizeSpend({ order_id: "ord_1", max_amount_minor: 100 }, store, new RecordingEventStore(), user);
    expect(res.isError).toBe(true);
    expect(text(res)).toContain("below the order total");
  });

  it("refuses an expired (but authorized) authorization — leaves the order QUOTED", async () => {
    const user: AuthUser = { sub: "u-exp", email: "x@y.z" };
    const store = new InMemoryFabricateStore();
    await store.saveOrder(makeOrder(), 4000, user.sub);
    const past = new Date(Date.now() - 3_600_000).toISOString();
    const authz: SpendAuthorization = {
      id: "authz-exp",
      user_id: user.sub,
      quote_id: "q_1",
      kind: "one_time",
      max_amount_minor: 5000,
      daily_cap_minor: null,
      process_allowlist: null,
      fab_allowlist: null,
      doc_hash: null,
      status: "authorized",
      expires_at: past,
      created_at: past,
    };
    await store.createAuthorization(authz, user.sub);
    store.creditWalletForTest(user.sub, 10000);

    const res = await placeOrder({ order_id: "ord_1", authorization_id: "authz-exp" }, store, new RecordingEventStore(), user);
    expect(res.isError).toBe(true);
    expect(text(res)).toContain("expired");
    expect((await store.getOrder("ord_1", user.sub))?.state).toBe("QUOTED");
  });

  it("finalizes a paid-but-unrecorded order on retry without double-debiting (the blocker)", async () => {
    const user: AuthUser = { sub: "u-crash", email: "x@y.z" };
    const store = new InMemoryFabricateStore();
    const es = new RecordingEventStore();
    await store.saveOrder(makeOrder(), 4000, user.sub);
    const a = json(await authorizeSpend({ order_id: "ord_1" }, store, es, user));
    store.approveAuthorizationForTest(a.authorization_id, user.sub);
    store.creditWalletForTest(user.sub, 10000);

    // Simulate a crash AFTER debit but BEFORE setOrderState: the debit commits
    // (authz consumed, balance down), but the order is left QUOTED.
    const d = await store.debit({
      userId: user.sub,
      amountMinor: 5000,
      orderId: "ord_1",
      authorizationId: a.authorization_id,
      idempotencyKey: "ord_1:debit",
    });
    expect(d.ok).toBe(true);
    expect((await store.getAuthorization(a.authorization_id, user.sub))?.status).toBe("consumed");
    expect((await store.getOrder("ord_1", user.sub))?.state).toBe("QUOTED");

    // Retry: must finalize to PAID via an idempotent replay — not a 2nd charge,
    // not a permanent strand.
    const res = await placeOrder({ order_id: "ord_1", authorization_id: a.authorization_id }, store, es, user);
    expect(res.isError).toBeFalsy();
    expect(json(res).state).toBe("PAID");
    expect(json(res).idempotent).toBe(true);
    expect((await store.getOrder("ord_1", user.sub))?.state).toBe("PAID");
  });

  it("rejects a reused idempotency key whose terms differ (no silent replay)", async () => {
    const user: AuthUser = { sub: "u-reuse", email: "x@y.z" };
    const store = new InMemoryFabricateStore();
    await store.saveOrder(makeOrder(), 4000, user.sub);
    const a = json(await authorizeSpend({ order_id: "ord_1" }, store, new RecordingEventStore(), user));
    store.approveAuthorizationForTest(a.authorization_id, user.sub);
    store.creditWalletForTest(user.sub, 10000);
    const key = "shared-key";

    const first = await store.debit({
      userId: user.sub, amountMinor: 5000, orderId: "ord_1", authorizationId: a.authorization_id, idempotencyKey: key,
    });
    expect(first.ok).toBe(true);
    const reused = await store.debit({
      userId: user.sub, amountMinor: 9999, orderId: "ord_1", authorizationId: a.authorization_id, idempotencyKey: key,
    });
    expect(reused.ok).toBe(false);
    expect(reused.reason).toBe("idempotency_key_reused");
  });
});
