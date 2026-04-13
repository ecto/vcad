import { create } from "zustand";

/** Standard orthographic and isometric view directions. */
export type ViewDirection =
  | "front"
  | "back"
  | "top"
  | "bottom"
  | "left"
  | "right"
  | "isometric";

/** 2D pan offset in SVG coordinates. */
export interface PanOffset {
  x: number;
  y: number;
}

/** Definition of a detail view (magnified region). */
export interface DetailViewDef {
  id: string;
  /** Center X in parent view coordinates. */
  centerX: number;
  /** Center Y in parent view coordinates. */
  centerY: number;
  /** Magnification factor (e.g., 2.0 = 2x). */
  scale: number;
  /** Width of region to capture in parent view units. */
  width: number;
  /** Height of region to capture in parent view units. */
  height: number;
  /** Label for the detail view (e.g., "A", "B"). */
  label: string;
}

/** Drawing view state for 2D technical drawing mode. */
interface DrawingState {
  /** Current view mode: 3D for interactive viewport, 2D for technical drawing. */
  viewMode: "3d" | "2d";
  /** View direction for 2D projection. */
  viewDirection: ViewDirection;
  /** Whether to show hidden lines (dashed) in 2D view. */
  showHiddenLines: boolean;
  /** Whether to show dimension annotations in 2D view. */
  showDimensions: boolean;
  /** Zoom level (1.0 = 100%). */
  zoom: number;
  /** Pan offset in SVG coordinates. */
  pan: PanOffset;
  /** Detail views (magnified regions). */
  detailViews: DetailViewDef[];
  /** Counter for generating unique detail view IDs. */
  nextDetailId: number;

  /** Switch between 3D and 2D view modes. */
  setViewMode: (mode: "3d" | "2d") => void;
  /** Set the view direction for 2D projection. */
  setViewDirection: (dir: ViewDirection) => void;
  /** Toggle hidden line visibility. */
  toggleHiddenLines: () => void;
  /** Toggle dimension annotation visibility. */
  toggleDimensions: () => void;
  /** Set the zoom level (clamped to 0.1-10). */
  setZoom: (zoom: number) => void;
  /** Adjust zoom by a delta (for wheel events). */
  adjustZoom: (delta: number, centerX?: number, centerY?: number) => void;
  /** Set the pan offset. */
  setPan: (pan: PanOffset) => void;
  /** Adjust pan by a delta. */
  adjustPan: (dx: number, dy: number) => void;
  /** Reset zoom and pan to defaults. */
  resetView: () => void;
  /** Add a new detail view. Returns the new detail view's ID. */
  addDetailView: (params: Omit<DetailViewDef, "id">) => string;
  /** Remove a detail view by ID. */
  removeDetailView: (id: string) => void;
  /** Update a detail view's parameters. */
  updateDetailView: (id: string, params: Partial<Omit<DetailViewDef, "id">>) => void;
  /** Clear all detail views. */
  clearDetailViews: () => void;
}

const MIN_ZOOM = 0.1;
const MAX_ZOOM = 10;

export const useDrawingStore = create<DrawingState>((set, get) => ({
  viewMode: "3d",
  viewDirection: "front",
  showHiddenLines: true,
  showDimensions: true,
  zoom: 1.0,
  pan: { x: 0, y: 0 },
  detailViews: [],
  nextDetailId: 1,

  setViewMode: (mode) => set({ viewMode: mode }),
  setViewDirection: (dir) => set({ viewDirection: dir, pan: { x: 0, y: 0 }, zoom: 1.0 }),
  toggleHiddenLines: () => set((s) => ({ showHiddenLines: !s.showHiddenLines })),
  toggleDimensions: () => set((s) => ({ showDimensions: !s.showDimensions })),

  setZoom: (zoom) =>
    set({ zoom: Math.max(MIN_ZOOM, Math.min(MAX_ZOOM, zoom)) }),

  adjustZoom: (delta) =>
    set((s) => {
      const newZoom = Math.max(MIN_ZOOM, Math.min(MAX_ZOOM, s.zoom * (1 + delta)));
      return { zoom: newZoom };
    }),

  setPan: (pan) => set({ pan }),

  adjustPan: (dx, dy) =>
    set((s) => ({
      pan: { x: s.pan.x + dx, y: s.pan.y + dy },
    })),

  resetView: () => set({ zoom: 1.0, pan: { x: 0, y: 0 } }),

  addDetailView: (params) => {
    const state = get();
    const id = `detail-${state.nextDetailId}`;
    set({
      detailViews: [...state.detailViews, { ...params, id }],
      nextDetailId: state.nextDetailId + 1,
    });
    return id;
  },

  removeDetailView: (id) =>
    set((s) => ({
      detailViews: s.detailViews.filter((d) => d.id !== id),
    })),

  updateDetailView: (id, params) =>
    set((s) => ({
      detailViews: s.detailViews.map((d) =>
        d.id === id ? { ...d, ...params } : d
      ),
    })),

  clearDetailViews: () => set({ detailViews: [] }),
}));
