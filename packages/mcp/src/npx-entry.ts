#!/usr/bin/env node
/**
 * npm/npx entry point for the published @vcad/mcp package.
 *
 * The npm tarball is a self-contained esbuild bundle, which relocates
 * wasm-singleton away from its source-relative `../kernel-wasm/…` path —
 * the same problem the Vercel serverless entry (services/mcp/entry.ts)
 * solves. Same fix: read the .wasm shipped next to the bundle and prime it
 * before anything can trigger kernel init, then hand off to the normal
 * stdio main in index.ts.
 */

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { primeKernelWasm } from "@vcad/engine";

primeKernelWasm(
  readFileSync(
    join(
      dirname(fileURLToPath(import.meta.url)),
      "vcad_kernel_wasm_bg.wasm",
    ),
  ),
);

await import("./index.js");
