/**
 * Pure-function mutations that apply a Rust `ToolOutcome` directly to a
 * `Document` IR object. Used by environments that don't have the CRDT
 * `WasmDocumentEngine` mounted — primarily the MCP server, which holds a
 * plain Document per session and needs to mutate it in response to chat
 * tool calls without going through Zustand.
 *
 * The web app's `executeCrud` path uses `dispatchOutcome` in `executors.ts`
 * instead — that one routes outcomes through the docstore's CRDT-backed
 * methods. Both paths consume the same `ToolOutcome` shape from the
 * Rust planner; only the apply step differs.
 */

import type { Document, CsgOp, Node, NodeId, SceneEntry } from "@vcad/ir";
import type { ToolOutcome } from "./registry.js";

/** Find the smallest unused integer node id for a fresh insertion. */
function nextNodeId(doc: Document): NodeId {
  let max = 0;
  for (const k of Object.keys(doc.nodes)) {
    const n = Number(k);
    if (Number.isFinite(n) && n > max) max = n;
  }
  return max + 1;
}

/** Find the root SceneEntry whose root node id matches the given partId
 *  (where partId is the stringified NodeId — the convention used here for
 *  IR-direct, non-CRDT identification). */
function findRootIndex(doc: Document, partId: string): number {
  return doc.roots.findIndex((r) => String(r.root) === partId);
}

/** Walk every CsgOp child reference. Recursive — yields the ids of every
 *  descendant node so callers can decide whether to drop them. */
function collectChildNodeIds(doc: Document, rootId: NodeId): Set<NodeId> {
  const seen = new Set<NodeId>();
  const stack: NodeId[] = [rootId];
  while (stack.length > 0) {
    const id = stack.pop()!;
    if (seen.has(id)) continue;
    seen.add(id);
    const node = doc.nodes[String(id)];
    if (!node) continue;
    for (const child of opChildren(node.op)) stack.push(child);
  }
  return seen;
}

/** Yield every direct child NodeId referenced by a CsgOp. Exhaustive over
 *  the op variants that have child refs; the rest contribute nothing. */
function opChildren(op: CsgOp): NodeId[] {
  switch (op.type) {
    case "Union":
    case "Difference":
    case "Intersection":
      return [op.left, op.right];
    case "Translate":
    case "Rotate":
    case "Scale":
    case "LinearPattern":
    case "CircularPattern":
    case "Shell":
    case "Fillet":
    case "Chamfer":
      return [op.child];
    case "Extrude":
    case "Revolve":
      return [op.sketch];
    case "Sweep":
      return [op.sketch];
    case "Loft":
      return op.sketches ?? [];
    default:
      return [];
  }
}

export interface ApplyOutcomeResult {
  /** The id (stable string form of the root NodeId) of the part the outcome
   *  acted on. For `add_feature` this is the newly created part. */
  partId: string;
  /** For `add_feature`, the NodeId of the new root (which is also its part id). */
  nodeId?: string;
}

/** Apply a `ToolOutcome` from the Rust planner to a Document IR object,
 *  in place. Mirrors the four cases handled by `dispatchOutcome` in
 *  `executors.ts`, but writes directly to `doc.nodes` / `doc.roots` /
 *  `doc.part_materials` instead of routing through CRDT engine methods. */
export function applyToolOutcome(
  doc: Document,
  outcome: ToolOutcome,
): ApplyOutcomeResult {
  switch (outcome.kind) {
    case "add_feature": {
      const id = nextNodeId(doc);
      const node: Node = {
        id,
        name: outcome.name ?? null,
        op: outcome.op as unknown as CsgOp,
      };
      doc.nodes[String(id)] = node;
      // A feature that references existing ROOT nodes as children consumes
      // those parts: the referenced roots become interior nodes of the new
      // part and must stop being independent scene entries — otherwise every
      // boolean/transform chain leaves its intermediates behind as ghost
      // parts that double-count volume. Mirrors the app docstore's
      // applyBoolean/wrap semantics. The first consumed entry is reused in
      // place so its material/visibility/scene position survive; any other
      // consumed entries (e.g. a boolean's right operand) are dropped.
      const children = new Set(opChildren(node.op));
      const consumedIdxs: number[] = [];
      doc.roots.forEach((r, i) => {
        if (children.has(r.root)) consumedIdxs.push(i);
      });
      if (consumedIdxs.length > 0) {
        const first = doc.roots[consumedIdxs[0]!]!;
        const firstPartId = String(first.root);
        doc.roots[consumedIdxs[0]!] = { ...first, root: id };
        if (doc.part_materials[firstPartId] !== undefined) {
          doc.part_materials[String(id)] = doc.part_materials[firstPartId]!;
          delete doc.part_materials[firstPartId];
        }
        for (let k = consumedIdxs.length - 1; k >= 1; k--) {
          const idx = consumedIdxs[k]!;
          delete doc.part_materials[String(doc.roots[idx]!.root)];
          doc.roots.splice(idx, 1);
        }
      } else {
        const entry: SceneEntry = { root: id, material: "default" };
        doc.roots.push(entry);
      }
      return { partId: String(id), nodeId: String(id) };
    }
    case "remove_part": {
      const idx = findRootIndex(doc, outcome.part_id);
      if (idx < 0) {
        throw new Error(
          `remove_part: no root entry with id "${outcome.part_id}" in document`,
        );
      }
      const entry = doc.roots[idx]!;
      doc.roots.splice(idx, 1);
      // Garbage-collect nodes that were exclusive to this root. A node is
      // exclusive when no remaining root reaches it.
      const reachableFromOthers = new Set<NodeId>();
      for (const r of doc.roots) {
        for (const id of collectChildNodeIds(doc, r.root)) {
          reachableFromOthers.add(id);
        }
      }
      const exclusive = collectChildNodeIds(doc, entry.root);
      for (const id of exclusive) {
        if (!reachableFromOthers.has(id)) delete doc.nodes[String(id)];
      }
      delete doc.part_materials[outcome.part_id];
      return { partId: outcome.part_id };
    }
    case "set_part_material": {
      const idx = findRootIndex(doc, outcome.part_id);
      if (idx < 0) {
        throw new Error(
          `set_part_material: no root entry with id "${outcome.part_id}"`,
        );
      }
      const prev = doc.roots[idx]!;
      doc.roots[idx] = { root: prev.root, material: outcome.material, visible: prev.visible };
      doc.part_materials[outcome.part_id] = outcome.material;
      return { partId: outcome.part_id };
    }
    case "update_params": {
      const node = doc.nodes[outcome.node_id];
      if (!node) {
        throw new Error(
          `update_params: no node with id "${outcome.node_id}" in document`,
        );
      }
      // Apply each (key, value) onto the op itself — primitive-shaped ops
      // store params as direct fields on the discriminated union (e.g.
      // CubeOp.size_x). For nested-shaped ops (Sketch2D, Extrude) the
      // planner doesn't currently emit update_params, so a flat copy is
      // sufficient. Skip the discriminator field defensively.
      const op = node.op as unknown as Record<string, unknown>;
      for (const [k, v] of Object.entries(outcome.params)) {
        if (k === "type") continue;
        op[k] = v;
      }
      return { partId: outcome.node_id, nodeId: outcome.node_id };
    }
  }
}

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
    return { id: String(entry.root), name, kind };
  });
}
