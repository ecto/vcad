import { describe, expect, it } from "vitest";
import type { Document } from "@vcad/ir";
import {
  ensurePartIds,
  forgetPartId,
  movePartId,
  partIdDeprecation,
  partIdForRoot,
  rootForPartId,
} from "../commands/part-ids.js";
import { listPartsFromDocument } from "../commands/document-mutations.js";

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
    doc.roots.splice(0, 1);
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
    doc.roots.splice(2, 1);
    ensurePartIds(doc);
    doc.nodes["13"] = { id: 13, name: "d", op: { type: "Sphere", radius: 1 } } as never;
    doc.roots.push({ root: 13, material: "default" } as never);
    const minted = partIdForRoot(doc, 2);
    expect(minted).not.toBe("0:12");
    expect(Number(minted.split(":")[1])).toBeGreaterThan(12);
    expect(new Set(Object.values(doc.partIds!)).size).toBe(Object.keys(doc.partIds!).length);
  });

  it("a part keeps its id when a feature consumes and re-roots it", () => {
    const doc = ensurePartIds(threeParts());
    // Union(4, 9) re-roots part "0:4" at the new node and consumes "0:9".
    doc.nodes["13"] = { id: 13, name: "u", op: { type: "Union", left: 4, right: 9 } } as never;
    movePartId(doc, 4, 13);
    forgetPartId(doc, 9);
    doc.roots.splice(0, 2, { root: 13, material: "default" } as never);
    ensurePartIds(doc);
    expect(doc.roots.map((r) => partIdForRoot(doc, r))).toEqual(["0:4", "0:12"]);
  });

  it("resolves stable, legacy node-id and name (never position) spellings", () => {
    const doc = ensurePartIds(threeParts());
    expect(rootForPartId(doc, "0:9")).toMatchObject({ index: 1, matchedBy: "stable" });
    expect(rootForPartId(doc, "12")).toMatchObject({ index: 2, partId: "0:12", matchedBy: "legacy-node-id" });
    // "1" is no root's NodeId: a stale id misses instead of hitting doc.roots[1].
    expect(rootForPartId(doc, "1")).toBeUndefined();
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
