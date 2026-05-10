/**
 * Sheet-metal evaluation hook for the engine.
 *
 * Sheet-metal ops bypass the regular Solid pipeline (the WASM kernel
 * doesn't have to know about them yet) and emit a {@link TriangleMesh}
 * directly via {@link tessellate}. The constructed
 * {@link SheetMetalModel} is attached to the {@link EvaluatedPart} so the
 * flat-pattern view + sheet-metal property panel can read panels, bends,
 * provenance, and the projected flat pattern.
 */

import type { Node, NodeId } from "@vcad/ir";
import {
  baseFlangeRect,
  addEdgeFlange,
  builtinBendTable,
  tessellate,
  type SheetMetalModel,
} from "@vcad/sheet-metal";
import type { TriangleMesh } from "./mesh.js";

/**
 * Walk back from `rootId` and, if the chain consists entirely of
 * sheet-metal ops, build the corresponding model. Returns `null` when the
 * root isn't a sheet-metal op.
 */
export function buildSheetMetalModel(
  rootId: NodeId,
  nodes: Record<string, Node>,
): SheetMetalModel | null {
  const root = nodes[String(rootId)];
  if (!root) return null;
  if (
    root.op.type !== "SheetMetalBaseFlangeRect" &&
    root.op.type !== "SheetMetalEdgeFlange"
  ) {
    return null;
  }
  // Walk to the chain's base flange (linear chain for the foundation tier).
  const chain: Node[] = [];
  let cursor: Node | undefined = root;
  while (cursor) {
    chain.push(cursor);
    if (cursor.op.type === "SheetMetalBaseFlangeRect") break;
    if (cursor.op.type !== "SheetMetalEdgeFlange") return null;
    cursor = nodes[String(cursor.op.parent)];
  }
  if (!cursor || cursor.op.type !== "SheetMetalBaseFlangeRect") return null;

  const table = builtinBendTable();
  // Apply ops base → tip.
  let model: SheetMetalModel | null = null;
  for (const node of chain.reverse()) {
    if (node.op.type === "SheetMetalBaseFlangeRect") {
      const op = node.op;
      model = baseFlangeRect(op.width, op.depth, op.thickness);
    } else if (node.op.type === "SheetMetalEdgeFlange") {
      if (model === null) return null;
      const op = node.op;
      addEdgeFlange(model, table, {
        panel: op.panel_id,
        edgeIndex: op.edge_index,
        length: op.length,
        angle: op.angle,
        radius: op.radius,
        direction: op.direction,
        position: "MaterialInside",
        material: "Al-soft",
        manualK: op.manual_k,
      });
    }
  }
  return model;
}

/**
 * Tessellate a sheet-metal model into a {@link TriangleMesh} matching the
 * engine's existing scene-renderer shape.
 */
export function sheetMetalToMesh(model: SheetMetalModel): TriangleMesh {
  const t = tessellate(model);
  return {
    positions: t.positions,
    indices: t.indices,
    normals: t.normals,
  };
}
