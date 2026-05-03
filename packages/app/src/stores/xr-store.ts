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
  // Disable the bundled iwer device emulator. It would otherwise auto-inject a
  // floating "Enter XR" DOM button on localhost, which clashes with our
  // in-menubar XR entry. Real headsets are unaffected.
  emulate: false,
});

type SupportState = {
  /** WebXR `immersive-vr` session is supported by this UA. */
  vr: boolean;
  /** WebXR `immersive-ar` session is supported by this UA. */
  ar: boolean;
  /** Whether we've finished the async support check. Until then UI hides buttons. */
  checked: boolean;
};

/**
 * Default scene scale on entering XR. 1/1000 maps the kernel's millimetre
 * units to physical metres, so a 100 mm cube reads as a 10 cm cube on the
 * desk. Exported for `XRSceneTransform` and the scale-teleport reset.
 */
export const XR_DEFAULT_SCALE = 0.001;
/** Headset-height + slightly forward, world units (metres). */
export const XR_DEFAULT_POSITION: readonly [number, number, number] = [0, 1.0, -0.7];

export interface XRViewTransform {
  /** Uniform scale applied to the kernel scene group. */
  scale: number;
  /** World-space position of the scene origin. */
  position: readonly [number, number, number];
}

interface XRSupportStore extends SupportState {
  refresh: () => Promise<void>;
  /** Live view transform — driven by scale-teleport gestures. */
  view: XRViewTransform;
  setView: (view: XRViewTransform) => void;
  resetView: () => void;
}

const DEFAULT_VIEW: XRViewTransform = {
  scale: XR_DEFAULT_SCALE,
  position: XR_DEFAULT_POSITION,
};

export const useXRSupportStore = create<XRSupportStore>((set) => ({
  vr: false,
  ar: false,
  checked: false,
  view: DEFAULT_VIEW,
  setView: (view) => set({ view }),
  resetView: () => set({ view: DEFAULT_VIEW }),
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
