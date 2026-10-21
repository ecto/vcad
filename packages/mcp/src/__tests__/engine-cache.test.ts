/**
 * Per-session document-engine cache: engine-lowered mutations reuse one
 * `WasmDocumentEngine` per session while the session IR is exactly what that
 * engine last produced, and re-import on any direct IR write. The checked-in
 * WASM may predate `apply_tool`, so the engine is stubbed (as in
 * engine-apply.test.ts).
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Document } from "@vcad/ir";

interface StubEngine {
  serial: number;
  imports: number;
  freed: boolean;
}
const engines: StubEngine[] = [];
const importIr = vi.fn();
let nextDoc: () => Document = () => cubes(1);

vi.mock("@vcad/engine", async (orig) => {
  const actual = (await orig()) as Record<string, unknown>;
  return {
    ...actual,
    getKernelWasmSync: () => ({
      WasmDocumentEngine: {
        withReplica: () => {
          const rec: StubEngine = { serial: engines.length, imports: 0, freed: false };
          engines.push(rec);
          return {
            import_ir: (j: string) => {
              if (rec.freed) throw new Error("use after free");
              rec.imports++;
              importIr(j);
            },
            apply_tool: () => {
              if (rec.freed) throw new Error("use after free");
              return JSON.stringify({
                ok: true,
                result: { document: nextDoc(), parts: [], warnings: [], createdFeatureId: "0:1" },
              });
            },
            free: () => {
              rec.freed = true;
            },
          };
        },
      },
    }),
  };
});

const { dispatchRegistryTool } = await import("../tools/registry-dispatch.js");
const {
  documents,
  getSession,
  registerSession,
  recordHistorySnapshot,
  hasCachedEngine,
  closeDocument,
  runInSessionScope,
} = await import("../tools/session.js");

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

const setMaterial = (id: string) =>
  dispatchRegistryTool("set_material", { document_id: id, part_id: "0:1", material: "steel" });

describe("per-session engine cache", () => {
  beforeEach(() => {
    documents.clear();
    engines.length = 0;
    importIr.mockClear();
    nextDoc = () => cubes(1);
  });

  it("reuses one engine across consecutive engine mutations", () => {
    const id = registerSession(cubes(1));
    setMaterial(id);
    setMaterial(id);
    setMaterial(id);
    expect(engines).toHaveLength(1);
    expect(importIr).toHaveBeenCalledTimes(1);
    expect(engines[0]!.freed).toBe(false);
    expect(hasCachedEngine(id)).toBe(true);
  });

  it("re-imports after a direct IR write (in place via snapshot, or documents.set)", () => {
    const id = registerSession(cubes(1));
    setMaterial(id);
    expect(importIr).toHaveBeenCalledTimes(1);

    // A non-engine writer mutating in place: the dispatch layer snapshots it
    // first, which invalidates the engine.
    recordHistorySnapshot(id, "set_parameters");
    (getSession(id).nodes["1"] as { name: string }).name = "renamed";
    setMaterial(id);
    expect(importIr).toHaveBeenCalledTimes(2);
    expect(engines).toHaveLength(2);
    expect(engines[0]!.freed).toBe(true);
    expect(JSON.parse(importIr.mock.calls[1]![0] as string).nodes["1"].name).toBe("renamed");

    // An engine-managed tool's own snapshot does NOT invalidate.
    recordHistorySnapshot(id, "set_material");
    setMaterial(id);
    expect(importIr).toHaveBeenCalledTimes(2);

    // Wholesale replacement (undo, batch edit, loon, ...) via documents.set.
    documents.set(id, cubes(1));
    setMaterial(id);
    expect(importIr).toHaveBeenCalledTimes(3);
    expect(engines[1]!.freed).toBe(true);
  });

  it("frees the engine and re-imports after a refused mutation", () => {
    const id = registerSession(cubes(1));
    setMaterial(id);
    expect(hasCachedEngine(id)).toBe(true);
    const good = nextDoc;
    nextDoc = () => {
      throw new Error("engine blew up");
    };
    expect(() => setMaterial(id)).toThrow(/engine blew up/);
    // A failed apply never leaves its (possibly half-applied) engine cached.
    expect(hasCachedEngine(id)).toBe(false);
    expect(engines[0]!.freed).toBe(true);
    nextDoc = good;
    setMaterial(id);
    expect(engines).toHaveLength(2);
    expect(importIr).toHaveBeenCalledTimes(2);
  });

  it("frees the engine when the session is closed or the cache cleared", () => {
    const a = registerSession(cubes(1));
    setMaterial(a);
    closeDocument({ document_id: a });
    expect(engines[0]!.freed).toBe(true);
    expect(hasCachedEngine(a)).toBe(false);

    const b = registerSession(cubes(1));
    setMaterial(b);
    documents.clear();
    expect(engines[1]!.freed).toBe(true);
  });

  it("isolates scoped caches: tenants never share an engine", () => {
    const userA = { id: "a" } as never;
    const userB = { id: "b" } as never;
    // The same document_id in two scopes is two independent sessions.
    const id = "doc_shared";
    runInSessionScope(userA, () => {
      documents.set(id, cubes(1));
      setMaterial(id);
      setMaterial(id);
      expect(hasCachedEngine(id)).toBe(true);
    });
    expect(engines).toHaveLength(1);
    // Scope end frees its engines.
    expect(engines[0]!.freed).toBe(true);

    runInSessionScope(userB, () => {
      expect(hasCachedEngine(id)).toBe(false);
      documents.set(id, cubes(1));
      setMaterial(id);
    });
    expect(engines).toHaveLength(2);
    expect(importIr).toHaveBeenCalledTimes(2);
    // Neither leaked into the process-wide fallback cache.
    expect(hasCachedEngine(id)).toBe(false);
  });
});
