/**
 * create_cad_loon module resolution: loon's `[use ...]` works over the MCP
 * seam, so a multi-file CAD project no longer needs an external linker that
 * concatenates the sources. Modules arrive either by value (`modules`, and
 * inline `loons`) or read server-side from `base_dir` — and a module read
 * off disk must produce the same document as the same module passed by
 * value, which is the point of the abstraction.
 */

import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { Engine } from "@vcad/engine";
import { composeLoonModules, toolDefs } from "../tools/loon.js";
import type { ToolContext } from "../tools/tool-def.js";
import { getSession, registerSession } from "../tools/session-core.js";

const BRACKET = `
[pub let plate [cube 40 20 4]]
[pub let post [cylinder 3 12]]
[let scrap [sphere 1]]
`;

const MAIN = `
[use bracket]
[root bracket.plate "aluminum"]
[root [translate 20 10 4 bracket.post] "steel"]
`;

let engine: Engine;
let ctx: ToolContext;
let dir: string;
const createCadLoonDef = toolDefs.find((t) => t.name === "create_cad_loon")!;

beforeAll(async () => {
  engine = await Engine.init();
  ctx = { engine, user: null } as unknown as ToolContext;
  dir = mkdtempSync(join(tmpdir(), "vcad-loon-modules-"));
  writeFileSync(join(dir, "bracket.loon"), BRACKET);
});

afterAll(() => {
  rmSync(dir, { recursive: true, force: true });
});

describe("create_cad_loon module resolution", () => {
  it("resolves [use] against an in-memory modules map", () => {
    const doc = engine.evalVcadSourceWithModules(MAIN, { bracket: BRACKET });
    expect(doc).not.toBeNull();
    expect(doc!.roots.length).toBe(2);
  });

  it("agrees with the same module read from base_dir", () => {
    const fromMap = engine.evalVcadSourceWithModules(MAIN, { bracket: BRACKET });
    const fromDisk = engine.evalVcadSourceWithModules(
      MAIN,
      composeLoonModules({ source: MAIN, base_dir: dir }),
    );
    expect(fromDisk).toEqual(fromMap);
  });

  it("falls back to $VCAD_LOON_PATH for modules not beside the file", () => {
    const lib = join(dir, "lib");
    mkdirSync(lib, { recursive: true });
    writeFileSync(join(lib, "shared.loon"), "[pub let peg [cylinder 2 8]]");
    const src = '[use shared]\n[root shared.peg "steel"]';
    const prev = process.env.VCAD_LOON_PATH;
    process.env.VCAD_LOON_PATH = lib;
    try {
      // Not beside the file: served from the lib path.
      const modules = composeLoonModules({ source: src, base_dir: dir });
      expect(Object.keys(modules)).toEqual(["shared"]);
      expect(modules.shared).toContain("peg");
      // Beside the file: the file wins over the lib path.
      writeFileSync(join(dir, "shared.loon"), "[pub let peg [cube 1 1 1]]");
      const local = composeLoonModules({ source: src, base_dir: dir });
      expect(local.shared).toContain("cube");
    } finally {
      if (prev === undefined) delete process.env.VCAD_LOON_PATH;
      else process.env.VCAD_LOON_PATH = prev;
    }
  });

  it("follows nested imports when reading from base_dir", () => {
    mkdirSync(join(dir, "sub"), { recursive: true });
    writeFileSync(join(dir, "sub", "base.loon"), "[pub let slab [cube 10 10 2]]");
    writeFileSync(
      join(dir, "stack.loon"),
      '[use sub.base]\n[pub let tower [translate 0 0 2 sub.base.slab]]',
    );
    const src = '[use stack]\n[root stack.tower "steel"]';
    const modules = composeLoonModules({ source: src, base_dir: dir });
    expect(Object.keys(modules).sort()).toEqual(["stack", "sub.base"]);
    const doc = engine.evalVcadSourceWithModules(src, modules);
    expect(doc!.roots.length).toBe(1);
  });

  it("refuses to read outside base_dir", () => {
    const modules = composeLoonModules({
      source: "[use ..secrets]",
      base_dir: dir,
    });
    expect(modules).toEqual({});
  });

  it("makes inline `loons` importable by name", () => {
    const modules = composeLoonModules({
      source: "[use widget [widget]]",
      loons: [{ name: "widget", source: "[pub let widget [cube 3 3 3]]" }],
    });
    expect(modules.widget).toContain("cube");
  });

  it("authors a multi-module document into an open session", async () => {
    const empty = engine.evalVcadSource("#[]");
    const id = registerSession(empty!);
    await createCadLoonDef.handler(
      { document_id: id, source: MAIN, modules: { bracket: BRACKET } },
      ctx,
    );
    const doc = getSession(id);
    expect(doc.roots.length).toBe(2);
  });

  it("advertises modules and base_dir in the input schema", () => {
    const props = createCadLoonDef.inputSchema.properties as Record<string, unknown>;
    expect(props.modules).toBeDefined();
    expect(props.base_dir).toBeDefined();
  });
});
