/**
 * Web Worker hosting the document engine (`WasmDocumentEngine`).
 *
 * Protocol: `{id, req: EngineRequest}` in, `{id, response: EngineResponse}`
 * out; see `engine-host.ts`. Requests are handled in arrival order.
 */
import {
  handleEngineRequest,
  type EngineHostState,
  type EngineRequest,
  type HostedEngineClass,
  type HostedEvaluator,
} from "./engine-host.js";
import { collectTransferables } from "./scene-convert.js";

let host: Promise<EngineHostState> | null = null;

function boot(): Promise<EngineHostState> {
  host ??= import("@vcad/kernel-wasm").then(async (wasm) => {
    await wasm.default();
    const k = wasm as unknown as {
      WasmDocumentEngine: HostedEngineClass;
      IncrementalEvaluator?: new () => HostedEvaluator;
    };
    // The engine and its evaluator share this worker: evaluation reads the
    // engine's document in place, no copy crosses a thread.
    return { Engine: k.WasmDocumentEngine, engine: null, Evaluator: k.IncrementalEvaluator };
  });
  return host;
}

self.onmessage = async (e: MessageEvent) => {
  const { id, req } = e.data as { id: number; req: EngineRequest };
  try {
    const h = await boot();
    const response = handleEngineRequest(h, req);
    const transfer = response.ok && response.scene ? collectTransferables(response.scene) : [];
    (self as unknown as DedicatedWorkerGlobalScope).postMessage({ id, response }, transfer);
  } catch (err) {
    self.postMessage({ id, response: { ok: false, message: `engine worker: ${String(err)}` } });
  }
};
