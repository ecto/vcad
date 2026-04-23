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

export interface InspectedTriangle {
  triangleIndex: number;
  faceKind: number;
  faceKindName: string;
  vertexIds: [number, number, number];
  positions: [[number, number, number], [number, number, number], [number, number, number]];
  centroid: [number, number, number];
  ccwNormal: [number, number, number];
  outwardDot: number;
}

interface DebugOverlayState {
  showBoundaryEdges: boolean;
  setShowBoundaryEdges: (v: boolean) => void;
  toggleBoundaryEdges: () => void;
  inspectTriangles: boolean;
  setInspectTriangles: (v: boolean) => void;
  toggleInspectTriangles: () => void;
  currentInspection: InspectedTriangle | null;
  setCurrentInspection: (v: InspectedTriangle | null) => void;
}

export const useDebugOverlayStore = create<DebugOverlayState>((set) => ({
  showBoundaryEdges: false,
  setShowBoundaryEdges: (v) => set({ showBoundaryEdges: v }),
  toggleBoundaryEdges: () =>
    set((s) => ({ showBoundaryEdges: !s.showBoundaryEdges })),
  inspectTriangles: false,
  setInspectTriangles: (v) =>
    set({ inspectTriangles: v, currentInspection: null }),
  toggleInspectTriangles: () =>
    set((s) => ({
      inspectTriangles: !s.inspectTriangles,
      currentInspection: null,
    })),
  currentInspection: null,
  setCurrentInspection: (v) => set({ currentInspection: v }),
}));

if (typeof window !== "undefined" && import.meta.env.DEV) {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (window as any).__VCAD_DEBUG_OVERLAY = useDebugOverlayStore;
}
