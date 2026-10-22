/**
 * Web Worker for off-main-thread document evaluation.
 *
 * Prefers the Rust WASM evaluateDocument when available, otherwise falls back
 * to the TypeScript evaluator (which still uses WASM Solid primitives but runs
 * the evaluation logic in JS). Either way, the main thread stays unblocked.
 *
 * Messages:
 *   → {type: 'init'}                                     — load WASM module
 *   ← {type: 'ready'}                                    — WASM ready
 *   → {type: 'evaluate', id, docJson, skipClashDetection} — evaluate document
 *   ← {type: 'result', id, scene}                        — evaluation result (with transferables)
 *   ← {type: 'error', id, message}                       — evaluation error
 */

import { evaluateDocument as evaluateDocumentTS } from "./evaluate.js";
import type { EvaluatedScene } from "./mesh.js";
import type { Document } from "@vcad/ir";
import {
  collectTransferables,
  postProcessScene,
  wasmResultToScene,
  type WasmEvaluatedScene,
} from "./scene-convert.js";

type WasmEvaluateDocumentFn = (docJson: string, skipClashDetection: boolean) => WasmEvaluatedScene;
/** The kernel module for the TS evaluator fallback */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
let kernelModule: any = null;

/** Native WASM evaluateDocument (may not be available in older builds) */
let wasmEvaluateDocument: WasmEvaluateDocumentFn | null = null;

/**
 * Incremental evaluator (kernel `IncrementalEvaluator`): this worker serves
 * one open document, so one evaluator carries its solids between edits and
 * each evaluation redoes only what the edit touched. Subtree fingerprints
 * keep it exact even if a different document arrives (stale entries are
 * dropped, so memory stays bounded to the current document).
 */
let incremental: { evaluate(docJson: string, skip: boolean): WasmEvaluatedScene } | null = null;

/** Whether we're using the fast WASM path or the TS fallback */
let evaluatorMode: "wasm" | "ts" = "ts";

/** Run evaluation using whichever path is available. */
function evaluate(docJson: string, skipClashDetection: boolean): EvaluatedScene {
  // Fast path: native WASM evaluator (with fallback to TS on failure)
  if (evaluatorMode === "wasm" && wasmEvaluateDocument) {
    try {
      const result = incremental
        ? incremental.evaluate(docJson, skipClashDetection)
        : wasmEvaluateDocument(docJson, skipClashDetection);
      const scene = wasmResultToScene(result);
      const doc: Document = JSON.parse(docJson);
      return postProcessScene(scene, doc, kernelModule);
    } catch {
      // WASM evaluator failed — fall through to TS evaluator
    }
  }

  // Fallback: TS evaluator (still uses WASM Solid class for primitives/booleans)
  const doc: Document = JSON.parse(docJson);
  return evaluateDocumentTS(doc, kernelModule, { skipClashDetection });
}

self.onmessage = async (e: MessageEvent) => {
  const { type } = e.data;

  if (type === "init") {
    try {
      const wasm = await import("@vcad/kernel-wasm");

      // If the main thread passed a pre-compiled WebAssembly.Module,
      // use it to skip recompilation (~3s savings).
      const compiledModule: WebAssembly.Module | undefined = e.data.module;
      if (compiledModule) {
        await wasm.default({ module_or_path: compiledModule });
      } else {
        await wasm.default();
      }

      // Build kernel module for TS evaluator
      kernelModule = {
        Solid: wasm.Solid,
        evaluateDocument: (wasm as Record<string, unknown>).evaluateDocument,
        evaluateSheetMetalChain: (wasm as Record<string, unknown>)
          .evaluateSheetMetalChain,
        embroideryDesignToMesh: (wasm as Record<string, unknown>)
          .embroideryDesignToMesh,
        transformMeshBuffers: (wasm as Record<string, unknown>)
          .transformMeshBuffers,
      };

      // Check if native WASM evaluator is available
      wasmEvaluateDocument = (wasm as Record<string, unknown>).evaluateDocument as WasmEvaluateDocumentFn | null;
      evaluatorMode = wasmEvaluateDocument ? "wasm" : "ts";
      const Incremental = (wasm as Record<string, unknown>).IncrementalEvaluator as
        | (new () => { evaluate(docJson: string, skip: boolean): WasmEvaluatedScene })
        | undefined;
      incremental = Incremental ? new Incremental() : null;

      self.postMessage({ type: "ready" });
    } catch (err) {
      self.postMessage({ type: "error", id: null, message: `WASM init failed: ${err}` });
    }
    return;
  }

  if (type === "evaluate") {
    const { id, docJson, skipClashDetection } = e.data;
    if (!kernelModule) {
      self.postMessage({ type: "error", id, message: "Worker not initialized" });
      return;
    }
    try {
      const t0 = performance.now();
      const scene = evaluate(docJson, skipClashDetection ?? false);
      const workerTotalMs = performance.now() - t0;
      const transferables = collectTransferables(scene);
      (self as unknown as DedicatedWorkerGlobalScope).postMessage(
        { type: "result", id, scene, workerTotalMs },
        transferables,
      );
    } catch (err) {
      self.postMessage({ type: "error", id, message: String(err) });
    }
    return;
  }
};
