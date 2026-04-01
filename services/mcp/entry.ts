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

import type { IncomingMessage, ServerResponse } from "node:http";
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

/** Send a JSON response using raw Node.js API (Build Output API
 *  serves raw ServerResponse, not Vercel's enhanced VercelResponse). */
function sendJson(res: ServerResponse, status: number, data: unknown): void {
  res.writeHead(status, { "Content-Type": "application/json" });
  res.end(JSON.stringify(data));
}

export default async function handler(
  req: IncomingMessage,
  res: ServerResponse,
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
    res.writeHead(204);
    res.end();
    return;
  }

  // Health check
  if (req.method === "GET" && req.url === "/health") {
    try {
      const engine = await getEngine();
      sendJson(res, 200, {
        status: "ok",
        engine: !!engine,
        timestamp: new Date().toISOString(),
      });
    } catch (err) {
      sendJson(res, 500, {
        status: "error",
        error: err instanceof Error ? err.message : String(err),
      });
    }
    return;
  }

  // MCP endpoint — parse body for POST, then delegate to transport
  try {
    const engine = await getEngine();
    const server = await createServer(engine);
    const transport = new StreamableHTTPServerTransport({
      sessionIdGenerator: undefined, // stateless
    });

    await server.connect(transport);

    try {
      if (req.method === "POST") {
        const body = await readBody(req);
        const parsed = JSON.parse(body);
        await transport.handleRequest(req, res, parsed);
      } else {
        await transport.handleRequest(req, res);
      }
    } finally {
      await transport.close();
      await server.close();
    }
  } catch (err) {
    console.error("[vcad-mcp] Error:", err);
    if (!res.headersSent) {
      sendJson(res, 500, { error: "Internal server error" });
    }
  }
}

/** Read request body as string. */
function readBody(req: IncomingMessage): Promise<string> {
  return new Promise((resolve, reject) => {
    const chunks: Buffer[] = [];
    req.on("data", (chunk: Buffer) => chunks.push(chunk));
    req.on("end", () => resolve(Buffer.concat(chunks).toString("utf-8")));
    req.on("error", reject);
  });
}
