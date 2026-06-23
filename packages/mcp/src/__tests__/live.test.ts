import { describe, it, expect } from "vitest";
import { appendOverlay, listEvents, OVERLAY_TYPES } from "../tools/live.js";
import type {
  SessionEvent,
  SessionEventStore,
  StoredSessionEvent,
} from "../session-store.js";

class FakeEventStore implements SessionEventStore {
  public appended: Array<{ sessionId: string } & SessionEvent> = [];
  public lastSince: number | undefined;
  private rows: StoredSessionEvent[] = [];
  seed(rows: StoredSessionEvent[]): void {
    this.rows = rows;
  }
  async append(sessionId: string, evt: SessionEvent): Promise<void> {
    this.appended.push({ sessionId, ...evt });
  }
  async list(_sessionId: string, sinceSeq?: number): Promise<StoredSessionEvent[]> {
    this.lastSince = sinceSeq;
    return this.rows;
  }
}

const row = (seq: number, over: Partial<StoredSessionEvent> = {}): StoredSessionEvent => ({
  id: seq,
  seq,
  session_id: "doc_x",
  author: "agent",
  kind: "kernel",
  type: "create",
  payload: {},
  created_at: new Date(0).toISOString(),
  ...over,
});

describe("appendOverlay", () => {
  it("appends a valid pin as a kind:'overlay' event with the anchor payload", async () => {
    const es = new FakeEventStore();
    const res = await appendOverlay(es, "doc_x", {
      type: "pin",
      payload: { anchor: { node: 3, face: 1 }, text: "too thin" },
      author: "reviewer",
    });
    expect(res.ok).toBe(true);
    expect(es.appended).toHaveLength(1);
    const evt = es.appended[0];
    expect(evt.kind).toBe("overlay");
    expect(evt.type).toBe("pin");
    // An untrusted (anonymous) author is namespaced so it can't impersonate.
    expect(evt.author).toBe("viewer:reviewer");
    expect(evt.payload).toEqual({ anchor: { node: 3, face: 1 }, text: "too thin" });
  });

  it("namespaces an anonymous author as viewer:<name> and caps it", async () => {
    const es = new FakeEventStore();
    await appendOverlay(es, "doc_x", { type: "flag", payload: {} });
    expect(es.appended[0].author).toBe("viewer:anon");

    const es2 = new FakeEventStore();
    await appendOverlay(es2, "doc_x", { type: "note", payload: {}, author: "x".repeat(200) });
    expect(es2.appended[0].author).toBe("viewer:" + "x".repeat(48));
  });

  it("never lets an anonymous author impersonate a reserved identity", async () => {
    const es = new FakeEventStore();
    await appendOverlay(es, "doc_x", { type: "pin", payload: {}, author: "agent" });
    expect(es.appended[0].author).toBe("viewer:agent"); // not the reserved "agent"
  });

  it("uses the verified identity authoritatively, ignoring a forged body author", async () => {
    const es = new FakeEventStore();
    await appendOverlay(
      es,
      "doc_x",
      { type: "pin", payload: {}, author: "victim@x.z" },
      { trustedAuthor: "alice@x.z" },
    );
    expect(es.appended[0].author).toBe("alice@x.z");
  });

  it("rejects an oversized payload (no spine bloat / broadcast amplification)", async () => {
    const es = new FakeEventStore();
    const res = await appendOverlay(es, "doc_x", {
      type: "pin",
      payload: { blob: "x".repeat(5000) },
    });
    expect(res.ok).toBe(false);
    if (!res.ok) expect(res.error).toContain("too large");
    expect(es.appended).toHaveLength(0);
  });

  it("rejects an unknown overlay type (never reaches the store)", async () => {
    const es = new FakeEventStore();
    const res = await appendOverlay(es, "doc_x", { type: "mutation", payload: {} });
    expect(res.ok).toBe(false);
    expect(es.appended).toHaveLength(0);
  });

  it("rejects a missing session id", async () => {
    const es = new FakeEventStore();
    const res = await appendOverlay(es, "", { type: "pin", payload: {} });
    expect(res.ok).toBe(false);
  });

  it("tolerates a non-object payload", async () => {
    const es = new FakeEventStore();
    await appendOverlay(es, "doc_x", { type: "pin", payload: "oops" as unknown });
    expect(es.appended[0].payload).toEqual({});
  });

  it("accepts every declared overlay type", async () => {
    for (const t of OVERLAY_TYPES) {
      const es = new FakeEventStore();
      const res = await appendOverlay(es, "doc_x", { type: t, payload: {} });
      expect(res.ok).toBe(true);
    }
  });
});

describe("listEvents", () => {
  it("returns all events in seq order", async () => {
    const es = new FakeEventStore();
    es.seed([row(1), row(2), row(3)]);
    const out = await listEvents(es, "doc_x");
    expect(out.map((e) => e.seq)).toEqual([1, 2, 3]);
  });

  it("returns only events after sinceSeq (late-join catch-up)", async () => {
    const es = new FakeEventStore();
    es.seed([row(1), row(2), row(3), row(4)]);
    const out = await listEvents(es, "doc_x", 2);
    expect(out.map((e) => e.seq)).toEqual([3, 4]);
  });

  it("pushes sinceSeq down to the store (server-side filter)", async () => {
    const es = new FakeEventStore();
    es.seed([row(3), row(4)]);
    await listEvents(es, "doc_x", 2);
    expect(es.lastSince).toBe(2);
  });

  it("returns [] for a missing session id", async () => {
    const es = new FakeEventStore();
    expect(await listEvents(es, "")).toEqual([]);
  });
});
