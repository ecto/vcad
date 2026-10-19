/**
 * create_cad_loon asset resolution: a multi-file loon project that imports an
 * STL loads over MCP — by value (`files`, base64) on any deployment, or from
 * `base_dir` when the server runs locally. Before this, the WASM kernel
 * evaluated `mesh_import` to nothing and a module the server couldn't supply
 * failed with "operation not supported on this platform".
 */

import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterAll, afterEach, beforeAll, describe, expect, it } from "vitest";
import { Engine } from "@vcad/engine";
import type { Document } from "@vcad/ir";
import { evalCadLoon, toolDefs } from "../tools/loon.js";

/** A one-triangle-per-face unit tetrahedron, ASCII STL, in metres. */
const TET_STL = `solid tet
facet normal 0 0 -1
 outer loop
  vertex 0 0 0
  vertex 0.01 0 0
  vertex 0 0.01 0
 endloop
endfacet
facet normal 0 -1 0
 outer loop
  vertex 0 0 0
  vertex 0 0 0.01
  vertex 0.01 0 0
 endloop
endfacet
facet normal -1 0 0
 outer loop
  vertex 0 0 0
  vertex 0 0.01 0
  vertex 0 0 0.01
 endloop
endfacet
facet normal 1 1 1
 outer loop
  vertex 0.01 0 0
  vertex 0 0 0.01
  vertex 0 0.01 0
 endloop
endfacet
endsolid tet
`;

const MINI = `
[pub let shell [import-mesh "meshes/tet.stl"]]
[pub let base [cube 20 20 2]]
`;

const MAIN = `
[use mini]
[root mini.base "steel"]
[root mini.shell "steel"]
`;

let engine: Engine;
let dir: string;

beforeAll(async () => {
  engine = await Engine.init();
  dir = mkdtempSync(join(tmpdir(), "vcad-loon-assets-"));
  writeFileSync(join(dir, "mini.loon"), MINI);
  mkdirSync(join(dir, "meshes"));
  writeFileSync(join(dir, "meshes", "tet.stl"), TET_STL);
});

afterAll(() => rmSync(dir, { recursive: true, force: true }));
afterEach(() => {
  delete process.env.VCAD_MCP_REMOTE;
});

function meshNodes(doc: Document) {
  return Object.values(doc.nodes).filter(
    (n) => (n.op as { type: string }).type === "ImportedMesh",
  );
}

function text(r: { content: Array<{ text: string }> }) {
  return r.content.map((c) => c.text).join("\n");
}

describe("create_cad_loon asset resolution", () => {
  it("loads a multi-file project + STL from base_dir on a local server", () => {
    const { result, doc } = evalCadLoon({ source: MAIN, base_dir: dir, format: "json" }, engine);
    expect(doc).not.toBeNull();
    const meshes = meshNodes(doc!);
    expect(meshes.length).toBe(1);
    const op = meshes[0].op as unknown as { positions: number[]; indices: number[] };
    expect(op.indices.length).toBe(12);
    // metres → mm, like the native evaluator
    expect(Math.max(...op.positions)).toBeCloseTo(10, 5);
    expect(text(result)).toMatch(/Imports resolved: 1 mesh/);
    // the inlined mesh is real geometry
    expect(engine.evaluate(doc!).parts.length).toBe(2);
  });

  it("loads the same project by value on a hosted server", () => {
    process.env.VCAD_MCP_REMOTE = "1";
    const { doc, result } = evalCadLoon(
      {
        source: MAIN,
        modules: { mini: MINI },
        files: { "./meshes/tet.stl": Buffer.from(TET_STL).toString("base64") },
      },
      engine,
    );
    expect(meshNodes(doc!).length).toBe(1);
    expect(text(result)).not.toMatch(/UNRESOLVED/);
  });

  it("reports an import it can't resolve instead of dropping it", () => {
    process.env.VCAD_MCP_REMOTE = "1";
    const { result } = evalCadLoon({ source: MAIN, modules: { mini: MINI } }, engine);
    expect(text(result)).toMatch(/UNRESOLVED[\s\S]*meshes\/tet\.stl[\s\S]*files/);
  });

  it("explains a module a hosted server can't read", () => {
    process.env.VCAD_MCP_REMOTE = "1";
    expect(() => evalCadLoon({ source: MAIN, base_dir: dir }, engine)).toThrow(
      /Module\(s\) not supplied: mini[\s\S]*hosted[\s\S]*`modules`/,
    );
  });

  it("advertises files in the input schema", () => {
    const def = toolDefs.find((t) => t.name === "create_cad_loon")!;
    const props = (def.inputSchema as { properties: Record<string, { description: string }> })
      .properties;
    expect(props.files.description).toMatch(/base64/);
    expect(props.base_dir.description).toMatch(/LOCAL/);
  });
});
