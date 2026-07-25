/**
 * Browser-safe core of the MCP session layer: the document cache, session
 * ids, undo snapshots, and the dual-mode document-input resolver. Split out
 * of `session.ts` so pure-compute tool modules (inspect, measure, clearance,
 * dfm, …) can bundle into the web app — this module imports no Node
 * builtins. `session.ts` re-exports everything here and layers the
 * Node-only parts on top (AsyncLocalStorage connection scoping, file-backed
 * save/load, the session tool defs).
 */

import { type Document } from "@vcad/ir";

// ─── Session cache (per-connection isolated, or process-wide fallback) ────────
//
// The cache used to be a single process-global Map keyed by bare document_id,
// shared across every connection on a warm serverless instance. Once sessions
// became user-owned that was a cross-tenant leak: user B passing user A's id
// would read A's warm-cached doc straight from the shared Map (getSession does
// no ownership check). The fix: a signed-in connection runs inside an
// AsyncLocalStorage scope with its OWN per-request Map — installed by
// `session.ts` via `setSessionScopeProvider` (AsyncLocalStorage is Node-only,
// so the indirection keeps this module browser-safe). Anonymous / stdio /
// in-browser callers have no tenant boundary and use the process-wide
// fallback.

/** Process-wide fallback cache: anonymous hosted calls, stdio, in-browser,
 *  and any access outside a per-connection scope. */
const fallbackDocuments = new Map<string, Document>();

/** Returns the current connection's isolated cache, or null to use the
 *  fallback. Installed by the Node layer (`session.ts`). */
let scopeProvider: () => Map<string, Document> | null = () => null;

/** Install the per-connection cache provider (Node's AsyncLocalStorage
 *  bridge). Browser bundles never call this. */
export function setSessionScopeProvider(
  provider: () => Map<string, Document> | null,
): void {
  scopeProvider = provider;
}

/** The cache for the current connection: a signed-in user's isolated
 *  per-request Map, or the process-wide fallback. */
function activeDocuments(): Map<string, Document> {
  return scopeProvider() ?? fallbackDocuments;
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

/** 9 random bytes as base64url, via WebCrypto so it works in both Node and
 *  the browser (node:crypto is unavailable in app bundles). */
function randomSuffix(): string {
  const bytes = new Uint8Array(9);
  globalThis.crypto.getRandomValues(bytes);
  let bin = "";
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

// ─── Boot generation (restart detection) ─────────────────────────────────────
//
// A non-durable server keeps sessions in process memory, so a restart silently
// invalidates every live document_id. The first symptom used to be an
// indistinguishable "Unknown document_id" much later — identical to the error
// for a typo'd id, even though the remediations are opposite (re-author the
// work vs fix the argument). Stamping the minting process's boot token INTO the
// id makes the two cases mechanically separable: an id whose token isn't ours
// was minted by a process that is gone.

/** Length of the boot-token prefix carried in a session id's suffix segment. */
const BOOT_TOKEN_LEN = 4;

/** This process's boot token — fresh at every cold start, stable for its life. */
const BOOT_TOKEN: string = randomSuffix().slice(0, BOOT_TOKEN_LEN);

/** The boot token of the process serving this call. Changes iff the server
 *  restarted, so a client that remembers it can detect a restart without
 *  polling anything. Surfaced on every tool result's `_meta`. */
export function currentBootToken(): string {
  return BOOT_TOKEN;
}

/** Extract the boot token from a session id, or null if the id doesn't carry
 *  one (a hand-written id, or one minted before ids were generation-tagged). */
export function sessionIdBootToken(documentId: string): string | null {
  const m = /^doc_\d+_([A-Za-z0-9_-]{4,})$/.exec(documentId);
  return m ? m[1].slice(0, BOOT_TOKEN_LEN) : null;
}

/** True when `documentId` was minted by a DIFFERENT process than the one
 *  serving this call — i.e. the server restarted (or, on a multi-instance
 *  deploy, another instance answered) since the id was handed out. */
export function isForeignSessionId(documentId: string): boolean {
  const token = sessionIdBootToken(documentId);
  return token !== null && token !== BOOT_TOKEN;
}

/** Whether this deployment's session store survives a restart. Injected by the
 *  Node layer (`session.ts`) because the probe reads process env; browser
 *  bundles keep the conservative default. */
let durabilityProbe: () => boolean = () => false;

/** Install the durability probe (Node-only bridge to `isSessionStoreDurable`). */
export function setDurabilityProbe(probe: () => boolean): void {
  durabilityProbe = probe;
}

export function nextSessionId(): string {
  // The counter keeps intra-process uniqueness; the random suffix makes the id
  // unguessable. Once a session persists to a user's account the id is a
  // capability — sequential ids would let one caller enumerate another's
  // warm-cache sessions. The boot token rides in front of the random bytes, so
  // it costs no extra id segment and can't be confused for one.
  return `doc_${nextId++}_${BOOT_TOKEN}${randomSuffix()}`;
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
  if (!doc) throw new Error(unknownSessionMessage(documentId));
  return doc;
}

/**
 * The "Unknown document_id" error text, specialized to WHY the id didn't
 * resolve. The dispatch layer has already tried the durable store by the time
 * this fires, so a miss is terminal — but the remediation differs sharply:
 *
 *  - foreign boot token → the minting process is gone. On a non-durable store
 *    the session's contents went with it and the only fix is to re-run the
 *    authoring calls. Saying so turns a mystifying late failure into a
 *    self-announcing one.
 *  - our boot token (or no token) → this process minted no such id: a typo, a
 *    stale copy/paste, or an already-closed session.
 */
export function unknownSessionMessage(documentId: string): string {
  const head = `Unknown document_id "${documentId}".`;
  if (isForeignSessionId(documentId)) {
    const durable = durabilityProbe();
    return durable
      ? `${head} It was minted by a different server process (this one booted ` +
          `as "${BOOT_TOKEN}") and was not found in the durable session store — ` +
          `it may have been closed or dropped. Re-open it, or re-run the ` +
          `authoring calls that built it.`
      : `${head} SESSION LOST TO A SERVER RESTART — this id was minted by an ` +
          `earlier process (this one booted as "${BOOT_TOKEN}") and sessions on ` +
          `this server are in-memory only, so its contents are gone. This is ` +
          `not a typo: re-run the authoring calls that built the document (a ` +
          `checkpoint_document / save_document snapshot, or the original ` +
          `create_cad_loon source, restores it). Check server_info — a low ` +
          `uptime_s confirms the restart. Configure a durable session store to ` +
          `prevent this.`;
  }
  return (
    `${head} This server has no such session — check for a typo, or it was ` +
    `closed. Open one with open_document first, or list active sessions with ` +
    `the documents map.`
  );
}

// ─── Dual-mode document input (session id OR inline document) ─────────────────
//
// The primary path is always a live `document_id` session. But a warm session
// is process-local: after a cold start or a serverless instance flip the id no
// longer resolves. An inline `document` object is the stateless escape hatch —
// the caller pastes the IR it already holds and the tool runs without a
// resident session. Generalized here (it began as ecad's private helper) so the
// core read-only tools share one contract and one error string.

/** A resolved document input: session-backed (preferred) or inline (the
 *  stateless escape hatch for cold serverless instances). */
export interface DocInputCtx {
  doc: Document;
  /** Set when the doc came from a live server session. */
  documentId?: string;
}

/**
 * Resolve a tool's document argument. `document_id` (a live session) is the
 * primary path; an inline document object is the stateless fallback that still
 * works when no session is resident (cold instance / instance flip). By default
 * the inline field is `document`; pass `inlineKeys` to accept aliases (e.g.
 * export_cad's legacy `ir`), tried in order.
 */
export function resolveDocInput(
  args: Record<string, unknown>,
  inlineKeys: readonly string[] = ["document"],
): DocInputCtx {
  const id = args.document_id ? String(args.document_id) : "";
  if (id) return { doc: getSession(id), documentId: id };
  for (const key of inlineKeys) {
    const inline = args[key];
    if (inline && typeof inline === "object") return { doc: inline as Document };
  }
  const names = inlineKeys.map((k) => `\`${k}\``).join(" or ");
  throw new Error(
    `Pass \`document_id\` (from open_document) — or an inline ${names} object for the stateless flow.`,
  );
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
  lastChangedParts.delete(documentId);
  lastTriangleCount.delete(documentId);
}

// ─── Last mutation diff (per session) ─────────────────────────────────────────
//
// Every mutation result carries a `changed` diff of the parts it touched; this
// map remembers the most recent one so `render_view {highlight_changed: true}`
// can spotlight exactly those parts without the agent re-plumbing ids. Process-
// local, like the undo stack: after an instance flip there is no "last
// mutation", and render_view reports that instead of guessing.

/** document_id → part ids from the most recent mutation's `changed` diff
 *  (added + modified; removed parts no longer exist to highlight). */
const lastChangedParts = new Map<string, string[]>();

/** Record the part ids a mutation just touched (added + modified). */
export function recordLastChanged(documentId: string, partIds: string[]): void {
  if (!documentId) return;
  lastChangedParts.set(documentId, partIds);
}

/** Part ids from the session's most recent mutation diff, or null when no
 *  mutation has been recorded on this instance. */
export function getLastChanged(documentId: string): string[] | null {
  return lastChangedParts.get(documentId) ?? null;
}

// ─── Last known tessellation size (per session) ──────────────────────────────
//
// The drafting renderer emits one SVG element per visible triangle, so a
// document's triangle count is a direct proxy for render memory: a ~380k-
// triangle document serializes to a >100 MB SVG string that OOMs the process
// before the rasterizer ever reports an error. Every mutation already pays
// for a full integrity evaluation (which counts triangles); remembering that
// number lets render_view refuse an un-renderable document up front instead
// of crashing the instance.

/** document_id → triangle count from the most recent integrity evaluation. */
const lastTriangleCount = new Map<string, number>();

/** Record the document's triangle count from a mutation's integrity pass. */
export function recordTriangles(documentId: string, triangles: number): void {
  if (!documentId) return;
  lastTriangleCount.set(documentId, triangles);
}

/** Triangle count from the session's most recent integrity evaluation, or
 *  null when no mutation has been recorded on this instance. */
export function getLastTriangles(documentId: string): number | null {
  return lastTriangleCount.get(documentId) ?? null;
}
