/**
 * Document session management for MCP. Mirrors the gym tools' pattern
 * (`simulations: Map<string, PhysicsEnv>` in tools/gym.ts) — each session
 * holds a `Document` that subsequent CRUD calls mutate in place.
 *
 * The chat surface in the web app does the same thing implicitly via the
 * Zustand `useDocumentStore`. MCP needs an explicit handle because each
 * call is stateless across the wire.
 */

import { createDocument } from "@vcad/ir";
import type { Document } from "@vcad/ir";
import { writeFileSync, readFileSync } from "node:fs";
import { randomBytes } from "node:crypto";
import { AsyncLocalStorage } from "node:async_hooks";
import { resolveWithinRoot } from "./safe-path.js";
import { storeArtifact } from "./artifact-store.js";
import { maxInlineArtifactBytes } from "./remote.js";
import type { SessionStore } from "../session-store.js";
import type { AuthUser } from "../oauth.js";

// ─── Session cache (per-connection isolated, or process-wide fallback) ────────
//
// The cache used to be a single process-global Map keyed by bare document_id,
// shared across every connection on a warm serverless instance. Once sessions
// became user-owned that was a cross-tenant leak: user B passing user A's id
// would read A's warm-cached doc straight from the shared Map (getSession does
// no ownership check). The fix: a signed-in connection runs inside an
// AsyncLocalStorage scope with its OWN per-request Map, populated only from
// that user's (user-scoped) durable store — so a cache hit can never serve
// another tenant's document, and the per-request Map is GC'd after the request
// (no unbounded growth). Anonymous / stdio callers have no tenant boundary and
// keep the process-wide fallback (cross-request warm cache, today's behavior).

/** Process-wide fallback cache: anonymous hosted calls, stdio, and any access
 *  outside a per-connection scope (e.g. direct test calls). */
const fallbackDocuments = new Map<string, Document>();

const sessionScope = new AsyncLocalStorage<{
  documents: Map<string, Document>;
  user: AuthUser | null;
}>();

/** The cache for the current connection: a signed-in user's isolated
 *  per-request Map, or the process-wide fallback. */
function activeDocuments(): Map<string, Document> {
  return sessionScope.getStore()?.documents ?? fallbackDocuments;
}

/**
 * Run `fn` with an isolated per-connection session cache when `user` is signed
 * in, so concurrent users on one warm serverless instance never share mutable
 * session state — a cache hit can't leak another tenant's document. Anonymous
 * and stdio callers (`user === null`) keep the process-wide fallback: no tenant
 * boundary to enforce, and their cross-request warm cache stays intact. The
 * dispatch handler wraps every tool call in this.
 */
export function runInSessionScope<T>(user: AuthUser | null, fn: () => T): T {
  if (!user) return fn();
  return sessionScope.run({ documents: new Map(), user }, fn);
}

/** The signed-in user for the current tool call, or null (anonymous/stdio).
 *  Read by telemetry to attribute events; outside any scope returns null. */
export function currentUser(): AuthUser | null {
  return sessionScope.getStore()?.user ?? null;
}

/**
 * Session cache facade. Every read/write routes to the active cache
 * (`activeDocuments`), so all existing `documents.has/get/set/delete/clear`
 * call sites — here, in server.ts, in dfm.ts, and in tests — are transparently
 * scope-aware with no change. Exported (in place of the raw Map) so tests can
 * still clear/inspect it; outside any scope it is exactly the fallback Map. */
export const documents = {
  has: (id: string): boolean => activeDocuments().has(id),
  get: (id: string): Document | undefined => activeDocuments().get(id),
  set: (id: string, doc: Document): Map<string, Document> =>
    activeDocuments().set(id, doc),
  delete: (id: string): boolean => activeDocuments().delete(id),
  clear: (): void => activeDocuments().clear(),
  get size(): number {
    return activeDocuments().size;
  },
};

let nextId = 1;

function nextSessionId(): string {
  // The counter keeps intra-process uniqueness; the random suffix makes the id
  // unguessable. Once a session persists to a user's account the id is a
  // capability — sequential ids would let one caller enumerate another's
  // warm-cache sessions.
  return `doc_${nextId++}_${randomBytes(9).toString("base64url")}`;
}

/** Register a freshly-built document as a session and return its id.
 *  Lets other tools (e.g. `sheet_metal_create`) hand back a
 *  `document_id` that `inspect_cad` / `export_cad` / `open_in_browser`
 *  can then operate on, without duplicating the id scheme. */
export function registerSession(doc: Document): string {
  const id = nextSessionId();
  documents.set(id, doc);
  return id;
}

/** Get a session document by id, or throw a helpful error. Synchronous and
 *  cache-only by design — the dispatch layer runs `hydrateSession` first, so a
 *  durably-stored session is already resident here by the time a tool reads
 *  it, and a genuine miss still throws the pinned error. */
export function getSession(documentId: string): Document {
  const doc = documents.get(documentId);
  if (!doc) {
    throw new Error(
      `Unknown document_id "${documentId}". Open one with open_document first, or list active sessions with the documents map.`,
    );
  }
  return doc;
}

// ─── Per-session undo history (in-memory snapshot stack) ──────────────────────
//
// The event spine (session_events) is an append-only telemetry/realtime log,
// not a reconstruction fold: it records {tool, args, changed} — never enough to
// rebuild geometry — and is a no-op without Supabase env (stdio/local). So undo
// can't be "fold all-but-last event". Instead the dispatch layer snapshots the
// document *before* each mutation onto this stack, and `undo` pops the last
// snapshot back into the live session. Restoring a full snapshot is exact and
// works identically on stdio, anonymous, and signed-in deployments.
//
// The stack is process-global (NOT per-request-scoped) so it survives across
// the per-connection session scopes a signed-in user gets — a snapshot pushed
// on call N must still be poppable on call N+1. It's keyed by the unguessable
// document_id (the session capability), and a pop only ever writes back into a
// session the caller already resolved via getSession, so it leaks nothing
// cross-tenant. Depth is capped so a long editing session can't grow unbounded.

/** Max snapshots retained per session — older edits drop off the bottom. */
const MAX_HISTORY = 50;

/** document_id → stack of pre-mutation Document snapshots (oldest first). */
const sessionHistory = new Map<string, Document[]>();

/** Deep clone a document — the same JSON round-trip openDocument uses, so a
 *  later in-place mutation can never corrupt a retained snapshot. */
function cloneDoc(doc: Document): Document {
  return JSON.parse(JSON.stringify(doc)) as Document;
}

/**
 * Push the current state of a session onto its undo stack, to be called by the
 * dispatch layer right before a mutating tool runs. No-op when the session
 * isn't resident (nothing to snapshot). Caps the stack at MAX_HISTORY by
 * dropping the oldest entry.
 */
export function recordHistorySnapshot(documentId: string): void {
  const doc = documents.get(documentId);
  if (!doc) return;
  const stack = sessionHistory.get(documentId) ?? [];
  stack.push(cloneDoc(doc));
  if (stack.length > MAX_HISTORY) stack.shift();
  sessionHistory.set(documentId, stack);
}

/** Number of undo steps available for a session. */
export function historyDepth(documentId: string): number {
  return sessionHistory.get(documentId)?.length ?? 0;
}

/**
 * Pop the last snapshot and restore it as the live session document. Returns
 * the restored Document, or null when there's nothing to undo. The caller is
 * expected to have already resolved the session (getSession) so ownership is
 * enforced before any state is rewound.
 */
export function undoLastSnapshot(documentId: string): Document | null {
  const stack = sessionHistory.get(documentId);
  if (!stack || stack.length === 0) return null;
  const snapshot = stack.pop()!;
  if (stack.length === 0) sessionHistory.delete(documentId);
  documents.set(documentId, snapshot);
  return snapshot;
}

/** Drop a session's undo history (on close/drop) so it can't outlive the doc. */
export function clearHistory(documentId: string): void {
  sessionHistory.delete(documentId);
}

// ─── Durable cache ⇄ store bridge ─────────────────────────────────────────────
//
// The three helpers below are the ONLY async surface added for durability, and
// they are called exclusively from the (already-async) dispatch handler — never
// from a tool — so `getSession`, `registerSession`, `openDocument`, etc. keep
// their synchronous signatures and no tool handler changes. The `store` is
// passed in (not a module global) so concurrent connections on one warm
// serverless instance can't clobber each other's binding.

/** Cache-miss rehydrate: if `documentId` isn't resident, try the durable store
 *  and populate the cache. Returns true if the doc is now cached. */
export async function hydrateSession(
  store: SessionStore,
  documentId: string,
): Promise<boolean> {
  if (documents.has(documentId)) return true;
  const doc = await store.load(documentId);
  if (doc) {
    documents.set(documentId, doc);
    return true;
  }
  return false;
}

/** Best-effort durable persist of a session's current cache contents. */
export async function persistSession(
  store: SessionStore,
  documentId: string,
  name?: string,
): Promise<void> {
  const doc = documents.get(documentId);
  if (doc) await store.save(documentId, doc, name);
}

/** Drop a session from both the cache and the durable store (close_document). */
export async function dropSession(
  store: SessionStore,
  documentId: string,
): Promise<void> {
  documents.delete(documentId);
  clearHistory(documentId);
  await store.drop(documentId);
}

// ─── open_document ────────────────────────────────────────────────────────

export const openDocumentSchema = {
  type: "object" as const,
  properties: {
    initial: {
      type: "object" as const,
      description:
        "Optional initial Document IR. If omitted, an empty document is created. Pass an existing IR (e.g. from import_step) to begin editing it.",
    },
  },
};

export function openDocument(args: Record<string, unknown>): {
  content: Array<{ type: "text"; text: string }>;
} {
  const initial = args.initial as Document | undefined;
  const doc: Document = initial
    ? // Defensive copy — callers shouldn't be able to mutate the session
      // doc by retaining the reference they passed in.
      JSON.parse(JSON.stringify(initial))
    : createDocument();
  const id = nextSessionId();
  documents.set(id, doc);
  return {
    content: [
      {
        type: "text",
        text: JSON.stringify({ document_id: id, parts: doc.roots.length }),
      },
    ],
  };
}

// ─── get_document ─────────────────────────────────────────────────────────

export const getDocumentSchema = {
  type: "object" as const,
  properties: {
    document_id: {
      type: "string" as const,
      description: "Session id from open_document.",
    },
  },
  required: ["document_id"],
};

export function getDocumentTool(args: Record<string, unknown>): {
  content: Array<{ type: "text"; text: string }>;
} {
  const id = String(args.document_id ?? "");
  const doc = getSession(id);
  const inline = JSON.stringify(doc);
  // A routed PCB or imported assembly serializes far past the tool-output
  // token budget. Same byte-cap idiom as export_gerber / import_step: under
  // the cap the full IR returns inline (the documented contract); over it the
  // IR moves to the artifact store and the result is a compact handle whose
  // manifest sha256 lets any consumer verify the downloaded snapshot.
  const cap = maxInlineArtifactBytes();
  if (Buffer.byteLength(inline, "utf8") > cap) {
    const handle = storeArtifact([{ name: `${id}.vcad`, content: inline }]);
    return {
      content: [
        {
          type: "text",
          text: JSON.stringify({
            document_id: id,
            parts: doc.roots?.length ?? 0,
            nodes: Object.keys(doc.nodes ?? {}).length,
            bytes: handle.bytes,
            artifact_id: handle.artifact_id,
            artifact_url: handle.artifact_url,
            manifest: handle.manifest,
            expires_at: handle.expires_at,
            note:
              `Document IR is ${handle.bytes} bytes — over the ${cap}-byte inline ` +
              "limit, so the full IR was written to the artifact store. Download " +
              "it at artifact_url (sha256 in the manifest verifies the snapshot); " +
              "the session stays live via document_id.",
          }),
        },
      ],
    };
  }
  return {
    content: [{ type: "text", text: inline }],
  };
}

// ─── close_document ───────────────────────────────────────────────────────

export const closeDocumentSchema = {
  type: "object" as const,
  properties: {
    document_id: {
      type: "string" as const,
      description: "Session id from open_document.",
    },
  },
  required: ["document_id"],
};

export function closeDocument(args: Record<string, unknown>): {
  content: Array<{ type: "text"; text: string }>;
} {
  const id = String(args.document_id ?? "");
  const existed = documents.delete(id);
  clearHistory(id);
  return {
    content: [
      {
        type: "text",
        text: JSON.stringify({ closed: existed, document_id: id }),
      },
    ],
  };
}

// ─── Durable persistence (save_document / load_document) ──────────────────────
//
// The session Map above is in-process only — a server restart or cold start
// loses every board, and there is no way to reopen one by id. These two tools
// add a file-backed persistence layer: `save_document` serializes a live
// session to `<name>.vcad` under the state root, and `load_document` reads it
// back into a fresh session.
//
// The state root is VCAD_MCP_STATE_DIR (or process.cwd()), and `resolveWithinRoot`
// both sanitizes `name` and confines reads/writes to that root, so a caller can
// never escape it with `../` or an absolute path.
//
// This is the local/stdio persistence layer. The natural extension for the
// hosted/multi-tenant deployment is durable storage in the Supabase `documents`
// table keyed by the OAuth user, rather than the local filesystem.

/** State root for saved `.vcad` files. VCAD_MCP_STATE_DIR or cwd. */
function stateRoot(): string {
  return process.env.VCAD_MCP_STATE_DIR ?? process.cwd();
}

// ─── save_document ────────────────────────────────────────────────────────

export const saveDocumentSchema = {
  type: "object" as const,
  properties: {
    document_id: {
      type: "string" as const,
      description: "Session id from open_document.",
    },
    name: {
      type: "string" as const,
      description:
        "Filename slug (no extension) to save under, relative to the server state directory " +
        "(VCAD_MCP_STATE_DIR if set, otherwise the working directory). Written as <name>.vcad.",
    },
  },
  required: ["document_id", "name"],
};

export function saveDocument(args: Record<string, unknown>): {
  content: Array<{ type: "text"; text: string }>;
} {
  const id = String(args.document_id ?? "");
  const name = String(args.name ?? "");
  const doc = getSession(id);
  // resolveWithinRoot sanitizes `name` (rejects absolute/.. /NUL/escape) and
  // confines the write to the state root.
  const path = resolveWithinRoot(`${name}.vcad`, stateRoot());
  writeFileSync(path, JSON.stringify(doc));
  return {
    content: [
      {
        type: "text",
        text: JSON.stringify({ saved: true, name, path }),
      },
    ],
  };
}

// ─── load_document ────────────────────────────────────────────────────────

export const loadDocumentSchema = {
  type: "object" as const,
  properties: {
    name: {
      type: "string" as const,
      description:
        "Filename slug (no extension) to load, relative to the server state directory " +
        "(VCAD_MCP_STATE_DIR if set, otherwise the working directory). Reads <name>.vcad.",
    },
  },
  required: ["name"],
};

export function loadDocument(args: Record<string, unknown>): {
  content: Array<{ type: "text"; text: string }>;
  isError?: boolean;
} {
  const name = String(args.name ?? "");
  const root = stateRoot();
  const path = resolveWithinRoot(`${name}.vcad`, root);
  let raw: string;
  try {
    raw = readFileSync(path, "utf8");
  } catch {
    return {
      isError: true,
      content: [
        {
          type: "text",
          text: `No saved document named "${name}" under ${root}`,
        },
      ],
    };
  }
  const doc = JSON.parse(raw) as Document;
  const id = registerSession(doc);
  return {
    content: [
      {
        type: "text",
        text: JSON.stringify({
          document_id: id,
          name,
          parts: doc.roots?.length ?? 0,
        }),
      },
    ],
  };
}
