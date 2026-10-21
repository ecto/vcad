/**
 * Web Worker hosting the document engine (`WasmDocumentEngine`).
 *
 * Protocol: `{id, req: EngineRequest}` in, `{id, response: EngineResponse}`
 * out; see `engine-host.ts`. Requests are handled in arrival order.
 */
import { handleEngineRequest, type EngineHostState, type EngineRequest, type HostedEngineClass } from "./engine-host.js";

let host: Promise<EngineHostState> | null = null;

function boot(): Promise<EngineHostState> {
  host ??= import("@vcad/kernel-wasm").then(async (wasm) => {
    await wasm.default();
    const Engine = (wasm as unknown as { WasmDocumentEngine: HostedEngineClass }).WasmDocumentEngine;
    return { Engine, engine: null };
  });
  return host;
}

self.onmessage = async (e: MessageEvent) => {
  const { id, req } = e.data as { id: number; req: EngineRequest };
  try {
    const h = await boot();
    self.postMessage({ id, response: handleEngineRequest(h, req) });
  } catch (err) {
    self.postMessage({ id, response: { ok: false, message: `engine worker: ${String(err)}` } });
  }
};
