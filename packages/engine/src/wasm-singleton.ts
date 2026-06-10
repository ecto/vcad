/**
 * Singleton WASM module loader.
 *
 * Owns the kernel-wasm lifecycle: runs `mod.default()` (browser) or
 * `mod.initSync()` (node) exactly once, then hands every caller the same
 * fully-initialized module. All consumers must use this instead of
 * `import("@vcad/kernel-wasm")` directly — a second `default()` call
 * instantiates a second WASM memory, and any handles (e.g. the CRDT
 * engine's pointer) baked against the first instance become invalid.
 *
 * Re-exported from `@vcad/core` so app code reaches it via the usual
 * `import { getKernelWasm } from "@vcad/core"`; lives here because
 * `Engine.init` needs to call it and engine can't depend on core
 * (core already depends on engine for its `Engine` re-export).
 */

type WasmModule = typeof import("@vcad/kernel-wasm");

let wasmPromise: Promise<WasmModule> | null = null;
let wasmModule: WasmModule | null = null;
let wasmInputHint: BufferSource | Response | undefined;
let bindgenStarted = false;

/**
 * Supply a pre-fetched WASM buffer (or `Response`) for the singleton to
 * pass into wasm-bindgen's `default()`. Lets callers that want real
 * byte-level download progress (see `packages/app/src/lib/bootstrap.ts`)
 * drive the fetch themselves. The prime window stays open until the
 * singleton is about to call `mod.default()` — so a caller that triggers
 * `getKernelWasm()` while the JS glue is still being dynamically imported
 * doesn't shut bootstrap out. Primes after `mod.default()` has fired are
 * ignored with a warning.
 */
export function primeKernelWasm(input: BufferSource | Response): void {
  if (bindgenStarted) {
    console.warn(
      "[wasm-singleton] primeKernelWasm called after init started — ignored",
    );
    return;
  }
  wasmInputHint = input;
}

/**
 * Get the kernel WASM module, loading and instantiating it exactly once.
 * Safe to call from multiple components — all callers share the same
 * instance.
 */
export async function getKernelWasm(): Promise<WasmModule> {
  if (wasmModule) return wasmModule;
  if (!wasmPromise) {
    wasmPromise = loadAndInit();
  }
  return wasmPromise;
}

async function loadAndInit(): Promise<WasmModule> {
  const mod = await import("@vcad/kernel-wasm");

  const isNode =
    typeof process !== "undefined" &&
    process.versions != null &&
    process.versions.node != null;

  if (isNode) {
    // A primed buffer takes precedence — bundlers (esbuild) relocate this
    // module, breaking the source-relative path below. Serverless entries
    // read the .wasm co-located with their bundle and prime it instead.
    const hint = wasmInputHint;
    wasmInputHint = undefined;
    const isBuffer = (v: BufferSource | Response): v is BufferSource =>
      typeof Response === "undefined" || !(v instanceof Response);
    let wasmBuffer: BufferSource;
    if (hint && isBuffer(hint)) {
      wasmBuffer = hint;
    } else {
      // Dynamic imports keep these out of browser bundles.
      const fs = await import("node:fs");
      const url = await import("node:url");
      const path = await import("node:path");

      const here = url.fileURLToPath(import.meta.url);
      const wasmPath = path.join(
        path.dirname(here),
        "..",
        "..",
        "kernel-wasm",
        "vcad_kernel_wasm_bg.wasm",
      );
      wasmBuffer = fs.readFileSync(wasmPath);
    }
    bindgenStarted = true;
    mod.initSync({ module: wasmBuffer });
  } else {
    bindgenStarted = true;
    const hint = wasmInputHint;
    wasmInputHint = undefined;
    await mod.default(hint ? { module_or_path: hint } : undefined);
  }

  wasmModule = mod;
  return mod;
}

/**
 * Synchronous accessor for the already-initialized kernel WASM module.
 *
 * Returns `null` if init hasn't completed yet — callers must handle
 * that case (or ensure they've `await getKernelWasm()` first). Used by
 * code paths that need to invoke WASM from a synchronous Zustand action.
 */
export function getKernelWasmSync(): WasmModule | null {
  return wasmModule;
}
