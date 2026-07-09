/**
 * The viewer's first-paint fast path: previewGlbFor's content-addressed GLB
 * cache, and attachInlinePreview riding a ready-to-render GLB on a mount
 * result's `_meta` so the iframe never round-trips get_preview_glb.
 */
import { describe, it, expect, beforeAll, beforeEach } from "vitest";
import { Engine } from "@vcad/engine";
import type { Document } from "@vcad/ir";
import { previewGlbFor, previewVersion } from "../tools/preview.js";
import { attachInlinePreview } from "../server.js";
import { documents } from "../tools/session.js";

let engine: Engine;
beforeAll(async () => {
  engine = await Engine.init();
});

function cubeDoc(size = 10): Document {
  return {
    version: "0.1",
    nodes: {
      "1": {
        id: 1,
        name: "cube",
        op: { type: "Cube", size: { x: size, y: size, z: size } },
      },
    },
    materials: {},
    part_materials: {},
    roots: [{ root: 1, material: "default" }],
  } as unknown as Document;
}

const throwingEngine = {
  evaluate: () => {
    throw new Error("evaluate must not run on a cache hit");
  },
} as unknown as Engine;

describe("previewGlbFor cache", () => {
  it("returns a GLB with the document's change token", async () => {
    const doc = cubeDoc();
    const preview = await previewGlbFor(doc, engine);
    expect(preview).not.toBeNull();
    expect(preview!.glb.length).toBeGreaterThan(0);
    expect(preview!.version).toBe(previewVersion(doc));
    expect(preview!.mode).toBeUndefined();
  });

  it("serves an identical document from cache without evaluating", async () => {
    const doc = cubeDoc(17);
    const first = await previewGlbFor(doc, engine);
    expect(first).not.toBeNull();
    // Same content → cache hit: the throwing engine is never consulted.
    const second = await previewGlbFor(cubeDoc(17), throwingEngine);
    expect(second).toEqual(first);
  });

  it("never serves stale geometry — an edit flips the cache key", async () => {
    await previewGlbFor(cubeDoc(21), engine);
    // Different content misses the cache; with a broken engine the parts
    // path yields no meshes → null, proving it recomputed rather than
    // reusing the size-21 entry.
    const changed = await previewGlbFor(cubeDoc(22), throwingEngine);
    expect(changed).toBeNull();
  });
});

describe("attachInlinePreview", () => {
  beforeEach(() => documents.clear());

  it("stamps a ready-to-render preview on the result _meta", async () => {
    documents.set("doc_inline", cubeDoc(30));
    const result: { _meta?: Record<string, unknown> } = {};
    await attachInlinePreview(result, "doc_inline", engine);
    const preview = result._meta?.["vcad.io/preview"] as Record<string, unknown>;
    expect(preview).toBeDefined();
    expect(preview.document_id).toBe("doc_inline");
    expect(typeof preview.glb).toBe("string");
    expect((preview.glb as string).length).toBeGreaterThan(0);
    expect(preview.version).toBe(previewVersion(cubeDoc(30)));
  });

  it("is best-effort: unknown session leaves the result untouched", async () => {
    const result: { _meta?: Record<string, unknown> } = {};
    await attachInlinePreview(result, "doc_missing", engine);
    expect(result._meta).toBeUndefined();
  });

  it("preserves _meta another extension already set", async () => {
    documents.set("doc_keep", cubeDoc(31));
    const result: { _meta?: Record<string, unknown> } = {
      _meta: { "other/ext": true },
    };
    await attachInlinePreview(result, "doc_keep", engine);
    expect(result._meta?.["other/ext"]).toBe(true);
    expect(result._meta?.["vcad.io/preview"]).toBeDefined();
  });
});
