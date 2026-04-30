/**
 * End-to-end smoke: a realistic agent session over the v2 surface.
 *
 * Walks through:
 *   1. Build a 50×30×5 mm aluminum plate
 *   2. Edit the handle to drill a hole (returns @2)
 *   3. Edit again to add a fillet (@3)
 *   4. `inspect` aggregate properties + per-part
 *   5. `query` the feature tree
 *   6. `render` a preview PNG
 *   7. `drawing` a 4-up orthographic SVG
 *   8. `export` to STL (embedded resource)
 *   9. `share` for a vcad.io URL
 *  10. `read` the IR back as JSON
 *
 * Each step asserts the envelope shape and that the handle chains
 * forward through the document_versions sequence.
 */

import { describe, it, expect, beforeAll } from "vitest";
import { Engine } from "@vcad/engine";
import { buildTool } from "../tools/build.js";
import { readTool } from "../tools/read.js";
import { queryTool } from "../tools/query.js";
import { inspectV2 } from "../tools/inspect-v2.js";
import { exportV2 } from "../tools/export-v2.js";
import { shareV2 } from "../tools/share-v2.js";
import { render } from "../tools/render.js";
import { drawing } from "../tools/drawing.js";
import { parseHandle } from "../handles.js";

let engine: Engine;
beforeAll(async () => {
  engine = await Engine.init();
});

function envelope(result: { content: Array<{ type: string; [k: string]: unknown }> }): {
  ok: boolean;
  doc?: string;
  result?: unknown;
  stats?: {
    parts: number;
    nodes: number;
    volume_mm3?: number;
    triangles?: number;
    elapsed_ms: number;
  };
  error?: { code: string; message: string };
} {
  const text = result.content.find((c) => c.type === "text");
  if (!text) throw new Error("envelope missing text content");
  return JSON.parse((text as { text: string }).text);
}

describe("v2 end-to-end agent walkthrough", () => {
  it("runs the canonical plate-with-hole-and-fillet flow", () => {
    const trace: { step: string; doc: string; volume?: number; elapsed_ms: number }[] = [];

    // 1. Build the base plate.
    const r1 = buildTool(
      {
        ops: [
          {
            op: "primitive",
            kind: "cube",
            size: { x: 50, y: 30, z: 5 },
            name: "plate",
            material: "aluminum",
          },
        ],
        materials: {
          aluminum: { kind: "named", name: "aluminum" },
        },
      },
      engine,
    );
    const env1 = envelope(r1);
    expect(env1.ok).toBe(true);
    expect(env1.stats?.volume_mm3).toBeCloseTo(7500, 0);
    trace.push({ step: "build:plate", doc: env1.doc!, volume: env1.stats?.volume_mm3, elapsed_ms: env1.stats!.elapsed_ms });
    const handle1 = env1.doc!;
    expect(parseHandle(handle1).version).toBe(1);

    // 2. Drill a 6 mm hole through the plate using the hole op.
    const r2 = buildTool(
      {
        doc: handle1,
        ops: [
          {
            op: "hole",
            target: "plate",
            at: { x: 25, y: 15, z: 5 },
            kind: "through",
            diameter: 6,
            depth: "through",
            name: "drilled",
          },
        ],
      },
      engine,
    );
    const env2 = envelope(r2);
    expect(env2.ok).toBe(true);
    expect(env2.stats?.volume_mm3).toBeLessThan(7500);
    expect(env2.stats?.volume_mm3).toBeGreaterThan(7300);
    trace.push({ step: "build:hole", doc: env2.doc!, volume: env2.stats?.volume_mm3, elapsed_ms: env2.stats!.elapsed_ms });
    const handle2 = env2.doc!;
    expect(parseHandle(handle2).version).toBe(2);
    expect(parseHandle(handle2).docId).toBe(parseHandle(handle1).docId);

    // 3. Fillet the result.
    const r3 = buildTool(
      {
        doc: handle2,
        ops: [
          {
            op: "fillet",
            edges: [{ node: "drilled", edges_role: "all_top" }],
            radius: 0.5,
            name: "filleted",
          },
        ],
      },
      engine,
    );
    const env3 = envelope(r3);
    expect(env3.ok).toBe(true);
    trace.push({ step: "build:fillet", doc: env3.doc!, volume: env3.stats?.volume_mm3, elapsed_ms: env3.stats!.elapsed_ms });
    const handle3 = env3.doc!;
    expect(parseHandle(handle3).version).toBe(3);

    // 4. Inspect aggregate + per-part.
    const r4 = inspectV2({ doc: handle3 }, engine);
    const env4 = envelope(r4);
    expect(env4.ok).toBe(true);
    const ins = env4.result as {
      aggregate: { volume_mm3: number; surface_area_mm2: number; bbox: { size: { x: number; y: number; z: number } } };
      per_part: Record<string, unknown>;
      validity: { manifold: boolean };
    };
    expect(ins.aggregate.volume_mm3).toBeCloseTo(env3.stats!.volume_mm3!, 0);
    expect(ins.aggregate.bbox.size.x).toBeCloseTo(50, 0);
    expect(ins.aggregate.bbox.size.y).toBeCloseTo(30, 0);
    expect(Object.keys(ins.per_part)).toHaveLength(1);

    // 5. Query the feature tree.
    const r5 = queryTool({ doc: handle3, q: { kind: "tree" } }, engine);
    const tree = envelope(r5).result as Array<{ op: string; children: unknown[] }>;
    expect(tree).toHaveLength(1);
    expect(tree[0].op).toBe("Fillet"); // outermost wrapper
    // Walk down: Fillet → Difference (hole) → Cube
    let cur: { op: string; children: { op: string; children: unknown[] }[] } | undefined = tree[0] as never;
    const ops: string[] = [];
    while (cur) {
      ops.push(cur.op);
      cur = cur.children?.[0] as never;
    }
    expect(ops).toContain("Fillet");
    expect(ops).toContain("Difference");
    expect(ops).toContain("Cube");

    // 6. Render a preview PNG.
    const r6 = render({ doc: handle3, quality: "preview" }, engine);
    const env6 = envelope(r6);
    expect(env6.ok).toBe(true);
    const rend = env6.result as {
      image: { mime: string; data_base64: string };
      width: number;
      height: number;
      quality: string;
    };
    expect(rend.image.mime).toBe("image/png");
    expect(rend.image.data_base64.length).toBeGreaterThan(1000);

    // 7. Drawing — 4-up SVG.
    const r7 = drawing({ doc: handle3 }, engine);
    const env7 = envelope(r7);
    expect(env7.ok).toBe(true);
    const dwg = env7.result as { svg: string; views: string[] };
    expect(dwg.views).toEqual(["ortho:front", "ortho:top", "ortho:right", "iso"]);
    expect(dwg.svg).toContain("<svg");

    // 8. Export to STL (embedded base64).
    const r8 = exportV2({ doc: handle3, format: "stl" }, engine);
    const env8 = envelope(r8);
    expect(env8.ok).toBe(true);
    const exp = env8.result as { resource: { mime: string; data_base64: string }; bytes: number };
    expect(exp.resource.mime).toBe("model/stl");
    expect(exp.bytes).toBeGreaterThan(1000);

    // 9. Share — produce a vcad.io URL.
    const r9 = shareV2({ doc: handle3, name: "demo plate" }, engine);
    const env9 = envelope(r9);
    expect(env9.ok).toBe(true);
    const sh = env9.result as { url: string; encoded_bytes: number };
    expect(sh.url).toMatch(/vcad\.io.*doc=/);

    // 10. Read the IR back as JSON.
    const r10 = readTool({ doc: handle3, format: "json" }, engine);
    const env10 = envelope(r10);
    expect(env10.ok).toBe(true);
    const irPayload = env10.result as { ir: string; format: string };
    const ir = JSON.parse(irPayload.ir);
    expect(ir.version).toBe("0.1");
    expect(ir.roots).toHaveLength(1);
    expect(Object.keys(ir.nodes).length).toBeGreaterThanOrEqual(4);

    // ── Demo trace (visible when the test prints) ────────────────────
    console.log("\n[v2-e2e]");
    for (const t of trace) {
      console.log(
        `  ${t.step.padEnd(16)} → ${t.doc} (volume=${t.volume?.toFixed(1)}mm³, ${t.elapsed_ms}ms)`,
      );
    }
    console.log(
      `  inspect          → bbox=${ins.aggregate.bbox.size.x}×${ins.aggregate.bbox.size.y}×${ins.aggregate.bbox.size.z}, manifold=${ins.validity.manifold}`,
    );
    console.log(`  render           → ${rend.width}×${rend.height} ${rend.image.mime} (${(rend.image.data_base64.length / 1024).toFixed(1)} KB base64)`);
    console.log(`  drawing          → ${dwg.views.length}-view SVG (${(dwg.svg.length / 1024).toFixed(1)} KB)`);
    console.log(`  export(stl)      → ${exp.bytes} bytes`);
    console.log(`  share            → ${sh.url.slice(0, 90)}…`);
    console.log(`  read             → ${ir.roots.length} root(s), ${Object.keys(ir.nodes).length} node(s)`);
  });

  it("handles the assembly + simulate flow", async () => {
    const r = buildTool(
      {
        ops: [
          { op: "primitive", kind: "cube", size: { x: 100, y: 100, z: 50 }, name: "base" },
          { op: "primitive", kind: "cube", size: { x: 20, y: 20, z: 100 }, name: "link" },
        ],
      },
      engine,
    );
    const handle = envelope(r).doc!;
    expect(handle).toMatch(/^vcad:doc:/);

    const { assemble } = await import("../tools/assemble.js");
    const a = assemble(
      {
        doc: handle,
        instances: [
          { name: "base_inst", part: "base" },
          { name: "link_inst", part: "link" },
        ],
        joints: [
          {
            name: "j1",
            parent: "base_inst",
            child: "link_inst",
            kind: "revolute",
            anchor_parent: { x: 0, y: 0, z: 25 },
            anchor_child: { x: 0, y: 0, z: -50 },
            axis: { x: 0, y: 1, z: 0 },
            limits: { min: -90, max: 90 },
          },
        ],
        ground: "base_inst",
      },
      engine,
    );
    const aenv = envelope(a);
    expect(aenv.ok).toBe(true);
    const aRes = aenv.result as { added_instances: string[]; added_joints: string[] };
    expect(aRes.added_instances).toEqual(["base_inst", "link_inst"]);
    expect(aRes.added_joints).toEqual(["j1"]);

    console.log(`  assemble         → 2 instances, 1 revolute joint, ${aRes.added_instances.length} added`);
  });
});
