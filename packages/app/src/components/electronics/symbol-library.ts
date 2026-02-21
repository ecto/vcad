/**
 * Hardcoded symbol library for schematic component placement.
 *
 * Each SymbolDef contains pin positions (on schematic grid),
 * SVG graphics for rendering, and a footprint template for auto-PCB placement.
 */

import type { SchematicPin, Pad, FootprintGraphic, PcbLayer } from "@vcad/ir";
import { fpChip, fpSOIC, fpSOT23, fpSOT223, fpQFP, fpDIP, fpPinHeader } from "./footprint-library";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export interface SymbolGraphic {
  type: "rect" | "line" | "circle" | "polyline";
  // rect
  x?: number;
  y?: number;
  width?: number;
  height?: number;
  // line
  x1?: number;
  y1?: number;
  x2?: number;
  y2?: number;
  // circle
  cx?: number;
  cy?: number;
  r?: number;
  // polyline
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
// Helper: SMD pad
// ---------------------------------------------------------------------------

function smdPad(
  num: string,
  x: number,
  y: number,
  w: number,
  h: number,
  layer: PcbLayer = "FCu",
): Pad {
  return {
    number: num,
    padType: "SMD",
    shape: { type: "Rect", width: w, height: h },
    position: { x, y },
    layers: [layer],
  };
}

function silkLine(
  x1: number,
  y1: number,
  x2: number,
  y2: number,
): FootprintGraphic {
  return {
    type: "Line",
    start: { x: x1, y: y1 },
    end: { x: x2, y: y2 },
    width: 0.12,
    layer: "FSilkS",
  };
}

// ---------------------------------------------------------------------------
// Library
// ---------------------------------------------------------------------------

export const SYMBOL_LIBRARY: SymbolDef[] = [
  // ---- Resistor ----
  {
    id: "resistor",
    name: "Resistor",
    prefix: "R",
    defaultValue: "10k",
    pins: [
      { number: "1", name: "1", pin_type: "Passive", position: { x: -8, y: 15 } },
      { number: "2", name: "2", pin_type: "Passive", position: { x: 48, y: 15 } },
    ],
    graphics: [
      { type: "rect", x: 5, y: 5, width: 30, height: 20 },
      { type: "line", x1: -8, y1: 15, x2: 5, y2: 15 },
      { type: "line", x1: 35, y1: 15, x2: 48, y2: 15 },
    ],
    footprintTemplate: fpChip("0805"),
  },

  // ---- Capacitor ----
  {
    id: "capacitor",
    name: "Capacitor",
    prefix: "C",
    defaultValue: "100nF",
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
    footprintTemplate: fpChip("0805"),
  },

  // ---- LED ----
  {
    id: "led",
    name: "LED",
    prefix: "D",
    defaultValue: "Red",
    pins: [
      { number: "A", name: "A", pin_type: "Passive", position: { x: -8, y: 15 } },
      { number: "K", name: "K", pin_type: "Passive", position: { x: 48, y: 15 } },
    ],
    graphics: [
      { type: "line", x1: -8, y1: 15, x2: 15, y2: 15 },
      { type: "polyline", points: [{ x: 15, y: 5 }, { x: 15, y: 25 }, { x: 30, y: 15 }, { x: 15, y: 5 }] },
      { type: "line", x1: 30, y1: 5, x2: 30, y2: 25 },
      { type: "line", x1: 30, y1: 15, x2: 48, y2: 15 },
      // Arrow indicators for LED
      { type: "line", x1: 25, y1: 3, x2: 30, y2: 0 },
      { type: "line", x1: 28, y1: 5, x2: 33, y2: 2 },
    ],
    footprintTemplate: fpChip("0805"),
  },

  // ---- Diode ----
  {
    id: "diode",
    name: "Diode",
    prefix: "D",
    defaultValue: "1N4148",
    pins: [
      { number: "A", name: "A", pin_type: "Passive", position: { x: -8, y: 15 } },
      { number: "K", name: "K", pin_type: "Passive", position: { x: 48, y: 15 } },
    ],
    graphics: [
      { type: "line", x1: -8, y1: 15, x2: 15, y2: 15 },
      { type: "polyline", points: [{ x: 15, y: 5 }, { x: 15, y: 25 }, { x: 30, y: 15 }, { x: 15, y: 5 }] },
      { type: "line", x1: 30, y1: 5, x2: 30, y2: 25 },
      { type: "line", x1: 30, y1: 15, x2: 48, y2: 15 },
    ],
    footprintTemplate: {
      name: "SOD-323",
      pads: [
        smdPad("A", -1.15, 0, 0.9, 0.6),
        smdPad("K", 1.15, 0, 0.9, 0.6),
      ],
      graphics: [
        silkLine(-0.5, -0.5, 0.5, -0.5),
        silkLine(-0.5, 0.5, 0.5, 0.5),
        silkLine(0.3, -0.5, 0.3, 0.5),
      ],
    },
  },

  // ---- NPN Transistor ----
  {
    id: "npn",
    name: "NPN Transistor",
    prefix: "Q",
    defaultValue: "2N2222",
    pins: [
      { number: "B", name: "B", pin_type: "Input", position: { x: -8, y: 15 } },
      { number: "C", name: "C", pin_type: "Output", position: { x: 35, y: 0 } },
      { number: "E", name: "E", pin_type: "Output", position: { x: 35, y: 30 } },
    ],
    graphics: [
      { type: "line", x1: -8, y1: 15, x2: 10, y2: 15 },
      { type: "line", x1: 10, y1: 5, x2: 10, y2: 25 },
      { type: "line", x1: 10, y1: 10, x2: 35, y2: 0 },
      { type: "line", x1: 10, y1: 20, x2: 35, y2: 30 },
      { type: "circle", cx: 18, cy: 15, r: 14 },
    ],
    footprintTemplate: fpSOT23(),
  },

  // ---- IC Header 8-pin ----
  {
    id: "ic8",
    name: "IC Header 8-pin",
    prefix: "U",
    defaultValue: "IC",
    pins: [
      { number: "1", name: "1", pin_type: "Bidirectional", position: { x: -8, y: 10 } },
      { number: "2", name: "2", pin_type: "Bidirectional", position: { x: -8, y: 24 } },
      { number: "3", name: "3", pin_type: "Bidirectional", position: { x: -8, y: 38 } },
      { number: "4", name: "4", pin_type: "Bidirectional", position: { x: -8, y: 52 } },
      { number: "5", name: "5", pin_type: "Bidirectional", position: { x: 48, y: 52 } },
      { number: "6", name: "6", pin_type: "Bidirectional", position: { x: 48, y: 38 } },
      { number: "7", name: "7", pin_type: "Bidirectional", position: { x: 48, y: 24 } },
      { number: "8", name: "8", pin_type: "Bidirectional", position: { x: 48, y: 10 } },
    ],
    graphics: [
      { type: "rect", x: 0, y: 0, width: 40, height: 62 },
      { type: "circle", cx: 6, cy: 4, r: 2 },
    ],
    footprintTemplate: fpDIP(8),
  },

  // ---- VCC Power Symbol ----
  {
    id: "vcc",
    name: "VCC",
    prefix: "PWR",
    defaultValue: "VCC",
    pins: [
      { number: "1", name: "1", pin_type: "PowerOutput", position: { x: 20, y: 30 } },
    ],
    graphics: [
      { type: "line", x1: 20, y1: 30, x2: 20, y2: 10 },
      { type: "polyline", points: [{ x: 10, y: 10 }, { x: 20, y: 0 }, { x: 30, y: 10 }] },
    ],
    footprintTemplate: null,
  },

  // ---- GND Power Symbol ----
  {
    id: "gnd",
    name: "GND",
    prefix: "PWR",
    defaultValue: "GND",
    pins: [
      { number: "1", name: "1", pin_type: "PowerInput", position: { x: 20, y: 0 } },
    ],
    graphics: [
      { type: "line", x1: 20, y1: 0, x2: 20, y2: 15 },
      { type: "line", x1: 8, y1: 15, x2: 32, y2: 15 },
      { type: "line", x1: 12, y1: 20, x2: 28, y2: 20 },
      { type: "line", x1: 16, y1: 25, x2: 24, y2: 25 },
    ],
    footprintTemplate: null,
  },

  // ---- Voltage Regulator (LDO) ----
  {
    id: "ldo",
    name: "Voltage Regulator (LDO)",
    prefix: "U",
    defaultValue: "AMS1117-3.3",
    pins: [
      { number: "1", name: "IN", pin_type: "PowerInput", position: { x: -8, y: 15 } },
      { number: "2", name: "GND", pin_type: "PowerInput", position: { x: 20, y: 40 } },
      { number: "3", name: "OUT", pin_type: "PowerOutput", position: { x: 48, y: 15 } },
    ],
    graphics: [
      { type: "rect", x: 0, y: 0, width: 40, height: 30 },
      { type: "line", x1: -8, y1: 15, x2: 0, y2: 15 },
      { type: "line", x1: 40, y1: 15, x2: 48, y2: 15 },
      { type: "line", x1: 20, y1: 30, x2: 20, y2: 40 },
    ],
    footprintTemplate: fpSOT223(),
  },

  // ---- Op-Amp ----
  {
    id: "opamp",
    name: "Op-Amp",
    prefix: "U",
    defaultValue: "LM358",
    pins: [
      { number: "2", name: "IN+", pin_type: "Input", position: { x: -8, y: 10 } },
      { number: "3", name: "IN-", pin_type: "Input", position: { x: -8, y: 24 } },
      { number: "1", name: "OUT", pin_type: "Output", position: { x: 48, y: 17 } },
      { number: "4", name: "VCC", pin_type: "PowerInput", position: { x: 20, y: -5 } },
      { number: "8", name: "GND", pin_type: "PowerInput", position: { x: 20, y: 40 } },
    ],
    graphics: [
      { type: "polyline", points: [{ x: 5, y: 0 }, { x: 5, y: 34 }, { x: 38, y: 17 }, { x: 5, y: 0 }] },
      { type: "line", x1: -8, y1: 10, x2: 5, y2: 10 },
      { type: "line", x1: -8, y1: 24, x2: 5, y2: 24 },
      { type: "line", x1: 38, y1: 17, x2: 48, y2: 17 },
    ],
    footprintTemplate: fpSOIC(8),
  },

  // ---- Microcontroller ----
  {
    id: "mcu32",
    name: "Microcontroller 32-pin",
    prefix: "U",
    defaultValue: "STM32F0",
    pins: (() => {
      const pins: SchematicPin[] = [];
      for (let i = 0; i < 8; i++) {
        pins.push({ number: String(i + 1), name: `P${i + 1}`, pin_type: "Bidirectional", position: { x: -8, y: 8 + i * 8 } });
        pins.push({ number: String(24 - i), name: `P${24 - i}`, pin_type: "Bidirectional", position: { x: 58, y: 8 + i * 8 } });
      }
      for (let i = 0; i < 8; i++) {
        pins.push({ number: String(9 + i), name: `P${9 + i}`, pin_type: "Bidirectional", position: { x: 12 + i * 5, y: 75 } });
        pins.push({ number: String(32 - i), name: `P${32 - i}`, pin_type: "Bidirectional", position: { x: 12 + i * 5, y: -5 } });
      }
      return pins;
    })(),
    graphics: [
      { type: "rect", x: 0, y: 0, width: 50, height: 70 },
      { type: "circle", cx: 6, cy: 4, r: 2 },
    ],
    footprintTemplate: fpQFP(32),
  },

  // ---- Resistor 0402 ----
  {
    id: "resistor_0402",
    name: "Resistor (0402)",
    prefix: "R",
    defaultValue: "10k",
    pins: [
      { number: "1", name: "1", pin_type: "Passive", position: { x: -8, y: 15 } },
      { number: "2", name: "2", pin_type: "Passive", position: { x: 48, y: 15 } },
    ],
    graphics: [
      { type: "rect", x: 5, y: 5, width: 30, height: 20 },
      { type: "line", x1: -8, y1: 15, x2: 5, y2: 15 },
      { type: "line", x1: 35, y1: 15, x2: 48, y2: 15 },
    ],
    footprintTemplate: fpChip("0402"),
  },

  // ---- Capacitor 0603 ----
  {
    id: "capacitor_0603",
    name: "Capacitor (0603)",
    prefix: "C",
    defaultValue: "100nF",
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
    footprintTemplate: fpChip("0603"),
  },

  // ---- Resistor 1206 ----
  {
    id: "resistor_1206",
    name: "Resistor (1206)",
    prefix: "R",
    defaultValue: "10k",
    pins: [
      { number: "1", name: "1", pin_type: "Passive", position: { x: -8, y: 15 } },
      { number: "2", name: "2", pin_type: "Passive", position: { x: 48, y: 15 } },
    ],
    graphics: [
      { type: "rect", x: 5, y: 5, width: 30, height: 20 },
      { type: "line", x1: -8, y1: 15, x2: 5, y2: 15 },
      { type: "line", x1: 35, y1: 15, x2: 48, y2: 15 },
    ],
    footprintTemplate: fpChip("1206"),
  },

  // ---- Pin Header 1x4 ----
  {
    id: "pinheader_1x4",
    name: "Pin Header 1x4",
    prefix: "J",
    defaultValue: "1x4",
    pins: [
      { number: "1", name: "1", pin_type: "Passive", position: { x: -8, y: 8 } },
      { number: "2", name: "2", pin_type: "Passive", position: { x: -8, y: 22 } },
      { number: "3", name: "3", pin_type: "Passive", position: { x: -8, y: 36 } },
      { number: "4", name: "4", pin_type: "Passive", position: { x: -8, y: 50 } },
    ],
    graphics: [
      { type: "rect", x: 0, y: 0, width: 20, height: 58 },
      { type: "line", x1: -8, y1: 8, x2: 0, y2: 8 },
      { type: "line", x1: -8, y1: 22, x2: 0, y2: 22 },
      { type: "line", x1: -8, y1: 36, x2: 0, y2: 36 },
      { type: "line", x1: -8, y1: 50, x2: 0, y2: 50 },
    ],
    footprintTemplate: fpPinHeader(1, 4),
  },

  // ---- Pin Header 2x5 ----
  {
    id: "pinheader_2x5",
    name: "Pin Header 2x5",
    prefix: "J",
    defaultValue: "2x5",
    pins: (() => {
      const pins: SchematicPin[] = [];
      for (let i = 0; i < 5; i++) {
        pins.push({ number: String(i * 2 + 1), name: String(i * 2 + 1), pin_type: "Passive", position: { x: -8, y: 8 + i * 14 } });
        pins.push({ number: String(i * 2 + 2), name: String(i * 2 + 2), pin_type: "Passive", position: { x: 48, y: 8 + i * 14 } });
      }
      return pins;
    })(),
    graphics: [
      { type: "rect", x: 0, y: 0, width: 40, height: 66 },
    ],
    footprintTemplate: fpPinHeader(2, 5),
  },
];

/** Lookup a symbol by ID. */
export function getSymbol(id: string): SymbolDef | undefined {
  return SYMBOL_LIBRARY.find((s) => s.id === id);
}
