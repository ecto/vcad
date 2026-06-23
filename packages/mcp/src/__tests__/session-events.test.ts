import { describe, it, expect, beforeEach, afterEach } from "vitest";
import {
  SupabaseSessionEventStore,
  NoopSessionEventStore,
  createSessionEventStore,
  setSessionFetch,
  type SessionEvent,
} from "../session-store.js";

/**
 * In-memory stand-in for the `session_events` table + the
 * `append_session_event` RPC. Reproduces the RPC's contract — per-session
 * monotonic `seq`, per-(session, key) idempotent replay — so the store's real
 * request shaping (RPC body, GET filters) is exercised without a network or a
 * live Postgres.
 */
function makeEventsFake() {
  const sessions = new Map<string, Array<Record<string, unknown>>>();
  const seenBodies: Array<Record<string, unknown>> = [];
  let nextId = 1;

  const fetchImpl = (async (input: unknown, init: RequestInit = {}) => {
    const url = new URL(String(input));
    const method = (init.method ?? "GET").toUpperCase();

    // append_session_event RPC
    if (method === "POST" && url.pathname.endsWith("/rpc/append_session_event")) {
      const b = JSON.parse(String(init.body)) as Record<string, unknown>;
      seenBodies.push(b);
      const sid = String(b.p_session_id);
      const key = String(b.p_idempotency_key);
      const list = sessions.get(sid) ?? [];
      const existing = list.find((r) => r.idempotency_key === key);
      if (existing) {
        return jsonResponse({
          ok: true,
          idempotent: true,
          id: existing.id,
          seq: existing.seq,
        });
      }
      const seq = list.length + 1;
      const id = nextId++;
      const row = {
        id,
        seq,
        session_id: sid,
        user_id: b.p_user ?? null,
        author: b.p_author,
        kind: b.p_kind,
        type: b.p_type,
        payload: b.p_payload ?? {},
        idempotency_key: key,
        created_at: new Date(0).toISOString(),
      };
      list.push(row);
      sessions.set(sid, list);
      return jsonResponse({ ok: true, id, seq });
    }

    // GET /session_events?session_id=eq.X&order=seq.asc
    if (method === "GET" && url.pathname.endsWith("/session_events")) {
      const sidParam = url.searchParams.get("session_id");
      const sid = sidParam?.startsWith("eq.") ? sidParam.slice(3) : null;
      const rows = (sid && sessions.get(sid)) || [];
      return jsonResponse(rows);
    }

    return new Response("unexpected", { status: 400 });
  }) as unknown as typeof fetch;

  return { sessions, seenBodies, fetchImpl };
}

function jsonResponse(body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: { "Content-Type": "application/json" },
  });
}

const CFG = {
  supabaseUrl: "https://supa.test",
  serviceRoleKey: "service-role-key",
  userId: "user-me",
};

const kernelEvt = (overrides: Partial<SessionEvent> = {}): SessionEvent => ({
  author: "agent",
  kind: "kernel",
  type: "create",
  payload: { tool: "create", args: { kind: "cube" } },
  ...overrides,
});

afterEach(() => {
  setSessionFetch(((...args: Parameters<typeof fetch>) =>
    fetch(...args)) as typeof fetch);
});

describe("SupabaseSessionEventStore", () => {
  it("assigns per-session monotonic seq and lists in order", async () => {
    const fake = makeEventsFake();
    setSessionFetch(fake.fetchImpl);
    const store = new SupabaseSessionEventStore(CFG);

    await store.append("doc_a", kernelEvt({ type: "create" }));
    await store.append("doc_a", kernelEvt({ type: "update" }));
    await store.append("doc_a", kernelEvt({ type: "delete" }));

    const events = await store.list("doc_a");
    expect(events.map((e) => e.seq)).toEqual([1, 2, 3]);
    expect(events.map((e) => e.type)).toEqual(["create", "update", "delete"]);
  });

  it("replays idempotently on a repeated (session, key) — no new row", async () => {
    const fake = makeEventsFake();
    setSessionFetch(fake.fetchImpl);
    const store = new SupabaseSessionEventStore(CFG);

    await store.append("doc_a", kernelEvt({ idempotencyKey: "k1" }));
    await store.append("doc_a", kernelEvt({ idempotencyKey: "k1" }));

    const events = await store.list("doc_a");
    expect(events).toHaveLength(1);
    expect(events[0].seq).toBe(1);
  });

  it("scopes seq per session — independent counters", async () => {
    const fake = makeEventsFake();
    setSessionFetch(fake.fetchImpl);
    const store = new SupabaseSessionEventStore(CFG);

    await store.append("doc_a", kernelEvt());
    await store.append("doc_b", kernelEvt());
    await store.append("doc_b", kernelEvt());

    expect((await store.list("doc_a")).map((e) => e.seq)).toEqual([1]);
    expect((await store.list("doc_b")).map((e) => e.seq)).toEqual([1, 2]);
  });

  it("round-trips the payload and forwards the caller's user_id", async () => {
    const fake = makeEventsFake();
    setSessionFetch(fake.fetchImpl);
    const store = new SupabaseSessionEventStore(CFG);

    const payload = { tool: "update", args: { node: 3 }, changed: { modified: [{ part_id: "0" }] } };
    await store.append("doc_a", kernelEvt({ payload }));

    const [evt] = await store.list("doc_a");
    expect(evt.payload).toEqual(payload);
    // The RPC body carries the caller's user id, not tool input.
    expect(fake.seenBodies[0].p_user).toBe("user-me");
    expect(fake.seenBodies[0].p_kind).toBe("kernel");
  });

  it("generates an idempotency key when none is supplied", async () => {
    const fake = makeEventsFake();
    setSessionFetch(fake.fetchImpl);
    const store = new SupabaseSessionEventStore(CFG);

    await store.append("doc_a", kernelEvt());
    await store.append("doc_a", kernelEvt());

    // Distinct generated keys → two distinct rows.
    expect(await store.list("doc_a")).toHaveLength(2);
    const keys = fake.seenBodies.map((b) => b.p_idempotency_key);
    expect(keys[0]).not.toBe(keys[1]);
  });

  it("passes null user_id for an anonymous session", async () => {
    const fake = makeEventsFake();
    setSessionFetch(fake.fetchImpl);
    const store = new SupabaseSessionEventStore({ ...CFG, userId: null });
    await store.append("doc_anon", kernelEvt());
    expect(fake.seenBodies[0].p_user).toBeNull();
  });

  it("never throws on a transport error (best-effort append)", async () => {
    setSessionFetch((() => {
      throw new Error("network down");
    }) as unknown as typeof fetch);
    const store = new SupabaseSessionEventStore(CFG);
    await expect(store.append("doc_a", kernelEvt())).resolves.toBeUndefined();
  });
});

describe("NoopSessionEventStore", () => {
  it("append is a no-op and list is empty", async () => {
    const store = new NoopSessionEventStore();
    await expect(store.append("doc_a", kernelEvt())).resolves.toBeUndefined();
    expect(await store.list("doc_a")).toEqual([]);
  });
});

describe("createSessionEventStore factory", () => {
  let prevUrl: string | undefined;
  let prevKey: string | undefined;

  beforeEach(() => {
    prevUrl = process.env.SUPABASE_URL;
    prevKey = process.env.SUPABASE_SERVICE_ROLE_KEY;
  });
  afterEach(() => {
    if (prevUrl === undefined) delete process.env.SUPABASE_URL;
    else process.env.SUPABASE_URL = prevUrl;
    if (prevKey === undefined) delete process.env.SUPABASE_SERVICE_ROLE_KEY;
    else process.env.SUPABASE_SERVICE_ROLE_KEY = prevKey;
  });

  it("returns the Supabase spine with url + key (signed-in or anon)", () => {
    process.env.SUPABASE_URL = "https://supa.test/";
    process.env.SUPABASE_SERVICE_ROLE_KEY = "k";
    expect(createSessionEventStore(null)).toBeInstanceOf(SupabaseSessionEventStore);
    expect(
      createSessionEventStore({ sub: "user-me", email: "a@b.c" }),
    ).toBeInstanceOf(SupabaseSessionEventStore);
  });

  it("returns the no-op spine without Supabase env (stdio/local)", () => {
    delete process.env.SUPABASE_URL;
    delete process.env.SUPABASE_SERVICE_ROLE_KEY;
    expect(createSessionEventStore(null)).toBeInstanceOf(NoopSessionEventStore);
  });
});
