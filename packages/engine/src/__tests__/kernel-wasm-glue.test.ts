import { describe, it, expect, beforeAll } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { getKernelWasm } from "../wasm-singleton.js";

/**
 * Guards against drift between the committed wasm-bindgen glue
 * (packages/kernel-wasm/vcad_kernel_wasm.js) and the kernel it wraps.
 *
 * The committed artifact once drifted from the Rust bindings —
 * `SliceResult.filamentGrams` called `wasm.circuitsim_dt` (a timestep!) and
 * `isEcadAvailable` called `wasm.isCamAvailable` — and nothing failed until
 * a human noticed a wrong number. Two layers of defense here:
 *
 * 1. A static scan of the glue — the authoritative layer. wasm-bindgen
 *    names class-method exports `<classname>_<method>` and free-function
 *    exports after the function itself, so a method body calling into a
 *    *different* class's export (or a free function calling a
 *    differently-named export) is exactly the historical drift shape.
 *    Verified to fire on the historical bug re-injected into the glue.
 * 2. A runtime smoke test that slices a real cube and cross-checks each
 *    `SliceResult` getter against `statsJson()`. This catches most
 *    mis-wirings (wrong offset/type ⇒ garbage) but NOT all: re-injecting
 *    the historical bug showed `circuitsim_dt` reads an f64 at the same
 *    struct offset as `filament_grams`, returning a bit-identical value.
 *    Layout coincidences are why the static scan exists.
 */

const GLUE_PATH = fileURLToPath(
  new URL("../../../kernel-wasm/vcad_kernel_wasm.js", import.meta.url),
);

/* eslint-disable @typescript-eslint/no-explicit-any */
type KernelWasm = any;

let wasm: KernelWasm;

beforeAll(async () => {
  wasm = (await getKernelWasm()) as KernelWasm;
});

/** A closed 10 mm cube: 8 vertices, 12 CCW-outward triangles. */
function cubeMesh(size = 10): { vertices: Float32Array; indices: Uint32Array } {
  const s = size;
  // prettier-ignore
  const vertices = new Float32Array([
    0, 0, 0,  s, 0, 0,  s, s, 0,  0, s, 0, // bottom ring (z = 0)
    0, 0, s,  s, 0, s,  s, s, s,  0, s, s, // top ring (z = s)
  ]);
  // prettier-ignore
  const indices = new Uint32Array([
    0, 2, 1,  0, 3, 2, // bottom (normal -z)
    4, 5, 6,  4, 6, 7, // top (normal +z)
    0, 1, 5,  0, 5, 4, // front (normal -y)
    2, 3, 7,  2, 7, 6, // back (normal +y)
    1, 2, 6,  1, 6, 5, // right (normal +x)
    3, 0, 4,  3, 4, 7, // left (normal -x)
  ]);
  return { vertices, indices };
}

describe("SliceResult bindings (runtime smoke)", () => {
  it("slices a cube and every getter agrees with statsJson", () => {
    const { vertices, indices } = cubeMesh();
    const settings = new wasm.SlicerSettings();
    const result = wasm.sliceMesh(vertices, indices, settings);

    // Plausibility: a 10 mm cube at 0.2 mm layers has dozens of layers and
    // uses a real amount of filament.
    expect(result.layerCount).toBeGreaterThan(10);
    expect(result.filamentMm).toBeGreaterThan(0);
    expect(result.filamentGrams).toBeGreaterThan(0);
    expect(result.printTimeSeconds).toBeGreaterThan(0);

    // Cross-check: each getter is one wasm export; statsJson() is a second,
    // independent export serializing the same stats struct. A getter wired
    // to the wrong export disagrees with the JSON unless the struct layouts
    // coincide (see module doc — the static scan covers that case).
    const stats = JSON.parse(result.statsJson());
    expect(result.layerCount).toBe(stats.layer_count);
    expect(result.filamentMm).toBeCloseTo(stats.filament_mm, 9);
    expect(result.filamentGrams).toBeCloseTo(stats.filament_grams, 9);
    expect(result.printTimeSeconds).toBeCloseTo(stats.print_time_seconds, 9);

    result.free();
    settings.free();
  });

  it("feature-detection exports exist and return booleans", () => {
    // isEcadAvailable once silently delegated to isCamAvailable; the static
    // scan below is what distinguishes the wiring — here we pin the runtime
    // contract (present, boolean) for every *Available export.
    const detectors = Object.keys(wasm).filter(
      (k) => /^is[A-Z]\w*Available$/.test(k) && typeof wasm[k] === "function",
    );
    expect(detectors).toContain("isEcadAvailable");
    expect(detectors).toContain("isCamAvailable");
    for (const name of detectors) {
      expect(typeof wasm[name](), `${name}()`).toBe("boolean");
    }
  });
});

describe("kernel-wasm glue drift (static scan)", () => {
  /**
   * Known-legit cross-class delegations: Rust free/static functions that
   * wasm-bindgen attaches under another class (e.g. `impl Raytracer`'s
   * static `can_raytrace(&Solid)` surfacing as a `Solid` method).
   */
  const ALLOWED_CROSS_CLASS = new Set(["Solid -> raytracer_canRaytrace"]);

  const glue = readFileSync(GLUE_PATH, "utf8");
  const lines = glue.split("\n");
  const classNames = new Set(
    [...glue.matchAll(/^export class (\w+)/gm)].map((m) => m[1].toLowerCase()),
  );

  it("class methods only call their own class's wasm exports", () => {
    const violations: string[] = [];
    let cls: string | null = null;
    for (const line of lines) {
      const m = line.match(/^export class (\w+)/);
      if (m) {
        cls = m[1];
        continue;
      }
      if (cls && /^}/.test(line)) {
        cls = null;
        continue;
      }
      if (!cls) continue;
      for (const call of line.matchAll(/wasm\.([A-Za-z0-9_]+)\(/g)) {
        const name = call[1];
        if (name.startsWith("__")) continue; // wbindgen helpers
        const prefix = name.match(/^([a-z0-9]+)_/)?.[1];
        // Only class-method-shaped exports whose prefix is a known class
        // can drift; other snake_case names are free functions.
        if (!prefix || !classNames.has(prefix)) continue;
        const edge = `${cls} -> ${name}`;
        if (prefix !== cls.toLowerCase() && !ALLOWED_CROSS_CLASS.has(edge)) {
          violations.push(edge);
        }
      }
    }
    expect(violations, "cross-class wasm calls (glue drift)").toEqual([]);
  });

  it("free functions call the same-named wasm export", () => {
    const violations: string[] = [];
    let fnName: string | null = null;
    let depth = 0;
    let calls: string[] = [];
    for (const line of lines) {
      if (!fnName) {
        const m = line.match(/^export function (\w+)\s*\(/);
        if (m) {
          fnName = m[1];
          depth = 0;
          calls = [];
        }
      }
      if (!fnName) continue;
      for (const call of line.matchAll(/wasm\.([A-Za-z0-9_]+)\(/g)) {
        if (!call[1].startsWith("__")) calls.push(call[1]);
      }
      depth += (line.match(/{/g) ?? []).length - (line.match(/}/g) ?? []).length;
      if (depth <= 0 && /}/.test(line)) {
        // Only single-call pass-through wrappers are unambiguous; anything
        // fancier (zero or multiple non-helper calls) is out of scope.
        const distinct = [...new Set(calls)];
        if (distinct.length === 1 && distinct[0] !== fnName) {
          violations.push(`${fnName} -> ${distinct[0]}`);
        }
        fnName = null;
      }
    }
    expect(violations, "mis-wired free-function exports (glue drift)").toEqual([]);
  });

  it("the scan actually covers the surface it claims to", () => {
    // If wasm-bindgen's glue format changes shape, the regexes above could
    // silently match nothing and the drift tests would pass vacuously.
    // Pin non-trivial coverage so a format change fails loudly instead.
    expect(classNames.size).toBeGreaterThan(5);
    expect(classNames.has("sliceresult")).toBe(true);
    const freeFns = [...glue.matchAll(/^export function (\w+)\s*\(/gm)];
    expect(freeFns.length).toBeGreaterThan(50);
  });
});
