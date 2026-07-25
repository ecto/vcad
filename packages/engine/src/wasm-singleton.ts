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

/** wasm-bindgen glue grows a `__vcad_reset_wasm` export (appended by the
 *  kernel-wasm build) that drops the cached instance so the next init
 *  re-instantiates. Optional so an older glue build degrades gracefully. */
type ResettableWasmModule = WasmModule & {
  __vcad_reset_wasm?: () => void;
  initSync?: (input: { module: BufferSource | WebAssembly.Module }) => unknown;
};

let wasmPromise: Promise<WasmModule> | null = null;
let wasmModule: WasmModule | null = null;
let wasmInputHint: BufferSource | Response | undefined;
let bindgenStarted = false;

/** The byte buffer the instance was built from, retained so a trap can
 *  re-instantiate without a disk read (serverless bundles relocate the
 *  source-relative .wasm path). Only set when we init from a buffer. */
let lastModuleBuffer: BufferSource | undefined;

/** Reason string of the most recent trap recovery, for diagnostics. The
 *  instance is healthy again after recovery — this is informational only,
 *  not a gate, so a recovered server keeps serving every session. */
let lastTrap: string | null = null;

/** Bumps on every successful (re)instantiation. Lets long-lived holders of
 *  the module detect that the underlying instance was swapped. */
let generation = 0;

/**
 * Recover the shared WASM instance after a trap.
 *
 * On wasm32 a Rust panic compiles to an `unreachable` trap: it does NOT
 * unwind, so the kernel's own `catch_unwind` never fires, destructors don't
 * run, and the instance is left in an undefined state — the shadow stack
 * pointer is decremented but never restored, and linear memory may be
 * mid-mutation. Reusing that instance corrupts every subsequent call, so a
 * single bad document used to poison the process and DoS every session.
 *
 * Instead of refusing all further calls, we drop the trapped instance and
 * re-instantiate a fresh one *in place*: the wasm-bindgen glue's exported
 * functions and classes (e.g. `Solid`, `render_svg`, and every reference
 * `Engine` captured at init) read the glue's module-level `wasm` binding at
 * call time, so swapping it under them transparently repairs all holders.
 * Re-init is eager and synchronous when we have the source buffer (the node
 * / MCP-server path) so a captured reference is never observed pointing at a
 * torn-down instance; otherwise it falls back to a lazy re-init on the next
 * `getKernelWasm()`.
 *
 * Callers that catch a `WebAssembly.RuntimeError` from a kernel call should
 * route it here.
 */
export function resetKernelWasm(reason: string): void {
  lastTrap = reason;
  const mod = wasmModule as ResettableWasmModule | null;

  if (!mod || typeof mod.__vcad_reset_wasm !== "function") {
    // Old glue without the reset hook, or never initialized. We can't drop
    // the cached instance in place; clear our refs so the next init at least
    // re-runs (it will no-op back to the same instance on old glue, but a
    // current build always has the hook).
    if (!mod) return;
    console.warn(
      "[wasm-singleton] kernel glue lacks __vcad_reset_wasm — cannot recover in place; rebuild @vcad/kernel-wasm",
    );
    wasmModule = null;
    wasmPromise = null;
    bindgenStarted = false;
    if (lastModuleBuffer) wasmInputHint = lastModuleBuffer;
    return;
  }

  // Drop the trapped instance: the glue sets its internal `wasm`/`wasmModule`
  // to undefined, so initSync/default re-instantiate rather than early-return.
  mod.__vcad_reset_wasm();

  // Eager, synchronous re-instantiation from the retained buffer. Keeps the
  // same glue module object, so nothing that captured `wasmModule` needs to
  // refetch — only the underlying instance changed.
  if (lastModuleBuffer && typeof mod.initSync === "function") {
    try {
      mod.initSync({ module: lastModuleBuffer });
      generation++;
      return;
    } catch (e) {
      console.error("[wasm-singleton] eager re-init after trap failed:", e);
    }
  }

  // No buffer to re-instantiate from synchronously (e.g. browser streamed a
  // Response). Fall back to a lazy re-init on the next getKernelWasm().
  wasmModule = null;
  wasmPromise = null;
  bindgenStarted = false;
  if (lastModuleBuffer) wasmInputHint = lastModuleBuffer;
}

/** The reason of the most recent trap recovery, or `null` if none. */
export function lastKernelTrapReason(): string | null {
  return lastTrap;
}

/** Monotonic counter of how many times the instance has been (re)created.
 *  `1` after first init; higher means a trap was recovered. */
export function kernelWasmGeneration(): number {
  return generation;
}

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
    // Retain the bytes so resetKernelWasm() can re-instantiate after a trap.
    lastModuleBuffer = wasmBuffer;
    bindgenStarted = true;
    mod.initSync({ module: wasmBuffer });
  } else {
    bindgenStarted = true;
    const hint = wasmInputHint;
    wasmInputHint = undefined;
    // Retain the buffer for trap recovery; a streamed Response can't be reused.
    if (hint && typeof Response !== "undefined" && !(hint instanceof Response)) {
      lastModuleBuffer = hint;
    }
    await mod.default(hint ? { module_or_path: hint } : undefined);
  }

  assertKernelBaseline(mod);
  wasmModule = mod;
  generation++;
  return mod;
}

/** Bindings every supported kernel-wasm bundle must export. The TS fallbacks
 *  that used to paper over their absence (semanticDiffFallback, the loon
 *  serializer mirror) were deleted 2026-07-24 — a bundle missing any of
 *  these is stale and fails init loudly instead of degrading silently. */
const REQUIRED_BINDINGS = [
  "evaluateDocument",
  "documentDiff",
  "documentMerge",
  "documentToLoon",
  "documentToLoonChecked",
] as const;

function assertKernelBaseline(mod: WasmModule): void {
  const missing = REQUIRED_BINDINGS.filter(
    (name) => typeof (mod as Record<string, unknown>)[name] !== "function",
  );
  if (missing.length > 0) {
    const version =
      typeof (mod as { get_kernel_version?: () => string }).get_kernel_version ===
      "function"
        ? (mod as { get_kernel_version: () => string }).get_kernel_version()
        : "unknown";
    throw new Error(
      `stale @vcad/kernel-wasm bundle (kernel version ${version}): missing required bindings ${missing.join(", ")} — rebuild the workspace (VCAD_WASM_SKIP unset) or update to a current bundle`,
    );
  }
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
