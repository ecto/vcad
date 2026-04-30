/**
 * v2 surface tests — handles, envelope, build, read, query, inspect,
 * export, share. The chunk that doesn't need a real Engine is fully
 * covered; engine-dependent assertions reuse the same `Engine.init()`
 * dance the legacy tests do.
 */

import { describe, it, expect, beforeAll } from "vitest";
import { Engine } from "@vcad/engine";
import {
  formatHandle,
  parseHandle,
  resolveRef,
  isHandle,
  getDocStore,
} from "../handles.js";
import { buildTool } from "../tools/build.js";
import { readTool } from "../tools/read.js";
import { queryTool } from "../tools/query.js";
import { inspectV2 } from "../tools/inspect-v2.js";
import { exportV2 } from "../tools/export-v2.js";
import { shareV2 } from "../tools/share-v2.js";

let engine: Engine;

beforeAll(async () => {
  engine = await Engine.init();
});

function envelope(result: { content: Array<{ type: string; [k: string]: unknown }> }): {
  ok: boolean;
  doc?: string;
  result?: unknown;
  stats?: { parts: number; nodes: number; volume_mm3?: number };
  error?: { code: string; message: string };
} {
  const text = result.content.find((c) => c.type === "text");
  if (!text) throw new Error("envelope missing text content");
  return JSON.parse((text as { text: string }).text);
}

describe("handles", () => {
  it("formats and parses a versioned handle", () => {
    const h = formatHandle("a1b2c3d4-0000-0000-0000-000000000000", 3);
    expect(h).toBe("vcad:doc:a1b2c3d4-0000-0000-0000-000000000000@3");
    const parsed = parseHandle(h);
    expect(parsed.docId).toBe("a1b2c3d4-0000-0000-0000-000000000000");
    expect(parsed.version).toBe(3);
  });

  it("formats and parses an unversioned handle", () => {
    const h = formatHandle("a1b2c3d4-0000-0000-0000-000000000000");
    expect(h).toBe("vcad:doc:a1b2c3d4-0000-0000-0000-000000000000");
    expect(parseHandle(h).version).toBeUndefined();
  });

  it("isHandle predicate", () => {
    expect(isHandle("vcad:doc:abcdef12@7")).toBe(true);
    expect(isHandle("vcad:doc:abcdef12")).toBe(true);
    expect(isHandle("not a handle")).toBe(false);
    expect(isHandle({ id: 1 })).toBe(false);
  });

  it("rejects malformed input", () => {
    expect(() => parseHandle("garbage")).toThrow();
    expect(() => parseHandle("vcad:doc:")).toThrow();
  });

  it("dedups identical content within the same docId", () => {
    const store = getDocStore();
    store.reset();
    const h1 = store.store({
      version: "0.1",
      nodes: { "1": { id: 1, name: null, op: { type: "Cube", size: { x: 1, y: 1, z: 1 } } } },
      materials: {},
      part_materials: {},
      roots: [{ root: 1, material: "default" }],
    });
    const { docId } = parseHandle(h1);
    const h2 = store.store(
      {
        version: "0.1",
        nodes: { "1": { id: 1, name: null, op: { type: "Cube", size: { x: 1, y: 1, z: 1 } } } },
        materials: {},
        part_materials: {},
        roots: [{ root: 1, material: "default" }],
      },
      { docId },
    );
    expect(h1).toBe(h2);
  });
});

describe("build (v2)", () => {
  it("creates a fresh document with a single primitive cube", () => {
    const r = buildTool(
      {
        ops: [
          { op: "primitive", kind: "cube", size: { x: 10, y: 10, z: 10 }, name: "plate" },
        ],
      },
      engine,
    );
    const env = envelope(r);
    expect(env.ok).toBe(true);
    expect(env.doc).toMatch(/^vcad:doc:[0-9a-f-]+@1$/i);
    expect(env.stats?.parts).toBe(1);
    expect(env.stats?.volume_mm3).toBeCloseTo(1000, 0);
    expect((env.result as { added_nodes: number[] }).added_nodes).toHaveLength(1);
    expect((env.result as { named_nodes: Record<string, number> }).named_nodes.plate).toBeDefined();
  });

  it("composes a difference: cube minus cylinder", () => {
    const r = buildTool(
      {
        ops: [
          { op: "primitive", kind: "cube", size: { x: 20, y: 20, z: 5 }, name: "plate" },
          { op: "primitive", kind: "cylinder", radius: 3, height: 5, name: "hole" },
          { op: "difference", subject: "plate", tools: ["hole"], name: "drilled" },
        ],
      },
      engine,
    );
    const env = envelope(r);
    expect(env.ok).toBe(true);
    expect(env.stats?.parts).toBe(1);
    // 20*20*5 - π*3²*5 ≈ 2000 - 141 ≈ 1859 (with mesh approximation)
    expect(env.stats?.volume_mm3).toBeGreaterThan(1700);
    expect(env.stats?.volume_mm3).toBeLessThan(2000);
  });

  it("edits a prior handle by appending a fillet", () => {
    const first = buildTool(
      { ops: [{ op: "primitive", kind: "cube", size: { x: 10, y: 10, z: 10 }, name: "plate" }] },
      engine,
    );
    const firstEnv = envelope(first);
    const handle = firstEnv.doc!;

    const second = buildTool(
      { doc: handle, ops: [{ op: "fillet", target: "plate", edges: [{ node: "plate", edge: 0 }], radius: 1 }] },
      engine,
    );
    const env = envelope(second);
    expect(env.ok).toBe(true);
    expect(env.doc).not.toBe(handle);
    expect(env.doc).toContain(parseHandle(handle).docId);
    expect(env.stats?.parts).toBe(1);
  });

  it("rejects an unsupported op clearly", () => {
    const r = buildTool(
      { ops: [{ op: "sketch", name: "s", plane: { kind: "xy" }, entities: [] }] },
      engine,
    );
    const env = envelope(r);
    expect(env.ok).toBe(false);
    expect(env.error?.code).toBe("op_failed");
    expect(env.error?.message).toContain("sketch");
  });

  it("supports raw_ir as an escape hatch", () => {
    const r = buildTool(
      {
        ops: [
          {
            op: "raw_ir",
            nodes: [
              { id: 1, name: "raw", op: { type: "Sphere", radius: 5, segments: 16 } },
            ],
            roots: [1],
          },
        ],
      },
      engine,
    );
    const env = envelope(r);
    expect(env.ok).toBe(true);
    expect(env.stats?.parts).toBe(1);
  });
});

describe("read (v2)", () => {
  it("hydrates a handle to JSON IR", () => {
    const built = buildTool(
      { ops: [{ op: "primitive", kind: "sphere", radius: 5, name: "ball" }] },
      engine,
    );
    const handle = envelope(built).doc!;
    const r = readTool({ doc: handle, format: "json" }, engine);
    const env = envelope(r);
    expect(env.ok).toBe(true);
    const ir = JSON.parse((env.result as { ir: string }).ir);
    expect(ir.version).toBe("0.1");
    expect(ir.roots).toHaveLength(1);
  });

  it("supports vcode format", () => {
    const built = buildTool(
      { ops: [{ op: "primitive", kind: "cube", size: { x: 1, y: 1, z: 1 } }] },
      engine,
    );
    const r = readTool({ doc: envelope(built).doc!, format: "vcode" }, engine);
    const env = envelope(r);
    const out = (env.result as { ir: string; format: string });
    expect(out.format).toBe("vcode");
    expect(out.ir).toMatch(/^#/);
  });
});

describe("query (v2)", () => {
  it("returns a feature tree", () => {
    const built = buildTool(
      {
        ops: [
          { op: "primitive", kind: "cube", size: { x: 5, y: 5, z: 5 }, name: "box" },
          { op: "fillet", target: "box", edges: [{ node: "box", edge: 0 }], radius: 0.5 },
        ],
      },
      engine,
    );
    const r = queryTool({ doc: envelope(built).doc!, q: { kind: "tree" } }, engine);
    const env = envelope(r);
    const tree = env.result as Array<{ op: string; children: unknown[] }>;
    expect(tree).toHaveLength(1);
    expect(tree[0].op).toBe("Fillet");
    expect(tree[0].children).toHaveLength(1);
  });

  it("lists parts", () => {
    const built = buildTool(
      {
        ops: [
          { op: "primitive", kind: "cube", size: { x: 1, y: 1, z: 1 }, name: "a" },
          { op: "primitive", kind: "sphere", radius: 1, name: "b" },
        ],
      },
      engine,
    );
    const r = queryTool({ doc: envelope(built).doc!, q: { kind: "list", of: "parts" } }, engine);
    const list = envelope(r).result as Array<{ name: string }>;
    expect(list.map((p) => p.name)).toEqual(["a", "b"]);
  });

  it("finds a node by name", () => {
    const built = buildTool(
      { ops: [{ op: "primitive", kind: "cube", size: { x: 1, y: 1, z: 1 }, name: "needle" }] },
      engine,
    );
    const r = queryTool({ doc: envelope(built).doc!, q: { kind: "find", name: "needle" } }, engine);
    const found = envelope(r).result as { id: number; node: { name: string } } | null;
    expect(found?.node.name).toBe("needle");
  });
});

describe("inspect (v2)", () => {
  it("returns aggregate + per-part properties", () => {
    const built = buildTool(
      { ops: [{ op: "primitive", kind: "cube", size: { x: 10, y: 10, z: 10 }, name: "block" }] },
      engine,
    );
    const r = inspectV2({ doc: envelope(built).doc! }, engine);
    const env = envelope(r);
    const res = env.result as {
      aggregate: { volume_mm3: number; triangles: number };
      per_part: Record<string, unknown>;
    };
    expect(res.aggregate.volume_mm3).toBeCloseTo(1000, 0);
    expect(res.aggregate.triangles).toBeGreaterThan(0);
    expect(res.per_part.block).toBeDefined();
  });
});

describe("export (v2)", () => {
  it("returns an embedded base64 STL by default", () => {
    const built = buildTool(
      { ops: [{ op: "primitive", kind: "cube", size: { x: 1, y: 1, z: 1 } }] },
      engine,
    );
    const r = exportV2({ doc: envelope(built).doc!, format: "stl" }, engine);
    const env = envelope(r);
    const res = env.result as {
      resource: { kind: string; mime: string; data_base64: string };
      bytes: number;
      format: string;
    };
    expect(res.resource.kind).toBe("embedded");
    expect(res.resource.mime).toBe("model/stl");
    expect(res.bytes).toBeGreaterThan(84);
    expect(res.format).toBe("stl");
  });

  it("rejects local-mode write when MCP_LOCAL is unset", () => {
    const prev = process.env.MCP_LOCAL;
    delete process.env.MCP_LOCAL;
    try {
      const built = buildTool(
        { ops: [{ op: "primitive", kind: "cube", size: { x: 1, y: 1, z: 1 } }] },
        engine,
      );
      const r = exportV2(
        { doc: envelope(built).doc!, format: "stl", target: { path: "out.stl" } },
        engine,
      );
      const env = envelope(r);
      expect(env.ok).toBe(false);
      expect(env.error?.code).toBe("local_disabled");
    } finally {
      if (prev !== undefined) process.env.MCP_LOCAL = prev;
    }
  });
});

describe("share (v2)", () => {
  it("returns a vcad.io URL with byte counts", () => {
    const built = buildTool(
      { ops: [{ op: "primitive", kind: "cube", size: { x: 1, y: 1, z: 1 } }] },
      engine,
    );
    const r = shareV2({ doc: envelope(built).doc!, name: "test" }, engine);
    const env = envelope(r);
    const res = env.result as {
      url: string;
      vcode_bytes: number;
      encoded_bytes: number;
    };
    expect(res.url).toContain("vcad.io");
    expect(res.url).toContain("doc=");
    expect(res.vcode_bytes).toBeGreaterThan(0);
    expect(res.encoded_bytes).toBeGreaterThan(0);
  });
});
