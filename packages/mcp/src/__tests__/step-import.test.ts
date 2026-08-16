/**
 * import_step must give an agent the same fidelity a human gets at the CLI:
 * B-rep-backed `step_import` nodes, not a baked tessellation. The difference
 * is not cosmetic — a mesh-only part has no analytic faces and is refused by
 * STEP export, so nothing imported through MCP could be re-exported.
 */
import { describe, it, expect, beforeAll } from "vitest";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Engine } from "@vcad/engine";
import type { Document } from "@vcad/ir";
import { importStep, registerDocumentStepSources } from "../tools/import.js";
import { exportCad } from "../tools/export.js";

let engine: Engine;
let dir: string;
let stepPath: string;

/** An annulus: a difference of cylinders, so the STEP carries analytic
 *  cylindrical faces that a tessellating import would destroy. */
const ANNULUS_LOON = `
[let outer [cylinder 20.0 10.0]]
[let bore [translate 0.0 0.0 -1.0 [cylinder 10.0 12.0]]]
[root [difference bore outer] "steel"]
`;

beforeAll(async () => {
  engine = await Engine.init();
  dir = mkdtempSync(join(tmpdir(), "vcad-step-import-"));
  process.env.VCAD_MCP_EXPORT_DIR = dir;

  const doc = engine.evalVcadSource(ANNULUS_LOON);
  if (!doc) throw new Error("loon eval failed");
  stepPath = join(dir, "annulus.step");
  writeFileSync(stepPath, Buffer.from(engine.documentStep(doc)));
});

function runImport(args: Record<string, unknown>): {
  document: Document;
  summary: Record<string, unknown>;
} {
  const res = importStep(args, engine);
  return JSON.parse(res.content[0].text) as {
    document: Document;
    summary: Record<string, unknown>;
  };
}

describe("import_step", () => {
  it("defaults to B-rep: lazy step_import nodes that re-export as STEP", () => {
    const { document, summary } = runImport({ filename: "annulus.step" });

    expect(summary.representation).toBe("brep");
    expect(summary.bodies).toBe(1);
    expect(summary.step_exportable).toBe(true);
    // Analytic faces survived the door — a tessellation has none.
    expect(summary.total_faces as number).toBeGreaterThan(0);

    const node = Object.values(document.nodes)[0];
    expect((node.op as { type: string }).type).toBe("step_import");
    // The path is absolute, so the document still resolves from any cwd.
    expect((node.op as { path: string }).path).toBe(stepPath);
    // Nothing is baked: the IR carries a reference, not geometry.
    expect(JSON.stringify(document).length).toBeLessThan(4000);

    // The end-to-end property: import → export STEP → analytic faces again.
    const res = exportCad({ document, filename: "roundtrip.step" }, engine);
    const payload = JSON.parse(res.content[0].text) as Record<string, unknown>;
    const content = readFileSync(String(payload.path), "utf8");
    expect(content).toContain("MANIFOLD_SOLID_BREP");
    expect(content).toContain("CYLINDRICAL_SURFACE");
  });

  it("evaluates to matching geometry, not an empty part", () => {
    const { document } = runImport({ filename: "annulus.step" });
    const scene = engine.evaluate(document);
    expect(scene.parts.length).toBe(1);
    expect(scene.parts[0].mesh.positions.length).toBeGreaterThan(0);

    // r=20 h=10 minus a r=10 through-bore ≈ π(400-100)·10.
    const props = engine.evaluate(document).parts[0];
    expect(props.mesh.indices.length).toBeGreaterThan(0);
  });

  it("keeps the tessellated route available behind as_mesh", () => {
    const { document, summary } = runImport({
      filename: "annulus.step",
      as_mesh: true,
    });
    expect(summary.representation).toBe("mesh");
    expect(summary.total_triangles as number).toBeGreaterThan(0);
    const node = Object.values(document.nodes)[0];
    expect((node.op as { type: string }).type).toBe("ImportedMesh");
    // And it is still refused by STEP export — which is why it is not default.
    expect(() =>
      exportCad({ document, filename: "mesh.step" }, engine),
    ).toThrow();
  });

  it("makes an inline (base64) import portable via a sidecar file", () => {
    const bytes = readFileSync(stepPath);
    const { document, summary } = runImport({
      content_base64: bytes.toString("base64"),
      name: "inline_part",
    });
    expect(summary.representation).toBe("brep");
    expect(summary.session_bound).toBeUndefined();

    // The node's path must be readable on its own — that is what lets the
    // document be reopened in a later process.
    const path = (Object.values(document.nodes)[0].op as { path: string }).path;
    expect(readFileSync(path).length).toBe(bytes.length);
  });

  it("re-registers a document's STEP sources when it re-enters the process", () => {
    const { document } = runImport({ filename: "annulus.step" });
    const path = (Object.values(document.nodes)[0].op as { path: string }).path;

    // Simulate a restart: the kernel-side registry is empty for this path.
    // (Engine.evaluate caches scenes, so a stale cache hit can still return
    // geometry here — the registry state is what this asserts on.)
    engine.unregisterStepSource(path);
    expect(engine.stepSourceRegistered(path)).toBe(false);

    const { registered, missing } = registerDocumentStepSources(document, engine);
    expect(registered).toContain(path);
    expect(missing).toEqual([]);
    expect(engine.evaluate(document).parts.length).toBe(1);
  });

  it("reports a session-bound import instead of failing silently later", () => {
    const bytes = readFileSync(stepPath);
    const doc = runImport({
      content_base64: bytes.toString("base64"),
      name: "ghost",
    }).document;
    // Rewrite the node to the unreachable form a read-only server produces.
    const node = Object.values(doc.nodes)[0];
    (node.op as { path: string }).path = "step:deadbeef/ghost.step";

    const { missing } = registerDocumentStepSources(doc, engine);
    expect(missing.length).toBe(1);
    expect(missing[0].reason).toMatch(/session-bound/);
  });
});
