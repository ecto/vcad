/**
 * Tiny Zustand store for dev-only viewport overlays (boundary edges,
 * BRep face colors, etc). Separated from the rest of the UI state
 * because these toggles are for debugging and shouldn't pollute the
 * production state shape.
 *
 * Toggle from the browser console:
 *   useDebugOverlayStore.getState().setShowBoundaryEdges(true)
 *
 * Or via the keyboard shortcut registered by `DebugOverlayHotkeys`.
 */
import { create } from "zustand";

interface DebugOverlayState {
  showBoundaryEdges: boolean;
  setShowBoundaryEdges: (v: boolean) => void;
  toggleBoundaryEdges: () => void;
}

export const useDebugOverlayStore = create<DebugOverlayState>((set) => ({
  showBoundaryEdges: false,
  setShowBoundaryEdges: (v) => set({ showBoundaryEdges: v }),
  toggleBoundaryEdges: () =>
    set((s) => ({ showBoundaryEdges: !s.showBoundaryEdges })),
}));

if (typeof window !== "undefined" && import.meta.env.DEV) {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (window as any).__VCAD_DEBUG_OVERLAY = useDebugOverlayStore;
}
