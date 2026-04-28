import { create } from "zustand";

/**
 * Opaque pointer to a SliceResult living inside the slicer Web Worker.
 * The WASM `SliceResult` object can't cross postMessage, so the main
 * thread holds only this lightweight handle and round-trips through
 * `slicer-client.ts` for layer previews and downstream gcode/3MF
 * generation.
 */
export interface SliceResult {
  handle: number;
  layerCount: number;
}

export type InfillPattern = "grid" | "lines" | "triangles" | "honeycomb" | "gyroid";

export interface SliceSettings {
  layerHeight: number;
  firstLayerHeight: number;
  nozzleDiameter: number;
  lineWidth: number;
  wallCount: number;
  infillDensity: number;
  infillPattern: InfillPattern;
  supportEnabled: boolean;
  supportAngle: number;
}

export interface SliceStats {
  layerCount: number;
  printTimeSeconds: number;
  filamentMm: number;
  filamentGrams: number;
  boundsMin: [number, number, number];
  boundsMax: [number, number, number];
}

export interface LayerPreview {
  z: number;
  index: number;
  outerPerimeters: [number, number][][];
  innerPerimeters: [number, number][][];
  infill: [number, number][][];
}

interface SlicerStore {
  // Settings
  settings: SliceSettings;
  setSettings: (settings: Partial<SliceSettings>) => void;
  resetSettings: () => void;

  // Slice result
  isSlicing: boolean;
  sliceError: string | null;
  stats: SliceStats | null;
  sliceResult: SliceResult | null;
  setSlicing: (slicing: boolean) => void;
  setSliceError: (error: string | null) => void;
  setStats: (stats: SliceStats | null) => void;
  setSliceResult: (result: SliceResult | null) => void;

  // Preview
  previewLayerIndex: number;
  setPreviewLayerIndex: (index: number) => void;
  currentLayerPreview: LayerPreview | null;
  setCurrentLayerPreview: (preview: LayerPreview | null) => void;

  // Panel state
  printPanelOpen: boolean;
  openPrintPanel: () => void;
  closePrintPanel: () => void;
}

const DEFAULT_SETTINGS: SliceSettings = {
  layerHeight: 0.2,
  firstLayerHeight: 0.25,
  nozzleDiameter: 0.4,
  lineWidth: 0.45,
  wallCount: 3,
  infillDensity: 0.15,
  infillPattern: "grid",
  supportEnabled: false,
  supportAngle: 45,
};

export const useSlicerStore = create<SlicerStore>((set) => ({
  // Settings
  settings: DEFAULT_SETTINGS,
  setSettings: (newSettings) =>
    set((state) => ({
      settings: { ...state.settings, ...newSettings },
    })),
  resetSettings: () => set({ settings: DEFAULT_SETTINGS }),

  // Slice result
  isSlicing: false,
  sliceError: null,
  stats: null,
  sliceResult: null,
  setSlicing: (slicing) => set({ isSlicing: slicing }),
  setSliceError: (error) => set({ sliceError: error }),
  setStats: (stats) => set({ stats }),
  setSliceResult: (result) => set({ sliceResult: result }),

  // Preview
  previewLayerIndex: 0,
  setPreviewLayerIndex: (index) => set({ previewLayerIndex: index }),
  currentLayerPreview: null,
  setCurrentLayerPreview: (preview) => set({ currentLayerPreview: preview }),

  // Panel state
  printPanelOpen: false,
  openPrintPanel: () => set({ printPanelOpen: true }),
  closePrintPanel: () => set({ printPanelOpen: false }),
}));

/**
 * Format seconds as human-readable duration
 */
export function formatDuration(seconds: number): string {
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);

  if (hours > 0) {
    return `${hours}h ${minutes}m`;
  }
  return `${minutes}m`;
}

/**
 * Convert infill pattern name to numeric ID for WASM
 */
export function infillPatternToId(pattern: InfillPattern): number {
  const map: Record<InfillPattern, number> = {
    grid: 0,
    lines: 1,
    triangles: 2,
    honeycomb: 3,
    gyroid: 4,
  };
  return map[pattern];
}
