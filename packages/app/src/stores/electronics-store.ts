import { create } from "zustand";
import type { PcbLayer, Vec2, MeanderStyle, LengthTuneParams } from "@vcad/ir";
import type {
  CircuitAcResult,
  CircuitBlocker,
  CircuitDcResult,
  CircuitMapResult,
  CircuitSpecDevice,
  CircuitTuneResult,
  ComponentMesh,
  DrcViolationResult,
  ErcViolationResult,
  NetlistResult,
} from "@vcad/engine";
import { useCoreElectronicsStore, getPcbNodeIds, useDocumentStore, isPcbBoardPart } from "@vcad/core";
import type { PcbBoardPartInfo, ReceiptEntry } from "@vcad/core";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/** The electronics workspace shows one view at a time; the toolbar toggles. */
export type ElectronicsLayout = "schematic" | "board";
export type PcbTool = "select" | "move" | "route" | "length-tune" | "delete" | "constrain";

/** A target picked by the constrain tool. */
export type ConstraintTarget =
  | { kind: "footprint"; ref: string }
  | { kind: "outlineVertex"; idx: number };

/** Constraint sub-tools offered by the toolbar. */
export type PcbConstraintType =
  | "coincident"
  | "horizontal"
  | "vertical"
  | "distance"
  | "fixed";
export type SchTool = "select" | "move" | "place" | "wire" | "label" | "delete";

export type ElectronicsSelection =
  | { type: "none" }
  | { type: "component"; ref: string }
  | { type: "net"; netId: string }
  | { type: "footprint"; ref: string }
  | { type: "trace"; idx: number; net: string }
  | { type: "via"; idx: number; net: string }
  | { type: "pad"; fpRef: string; padNum: string; net: string };

/**
 * One-shot analysis results (DC operating point + AC sweep) over the mapped
 * schematic, plus the fail-closed blocker list when mapping refuses. `spec`
 * and `mapping` are kept so follow-up runs (output-net change, tune) don't
 * re-map.
 */
export interface CircuitAnalysisState {
  status: "idle" | "running" | "ok" | "blocked" | "error";
  error: string | null;
  /** Components that blocked simulation, pinned by refdes (fail-closed). */
  blockers: CircuitBlocker[];
  mapping: CircuitMapResult | null;
  spec: { devices: CircuitSpecDevice[] } | null;
  dc: CircuitDcResult | null;
  ac: CircuitAcResult | null;
  /** Net whose voltage the Bode panel plots. */
  outNet: string | null;
  /** Device id of the AC driving source. */
  sourceId: number | null;
  sweep: { startHz: number; stopHz: number; points: number };
  /** Show the analysis (Bode + health) panel. */
  showPanel: boolean;
  /** Show DC node voltages / device currents on the schematic. */
  showDcAnnotations: boolean;
  /** Structural signature of the schematic at run time (staleness check). */
  signature: string | null;
  /** Refdes with the tune dialog open, if any. */
  tuningRef: string | null;
  /** True while the adjoint tuner is running / animating. */
  tuneBusy: boolean;
  tuneResult: CircuitTuneResult | null;
}

const INITIAL_ANALYSIS: CircuitAnalysisState = {
  status: "idle",
  error: null,
  blockers: [],
  mapping: null,
  spec: null,
  dc: null,
  ac: null,
  outNet: null,
  sourceId: null,
  sweep: { startHz: 10, stopHz: 1e6, points: 60 },
  showPanel: false,
  showDcAnnotations: true,
  signature: null,
  tuningRef: null,
  tuneBusy: false,
  tuneResult: null,
};

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
  focusedPane: "schematic" | "pcb";

  // Net-centric selection (Principle 2)
  selection: ElectronicsSelection;
  hoveredNet: string | null;

  // Continuous sync (Principle 7)
  netlist: NetlistResult | null;
  orphanFootprints: string[];
  unplacedComponents: string[];

  // Live circuit simulation ("come alive")
  simulating: boolean;
  simNodeVoltages: number[] | null;
  simDeviceCurrents: number[] | null;
  simRotorAngles: number[] | null;
  simNetToNode: Record<string, number> | null;
  simRefToDevice: Record<string, number> | null;

  // One-shot DC/AC analysis + tune (Analyze flow)
  analysis: CircuitAnalysisState;

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
  /** Active constraint sub-tool. */
  pcbConstraintType: PcbConstraintType;
  /** Targets picked so far by the constrain tool (max 2). */
  pcbConstraintTargets: ConstraintTarget[];
  /** Dimensional constraint awaiting a value before commit. */
  pcbConstraintPending: { type: PcbConstraintType; targets: ConstraintTarget[] } | null;
  /** Last design-constraint solve outcome (status chip). */
  pcbSolveStatus: { converged: boolean; dof: number; overConstrained: boolean } | null;

  // Real-time validation
  drcViolations: DrcViolationResult[];
  ercViolations: ErcViolationResult[];

  // Live Receipt ledger (#280): one attributed entry per board mutation that
  // changed DRC, newest last. `pendingMutation` is an optional label an action
  // can stash so the recorder tags the next entry (e.g. "autoroute").
  receiptEntries: ReceiptEntry[];
  showReceiptPanel: boolean;
  pendingMutation: { tool: string; args: Record<string, unknown> } | null;

  // Route-vs-enclosure: footprint refs whose 3D body intersects a mechanical
  // part (Phase 3 — live MCAD/ECAD interference).
  interferingFootprints: string[];

  // Component 3D bodies (height-aware extrusions per footprint), recomputed by
  // the sync hook and rendered on the board. Whether to show them.
  componentBodies: ComponentMesh[];
  showComponentBodies: boolean;

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
  /** Instant-start: scaffold a default board + empty schematic if the document
   *  has none, then enter the circuit (lands in the schematic). */
  startCircuit: () => void;
  exit: () => void;
  setLayout: (l: ElectronicsLayout) => void;
  /** Flip between the schematic and board views. */
  toggleLayout: () => void;
  setFocusedPane: (p: "schematic" | "pcb") => void;
  select: (sel: ElectronicsSelection) => void;
  setHoveredNet: (netId: string | null) => void;
  setPcbTool: (t: PcbTool) => void;
  setPcbConstraintType: (t: PcbConstraintType) => void;
  pushConstraintTarget: (t: ConstraintTarget) => void;
  clearConstraintPicks: () => void;
  setPcbConstraintPending: (p: { type: PcbConstraintType; targets: ConstraintTarget[] } | null) => void;
  setPcbSolveStatus: (s: { converged: boolean; dof: number; overConstrained: boolean } | null) => void;
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
  appendReceiptEntry: (entry: ReceiptEntry) => void;
  clearReceipt: () => void;
  toggleReceiptPanel: () => void;
  /** Stash a label for the next recorded mutation (e.g. "autoroute"). */
  noteMutation: (tool: string, args?: Record<string, unknown>) => void;
  /** Read and clear the pending label — the recorder calls this per entry. */
  consumePendingMutation: () => { tool: string; args: Record<string, unknown> } | null;
  setInterferingFootprints: (refs: string[]) => void;
  setComponentBodies: (bodies: ComponentMesh[]) => void;
  toggleComponentBodies: () => void;
  setNetlist: (n: NetlistResult) => void;
  setOrphanFootprints: (refs: string[]) => void;
  setUnplacedComponents: (refs: string[]) => void;
  setSimulating: (b: boolean) => void;
  setSimMaps: (
    netToNode: Record<string, number>,
    refToDevice: Record<string, number>,
  ) => void;
  setSimObservation: (
    nodeVoltages: number[],
    deviceCurrents: number[],
    rotorAngles: number[],
  ) => void;
  clearSim: () => void;
  /** Merge fields into the analysis state. */
  setAnalysis: (patch: Partial<CircuitAnalysisState>) => void;
  /** Drop all analysis results (schematic edited / workspace exit). */
  clearAnalysis: () => void;
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
  layout: "schematic",
  focusedPane: "schematic",

  selection: { type: "none" },
  hoveredNet: null,

  netlist: null,
  orphanFootprints: [],
  unplacedComponents: [],

  simulating: false,
  simNodeVoltages: null,
  simDeviceCurrents: null,
  simRotorAngles: null,
  simNetToNode: null,
  simRefToDevice: null,

  analysis: { ...INITIAL_ANALYSIS },

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
  pcbConstraintType: "distance" as PcbConstraintType,
  pcbConstraintTargets: [],
  pcbConstraintPending: null,
  pcbSolveStatus: null,

  drcViolations: [],
  ercViolations: [],
  receiptEntries: [],
  showReceiptPanel: false,
  pendingMutation: null,
  interferingFootprints: [],
  componentBodies: [],
  showComponentBodies: true,

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
    set({ active: true, layout: "schematic" });
  },
  startCircuit: () => {
    const docStore = useDocumentStore.getState();
    if (!docStore.document.schematic) docStore.initSchematic();
    if (useDocumentStore.getState().document.pcb == null) {
      useDocumentStore.getState().initPcb();
    }
    get().enter();
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

  // The focused pane (which tool shortcuts apply) always tracks the visible
  // view, since only one view shows at a time.
  setLayout: (layout) =>
    set({ layout, focusedPane: layout === "schematic" ? "schematic" : "pcb" }),
  toggleLayout: () =>
    set((s) => {
      const layout = s.layout === "schematic" ? "board" : "schematic";
      return { layout, focusedPane: layout === "schematic" ? "schematic" : "pcb" };
    }),
  setFocusedPane: (focusedPane) => set({ focusedPane }),

  select: (selection) => set({ selection }),
  setHoveredNet: (hoveredNet) => set({ hoveredNet }),

  setPcbTool: (pcbTool) =>
    set({
      pcbTool,
      routeActive: pcbTool === "route" ? undefined : false,
      routeStartPad: pcbTool === "route" ? undefined : null,
      routePreview: pcbTool === "route" ? undefined : [],
      pcbConstraintTargets: [],
      pcbConstraintPending: null,
    }),
  setPcbConstraintType: (pcbConstraintType) =>
    set({ pcbConstraintType, pcbConstraintTargets: [], pcbConstraintPending: null }),
  pushConstraintTarget: (t) =>
    set((state) => {
      const targets = [...state.pcbConstraintTargets, t];
      const needed = state.pcbConstraintType === "fixed" ? 1 : 2;
      if (targets.length < needed) return { pcbConstraintTargets: targets };
      // Enough targets: dimensional types wait for a value, the rest are
      // committed by the toolbar effect watching pcbConstraintPending.
      return {
        pcbConstraintTargets: [],
        pcbConstraintPending: { type: state.pcbConstraintType, targets },
      };
    }),
  clearConstraintPicks: () =>
    set({ pcbConstraintTargets: [], pcbConstraintPending: null }),
  setPcbConstraintPending: (pcbConstraintPending) => set({ pcbConstraintPending }),
  setPcbSolveStatus: (pcbSolveStatus) => set({ pcbSolveStatus }),
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
  appendReceiptEntry: (entry) =>
    set((s) => ({ receiptEntries: [...s.receiptEntries, entry] })),
  clearReceipt: () => set({ receiptEntries: [] }),
  toggleReceiptPanel: () => set((s) => ({ showReceiptPanel: !s.showReceiptPanel })),
  noteMutation: (tool, args = {}) => set({ pendingMutation: { tool, args } }),
  consumePendingMutation: () => {
    const p = get().pendingMutation;
    if (p) set({ pendingMutation: null });
    return p;
  },
  setInterferingFootprints: (interferingFootprints) => set({ interferingFootprints }),
  setComponentBodies: (componentBodies) => set({ componentBodies }),
  toggleComponentBodies: () => set((s) => ({ showComponentBodies: !s.showComponentBodies })),
  setNetlist: (netlist) => set({ netlist }),
  setSimulating: (simulating) =>
    set(
      simulating
        ? { simulating }
        : {
            simulating: false,
            simNodeVoltages: null,
            simDeviceCurrents: null,
            simRotorAngles: null,
            simNetToNode: null,
            simRefToDevice: null,
          },
    ),
  setSimMaps: (simNetToNode, simRefToDevice) => set({ simNetToNode, simRefToDevice }),
  setSimObservation: (simNodeVoltages, simDeviceCurrents, simRotorAngles) =>
    set({ simNodeVoltages, simDeviceCurrents, simRotorAngles }),
  clearSim: () =>
    set({
      simNodeVoltages: null,
      simDeviceCurrents: null,
      simRotorAngles: null,
      simNetToNode: null,
      simRefToDevice: null,
    }),
  setAnalysis: (patch) => set((s) => ({ analysis: { ...s.analysis, ...patch } })),
  clearAnalysis: () =>
    set((s) => ({
      analysis: {
        ...INITIAL_ANALYSIS,
        sweep: s.analysis.sweep,
        showPanel: s.analysis.showPanel,
        showDcAnnotations: s.analysis.showDcAnnotations,
      },
    })),
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

  // PCB 3D view
  setTiltAngle: (tiltAngle) => set({ tiltAngle: Math.max(0, Math.min(75, tiltAngle)) }),
  setStackupExplosion: (stackupExplosion) => set({ stackupExplosion: Math.max(0, Math.min(1, stackupExplosion)) }),
  toggleStackupExplosion: () => set((s) => ({ stackupExplosion: s.stackupExplosion > 0.5 ? 0 : 1 })),

  // PCB drag
  startPcbDrag: (fpIdx, startPos) =>
    set({ pcbDragging: { fpIdx, startPos } }),

  cancelPcbDrag: () => set({ pcbDragging: null }),
}));
