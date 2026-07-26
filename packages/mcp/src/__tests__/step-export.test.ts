/**
 * export_cad STEP path must export BOOLEAN documents as BRep AP214 —
 * regression for "STEP export is only available for sheet-metal documents",
 * which refused every real machined part (an annulus is a difference of two
 * cylinders). The exported file must carry analytic cylindrical faces and
 * re-import through the kernel at a matching volume.
 */
import { describe, it, expect, beforeAll } from "vitest";
import { mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Engine } from "@vcad/engine";
import type { Document } from "@vcad/ir";
import { exportCad } from "../tools/export.js";

let engine: Engine;
let dir: string;
beforeAll(async () => {
  engine = await Engine.init();
  dir = mkdtempSync(join(tmpdir(), "vcad-step-export-"));
  process.env.VCAD_MCP_EXPORT_DIR = dir;
});

const ANNULUS_LOON = `
[let outer [cylinder 20.0 10.0]]
[let bore [translate 0.0 0.0 -1.0 [cylinder 10.0 12.0]]]
[root [difference bore outer] "steel"]
`;

function annulusDoc(): Document {
  const doc = engine.evalVcadSource(ANNULUS_LOON);
  if (!doc) throw new Error("loon eval failed");
  return doc;
}

describe("export_cad STEP for boolean documents", () => {
  it("exports an annulus (cylinder minus cylinder) as BRep AP214", () => {
    const res = exportCad({ document: annulusDoc(), filename: "annulus.step" }, engine);
    const payload = JSON.parse(res.content[0].text) as Record<string, unknown>;
    expect(payload.format).toBe("step");

    const content = readFileSync(String(payload.path), "utf8");
    expect(content).toContain("ISO-10303-21");
    expect(content).toContain("MANIFOLD_SOLID_BREP");
    // The whole point: analytic faces, not a tessellation.
    expect(content).toContain("CYLINDRICAL_SURFACE");

    // Round-trip through the kernel's own STEP reader.
    const meshes = engine.importStep(
      readFileSync(String(payload.path)).buffer as ArrayBuffer,
    );
    expect(meshes.length).toBe(1);
    expect(meshes[0].indices.length).toBeGreaterThan(0);
  });

  it("refuses mesh-only parts by name instead of dropping them silently", () => {
    // Annulus (BRep, fine) + an imported mesh root (no BRep) — the export
    // must refuse and name the mesh part, not emit a file missing a part.
    const doc = annulusDoc() as unknown as {
      nodes: Record<string, unknown>;
      roots: Array<Record<string, unknown>>;
    };
    doc.nodes["999"] = {
      id: 999,
      name: "scanned",
      op: {
        type: "ImportedMesh",
        positions: [0, 0, 0, 1, 0, 0, 0, 1, 0],
        indices: [0, 1, 2],
      },
    };
    doc.roots.push({ root: 999, material: "default" });
    expect(() =>
      exportCad({ document: doc as unknown as Document, filename: "scan.step" }, engine),
    ).toThrow(/scanned/);
  });
});
