/**
 * IR-direct document helpers for environments that hold a plain `Document`
 * (the MCP server) rather than the app's docstore. Document mutations run
 * through the Rust document engine (`WasmDocumentEngine.apply_tool`); this
 * module only reads.
 */

import type { Document } from "@vcad/ir";
import { partIdForRoot } from "./part-ids.js";

/** Build a chat-style "parts" summary from the document — the array of
 *  `{ id, name, kind }` triples that the chat surface returns from `read`.
 *  IR-direct equivalent of the docstore's `parts` array. */
export function listPartsFromDocument(
  doc: Document,
): Array<{ id: string; name: string; kind: string }> {
  return doc.roots.map((entry) => {
    const node = doc.nodes[String(entry.root)];
    const name = node?.name ?? `Part ${entry.root}`;
    const kind = node?.op?.type ?? "Unknown";
    return { id: partIdForRoot(doc, entry), name, kind };
  });
}
