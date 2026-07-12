/**
 * Assembly documents (partDefs + instances, no scene roots) must be visible
 * through the see/measure surface: GLB preview and inspect_cad. Regression
 * for the "no geometry to preview" / "no parts to inspect" family — the
 * evaluator returns assembly geometry as `scene.instances`, which these
 * paths previously ignored.
 */
import { describe, it, expect, beforeAll } from "vitest";
import { Engine } from "@vcad/engine";
import type { Document } from "@vcad/ir";
import { previewGlbFor } from "../tools/preview.js";
import { computeInspection } from "../tools/inspect.js";

let engine: Engine;
beforeAll(async () => {
  engine = await Engine.init();
});

const ASSEMBLY_LOON = `
[let base [cube 40 40 5]]
[let post [cylinder 5 30]]
[assembly
  #[[part "base" base "aluminum"]
    [part "post" post "steel"]]
  #[[instance "base1" "base" 0 0 0]
    [instance "post1" "post" 20 20 5]]
  #[]
  "base1"]
`;

function assemblyDoc(): Document {
  const doc = engine.evalVcadSource(ASSEMBLY_LOON);
  if (!doc) throw new Error("loon eval failed");
  return doc;
}

describe("assembly visibility", () => {
  it("previewGlbFor renders instances of an assembly-only document", async () => {
    const doc = assemblyDoc();
    expect(doc.roots?.length ?? 0).toBe(0);
    expect(doc.instances?.length).toBe(2);
    const preview = await previewGlbFor(doc, engine);
    expect(preview).not.toBeNull();
    expect(preview!.glb.length).toBeGreaterThan(0);
  });

  it("computeInspection aggregates instance geometry", () => {
    const doc = assemblyDoc();
    const result = computeInspection(doc, engine);
    expect(result.parts).toBe(2);
    // base 40*40*5 + post ~ pi*25*30 (world-placed, tessellated)
    expect(result.volume_mm3).toBeGreaterThan(8000);
    // The post instance is translated to z=5..35 — world bbox must include it.
    expect(result.bounding_box.max.z).toBeGreaterThan(30);
    expect(result.bounding_box.max.x).toBeGreaterThanOrEqual(25);
  });
});
