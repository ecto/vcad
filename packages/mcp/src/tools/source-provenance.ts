/**
 * Document ⇄ source provenance.
 *
 * `create_cad_loon` used to evaluate loon source into a session document and
 * then throw the source away: `get_document` returned IR, the authored form
 * was unrecoverable, and a session and the `.loon` file that produced it could
 * drift apart with nothing detecting it. This module keeps the two linked.
 *
 * The link lives on the document itself (`Document.source`, see the Rust
 * `vcad-ir` crate) rather than in a side map, so it survives the durable
 * session store, `save_document`, and a `.vcad` round-trip — exactly the
 * places a side map would silently lose it.
 *
 * Two ways a document goes stale, both reported by `sourceStatus`:
 *  - the FILE changed underneath a session loaded from a path (hash mismatch);
 *  - the SESSION was mutated by an incremental create/update/delete, which
 *    edits IR directly and cannot round-trip back to loon (`diverged`).
 *
 * We don't try to reconcile the second case — the honest move is to mark the
 * document as diverged from its source and say so.
 */

import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import type { Document, DocumentSource } from "@vcad/ir";
import { documents } from "./session-core.js";

/** SHA-256 (hex) of a source text — the same digest stored in `source.hash`. */
export function sourceHash(text: string): string {
  return createHash("sha256").update(text, "utf8").digest("hex");
}

/** How many diverging tool names to remember; enough to explain, not a log. */
const MAX_DIVERGED_BY = 8;

/** Inputs `create_cad_loon` (or a file load) evaluated. */
export interface LoonProvenance {
  text: string;
  modules?: Record<string, string>;
  base_dir?: string;
  path?: string;
}

/**
 * Record on `doc` the loon source it was evaluated from. Called at every
 * authoring entry point, so a document can always say what made it.
 */
export function attachLoonSource(
  doc: Document,
  { text, modules, base_dir, path }: LoonProvenance,
): void {
  const source: DocumentSource = {
    language: "loon",
    text,
    modules: modules && Object.keys(modules).length ? { ...modules } : {},
    hash: sourceHash(text),
    diverged: false,
    diverged_by: [],
  };
  if (base_dir) source.base_dir = base_dir;
  if (path) source.path = path;
  doc.source = source;
}

/**
 * Mark a session's document as diverged from its stored source, naming the
 * tool that did it. No-op when the document has no source (nothing to diverge
 * from) or is already diverged by the same tool most recently.
 */
export function markSourceDiverged(documentId: string, tool: string): boolean {
  const src = documents.get(documentId)?.source;
  if (!src) return false;
  const firstTime = !src.diverged;
  src.diverged = true;
  const by = (src.diverged_by ??= []);
  if (by[by.length - 1] !== tool) by.push(tool);
  if (by.length > MAX_DIVERGED_BY) by.splice(0, by.length - MAX_DIVERGED_BY);
  return firstTime;
}

/** Whether the document's source is still an accurate account of it. */
export interface SourceStatus {
  /** True when the session's geometry no longer corresponds to its source. */
  source_stale: boolean;
  /** Human-readable reason, present only when stale. */
  reason?: string;
  /** Path the source came from, when the document is *of* a file. */
  source_path?: string;
}

/**
 * Compare a document against its stored source. Checks the on-disk file first
 * (a file that changed underneath the session is the case the agent can't see
 * at all), then in-session divergence.
 */
export function sourceStatus(doc: Document | undefined): SourceStatus {
  const src = doc?.source;
  if (!src) return { source_stale: false };
  const path = src.path;

  if (path) {
    let onDisk: string | null = null;
    try {
      onDisk = readFileSync(path, "utf8");
    } catch {
      return {
        source_stale: true,
        source_path: path,
        reason:
          `The source file ${path} can no longer be read — this session is ` +
          `no longer backed by the file it was loaded from.`,
      };
    }
    if (sourceHash(onDisk) !== src.hash) {
      return {
        source_stale: true,
        source_path: path,
        reason:
          `${path} changed on disk since this session was evaluated from it — ` +
          `the session's geometry is from the OLD source. Re-load the file to ` +
          `pick up the edits.`,
      };
    }
  }

  if (src.diverged) {
    const by = src.diverged_by ?? [];
    return {
      source_stale: true,
      ...(path ? { source_path: path } : {}),
      reason:
        `This session has been edited directly (${by.join(", ") || "mutation"})` +
        `, and those edits cannot round-trip back to loon. The stored source ` +
        `describes the document's ORIGIN, not its current state` +
        (path
          ? ` — saving would not reproduce ${path}, and re-evaluating ${path} ` +
            `would discard the edits.`
          : `.`),
    };
  }

  return { source_stale: false, ...(path ? { source_path: path } : {}) };
}
