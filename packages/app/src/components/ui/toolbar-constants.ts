import type { ToolbarTab } from "@vcad/core";

// Responsive breakpoints
export const MOBILE_BREAKPOINT = 640;

// Tab colors for main toolbar
export const TAB_COLORS: Record<ToolbarTab, string> = {
  create: "text-emerald-400",
  transform: "text-blue-400",
  combine: "text-violet-400",
  modify: "text-amber-400",
  assembly: "text-rose-400",
  simulate: "text-cyan-400",
  build: "text-slate-400",
  sketch: "text-amber-400",
};

// Electronics toolbar tab types
export type ElectronicsTab = "schematic" | "components" | "pcb" | "view" | "finish";

// Tab colors for electronics toolbar
export const ELECTRONICS_TAB_COLORS: Record<ElectronicsTab, string> = {
  schematic: "text-indigo-400",
  components: "text-violet-400",
  pcb: "text-teal-400",
  view: "text-sky-400",
  finish: "text-rose-400",
};
