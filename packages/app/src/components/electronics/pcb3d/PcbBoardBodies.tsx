/**
 * Renders the FR4 slab for every PcbBoard in the document so a board is
 * visible as a body in the main scene even when it is NOT being edited.
 *
 * The board's geometry comes straight from its PCB data (via the proven
 * PcbBoardMesh), independent of the kernel evaluator — the app's live WASM
 * evaluator still returns an empty solid for PcbBoard, so without this a board
 * would be invisible outside edit focus.
 *
 * The board with edit focus is skipped: PcbScene already draws its slab.
 */

import { useDocumentStore, getPcbNodeIds, getNodePcb } from "@vcad/core";
import type { NodeId } from "@vcad/ir";
import { PcbBoardMesh } from "./PcbBoardMesh";

export function PcbBoardBodies({ excludeNodeId }: { excludeNodeId: NodeId | null }) {
  const doc = useDocumentStore((s) => s.document);
  const boardIds = getPcbNodeIds(doc);

  if (boardIds.length === 0) {
    // CRDT dual-path: the board lives in doc.pcb with no PcbBoard node id.
    // When a board has edit focus (excludeNodeId set) PcbScene draws it.
    return doc.pcb && excludeNodeId == null ? (
      <PcbBoardMesh pcb={doc.pcb} explosion={0} />
    ) : null;
  }

  return (
    <>
      {boardIds.map((id) => {
        if (id === excludeNodeId) return null;
        const pcb = getNodePcb(doc, id);
        return pcb ? <PcbBoardMesh key={String(id)} pcb={pcb} explosion={0} /> : null;
      })}
    </>
  );
}
