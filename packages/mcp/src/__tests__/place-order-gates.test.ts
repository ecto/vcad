import { describe, it, expect, beforeAll, beforeEach, afterEach } from "vitest";
import { Engine } from "@vcad/engine";
import type { Document } from "@vcad/ir";
import { documents, getSession, openDocument } from "../tools/session.js";
import { quoteManufacturing } from "../tools/order.js";
import { authorizeSpend, placeOrder } from "../tools/ordering.js";
import { checkClearance } from "../tools/clearance.js";
import { predictPhysicsTool } from "../tools/physics.js";
import { InMemoryFabricateStore } from "../fabricate/store.js";
import type { AuthUser } from "../oauth.js";
import type {
  SessionEvent,
  SessionEventStore,
  SessionStore,
  StoredSessionEvent,
} from "../session-store.js";

/**
 * The M4 fail-closed money gates in place_order, driven end-to-end through a
 * REAL session (quotes from inline IR get `document_id: "inline:<hash>"` and
 * deliberately SKIP both gates — so every case here quotes by document_id):
 *
 *   gate 1 — geometry: doc_hash re-hashed at place time must match the quote.
 *   gate 2 — receipt: persisted clearance specs re-verified; fail refuses;
 *            no specs proceeds flagged "unverified"; passing specs → "holds".
 */

let engine: Engine;

beforeAll(async () => {
  engine = await Engine.init();
});

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

// eslint-disable-next-line @typescript-eslint/no-explicit-any
const out = (r: { content: Array<{ text: string }> }): any =>
  JSON.parse(r.content[0].text);
const text = (r: { content: Array<{ text: string }> }) => r.content[0].text;

/** Rotor/stator fixture (the clearance.test.ts pattern): radius 5.0 leaves a
 *  clean 1.0 mm air gap; radius 7.0 pierces the stator (negative distance). */
function rotorStatorDocument(rotorRadius: number): Document {
  const nodes: Record<string, unknown> = {};
  let id = 0;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const add = (name: string, op: any): number => {
    id += 1;
    nodes[String(id)] = { id, name, op };
    return id;
  };
  const rotorCyl = add("rotor-solid", {
    type: "Cylinder",
    radius: rotorRadius,
    height: 8,
    segments: 128,
  });
  const rotor = add("rotor", {
    type: "Translate",
    child: rotorCyl,
    offset: { x: 0, y: 0, z: 1 },
  });
  const statorOuter = add("stator-outer", {
    type: "Cylinder",
    radius: 10,
    height: 10,
    segments: 128,
  });
  const statorBoreCyl = add("stator-bore-solid", {
    type: "Cylinder",
    radius: 6,
    height: 12,
    segments: 128,
  });
  const statorBore = add("stator-bore", {
    type: "Translate",
    child: statorBoreCyl,
    offset: { x: 0, y: 0, z: -1 },
  });
  const stator = add("stator", { type: "Difference", left: statorOuter, right: statorBore });
  return {
    version: "0.1",
    nodes,
    materials: {},
    part_materials: {},
    roots: [
      { root: rotor, material: "steel" },
      { root: stator, material: "aluminum" },
    ],
  } as unknown as Document;
}

/** Quote the session, propose + human-approve a spend, fund the wallet —
 *  everything up to the place_order gates. */
async function quotedAndApproved(
  docId: string,
  store: InMemoryFabricateStore,
  es: RecordingEventStore,
  user: AuthUser,
): Promise<{ orderId: string; authorizationId: string }> {
  const quote = out(
    await quoteManufacturing(
      { document_id: docId, process: "cast_metal", quantity: 1, material: "stainless" },
      engine,
      store,
      user,
    ),
  );
  expect(quote.order_id).toBeTruthy();
  const a = out(await authorizeSpend({ order_id: quote.order_id }, store, es, user));
  expect(store.approveAuthorizationForTest(a.authorization_id, user.sub)).toBe(true);
  store.creditWalletForTest(user.sub, 10_000_000);
  return { orderId: quote.order_id, authorizationId: a.authorization_id };
}

describe("place_order money gates (doc-hash + receipt, fail-closed)", () => {
  let prev: string | undefined;
  beforeEach(() => {
    prev = process.env.VCAD_FABRICATE_ORDERING;
    process.env.VCAD_FABRICATE_ORDERING = "1";
    documents.clear();
  });
  afterEach(() => {
    if (prev === undefined) delete process.env.VCAD_FABRICATE_ORDERING;
    else process.env.VCAD_FABRICATE_ORDERING = prev;
  });

  it("refuses on doc_hash mismatch: geometry edited after quoting kills the quote", async () => {
    const user: AuthUser = { sub: "u-gate-hash", email: "x@y.z" };
    const store = new InMemoryFabricateStore();
    const es = new RecordingEventStore();
    const docId = out(openDocument({ initial: rotorStatorDocument(5.0) })).document_id;
    const { orderId, authorizationId } = await quotedAndApproved(docId, store, es, user);

    // Edit the design AFTER quoting — the quote priced different geometry.
    const doc = getSession(docId);
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (Object.values(doc.nodes).find((n) => n.name === "rotor-solid")!.op as any).radius = 5.5;

    const res = await placeOrder(
      { order_id: orderId, authorization_id: authorizationId },
      store,
      es,
      user,
      engine,
    );
    expect(res.isError).toBe(true);
    expect(text(res)).toContain("doc_hash mismatch");
    expect(text(res)).toContain("re-quote");
    // No money moved — and the refusal is DURABLE: the order is expired so a
    // retry on an instance where the session isn't resident still refuses.
    expect((await store.getOrder(orderId, user.sub))?.state).toBe("EXPIRED");
    const blocked = es.events.find((e) => e.type === "order_blocked");
    expect(blocked).toBeTruthy();
    expect((blocked!.payload as { reason?: string }).reason).toBe("doc_hash_mismatch");

    // Simulate instance churn / close_document: the session is gone, but the
    // EXPIRED order refuses at the entry check — the gate can't be bypassed
    // by making the document non-resident.
    documents.clear();
    const retry = await placeOrder(
      { order_id: orderId, authorization_id: authorizationId },
      store,
      es,
      user,
      engine,
    );
    expect(retry.isError).toBe(true);
    expect(text(retry)).toContain("not placeable");
  });

  it("refuses receipt_violated: a failing clearance claim blocks the debit", async () => {
    const user: AuthUser = { sub: "u-gate-recv", email: "x@y.z" };
    const store = new InMemoryFabricateStore();
    const es = new RecordingEventStore();
    // Rotor radius 7.0 pierces the stator — measured distance is NEGATIVE, so
    // any positive min_mm makes the persisted claim fail at place time.
    const docId = out(openDocument({ initial: rotorStatorDocument(7.0) })).document_id;
    const clearance = await checkClearance(
      { document_id: docId, group_a: ["rotor"], group_b: ["stator"], min_mm: 0.1, label: "air-gap" },
      engine,
    );
    expect(clearance.isError).toBeUndefined();
    expect(out(clearance).pass).toBe(false);

    // Spec persisted BEFORE quoting, so gate 1 (doc hash) still holds and the
    // refusal is attributable to gate 2 alone.
    const { orderId, authorizationId } = await quotedAndApproved(docId, store, es, user);
    const res = await placeOrder(
      { order_id: orderId, authorization_id: authorizationId },
      store,
      es,
      user,
      engine,
    );
    expect(res.isError).toBe(true);
    expect(text(res)).toContain("receipt violated");
    expect(text(res)).toContain("mech.clearance.air-gap");
    const after = await store.getOrder(orderId, user.sub);
    expect(after?.state).toBe("QUOTED");
    // The verdict is persisted (state stays QUOTED) so the refusal is durable.
    expect(after?.receipt_status).toBe("violated");
    const blocked = es.events.find((e) => e.type === "order_blocked");
    expect(blocked).toBeTruthy();
    expect((blocked!.payload as { reason?: string }).reason).toBe("receipt_violated");

    // close_document / instance churn must NOT bypass the known-failing
    // receipt: with the session gone the entry check still refuses (before
    // the gates would have degraded to "unverified — proceeding").
    documents.clear();
    const retry = await placeOrder(
      { order_id: orderId, authorization_id: authorizationId },
      store,
      es,
      user,
      engine,
    );
    expect(retry.isError).toBe(true);
    expect(text(retry)).toContain("receipt violated");
    expect((await store.getOrder(orderId, user.sub))?.state).toBe("QUOTED");
  });

  it("proceeds flagged 'unverified' when the document carries no clearance specs", async () => {
    const user: AuthUser = { sub: "u-gate-unv", email: "x@y.z" };
    const store = new InMemoryFabricateStore();
    const es = new RecordingEventStore();
    const docId = out(openDocument({ initial: rotorStatorDocument(5.0) })).document_id;
    const { orderId, authorizationId } = await quotedAndApproved(docId, store, es, user);

    const res = await placeOrder(
      { order_id: orderId, authorization_id: authorizationId },
      store,
      es,
      user,
      engine,
    );
    expect(res.isError).toBeFalsy();
    const body = out(res);
    expect(body.state).toBe("PAID");
    expect(body.receipt.status).toBe("unverified");
    expect((await store.getOrder(orderId, user.sub))?.state).toBe("PAID");
    expect((await store.getOrder(orderId, user.sub))?.receipt_status).toBe("unverified");
  });

  it("re-verifies a passing spec at place time and records receipt 'holds'", async () => {
    const user: AuthUser = { sub: "u-gate-holds", email: "x@y.z" };
    const store = new InMemoryFabricateStore();
    const es = new RecordingEventStore();
    const docId = out(openDocument({ initial: rotorStatorDocument(5.0) })).document_id;
    const clearance = await checkClearance(
      { document_id: docId, group_a: ["rotor"], group_b: ["stator"], min_mm: 0.9, label: "air-gap" },
      engine,
    );
    expect(out(clearance).pass).toBe(true);

    const { orderId, authorizationId } = await quotedAndApproved(docId, store, es, user);
    const res = await placeOrder(
      { order_id: orderId, authorization_id: authorizationId },
      store,
      es,
      user,
      engine,
    );
    expect(res.isError).toBeFalsy();
    const body = out(res);
    expect(body.state).toBe("PAID");
    expect(body.receipt.status).toBe("holds");
    expect(body.receipt.note).toContain("re-verified");
    expect((await store.getOrder(orderId, user.sub))?.receipt_status).toBe("holds");
    // The placement event carries the verdict for the feed.
    const placed = es.events.find((e) => e.type === "order_placed");
    expect((placed!.payload as { receipt_status?: string }).receipt_status).toBe("holds");
  });

  it("refuses receipt_violated: a failing physics claim blocks the debit", async () => {
    const user: AuthUser = { sub: "u-gate-phys", email: "x@y.z" };
    const store = new InMemoryFabricateStore();
    const es = new RecordingEventStore();
    const docId = out(openDocument({ initial: rotorStatorDocument(5.0) })).document_id;
    // Persist a physics spec whose displacement limit cannot hold: the rotor
    // (radius 5, z 1..9) under a 100 N lateral tip load vs a 1e-6 mm limit.
    // Persisted BEFORE quoting so gate 1 (doc hash) holds and the refusal is
    // attributable to gate 2 alone.
    predictPhysicsTool(
      {
        document_id: docId,
        part: "rotor",
        loads: [{ region: { min: [-5, -5, 9], max: [5, 5, 9] }, force: [100, 0, 0] }],
        supports: [{ region: { min: [-5, -5, 1], max: [5, 5, 1] } }],
        label: "overload",
        max_displacement_mm: 1e-6,
      },
      engine,
    );

    const { orderId, authorizationId } = await quotedAndApproved(docId, store, es, user);
    const res = await placeOrder(
      { order_id: orderId, authorization_id: authorizationId },
      store,
      es,
      user,
      engine,
    );
    expect(res.isError).toBe(true);
    expect(text(res)).toContain("receipt violated");
    expect(text(res)).toContain("physics.static.overload.displacement");
    const after = await store.getOrder(orderId, user.sub);
    expect(after?.state).toBe("QUOTED");
    expect(after?.receipt_status).toBe("violated");
    const blocked = es.events.find((e) => e.type === "order_blocked");
    expect((blocked!.payload as { reason?: string }).reason).toBe("receipt_violated");
  });

  it("a passing physics spec alone (no clearance specs) re-verifies to 'holds'", async () => {
    const user: AuthUser = { sub: "u-gate-phys-holds", email: "x@y.z" };
    const store = new InMemoryFabricateStore();
    const es = new RecordingEventStore();
    const docId = out(openDocument({ initial: rotorStatorDocument(5.0) })).document_id;
    predictPhysicsTool(
      {
        document_id: docId,
        part: "rotor",
        loads: [{ region: { min: [-5, -5, 9], max: [5, 5, 9] }, force: [100, 0, 0] }],
        supports: [{ region: { min: [-5, -5, 1], max: [5, 5, 1] } }],
        label: "tip-load",
        max_displacement_mm: 10,
      },
      engine,
    );

    const { orderId, authorizationId } = await quotedAndApproved(docId, store, es, user);
    const res = await placeOrder(
      { order_id: orderId, authorization_id: authorizationId },
      store,
      es,
      user,
      engine,
    );
    expect(res.isError).toBeFalsy();
    const body = out(res);
    expect(body.state).toBe("PAID");
    // The specs-presence check counts physics specs: this must NOT degrade to
    // "unverified" just because there are no clearance specs.
    expect(body.receipt.status).toBe("holds");
    expect((await store.getOrder(orderId, user.sub))?.receipt_status).toBe("holds");
  });

  it("consumed-authz replay skips the gates: a committed debit finalizes even after drift", async () => {
    const user: AuthUser = { sub: "u-gate-replay", email: "x@y.z" };
    const store = new InMemoryFabricateStore();
    const es = new RecordingEventStore();
    const docId = out(openDocument({ initial: rotorStatorDocument(5.0) })).document_id;
    const { orderId, authorizationId } = await quotedAndApproved(docId, store, es, user);

    // Simulate the crash window: the debit COMMITS (authz consumed) but the
    // PAID state write never lands — the order is still QUOTED.
    const debit = await store.debit({
      userId: user.sub,
      amountMinor: (await store.getOrder(orderId, user.sub))!.amount_total_minor,
      orderId,
      authorizationId,
      idempotencyKey: `${orderId}:debit`, // place_order's default key
    });
    expect(debit.ok).toBe(true);
    expect((await store.getAuthorization(authorizationId, user.sub))?.status).toBe("consumed");

    // The design drifts AFTER the debit — historically this stranded the
    // order at QUOTED forever (gate 1 refused every replay attempt).
    const doc = getSession(docId);
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (Object.values(doc.nodes).find((n) => n.name === "rotor-solid")!.op as any).radius = 5.5;

    const res = await placeOrder(
      { order_id: orderId, authorization_id: authorizationId },
      store,
      es,
      user,
      engine,
    );
    expect(res.isError).toBeFalsy();
    const body = out(res);
    expect(body.state).toBe("PAID");
    expect(body.idempotent).toBe(true); // replay-matched, no second debit
    expect(body.receipt.note).toContain("idempotent replay");
    expect((await store.getOrder(orderId, user.sub))?.state).toBe("PAID");
    // The gates never fired for the replay — no order_blocked on the spine.
    expect(es.events.some((e) => e.type === "order_blocked")).toBe(false);
  });

  it("hydrates the order's session before the gates (signed-in path has no args.document_id)", async () => {
    const user: AuthUser = { sub: "u-gate-hydrate", email: "x@y.z" };
    const store = new InMemoryFabricateStore();
    const es = new RecordingEventStore();
    const docId = out(openDocument({ initial: rotorStatorDocument(5.0) })).document_id;
    const { orderId, authorizationId } = await quotedAndApproved(docId, store, es, user);

    // The durable store holds a DRIFTED version of the design; the warm cache
    // is empty (fresh per-request scope / cold instance). Without hydration
    // the doc-hash gate silently skipped and the wallet was debited.
    const drifted = JSON.parse(JSON.stringify(getSession(docId))) as Document;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (Object.values(drifted.nodes).find((n) => n.name === "rotor-solid")!.op as any).radius = 6.2;
    const durable = new Map<string, Document>([[docId, drifted]]);
    const sessionStore: SessionStore = {
      scope: "user",
      load: async (id: string) => durable.get(id) ?? null,
      save: async () => {},
      drop: async () => {},
    };
    documents.clear(); // not resident — only the durable store has it

    const res = await placeOrder(
      { order_id: orderId, authorization_id: authorizationId },
      store,
      es,
      user,
      engine,
      sessionStore,
    );
    expect(res.isError).toBe(true);
    expect(text(res)).toContain("doc_hash mismatch");
    expect((await store.getOrder(orderId, user.sub))?.state).toBe("EXPIRED");
  });
});
