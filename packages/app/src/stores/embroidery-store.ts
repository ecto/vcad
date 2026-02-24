import { create } from "zustand";

export interface EmbroideryThread {
  color: [number, number, number];
  name: string;
}

export interface StitchPath {
  threadIndex: number;
  color: [number, number, number];
  points: [number, number][];
}

export interface EmbroideryPatternData {
  stitchCount: number;
  colorCount: number;
  width: number;
  height: number;
  threads: EmbroideryThread[];
  stitchPaths: StitchPath[];
}

export interface EmbroideryStats {
  stitchCount: number;
  colorCount: number;
  width: number;
  height: number;
  estimatedTimeSeconds: number;
  threadLength: number;
}

interface EmbroideryStore {
  panelOpen: boolean;
  openPanel: () => void;
  closePanel: () => void;

  pattern: EmbroideryPatternData | null;
  setPattern: (p: EmbroideryPatternData | null) => void;

  stats: EmbroideryStats | null;
  setStats: (s: EmbroideryStats | null) => void;

  error: string | null;
  setError: (e: string | null) => void;

  selectedFormat: "pes" | "dst";
  setSelectedFormat: (f: "pes" | "dst") => void;

  fileName: string | null;
  setFileName: (name: string | null) => void;

  patternJson: string | null;
  setPatternJson: (json: string | null) => void;
}

export const useEmbroideryStore = create<EmbroideryStore>((set) => ({
  panelOpen: false,
  openPanel: () => set({ panelOpen: true }),
  closePanel: () => set({ panelOpen: false, error: null }),

  pattern: null,
  setPattern: (pattern) => set({ pattern }),

  stats: null,
  setStats: (stats) => set({ stats }),

  error: null,
  setError: (error) => set({ error }),

  selectedFormat: "pes",
  setSelectedFormat: (selectedFormat) => set({ selectedFormat }),

  fileName: null,
  setFileName: (fileName) => set({ fileName }),

  patternJson: null,
  setPatternJson: (patternJson) => set({ patternJson }),
}));

/**
 * Format seconds as human-readable duration.
 */
export function formatEmbroideryDuration(seconds: number): string {
  const minutes = Math.floor(seconds / 60);
  const secs = Math.floor(seconds % 60);
  if (minutes > 0) {
    return `${minutes}m ${secs}s`;
  }
  return `${secs}s`;
}
