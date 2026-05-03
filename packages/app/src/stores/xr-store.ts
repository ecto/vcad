import { createXRStore } from "@react-three/xr";
import { create, useStore } from "zustand";

/**
 * The shared @react-three/xr store. Hand tracking + transient pointers are
 * enabled by default; we bind it to our scene via `<XR store={xrStore}>`.
 *
 * `foveation` is set to 0 (sharpest) so CAD edges read crisply at near-field
 * Vision Pro distances. Tweak down to 1.0 if framerate suffers on Quest.
 */
export const xrStore = createXRStore({
  hand: true,
  controller: true,
  foveation: 0,
});

type SupportState = {
  /** WebXR `immersive-vr` session is supported by this UA. */
  vr: boolean;
  /** WebXR `immersive-ar` session is supported by this UA. */
  ar: boolean;
  /** Whether we've finished the async support check. Until then UI hides buttons. */
  checked: boolean;
};

interface XRSupportStore extends SupportState {
  refresh: () => Promise<void>;
}

export const useXRSupportStore = create<XRSupportStore>((set) => ({
  vr: false,
  ar: false,
  checked: false,
  refresh: async () => {
    const xr = (navigator as unknown as { xr?: XRSystem }).xr;
    if (!xr) {
      set({ vr: false, ar: false, checked: true });
      return;
    }
    try {
      const [vr, ar] = await Promise.all([
        xr.isSessionSupported("immersive-vr").catch(() => false),
        xr.isSessionSupported("immersive-ar").catch(() => false),
      ]);
      set({ vr, ar, checked: true });
    } catch {
      set({ vr: false, ar: false, checked: true });
    }
  },
}));

/** Kick off the support check once at module load. */
if (typeof navigator !== "undefined") {
  void useXRSupportStore.getState().refresh();
}

/**
 * React hook that returns true while a WebXR session is active.
 *
 * Usable outside the `<Canvas>` / `<XR>` tree (where `useXR` from
 * `@react-three/xr` won't work). Drives the Canvas `frameloop` prop so
 * we ride the WebXR rAF instead of `"demand"` while presenting.
 */
export function useXRPresenting(): boolean {
  return useStore(xrStore, (s) => s.mode != null);
}
