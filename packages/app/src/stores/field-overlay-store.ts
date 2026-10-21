/**
 * Compressed-field overlay state: a stack of co-registered decoded fields
 * (one per physics), the active layer, and how it is drawn.
 *
 * Open with `?fields=/fields/bracket` (a `stack.json` directory) or
 * `?field=/fields/hot_chip` (one `.tc` + `.json`, a one-layer stack).
 * Decode backend is automatic (CPU below 4M cells); `&fieldgpu` / `&fieldcpu` force one. Each
 * layer also loads `<file>.ref.f32` when present, to measure the real error.
 * Console: `useFieldOverlayStore.getState().loadStack("/fields/bracket")`.
 */
import { create } from "zustand";
import {
  loadCompressedField,
  loadFieldStack,
  type FieldStack,
  type StackLayer,
} from "@/lib/compressed-field";

export type FieldViewMode = "skin" | "slice";
export type Axis = "x" | "y" | "z";

export interface FieldProbe {
  layerId: string;
  cell: number;
  value: number;
  pos: [number, number, number];
}

interface FieldOverlayState {
  stack: FieldStack | null;
  layers: StackLayer[];
  active: string | null;
  mode: FieldViewMode;
  axis: Axis;
  slice: number;
  probe: FieldProbe | null;
  totalMs: number;
  loading: boolean;
  error: string | null;
  loadStack: (dir: string, opts?: { forceCpu?: boolean; forceGpu?: boolean }) => Promise<void>;
  load: (base: string, opts?: { forceCpu?: boolean; forceGpu?: boolean }) => Promise<void>;
  setActive: (id: string) => void;
  setMode: (m: FieldViewMode) => void;
  setAxis: (a: Axis) => void;
  setSlice: (k: number) => void;
  setProbe: (p: FieldProbe | null) => void;
  clear: () => void;
}

const axisLen = (l: StackLayer | undefined, a: Axis) => {
  if (!l) return 1;
  const [nz, ny, nx] = l.field.meta.dims_zyx;
  return a === "x" ? nx : a === "y" ? ny : nz;
};

export const activeLayer = (s: Pick<FieldOverlayState, "layers" | "active">) =>
  s.layers.find((l) => l.id === s.active);

function publish(layers: StackLayer[], totalMs: number) {
  const summary = layers.map((l) => ({
    id: l.id,
    backend: l.field.backend,
    hash: l.field.hash,
    native: l.field.meta.native_fnv1a,
    hashMatch: l.field.hashMatch,
    measuredMaxAbs: l.field.measuredMaxAbs,
    bound: l.field.meta.bound.value,
    decodeMs: l.field.timings.decodeMs,
  }));
  console.info("[compressed-field]", { totalMs, layers: summary });
  (window as unknown as { __vcadField?: unknown }).__vcadField = { totalMs, layers: summary };
}

export const useFieldOverlayStore = create<FieldOverlayState>((set, get) => ({
  stack: null,
  layers: [],
  active: null,
  mode: "skin",
  axis: "z",
  slice: 0,
  probe: null,
  totalMs: 0,
  loading: false,
  error: null,
  loadStack: async (dir, opts) => {
    set({ loading: true, error: null });
    try {
      const { stack, layers, totalMs } = await loadFieldStack(dir, opts);
      set({
        stack,
        layers,
        active: layers[0]?.id ?? null,
        mode: stack.geometry ? "skin" : "slice",
        axis: "z",
        slice: Math.floor(axisLen(layers[0], "z") / 2),
        probe: null,
        totalMs,
        loading: false,
      });
      publish(layers, totalMs);
    } catch (e) {
      set({ loading: false, error: e instanceof Error ? e.message : String(e) });
    }
  },
  load: async (base, opts) => {
    set({ loading: true, error: null });
    try {
      const t0 = performance.now();
      const field = await loadCompressedField(base, { ...opts, referenceUrl: `${base}.ref.f32` });
      const layer: StackLayer = { id: field.name, label: field.meta.field, field, range: [field.meta.min, field.meta.max] };
      // Default to the hottest z-slice — where the interesting structure is.
      const [nz, ny, nx] = field.meta.dims_zyx;
      let best = 0;
      let bestV = -Infinity;
      for (let i = 0; i < nz * ny * nx; i++) {
        if (field.data[i]! > bestV) {
          bestV = field.data[i]!;
          best = Math.floor(i / (nx * ny));
        }
      }
      const totalMs = performance.now() - t0;
      set({
        stack: { name: field.name, title: field.name, layers: [{ id: layer.id, label: layer.label, file: field.name }] },
        layers: [layer],
        active: layer.id,
        mode: "slice",
        axis: "z",
        slice: best,
        probe: null,
        totalMs,
        loading: false,
      });
      publish([layer], totalMs);
    } catch (e) {
      set({ loading: false, error: e instanceof Error ? e.message : String(e) });
    }
  },
  setActive: (id) => {
    const s = get();
    const l = s.layers.find((x) => x.id === id);
    set({ active: id, probe: null, slice: Math.min(s.slice, axisLen(l, s.axis) - 1) });
  },
  setMode: (mode) => set({ mode }),
  setAxis: (axis) => set({ axis, slice: Math.floor(axisLen(activeLayer(get()), axis) / 2) }),
  setSlice: (slice) => set({ slice }),
  setProbe: (probe) => set({ probe }),
  clear: () => set({ stack: null, layers: [], active: null, probe: null, error: null }),
}));

if (typeof window !== "undefined") {
  (window as unknown as { useFieldOverlayStore?: unknown }).useFieldOverlayStore = useFieldOverlayStore;
  const p = new URLSearchParams(window.location.search);
  const opts = { forceCpu: p.has("fieldcpu"), forceGpu: p.has("fieldgpu") };
  const dir = p.get("fields");
  const base = p.get("field");
  if (dir) void useFieldOverlayStore.getState().loadStack(dir, opts);
  else if (base) void useFieldOverlayStore.getState().load(base, opts);
}
