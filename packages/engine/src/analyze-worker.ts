/**
 * Web Worker for off-main-thread solver studies (Analyze mode, #592).
 *
 * Runs the heavy kernel solvers (structural FEA, tolerance stackup) on a
 * dedicated thread with its own WASM instance so the viewport stays live.
 * Modeled on eval-worker.ts.
 *
 * Messages:
 *   → {type: 'init', module?}                            — load WASM module
 *   ← {type: 'ready'}                                    — WASM ready
 *   → {type: 'fea', id, specJson, optionsJson, positions, indices}
 *   → {type: 'tolerance', id, specJson, paramsJson, optionsJson}
 *   ← {type: 'result', id, analysis}                     — solver result
 *   ← {type: 'error', id, message}                       — solver error
 */

type FeaAnalyzeFn = (
  specJson: string,
  optionsJson: string,
  positions: Float32Array,
  indices: Uint32Array,
) => unknown;
type ToleranceAnalyzeFn = (
  specJson: string,
  paramsJson: string,
  optionsJson: string,
) => unknown;

let feaAnalyzeMesh: FeaAnalyzeFn | null = null;
let toleranceAnalyze: ToleranceAnalyzeFn | null = null;

self.onmessage = async (e: MessageEvent) => {
  const { type, id } = e.data;

  if (type === "init") {
    try {
      const wasm = await import("@vcad/kernel-wasm");
      const compiledModule: WebAssembly.Module | undefined = e.data.module;
      if (compiledModule) {
        await wasm.default({ module_or_path: compiledModule });
      } else {
        await wasm.default();
      }
      const m = wasm as Record<string, unknown>;
      feaAnalyzeMesh = m.feaAnalyzeMesh as FeaAnalyzeFn | null;
      toleranceAnalyze = m.toleranceAnalyze as ToleranceAnalyzeFn | null;
      self.postMessage({ type: "ready" });
    } catch (err) {
      self.postMessage({ type: "error", id: null, message: `WASM init failed: ${err}` });
    }
    return;
  }

  if (type === "fea") {
    if (!feaAnalyzeMesh) {
      self.postMessage({ type: "error", id, message: "Analyze worker not initialized" });
      return;
    }
    try {
      const { specJson, optionsJson, positions, indices } = e.data;
      const analysis = feaAnalyzeMesh(
        specJson,
        optionsJson ?? "",
        positions instanceof Float32Array ? positions : new Float32Array(positions),
        indices instanceof Uint32Array ? indices : new Uint32Array(indices),
      );
      self.postMessage({ type: "result", id, analysis });
    } catch (err) {
      self.postMessage({ type: "error", id, message: String(err) });
    }
    return;
  }

  if (type === "tolerance") {
    if (!toleranceAnalyze) {
      self.postMessage({ type: "error", id, message: "Analyze worker not initialized" });
      return;
    }
    try {
      const { specJson, paramsJson, optionsJson } = e.data;
      const analysis = toleranceAnalyze(specJson, paramsJson ?? "{}", optionsJson ?? "{}");
      self.postMessage({ type: "result", id, analysis });
    } catch (err) {
      self.postMessage({ type: "error", id, message: String(err) });
    }
    return;
  }
};
