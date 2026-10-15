// Smoke: the real rose-full document (plate vocabulary in a
// sibling module) resolves `[use plates [...]]` the way `load_document`
// does — base_dir read in TS, evaluated in the WASM kernel.
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { Engine } from "@vcad/engine";
import { composeLoonModules } from "../tools/loon.js";

describe("rose-full modules via load_document path", () => {
  it("imports plates.loon beside the file", async () => {
    const dir = resolve(__dirname, "../../../../hardware/rose-full");
    const raw = readFileSync(resolve(dir, "rose-full.loon"), "utf8");
    const modules = composeLoonModules({ source: raw, base_dir: dir });
    expect(Object.keys(modules)).toEqual(["plates"]);
    const engine = await Engine.init();
    const doc = engine.evalVcadSourceWithModules(raw, modules);
    expect(doc!.roots.length).toBeGreaterThanOrEqual(108);
  });
});
