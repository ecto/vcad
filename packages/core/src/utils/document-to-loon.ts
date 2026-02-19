/**
 * Convert a vcad Document back to loon source code.
 *
 * Walks the node graph in topological order (dependencies before dependents)
 * and emits `[let nN ...]` bindings for each node, with `[root ...]` entries
 * for scene roots.
 */

import type { Document, Node, NodeId, CsgOp } from "@vcad/ir";

/** Convert a Document to loon source code. */
export function documentToLoon(doc: Document): string {
  const lines: string[] = [];
  const emitted = new Set<string>();

  // Topological sort: emit dependencies first
  const order = topoSort(doc);

  for (const nodeId of order) {
    const node = doc.nodes[String(nodeId)];
    if (!node) continue;
    emitNode(node, doc, lines, emitted);
  }

  // Emit scene roots
  for (const entry of doc.roots) {
    const name = nodeName(entry.root);
    const mat = entry.material || "default";
    lines.push(`[root ${name} ${JSON.stringify(mat)}]`);
  }

  return lines.join("\n") + "\n";
}

function nodeName(id: NodeId): string {
  return `n${id}`;
}

function emitNode(
  node: Node,
  doc: Document,
  lines: string[],
  emitted: Set<string>,
): void {
  const key = String(node.id);
  if (emitted.has(key)) return;
  emitted.add(key);

  // Ensure dependencies are emitted first
  for (const dep of getChildIds(node.op)) {
    const depNode = doc.nodes[String(dep)];
    if (depNode) emitNode(depNode, doc, lines, emitted);
  }

  const name = nodeName(node.id);
  const expr = opToLoon(node.op);
  if (expr) {
    lines.push(`[let ${name} ${expr}]`);
  }
}

function f(n: number): string {
  // Format number: use integer form if whole, otherwise fixed precision
  if (Number.isInteger(n)) return n.toFixed(1);
  return String(n);
}

function opToLoon(op: CsgOp): string | null {
  switch (op.type) {
    case "Cube":
      return `[cube ${f(op.size.x)} ${f(op.size.y)} ${f(op.size.z)}]`;

    case "Cylinder":
      return `[cylinder ${f(op.radius)} ${f(op.height)}]`;

    case "Sphere":
      return `[sphere ${f(op.radius)}]`;

    case "Cone":
      return `[cone ${f(op.radius_bottom)} ${f(op.radius_top)} ${f(op.height)}]`;

    case "Empty":
      return "Empty";

    case "Union":
      return `[union ${nodeName(op.left)} ${nodeName(op.right)}]`;

    case "Difference":
      return `[difference ${nodeName(op.left)} ${nodeName(op.right)}]`;

    case "Intersection":
      return `[intersection ${nodeName(op.left)} ${nodeName(op.right)}]`;

    case "Translate":
      return `[translate ${f(op.offset.x)} ${f(op.offset.y)} ${f(op.offset.z)} ${nodeName(op.child)}]`;

    case "Rotate":
      return `[rotate ${f(op.angles.x)} ${f(op.angles.y)} ${f(op.angles.z)} ${nodeName(op.child)}]`;

    case "Scale":
      return `[scale ${f(op.factor.x)} ${f(op.factor.y)} ${f(op.factor.z)} ${nodeName(op.child)}]`;

    case "Fillet":
      return `[fillet ${f(op.radius)} ${nodeName(op.child)}]`;

    case "Chamfer":
      return `[chamfer ${f(op.distance)} ${nodeName(op.child)}]`;

    case "Shell":
      return `[shell ${f(op.thickness)} ${nodeName(op.child)}]`;

    case "LinearPattern":
      return `[linear-pattern ${f(op.direction.x)} ${f(op.direction.y)} ${f(op.direction.z)} ${op.count} ${f(op.spacing)} ${nodeName(op.child)}]`;

    case "CircularPattern":
      return `[circular-pattern ${f(op.axis_origin.x)} ${f(op.axis_origin.y)} ${f(op.axis_origin.z)} ${f(op.axis_dir.x)} ${f(op.axis_dir.y)} ${f(op.axis_dir.z)} ${op.count} ${f(op.angle_deg)} ${nodeName(op.child)}]`;

    case "Sketch2D": {
      const segs = op.segments
        .map((seg) => {
          if (seg.type === "Line") {
            return `[line ${f(seg.start.x)} ${f(seg.start.y)} ${f(seg.end.x)} ${f(seg.end.y)}]`;
          } else {
            return `[arc ${f(seg.start.x)} ${f(seg.start.y)} ${f(seg.end.x)} ${f(seg.end.y)} ${f(seg.center.x)} ${f(seg.center.y)} ${seg.ccw}]`;
          }
        })
        .join("\n    ");
      return `[sketch ${f(op.origin.x)} ${f(op.origin.y)} ${f(op.origin.z)} ${f(op.x_dir.x)} ${f(op.x_dir.y)} ${f(op.x_dir.z)} ${f(op.y_dir.x)} ${f(op.y_dir.y)} ${f(op.y_dir.z)}\n  #[${segs}]]`;
    }

    case "Extrude":
      return `[extrude ${f(op.direction.x)} ${f(op.direction.y)} ${f(op.direction.z)} ${nodeName(op.sketch)}]`;

    case "Revolve":
      return `[revolve ${f(op.axis_origin.x)} ${f(op.axis_origin.y)} ${f(op.axis_origin.z)} ${f(op.axis_dir.x)} ${f(op.axis_dir.y)} ${f(op.axis_dir.z)} ${f(op.angle_deg)} ${nodeName(op.sketch)}]`;

    case "Sweep":
    case "Loft":
    case "Text2D":
    case "ImportedMesh":
      // These are complex types that don't have simple loon representations yet
      return null;

    default:
      return null;
  }
}

function getChildIds(op: CsgOp): NodeId[] {
  switch (op.type) {
    case "Translate":
    case "Rotate":
    case "Scale":
    case "Fillet":
    case "Chamfer":
    case "Shell":
    case "LinearPattern":
    case "CircularPattern":
      return [op.child];
    case "Union":
    case "Difference":
    case "Intersection":
      return [op.left, op.right];
    case "Extrude":
    case "Revolve":
    case "Sweep":
      return [op.sketch];
    case "Loft":
      return op.sketches;
    default:
      return [];
  }
}

/** Topological sort of node IDs (dependencies first). */
function topoSort(doc: Document): NodeId[] {
  const visited = new Set<string>();
  const order: NodeId[] = [];

  function visit(id: NodeId) {
    const key = String(id);
    if (visited.has(key)) return;
    visited.add(key);
    const node = doc.nodes[key];
    if (!node) return;
    for (const child of getChildIds(node.op)) {
      visit(child);
    }
    order.push(id);
  }

  // Start from roots
  for (const entry of doc.roots) {
    visit(entry.root);
  }
  // Also visit any orphaned nodes
  for (const key of Object.keys(doc.nodes)) {
    visit(Number(key));
  }

  return order;
}
