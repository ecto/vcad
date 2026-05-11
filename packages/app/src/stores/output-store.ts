import { create } from "zustand";
import type { DfmProcess } from "@vcad/engine";

/**
 * Three top-level material picks for the marketing-facing QuotePanel.
 * Each maps to a specific (`process`, `material name`) pair the kernel's
 * shared cost model recognises in `vcad-kernel-cost::Material::catalog()`.
 *
 * The store no longer owns price/volume math — that lives in
 * `engine.estimateCost()`, which calls the same Rust estimator the
 * slicer and DFM cost section use. The QuotePanel calls it
 * asynchronously and animates the result.
 */
export type MaterialType = "pla" | "aluminum" | "steel";

export interface MaterialMapping {
  /** Manufacturing process used for cost estimation. */
  process: DfmProcess;
  /** Material name from `vcad-kernel-cost`'s catalog. */
  catalogName: string;
  /** Display label / lead time / method shown in the panel. */
  display: { name: string; method: string; days: number };
}

export const MATERIAL_MAPPINGS: Record<MaterialType, MaterialMapping> = {
  pla: {
    process: "fdm",
    catalogName: "PLA",
    display: { name: "PLA", method: "3D Print", days: 3 },
  },
  aluminum: {
    process: "cnc_3axis",
    catalogName: "Aluminum 6061",
    display: { name: "Aluminum", method: "CNC", days: 5 },
  },
  steel: {
    process: "cnc_3axis",
    catalogName: "Steel 1018",
    display: { name: "Steel", method: "CNC", days: 7 },
  },
};

interface OutputStore {
  // Quote panel state
  quotePanelOpen: boolean;
  openQuotePanel: () => void;
  closeQuotePanel: () => void;

  // Material selection
  selectedMaterial: MaterialType;
  setSelectedMaterial: (m: MaterialType) => void;

  /** Last computed prices keyed by material. Populated by the
   *  QuotePanel's async estimateCost loop; read by tooltips and
   *  toolbars that need a cached number without re-fetching. */
  cachedPrices: Partial<Record<MaterialType, number>>;
  setCachedPrices: (prices: Partial<Record<MaterialType, number>>) => void;
}

export const useOutputStore = create<OutputStore>((set) => ({
  // Quote panel
  quotePanelOpen: false,
  openQuotePanel: () => set({ quotePanelOpen: true }),
  closeQuotePanel: () => set({ quotePanelOpen: false }),

  // Material
  selectedMaterial: "pla",
  setSelectedMaterial: (m) => set({ selectedMaterial: m }),

  // Price cache
  cachedPrices: {},
  setCachedPrices: (cachedPrices) => set({ cachedPrices }),
}));
