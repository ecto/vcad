/**
 * Compressed-field overlay state: the decoded field and which z-slice of
 * it is drawn in the viewport.
 *
 * Open one with `?field=/fields/hot_chip` (loads `.tc` + `.json`, and
 * `.ref.f32` when present to measure the real error), or from the console:
 *   useFieldOverlayStore.getState().load("/fields/hot_chip")
 */
import { create } from "zustand";
import { loadCompressedField, type DecodedField } from "@/lib/compressed-field";

interface FieldOverlayState {
  field: DecodedField | null;
  slice: number;
  loading: boolean;
  error: string | null;
  load: (base: string, opts?: { forceCpu?: boolean }) => Promise<void>;
  setSlice: (k: number) => void;
  clear: () => void;
}

export const useFieldOverlayStore = create<FieldOverlayState>((set) => ({
  field: null,
  slice: 0,
  loading: false,
  error: null,
  load: async (base, opts) => {
    set({ loading: true, error: null });
    try {
      const field = await loadCompressedField(base, { ...opts, referenceUrl: `${base}.ref.f32` });
      // Default to the hottest slice — where the interesting structure is.
      const [nz, ny, nx] = field.meta.dims_zyx;
      let best = 0;
      let bestV = -Infinity;
      for (let k = 0; k < nz; k++) {
        for (let i = k * nx * ny; i < (k + 1) * nx * ny; i++) {
          if (field.data[i]! > bestV) {
            bestV = field.data[i]!;
            best = k;
          }
        }
      }
      set({ field, slice: best, loading: false });
      console.info("[compressed-field]", {
        backend: field.backend,
        hash: field.hash,
        hashMatch: field.hashMatch,
        measuredMaxAbs: field.measuredMaxAbs,
        timings: field.timings,
      });
      (window as unknown as { __vcadField?: unknown }).__vcadField = {
        backend: field.backend,
        hash: field.hash,
        native: field.meta.native_fnv1a,
        hashMatch: field.hashMatch,
        measuredMaxAbs: field.measuredMaxAbs,
        bound: field.meta.bound,
        timings: field.timings,
      };
    } catch (e) {
      set({ loading: false, error: e instanceof Error ? e.message : String(e) });
    }
  },
  setSlice: (k) => set({ slice: k }),
  clear: () => set({ field: null, error: null }),
}));

if (typeof window !== "undefined") {
  (window as unknown as { useFieldOverlayStore?: unknown }).useFieldOverlayStore = useFieldOverlayStore;
  const p = new URLSearchParams(window.location.search);
  const base = p.get("field");
  if (base) void useFieldOverlayStore.getState().load(base, { forceCpu: p.has("fieldcpu") });
}
