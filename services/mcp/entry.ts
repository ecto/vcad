/**
 * Vercel serverless function entry point for the vcad MCP server.
 *
 * Solves the WASM path problem: after esbuild bundles everything into a
 * single file, Engine.init()'s relative path to the .wasm file breaks
 * (it resolves to /kernel-wasm/... outside the function sandbox).
 *
 * We bypass Engine.init() entirely and construct the Engine directly:
 * 1. Read the .wasm file from next to this bundle
 * 2. Call initSync() to initialize the WASM module
 * 3. Construct Engine with the bindings from the initialized module
 */

import type { VercelRequest, VercelResponse } from "@vercel/node";
import { StreamableHTTPServerTransport } from "@modelcontextprotocol/sdk/server/streamableHttp.js";
import { createServer } from "@vcad/mcp/server";
import { Engine } from "@vcad/engine";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { readFileSync } from "node:fs";

// Locate WASM file next to this bundle
const __bundleDir = dirname(fileURLToPath(import.meta.url));
const WASM_PATH = join(__bundleDir, "vcad_kernel_wasm_bg.wasm");

// Module-scoped engine — survives warm invocations (Fluid Compute)
let _engine: Engine | undefined;

async function getEngine(): Promise<Engine> {
  if (_engine) return _engine;

  // Initialize WASM from the co-located .wasm file.
  // We bypass Engine.init() because it calculates the WASM path relative
  // to the original source location, which breaks after esbuild bundling.
  const wasmModule = await import("@vcad/kernel-wasm");
  const wasmBuffer = readFileSync(WASM_PATH);
  wasmModule.initSync({ module: wasmBuffer });

  // Extract the compiled module for potential worker sharing
  const getCompiledModule = (wasmModule as Record<string, unknown>)
    .getCompiledModule as (() => WebAssembly.Module | undefined) | undefined;
  const compiledWasmModule = getCompiledModule?.();

  // Construct Engine directly with WASM bindings — same as Engine.init()
  // does at packages/engine/src/index.ts:320-329
  _engine = new Engine(
    {
      Solid: wasmModule.Solid,
      WasmAnnotationLayer: wasmModule.WasmAnnotationLayer,
      projectMesh: wasmModule.projectMesh,
      importStepBuffer: wasmModule.importStepBuffer,
      exportProjectedViewToDxf: wasmModule.exportProjectedViewToDxf,
      createDetailView: wasmModule.createDetailView,
      evaluateDocument: (wasmModule as Record<string, unknown>)
        .evaluateDocument as Parameters<typeof Engine.prototype.evaluate>[1],
      evalVcadSource: (wasmModule as Record<string, unknown>)
        .evalVcadSource as Parameters<typeof Engine.prototype.evaluate>[1],
    } as ConstructorParameters<typeof Engine>[0],
    compiledWasmModule,
  );

  return _engine;
}

export default async function handler(
  req: VercelRequest,
  res: VercelResponse,
): Promise<void> {
  // CORS
  res.setHeader("Access-Control-Allow-Origin", "*");
  res.setHeader("Access-Control-Allow-Methods", "GET, POST, DELETE, OPTIONS");
  res.setHeader(
    "Access-Control-Allow-Headers",
    "Content-Type, mcp-session-id, Last-Event-ID, mcp-protocol-version",
  );
  res.setHeader(
    "Access-Control-Expose-Headers",
    "mcp-session-id, mcp-protocol-version",
  );

  if (req.method === "OPTIONS") {
    res.status(204).end();
    return;
  }

  // Health check
  if (req.method === "GET" && req.url === "/health") {
    try {
      const engine = await getEngine();
      res.status(200).json({
        status: "ok",
        engine: !!engine,
        timestamp: new Date().toISOString(),
      });
    } catch (err) {
      res.status(500).json({
        status: "error",
        error: err instanceof Error ? err.message : String(err),
      });
    }
    return;
  }

  try {
    const engine = await getEngine();
    const server = await createServer(engine);
    const transport = new StreamableHTTPServerTransport({
      sessionIdGenerator: undefined, // stateless
    });

    await server.connect(transport);

    try {
      await transport.handleRequest(req, res, req.body);
    } finally {
      await transport.close();
      await server.close();
    }
  } catch (err) {
    console.error("[vcad-mcp] Error:", err);
    if (!res.headersSent) {
      res.status(500).json({ error: "Internal server error" });
    }
  }
}
