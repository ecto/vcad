/**
 * query tool — read-only structured introspection over a doc handle.
 *
 * Cheap, side-effect free, and tightly typed. Agents use this to look
 * up node ids by name, list joints/sketches/parts, walk dependencies,
 * or read named parameters without dumping raw IR.
 */

import type { Document, Node, NodeId } from "@vcad/ir";
import type { Engine } from "@vcad/engine";
import type { ToolResult } from "../envelope.js";
import { fail, ok } from "../envelope.js";
import { resolveRef } from "../handles.js";
import type { DocRef } from "../types.js";

export const querySchema = {
  type: "object" as const,
  properties: {
    doc: { description: "Doc handle or inline IR." },
    q: {
      type: "object" as const,
      description:
        "Query discriminator: `{ kind: 'tree' }`, `{ kind: 'list', of: 'parts'|'sketches'|'joints'|'instances' }`, " +
        "`{ kind: 'find', name: string }`, `{ kind: 'parameters' }`, `{ kind: 'dependencies', of: NodeRef }`, " +
        "`{ kind: 'node', id: NodeId }`.",
    },
  },
  required: ["doc", "q"],
};

type Query =
  | { kind: "tree" }
  | { kind: "list"; of: "parts" | "sketches" | "joints" | "instances" | "materials" }
  | { kind: "find"; name: string }
  | { kind: "parameters" }
  | { kind: "dependencies"; of: string | number }
  | { kind: "node"; id: number };

interface QueryInput {
  doc: DocRef;
  q: Query;
}

export function queryTool(input: unknown, engine: Engine): ToolResult {
  const startedAt = performance.now();
  const args = (input ?? {}) as QueryInput;
  if (args.doc === undefined) return fail("invalid_input", "Missing `doc`.");
  if (!args.q || typeof args.q !== "object") return fail("invalid_input", "Missing `q`.");

  const { doc, handle } = resolveRef(args.doc);
  const result = runQuery(doc, args.q);

  return ok({
    result,
    handle,
    doc,
    engine,
    startedAt,
    skipPreview: true,
  });
}

function runQuery(doc: Document, q: Query): unknown {
  switch (q.kind) {
    case "tree":
      return buildTree(doc);
    case "list":
      return listOf(doc, q.of);
    case "find":
      return findByName(doc, q.name);
    case "parameters":
      return doc.parameters ?? {};
    case "dependencies":
      return collectDependencies(doc, q.of);
    case "node":
      return doc.nodes[String(q.id)] ?? null;
    default:
      throw new Error(`Unknown query kind: ${(q as { kind: string }).kind}`);
  }
}

interface TreeNode {
  id: NodeId;
  name: string | null;
  op: string;
  children: TreeNode[];
}

function buildTree(doc: Document): TreeNode[] {
  return doc.roots.map((r) => walk(doc, r.root));
}

function walk(doc: Document, id: NodeId): TreeNode {
  const node = doc.nodes[String(id)];
  if (!node) {
    return { id, name: null, op: "<missing>", children: [] };
  }
  const children = childIdsOf(node).map((cid) => walk(doc, cid));
  return { id, name: node.name, op: node.op.type, children };
}

function childIdsOf(node: Node): NodeId[] {
  const op = node.op as Record<string, unknown>;
  const out: NodeId[] = [];
  for (const k of ["child", "left", "right", "sketch"]) {
    const v = op[k];
    if (typeof v === "number") out.push(v);
  }
  const sketches = op.sketches as unknown;
  if (Array.isArray(sketches)) for (const s of sketches) if (typeof s === "number") out.push(s);
  return out;
}

function listOf(
  doc: Document,
  of: "parts" | "sketches" | "joints" | "instances" | "materials",
): unknown {
  switch (of) {
    case "parts":
      return doc.roots.map((r, i) => {
        const node = doc.nodes[String(r.root)];
        return {
          index: i,
          id: r.root,
          name: node?.name ?? null,
          op: node?.op.type ?? null,
          material: r.material,
          visible: r.visible !== false,
        };
      });
    case "sketches":
      return Object.values(doc.nodes)
        .filter((n) => n.op.type === "Sketch2D" || n.op.type === "Text2D")
        .map((n) => ({ id: n.id, name: n.name, op: n.op.type }));
    case "joints":
      return doc.joints ?? [];
    case "instances":
      return doc.instances ?? [];
    case "materials":
      return doc.materials ?? {};
    default:
      throw new Error(`Unknown list target: ${of}`);
  }
}

function findByName(doc: Document, name: string): { id: NodeId; node: Node } | null {
  for (const node of Object.values(doc.nodes)) {
    if (node.name === name) return { id: node.id, node };
  }
  return null;
}

function collectDependencies(doc: Document, ref: string | number): NodeId[] {
  const id =
    typeof ref === "number"
      ? ref
      : Object.values(doc.nodes).find((n) => n.name === ref)?.id;
  if (id === undefined) return [];

  const out = new Set<NodeId>();
  const stack: NodeId[] = [id];
  while (stack.length) {
    const cur = stack.pop()!;
    if (out.has(cur)) continue;
    out.add(cur);
    const node = doc.nodes[String(cur)];
    if (!node) continue;
    for (const cid of childIdsOf(node)) stack.push(cid);
  }
  out.delete(id);
  return [...out];
}
