import { create } from "zustand";
import type { PcbLayer, Vec2 } from "@vcad/ir";
import type {
  DrcViolationResult,
  ErcViolationResult,
  NetlistResult,
} from "@vcad/engine";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export type ElectronicsLayout = "split" | "schematic-only" | "pcb-only";
export type PcbTool = "select" | "move" | "route";
export type SchTool = "select" | "move";

export type ElectronicsSelection =
  | { type: "none" }
  | { type: "component"; ref: string }
  | { type: "net"; netId: string }
  | { type: "footprint"; ref: string }
  | { type: "trace"; idx: number; net: string }
  | { type: "via"; idx: number; net: string }
  | { type: "pad"; fpRef: string; padNum: string; net: string };

export interface LayerConfig {
  layer: PcbLayer;
  color: string;
  visible: boolean;
  opacity: number;
}

// ---------------------------------------------------------------------------
// Default layer colors (KiCad convention)
// ---------------------------------------------------------------------------

const DEFAULT_LAYERS: LayerConfig[] = [
  { layer: "FCu", color: "#F44336", visible: true, opacity: 1.0 },
  { layer: "BCu", color: "#2196F3", visible: true, opacity: 1.0 },
  { layer: "In1Cu", color: "#FFEB3B", visible: false, opacity: 1.0 },
  { layer: "In2Cu", color: "#4CAF50", visible: false, opacity: 1.0 },
  { layer: "FSilkS", color: "#FFEB3B", visible: true, opacity: 1.0 },
  { layer: "BSilkS", color: "#E040FB", visible: true, opacity: 1.0 },
  { layer: "FMask", color: "#9C27B0", visible: false, opacity: 0.5 },
  { layer: "BMask", color: "#4CAF50", visible: false, opacity: 0.5 },
  { layer: "EdgeCuts", color: "#FFD600", visible: true, opacity: 1.0 },
  { layer: "FCrtYd", color: "#FF9800", visible: false, opacity: 0.6 },
  { layer: "BCrtYd", color: "#00BCD4", visible: false, opacity: 0.6 },
  { layer: "FFab", color: "#795548", visible: false, opacity: 0.8 },
  { layer: "BFab", color: "#607D8B", visible: false, opacity: 0.8 },
];

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

export interface ElectronicsState {
  // Workspace
  active: boolean;
  layout: ElectronicsLayout;
  splitRatio: number;
  focusedPane: "schematic" | "pcb";

  // Net-centric selection (Principle 2)
  selection: ElectronicsSelection;
  hoveredNet: string | null;

  // Continuous sync (Principle 7)
  netlist: NetlistResult | null;
  orphanFootprints: string[];
  unplacedComponents: string[];

  // PCB view state
  pcbZoom: number;
  pcbPan: Vec2;
  pcbTool: PcbTool;
  pcbActiveLayer: PcbLayer;
  pcbLayers: LayerConfig[];
  pcbGridSize: number;
  pcbSnapToGrid: boolean;

  // Schematic view state
  schZoom: number;
  schPan: Vec2;
  schTool: SchTool;

  // Real-time validation
  drcViolations: DrcViolationResult[];
  ercViolations: ErcViolationResult[];

  // Route state (Principle 3: constraint-first)
  routeActive: boolean;
  routeStartPad: { fpRef: string; padNum: string; net: string } | null;
  routePreview: Vec2[];
  routeClearanceCorridor: number;

  // Actions
  enter: () => void;
  exit: () => void;
  setLayout: (l: ElectronicsLayout) => void;
  setFocusedPane: (p: "schematic" | "pcb") => void;
  select: (sel: ElectronicsSelection) => void;
  setHoveredNet: (netId: string | null) => void;
  setPcbTool: (t: PcbTool) => void;
  setSchTool: (t: SchTool) => void;
  setPcbActiveLayer: (l: PcbLayer) => void;
  inferLayerFromPad: (layers: PcbLayer[]) => void;
  adjustPcbZoom: (delta: number) => void;
  adjustPcbPan: (dx: number, dy: number) => void;
  adjustSchZoom: (delta: number) => void;
  adjustSchPan: (dx: number, dy: number) => void;
  setPcbGridSize: (size: number) => void;
  setPcbSnapToGrid: (snap: boolean) => void;
  setLayerVisible: (layer: PcbLayer, visible: boolean) => void;
  setLayerOpacity: (layer: PcbLayer, opacity: number) => void;
  setDrcViolations: (v: DrcViolationResult[]) => void;
  setErcViolations: (v: ErcViolationResult[]) => void;
  setNetlist: (n: NetlistResult) => void;
  setOrphanFootprints: (refs: string[]) => void;
  setUnplacedComponents: (refs: string[]) => void;
  startRouteFromRatsnest: (fpRef: string, padNum: string, net: string) => void;
  startRoute: (fpRef: string, padNum: string, net: string) => void;
  updateRoutePreview: (points: Vec2[]) => void;
  cancelRoute: () => void;
  finishRoute: () => void;
}

export const useElectronicsStore = create<ElectronicsState>((set) => ({
  active: false,
  layout: "split",
  splitRatio: 0.5,
  focusedPane: "schematic",

  selection: { type: "none" },
  hoveredNet: null,

  netlist: null,
  orphanFootprints: [],
  unplacedComponents: [],

  pcbZoom: 1,
  pcbPan: { x: 0, y: 0 },
  pcbTool: "select",
  pcbActiveLayer: "FCu",
  pcbLayers: DEFAULT_LAYERS.map((l) => ({ ...l })),
  pcbGridSize: 0.5,
  pcbSnapToGrid: true,

  schZoom: 1,
  schPan: { x: 0, y: 0 },
  schTool: "select",

  drcViolations: [],
  ercViolations: [],

  routeActive: false,
  routeStartPad: null,
  routePreview: [],
  routeClearanceCorridor: 0.15,

  enter: () => set({ active: true }),
  exit: () =>
    set({
      active: false,
      selection: { type: "none" },
      hoveredNet: null,
      routeActive: false,
      routeStartPad: null,
      routePreview: [],
    }),

  setLayout: (layout) => set({ layout }),
  setFocusedPane: (focusedPane) => set({ focusedPane }),

  select: (selection) => set({ selection }),
  setHoveredNet: (hoveredNet) => set({ hoveredNet }),

  setPcbTool: (pcbTool) =>
    set({
      pcbTool,
      routeActive: pcbTool === "route" ? undefined : false,
      routeStartPad: pcbTool === "route" ? undefined : null,
      routePreview: pcbTool === "route" ? undefined : [],
    }),
  setSchTool: (schTool) => set({ schTool }),

  // Principle 5: layer follows intent
  setPcbActiveLayer: (pcbActiveLayer) => set({ pcbActiveLayer }),
  inferLayerFromPad: (layers) => {
    const copper = layers.find(
      (l) => l === "FCu" || l === "BCu" || l.startsWith("In"),
    );
    if (copper) set({ pcbActiveLayer: copper });
  },

  adjustPcbZoom: (delta) =>
    set((s) => ({
      pcbZoom: Math.max(0.1, Math.min(50, s.pcbZoom * (1 + delta))),
    })),
  adjustPcbPan: (dx, dy) =>
    set((s) => ({ pcbPan: { x: s.pcbPan.x + dx, y: s.pcbPan.y + dy } })),
  adjustSchZoom: (delta) =>
    set((s) => ({
      schZoom: Math.max(0.1, Math.min(50, s.schZoom * (1 + delta))),
    })),
  adjustSchPan: (dx, dy) =>
    set((s) => ({ schPan: { x: s.schPan.x + dx, y: s.schPan.y + dy } })),
  setPcbGridSize: (pcbGridSize) => set({ pcbGridSize }),
  setPcbSnapToGrid: (pcbSnapToGrid) => set({ pcbSnapToGrid }),

  setLayerVisible: (layer, visible) =>
    set((s) => ({
      pcbLayers: s.pcbLayers.map((l) =>
        l.layer === layer ? { ...l, visible } : l,
      ),
    })),
  setLayerOpacity: (layer, opacity) =>
    set((s) => ({
      pcbLayers: s.pcbLayers.map((l) =>
        l.layer === layer ? { ...l, opacity } : l,
      ),
    })),

  setDrcViolations: (drcViolations) => set({ drcViolations }),
  setErcViolations: (ercViolations) => set({ ercViolations }),
  setNetlist: (netlist) => set({ netlist }),
  setOrphanFootprints: (orphanFootprints) => set({ orphanFootprints }),
  setUnplacedComponents: (unplacedComponents) => set({ unplacedComponents }),

  // Principle 4: ratsnest affordance
  startRouteFromRatsnest: (fpRef, padNum, net) =>
    set({
      pcbTool: "route",
      routeActive: true,
      routeStartPad: { fpRef, padNum, net },
      routePreview: [],
      selection: { type: "pad", fpRef, padNum, net },
    }),

  startRoute: (fpRef, padNum, net) =>
    set({
      routeActive: true,
      routeStartPad: { fpRef, padNum, net },
      routePreview: [],
    }),

  updateRoutePreview: (routePreview) => set({ routePreview }),

  cancelRoute: () =>
    set({
      routeActive: false,
      routeStartPad: null,
      routePreview: [],
    }),

  finishRoute: () =>
    set({
      routeActive: false,
      routeStartPad: null,
      routePreview: [],
    }),
}));
