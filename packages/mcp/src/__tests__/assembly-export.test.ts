/**
 * export_cad must export ASSEMBLY documents (partDefs + instances, no scene
 * roots) — regression for "Document has no parts to export", which fired on
 * every assembly because the exporter walked `doc.roots` only.
 *
 * STL bakes instance transforms into world space; GLB keeps geometry
 * part-local and carries the world pose on one named node per instance.
 */
import { describe, it, expect, beforeAll } from "vitest";
import { mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Engine } from "@vcad/engine";
import type { Document } from "@vcad/ir";
import { exportCad } from "../tools/export.js";
import { computeInspection } from "../tools/inspect.js";
import { measureDocument } from "../fabricate/geometry.js";

let engine: Engine;
let dir: string;
beforeAll(async () => {
  engine = await Engine.init();
  dir = mkdtempSync(join(tmpdir(), "vcad-asm-export-"));
  process.env.VCAD_MCP_EXPORT_DIR = dir;
});

/** Two parts, one revolute joint — the smallest real assembly. */
const ASSEMBLY_LOON = `
[assembly
  #[[part "base" [cylinder 40.0 30.0] "steel"]
    [part "arm" [cube 80.0 20.0 20.0] "aluminum"]]
  #[[instance "base-inst" "base" 0.0 0.0 0.0]
    [instance "arm-inst" "arm" 0.0 0.0 30.0]]
  #[[revolute-joint "shoulder" 0.0 1.0 0.0 -90.0 90.0
      "base-inst" 0.0 0.0 25.0
      "arm-inst" 0.0 0.0 0.0]]
  "base-inst"]
`;

function assemblyDoc(): Document {
  const doc = engine.evalVcadSource(ASSEMBLY_LOON);
  if (!doc) throw new Error("loon eval failed");
  return doc;
}

function runExport(doc: Document, filename: string): Record<string, unknown> {
  const res = exportCad({ document: doc, filename }, engine);
  return JSON.parse(res.content[0].text) as Record<string, unknown>;
}

/** Bounding box of a binary STL's triangle soup. */
function stlBbox(bytes: Uint8Array): {
  min: [number, number, number];
  max: [number, number, number];
  triangles: number;
} {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const triangles = view.getUint32(80, true);
  const min: [number, number, number] = [Infinity, Infinity, Infinity];
  const max: [number, number, number] = [-Infinity, -Infinity, -Infinity];
  for (let t = 0; t < triangles; t++) {
    const base = 84 + t * 50 + 12; // skip the per-facet normal
    for (let v = 0; v < 3; v++) {
      for (let c = 0; c < 3; c++) {
        const x = view.getFloat32(base + v * 12 + c * 4, true);
        min[c] = Math.min(min[c], x);
        max[c] = Math.max(max[c], x);
      }
    }
  }
  return { min, max, triangles };
}

/** glTF JSON chunk of a GLB. */
function glbJson(bytes: Uint8Array): Record<string, any> {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  expect(new TextDecoder().decode(bytes.slice(0, 4))).toBe("glTF");
  const jsonLen = view.getUint32(12, true);
  return JSON.parse(new TextDecoder().decode(bytes.slice(20, 20 + jsonLen)));
}

describe("export_cad on assembly documents", () => {
  it("exports STL with instance transforms baked, matching inspect_cad's bbox", () => {
    const doc = assemblyDoc();
    expect(doc.roots?.length ?? 0).toBe(0);
    expect(doc.instances?.length).toBe(2);

    const payload = runExport(doc, "asm.stl");
    expect(payload.parts).toBe(2);
    expect(payload.instances).toBe(2);

    const bytes = readFileSync(String(payload.path));
    expect(bytes.length).toBeGreaterThan(84);
    const stl = stlBbox(new Uint8Array(bytes));
    expect(stl.triangles).toBeGreaterThan(0);

    // World-space bbox must agree with what inspect_cad reports for the same
    // document — i.e. the arm instance's z=30 offset is really baked in.
    const inspected = computeInspection(doc, engine).bounding_box;
    const tol = 1e-3;
    expect(stl.min[0]).toBeCloseTo(inspected.min.x, 2);
    expect(stl.min[1]).toBeCloseTo(inspected.min.y, 2);
    expect(stl.min[2]).toBeCloseTo(inspected.min.z, 2);
    expect(stl.max[0]).toBeCloseTo(inspected.max.x, 2);
    expect(stl.max[1]).toBeCloseTo(inspected.max.y, 2);
    expect(stl.max[2]).toBeCloseTo(inspected.max.z, 2);
    expect(stl.max[2]).toBeGreaterThan(30 - tol);
  });

  it("exports GLB with one named node per instance", () => {
    const doc = assemblyDoc();
    const payload = runExport(doc, "asm.glb");
    const bytes = new Uint8Array(readFileSync(String(payload.path)));
    expect(bytes.length).toBeGreaterThan(0);

    const json = glbJson(bytes);
    expect(json.nodes).toHaveLength(doc.instances!.length);
    const names = json.nodes.map((n: { name: string }) => n.name);
    expect(names).toContain("base-inst:base-inst");
    expect(names.some((n: string) => n.startsWith("arm-inst:"))).toBe(true);

    // Structure preserved, not flattened: the arm's world pose rides on the
    // node TRS rather than being welded into the vertices.
    const arm = json.nodes.find((n: { name: string }) =>
      n.name.startsWith("arm-inst:"),
    );
    // FK-solved: the joint anchor, not the authored instance origin.
    const fk = engine
      .evaluate(doc)
      .instances!.find((i) => i.instanceId === "arm-inst")!;
    expect(arm.translation?.[2]).toBeCloseTo(fk.transform!.translation.z, 3);
    expect(fk.transform!.translation.z).toBeGreaterThan(0);
  });

  it("quote_manufacturing's geometry measurement sees assemblies", () => {
    const metrics = measureDocument(assemblyDoc(), engine);
    expect(metrics.ok).toBe(true);
    expect(metrics.parts).toBe(2);
    expect(metrics.volume_mm3).toBeGreaterThan(0);
    expect(metrics.bbox!.max[2]).toBeGreaterThan(30);
  });

  it("names the real reason when there is genuinely nothing to export", () => {
    const empty = {
      version: "1",
      nodes: {},
      roots: [],
      materials: {},
      part_materials: {},
    } as unknown as Document;
    expect(() => runExport(empty, "empty.stl")).toThrow(/no scene roots/);
  });
});
