/**
 * Re-export of the kernel-wasm singleton loader.
 *
 * The actual implementation lives in `@vcad/engine/wasm-singleton` because
 * `Engine.init` needs to call it and engine can't depend on core (core
 * already depends on engine for the `Engine` re-export). Consumers import
 * through `@vcad/core` for consistency with the rest of the shared API.
 */

export {
  getKernelWasm,
  getKernelWasmSync,
  primeKernelWasm,
} from "@vcad/engine";
