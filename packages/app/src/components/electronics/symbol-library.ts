/**
 * Symbol library — backed by Rust WASM (canonical), with sync API for React.
 *
 * On module load, eagerly requests symbols from the Rust kernel via WASM.
 * Until the WASM module is ready, returns an empty library (no flash of content).
 * Callers that render SYMBOL_LIBRARY should re-render when ECAD features init.
 */

import { useSyncExternalStore } from "react";
import type { SchematicPin, Pad, FootprintGraphic, PcbLayer } from "@vcad/ir";
import { builtinSymbols as wasmBuiltinSymbols } from "@vcad/engine";

// ---------------------------------------------------------------------------
// Types (re-exported for consumers)
// ---------------------------------------------------------------------------

export interface SymbolGraphic {
  type: "rect" | "line" | "circle" | "polyline";
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
  points?: { x: number; y: number }[];
}

export interface FootprintTemplate {
  name: string;
  pads: Pad[];
  graphics: FootprintGraphic[];
}

export interface SymbolDef {
  id: string;
  name: string;
  prefix: string;
  defaultValue: string;
  pins: SchematicPin[];
  graphics: SymbolGraphic[];
  footprintTemplate: FootprintTemplate | null;
}

// ---------------------------------------------------------------------------
// Cached library (populated from WASM)
// ---------------------------------------------------------------------------

let _cachedLibrary: SymbolDef[] | null = null;
let _initPromise: Promise<void> | null = null;

// React subscribers (via useSymbolLibrary) so palettes re-render once the full
// WASM symbol set replaces the hardcoded fallback.
const _listeners = new Set<() => void>();
function _notify(): void {
  for (const l of _listeners) l();
}

/** Initialize the symbol library from WASM. Call early in app startup.
 *
 * Retries: this runs at module import, before the kernel WASM has finished
 * instantiating, so the first call(s) come back empty. Poll until the WASM is
 * ready (or give up after a few seconds and keep the hardcoded fallback), then
 * notify React subscribers so the palettes swap to the full builtin set. */
export function initSymbolLibrary(): Promise<void> {
  if (!_initPromise) {
    _initPromise = (async () => {
      for (let attempt = 0; attempt < 40; attempt++) {
        try {
          const symbols = await wasmBuiltinSymbols();
          if (symbols.length > 0) {
            _cachedLibrary = symbols as unknown as SymbolDef[];
            _notify();
            return;
          }
        } catch {
          // WASM not ready / unavailable — retry below.
        }
        await new Promise((r) => setTimeout(r, 200));
      }
    })();
  }
  return _initPromise!;
}

// Eagerly start loading
initSymbolLibrary();

// ---------------------------------------------------------------------------
// Hardcoded fallback (used until WASM loads, or if WASM is unavailable)
// ---------------------------------------------------------------------------

function smdPad(
  num: string, x: number, y: number, w: number, h: number, layer: PcbLayer = "FCu",
): Pad {
  return { number: num, padType: "SMD", shape: { type: "Rect", width: w, height: h }, position: { x, y }, layers: [layer] };
}

function silkLine(x1: number, y1: number, x2: number, y2: number): FootprintGraphic {
  return { type: "Line", start: { x: x1, y: y1 }, end: { x: x2, y: y2 }, width: 0.12, layer: "FSilkS" };
}

const FALLBACK_LIBRARY: SymbolDef[] = [
  {
    id: "resistor", name: "Resistor", prefix: "R", defaultValue: "10k",
    pins: [
      { number: "1", name: "1", pin_type: "Passive", position: { x: -8, y: 15 } },
      { number: "2", name: "2", pin_type: "Passive", position: { x: 48, y: 15 } },
    ],
    graphics: [
      { type: "rect", x: 5, y: 5, width: 30, height: 20 },
      { type: "line", x1: -8, y1: 15, x2: 5, y2: 15 },
      { type: "line", x1: 35, y1: 15, x2: 48, y2: 15 },
    ],
    footprintTemplate: {
      name: "0805",
      pads: [smdPad("1", -1.0, 0, 1.0, 1.2), smdPad("2", 1.0, 0, 1.0, 1.2)],
      graphics: [silkLine(-0.5, -0.7, 0.5, -0.7), silkLine(-0.5, 0.7, 0.5, 0.7)],
    },
  },
  {
    id: "capacitor", name: "Capacitor", prefix: "C", defaultValue: "100nF",
    pins: [
      { number: "1", name: "1", pin_type: "Passive", position: { x: -8, y: 15 } },
      { number: "2", name: "2", pin_type: "Passive", position: { x: 48, y: 15 } },
    ],
    graphics: [
      { type: "line", x1: -8, y1: 15, x2: 16, y2: 15 },
      { type: "line", x1: 16, y1: 3, x2: 16, y2: 27 },
      { type: "line", x1: 24, y1: 3, x2: 24, y2: 27 },
      { type: "line", x1: 24, y1: 15, x2: 48, y2: 15 },
    ],
    footprintTemplate: {
      name: "0805",
      pads: [smdPad("1", -1.0, 0, 1.0, 1.2), smdPad("2", 1.0, 0, 1.0, 1.2)],
      graphics: [silkLine(-0.5, -0.7, 0.5, -0.7), silkLine(-0.5, 0.7, 0.5, 0.7)],
    },
  },
  {
    id: "vcc", name: "VCC", prefix: "PWR", defaultValue: "VCC",
    pins: [{ number: "1", name: "1", pin_type: "PowerOutput", position: { x: 20, y: 30 } }],
    graphics: [
      { type: "line", x1: 20, y1: 30, x2: 20, y2: 10 },
      { type: "polyline", points: [{ x: 10, y: 10 }, { x: 20, y: 0 }, { x: 30, y: 10 }] },
    ],
    footprintTemplate: null,
  },
  {
    id: "gnd", name: "GND", prefix: "PWR", defaultValue: "GND",
    pins: [{ number: "1", name: "1", pin_type: "PowerInput", position: { x: 20, y: 0 } }],
    graphics: [
      { type: "line", x1: 20, y1: 0, x2: 20, y2: 15 },
      { type: "line", x1: 8, y1: 15, x2: 32, y2: 15 },
      { type: "line", x1: 12, y1: 20, x2: 28, y2: 20 },
      { type: "line", x1: 16, y1: 25, x2: 24, y2: 25 },
    ],
    footprintTemplate: null,
  },
];

// ---------------------------------------------------------------------------
// Public API (sync — used by React components)
// ---------------------------------------------------------------------------

/** All builtin symbol definitions. Prefer WASM source when loaded. */
export const SYMBOL_LIBRARY: SymbolDef[] = new Proxy(FALLBACK_LIBRARY, {
  get(target, prop, receiver) {
    const source = _cachedLibrary ?? target;
    return Reflect.get(source, prop, receiver);
  },
});

/** Lookup a symbol by ID. */
export function getSymbol(id: string): SymbolDef | undefined {
  const source = _cachedLibrary ?? FALLBACK_LIBRARY;
  return source.find((s) => s.id === id);
}

/**
 * Reactive symbol library for React components. Re-renders when the full WASM
 * symbol set finishes loading (the static `SYMBOL_LIBRARY` proxy can't trigger
 * a re-render, so palettes that read it directly are stuck on the 4-symbol
 * fallback). The snapshot is referentially stable, so it's safe with
 * useSyncExternalStore.
 */
export function useSymbolLibrary(): SymbolDef[] {
  return useSyncExternalStore(
    (cb) => {
      _listeners.add(cb);
      return () => _listeners.delete(cb);
    },
    () => _cachedLibrary ?? FALLBACK_LIBRARY,
    () => _cachedLibrary ?? FALLBACK_LIBRARY,
  );
}
