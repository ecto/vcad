// Smoke: the real rose-full document resolves its modules the way
// `load_document` does (base_dir read in TS, evaluated in the WASM kernel):
// one module per link under links/, the shared actuators, and the plate
// vocabulary the links use.
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { Engine } from "@vcad/engine";
import { composeLoonModules } from "../tools/loon.js";

describe("rose-full modules via load_document path", () => {
  it("resolves every [use] module beside the file", async () => {
    const dir = resolve(__dirname, "../../../../hardware/rose-full");
    const raw = readFileSync(resolve(dir, "rose-full.loon"), "utf8");
    const modules = composeLoonModules({ source: raw, base_dir: dir });
    const keys = Object.keys(modules);
    const used = [...raw.matchAll(/^\[use ([A-Za-z0-9_.]+)/gm)].map((m) => m[1]);
    expect(used.length).toBeGreaterThan(0);
    for (const name of used) expect(keys).toContain(name);
    expect(keys).toContain("plates");
    const engine = await Engine.init();
    const doc = engine.evalVcadSourceWithModules(raw, modules);
    expect(doc!.roots.length).toBeGreaterThanOrEqual(108);
  });
});
