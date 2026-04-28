/**
 * Main-thread client for the slicer Web Worker.
 *
 * The slicer's `sliceMesh` is a multi-second WASM call. Running it inline
 * in React froze the UI (no event-loop ticks, no spinner updates, no
 * cancel). This module owns one Worker, sends it requests, and resolves
 * promises against ids. Cancellation is best-effort: we `terminate()` the
 * worker and lazily spin up a new one for the next call.
 *
 * The heavy `SliceResult` lives entirely on the worker side, identified
 * by an opaque integer handle. The main thread only ever holds stats
 * (small JSON) and the polylines for the currently-shown layer.
 */

import type { SliceSettings as PlainSliceSettings } from "@/stores/slicer-store";
import { infillPatternToId } from "@/stores/slicer-store";

export interface SliceHandle {
  /** Worker-side identifier; release with `releaseSlice()`. */
  handle: number;
  layerCount: number;
}

export interface LayerPreview {
  z: number;
  index: number;
  outerPerimeters: [number, number][][];
  innerPerimeters: [number, number][][];
  infill: [number, number][][];
}

export interface SliceOutcome {
  handle: SliceHandle;
  statsJson: string;
  firstPreview: LayerPreview | null;
}

/** Progress callback signature: (stage, current, total). */
export type SliceProgressFn = (stage: string, current: number, total: number) => void;

interface PendingRequest {
  resolve: (value: unknown) => void;
  reject: (reason: Error) => void;
  onProgress?: SliceProgressFn;
}

let worker: Worker | null = null;
let workerReady: Promise<void> | null = null;
let nextId = 1;
const pending = new Map<number, PendingRequest>();

function ensureWorker(): { worker: Worker; ready: Promise<void> } {
  if (worker && workerReady) {
    return { worker, ready: workerReady };
  }

  const w = new Worker(new URL("./slicer-worker.ts", import.meta.url), {
    type: "module",
    name: "vcad-slicer",
  });

  workerReady = new Promise<void>((resolve, reject) => {
    const onMsg = (e: MessageEvent) => {
      const data = e.data as { type: string; ok?: boolean; error?: string; id?: number };
      if (data.type === "ready") {
        w.removeEventListener("message", onMsg);
        resolve();
        return;
      }
      // Init failure surfaces as a result with id === -1.
      if (data.type === "result" && data.id === -1 && data.ok === false) {
        w.removeEventListener("message", onMsg);
        reject(new Error(data.error ?? "slicer worker init failed"));
      }
    };
    w.addEventListener("message", onMsg);
  });

  w.addEventListener("message", (e: MessageEvent) => {
    const data = e.data as
      | { type: "result"; id: number; ok: true; payload: unknown }
      | { type: "result"; id: number; ok: false; error: string }
      | { type: "progress"; id: number; stage: string; current: number; total: number }
      | { type: "ready" };
    if (data.type === "progress") {
      const slot = pending.get(data.id);
      if (slot?.onProgress) slot.onProgress(data.stage, data.current, data.total);
      return;
    }
    if (data.type !== "result") return;
    if (data.id < 0) return; // init signal handled above
    const slot = pending.get(data.id);
    if (!slot) return;
    pending.delete(data.id);
    if (data.ok) slot.resolve(data.payload);
    else slot.reject(new Error(data.error));
  });

  w.addEventListener("error", (e) => {
    // Reject every in-flight request rather than letting them hang.
    for (const [id, slot] of pending) {
      slot.reject(new Error(`slicer worker error: ${e.message}`));
      pending.delete(id);
    }
  });

  w.postMessage({ type: "init" });
  worker = w;
  return { worker: w, ready: workerReady };
}

function send<T>(
  message: object,
  transfer: Transferable[] = [],
  onProgress?: SliceProgressFn,
): Promise<T> {
  const { worker: w, ready } = ensureWorker();
  const id = nextId++;
  const promise = new Promise<T>((resolve, reject) => {
    pending.set(id, {
      resolve: (v) => resolve(v as T),
      reject,
      onProgress,
    });
  });
  ready
    .then(() => {
      w.postMessage({ ...message, id }, transfer);
    })
    .catch((err: Error) => {
      const slot = pending.get(id);
      if (slot) {
        pending.delete(id);
        slot.reject(err);
      }
    });
  return promise;
}

/** Convert plain settings into the on-the-wire payload. */
function toPlainPayload(settings: PlainSliceSettings) {
  return {
    layerHeight: settings.layerHeight,
    firstLayerHeight: settings.firstLayerHeight,
    nozzleDiameter: settings.nozzleDiameter,
    lineWidth: settings.lineWidth,
    wallCount: settings.wallCount,
    infillDensity: settings.infillDensity,
    infillPattern: infillPatternToId(settings.infillPattern),
    supportEnabled: settings.supportEnabled,
    supportAngle: settings.supportAngle,
  };
}

export async function sliceMesh(
  vertices: Float32Array,
  indices: Uint32Array,
  settings: PlainSliceSettings,
  onProgress?: SliceProgressFn,
): Promise<SliceOutcome> {
  const result = await send<{
    sliceHandle: number;
    layerCount: number;
    statsJson: string;
    firstPreview: LayerPreview | null;
  }>(
    {
      type: "slice",
      vertices,
      indices,
      settings: toPlainPayload(settings),
    },
    [vertices.buffer as ArrayBuffer, indices.buffer as ArrayBuffer],
    onProgress,
  );
  return {
    handle: { handle: result.sliceHandle, layerCount: result.layerCount },
    statsJson: result.statsJson,
    firstPreview: result.firstPreview,
  };
}

export async function getLayerPreview(
  handle: SliceHandle,
  index: number,
): Promise<LayerPreview> {
  return send<LayerPreview>({
    type: "getLayerPreview",
    sliceHandle: handle.handle,
    index,
  });
}

export async function generateGcode(
  handle: SliceHandle,
  profile: string,
  printTemp: number,
  bedTemp: number,
): Promise<string> {
  const out = await send<{ gcode: string }>({
    type: "generateGcode",
    sliceHandle: handle.handle,
    profile,
    printTemp,
    bedTemp,
  });
  return out.gcode;
}

export async function generate3mfWithGcode(
  handle: SliceHandle,
  name: string,
  vertices: Float32Array,
  indices: Uint32Array,
  gcode: Uint8Array,
  settingsJson: string,
): Promise<Uint8Array> {
  const out = await send<{ bytes: Uint8Array }>(
    {
      type: "generate3mfWithGcode",
      sliceHandle: handle.handle,
      name,
      vertices,
      indices,
      gcode,
      settingsJson,
    },
    [vertices.buffer as ArrayBuffer, indices.buffer as ArrayBuffer, gcode.buffer as ArrayBuffer],
  );
  return out.bytes;
}

export async function generate3mf(
  name: string,
  vertices: Float32Array,
  indices: Uint32Array,
  settingsJson: string,
): Promise<Uint8Array> {
  const out = await send<{ bytes: Uint8Array }>(
    {
      type: "generate3mf",
      name,
      vertices,
      indices,
      settingsJson,
    },
    [vertices.buffer as ArrayBuffer, indices.buffer as ArrayBuffer],
  );
  return out.bytes;
}

export async function releaseSlice(handle: SliceHandle): Promise<void> {
  await send<null>({ type: "release", sliceHandle: handle.handle });
}

/**
 * Forcibly stop in-flight slicing. Terminates the Worker (the only way to
 * interrupt a synchronous WASM call), rejects every pending promise, and
 * resets state so the next call lazily spins up a fresh worker.
 */
export function cancelSlicing() {
  if (!worker) return;
  worker.terminate();
  worker = null;
  workerReady = null;
  for (const [id, slot] of pending) {
    slot.reject(new Error("slicing cancelled"));
    pending.delete(id);
  }
}
