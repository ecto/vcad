/**
 * Durable backing store for MCP document sessions.
 *
 * WHY: the hosted MCP server runs as a Vercel serverless function, so the
 * module-global `documents` Map in tools/session.ts is per-instance — a cold
 * start, or a request routed to a different instance, loses every open session
 * and surfaces as "Unknown document_id" mid-build. This store turns that Map
 * into a warm-instance CACHE in front of a durable backend keyed by
 * (user, document): the dispatch layer hydrates the cache on a miss and
 * persists after a write, so a cold instance rehydrates the board instead of
 * throwing.
 *
 * When a signed-in user AND a Supabase service-role key are present, sessions
 * persist to the cloud `documents` table — the same table the web app syncs to,
 * so an agent's work also shows up at vcad.io. Otherwise (local stdio, or an
 * anonymous hosted call during the pre-MCP_REQUIRE_AUTH transition) the
 * in-memory impl reproduces today's behavior exactly.
 */
import { randomUUID } from "node:crypto";
import type { Document } from "@vcad/ir";
import type { AuthUser } from "./oauth.js";

export interface SessionStore {
  /** Fetch a session's Document, or null on miss. Never throws for
   *  not-found; transport/Supabase errors are logged and surfaced as null so
   *  an outage degrades to "cache only", not a tool failure. */
  load(documentId: string): Promise<Document | null>;
  /** Create-or-update the durable row for this session. Idempotent on
   *  (user_id, local_id). Best-effort: errors are logged, never thrown. */
  save(documentId: string, doc: Document, name?: string): Promise<void>;
  /** Forget the durable row (close_document). Best-effort. */
  drop(documentId: string): Promise<void>;
}

/**
 * No-op store = today's behavior: the `documents` cache is the source of truth,
 * load always misses (so getSession throws "Unknown document_id" exactly as
 * before), save/drop do nothing. Used for stdio and anonymous HTTP calls.
 */
export class InMemorySessionStore implements SessionStore {
  async load(): Promise<Document | null> {
    return null;
  }
  async save(): Promise<void> {
    /* the in-memory cache is the source of truth */
  }
  async drop(): Promise<void> {
    /* nothing durable to forget */
  }
}

/**
 * Low-level fetch seam, mirrors oauth.ts's injectable `supabaseExchange` so
 * SupabaseSessionStore can be unit-tested without a network or
 * @supabase/supabase-js.
 */
export let sessionFetch: typeof fetch = (...args) => fetch(...args);
/** Test hook — mirrors setSupabaseExchange in oauth.ts. */
export function setSessionFetch(fn: typeof fetch): void {
  sessionFetch = fn;
}

export interface SupabaseStoreConfig {
  /** SUPABASE_URL, trailing slash already stripped. */
  supabaseUrl: string;
  /** SUPABASE_SERVICE_ROLE_KEY — server-only; never reaches a client bundle. */
  serviceRoleKey: string;
  /** Caller's Supabase user id (access-token `sub` = auth.users.id). */
  userId: string;
}

/**
 * Cloud-backed store via raw PostgREST fetch with the service-role key.
 *
 * RLS is bypassed by the service role, so ownership is enforced in code: every
 * query filters `user_id=eq.<caller>` and every write sets `user_id` to the
 * caller — a caller can only ever touch their own MCP sessions.
 *
 * The `content` column holds the RAW Document IR. The web app's
 * `cloudContentToVcadFile` recognizes `{ nodes, roots }` as a legacy IR
 * document and renders it natively, so an MCP-authored board opens at
 * vcad.io — it isn't just a dead row.
 */
export class SupabaseSessionStore implements SessionStore {
  /**
   * Per-session monotonic version. The `version` column is int4, so a
   * timestamp can't be used (Date.now() overflows). A cold instance restarts
   * the counter at 1; document_versions has no unique on version_number
   * (migration 002), so a duplicate history number is cosmetic, not a fault.
   */
  private versions = new Map<string, number>();

  constructor(private cfg: SupabaseStoreConfig) {}

  /** `mcp:` prefix guarantees an MCP session can never collide with — or
   *  upsert over — a web-app/IndexedDB row (those use bare UUIDs). */
  private localId(documentId: string): string {
    return `mcp:${documentId}`;
  }

  private rowsUrl(query = ""): string {
    return `${this.cfg.supabaseUrl}/rest/v1/documents${query}`;
  }

  private headers(extra: Record<string, string> = {}): Record<string, string> {
    return {
      apikey: this.cfg.serviceRoleKey,
      Authorization: `Bearer ${this.cfg.serviceRoleKey}`,
      "Content-Type": "application/json",
      ...extra,
    };
  }

  /** `?user_id=eq.<caller>&local_id=eq.mcp:<id>` — the ownership-scoped key. */
  private scope(documentId: string): string {
    const uid = encodeURIComponent(this.cfg.userId);
    const lid = encodeURIComponent(this.localId(documentId));
    return `?user_id=eq.${uid}&local_id=eq.${lid}`;
  }

  async load(documentId: string): Promise<Document | null> {
    try {
      const res = await sessionFetch(
        this.rowsUrl(`${this.scope(documentId)}&select=content&limit=1`),
        {
          method: "GET",
          headers: this.headers({ Accept: "application/vnd.pgrst.object+json" }),
        },
      );
      if (!res.ok) return null; // 406 = zero/≠1 rows → treat as a miss
      const row = (await res.json()) as { content?: unknown };
      const ir = unwrapDocument(row?.content);
      // Defensive copy — match openDocument's discipline so a caller can't
      // mutate the cached doc through a retained reference.
      return ir ? (JSON.parse(JSON.stringify(ir)) as Document) : null;
    } catch (err) {
      console.error("[session-store] load failed:", err);
      return null;
    }
  }

  async save(documentId: string, doc: Document, name?: string): Promise<void> {
    try {
      const version = (this.versions.get(documentId) ?? 0) + 1;
      this.versions.set(documentId, version);
      const body = [
        {
          user_id: this.cfg.userId, // always the caller — never tool input
          local_id: this.localId(documentId),
          name: name ?? "MCP session",
          content: doc, // raw IR → the app renders it as a legacy document
          version,
          device_modified_at: Date.now(),
        },
      ];
      const res = await sessionFetch(
        this.rowsUrl("?on_conflict=user_id,local_id"),
        {
          method: "POST",
          headers: this.headers({
            Prefer: "resolution=merge-duplicates,return=minimal",
          }),
          body: JSON.stringify(body),
        },
      );
      if (!res.ok) {
        console.error(
          "[session-store] save failed:",
          res.status,
          await res.text().catch(() => ""),
        );
      }
    } catch (err) {
      console.error("[session-store] save failed:", err);
    }
  }

  async drop(documentId: string): Promise<void> {
    try {
      await sessionFetch(this.rowsUrl(this.scope(documentId)), {
        method: "DELETE",
        headers: this.headers(),
      });
    } catch (err) {
      console.error("[session-store] drop failed:", err);
    }
  }
}

/**
 * Cloud-backed store for ANONYMOUS sessions, keyed by the (unguessable)
 * document id — capability access, no user. Backed by the `mcp_sessions` table
 * (service-role only). Same hydrate-on-miss / persist-after-write contract as
 * SupabaseSessionStore, so anonymous callers survive serverless cold starts and
 * cross-instance routing exactly like signed-in users do.
 */
export class AnonSupabaseSessionStore implements SessionStore {
  constructor(private cfg: { supabaseUrl: string; serviceRoleKey: string }) {}

  private headers(extra: Record<string, string> = {}): Record<string, string> {
    return {
      apikey: this.cfg.serviceRoleKey,
      Authorization: `Bearer ${this.cfg.serviceRoleKey}`,
      "Content-Type": "application/json",
      ...extra,
    };
  }

  private url(query = ""): string {
    return `${this.cfg.supabaseUrl}/rest/v1/mcp_sessions${query}`;
  }

  async load(documentId: string): Promise<Document | null> {
    try {
      const res = await sessionFetch(
        this.url(
          `?document_id=eq.${encodeURIComponent(documentId)}&select=content&limit=1`,
        ),
        {
          method: "GET",
          headers: this.headers({ Accept: "application/vnd.pgrst.object+json" }),
        },
      );
      if (!res.ok) return null; // 406 = zero/≠1 rows → miss
      const row = (await res.json()) as { content?: unknown };
      const ir = unwrapDocument(row?.content);
      return ir ? (JSON.parse(JSON.stringify(ir)) as Document) : null;
    } catch (err) {
      console.error("[session-store] anon load failed:", err);
      return null;
    }
  }

  async save(documentId: string, doc: Document): Promise<void> {
    try {
      const res = await sessionFetch(this.url("?on_conflict=document_id"), {
        method: "POST",
        headers: this.headers({
          Prefer: "resolution=merge-duplicates,return=minimal",
        }),
        body: JSON.stringify([{ document_id: documentId, content: doc }]),
      });
      if (!res.ok) {
        console.error(
          "[session-store] anon save failed:",
          res.status,
          await res.text().catch(() => ""),
        );
      }
    } catch (err) {
      console.error("[session-store] anon save failed:", err);
    }
  }

  async drop(documentId: string): Promise<void> {
    try {
      await sessionFetch(
        this.url(`?document_id=eq.${encodeURIComponent(documentId)}`),
        { method: "DELETE", headers: this.headers() },
      );
    } catch (err) {
      console.error("[session-store] anon drop failed:", err);
    }
  }
}

/**
 * Resolve a public share token (`document_shares.token`) to its cloud document
 * content + name, via the `get_shared_document` SECURITY DEFINER RPC — the same
 * path the web app's `fetchSharedDocument` uses. This is the server side of the
 * "Continue in Claude" handoff: the browser mints a share token for the part the
 * user is looking at, and `continue_document` resolves it here. Service-role
 * keyed (the RPC is granted to anon/authenticated, but we already hold the key
 * and it avoids spinning a second client). Returns null on miss / invalid token
 * / no env.
 *
 * `content` is the RAW stored content — a CRDT state blob (`{replica_id, ops}`),
 * a raw IR Document (`{nodes, roots}`), or a loon envelope — so the caller
 * materializes it to IR (CRDT needs the kernel engine).
 */
export async function resolveShareToken(
  token: string,
): Promise<{ content: unknown; name: string } | null> {
  const url = (process.env.SUPABASE_URL || "").replace(/\/+$/, "");
  const key = process.env.SUPABASE_SERVICE_ROLE_KEY || "";
  if (!url || !key) return null;
  // The RPC param is typed `uuid`; a non-uuid token 400s. Cheap guard so a
  // malformed handoff degrades to a clean miss, not a logged PostgREST error.
  if (
    !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(token)
  ) {
    return null;
  }
  try {
    const res = await sessionFetch(`${url}/rest/v1/rpc/get_shared_document`, {
      method: "POST",
      headers: {
        apikey: key,
        Authorization: `Bearer ${key}`,
        "Content-Type": "application/json",
      },
      body: JSON.stringify({ p_token: token }),
    });
    if (!res.ok) return null;
    const body = (await res.json()) as
      | Array<{ name?: string; content?: unknown }>
      | { name?: string; content?: unknown };
    const row = Array.isArray(body) ? body[0] : body;
    if (!row || row.content == null) return null;
    return {
      content: row.content,
      name: typeof row.name === "string" ? row.name : "Shared document",
    };
  } catch (err) {
    console.error("[session-store] resolveShareToken failed:", err);
    return null;
  }
}

/** True when `content` looks like a raw Document IR (has nodes + roots). */
function looksLikeDocument(content: unknown): content is Document {
  return (
    !!content &&
    typeof content === "object" &&
    "nodes" in (content as object) &&
    "roots" in (content as object)
  );
}

/** Unwrap a stored `content` blob into a Document IR. Raw IR is stored
 *  directly; tolerate a legacy `{ ir }` envelope defensively. */
function unwrapDocument(content: unknown): Document | null {
  if (looksLikeDocument(content)) return content;
  const env = content as { ir?: unknown } | null;
  if (env && looksLikeDocument(env.ir)) return env.ir;
  return null;
}

/**
 * Choose the store impl from env + the per-connection user. With Supabase env
 * present: a signed-in user gets the user-owned `documents` store (also renders
 * at vcad.io); an anonymous caller gets the capability-keyed `mcp_sessions`
 * store (durable across instances, the unguessable id is the capability). With
 * no Supabase env (stdio / local) it's the in-memory no-op store — today's
 * behavior. Either way the service-role key only loads when the env provides it.
 */
export function createSessionStore(user: AuthUser | null): SessionStore {
  const url = (process.env.SUPABASE_URL || "").replace(/\/+$/, "");
  const key = process.env.SUPABASE_SERVICE_ROLE_KEY || "";
  if (url && key) {
    return user
      ? new SupabaseSessionStore({ supabaseUrl: url, serviceRoleKey: key, userId: user.sub })
      : new AnonSupabaseSessionStore({ supabaseUrl: url, serviceRoleKey: key });
  }
  return new InMemorySessionStore();
}

// ─── Event spine — the per-session append-only log (migration 028) ───────────
//
// State = fold(log); the content snapshot the SessionStore writes is a derived
// materialization. This store is the canonical record: a kernel mutation, an
// overlay annotation, or a control event each appends one row, which the DB
// fans out over Realtime. Best-effort throughout (mirrors SessionStore): a
// failed append must never turn a successful tool call into an error.

/** An event to append. `idempotencyKey` is generated when omitted. */
export interface SessionEvent {
  /** Emitter: a user sub, the literal "agent", or "human". */
  author: string;
  /** kernel = folds into geometry; overlay = annotation; control = lifecycle. */
  kind: "kernel" | "overlay" | "control";
  /** Fine type: the tool name for kernel, "pin"/"flag" for overlay, etc. */
  type: string;
  /** kernel: {tool, args, changed?}; overlay: {anchor, text, …}. */
  payload: Record<string, unknown>;
  /** Per-session idempotency key; a random uuid is used when omitted. */
  idempotencyKey?: string;
}

/** A row read back from `session_events`. */
export interface StoredSessionEvent {
  id: number;
  seq: number;
  session_id: string;
  author: string;
  kind: string;
  type: string;
  payload: Record<string, unknown>;
  created_at: string;
}

export interface SessionEventStore {
  /** Append one event. Best-effort: errors are logged, never thrown. */
  append(sessionId: string, evt: SessionEvent): Promise<void>;
  /** Read a session's events in seq order (replay / live window). With
   *  `sinceSeq`, only events after it — the backend filters server-side so a
   *  late-join catch-up doesn't pull the whole log. */
  list(sessionId: string, sinceSeq?: number): Promise<StoredSessionEvent[]>;
}

/** No-op store = stdio/local: nothing durable, list is empty. */
export class NoopSessionEventStore implements SessionEventStore {
  async append(): Promise<void> {
    /* no spine without Supabase env */
  }
  async list(): Promise<StoredSessionEvent[]> {
    return [];
  }
}

/**
 * Cloud-backed spine. Appends via the `append_session_event` RPC (the sole
 * writer; the table grants service_role SELECT only), reads the table directly.
 * The service role bypasses RLS, so `list` sees every row for a session
 * regardless of ownership — the unguessable session_id is the capability.
 */
export class SupabaseSessionEventStore implements SessionEventStore {
  constructor(
    private cfg: {
      supabaseUrl: string;
      serviceRoleKey: string;
      /** Caller's user id, or null for an anonymous capability session. */
      userId: string | null;
    },
  ) {}

  private headers(extra: Record<string, string> = {}): Record<string, string> {
    return {
      apikey: this.cfg.serviceRoleKey,
      Authorization: `Bearer ${this.cfg.serviceRoleKey}`,
      "Content-Type": "application/json",
      ...extra,
    };
  }

  async append(sessionId: string, evt: SessionEvent): Promise<void> {
    try {
      const res = await sessionFetch(
        `${this.cfg.supabaseUrl}/rest/v1/rpc/append_session_event`,
        {
          method: "POST",
          headers: this.headers(),
          body: JSON.stringify({
            p_session_id: sessionId,
            p_user: this.cfg.userId,
            p_author: evt.author,
            p_kind: evt.kind,
            p_type: evt.type,
            p_payload: evt.payload ?? {},
            p_idempotency_key: evt.idempotencyKey ?? randomUUID(),
          }),
        },
      );
      if (!res.ok) {
        console.error(
          "[session-events] append failed:",
          res.status,
          await res.text().catch(() => ""),
        );
      }
    } catch (err) {
      console.error("[session-events] append failed:", err);
    }
  }

  async list(sessionId: string, sinceSeq?: number): Promise<StoredSessionEvent[]> {
    try {
      const sinceFilter =
        typeof sinceSeq === "number" && Number.isFinite(sinceSeq)
          ? `&seq=gt.${Math.trunc(sinceSeq)}`
          : "";
      const res = await sessionFetch(
        `${this.cfg.supabaseUrl}/rest/v1/session_events` +
          `?session_id=eq.${encodeURIComponent(sessionId)}` +
          sinceFilter +
          `&order=seq.asc` +
          `&select=id,seq,session_id,author,kind,type,payload,created_at`,
        { method: "GET", headers: this.headers() },
      );
      if (!res.ok) return [];
      return (await res.json()) as StoredSessionEvent[];
    } catch (err) {
      console.error("[session-events] list failed:", err);
      return [];
    }
  }
}

/**
 * Choose the spine impl from env + user. With Supabase env present, both
 * signed-in and anonymous sessions append via the same RPC (it takes a nullable
 * user). Without it (stdio/local) the no-op store reproduces today's behavior.
 */
export function createSessionEventStore(
  user: AuthUser | null,
): SessionEventStore {
  const url = (process.env.SUPABASE_URL || "").replace(/\/+$/, "");
  const key = process.env.SUPABASE_SERVICE_ROLE_KEY || "";
  if (url && key) {
    return new SupabaseSessionEventStore({
      supabaseUrl: url,
      serviceRoleKey: key,
      userId: user?.sub ?? null,
    });
  }
  return new NoopSessionEventStore();
}

// ─── Live-window share gate (migration 029) ──────────────────────────────────
//
// Sessions are PRIVATE by default. The /live/* routes work only after the
// driver explicitly shares the session (share_session writes a live_shares
// row); without a row they 404 even with VCAD_LIVE_WINDOW on. Revocable.

/** The active share row, or null when a session isn't shared. */
export interface ShareRecord {
  /** The driver who shared it, or null for an anonymous capability session. */
  shared_by: string | null;
}

export interface ShareStore {
  /** The active share row for a session, or null if it isn't shared. The
   *  owner (shared_by) scopes the geometry resolve so a link-holder can only
   *  ever see the actual sharer's document. */
  getShare(sessionId: string): Promise<ShareRecord | null>;
  /** Mark a session shared (idempotent). `sharedBy` = the driver's user id. */
  share(sessionId: string, sharedBy: string | null): Promise<void>;
  /** Revoke the share — the live link goes dead. */
  unshare(sessionId: string): Promise<void>;
}

/** No-op = stdio/local: nothing is shareable (no live window without Supabase). */
export class NoopShareStore implements ShareStore {
  async getShare(): Promise<ShareRecord | null> {
    return null;
  }
  async share(): Promise<void> {}
  async unshare(): Promise<void> {}
}

/** Cloud-backed share gate over the `live_shares` table (service role). */
export class SupabaseShareStore implements ShareStore {
  constructor(private cfg: { supabaseUrl: string; serviceRoleKey: string }) {}

  private headers(extra: Record<string, string> = {}): Record<string, string> {
    return {
      apikey: this.cfg.serviceRoleKey,
      Authorization: `Bearer ${this.cfg.serviceRoleKey}`,
      "Content-Type": "application/json",
      ...extra,
    };
  }
  private url(query = ""): string {
    return `${this.cfg.supabaseUrl}/rest/v1/live_shares${query}`;
  }

  async getShare(sessionId: string): Promise<ShareRecord | null> {
    try {
      const res = await sessionFetch(
        this.url(`?session_id=eq.${encodeURIComponent(sessionId)}&select=session_id,shared_by&limit=1`),
        { method: "GET", headers: this.headers() },
      );
      if (!res.ok) return null;
      const rows = (await res.json()) as Array<{ shared_by?: string | null }>;
      return Array.isArray(rows) && rows[0]
        ? { shared_by: rows[0].shared_by ?? null }
        : null;
    } catch (err) {
      console.error("[live-share] getShare failed:", err);
      return null;
    }
  }

  async share(sessionId: string, sharedBy: string | null): Promise<void> {
    try {
      const res = await sessionFetch(this.url("?on_conflict=session_id"), {
        method: "POST",
        headers: this.headers({
          Prefer: "resolution=merge-duplicates,return=minimal",
        }),
        body: JSON.stringify([{ session_id: sessionId, shared_by: sharedBy }]),
      });
      if (!res.ok) {
        console.error(
          "[live-share] share failed:",
          res.status,
          await res.text().catch(() => ""),
        );
      }
    } catch (err) {
      console.error("[live-share] share failed:", err);
    }
  }

  async unshare(sessionId: string): Promise<void> {
    try {
      await sessionFetch(
        this.url(`?session_id=eq.${encodeURIComponent(sessionId)}`),
        { method: "DELETE", headers: this.headers() },
      );
    } catch (err) {
      console.error("[live-share] unshare failed:", err);
    }
  }
}

/** Share gate from env — Supabase-backed when configured, else a no-op. */
export function createShareStore(): ShareStore {
  const url = (process.env.SUPABASE_URL || "").replace(/\/+$/, "");
  const key = process.env.SUPABASE_SERVICE_ROLE_KEY || "";
  return url && key
    ? new SupabaseShareStore({ supabaseUrl: url, serviceRoleKey: key })
    : new NoopShareStore();
}

/**
 * Resolve a session's Document IR by session id ALONE, via the service role —
 * for the capability-keyed live geometry endpoint, which has no logged-in user.
 * Tries the anonymous `mcp_sessions` table first, then the user-owned
 * `documents` table by its `mcp:<id>` local_id with NO user filter (the share
 * gate + unguessable id are the capability). Returns null on miss / no env.
 */
export async function resolveSessionIr(
  sessionId: string,
  owner?: string | null,
): Promise<Document | null> {
  const url = (process.env.SUPABASE_URL || "").replace(/\/+$/, "");
  const key = process.env.SUPABASE_SERVICE_ROLE_KEY || "";
  if (!url || !key) return null;
  const headers = { apikey: key, Authorization: `Bearer ${key}` };
  const firstContent = async (endpoint: string): Promise<Document | null> => {
    try {
      const res = await sessionFetch(`${url}${endpoint}`, { method: "GET", headers });
      if (!res.ok) return null;
      const rows = (await res.json()) as Array<{ content?: unknown }>;
      const ir = Array.isArray(rows) && rows[0] ? unwrapDocument(rows[0].content) : null;
      return ir ? (JSON.parse(JSON.stringify(ir)) as Document) : null;
    } catch (err) {
      console.error("[resolveSessionIr] read failed:", err);
      return null;
    }
  };
  const sid = encodeURIComponent(sessionId);
  const anon = await firstContent(
    `/rest/v1/mcp_sessions?document_id=eq.${sid}&select=content&limit=1`,
  );
  if (anon) return anon;
  // The documents fallback MUST be scoped to the sharer: local_id is the
  // client-supplied IndexedDB id (unique only per user_id), so a global
  // local_id='mcp:<id>' match would let any signed-in user spoof a shared
  // session's geometry by syncing a doc with that id. Resolve only the owner's.
  if (!owner) return null;
  const lid = encodeURIComponent(`mcp:${sessionId}`);
  const uid = encodeURIComponent(owner);
  return firstContent(
    `/rest/v1/documents?local_id=eq.${lid}&user_id=eq.${uid}&select=content&order=device_modified_at.desc&limit=1`,
  );
}
