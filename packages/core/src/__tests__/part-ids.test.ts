import { describe, expect, it } from "vitest";
import type { Document } from "@vcad/ir";
import {
  ensurePartIds,
  partIdDeprecation,
  partIdForRoot,
  rootForPartId,
} from "../commands/part-ids.js";
import { applyToolOutcome, listPartsFromDocument } from "../commands/document-mutations.js";

function cube(id: number, name: string) {
  return { id, name, op: { type: "Cube", size: { x: 1, y: 1, z: 1 } } };
}

function threeParts(): Document {
  return {
    version: "0.1",
    nodes: {
      "4": cube(4, "a"),
      "9": cube(9, "b"),
      "12": cube(12, "c"),
    },
    materials: {},
    part_materials: {},
    roots: [
      { root: 4, material: "default" },
      { root: 9, material: "default" },
      { root: 12, material: "default" },
    ],
  } as unknown as Document;
}

describe("part ids", () => {
  it("migrates a document without partIds deterministically", () => {
    const a = ensurePartIds(threeParts());
    const b = ensurePartIds(threeParts());
    expect(a.partIds).toEqual({ "4": "0:4", "9": "0:9", "12": "0:12" });
    expect(b.partIds).toEqual(a.partIds);
  });

  it("ids of later parts do not shift when an earlier part is deleted", () => {
    const doc = ensurePartIds(threeParts());
    const before = listPartsFromDocument(doc).map((p) => p.id);
    applyToolOutcome(doc, { kind: "remove_part", part_id: before[0]! });
    ensurePartIds(doc);
    const after = listPartsFromDocument(doc).map((p) => p.id);
    expect(after).toEqual([before[1], before[2]]);
    expect(rootForPartId(doc, before[2]!)?.entry.root).toBe(12);
  });

  it("mints above every existing seq, never colliding", () => {
    const doc = threeParts();
    doc.partIds = { "4": "1:7", "9": "0:3" };
    expect(partIdForRoot(doc, 2)).toBe("0:12");
    // A new part after a delete does not reuse the freed id.
    applyToolOutcome(doc, { kind: "remove_part", part_id: "0:12" });
    const added = applyToolOutcome(doc, {
      kind: "add_feature",
      op: { type: "Sphere", radius: 1 },
    } as never);
    expect(added.partId).toBe("0:10");
    expect(new Set(Object.values(doc.partIds!)).size).toBe(Object.keys(doc.partIds!).length);
  });

  it("a part keeps its id when a feature consumes and re-roots it", () => {
    const doc = ensurePartIds(threeParts());
    const added = applyToolOutcome(doc, {
      kind: "add_feature",
      op: { type: "Union", left: 4, right: 9 },
    } as never);
    expect(added.partId).toBe("0:4");
    expect(doc.roots.map((r) => partIdForRoot(doc, r))).toEqual(["0:4", "0:12"]);
  });

  it("resolves stable, legacy node-id, legacy index and name spellings", () => {
    const doc = ensurePartIds(threeParts());
    expect(rootForPartId(doc, "0:9")).toMatchObject({ index: 1, matchedBy: "stable" });
    expect(rootForPartId(doc, "12")).toMatchObject({ index: 2, partId: "0:12", matchedBy: "legacy-node-id" });
    expect(rootForPartId(doc, "1")).toMatchObject({ index: 1, matchedBy: "legacy-index" });
    expect(rootForPartId(doc, "c")).toMatchObject({ index: 2, matchedBy: "name" });
    expect(rootForPartId(doc, "0:99")).toBeUndefined();
    expect(rootForPartId(doc, "nope")).toBeUndefined();
    expect(partIdDeprecation("12", rootForPartId(doc, "12"))).toContain('"0:12"');
    expect(partIdDeprecation("0:9", rootForPartId(doc, "0:9"))).toBeUndefined();
  });

  it("prunes ids of roots that left the scene", () => {
    const doc = ensurePartIds(threeParts());
    doc.roots.splice(0, 1);
    ensurePartIds(doc);
    expect(doc.partIds).toEqual({ "9": "0:9", "12": "0:12" });
  });
});
