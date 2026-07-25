/**
 * Convert a vcad Document back to loon source code.
 *
 * Thin wrapper over the kernel's `documentToLoon` / `documentToLoonChecked`
 * WASM bindings — the Rust serializer (crates/vcad-loon) is the single
 * source of truth. The former ~400-line TypeScript mirror was deleted
 * (2026-07-24): every supported deployment ships a kernel-wasm bundle that
 * includes these bindings (checked-in artifacts refreshed by
 * wasm-refresh.yml; the MCP npm tarball bundles server + kernel together),
 * and the mirror's local `loadWasm` was never invoked, so the TS path was
 * silently serving every caller with a drifting second implementation.
 *
 * Callers must initialize the kernel first (`await getKernelWasm()` — the
 * app does this at bootstrap). These functions throw a clear error instead
 * of approximating when the kernel isn't ready.
 */

import type { Document } from "@vcad/ir";
import { getKernelWasmSync } from "../wasm-singleton.js";

function requireLoonBindings() {
  const wasm = getKernelWasmSync();
  if (!wasm) {
    throw new Error(
      "documentToLoon requires the kernel WASM module — call getKernelWasm() during app init before converting documents to loon",
    );
  }
  if (
    typeof wasm.documentToLoon !== "function" ||
    typeof wasm.documentToLoonChecked !== "function"
  ) {
    throw new Error(
      "kernel WASM bundle is missing the documentToLoon bindings — rebuild @vcad/kernel-wasm (stale bundle)",
    );
  }
  return wasm;
}

/** Convert a Document to loon source code. */
export function documentToLoon(doc: Document): string {
  return requireLoonBindings().documentToLoon(JSON.stringify(doc)) as string;
}

/**
 * Convert a Document to loon, also returning names of unsupported variants.
 *
 * When `unsupported` is non-empty, those nodes were replaced with comment
 * placeholders. Callers should surface a warning so the user knows data will
 * be lost if they save the loon output.
 */
export function documentToLoonChecked(doc: Document): {
  source: string;
  unsupported: string[];
} {
  return requireLoonBindings().documentToLoonChecked(JSON.stringify(doc)) as {
    source: string;
    unsupported: string[];
  };
}
