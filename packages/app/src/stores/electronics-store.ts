import { create } from "zustand";
import type { PcbLayer, Vec2, MeanderStyle, LengthTuneParams } from "@vcad/ir";
import type {
  DrcViolationResult,
  ErcViolationResult,
  NetlistResult,
} from "@vcad/engine";
import { useCoreElectronicsStore, getPcbNodeIds, useDocumentStore, isPcbBoardPart } from "@vcad/core";
import type { PcbBoardPartInfo } from "@vcad/core";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export type ElectronicsLayout = "split" | "schematic-only" | "pcb-only";
export type PcbTool = "select" | "move" | "route" | "length-tune" | "delete";
export type SchTool = "select" | "move" | "place" | "wire" | "label" | "delete";

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

export type SchematicDocked = "left" | "right" | "hidden";

export interface ElectronicsState {
  // Workspace
  active: boolean;
  layout: ElectronicsLayout;
  splitRatio: number;
  focusedPane: "schematic" | "pcb";

  // Schematic overlay docking (new 3D canvas mode)
  schematicDocked: SchematicDocked;

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

  // Schematic editing transient state
  schPlacingSymbol: string | null;
  schPlacingRotation: number;
  schWireStart: Vec2 | null;
  schWirePreview: Vec2 | null;
  schLabelName: string;
  schGridSize: number;
  schRefCounters: Record<string, number>;

  // PCB drag state
  pcbDragging: { fpIdx: number; startPos: Vec2 } | null;

  // Real-time validation
  drcViolations: DrcViolationResult[];
  ercViolations: ErcViolationResult[];

  // PCB 3D view state (Phase 2: tilt-to-3D + exploded stackup)
  tiltAngle: number; // degrees, 0 = top-down, >5 = tilted 3D
  stackupExplosion: number; // 0 = flat, 1 = fully exploded

  // Route state (Principle 3: constraint-first)
  routeActive: boolean;
  routeStartPad: { fpRef: string; padNum: string; net: string } | null;
  routePreview: Vec2[];
  routeClearanceCorridor: number;

  // Length tuning state
  lengthTuneParams: LengthTuneParams;
  lengthTuneNet: string | null;

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
  /** Zoom PCB canvas toward a screen point (relative to SVG top-left minus center offset). */
  zoomPcbAt: (delta: number, cx: number, cy: number) => void;
  adjustPcbPan: (dx: number, dy: number) => void;
  /** Zoom schematic canvas toward a screen point (relative to SVG top-left minus center offset). */
  zoomSchAt: (delta: number, cx: number, cy: number) => void;
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

  // Schematic editing actions
  setSchPlacingSymbol: (symbolId: string | null) => void;
  rotateSchPlacement: () => void;
  startSchWire: (pos: Vec2) => void;
  updateSchWirePreview: (pos: Vec2 | null) => void;
  cancelSchWire: () => void;
  setSchLabelName: (name: string) => void;
  nextRef: (prefix: string) => string;

  // Schematic overlay actions
  setSchematicDocked: (docked: SchematicDocked) => void;

  // PCB 3D view actions
  setTiltAngle: (angle: number) => void;
  setStackupExplosion: (explosion: number) => void;
  toggleStackupExplosion: () => void;

  // Length tuning actions
  setLengthTuneParams: (params: Partial<LengthTuneParams>) => void;
  startLengthTune: (net: string) => void;
  cancelLengthTune: () => void;

  // PCB drag actions
  startPcbDrag: (fpIdx: number, startPos: Vec2) => void;
  cancelPcbDrag: () => void;
}

export const useElectronicsStore = create<ElectronicsState>((set, get) => ({
  active: false,
  layout: "split",
  splitRatio: 0.5,
  focusedPane: "schematic",
  schematicDocked: "left",

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

  schPlacingSymbol: null,
  schPlacingRotation: 0,
  schWireStart: null,
  schWirePreview: null,
  schLabelName: "NET",
  schGridSize: 10,
  schRefCounters: {},

  pcbDragging: null,

  drcViolations: [],
  ercViolations: [],

  tiltAngle: 0,
  stackupExplosion: 0,

  routeActive: false,
  routeStartPad: null,
  routePreview: [],
  routeClearanceCorridor: 0.15,

  lengthTuneParams: {
    target_length: 50.0,
    max_amplitude: 2.0,
    spacing: 1.0,
    style: "Trombone" as MeanderStyle,
  },
  lengthTuneNet: null,

  enter: () => {
    // Find first PcbBoard node and enter the core electronics store with it
    const docStore = useDocumentStore.getState();
    const doc = docStore.document;
    const boardIds = getPcbNodeIds(doc);
    let boardNodeId = boardIds[0] ?? null;
    // CRDT materializer creates Empty nodes + stores PCB in doc.pcb,
    // so getPcbNodeIds may miss them. Fall back to parts array.
    if (boardNodeId == null) {
      const pcbPart = docStore.parts.find(isPcbBoardPart) as PcbBoardPartInfo | undefined;
      if (pcbPart) boardNodeId = pcbPart.boardNodeId;
    }
    if (boardNodeId != null) {
      useCoreElectronicsStore.getState().enter(boardNodeId);
    }
    set({ active: true, schematicDocked: "left" });
  },
  exit: () => {
    useCoreElectronicsStore.getState().exit();
    set({
      active: false,
      selection: { type: "none" },
      hoveredNet: null,
      routeActive: false,
      routeStartPad: null,
      routePreview: [],
    });
  },

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

  zoomPcbAt: (delta, cx, cy) =>
    set((s) => {
      const oldZoom = s.pcbZoom;
      const newZoom = Math.max(0.1, Math.min(50, oldZoom * (1 + delta)));
      if (newZoom === oldZoom) return {};
      return {
        pcbZoom: newZoom,
        pcbPan: {
          x: s.pcbPan.x + cx * (1 / newZoom - 1 / oldZoom),
          y: s.pcbPan.y + cy * (1 / newZoom - 1 / oldZoom),
        },
      };
    }),
  adjustPcbPan: (dx, dy) =>
    set((s) => ({ pcbPan: { x: s.pcbPan.x + dx, y: s.pcbPan.y + dy } })),
  zoomSchAt: (delta, cx, cy) =>
    set((s) => {
      const oldZoom = s.schZoom;
      const newZoom = Math.max(0.1, Math.min(50, oldZoom * (1 + delta)));
      if (newZoom === oldZoom) return {};
      return {
        schZoom: newZoom,
        schPan: {
          x: s.schPan.x + cx * (1 / newZoom - 1 / oldZoom),
          y: s.schPan.y + cy * (1 / newZoom - 1 / oldZoom),
        },
      };
    }),
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

  // Schematic editing
  setSchPlacingSymbol: (symbolId) =>
    set({
      schPlacingSymbol: symbolId,
      schPlacingRotation: 0,
      schTool: symbolId ? "place" : "select",
    }),

  rotateSchPlacement: () =>
    set((s) => ({ schPlacingRotation: (s.schPlacingRotation + 90) % 360 })),

  startSchWire: (pos) =>
    set({ schWireStart: pos, schWirePreview: pos }),

  updateSchWirePreview: (pos) =>
    set({ schWirePreview: pos }),

  cancelSchWire: () =>
    set({ schWireStart: null, schWirePreview: null }),

  setSchLabelName: (name) => set({ schLabelName: name }),

  nextRef: (prefix) => {
    const s = get();
    const schematic = useDocumentStore.getState().document.schematic;
    let maxNum = s.schRefCounters[prefix] ?? 0;
    if (schematic) {
      for (const comp of schematic.components) {
        if (comp.ref.startsWith(prefix)) {
          const num = parseInt(comp.ref.slice(prefix.length), 10);
          if (!isNaN(num) && num > maxNum) maxNum = num;
        }
      }
    }
    const count = maxNum + 1;
    set({ schRefCounters: { ...s.schRefCounters, [prefix]: count } });
    return `${prefix}${count}`;
  },

  // Length tuning
  setLengthTuneParams: (params) =>
    set((s) => ({
      lengthTuneParams: { ...s.lengthTuneParams, ...params },
    })),

  startLengthTune: (net) =>
    set({
      pcbTool: "length-tune",
      lengthTuneNet: net,
    }),

  cancelLengthTune: () =>
    set({
      lengthTuneNet: null,
      pcbTool: "select",
    }),

  // Schematic overlay
  setSchematicDocked: (schematicDocked) => set({ schematicDocked }),

  // PCB 3D view
  setTiltAngle: (tiltAngle) => set({ tiltAngle: Math.max(0, Math.min(75, tiltAngle)) }),
  setStackupExplosion: (stackupExplosion) => set({ stackupExplosion: Math.max(0, Math.min(1, stackupExplosion)) }),
  toggleStackupExplosion: () => set((s) => ({ stackupExplosion: s.stackupExplosion > 0.5 ? 0 : 1 })),

  // PCB drag
  startPcbDrag: (fpIdx, startPos) =>
    set({ pcbDragging: { fpIdx, startPos } }),

  cancelPcbDrag: () => set({ pcbDragging: null }),
}));
