import { describe, it, expect, beforeEach } from "vitest";
import { useDocumentStore } from "../stores/document-store.js";

describe("updateOperation", () => {
  beforeEach(() => {
    // Reset store to initial state
    useDocumentStore.setState({
      ...useDocumentStore.getState(),
      document: { version: "1", nodes: {}, materials: {}, part_materials: {}, roots: [] },
      parts: [],
      partIndex: new Map(),
      consumedParts: {},
      nextNodeId: 1,
      nextPartNum: 1,
      isDirty: false,
      dirtyNodeIds: new Set(),
      undoStack: [],
      redoStack: [],
    });
  });

  it("updates a node op and marks dirty", () => {
    const store = useDocumentStore.getState();
    // Add a primitive to get a real node
    const partId = store.addPrimitive("cube");
    // Reset dirty state after add
    useDocumentStore.setState({ isDirty: false, dirtyNodeIds: new Set() });

    const state = useDocumentStore.getState();
    const part = state.partIndex.get(partId)!;
    expect(part.kind).toBe("cube");

    // Find the primitive node ID
    const primitiveNodeId =
      "primitiveNodeId" in part ? (part.primitiveNodeId as number) : -1;
    expect(primitiveNodeId).toBeGreaterThan(0);

    // Verify initial size
    const node = state.document.nodes[String(primitiveNodeId)]!;
    expect(node.op.type).toBe("Cube");

    // Update the operation via updateOperation
    useDocumentStore.getState().updateOperation(primitiveNodeId, {
      size: { x: 10, y: 20, z: 30 },
    } as never);

    const after = useDocumentStore.getState();
    expect(after.isDirty).toBe(true);
    expect(after.dirtyNodeIds.has(primitiveNodeId)).toBe(true);

    // Verify the op was updated and type preserved
    const updatedNode = after.document.nodes[String(primitiveNodeId)]!;
    expect(updatedNode.op.type).toBe("Cube");
    expect((updatedNode.op as { size: { x: number; y: number; z: number } }).size).toEqual({
      x: 10,
      y: 20,
      z: 30,
    });
  });

  it("preserves the type discriminant even if updates contain type", () => {
    const store = useDocumentStore.getState();
    const partId = store.addPrimitive("cylinder");
    useDocumentStore.setState({ isDirty: false, dirtyNodeIds: new Set() });

    const state = useDocumentStore.getState();
    const part = state.partIndex.get(partId)!;
    const primitiveNodeId =
      "primitiveNodeId" in part ? (part.primitiveNodeId as number) : -1;

    // Try to overwrite the type — should be preserved
    useDocumentStore.getState().updateOperation(primitiveNodeId, {
      type: "Cube" as never,
      radius: 42,
    } as never);

    const after = useDocumentStore.getState();
    const updatedNode = after.document.nodes[String(primitiveNodeId)]!;
    expect(updatedNode.op.type).toBe("Cylinder");
    expect((updatedNode.op as { radius: number }).radius).toBe(42);
  });

  it("pushes undo unless skipUndo is true", () => {
    const store = useDocumentStore.getState();
    const partId = store.addPrimitive("cube");
    // Clear undo stack from the addPrimitive
    useDocumentStore.setState({ undoStack: [], redoStack: [] });

    const state = useDocumentStore.getState();
    const part = state.partIndex.get(partId)!;
    const primitiveNodeId =
      "primitiveNodeId" in part ? (part.primitiveNodeId as number) : -1;

    // With undo
    useDocumentStore.getState().updateOperation(primitiveNodeId, {
      size: { x: 5, y: 5, z: 5 },
    } as never);
    expect(useDocumentStore.getState().undoStack).toHaveLength(1);

    // With skipUndo
    useDocumentStore.getState().updateOperation(
      primitiveNodeId,
      { size: { x: 7, y: 7, z: 7 } } as never,
      true,
    );
    expect(useDocumentStore.getState().undoStack).toHaveLength(1);
  });

  it("no-ops for non-existent nodeId", () => {
    const store = useDocumentStore.getState();
    store.addPrimitive("cube");
    useDocumentStore.setState({ isDirty: false, dirtyNodeIds: new Set() });

    useDocumentStore.getState().updateOperation(9999, { size: { x: 1, y: 1, z: 1 } } as never);
    expect(useDocumentStore.getState().isDirty).toBe(false);
  });
});
