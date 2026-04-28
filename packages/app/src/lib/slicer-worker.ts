/**
 * Web Worker that runs the WASM slicer off the main thread.
 *
 * The Print panel used to call `sliceMesh()` synchronously inside React's
 * render thread, which froze the UI for many seconds on real models. This
 * worker hosts the slicer's WASM instance and keeps `SliceResult` objects
 * alive in a handle map (they can't cross postMessage), so the main thread
 * sees only stats + extracted preview points.
 *
 * Protocol: every request carries an `id`; the worker replies with
 * `{type: "result", id, ok, payload | error}`. After WASM init it also
 * fires a one-shot `{type: "ready"}` so the client knows when to start
 * sending real work.
 */
/// <reference lib="webworker" />

interface PlainSlicerSettings {
  layerHeight: number;
  firstLayerHeight: number;
  nozzleDiameter: number;
  lineWidth: number;
  wallCount: number;
  infillDensity: number;
  /** Numeric infill ID matching the Rust enum (see infillPatternToId). */
  infillPattern: number;
  supportEnabled: boolean;
  supportAngle: number;
}

interface LayerPreviewPayload {
  z: number;
  index: number;
  outerPerimeters: [number, number][][];
  innerPerimeters: [number, number][][];
  infill: [number, number][][];
}

interface SlicerWasm {
  default: (input?: WebAssembly.Module | { module_or_path: WebAssembly.Module }) => Promise<unknown>;
  isSlicerAvailable?: () => boolean;
  SlicerSettings: new () => {
    layer_height: number;
    first_layer_height: number;
    nozzle_diameter: number;
    line_width: number;
    wall_count: number;
    infill_density: number;
    infill_pattern: number;
    support_enabled: boolean;
    support_angle: number;
  };
  sliceMesh: (
    vertices: Float32Array,
    indices: Uint32Array,
    settings: object,
  ) => SliceResultHandle;
  sliceMeshWithProgress: (
    vertices: Float32Array,
    indices: Uint32Array,
    settings: object,
    progressCb: (stage: string, current: number, total: number) => void,
  ) => SliceResultHandle;
  generateGcode: (
    result: SliceResultHandle,
    profile: string,
    printTemp: number,
    bedTemp: number,
  ) => string;
  generate3mf: (
    name: string,
    vertices: Float32Array,
    indices: Uint32Array,
    settingsJson: string,
  ) => Uint8Array;
  generate3mfWithGcode: (
    name: string,
    vertices: Float32Array,
    indices: Uint32Array,
    gcode: Uint8Array,
    settingsJson: string,
  ) => Uint8Array;
}

interface RawLayerPreview {
  z: number;
  index: number;
  outer_perimeters: [number, number][][];
  inner_perimeters: [number, number][][];
  infill: [number, number][][];
}

interface SliceResultHandle {
  layerCount: number;
  statsJson(): string;
  getLayerPreview(index: number): RawLayerPreview;
}

let wasm: SlicerWasm | null = null;
const sliceResults = new Map<number, SliceResultHandle>();
let nextSliceHandle = 1;

function buildSettings(plain: PlainSlicerSettings) {
  if (!wasm) throw new Error("slicer wasm not initialized");
  const s = new wasm.SlicerSettings();
  s.layer_height = plain.layerHeight;
  s.first_layer_height = plain.firstLayerHeight;
  s.nozzle_diameter = plain.nozzleDiameter;
  s.line_width = plain.lineWidth;
  s.wall_count = plain.wallCount;
  s.infill_density = plain.infillDensity;
  s.infill_pattern = plain.infillPattern;
  s.support_enabled = plain.supportEnabled;
  s.support_angle = plain.supportAngle;
  return s;
}

function previewToPayload(raw: RawLayerPreview): LayerPreviewPayload {
  return {
    z: raw.z,
    index: raw.index,
    outerPerimeters: raw.outer_perimeters,
    innerPerimeters: raw.inner_perimeters,
    infill: raw.infill,
  };
}

type Reply =
  | { type: "ready" }
  | { type: "result"; id: number; ok: true; payload: unknown }
  | { type: "result"; id: number; ok: false; error: string }
  | { type: "progress"; id: number; stage: string; current: number; total: number };

function reply(msg: Reply, transfer?: Transferable[]) {
  if (transfer && transfer.length > 0) {
    (self as unknown as DedicatedWorkerGlobalScope).postMessage(msg, transfer);
  } else {
    self.postMessage(msg);
  }
}

self.onmessage = async (e: MessageEvent) => {
  const msg = e.data as
    | { type: "init" }
    | {
        type: "slice";
        id: number;
        vertices: Float32Array;
        indices: Uint32Array;
        settings: PlainSlicerSettings;
      }
    | { type: "getLayerPreview"; id: number; sliceHandle: number; index: number }
    | {
        type: "generateGcode";
        id: number;
        sliceHandle: number;
        profile: string;
        printTemp: number;
        bedTemp: number;
      }
    | {
        type: "generate3mfWithGcode";
        id: number;
        sliceHandle: number;
        name: string;
        vertices: Float32Array;
        indices: Uint32Array;
        gcode: Uint8Array;
        settingsJson: string;
      }
    | {
        type: "generate3mf";
        id: number;
        name: string;
        vertices: Float32Array;
        indices: Uint32Array;
        settingsJson: string;
      }
    | { type: "release"; id: number; sliceHandle: number };

  if (msg.type === "init") {
    try {
      const mod = (await import("@vcad/kernel-wasm")) as unknown as SlicerWasm;
      await mod.default();
      if (typeof mod.isSlicerAvailable !== "function" || !mod.isSlicerAvailable()) {
        throw new Error("slicer is not enabled in this WASM build");
      }
      wasm = mod;
      reply({ type: "ready" });
    } catch (err) {
      reply({
        type: "result",
        id: -1,
        ok: false,
        error: err instanceof Error ? err.message : String(err),
      });
    }
    return;
  }

  if (!wasm) {
    if ("id" in msg) {
      reply({ type: "result", id: msg.id, ok: false, error: "slicer worker not initialized" });
    }
    return;
  }

  try {
    switch (msg.type) {
      case "slice": {
        const settings = buildSettings(msg.settings);
        // Progress callback fires synchronously inside the WASM call. We're
        // on the worker thread, so postMessage to the main thread doesn't
        // block anything that matters.
        const progressCb = (stage: string, current: number, total: number) => {
          self.postMessage({
            type: "progress",
            id: msg.id,
            stage,
            current,
            total,
          });
        };
        const useProgress = typeof wasm.sliceMeshWithProgress === "function";
        const result = useProgress
          ? wasm.sliceMeshWithProgress(msg.vertices, msg.indices, settings, progressCb)
          : wasm.sliceMesh(msg.vertices, msg.indices, settings);
        const handle = nextSliceHandle++;
        sliceResults.set(handle, result);
        const layerCount = result.layerCount;
        const statsJson = result.statsJson();
        const firstPreview =
          layerCount > 0 ? previewToPayload(result.getLayerPreview(0)) : null;
        reply({
          type: "result",
          id: msg.id,
          ok: true,
          payload: { sliceHandle: handle, layerCount, statsJson, firstPreview },
        });
        return;
      }
      case "getLayerPreview": {
        const result = sliceResults.get(msg.sliceHandle);
        if (!result) throw new Error(`unknown slice handle ${msg.sliceHandle}`);
        const payload = previewToPayload(result.getLayerPreview(msg.index));
        reply({ type: "result", id: msg.id, ok: true, payload });
        return;
      }
      case "generateGcode": {
        const result = sliceResults.get(msg.sliceHandle);
        if (!result) throw new Error(`unknown slice handle ${msg.sliceHandle}`);
        const gcode = wasm.generateGcode(result, msg.profile, msg.printTemp, msg.bedTemp);
        reply({ type: "result", id: msg.id, ok: true, payload: { gcode } });
        return;
      }
      case "generate3mfWithGcode": {
        const bytes = wasm.generate3mfWithGcode(
          msg.name,
          msg.vertices,
          msg.indices,
          msg.gcode,
          msg.settingsJson,
        );
        reply(
          { type: "result", id: msg.id, ok: true, payload: { bytes } },
          [bytes.buffer as ArrayBuffer],
        );
        return;
      }
      case "generate3mf": {
        const bytes = wasm.generate3mf(msg.name, msg.vertices, msg.indices, msg.settingsJson);
        reply(
          { type: "result", id: msg.id, ok: true, payload: { bytes } },
          [bytes.buffer as ArrayBuffer],
        );
        return;
      }
      case "release": {
        sliceResults.delete(msg.sliceHandle);
        reply({ type: "result", id: msg.id, ok: true, payload: null });
        return;
      }
    }
  } catch (err) {
    reply({
      type: "result",
      id: "id" in msg ? msg.id : -1,
      ok: false,
      error: err instanceof Error ? err.message : String(err),
    });
  }
};
