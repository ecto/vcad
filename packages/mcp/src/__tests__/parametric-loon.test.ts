import { describe, it, expect, beforeAll } from "vitest";
import { Engine, resolveDocument } from "@vcad/engine";
import type { Document } from "@vcad/ir";
import { createCadLoon } from "../tools/loon.js";
import { documents } from "../tools/session.js";
import { listParameters, setParameters } from "../tools/parameters.js";

/**
 * The parametric loon surface, end to end through the MCP tools: a value the
 * author named survives create_cad_loon as a document parameter, is visible to
 * list_parameters, and moves geometry via set_parameters — without the source
 * being re-sent.
 */

let engine: Engine;
beforeAll(async () => {
  engine = await Engine.init();
});

const LEG = `
[defparam pitch_axis_x 310.0]
[stack y "leg" 131.0
  [lane "femur_inner" 5.0]
  [gap  "idler_run"   1.0]
  [lane "idler_boss"  3.0]]
[root [translate pitch_axis_x [datum "leg_idler_boss_lo"] 0.0 [cube 10 2 4]] "steel"]
`;

/** Author into a session and return the stored document. */
function author(source: string, id: string): Document {
  const doc = engine.evalVcadSourceWithModules(source, {}) as Document;
  documents.set(id, doc);
  return doc;
}

/**
 * The Translate offset as the kernel sees it. Bindings are a sidecar resolved
 * at evaluation time (evaluate.ts does exactly this), so the stored document
 * keeps its literals and the resolved view is what geometry is built from.
 */
function translateOffset(doc: Document): { x: number; y: number; z: number } {
  const resolved = resolveDocument(doc).doc;
  const node = Object.values(resolved.nodes).find((n) => n.op.type === "Translate");
  return (node!.op as { offset: { x: number; y: number; z: number } }).offset;
}

describe("parametric loon through MCP", () => {
  it("reports the parameters, derived values and datums a program declared", () => {
    const res = createCadLoon({ source: LEG, format: "vcode" }, engine);
    const note = res.content.map((c) => c.text).join("\n");
    expect(note).toContain("settable");
    expect(note).toContain("pitch_axis_x");
    expect(note).toContain("leg_idler_run");
    // Boundaries are derived, not knobs.
    expect(note).toContain("leg_idler_boss_lo");
    expect(note).toContain("Datums");
  });

  it("says nothing extra for a program that declares no parameters", () => {
    const res = createCadLoon(
      { source: '[root [cube 10 20 30] "steel"]', format: "vcode" },
      engine,
    );
    expect(res.content).toHaveLength(1);
  });

  it("list_parameters sees a loon-authored parameter", async () => {
    author(LEG, "loon-list");
    const out = await listParameters({ document_id: "loon-list" }, engine);
    const json = JSON.parse(out.content[0].text as string);
    const names = json.parameters.map((p: { name: string }) => p.name);
    expect(names).toContain("pitch_axis_x");
    expect(names).toContain("leg_origin");
  });

  it("set_parameters moves geometry authored in loon", async () => {
    const doc = author(LEG, "loon-set");
    expect(translateOffset(doc).x).toBe(310);
    expect(translateOffset(doc).y).toBe(137);

    await setParameters(
      { document_id: "loon-set", parameters: { pitch_axis_x: 315 } },
      engine,
    );
    const after = documents.get("loon-set") as Document;
    expect(translateOffset(after).x).toBe(315);
    // Nothing else moved.
    expect(translateOffset(after).y).toBe(137);
  });

  it("opening a named clearance slides everything outboard of it", async () => {
    author(LEG, "loon-gap");
    await setParameters(
      { document_id: "loon-gap", parameters: { leg_idler_run: 2 } },
      engine,
    );
    const after = documents.get("loon-gap") as Document;
    expect(translateOffset(after).y).toBe(138);
  });

  it("reports that setting a derived parameter has no effect, and what to set instead", async () => {
    // A stack boundary follows from the thicknesses and gaps; geometry is
    // bound to those, so writing the boundary would silently do nothing.
    author(LEG, "loon-derived");
    const out = await setParameters(
      { document_id: "loon-derived", parameters: { leg_idler_boss_lo: 200 } },
      engine,
    );
    const json = JSON.parse(out.content[0].text as string);
    expect(json.no_effect).toHaveLength(1);
    expect(json.no_effect[0].name).toBe("leg_idler_boss_lo");
    expect(json.no_effect[0].set_instead).toContain("leg_idler_run");
    // And it really is inert — the geometry did not move.
    const after = documents.get("loon-derived") as Document;
    expect(translateOffset(after).y).toBe(137);
  });

  it("says nothing about parameters that do drive geometry", async () => {
    author(LEG, "loon-effective");
    const out = await setParameters(
      { document_id: "loon-effective", parameters: { pitch_axis_x: 315 } },
      engine,
    );
    const json = JSON.parse(out.content[0].text as string);
    expect(json.no_effect).toBeUndefined();
  });
});
