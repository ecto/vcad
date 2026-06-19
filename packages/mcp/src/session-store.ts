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
