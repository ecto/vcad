// Smoke: the real rose-pro document (108 roots, plate vocabulary in a
// sibling module) resolves `[use plates [...]]` the way `load_document`
// does — base_dir read in TS, evaluated in the WASM kernel.
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { Engine } from "@vcad/engine";
import { composeLoonModules } from "../tools/loon.js";

describe("rose-pro modules via load_document path", () => {
  it("imports plates.loon beside the file", async () => {
    const dir = resolve(__dirname, "../../../../hardware/rose-pro");
    const raw = readFileSync(resolve(dir, "rose-pro.loon"), "utf8");
    const modules = composeLoonModules({ source: raw, base_dir: dir });
    expect(Object.keys(modules)).toEqual(["plates"]);
    const engine = await Engine.init();
    const doc = engine.evalVcadSourceWithModules(raw, modules);
    expect(doc!.roots.length).toBe(108);
  });
});
