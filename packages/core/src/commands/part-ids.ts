/**
 * Stable part ids — the single translation layer between a scene root and
 * the `part_id` handed to agents and the app.
 *
 * A part id is the document engine's stable `"replica:seq"` id (e.g. `"0:7"`),
 * stored on the IR as `Document.partIds` keyed by the stringified root
 * NodeId. Unlike the old ids (the root NodeId itself), a stable id never shifts when another part is deleted, and it
 * survives IR round trips through the engine (the materializer writes
 * `partIds`, import honors it).
 *
 * Documents that predate `partIds` (hand-written IR, older sessions) are
 * migrated lazily: the first time a root is touched it is assigned a fresh
 * id, which is persisted into `doc.partIds` so it stays stable.
 */
import type { Document, NodeId, SceneEntry } from "@vcad/ir";

/** How {@link rootForPartId} matched its input. Anything but `"stable"` is a
 *  deprecated spelling kept for compatibility. */
export type PartIdMatch = "stable" | "legacy-node-id" | "name";

/** A resolved scene root. */
export interface ResolvedPart {
  /** Position of the entry in `doc.roots`. */
  index: number;
  /** The scene entry itself. */
  entry: SceneEntry;
  /** The part's stable id. */
  partId: string;
  /** How the input matched. */
  matchedBy: PartIdMatch;
}

const STABLE_ID = /^(\d+):(\d+)$/;

/** True when `id` is spelled like a stable `"replica:seq"` id. */
export function isStablePartId(id: string): boolean {
  return STABLE_ID.test(id);
}

function idMap(doc: Document): Record<string, string> {
  if (!doc.partIds) doc.partIds = {};
  return doc.partIds;
}

/** Highest `seq` among every stable id the document already carries. */
function maxSeq(map: Record<string, string>): number {
  let max = -1;
  for (const v of Object.values(map)) {
    const m = STABLE_ID.exec(v);
    if (m) max = Math.max(max, Number(m[2]));
  }
  return max;
}

/** Mint a fresh id that collides with nothing in `map`. Deterministic:
 *  `"0:<n>"` with `n` above any existing seq, and at least the root's NodeId
 *  (node ids only grow, so a part created after a delete does not pick up
 *  the deleted part's id, and a migrated legacy id "12" reads as "0:12"). */
function mint(map: Record<string, string>, root?: NodeId | string): string {
  const floor = root !== undefined && /^\d+$/.test(String(root)) ? Number(root) : 0;
  return `0:${Math.max(maxSeq(map) + 1, floor)}`;
}

function entryAt(
  doc: Document,
  rootIndexOrEntry: number | SceneEntry,
): SceneEntry | undefined {
  return typeof rootIndexOrEntry === "number"
    ? doc.roots[rootIndexOrEntry]
    : rootIndexOrEntry;
}

/**
 * The stable part id of a scene root (by `doc.roots` index or entry). Mints
 * and persists one when the root has none yet. Throws on an out-of-range
 * index.
 */
export function partIdForRoot(
  doc: Document,
  rootIndexOrEntry: number | SceneEntry,
): string {
  const entry = entryAt(doc, rootIndexOrEntry);
  if (!entry) throw new Error(`partIdForRoot: no scene root at ${String(rootIndexOrEntry)}`);
  return partIdForNode(doc, entry.root);
}

/** The stable part id of the scene root whose root node is `root`. Mints and
 *  persists one if absent. */
export function partIdForNode(doc: Document, root: NodeId | string): string {
  const map = idMap(doc);
  const key = String(root);
  const existing = map[key];
  if (existing !== undefined) return existing;
  const id = mint(map, root);
  map[key] = id;
  return id;
}

/**
 * Assign an id to every root lacking one (in `doc.roots` order, so the
 * migration is deterministic) and prune entries whose root no longer exists.
 * Returns the doc.
 */
export function ensurePartIds(doc: Document): Document {
  const map = idMap(doc);
  const live = new Set(doc.roots.map((r) => String(r.root)));
  for (const key of Object.keys(map)) {
    if (!live.has(key)) delete map[key];
  }
  for (const r of doc.roots) partIdForNode(doc, r.root);
  return doc;
}

/**
 * Read-only view of every live root's part id, keyed by stringified root
 * NodeId: exactly what {@link ensurePartIds} would assign, computed on a copy
 * so the document is not mutated (for pure reads such as a content-hashed
 * preview, where minting into the doc would change its hash).
 */
export function partIdMap(doc: Document): Record<string, string> {
  const copy = { roots: doc.roots, partIds: { ...(doc.partIds ?? {}) } } as Document;
  return ensurePartIds(copy).partIds!;
}

/**
 * Carry a part's stable id over when its scene root is rewritten to a new
 * node (e.g. a boolean consuming the part wraps it in a new root). The part
 * keeps its identity; the old key is dropped.
 */
export function movePartId(
  doc: Document,
  oldRoot: NodeId | string,
  newRoot: NodeId | string,
): void {
  const map = idMap(doc);
  const from = String(oldRoot);
  const to = String(newRoot);
  if (from === to) return;
  const id = map[from] ?? mint(map, newRoot);
  delete map[from];
  map[to] = id;
}

/** Drop the stable id of a root that left the scene. */
export function forgetPartId(doc: Document, root: NodeId | string): void {
  if (doc.partIds) delete doc.partIds[String(root)];
}

/**
 * Resolve a `part_id` to its scene root. Accepts, in order:
 *  1. a stable `"replica:seq"` id;
 *  2. a legacy digit id — the root NodeId the old surface handed out
 *     (deprecated; never a position — a stale id must miss, not misfire);
 *  3. a part name (the root node's name; first match).
 * Returns undefined when nothing matches. Never mints for the input itself,
 * but does persist ids for any root it returns.
 */
export function rootForPartId(
  doc: Document,
  id: string | number,
): ResolvedPart | undefined {
  const key = String(id).trim();
  if (!key) return undefined;
  const done = (index: number, matchedBy: PartIdMatch): ResolvedPart => {
    const entry = doc.roots[index]!;
    return { index, entry, partId: partIdForRoot(doc, entry), matchedBy };
  };

  if (STABLE_ID.test(key)) {
    const map = doc.partIds ?? {};
    const index = doc.roots.findIndex((r) => map[String(r.root)] === key);
    if (index >= 0) return done(index, "stable");
  }
  if (/^\d+$/.test(key)) {
    const byNode = doc.roots.findIndex((r) => String(r.root) === key);
    if (byNode >= 0) return done(byNode, "legacy-node-id");
    // No positional fallback: old ids were NodeIds, so a stale NodeId must
    // miss rather than land on whichever part now sits at that index.
  }
  const byName = doc.roots.findIndex((r) => doc.nodes[String(r.root)]?.name === key);
  if (byName >= 0) return done(byName, "name");
  return undefined;
}

/** {@link rootForPartId}, returning just the `doc.roots` index (-1 if none). */
export function rootIndexForPartId(doc: Document, id: string | number): number {
  return rootForPartId(doc, id)?.index ?? -1;
}

/** A one-line deprecation note for a non-stable match, or undefined. */
export function partIdDeprecation(
  input: string | number,
  resolved: ResolvedPart | undefined,
): string | undefined {
  if (!resolved || resolved.matchedBy === "stable") return undefined;
  const how =
    resolved.matchedBy === "name"
      ? "a part name"
      : "a legacy node-id part id";
  return `part_id "${String(input)}" was resolved as ${how}; use the stable id "${resolved.partId}" instead (legacy spellings are deprecated).`;
}
