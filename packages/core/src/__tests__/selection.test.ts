import { describe, it, expect, beforeEach } from "vitest";
import { useUiStore } from "../stores/ui-store.js";
import { selectionItemsEqual } from "../types.js";
import type { SelectionItem } from "../types.js";

describe("selection store", () => {
  beforeEach(() => {
    useUiStore.getState().clearSelection();
    useUiStore.getState().setHoveredItem(null);
    useUiStore.getState().setSelectionFilter("auto");
  });

  it("starts empty", () => {
    const s = useUiStore.getState();
    expect(s.selection).toHaveLength(0);
    expect(s.selectedPartIds.size).toBe(0);
    expect(s.hoveredItem).toBeNull();
    expect(s.hoveredPartId).toBeNull();
    expect(s.selectionFilter).toBe("auto");
  });

  it("part-only writes via select() keep selectedPartIds in sync", () => {
    useUiStore.getState().select("p1");
    const s = useUiStore.getState();
    expect(s.selection).toEqual([{ kind: "part", id: "p1" }]);
    expect(s.selectedPartIds.has("p1")).toBe(true);
    expect(s.selectedPartIds.size).toBe(1);
  });

  it("toggleSelect() round-trips through both shapes", () => {
    useUiStore.getState().toggleSelect("a");
    expect(useUiStore.getState().selectedPartIds.has("a")).toBe(true);
    useUiStore.getState().toggleSelect("a");
    expect(useUiStore.getState().selectedPartIds.has("a")).toBe(false);
    expect(useUiStore.getState().selection).toHaveLength(0);
  });

  it("selectMultiple() preserves order in selection while feeding the Set view", () => {
    useUiStore.getState().selectMultiple(["a", "b", "c"]);
    const s = useUiStore.getState();
    expect(s.selection.map((it) => (it.kind === "part" ? it.id : null))).toEqual([
      "a",
      "b",
      "c",
    ]);
    expect(Array.from(s.selectedPartIds).sort()).toEqual(["a", "b", "c"]);
  });

  it("selectItem() with a face stores the tagged item but excludes it from selectedPartIds", () => {
    const face: SelectionItem = { kind: "face", partId: "p1", faceIndex: 3 };
    useUiStore.getState().selectItem(face);
    const s = useUiStore.getState();
    expect(s.selection).toEqual([face]);
    expect(s.selectedPartIds.size).toBe(0);
  });

  it("toggleItem() handles face / edge / vertex equality", () => {
    const face: SelectionItem = { kind: "face", partId: "p1", faceIndex: 3 };
    const otherFace: SelectionItem = { kind: "face", partId: "p1", faceIndex: 4 };
    useUiStore.getState().toggleItem(face);
    useUiStore.getState().toggleItem(otherFace);
    expect(useUiStore.getState().selection).toHaveLength(2);
    useUiStore.getState().toggleItem(face);
    const remaining = useUiStore.getState().selection;
    expect(remaining).toHaveLength(1);
    expect(selectionItemsEqual(remaining[0]!, otherFace)).toBe(true);
  });

  it("setHoveredPartId() also updates hoveredItem", () => {
    useUiStore.getState().setHoveredPartId("x");
    expect(useUiStore.getState().hoveredItem).toEqual({ kind: "part", id: "x" });
    useUiStore.getState().setHoveredPartId(null);
    expect(useUiStore.getState().hoveredItem).toBeNull();
  });

  it("setHoveredItem() with a non-part clears hoveredPartId", () => {
    useUiStore.getState().setHoveredItem({ kind: "edge", partId: "p1", edgeId: 7 });
    expect(useUiStore.getState().hoveredPartId).toBeNull();
    expect(useUiStore.getState().hoveredItem).toEqual({
      kind: "edge",
      partId: "p1",
      edgeId: 7,
    });
  });

  it("clearSelection() empties both views", () => {
    useUiStore.getState().selectMultiple(["a", "b"]);
    useUiStore.getState().toggleItem({ kind: "face", partId: "a", faceIndex: 0 });
    useUiStore.getState().clearSelection();
    expect(useUiStore.getState().selection).toHaveLength(0);
    expect(useUiStore.getState().selectedPartIds.size).toBe(0);
  });
});

describe("selectionItemsEqual", () => {
  it("matches identical parts", () => {
    expect(
      selectionItemsEqual({ kind: "part", id: "a" }, { kind: "part", id: "a" }),
    ).toBe(true);
  });

  it("rejects different kinds", () => {
    expect(
      selectionItemsEqual(
        { kind: "part", id: "a" },
        { kind: "face", partId: "a", faceIndex: 0 },
      ),
    ).toBe(false);
  });

  it("matches identical sub-feature items", () => {
    expect(
      selectionItemsEqual(
        { kind: "edge", partId: "p", edgeId: 7 },
        { kind: "edge", partId: "p", edgeId: 7 },
      ),
    ).toBe(true);
    expect(
      selectionItemsEqual(
        { kind: "vertex", partId: "p", vertexId: 1 },
        { kind: "vertex", partId: "p", vertexId: 2 },
      ),
    ).toBe(false);
  });

  it("matches sketch segments and constraints", () => {
    expect(
      selectionItemsEqual(
        { kind: "segment", sketchId: "s", index: 3 },
        { kind: "segment", sketchId: "s", index: 3 },
      ),
    ).toBe(true);
    expect(
      selectionItemsEqual(
        { kind: "constraint", sketchId: "s", index: 1 },
        { kind: "constraint", sketchId: "s", index: 2 },
      ),
    ).toBe(false);
  });
});
