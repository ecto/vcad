/**
 * Convert a vcad Document back to loon source code.
 *
 * Walks the node graph in topological order (dependencies before dependents)
 * and emits `[let nN ...]` bindings for each node, with `[root ...]` entries
 * for scene roots. Materials are emitted first.
 *
 * Handles all CsgOp types:
 * - Primitives: Cube, Cylinder, Sphere, Cone, Empty
 * - Booleans: Union, Difference, Intersection
 * - Transforms: Translate, Rotate, Scale
 * - Sketch ops: Sketch2D, Extrude, Revolve
 * - Features: Shell, Fillet, Chamfer
 * - Patterns: LinearPattern, CircularPattern
 * - Sweep: SweepLine, SweepHelix (via path type)
 * - Loft: open and closed
 * - Text2D: emitted as comment (not round-trippable)
 * - ImportedMesh: emitted as comment (raw mesh data, not parametric)
 */

import type { Document, Node, NodeId, CsgOp } from "@vcad/ir";

/** Convert a Document to loon source code. */
export function documentToLoon(doc: Document): string {
  const lines: string[] = [];
  const emitted = new Set<string>();

  // Header
  lines.push("; Generated from vcad document");
  lines.push("");

  // Emit materials (before geometry so they can be referenced)
  const matEntries = Object.entries(doc.materials);
  if (matEntries.length > 0) {
    for (const [key, mat] of matEntries) {
      if (key === "default") continue;
      lines.push(
        `[let mat-${sanitizeName(key)} [material ${JSON.stringify(mat.name)} ${f(mat.color[0])} ${f(mat.color[1])} ${f(mat.color[2])} ${f(mat.metallic)} ${f(mat.roughness)}]]`,
      );
    }
    lines.push("");
  }

  // Topological sort: emit dependencies first
  const order = topoSort(doc);

  for (const nodeId of order) {
    const node = doc.nodes[String(nodeId)];
    if (!node) continue;
    emitNode(node, doc, lines, emitted);
  }

  // Emit scene roots
  if (doc.roots.length > 0) {
    lines.push("");
    if (doc.roots.length === 1) {
      const entry = doc.roots[0]!;
      const name = nodeName(entry.root, doc);
      const mat = entry.material || "default";
      lines.push(`[root ${name} ${JSON.stringify(mat)}]`);
    } else {
      // Multiple roots → emit as a vec
      lines.push("#[");
      for (const entry of doc.roots) {
        const name = nodeName(entry.root, doc);
        const mat = entry.material || "default";
        lines.push(`  [root ${name} ${JSON.stringify(mat)}]`);
      }
      lines.push("]");
    }
  }

  return lines.join("\n") + "\n";
}

/** Sanitize a name for use as a loon identifier. */
function sanitizeName(name: string): string {
  return name.replace(/[^a-zA-Z0-9_-]/g, "-").toLowerCase();
}

/** Get a readable name for a node — uses node.name if available, else nN. */
function nodeName(id: NodeId, doc: Document): string {
  const node = doc.nodes[String(id)];
  if (node?.name) {
    return sanitizeName(node.name);
  }
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

  const name = nodeName(node.id, doc);
  const expr = opToLoon(node.op, doc);
  if (expr) {
    lines.push(`[let ${name} ${expr}]`);
  }
}

/** Format a number for loon output. Always includes decimal for floats. */
function f(n: number): string {
  if (Number.isInteger(n)) return n.toFixed(1);
  // Avoid excessive precision
  const s = n.toPrecision(10);
  // Strip trailing zeros after decimal
  return s.includes(".") ? s.replace(/0+$/, "").replace(/\.$/, ".0") : s;
}

function ref(id: NodeId, doc: Document): string {
  return nodeName(id, doc);
}

function opToLoon(op: CsgOp, doc: Document): string | null {
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
      return `[union ${ref(op.left, doc)} ${ref(op.right, doc)}]`;

    case "Difference":
      return `[difference ${ref(op.left, doc)} ${ref(op.right, doc)}]`;

    case "Intersection":
      return `[intersection ${ref(op.left, doc)} ${ref(op.right, doc)}]`;

    case "Translate":
      return `[translate ${f(op.offset.x)} ${f(op.offset.y)} ${f(op.offset.z)} ${ref(op.child, doc)}]`;

    case "Rotate":
      return `[rotate ${f(op.angles.x)} ${f(op.angles.y)} ${f(op.angles.z)} ${ref(op.child, doc)}]`;

    case "Scale":
      return `[scale ${f(op.factor.x)} ${f(op.factor.y)} ${f(op.factor.z)} ${ref(op.child, doc)}]`;

    case "Fillet":
      return `[fillet ${f(op.radius)} ${ref(op.child, doc)}]`;

    case "Chamfer":
      return `[chamfer ${f(op.distance)} ${ref(op.child, doc)}]`;

    case "Shell":
      return `[shell ${f(op.thickness)} ${ref(op.child, doc)}]`;

    case "LinearPattern":
      return `[linear-pattern ${f(op.direction.x)} ${f(op.direction.y)} ${f(op.direction.z)} ${op.count} ${f(op.spacing)} ${ref(op.child, doc)}]`;

    case "CircularPattern":
      return `[circular-pattern ${f(op.axis_origin.x)} ${f(op.axis_origin.y)} ${f(op.axis_origin.z)} ${f(op.axis_dir.x)} ${f(op.axis_dir.y)} ${f(op.axis_dir.z)} ${op.count} ${f(op.angle_deg)} ${ref(op.child, doc)}]`;

    case "Sketch2D": {
      const segs = op.segments
        .map((seg) => {
          if (seg.type === "Line") {
            return `    [line ${f(seg.start.x)} ${f(seg.start.y)} ${f(seg.end.x)} ${f(seg.end.y)}]`;
          } else {
            return `    [arc ${f(seg.start.x)} ${f(seg.start.y)} ${f(seg.end.x)} ${f(seg.end.y)} ${f(seg.center.x)} ${f(seg.center.y)} ${seg.ccw}]`;
          }
        })
        .join("\n");
      return [
        `[sketch`,
        `  ${f(op.origin.x)} ${f(op.origin.y)} ${f(op.origin.z)}`,
        `  ${f(op.x_dir.x)} ${f(op.x_dir.y)} ${f(op.x_dir.z)}`,
        `  ${f(op.y_dir.x)} ${f(op.y_dir.y)} ${f(op.y_dir.z)}`,
        `  #[`,
        segs,
        `  ]]`,
      ].join("\n");
    }

    case "Extrude":
      return `[extrude ${f(op.direction.x)} ${f(op.direction.y)} ${f(op.direction.z)} ${ref(op.sketch, doc)}]`;

    case "Revolve":
      return `[revolve ${f(op.axis_origin.x)} ${f(op.axis_origin.y)} ${f(op.axis_origin.z)} ${f(op.axis_dir.x)} ${f(op.axis_dir.y)} ${f(op.axis_dir.z)} ${f(op.angle_deg)} ${ref(op.sketch, doc)}]`;

    case "Sweep": {
      const sk = ref(op.sketch, doc);
      if (op.path.type === "Line") {
        return `[sweep-line ${f(op.path.start.x)} ${f(op.path.start.y)} ${f(op.path.start.z)} ${f(op.path.end.x)} ${f(op.path.end.y)} ${f(op.path.end.z)} ${sk}]`;
      } else {
        // Helix
        return `[sweep-helix ${f(op.path.radius)} ${f(op.path.pitch)} ${f(op.path.height)} ${f(op.path.turns)} ${sk}]`;
      }
    }

    case "Loft": {
      const sketchRefs = op.sketches.map((id) => ref(id, doc)).join(" ");
      if (op.closed) {
        return `[loft-closed #[${sketchRefs}]]`;
      }
      return `[loft #[${sketchRefs}]]`;
    }

    case "Text2D":
      // Text2D can't round-trip through loon yet — emit as comment
      return `; TODO: Text2D "${op.text}" (h=${f(op.height)}) — not yet supported in loon\n; [text2d ...]`;

    case "ImportedMesh":
      // ImportedMesh is raw triangles — can't represent parametrically
      return `; TODO: ImportedMesh (${op.positions.length / 3} vertices, ${op.indices.length / 3} triangles${op.source ? `, source: ${op.source}` : ""}) — not representable in loon`;

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
