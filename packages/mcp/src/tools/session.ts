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
import type { Engine } from "@vcad/engine";
import { writeFileSync, readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { randomBytes } from "node:crypto";
import { AsyncLocalStorage } from "node:async_hooks";
import { resolveWithinRoot } from "./safe-path.js";
import { composeLoonModules } from "./loon.js";
import { storeArtifact } from "./artifact-store.js";
import { maxInlineArtifactBytes } from "./remote.js";
import {
  isSessionStoreDurable,
  sessionStoreInfo,
  type SessionStore,
} from "../session-store.js";
import type { AuthUser } from "../oauth.js";
import { behavior, type ToolDef } from "./tool-def.js";
import {
  documents,
  nextSessionId,
  registerSession,
  getSession,
  clearHistory,
  setSessionScopeProvider,
  setDurabilityProbe,
} from "./session-core.js";
import { attachLoonSource, sourceStatus } from "./source-provenance.js";

// The browser-safe core of the session layer (document cache, ids, undo
// snapshots, resolveDocInput) lives in session-core.ts so pure-compute tool
// modules can bundle into the web app. This module re-exports it and adds the
// Node-only pieces: AsyncLocalStorage connection scoping, file-backed
// save/load, and the session tool defs.
export {
  documents,
  nextSessionId,
  registerSession,
  getSession,
  resolveDocInput,
  recordHistorySnapshot,
  historyDepth,
  undoLastSnapshot,
  clearHistory,
  recordLastChanged,
  getLastChanged,
  recordTriangles,
  getLastTriangles,
  currentBootToken,
  sessionIdBootToken,
  isForeignSessionId,
  unknownSessionMessage,
  type DocInputCtx,
} from "./session-core.js";

/**
 * Warning attached to every result that MINTS a session, when this server's
 * sessions won't survive a restart.
 *
 * WHY it goes on the mint result rather than only in `server_info`: an agent
 * learns it needed `checkpoint_document` only when a later call fails with a
 * dead id, by which point the work is already gone. Stating the storage
 * contract at the moment the handle is issued is what lets an agent decide to
 * keep the authoring source instead of treating the server as storage. Returns
 * undefined (field omitted) when the store IS durable, so the happy path stays
 * quiet.
 */
export function durabilityWarning(): string | undefined {
  if (isSessionStoreDurable()) return undefined;
  return (
    "NON-DURABLE SESSION: this server keeps documents IN MEMORY ONLY — a " +
    "restart destroys this document_id and everything built under it, with no " +
    "other warning. Keep the authoring source (the create_cad_loon call, or " +
    "your own record of the edits) so the document can be rebuilt, and " +
    "snapshot long-lived work with checkpoint_document / save_document. To " +
    "make sessions durable: unset VCAD_MCP_DISK_SESSIONS=0 for local runs " +
    "(sessions then persist under ~/.vcad/mcp-sessions), or set SUPABASE_URL " +
    "+ SUPABASE_SERVICE_ROLE_KEY on a hosted deploy."
  );
}

// ─── Per-connection scoping (Node-only: AsyncLocalStorage) ────────────────────
//
// A signed-in connection runs inside an AsyncLocalStorage scope with its OWN
// per-request document Map, populated only from that user's (user-scoped)
// durable store — so a cache hit can never serve another tenant's document.
// The scope is installed into session-core via setSessionScopeProvider;
// browser bundles never import this module, so they keep the fallback Map.

const sessionScope = new AsyncLocalStorage<{
  documents: Map<string, Document>;
  user: AuthUser | null;
}>();

setSessionScopeProvider(() => sessionScope.getStore()?.documents ?? null);

// Teach the browser-safe core whether this deployment's sessions survive a
// restart, so `getSession`'s miss message can tell "lost to a restart, re-author
// it" from "not found in the durable store". Reads process env, hence the
// Node-side injection.
setDurabilityProbe(isSessionStoreDurable);

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
        text: JSON.stringify({
          document_id: id,
          parts: doc.roots.length,
          // Surfaced so the agent can offer the user a persistent viewport
          // outside the chat transcript (the inline canvas scrolls away).
          live_view:
            "For a live view that stays open while we work, offer the user " +
            "a shareable watch link (share_session) or a vcad.io deep link " +
            "(open_in_browser).",
          ...(durabilityWarning() ? { durability: durabilityWarning() } : {}),
        }),
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
  structuredContent?: Record<string, unknown>;
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
    const overflow = {
      document_id: id,
      parts: doc.roots?.length ?? 0,
      nodes: Object.keys(doc.nodes ?? {}).length,
      bytes: handle.bytes,
      artifact_id: handle.artifact_id,
      artifact_url: handle.artifact_url,
      manifest: handle.manifest,
      expires_at: handle.expires_at,
      ...sourceStatus(doc),
      note:
        `Document IR is ${handle.bytes} bytes — over the ${cap}-byte inline ` +
        "limit, so the full IR was written to the artifact store. Download " +
        "it at artifact_url (sha256 in the manifest verifies the snapshot); " +
        "the session stays live via document_id.",
    };
    return {
      content: [{ type: "text", text: JSON.stringify(overflow) }],
      structuredContent: overflow,
    };
  }
  // Mirror the IR into structuredContent as well as the text body. The
  // dispatch layer stamps geometry results' structuredContent with a
  // {document_id, document_version} preview handle; clients that surface
  // structuredContent instead of the text block would otherwise see only
  // that stub and never the document the tool exists to return. Carrying
  // the IR in both places makes get_document return the document on every
  // client. (Large docs offload above, so this never duplicates megabytes.)
  // `doc.source` rides inside the IR, so the authored loon comes back with the
  // document — a document can say what made it. The status flags sit alongside
  // it because staleness is a comparison against the world (the file on disk,
  // the mutations since), not a field of the IR.
  return {
    content: [{ type: "text", text: inline }],
    structuredContent: {
      document: doc,
      document_id: id,
      ...sourceStatus(doc),
    },
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
// loses every board, and there is no way to reopen one by name. These two
// tools add a named persistence layer, routed by the session store's scope:
//
// - "memory" (stdio/local): file-backed — `save_document` serializes the
//   session to `<name>.vcad` under the state root and `load_document` reads it
//   back. The state root is VCAD_MCP_STATE_DIR (or process.cwd()), and
//   `resolveWithinRoot` sanitizes `name` and confines reads/writes to that
//   root, so a caller can never escape it with `../` or an absolute path.
//
// - "user" (hosted, signed in): durable rows in the caller's own `documents`
//   table under the `saved:<slug>` key (→ local_id `mcp:saved:<slug>`), so a
//   plain human name is a safe, per-user key and the save also shows up at
//   vcad.io. The hosted filesystem is read-only — writeFileSync there was why
//   save_document failed 100% of the time in production.
//
// - "capability" (hosted, anonymous): rows are keyed by id ALONE, so a
//   name-derived key would be guessable across tenants. The save is keyed by
//   an unguessable `saved_…` id returned to the caller, which load_document
//   accepts in place of a name.

/** State root for saved `.vcad` files. VCAD_MCP_STATE_DIR or cwd. */
function stateRoot(): string {
  return process.env.VCAD_MCP_STATE_DIR ?? process.cwd();
}

/** Normalize a human save name to the deterministic durable-key slug. Both
 *  save and load apply this, so "My Part" and "my-part" reopen the same row. */
function savedNameSlug(name: string): string {
  return name
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9_-]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 64);
}

/** Durable key for a user-scoped named save. The `saved:` prefix keeps names
 *  out of the live-session id namespace (`doc_*` / `ckpt_*`). */
function savedKey(slug: string): string {
  return `saved:${slug}`;
}

/** True for the unguessable ids minted by anonymous saves — load_document
 *  passes these through verbatim instead of slugging them. */
function isCapabilitySavedId(name: string): boolean {
  return /^saved_[A-Za-z0-9_-]+$/.test(name);
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
        "Name to save under and reopen with load_document. On the hosted " +
        "server this is a durable per-user name (normalized to lowercase-and-" +
        "dashes); on a local/stdio server it is a filename slug written as " +
        "<name>.vcad under VCAD_MCP_STATE_DIR (or the working directory).",
    },
  },
  required: ["document_id", "name"],
};

export async function saveDocument(
  args: Record<string, unknown>,
  store: SessionStore,
): Promise<{
  content: Array<{ type: "text"; text: string }>;
  isError?: boolean;
}> {
  const id = String(args.document_id ?? "");
  const name = String(args.name ?? "");
  // The schema marks `name` required, but schema `required` isn't enforced
  // at dispatch — without this guard an omitted name silently saved a
  // literal `.vcad` (empty slug) into the state root.
  if (!name.trim()) {
    throw new Error(
      "Pass `name` — the save key load_document reopens the document with.",
    );
  }
  const doc = getSession(id);

  // Local/stdio: file-backed, exactly as before. resolveWithinRoot sanitizes
  // `name` (rejects absolute/.. /NUL/escape) and confines the write to the
  // state root.
  if (store.scope === "memory") {
    const path = resolveWithinRoot(`${name}.vcad`, stateRoot());
    writeFileSync(path, JSON.stringify(doc));
    // A document that knows its source saves BOTH forms, so the file and the
    // session cannot diverge without someone choosing it: the `.vcad` is the
    // evaluated IR, the `.loon` is the authored form `load_document({path})`
    // re-evaluates. Skipped once the session has diverged — writing loon that
    // no longer produces this geometry would be worse than writing none.
    const status = sourceStatus(doc);
    let sourcePath: string | undefined;
    if (doc.source && !status.source_stale) {
      sourcePath = resolveWithinRoot(`${name}.loon`, stateRoot());
      writeFileSync(sourcePath, doc.source.text);
      doc.source.path = sourcePath;
    }
    return {
      content: [
        {
          type: "text",
          text: JSON.stringify({
            saved: true,
            name,
            path,
            ...(sourcePath ? { source_path: sourcePath } : {}),
            ...(doc.source && status.source_stale
              ? {
                  source_not_written:
                    `${status.reason} The .loon was NOT written — it would not ` +
                    `reproduce this document. The .vcad IR is the source of truth.`,
                }
              : {}),
          }),
        },
      ],
    };
  }

  // Hosted: the serverless filesystem is read-only, so the save goes to the
  // durable session store instead.
  const slug = savedNameSlug(name);
  if (!slug) {
    return {
      isError: true,
      content: [
        {
          type: "text",
          text:
            `Invalid save name "${name}" — use letters, digits, dashes, or ` +
            `underscores (e.g. "motor-mount-v2").`,
        },
      ],
    };
  }

  // Frozen snapshot: later edits to the live session must not mutate the save.
  const snapshot = JSON.parse(JSON.stringify(doc)) as Document;
  // "user" rows are scoped per-caller, so the plain name is a safe key.
  // "capability" rows are keyed by id alone, so a name would be guessable
  // across tenants — mint an unguessable id instead (same posture as
  // checkpoint ids) and hand it back as the reopen handle.
  const key =
    store.scope === "user"
      ? savedKey(slug)
      : `saved_${slug}_${randomBytes(9).toString("base64url")}`;
  // Warm-cache so a same-instance load works even if the (best-effort)
  // durable write degrades; the store row is what survives a redeploy.
  documents.set(key, snapshot);
  await store.save(key, snapshot, name.trim() || slug);

  return {
    content: [
      {
        type: "text",
        text: JSON.stringify(
          store.scope === "user"
            ? {
                saved: true,
                name: slug,
                hint:
                  `Saved durably to your account — reopen anytime with ` +
                  `load_document({name: "${slug}"}). It also appears in your ` +
                  `documents at vcad.io.`,
              }
            : {
                saved: true,
                name: key,
                hint:
                  `Saved durably under an anonymous key — reopen with ` +
                  `load_document({name: "${key}"}). Keep the key; anonymous ` +
                  `saves can't be listed or recovered by plain name (sign in ` +
                  `to save by name).`,
              },
        ),
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
        "The name passed to save_document (or the `saved_…` key an anonymous " +
        "save returned). On a local/stdio server this reads <name>.vcad from " +
        "the state directory. Provide this OR `path`.",
    },
    path: {
      type: "string" as const,
      description:
        "Path to a `.loon` source file (evaluated on load) or a `.vcad` " +
        "document to open as a session. The session records the path and a " +
        "content hash, so it is explicitly *of* that file: later calls report " +
        "`source_stale` when the file changes underneath it or the session is " +
        "edited away from it. Local/stdio servers only — a hosted server's " +
        "filesystem is ephemeral.",
    },
  },
};

export async function loadDocument(
  args: Record<string, unknown>,
  store: SessionStore,
  engine?: Engine,
): Promise<{
  content: Array<{ type: "text"; text: string }>;
  isError?: boolean;
}> {
  const path = typeof args.path === "string" ? args.path.trim() : "";
  if (path) return loadDocumentFromPath(path, store, engine);

  const name = String(args.name ?? "");
  if (!name.trim()) {
    throw new Error(
      "Pass `name` (a save_document save) or `path` (a .loon or .vcad file).",
    );
  }

  // Local/stdio: file-backed, exactly as before.
  if (store.scope === "memory") {
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
    return {
      content: [
        {
          type: "text",
          text: JSON.stringify(openSavedSnapshot(JSON.parse(raw) as Document, name)),
        },
      ],
    };
  }

  // Hosted: resolve the save from the warm cache, then the durable store.
  // An anonymous `saved_…` key passes through verbatim; a plain name resolves
  // via the same slug normalization save_document applied.
  const candidates: string[] = [];
  if (isCapabilitySavedId(name)) candidates.push(name);
  const slug = savedNameSlug(name);
  if (slug) candidates.push(savedKey(slug));

  for (const key of candidates) {
    let snapshot = documents.get(key) ?? null;
    if (!snapshot) snapshot = await store.load(key);
    if (snapshot) {
      return {
        content: [
          {
            type: "text",
            text: JSON.stringify(openSavedSnapshot(snapshot, name)),
          },
        ],
      };
    }
  }

  return {
    isError: true,
    content: [
      {
        type: "text",
        text:
          `No saved document named "${name}". Save one first with ` +
          `save_document(document_id, name). (Anonymous saves are reopened by ` +
          `the exact \`saved_…\` key save_document returned; checkpoints use ` +
          `branch_from, not load_document.)`,
      },
    ],
  };
}

/**
 * Open a `.loon` (evaluated) or `.vcad` (parsed) file as a session that knows
 * which file it came from.
 *
 * This is the half of the source⇄document link that runs in the file's
 * direction: without it a `.loon` edited on disk only reached the server by
 * someone remembering to re-paste it through `create_cad_loon`, and nothing
 * marked the session as no longer matching the file.
 */
async function loadDocumentFromPath(
  path: string,
  store: SessionStore,
  engine?: Engine,
): Promise<{
  content: Array<{ type: "text"; text: string }>;
  isError?: boolean;
}> {
  const fail = (text: string) => ({
    isError: true,
    content: [{ type: "text" as const, text }],
  });

  // Hosted instances have an ephemeral, unshared filesystem: a path load would
  // succeed once and then resolve to nothing after an instance flip. Refusing
  // is the honest answer.
  if (store.scope !== "memory") {
    return fail(
      "`path` loads are local/stdio only — this server's filesystem is " +
        "ephemeral. Pass the source inline to create_cad_loon, or save with " +
        "save_document and reopen by `name`.",
    );
  }

  const abs = resolve(path);
  let raw: string;
  try {
    raw = readFileSync(abs, "utf8");
  } catch {
    return fail(`Cannot read ${abs} — no such file, or it is not readable.`);
  }

  if (abs.endsWith(".vcad") || abs.endsWith(".json")) {
    let doc: Document;
    try {
      doc = JSON.parse(raw) as Document;
    } catch (e) {
      return fail(`${abs} is not valid .vcad JSON: ${(e as Error).message}`);
    }
    const id = registerSession(doc);
    return {
      content: [
        {
          type: "text",
          text: JSON.stringify({
            document_id: id,
            path: abs,
            parts: doc.roots?.length ?? 0,
            ...sourceStatus(doc),
          }),
        },
      ],
    };
  }

  if (!engine) {
    return fail(
      "Loon evaluation is unavailable in this server build — load a .vcad " +
        "document instead.",
    );
  }
  // `[use ...]` resolves against the file's own directory, so a multi-file
  // project loads the way it reads on disk.
  const baseDir = dirname(abs);
  const modules = composeLoonModules({ source: raw, base_dir: baseDir });
  let doc: Document | null;
  try {
    doc = engine.evalVcadSourceWithModules(raw, modules) ?? null;
  } catch (e) {
    return fail(`Loon evaluation of ${abs} failed: ${(e as Error).message}`);
  }
  if (!doc) return fail(`Loon evaluation of ${abs} produced no document.`);

  attachLoonSource(doc, {
    text: raw,
    modules,
    base_dir: baseDir,
    path: abs,
  });
  const id = registerSession(doc);
  return {
    content: [
      {
        type: "text",
        text: JSON.stringify({
          document_id: id,
          path: abs,
          parts: doc.roots?.length ?? 0,
          source_stale: false,
          hint:
            "This session is bound to the file: mutating tools report " +
            "`source_stale` once it diverges, and save_document rewrites the " +
            "loon alongside the .vcad.",
          ...(durabilityWarning() ? { durability: durabilityWarning() } : {}),
        }),
      },
    ],
  };
}

/** Open a saved snapshot as a fresh, independent session. The deep copy keeps
 *  the saved row frozen — editing the loaded session never mutates the save. */
function openSavedSnapshot(
  snapshot: Document,
  name: string,
): Record<string, unknown> {
  const copy = JSON.parse(JSON.stringify(snapshot)) as Document;
  const id = registerSession(copy);
  return {
    document_id: id,
    name,
    parts: copy.roots?.length ?? 0,
  };
}

export const toolDefs: ToolDef[] = [
  {
    name: "open_document",
    pack: null,
    description:
      "Open an editing session for a CAD document. Returns a `document_id` to pass to subsequent tool calls (create, update, place_part, inspect_cad, …). Pass an `initial` IR to begin editing an existing document; omit it for a fresh empty document.",
    inputSchema: openDocumentSchema,
    handler: (a) => openDocument(a),
    behavior: behavior({ writesDoc: true, geometry: true, mount: true }),
  },
  {
    name: "get_document",
    pack: null,
    description:
      "Return the full IR Document JSON for an open session. Use after a series of mutations to capture the result, or to feed into `export_cad` / `open_in_browser`. A document authored from loon carries that source under `source`, and the result reports `source_stale` when the session no longer matches it (the file changed, or incremental edits moved the session away from it). Very large documents come back as a compact artifact handle instead ({document_id, artifact_url, manifest with sha256, …}) — download the full IR at `artifact_url`.",
    inputSchema: getDocumentSchema,
    handler: (a) => getDocumentTool(a),
    behavior: behavior({ geometry: true, widgetCallable: true, pureJson: true }),
  },
  {
    name: "close_document",
    pack: null,
    description:
      "Close a document session and free its memory. Idempotent — closing an unknown id reports `closed: false`.",
    inputSchema: closeDocumentSchema,
    handler: (a) => closeDocument(a),
    behavior: behavior({}),
  },
  {
    name: "save_document",
    pack: null,
    description:
      "Persist a live session under a name so it can be reopened with " +
      "load_document. On the hosted server the save is durable: a signed-in " +
      "user's save goes to their vcad.io account under the (normalized) name; " +
      "an anonymous save returns an unguessable `saved_…` key to reopen with. " +
      "On a local/stdio server it writes `<name>.vcad` under VCAD_MCP_STATE_DIR " +
      "(or the working directory) — plus `<name>.loon` when the document knows " +
      "the source it was authored from, so the file and the session can't " +
      "diverge without someone choosing it.",
    inputSchema: saveDocumentSchema,
    handler: (a, c) => saveDocument(a, c.sessionStore),
    behavior: behavior({}),
  },
  {
    name: "load_document",
    pack: null,
    description:
      "Reopen a save_document save into a fresh session and return its new " +
      "document_id. Pass the same name you saved under (or the `saved_…` key " +
      "an anonymous save returned). The cheap way to resume a board/part " +
      "across runs instead of rebuilding it. Pass `path` instead to open a " +
      "`.loon` source file (evaluated on load) or a `.vcad` document from " +
      "disk — that session stays bound to the file and reports " +
      "`source_stale` if the two drift apart.",
    inputSchema: loadDocumentSchema,
    handler: (a, c) => loadDocument(a, c.sessionStore, c.engine),
    behavior: behavior({ writesDoc: true, geometry: true, mount: true }),
  },
];
