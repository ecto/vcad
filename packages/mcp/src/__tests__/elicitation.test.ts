import { describe, it, expect, beforeAll, beforeEach, afterEach } from "vitest";
import { Engine } from "@vcad/engine";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { ElicitRequestSchema } from "@modelcontextprotocol/sdk/types.js";
import { createServer } from "../server.js";
import { documents } from "../tools/session.js";
import { InMemoryFabricateStore } from "../fabricate/store.js";
import type { Order } from "../fabricate/types.js";

/**
 * M3 URL-mode elicitation through the REAL server pipeline: the elicit bridge
 * injected by createServer detects the client's `elicitation.url` capability
 * at call time and carries the spend-approval page to the human in-band.
 * decline ⇒ the authorization is revoked; cancel ⇒ the proposal stands;
 * no capability ⇒ the bridge reports unsupported and the out-of-band note is
 * returned (the dock is the floor, elicitation is the accelerator).
 *
 * Anonymous connections use the in-memory fabricate store, whose state is
 * module-global — seeding an order under owner "local" from the test makes it
 * visible to the server's own store instance.
 */

let engine: Engine;

beforeAll(async () => {
  engine = await Engine.init();
});

const OWNER = "local"; // ownerId(null) — anonymous connection scope

function makeOrder(orderId: string): Order {
  const now = new Date().toISOString();
  return {
    order_id: orderId,
    document_id: "doc_elicit",
    quote_id: `q_${orderId}`,
    state: "QUOTED",
    fab: "digitalmetal",
    fab_order_ref: null,
    amount_total_minor: 5000,
    currency: "USD",
    ship_to: null,
    events: [{ state: "QUOTED", at: now, note: "quote" }],
    created_at: now,
    updated_at: now,
  };
}

interface ElicitSeen {
  mode?: string;
  message?: string;
  url?: string;
  elicitationId?: string;
}

async function connectWith(
  action: "decline" | "cancel" | null,
  seen: ElicitSeen[],
) {
  const server = await createServer(engine, { user: null });
  const [clientT, serverT] = InMemoryTransport.createLinkedPair();
  const client = new Client(
    { name: "test", version: "0.0.0" },
    // null = a client that never declared the elicitation capability.
    { capabilities: action ? { elicitation: { url: {} } } : {} },
  );
  if (action) {
    client.setRequestHandler(ElicitRequestSchema, async (req) => {
      const p = req.params as ElicitSeen;
      seen.push({
        mode: p.mode,
        message: p.message,
        url: p.url,
        elicitationId: p.elicitationId,
      });
      return { action };
    });
  }
  await Promise.all([client.connect(clientT), server.connect(serverT)]);
  return { client, server };
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
const body = (r: unknown): any =>
  JSON.parse((r as { content: Array<{ text: string }> }).content[0].text);

describe("authorize_spend URL elicitation (capability-detected, fail-open to out-of-band)", () => {
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

  it("decline revokes the authorization (nothing can ever be charged against it)", async () => {
    const store = new InMemoryFabricateStore();
    await store.saveOrder(makeOrder("ord_elicit_decline"), 4000, OWNER);
    const seen: ElicitSeen[] = [];
    const { client, server } = await connectWith("decline", seen);

    const res = await client.callTool({
      name: "authorize_spend",
      arguments: { order_id: "ord_elicit_decline" },
    });
    expect(res.isError ?? false).toBe(false);
    const out = body(res);
    expect(out.status).toBe("revoked");
    expect(out.note).toContain("Declined");

    // The elicitation was URL-mode with the approval deep link, keyed by the
    // authorization id (the completion-notification handle).
    expect(seen).toHaveLength(1);
    expect(seen[0].mode).toBe("url");
    expect(seen[0].url).toBe(`https://vcad.io/authorize/${out.authorization_id}`);
    expect(seen[0].elicitationId).toBe(out.authorization_id);
    expect(seen[0].message).toContain("$50.00");

    // The DB row is the truth: revoked, and place_order refuses it.
    expect((await store.getAuthorization(out.authorization_id, OWNER))?.status).toBe(
      "revoked",
    );
    const placed = await client.callTool({
      name: "place_order",
      arguments: {
        order_id: "ord_elicit_decline",
        authorization_id: out.authorization_id,
      },
    });
    expect(placed.isError).toBe(true);
    expect(body(placed).error).toContain("revoked");

    await client.close();
    await server.close();
  });

  it("cancel leaves the proposal pending (a dismissed prompt is not a decision)", async () => {
    const store = new InMemoryFabricateStore();
    await store.saveOrder(makeOrder("ord_elicit_cancel"), 4000, OWNER);
    const seen: ElicitSeen[] = [];
    const { client, server } = await connectWith("cancel", seen);

    const res = await client.callTool({
      name: "authorize_spend",
      arguments: { order_id: "ord_elicit_cancel" },
    });
    const out = body(res);
    expect(seen).toHaveLength(1); // the elicitation WAS attempted
    expect(out.status).toBe("pending_human");
    expect(out.note).toContain("HUMAN must approve");
    expect(
      (await store.getAuthorization(out.authorization_id, OWNER))?.status,
    ).toBe("pending_human");

    await client.close();
    await server.close();
  });

  it("without the elicitation.url capability the bridge stays quiet (out-of-band note)", async () => {
    const store = new InMemoryFabricateStore();
    await store.saveOrder(makeOrder("ord_elicit_nocap"), 4000, OWNER);
    const seen: ElicitSeen[] = [];
    const { client, server } = await connectWith(null, seen);

    const res = await client.callTool({
      name: "authorize_spend",
      arguments: { order_id: "ord_elicit_nocap" },
    });
    const out = body(res);
    expect(seen).toHaveLength(0); // urlSupported() was false — never attempted
    expect(out.status).toBe("pending_human");
    expect(out.note).toContain("HUMAN must approve");
    expect(
      (await store.getAuthorization(out.authorization_id, OWNER))?.status,
    ).toBe("pending_human");

    await client.close();
    await server.close();
  });
});
