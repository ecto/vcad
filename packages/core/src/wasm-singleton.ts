/**
 * Singleton WASM module loader.
 *
 * Prevents double-instantiation of the kernel WASM module which corrupts
 * the CRDT engine's memory pointer. All consumers should use this instead
 * of `import("@vcad/kernel-wasm")` directly.
 */

type WasmModule = typeof import("@vcad/kernel-wasm");

let wasmPromise: Promise<WasmModule> | null = null;
let wasmModule: WasmModule | null = null;

/**
 * Get the kernel WASM module, loading it exactly once.
 * Safe to call from multiple components — all callers share the same instance.
 */
export async function getKernelWasm(): Promise<WasmModule> {
  if (wasmModule) return wasmModule;
  if (!wasmPromise) {
    wasmPromise = import("@vcad/kernel-wasm").then((mod) => {
      wasmModule = mod;
      return mod;
    });
  }
  return wasmPromise;
}

/**
 * Synchronous accessor for the already-loaded kernel WASM module.
 *
 * Returns `null` if the module hasn't been loaded yet — callers must handle
 * that case (or ensure they've `await getKernelWasm()` first). Used by code
 * paths that need to invoke WASM from a synchronous Zustand action.
 */
export function getKernelWasmSync(): WasmModule | null {
  return wasmModule;
}
