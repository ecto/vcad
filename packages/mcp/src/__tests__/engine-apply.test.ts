/**
 * The engine path of `runMutation`: with a kernel WASM that exposes
 * `WasmDocumentEngine.apply_tool`, engine-lowered tools go through it and the
 * session document is replaced by the engine's `result.document`. The
 * checked-in WASM may predate `apply_tool`, so the engine is stubbed here.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Document } from "@vcad/ir";

const calls: Array<{ tool: string; args: Record<string, unknown>; imported: Document }> = [];
let nextEnvelope: unknown = null;

vi.mock("@vcad/engine", async (orig) => {
  const actual = (await orig()) as Record<string, unknown>;
  return {
    ...actual,
    getKernelWasmSync: () => ({
      WasmDocumentEngine: {
        withReplica: () => {
          let imported: Document | null = null;
          return {
            import_ir: (j: string) => {
              imported = JSON.parse(j) as Document;
            },
            apply_tool: (tool: string, a: string) => {
              calls.push({ tool, args: JSON.parse(a), imported: imported! });
              return JSON.stringify(nextEnvelope);
            },
            free: () => {},
          };
        },
      },
    }),
  };
});

const { dispatchRegistryTool } = await import("../tools/registry-dispatch.js");
const { documents, getSession, registerSession } = await import("../tools/session.js");

function cubes(n: number): Document {
  const nodes: Record<string, unknown> = {};
  const roots = [];
  const partIds: Record<string, string> = {};
  for (let i = 1; i <= n; i++) {
    nodes[String(i)] = { id: i, name: `c${i}`, op: { type: "Cube", size: { x: 1, y: 1, z: 1 } } };
    roots.push({ root: i, material: "default" });
    partIds[String(i)] = `0:${i}`;
  }
  return { version: "0.1", nodes, materials: {}, part_materials: {}, roots, partIds } as unknown as Document;
}

const json = (r: { content: Array<{ text: string }> }) =>
  JSON.parse(r.content[0]!.text) as Record<string, unknown>;

describe("runMutation via the document engine", () => {
  beforeEach(() => {
    documents.clear();
    calls.length = 0;
  });

  it("replaces the session doc with result.document and reports createdFeatureId", () => {
    const id = registerSession(cubes(1));
    nextEnvelope = {
      ok: true,
      result: { document: cubes(2), parts: [], consumedPartIds: [], warnings: [], createdFeatureId: "0:2" },
    };
    const r = dispatchRegistryTool("create", {
      document_id: id,
      type: "cube",
      params: { size: { x: 1, y: 1, z: 1 } },
    });
    const out = json(r);
    expect(calls[0]!.tool).toBe("create");
    expect(calls[0]!.imported.roots).toHaveLength(1);
    expect(out.part_id).toBe("0:2");
    expect(out.node_id).toBe(2);
    expect((out.changed as { added: unknown[] }).added).toHaveLength(1);
    expect(getSession(id).partIds).toEqual({ "1": "0:1", "2": "0:2" });
  });

  it("translates legacy NodeId refs to stable ids before calling the engine", () => {
    const id = registerSession(cubes(3));
    nextEnvelope = { ok: true, result: { document: cubes(2), parts: [], warnings: [] } };
    const out = json(dispatchRegistryTool("delete", { document_id: id, part_id: "3" }));
    expect(calls[0]!.args.part_id).toBe("0:3");
    expect(out.removed).toBe("0:3");
    expect(out.warnings).toBeDefined();
  });

  it("maps engine errors to thrown MCP errors", () => {
    const id = registerSession(cubes(1));
    nextEnvelope = { ok: false, error: { kind: "unknown_id", id: "0:9", message: "no feature" } };
    expect(() => dispatchRegistryTool("delete", { document_id: id, part_id: "0:9" })).toThrow(
      /no part with id "0:9"/,
    );
    expect(getSession(id).roots).toHaveLength(1);
  });

  it("has no planner fallback: `unsupported` is an error and the doc is untouched", () => {
    const id = registerSession(cubes(2));
    const before = JSON.stringify(getSession(id));
    nextEnvelope = { ok: false, error: { kind: "unsupported", tool: "delete", message: "not lowered" } };
    expect(() => dispatchRegistryTool("delete", { document_id: id, part_id: "0:2" })).toThrow(
      /delete: not lowered/,
    );
    expect(calls).toHaveLength(1);
    expect(JSON.stringify(getSession(id))).toBe(before);
  });

  it("ignores the retired VCAD_MCP_ENGINE_APPLY switch: every mutation reaches the engine", () => {
    const id = registerSession(cubes(2));
    process.env.VCAD_MCP_ENGINE_APPLY = "0";
    try {
      nextEnvelope = { ok: true, result: { document: cubes(1), parts: [], warnings: [] } };
      const out = json(dispatchRegistryTool("delete", { document_id: id, part_id: "0:2" }));
      expect(calls.map((c) => c.tool)).toEqual(["delete"]);
      expect(calls[0]!.args.part_id).toBe("0:2");
      expect(out.removed).toBe("0:2");
      expect(getSession(id).roots).toHaveLength(1);
    } finally {
      delete process.env.VCAD_MCP_ENGINE_APPLY;
    }
  });

  it("reports the edited part for in-place transforms", () => {
    const id = registerSession(cubes(2));
    nextEnvelope = { ok: true, result: { document: cubes(2), parts: [], warnings: [] } };
    const out = json(
      dispatchRegistryTool("create", {
        document_id: id,
        type: "translate",
        params: { child: 2, offset: { x: 1, y: 0, z: 0 } },
      }),
    );
    expect((calls[0]!.args.params as { child: unknown }).child).toBe("0:2");
    expect(out.part_id).toBe("0:2");
  });

  it("dispatches place through the engine with stable ids for part anchors", () => {
    const id = registerSession(cubes(2));
    nextEnvelope = { ok: true, result: { document: cubes(2), parts: [], warnings: [] } };
    const out = json(
      dispatchRegistryTool("place", {
        document_id: id,
        part_id: "2",
        from: "bottom",
        to: { part: "1", anchor: "top" },
      }),
    );
    expect(calls).toHaveLength(1);
    expect(calls[0]!.tool).toBe("place");
    expect(calls[0]!.args.part_id).toBe("0:2");
    expect(calls[0]!.args.from).toBe("bottom");
    expect(calls[0]!.args.to).toEqual({ part: "0:1", anchor: "top" });
    expect(out.part_id).toBe("0:2");
  });
});
