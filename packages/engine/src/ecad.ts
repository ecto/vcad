/**
 * ECAD (Electronics) WASM wrappers.
 *
 * Lazy-loads the kernel WASM and provides typed wrappers for DRC, ERC,
 * netlist generation, routing, and zone fill.
 */

import type { SchematicSheet, Pcb, Vec2, PcbLayer } from "@vcad/ir";

// ---------------------------------------------------------------------------
// Result types (mirrors Rust serde output)
// ---------------------------------------------------------------------------

export type DrcRuleType =
  | "Clearance"
  | "MinTraceWidth"
  | "MinDrill"
  | "AnnularRing"
  | "EdgeClearance"
  | "HoleToHole"
  | "UnconnectedNet"
  | "SilkscreenClearance"
  | "CourtyardOverlap"
  | "AcidTrap";

export type DrcSeverity = "Error" | "Warning";

export interface DrcViolationResult {
  rule: DrcRuleType;
  severity: DrcSeverity;
  position: Vec2;
  message: string;
  actual: number;
  required: number;
}

export type ErcSeverity = "Error" | "Warning";

export interface ErcViolationResult {
  severity: ErcSeverity;
  message: string;
  position: Vec2 | null;
}

export interface NetConnection {
  component_ref: string;
  pin_number: string;
}

export interface NetlistNet {
  name: string;
  connections: NetConnection[];
}

export interface NetlistResult {
  nets: NetlistNet[];
}

export interface RouteSegment {
  start: Vec2;
  end: Vec2;
}

export interface RouteResult {
  net: string;
  segments: [Vec2, Vec2][];
  vias: Vec2[];
  success: boolean;
}

export interface FilledZoneResult {
  polygons: Vec2[][];
  net: string;
  layer: PcbLayer;
}

// ---------------------------------------------------------------------------
// Lazy WASM loader
// ---------------------------------------------------------------------------

// eslint-disable-next-line @typescript-eslint/no-explicit-any
let wasmModule: any = null;

async function loadEcadWasm(): Promise<typeof wasmModule | null> {
  if (wasmModule) return wasmModule;
  try {
    const wasm = await import("@vcad/kernel-wasm");
    if (typeof (wasm as Record<string, unknown>).isEcadAvailable !== "function") {
      return null;
    }
    wasmModule = wasm;
    return wasmModule;
  } catch {
    return null;
  }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/** Check if ECAD WASM features are available. */
export async function isEcadAvailable(): Promise<boolean> {
  const wasm = await loadEcadWasm();
  return wasm !== null;
}

/** Run Design Rule Check on a PCB. */
export async function runDrc(pcb: Pcb): Promise<DrcViolationResult[]> {
  const wasm = await loadEcadWasm();
  if (!wasm) return [];
  try {
    return wasm.ecadCheckDrc(JSON.stringify(pcb)) as DrcViolationResult[];
  } catch (e) {
    console.warn("[ECAD] DRC failed:", e);
    return [];
  }
}

/** Run Electrical Rule Check on a schematic. */
export async function runErc(sheet: SchematicSheet): Promise<ErcViolationResult[]> {
  const wasm = await loadEcadWasm();
  if (!wasm) return [];
  try {
    return wasm.ecadCheckErc(JSON.stringify(sheet)) as ErcViolationResult[];
  } catch (e) {
    console.warn("[ECAD] ERC failed:", e);
    return [];
  }
}

/** Generate a netlist from a schematic sheet. */
export async function generateNetlist(sheet: SchematicSheet): Promise<NetlistResult> {
  const wasm = await loadEcadWasm();
  if (!wasm) return { nets: [] };
  try {
    return wasm.ecadGenerateNetlist(JSON.stringify(sheet)) as NetlistResult;
  } catch (e) {
    console.warn("[ECAD] Netlist generation failed:", e);
    return { nets: [] };
  }
}

/** Route a net between two points on the PCB. */
export async function routeNet(
  pcb: Pcb,
  net: string,
  start: Vec2,
  end: Vec2,
  width: number,
): Promise<RouteResult> {
  const wasm = await loadEcadWasm();
  const fail: RouteResult = { net, segments: [], vias: [], success: false };
  if (!wasm) return fail;
  try {
    return wasm.ecadRouteNet(
      JSON.stringify(pcb),
      net,
      start.x,
      start.y,
      end.x,
      end.y,
      width,
    ) as RouteResult;
  } catch (e) {
    console.warn("[ECAD] Routing failed:", e);
    return fail;
  }
}

/**
 * Route a net with the push-and-shove router.
 *
 * Continuous-space counterpart to {@link routeNet}: it detours around existing
 * copper on *other* nets, producing cleaner diagonal paths than the grid/wave
 * router. Same result shape; returns a failed result if the kernel is
 * unavailable.
 */
export async function routeNetShove(
  pcb: Pcb,
  net: string,
  start: Vec2,
  end: Vec2,
  width: number,
): Promise<RouteResult> {
  const wasm = await loadEcadWasm();
  const fail: RouteResult = { net, segments: [], vias: [], success: false };
  if (!wasm) return fail;
  try {
    return wasm.ecadRouteNetShove(
      JSON.stringify(pcb),
      net,
      start.x,
      start.y,
      end.x,
      end.y,
      width,
    ) as RouteResult;
  } catch (e) {
    console.warn("[ECAD] Push-shove routing failed:", e);
    return fail;
  }
}

/** Parse a KiCad .kicad_pcb file into a Pcb struct. */
export async function parseKicadPcb(content: string): Promise<Pcb | null> {
  const wasm = await loadEcadWasm();
  if (!wasm) return null;
  try {
    return wasm.parseKicadPcb(content) as Pcb;
  } catch (e) {
    console.warn("[ECAD] KiCad PCB parse failed:", e);
    return null;
  }
}

/** Fill copper pour zones on the PCB. */
export async function fillZones(pcb: Pcb): Promise<FilledZoneResult[]> {
  const wasm = await loadEcadWasm();
  if (!wasm) return [];
  try {
    return wasm.ecadFillZones(JSON.stringify(pcb)) as FilledZoneResult[];
  } catch (e) {
    console.warn("[ECAD] Zone fill failed:", e);
    return [];
  }
}

// ---------------------------------------------------------------------------
// Builtin symbol/footprint library
// ---------------------------------------------------------------------------

export interface SymbolGraphic {
  type: "Rect" | "Line" | "Circle" | "Polyline";
  x?: number;
  y?: number;
  width?: number;
  height?: number;
  x1?: number;
  y1?: number;
  x2?: number;
  y2?: number;
  cx?: number;
  cy?: number;
  r?: number;
  points?: Vec2[];
}

export interface FootprintTemplate {
  name: string;
  pads: import("@vcad/ir").Pad[];
  graphics: import("@vcad/ir").FootprintGraphic[];
}

export interface SymbolDef {
  id: string;
  name: string;
  prefix: string;
  defaultValue: string;
  pins: import("@vcad/ir").SchematicPin[];
  graphics: SymbolGraphic[];
  footprintTemplate: FootprintTemplate | null;
}

/** Get all builtin symbol definitions from the Rust library. */
export async function builtinSymbols(): Promise<SymbolDef[]> {
  const wasm = await loadEcadWasm();
  if (!wasm) return [];
  try {
    return wasm.ecadBuiltinSymbols() as SymbolDef[];
  } catch (e) {
    console.warn("[ECAD] builtinSymbols failed:", e);
    return [];
  }
}

/** Look up a single builtin symbol by ID. */
export async function getSymbol(id: string): Promise<SymbolDef | null> {
  const wasm = await loadEcadWasm();
  if (!wasm) return null;
  try {
    return (wasm.ecadGetSymbol(id) as SymbolDef) ?? null;
  } catch (e) {
    console.warn("[ECAD] getSymbol failed:", e);
    return null;
  }
}

// ---------------------------------------------------------------------------
// Ratsnest + PCB geometry
// ---------------------------------------------------------------------------

export interface RatsnestLine {
  net: string;
  from: Vec2;
  to: Vec2;
  fp_ref: string;
  pad_num: string;
}

/** Compute ratsnest lines for unrouted net connections. */
export async function computeRatsnest(
  pcb: Pcb,
  netlist: NetlistResult,
): Promise<RatsnestLine[]> {
  const wasm = await loadEcadWasm();
  if (!wasm) return [];
  try {
    return wasm.ecadComputeRatsnest(
      JSON.stringify(pcb),
      JSON.stringify(netlist),
    ) as RatsnestLine[];
  } catch (e) {
    console.warn("[ECAD] computeRatsnest failed:", e);
    return [];
  }
}

/** Get Z position for a PCB layer. */
export async function layerZ(
  layer: PcbLayer,
  thickness: number,
  explosion = 0,
): Promise<number> {
  const wasm = await loadEcadWasm();
  if (!wasm) return 0;
  try {
    return wasm.ecadLayerZ(layer, thickness, explosion) as number;
  } catch (e) {
    return 0;
  }
}

// ---------------------------------------------------------------------------
// Component 3D meshes
// ---------------------------------------------------------------------------

export interface ComponentMesh {
  footprint_ref: string;
  positions: number[];
  indices: number[];
  normals: number[];
  color: [number, number, number];
  metalness: number;
}

/** Generate 3D component body meshes for all footprints on a PCB. */
export async function componentMeshes(pcb: Pcb): Promise<ComponentMesh[]> {
  const wasm = await loadEcadWasm();
  if (!wasm) return [];
  try {
    return wasm.ecadComponentMeshes(JSON.stringify(pcb)) as ComponentMesh[];
  } catch (e) {
    console.warn("[ECAD] componentMeshes failed:", e);
    return [];
  }
}

// ---------------------------------------------------------------------------
// Schematic geometry helpers
// ---------------------------------------------------------------------------

export interface SnapResult {
  position: Vec2;
  is_pin: boolean;
}

/** Snap a position to the nearest component pin or grid point. */
export async function snapToGridOrPin(
  pos: Vec2,
  components: import("@vcad/ir").SchematicComponent[],
  grid: number,
  threshold = 12,
): Promise<SnapResult> {
  const wasm = await loadEcadWasm();
  if (!wasm) {
    // Fallback: simple grid snap
    return {
      position: {
        x: Math.round(pos.x / grid) * grid,
        y: Math.round(pos.y / grid) * grid,
      },
      is_pin: false,
    };
  }
  try {
    return wasm.ecadSnapToGridOrPin(
      pos.x,
      pos.y,
      JSON.stringify(components),
      grid,
      threshold,
    ) as SnapResult;
  } catch (e) {
    console.warn("[ECAD] snapToGridOrPin failed:", e);
    return {
      position: {
        x: Math.round(pos.x / grid) * grid,
        y: Math.round(pos.y / grid) * grid,
      },
      is_pin: false,
    };
  }
}

/** Get the net for a wire based on endpoint proximity to component pins. */
export async function netForWire(
  wire: import("@vcad/ir").SchematicWire,
  netlist: NetlistResult,
  components: import("@vcad/ir").SchematicComponent[],
): Promise<string | null> {
  const wasm = await loadEcadWasm();
  if (!wasm) return null;
  try {
    return (wasm.ecadNetForWire(
      JSON.stringify(wire),
      JSON.stringify(netlist),
      JSON.stringify(components),
    ) as string) ?? null;
  } catch (e) {
    console.warn("[ECAD] netForWire failed:", e);
    return null;
  }
}
