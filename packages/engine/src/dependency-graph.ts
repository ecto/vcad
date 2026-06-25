import type { Document, Node, NodeId, CsgOp } from "@vcad/ir";

/**
 * Extract child node IDs from a CsgOp.
 * Returns an array of NodeIds that this operation depends on.
 */
function getChildNodeIds(op: CsgOp): NodeId[] {
  switch (op.type) {
    case "Cube":
    case "Cylinder":
    case "Sphere":
    case "Cone":
    case "Torus":
    case "Empty":
    case "Sketch2D":
    case "ImportedMesh":
      // Leaf nodes - no children
      return [];

    case "Translate":
    case "Rotate":
    case "Scale":
    case "Fillet":
    case "Chamfer":
    case "Shell":
    case "LinearPattern":
    case "CircularPattern":
      // Single child operations
      return [op.child];

    case "Union":
    case "Difference":
    case "Intersection":
      // Binary operations
      return [op.left, op.right];

    case "Extrude":
    case "Revolve":
      // Reference a sketch node
      return [op.sketch];

    case "Sweep":
      return [op.sketch];

    case "Loft":
      return [...op.sketches];

    default:
      return [];
  }
}

/**
 * Dependency graph for tracking node relationships in a document.
 *
 * Allows efficient computation of which nodes need to be re-evaluated
 * when a node changes. Tracks both dependencies (what a node depends on)
 * and dependents (what depends on a node).
 */
export class DependencyGraph {
  /** Map from node ID to the IDs it depends on (children) */
  private dependencies = new Map<NodeId, Set<NodeId>>();

  /** Map from node ID to the IDs that depend on it (parents) */
  private dependents = new Map<NodeId, Set<NodeId>>();

  /** Build the dependency graph from a document */
  build(doc: Document): void {
    this.dependencies.clear();
    this.dependents.clear();

    for (const [idStr, node] of Object.entries(doc.nodes)) {
      const nodeId = Number(idStr) as NodeId;
      const childIds = getChildNodeIds(node.op);

      // Store dependencies
      this.dependencies.set(nodeId, new Set(childIds));

      // Store reverse mapping (dependents)
      for (const childId of childIds) {
        let deps = this.dependents.get(childId);
        if (!deps) {
          deps = new Set();
          this.dependents.set(childId, deps);
        }
        deps.add(nodeId);
      }
    }
  }

  /**
   * Get all nodes affected by changes to the given nodes.
   * Returns the input nodes plus all their transitive dependents.
   *
   * This tells you which cached solids need to be invalidated when
   * a set of nodes change.
   */
  getAffectedNodes(changedNodeIds: Set<NodeId>): Set<NodeId> {
    const affected = new Set<NodeId>();
    const queue = Array.from(changedNodeIds);

    while (queue.length > 0) {
      const nodeId = queue.pop()!;
      if (affected.has(nodeId)) continue;

      affected.add(nodeId);

      // Add all dependents to the queue
      const deps = this.dependents.get(nodeId);
      if (deps) {
        for (const depId of deps) {
          if (!affected.has(depId)) {
            queue.push(depId);
          }
        }
      }
    }

    return affected;
  }

  /**
   * Get the direct dependencies (children) of a node.
   */
  getDependencies(nodeId: NodeId): Set<NodeId> {
    return this.dependencies.get(nodeId) ?? new Set();
  }

  /**
   * Get the direct dependents (parents) of a node.
   */
  getDependents(nodeId: NodeId): Set<NodeId> {
    return this.dependents.get(nodeId) ?? new Set();
  }

  /**
   * Get nodes that should be evaluated in topological order.
   * Leaf nodes come first, then their parents, etc.
   */
  getEvaluationOrder(nodeIds: Set<NodeId>): NodeId[] {
    const visited = new Set<NodeId>();
    const result: NodeId[] = [];

    const visit = (nodeId: NodeId) => {
      if (visited.has(nodeId) || !nodeIds.has(nodeId)) return;
      visited.add(nodeId);

      // Visit dependencies first
      const deps = this.dependencies.get(nodeId);
      if (deps) {
        for (const depId of deps) {
          visit(depId);
        }
      }

      result.push(nodeId);
    };

    for (const nodeId of nodeIds) {
      visit(nodeId);
    }

    return result;
  }

  /**
   * Clear the graph.
   */
  clear(): void {
    this.dependencies.clear();
    this.dependents.clear();
  }
}
