import { beforeEach, describe, expect, it } from "vitest";
import type { Document } from "@vcad/ir";
import { dispatchRegistryTool } from "../tools/registry-dispatch.js";
import { documents, getSession, registerSession } from "../tools/session.js";
import { findPartCandidate, migrateLegacyPartId } from "../tools/part-ids.js";

function cubes(): Document {
  const cube = (id: number, name: string) => ({
    id,
    name,
    op: { type: "Cube", size: { x: 1, y: 1, z: 1 } },
  });
  return {
    version: "0.1",
    nodes: { "1": cube(1, "a"), "2": cube(2, "b"), "3": cube(3, "c") },
    materials: {},
    part_materials: {},
    roots: [
      { root: 1, material: "default" },
      { root: 2, material: "default" },
      { root: 3, material: "default" },
    ],
  } as unknown as Document;
}

const json = (r: { content: Array<{ text: string }> }) =>
  JSON.parse(r.content[0]!.text) as Record<string, unknown>;

const listIds = (id: string) =>
  (json(dispatchRegistryTool("read", { document_id: id })).parts as Array<{ id: string }>).map(
    (p) => p.id,
  );

describe("MCP stable part ids", () => {
  beforeEach(() => documents.clear());

  it("assigns ids when a session is registered (migrated on load)", () => {
    const id = registerSession(cubes());
    expect(getSession(id).partIds).toEqual({ "1": "0:1", "2": "0:2", "3": "0:3" });
    expect(listIds(id)).toEqual(["0:1", "0:2", "0:3"]);
  });

  it("deleting an earlier part does not shift the ids of later parts", () => {
    const id = registerSession(cubes());
    const [first, second, third] = listIds(id);
    const del = json(dispatchRegistryTool("delete", { document_id: id, part_id: first }));
    expect(del.removed).toBe(first);
    expect(del.warnings).toBeUndefined();
    expect(listIds(id)).toEqual([second, third]);
    // The surviving part is still addressed by the same id.
    const read = json(dispatchRegistryTool("read", { document_id: id, part_id: third }));
    expect(read.part_id).toBe(third);
    expect(read.node_id).toBe("3");
  });

  it("accepts a legacy digit id with a deprecation warning", () => {
    const id = registerSession(cubes());
    const res = json(
      dispatchRegistryTool("set_material", { document_id: id, part_id: "2", material: "steel" }),
    );
    expect(res.part_id).toBe("0:2");
    expect(String((res.warnings as string[])[0])).toContain("deprecated");
    expect(json(dispatchRegistryTool("read", { document_id: id, part_id: "c" })).part_id).toBe(
      "0:3",
    );
  });

  it("resolves candidates and migrates persisted legacy ids", () => {
    const id = registerSession(cubes());
    const doc = getSession(id);
    const cands = [{ id: "0:1", name: "a" }, { id: "0:3", name: "c" }, { id: "arm" }];
    expect(findPartCandidate(doc, cands, "3")?.id).toBe("0:3");
    expect(findPartCandidate(doc, cands, "a")?.id).toBe("0:1");
    expect(findPartCandidate(doc, cands, "arm")?.id).toBe("arm");
    expect(migrateLegacyPartId(doc, "3")).toBe("0:3");
    expect(migrateLegacyPartId(doc, "0:2")).toBe("0:2");
    expect(migrateLegacyPartId(doc, "rotor")).toBe("rotor");
  });
});
