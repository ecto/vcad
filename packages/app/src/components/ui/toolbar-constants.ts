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

// Per-tab style theme. All class names are spelled out as literal strings so
// Tailwind's JIT scanner can pick them up — string concatenation at runtime
// still works because the literals appear here in source.
export interface TabTheme {
  /** active icon/text color (e.g. text-emerald-400) */
  text: string;
  /** group-hover variant of `text` for inactive icons that color in on hover */
  groupHoverText: string;
  /** active background tint */
  bg: string;
  /** hover background tint for inactive items */
  hoverBg: string;
  /** solid accent (tooltip stripe, focus ring) */
  accent: string;
}

export const TAB_THEMES: Record<ToolbarTab, TabTheme> = {
  create: {
    text: "text-emerald-400",
    groupHoverText: "group-hover:text-emerald-400",
    bg: "bg-emerald-400/10",
    hoverBg: "hover:bg-emerald-400/5",
    accent: "bg-emerald-400",
  },
  transform: {
    text: "text-blue-400",
    groupHoverText: "group-hover:text-blue-400",
    bg: "bg-blue-400/10",
    hoverBg: "hover:bg-blue-400/5",
    accent: "bg-blue-400",
  },
  combine: {
    text: "text-violet-400",
    groupHoverText: "group-hover:text-violet-400",
    bg: "bg-violet-400/10",
    hoverBg: "hover:bg-violet-400/5",
    accent: "bg-violet-400",
  },
  modify: {
    text: "text-amber-400",
    groupHoverText: "group-hover:text-amber-400",
    bg: "bg-amber-400/10",
    hoverBg: "hover:bg-amber-400/5",
    accent: "bg-amber-400",
  },
  assembly: {
    text: "text-rose-400",
    groupHoverText: "group-hover:text-rose-400",
    bg: "bg-rose-400/10",
    hoverBg: "hover:bg-rose-400/5",
    accent: "bg-rose-400",
  },
  simulate: {
    text: "text-cyan-400",
    groupHoverText: "group-hover:text-cyan-400",
    bg: "bg-cyan-400/10",
    hoverBg: "hover:bg-cyan-400/5",
    accent: "bg-cyan-400",
  },
  build: {
    text: "text-slate-300",
    groupHoverText: "group-hover:text-slate-200",
    bg: "bg-slate-400/10",
    hoverBg: "hover:bg-slate-400/5",
    accent: "bg-slate-300",
  },
  sketch: {
    text: "text-amber-400",
    groupHoverText: "group-hover:text-amber-400",
    bg: "bg-amber-400/10",
    hoverBg: "hover:bg-amber-400/5",
    accent: "bg-amber-400",
  },
};

// One-line descriptions surfaced inside the rich tab tooltips.
export const TAB_DESCRIPTIONS: Record<ToolbarTab, string> = {
  create: "Add primitives, sketches, text, and PCB boards.",
  sketch: "Draw 2D profiles and convert to 3D with extrude, revolve, sweep, or loft.",
  transform: "Move, rotate, and scale the selected parts.",
  combine: "Boolean union, difference, and intersection of two parts.",
  modify: "Fillets, chamfers, shells, patterns, and mirror.",
  assembly: "Define parts, place instances, and connect them with joints.",
  simulate: "Run physics on jointed assemblies — play, pause, step.",
  build: "Switch views, export STL/GLB/STEP, print, route, and quote.",
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
