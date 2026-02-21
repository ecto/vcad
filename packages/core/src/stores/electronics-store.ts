import { create } from "zustand";
import type { NodeId, PcbLayer } from "@vcad/ir";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export type PcbTool = "select" | "move" | "route" | "delete";
export type SchTool = "select" | "move" | "place" | "wire" | "label" | "delete";

export type ElectronicsSelection =
  | { type: "none" }
  | { type: "component"; ref: string }
  | { type: "net"; netId: string }
  | { type: "footprint"; ref: string }
  | { type: "trace"; idx: number; net: string }
  | { type: "via"; idx: number; net: string }
  | { type: "pad"; fpRef: string; padNum: string; net: string };

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

export interface CoreElectronicsState {
  /** Which PcbBoard node is currently being edited (null = none active) */
  activeBoardNodeId: NodeId | null;

  /** Whether the electronics workspace is active */
  active: boolean;

  /** Current selection */
  selection: ElectronicsSelection;

  /** Active PCB tool */
  pcbTool: PcbTool;

  /** Active schematic tool */
  schTool: SchTool;

  /** Active PCB copper layer */
  pcbActiveLayer: PcbLayer;

  // Actions
  enter: (nodeId: NodeId) => void;
  exit: () => void;
  select: (sel: ElectronicsSelection) => void;
  setPcbTool: (t: PcbTool) => void;
  setSchTool: (t: SchTool) => void;
  setPcbActiveLayer: (l: PcbLayer) => void;
}

export const useCoreElectronicsStore = create<CoreElectronicsState>((set) => ({
  activeBoardNodeId: null,
  active: false,
  selection: { type: "none" },
  pcbTool: "select",
  schTool: "select",
  pcbActiveLayer: "FCu",

  enter: (nodeId) => set({ active: true, activeBoardNodeId: nodeId }),
  exit: () =>
    set({
      active: false,
      activeBoardNodeId: null,
      selection: { type: "none" },
    }),

  select: (selection) => set({ selection }),
  setPcbTool: (pcbTool) => set({ pcbTool }),
  setSchTool: (schTool) => set({ schTool }),
  setPcbActiveLayer: (pcbActiveLayer) => set({ pcbActiveLayer }),
}));
