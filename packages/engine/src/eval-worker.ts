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

import { evaluateDocument as evaluateDocumentTS, type EvaluateOptions } from "./evaluate.js";
import type { EvaluatedScene, EvalTimingData, TriangleMesh } from "./mesh.js";
import type { Document } from "@vcad/ir";

/** WASM evaluator result shape (typed arrays from Rust, or plain arrays from legacy) */
interface WasmMesh {
  positions: Float32Array | number[];
  indices: Uint32Array | number[];
  normals?: Float32Array | number[];
}

interface WasmEvaluatedScene {
  parts: Array<{ mesh: WasmMesh; material: string }>;
  partDefs?: Array<{ id: string; mesh: WasmMesh }>;
  instances?: Array<{
    instance_id: string;
    part_def_id: string;
    name?: string;
    mesh: WasmMesh;
    material: string;
    transform?: unknown;
  }>;
  clashes: Array<WasmMesh>;
  timing?: EvalTimingData;
}

type WasmEvaluateDocumentFn = (docJson: string, skipClashDetection: boolean) => WasmEvaluatedScene;

/** The kernel module for the TS evaluator fallback */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
let kernelModule: any = null;

/** Native WASM evaluateDocument (may not be available in older builds) */
let wasmEvaluateDocument: WasmEvaluateDocumentFn | null = null;

/** Whether we're using the fast WASM path or the TS fallback */
let evaluatorMode: "wasm" | "ts" = "ts";

/** Collect ArrayBuffers from an EvaluatedScene for zero-copy transfer. */
function collectTransferables(scene: EvaluatedScene): ArrayBuffer[] {
  const buffers: ArrayBuffer[] = [];

  const collectMesh = (m: TriangleMesh) => {
    buffers.push(m.positions.buffer as ArrayBuffer);
    buffers.push(m.indices.buffer as ArrayBuffer);
    if (m.normals) buffers.push(m.normals.buffer as ArrayBuffer);
  };

  for (const p of scene.parts) collectMesh(p.mesh);
  if (scene.partDefs) for (const pd of scene.partDefs) collectMesh(pd.mesh);
  if (scene.instances) for (const inst of scene.instances) collectMesh(inst.mesh);
  for (const c of scene.clashes) collectMesh(c);

  return buffers;
}

/** Convert WASM result to EvaluatedScene. Handles both typed arrays (fast path) and plain arrays. */
function wasmResultToScene(result: WasmEvaluatedScene): EvaluatedScene {
  const toMesh = (m: WasmMesh): TriangleMesh => ({
    positions: m.positions instanceof Float32Array ? m.positions : new Float32Array(m.positions),
    indices: m.indices instanceof Uint32Array ? m.indices : new Uint32Array(m.indices),
    normals: m.normals
      ? m.normals instanceof Float32Array ? m.normals : new Float32Array(m.normals)
      : undefined,
  });

  return {
    parts: result.parts.map((p) => ({ mesh: toMesh(p.mesh), material: p.material })),
    partDefs: result.partDefs?.map((pd) => ({ id: pd.id, mesh: toMesh(pd.mesh) })),
    instances: result.instances?.map((inst) => ({
      instanceId: inst.instance_id,
      partDefId: inst.part_def_id,
      name: inst.name,
      mesh: toMesh(inst.mesh),
      material: inst.material,
      transform: inst.transform as EvaluatedScene["instances"] extends Array<infer T> ? T extends { transform?: infer X } ? X : never : never,
    })),
    clashes: result.clashes.map(toMesh),
    timing: result.timing,
  };
}

/** Run evaluation using whichever path is available. */
function evaluate(docJson: string, skipClashDetection: boolean): EvaluatedScene {
  // Fast path: native WASM evaluator (with fallback to TS on failure)
  if (evaluatorMode === "wasm" && wasmEvaluateDocument) {
    try {
      const result = wasmEvaluateDocument(docJson, skipClashDetection);
      return wasmResultToScene(result);
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
      };

      // Check if native WASM evaluator is available
      wasmEvaluateDocument = (wasm as Record<string, unknown>).evaluateDocument as WasmEvaluateDocumentFn | null;
      evaluatorMode = wasmEvaluateDocument ? "wasm" : "ts";

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
