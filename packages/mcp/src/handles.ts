/**
 * Document handles — opaque, version-pinned references to stored documents.
 *
 * A handle looks like `vcad:doc:<uuid>` (latest) or `vcad:doc:<uuid>@<n>`
 * (an immutable, pinned version). Tools accept either a handle or an
 * inline IR literal in any field typed as `DocRef`; outputs always return
 * a handle.
 *
 * This module provides:
 *   - the handle type + parser/formatter
 *   - an in-process versioned store (Phase 1's MVP — Supabase migration is
 *     a Phase 9 swap; the rest of the protocol is unchanged)
 *   - a tiny LRU on top so repeated reads hit memory
 *
 * The Supabase swap replaces `MemoryDocStore` with a `SupabaseDocStore`
 * that conforms to the same `DocStore` interface. No tool code changes.
 */

import { createDocument } from "@vcad/ir";
import type { Document } from "@vcad/ir";
import { createHash, randomUUID } from "node:crypto";

/** A versioned document handle. */
export type DocHandle = `vcad:doc:${string}` | `vcad:doc:${string}@${number}`;

/** Anywhere a tool accepts geometry it accepts a handle or an inline IR. */
export type DocRef = DocHandle | Document;

/** Parse a `vcad:doc:<uuid>[@<n>]` string. Throws on malformed input. */
export function parseHandle(s: string): { docId: string; version?: number } {
  const m = /^vcad:doc:([0-9a-f-]{8,})(?:@(\d+))?$/i.exec(s);
  if (!m) throw new Error(`Malformed doc handle: ${s}`);
  const version = m[2] ? Number(m[2]) : undefined;
  return { docId: m[1], version };
}

/** Format a handle. Omit `version` for a "latest" handle. */
export function formatHandle(docId: string, version?: number): DocHandle {
  return version === undefined
    ? (`vcad:doc:${docId}` as DocHandle)
    : (`vcad:doc:${docId}@${version}` as DocHandle);
}

/** True iff the value looks like a handle string. */
export function isHandle(v: unknown): v is DocHandle {
  return typeof v === "string" && /^vcad:doc:[0-9a-f-]{8,}(?:@\d+)?$/i.test(v);
}

/** A version row in the store. `content_sha256` enables dedup. */
export interface DocVersion {
  docId: string;
  version: number;
  ir: Document;
  contentSha256: string;
  createdAt: number;
}

/** Per-document metadata. */
export interface DocMeta {
  docId: string;
  currentVersion: number;
  ownerId: string | null;
  createdVia: "mcp" | "app" | "cli" | "import";
  createdAt: number;
  ttlAt?: number;
  name?: string;
}

/** Storage backend abstraction. Memory now, Supabase later — same shape. */
export interface DocStore {
  /** Read a specific version (or current if `version` is undefined). */
  resolve(docId: string, version?: number): Document | undefined;

  /** Read metadata for a document. */
  meta(docId: string): DocMeta | undefined;

  /**
   * Append a new version to `docId` (or create the row if absent),
   * advance the current pointer, and return the new handle.
   *
   * If `docId` is omitted a fresh UUID is minted.
   */
  store(
    ir: Document,
    opts?: { docId?: string; ownerId?: string | null; ttlSeconds?: number; name?: string },
  ): DocHandle;

  /** List active doc ids (for testing / debug). */
  list(): string[];

  /** Drop everything (testing). */
  reset(): void;
}

const SHA = (s: string) => createHash("sha256").update(s).digest("hex");

/** In-memory implementation — fine for stdio servers and tests. */
class MemoryDocStore implements DocStore {
  private metas = new Map<string, DocMeta>();
  private versions = new Map<string, DocVersion[]>();

  resolve(docId: string, version?: number): Document | undefined {
    const rows = this.versions.get(docId);
    if (!rows || rows.length === 0) return undefined;
    if (version === undefined) return rows[rows.length - 1].ir;
    return rows.find((r) => r.version === version)?.ir;
  }

  meta(docId: string): DocMeta | undefined {
    return this.metas.get(docId);
  }

  store(
    ir: Document,
    opts: { docId?: string; ownerId?: string | null; ttlSeconds?: number; name?: string } = {},
  ): DocHandle {
    const docId = opts.docId ?? randomUUID();
    const sha = SHA(JSON.stringify(ir));
    let rows = this.versions.get(docId);
    if (!rows) {
      rows = [];
      this.versions.set(docId, rows);
      this.metas.set(docId, {
        docId,
        currentVersion: 0,
        ownerId: opts.ownerId ?? null,
        createdVia: "mcp",
        createdAt: Date.now(),
        ttlAt: opts.ttlSeconds ? Date.now() + opts.ttlSeconds * 1000 : undefined,
        name: opts.name,
      });
    }
    // Dedup: if the latest version has the same sha, return its handle.
    const last = rows[rows.length - 1];
    if (last && last.contentSha256 === sha) {
      return formatHandle(docId, last.version);
    }
    const version = (last?.version ?? 0) + 1;
    rows.push({ docId, version, ir, contentSha256: sha, createdAt: Date.now() });
    const meta = this.metas.get(docId)!;
    meta.currentVersion = version;
    if (opts.name) meta.name = opts.name;
    return formatHandle(docId, version);
  }

  list(): string[] {
    return [...this.metas.keys()];
  }

  reset(): void {
    this.metas.clear();
    this.versions.clear();
  }
}

let store: DocStore = new MemoryDocStore();

/** Swap the storage backend (used by Phase 9 Supabase work + tests). */
export function setDocStore(s: DocStore): void {
  store = s;
}

/** Fetch the active store. */
export function getDocStore(): DocStore {
  return store;
}

/**
 * Resolve a `DocRef` to a concrete `Document` + the handle that backs it.
 *
 * Inline IR is materialized into the store on first contact so subsequent
 * tool calls can address it by handle. If the caller passed a literal
 * `{ version, nodes, ... }` they get back a freshly-minted handle.
 */
export function resolveRef(ref: DocRef | undefined): { doc: Document; handle: DocHandle } {
  if (ref === undefined) {
    const doc = createDocument();
    const handle = store.store(doc);
    return { doc, handle };
  }
  if (typeof ref === "string") {
    const { docId, version } = parseHandle(ref);
    const doc = store.resolve(docId, version);
    if (!doc) throw new Error(`No such document: ${ref}`);
    return { doc, handle: formatHandle(docId, version ?? store.meta(docId)?.currentVersion) };
  }
  // Inline IR — store and return.
  const handle = store.store(ref);
  const { docId, version } = parseHandle(handle);
  const doc = store.resolve(docId, version)!;
  return { doc, handle };
}

/** Append a new version of `doc` under the same root id as `parent` (or new). */
export function commitDoc(
  doc: Document,
  parent?: DocHandle,
  opts?: { name?: string; ownerId?: string | null },
): DocHandle {
  const docId = parent ? parseHandle(parent).docId : undefined;
  return store.store(doc, { docId, ...opts });
}
