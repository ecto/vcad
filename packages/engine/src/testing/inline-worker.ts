/**
 * Test infrastructure: an in-thread stand-in for `engine-worker.ts` that
 * speaks the real worker protocol. Every message is structured-cloned both
 * ways and delivered on a later task, as a real `Worker` would, so code
 * driven through {@link workerTransport} meets the same async boundary
 * (no shared references, no synchronous replies) without a browser.
 */
import { handleEngineRequest, type EngineHostState, type EngineRequest } from "../engine-host.js";

/** What `workerTransport` needs from a `Worker`. */
export interface InlineEngineWorker {
  postMessage(data: unknown): void;
  addEventListener(type: "message", listener: (e: MessageEvent) => void): void;
  terminate(): void;
  /** Messages handled so far (for asserting the boundary was crossed). */
  readonly handled: number;
}

/** A fake worker hosting `host`'s engine. */
export function inlineEngineWorker(host: EngineHostState): InlineEngineWorker {
  const listeners: Array<(e: MessageEvent) => void> = [];
  let alive = true;
  let handled = 0;
  return {
    postMessage(data) {
      const { id, req } = structuredClone(data) as { id: number; req: EngineRequest };
      setTimeout(() => {
        if (!alive) return;
        handled++;
        const reply = structuredClone({ id, response: handleEngineRequest(host, req) });
        for (const l of listeners) l({ data: reply } as MessageEvent);
      }, 0);
    },
    addEventListener(_type, listener) {
      listeners.push(listener);
    },
    terminate() {
      alive = false;
      host.engine?.free();
      host.engine = null;
    },
    get handled() {
      return handled;
    },
  };
}
