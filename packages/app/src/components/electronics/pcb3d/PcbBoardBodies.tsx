/**
 * Legacy fallback for rendering a board's FR4 slab in the main scene.
 *
 * Normal boards are real `PcbBoard` nodes whose kernel op now evaluates to a
 * genuine extruded slab (see crates/vcad-kernel-wasm `evaluate_node` +
 * vcad-app `materializer`). Those render as ordinary parts via `SceneMesh`, so
 * they are deliberately NOT drawn here — doing so would z-fight / double the
 * body.
 *
 * The only case left for this component is the legacy CRDT dual-path: a board
 * that lives solely in `doc.pcb` with no `PcbBoard` node id, so the kernel has
 * nothing to extrude. The focused board is drawn by `PcbScene`, so we only show
 * the slab here when nothing has edit focus.
 */

import { useDocumentStore, getPcbNodeIds } from "@vcad/core";
import type { NodeId } from "@vcad/ir";
import { PcbBoardMesh } from "./PcbBoardMesh";

export function PcbBoardBodies({ excludeNodeId }: { excludeNodeId: NodeId | null }) {
  const doc = useDocumentStore((s) => s.document);

  // Real PcbBoard nodes are extruded by the kernel and rendered as parts.
  if (getPcbNodeIds(doc).length > 0) return null;

  // Legacy dual-path: board only in doc.pcb with no node.
  return doc.pcb && excludeNodeId == null ? (
    <PcbBoardMesh pcb={doc.pcb} explosion={0} />
  ) : null;
}
