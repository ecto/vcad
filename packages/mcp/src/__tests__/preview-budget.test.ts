import { describe, it, expect, beforeAll } from "vitest";
import { Engine } from "@vcad/engine";
import {
  PREVIEW_MAX_BASE64,
  fitPreviewToBudget,
  decimateMesh,
} from "../tools/preview.js";
import type { GlbMesh } from "../export/glb.js";
import type { Document } from "@vcad/ir";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { createServer } from "../server.js";
import { documents } from "../tools/session.js";

/**
 * Covers the OVER-CAP branch end to end: a document whose preview GLB exceeds
 * INLINE_PREVIEW_MAX_BASE64 used to attach no inline `_meta` preview and then
 * fall through to a fetch path with no ceiling of its own — guaranteed to blow
 * the transport's result ceiling and surface as a bare "preview unavailable".
 * The budget now belongs to the shared build path, so over-cap documents come
 * back degraded-but-renderable instead of not at all.
 */

/** A dense sphere-ish mesh: `n` triangles of real, non-degenerate geometry
 *  spread over a unit sphere, so vertex clustering actually has something to
 *  collapse (unlike a random point cloud). */
function denseMesh(name: string, triangles: number): GlbMesh {
  const positions: number[] = [];
  const indices: number[] = [];
  const normals: number[] = [];
  const golden = Math.PI * (3 - Math.sqrt(5));
  for (let t = 0; t < triangles; t++) {
    for (let k = 0; k < 3; k++) {
      const i = t * 3 + k;
      const y = 1 - (i / (triangles * 3)) * 2;
      const r = Math.sqrt(Math.max(0, 1 - y * y));
      const th = golden * i;
      const x = Math.cos(th) * r;
      const z = Math.sin(th) * r;
      positions.push(x * 50, y * 50, z * 50);
      normals.push(x, y, z);
      indices.push(i);
    }
  }
  return {
    name,
    positions: new Float32Array(positions),
    indices: new Uint32Array(indices),
    normals: new Float32Array(normals),
    color: [0.7, 0.7, 0.7],
    metallic: 0.1,
    roughness: 0.6,
  };
}

function trianglesOf(mesh: GlbMesh): number {
  return mesh.indices.length / 3;
}

/** A document heavy enough to exceed PREVIEW_MAX_BASE64 once tessellated. */
function heavySphereDoc(n: number): Document {
  const nodes: Record<string, unknown> = {};
  const roots: Array<{ root: number; material: string }> = [];
  for (let i = 1; i <= n; i++) {
    nodes[String(i)] = { id: i, name: `s${i}`, op: { type: "Sphere", radius: 20 } };
    roots.push({ root: i, material: "default" });
  }
  return {
    version: "0.1",
    nodes,
    materials: {},
    part_materials: {},
    roots,
  } as unknown as Document;
}

describe("over-cap document through the viewer's fetch path", () => {
  let engine: Engine;
  beforeAll(async () => {
    engine = await Engine.init();
  }, 60_000);

  it("get_preview_glb returns bounded, renderable geometry (not a dropped payload)", async () => {
    documents.clear();
    const server = await createServer(engine, { user: null });
    const [clientT, serverT] = InMemoryTransport.createLinkedPair();
    const client = new Client({ name: "test", version: "0.0.0" }, { capabilities: {} });
    await Promise.all([client.connect(clientT), server.connect(serverT)]);

    const docId = "over-cap-doc";
    documents.set(docId, heavySphereDoc(40));

    const res = (await client.callTool({
      name: "get_preview_glb",
      arguments: { document_id: docId },
    })) as { isError?: boolean; content: Array<{ type: string; text: string }> };
    expect(res.isError ?? false).toBe(false);
    const payload = JSON.parse(res.content[0].text) as {
      _vcad_glb: string | null;
      degraded?: boolean;
      error?: string;
    };

    // The regression: this used to be an unbounded payload the transport
    // dropped, leaving the viewer on a bare "preview unavailable".
    expect(payload.error).toBeUndefined();
    expect(typeof payload._vcad_glb).toBe("string");
    expect(payload._vcad_glb!.length).toBeLessThanOrEqual(PREVIEW_MAX_BASE64);
    expect(payload.degraded).toBe(true);

    // Renderable: a well-formed GLB container with actual meshes in it.
    const bytes = Buffer.from(payload._vcad_glb!, "base64");
    expect(bytes.subarray(0, 4).toString("ascii")).toBe("glTF");
    const jsonLen = bytes.readUInt32LE(12);
    const gltf = JSON.parse(bytes.subarray(20, 20 + jsonLen).toString()) as {
      meshes?: unknown[];
      nodes?: unknown[];
    };
    expect(gltf.meshes?.length ?? 0).toBeGreaterThan(0);
    expect(gltf.nodes?.length ?? 0).toBeGreaterThan(0);

    await client.close();
    await server.close();
  }, 120_000);
});

describe("preview payload budget", () => {
  // buildGlb serializes through the kernel WASM writer.
  beforeAll(async () => {
    await Engine.init();
  });

  it("leaves a small document at full fidelity", () => {
    const meshes = [denseMesh("part_0", 200)];
    const built = fitPreviewToBudget(meshes);
    expect(built.glb.length).toBeLessThanOrEqual(PREVIEW_MAX_BASE64);
    expect(built.degraded).toBeUndefined();
    expect(built.oversize).toBeUndefined();
  });

  it("fits an over-cap document to the budget instead of dropping it", () => {
    // ~90k triangles across 30 parts — the shape of the reported repro
    // (31 filleted parts, ~81k triangles, 9.1M base64 chars).
    const meshes = Array.from({ length: 30 }, (_, i) => denseMesh(`part_${i}`, 3000));
    const full = fitPreviewToBudget([meshes[0]]);
    expect(full.glb.length).toBeGreaterThan(0);

    const built = fitPreviewToBudget(meshes);
    // The whole point: still renderable geometry, and inside the ceiling.
    expect(built.glb.length).toBeLessThanOrEqual(PREVIEW_MAX_BASE64);
    expect(built.degraded).toBe(true);
    expect(built.oversize).toBeUndefined();
    expect(built.glb.length).toBeGreaterThan(0);
    // Valid GLB container, not a truncated payload.
    const bytes = Buffer.from(built.glb, "base64");
    expect(bytes.subarray(0, 4).toString("ascii")).toBe("glTF");
    expect(bytes.readUInt32LE(8)).toBe(bytes.length);
  });

  it("decimation preserves per-part mesh identity and materials", () => {
    const meshes = Array.from({ length: 4 }, (_, i) => denseMesh(`part_${i}`, 5000));
    const reduced = meshes.map((m) => decimateMesh(m, 4, [-50, -50, -50]));
    expect(reduced.map((m) => m.name)).toEqual(meshes.map((m) => m.name));
    for (let i = 0; i < reduced.length; i++) {
      expect(reduced[i].color).toEqual(meshes[i].color);
      expect(trianglesOf(reduced[i])).toBeLessThan(trianglesOf(meshes[i]));
      expect(trianglesOf(reduced[i])).toBeGreaterThan(0);
      // Normals are recomputed, not carried over stale — one per vertex.
      expect(reduced[i].normals!.length).toBe(reduced[i].positions.length);
    }
  });

  it("decimation emits only in-range, non-degenerate triangles", () => {
    const mesh = denseMesh("part_0", 4000);
    const reduced = decimateMesh(mesh, 6, [-50, -50, -50]);
    const vertCount = reduced.positions.length / 3;
    for (let t = 0; t < reduced.indices.length; t += 3) {
      const [a, b, c] = [
        reduced.indices[t],
        reduced.indices[t + 1],
        reduced.indices[t + 2],
      ];
      for (const i of [a, b, c]) {
        expect(i).toBeGreaterThanOrEqual(0);
        expect(i).toBeLessThan(vertCount);
      }
      expect(new Set([a, b, c]).size).toBe(3);
    }
    for (const v of reduced.positions) expect(Number.isFinite(v)).toBe(true);
    for (const v of reduced.normals as Float32Array) {
      expect(Number.isFinite(v)).toBe(true);
    }
  });
});
