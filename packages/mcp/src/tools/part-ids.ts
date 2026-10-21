/**
 * Stable part ids for the MCP surface. The core implementation lives in
 * `@vcad/core` (core's document mutations mint ids too, and core cannot
 * import from mcp); this module is the one place MCP tools import it from,
 * plus the MCP-only helpers below.
 */
import type { Document } from "@vcad/ir";
import { rootForPartId } from "@vcad/core";

export {
  partIdForRoot,
  partIdForNode,
  ensurePartIds,
  partIdMap,
  movePartId,
  forgetPartId,
  rootForPartId,
  rootIndexForPartId,
  isStablePartId,
  partIdDeprecation,
} from "@vcad/core";
export type { ResolvedPart, PartIdMatch } from "@vcad/core";

/**
 * Pick a measured/evaluated part candidate (`id` = stable part id or an
 * instance id) for a caller-supplied id: exact id first, then through the
 * stable-id layer (legacy digit ids), then by exact name.
 */
export function findPartCandidate<T extends { id: string; name?: string }>(
  doc: Document,
  candidates: T[],
  wanted: string,
): T | undefined {
  const exact = candidates.find((c) => c.id === wanted);
  if (exact) return exact;
  const stable = rootForPartId(doc, wanted)?.partId;
  return (
    (stable !== undefined ? candidates.find((c) => c.id === stable) : undefined) ??
    candidates.find((c) => c.name === wanted)
  );
}

/**
 * Migrate a persisted legacy digit part id (root NodeId or position) to its
 * stable id. Stable ids, names and anything unresolvable pass through as-is.
 */
export function migrateLegacyPartId(doc: Document, id: string): string {
  if (!/^\d+$/.test(id)) return id;
  const r = rootForPartId(doc, id);
  return r && r.matchedBy !== "stable" && r.matchedBy !== "name" ? r.partId : id;
}
