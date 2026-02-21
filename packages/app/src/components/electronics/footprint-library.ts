/**
 * Parametric footprint generators based on IPC-7351B standards.
 *
 * Each generator returns a FootprintTemplate with pads and silkscreen graphics.
 */

import type { Pad, FootprintGraphic, PcbLayer } from "@vcad/ir";
import type { FootprintTemplate } from "./symbol-library";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function smdPad(
  num: string, x: number, y: number, w: number, h: number, layer: PcbLayer = "FCu",
): Pad {
  return {
    number: num, padType: "SMD",
    shape: { type: "Rect", width: w, height: h },
    position: { x, y }, layers: [layer],
  };
}

function thtPad(
  num: string, x: number, y: number, padDia: number, drillDia: number,
): Pad {
  return {
    number: num, padType: "THT",
    shape: { type: "Circle", diameter: padDia },
    position: { x, y },
    drill: { diameter: drillDia },
    layers: ["FCu" as const, "BCu" as const],
  };
}

function silkLine(x1: number, y1: number, x2: number, y2: number): FootprintGraphic {
  return { type: "Line", start: { x: x1, y: y1 }, end: { x: x2, y: y2 }, width: 0.12, layer: "FSilkS" };
}

function silkRect(x1: number, y1: number, x2: number, y2: number): FootprintGraphic[] {
  return [
    silkLine(x1, y1, x2, y1),
    silkLine(x2, y1, x2, y2),
    silkLine(x2, y2, x1, y2),
    silkLine(x1, y2, x1, y1),
  ];
}

// ---------------------------------------------------------------------------
// Chip resistor/capacitor footprints (2-pad)
// ---------------------------------------------------------------------------

type ChipSize = "0402" | "0603" | "0805" | "1206";

const CHIP_PARAMS: Record<ChipSize, { padW: number; padH: number; gap: number; silkY: number }> = {
  "0402": { padW: 0.5, padH: 0.5, gap: 0.5, silkY: 0.35 },
  "0603": { padW: 0.8, padH: 0.9, gap: 0.8, silkY: 0.55 },
  "0805": { padW: 1.0, padH: 1.2, gap: 1.0, silkY: 0.7 },
  "1206": { padW: 1.6, padH: 1.8, gap: 1.0, silkY: 1.0 },
};

export function fpChip(size: ChipSize): FootprintTemplate {
  const p = CHIP_PARAMS[size];
  const cx = (p.gap + p.padW) / 2;
  return {
    name: size,
    pads: [smdPad("1", -cx, 0, p.padW, p.padH), smdPad("2", cx, 0, p.padW, p.padH)],
    graphics: [silkLine(-p.gap / 2, -p.silkY, p.gap / 2, -p.silkY), silkLine(-p.gap / 2, p.silkY, p.gap / 2, p.silkY)],
  };
}

// ---------------------------------------------------------------------------
// SOIC (Small Outline IC)
// ---------------------------------------------------------------------------

export function fpSOIC(pins: 8 | 14 | 16): FootprintTemplate {
  const pitch = 1.27;
  const bodyW = 3.9; // nominal body width (pad-to-pad center 5.4mm for SOIC-wide)
  const padW = 0.6;
  const padH = 2.2;
  const rowX = 2.7; // half distance between rows
  const half = pins / 2;
  const pads: Pad[] = [];

  for (let i = 0; i < half; i++) {
    const y = (i - (half - 1) / 2) * pitch;
    pads.push(smdPad(String(i + 1), -rowX, y, padH, padW));
    pads.push(smdPad(String(pins - i), rowX, y, padH, padW));
  }

  const halfLen = (half * pitch) / 2;
  const graphics = [
    ...silkRect(-bodyW / 2, -halfLen, bodyW / 2, halfLen),
    // Pin 1 dot
    { type: "Circle" as const, center: { x: -bodyW / 2 + 0.5, y: -halfLen + 0.5 }, radius: 0.25, width: 0.12, layer: "FSilkS" as const },
  ];

  return { name: `SOIC-${pins}`, pads, graphics };
}

// ---------------------------------------------------------------------------
// QFP (Quad Flat Package)
// ---------------------------------------------------------------------------

export function fpQFP(pins: 32 | 48 | 64, pitch = 0.8): FootprintTemplate {
  const pinsPerSide = pins / 4;
  const bodySize = pins === 32 ? 7 : pins === 48 ? 9 : 12;
  const padW = 0.4;
  const padH = 1.5;
  const rowOffset = bodySize / 2 + padH / 2 - 0.3;
  const pads: Pad[] = [];
  let num = 1;

  // Bottom side (pins go left to right)
  for (let i = 0; i < pinsPerSide; i++) {
    const x = (i - (pinsPerSide - 1) / 2) * pitch;
    pads.push(smdPad(String(num++), x, rowOffset, padW, padH));
  }
  // Right side (pins go bottom to top)
  for (let i = 0; i < pinsPerSide; i++) {
    const y = ((pinsPerSide - 1) / 2 - i) * pitch;
    pads.push(smdPad(String(num++), rowOffset, y, padH, padW));
  }
  // Top side (pins go right to left)
  for (let i = 0; i < pinsPerSide; i++) {
    const x = ((pinsPerSide - 1) / 2 - i) * pitch;
    pads.push(smdPad(String(num++), x, -rowOffset, padW, padH));
  }
  // Left side (pins go top to bottom)
  for (let i = 0; i < pinsPerSide; i++) {
    const y = (i - (pinsPerSide - 1) / 2) * pitch;
    pads.push(smdPad(String(num++), -rowOffset, y, padH, padW));
  }

  const hs = bodySize / 2;
  const graphics = [
    ...silkRect(-hs, -hs, hs, hs),
    { type: "Circle" as const, center: { x: -hs + 0.8, y: hs - 0.8 }, radius: 0.3, width: 0.12, layer: "FSilkS" as const },
  ];

  return { name: `QFP-${pins}`, pads, graphics };
}

// ---------------------------------------------------------------------------
// DIP (Dual In-Line Package)
// ---------------------------------------------------------------------------

export function fpDIP(pins: 8 | 14 | 16 | 28 | 40): FootprintTemplate {
  const pitch = 2.54;
  const rowSpacing = 7.62; // 300 mil
  const padDia = 1.6;
  const drillDia = 0.8;
  const half = pins / 2;
  const pads: Pad[] = [];

  for (let i = 0; i < half; i++) {
    const y = (i - (half - 1) / 2) * pitch;
    pads.push(thtPad(String(i + 1), -rowSpacing / 2, y, padDia, drillDia));
    pads.push(thtPad(String(pins - i), rowSpacing / 2, y, padDia, drillDia));
  }

  const halfLen = (half * pitch) / 2 + 0.5;
  const halfW = rowSpacing / 2 + 1.5;
  const graphics = [
    ...silkRect(-halfW, -halfLen, halfW, halfLen),
    { type: "Circle" as const, center: { x: -rowSpacing / 2, y: -halfLen + 1 }, radius: 0.5, width: 0.12, layer: "FSilkS" as const },
  ];

  return { name: `DIP-${pins}`, pads, graphics };
}

// ---------------------------------------------------------------------------
// SOT-23 (3-pin)
// ---------------------------------------------------------------------------

export function fpSOT23(): FootprintTemplate {
  return {
    name: "SOT-23",
    pads: [
      smdPad("1", -0.95, 1.1, 0.6, 0.7),
      smdPad("2", 0.95, 1.1, 0.6, 0.7),
      smdPad("3", 0, -1.1, 0.6, 0.7),
    ],
    graphics: [
      silkLine(-1.3, -0.6, 1.3, -0.6),
      silkLine(-1.3, 0.6, -1.3, -0.6),
      silkLine(1.3, 0.6, 1.3, -0.6),
    ],
  };
}

// ---------------------------------------------------------------------------
// SOT-223 (4-pin with thermal tab)
// ---------------------------------------------------------------------------

export function fpSOT223(): FootprintTemplate {
  const pitch = 2.3;
  return {
    name: "SOT-223",
    pads: [
      smdPad("1", -pitch, 3.15, 0.7, 1.5),
      smdPad("2", 0, 3.15, 0.7, 1.5),
      smdPad("3", pitch, 3.15, 0.7, 1.5),
      // Large thermal tab (top side)
      smdPad("4", 0, -3.15, 3.5, 1.5),
    ],
    graphics: [
      ...silkRect(-3.3, -2.3, 3.3, 2.3),
    ],
  };
}

// ---------------------------------------------------------------------------
// Pin Header
// ---------------------------------------------------------------------------

export function fpPinHeader(rows: 1 | 2, cols: number): FootprintTemplate {
  const pitch = 2.54;
  const padDia = 2.5;
  const drillDia = 1.0;
  const pads: Pad[] = [];
  let num = 1;

  for (let c = 0; c < cols; c++) {
    for (let r = 0; r < rows; r++) {
      const x = rows === 1 ? 0 : (r - 0.5) * pitch;
      const y = (c - (cols - 1) / 2) * pitch;
      pads.push(thtPad(String(num++), x, y, padDia, drillDia));
    }
  }

  const halfW = rows === 1 ? 1.5 : pitch;
  const halfH = ((cols - 1) * pitch) / 2 + 1.5;
  const graphics = silkRect(-halfW, -halfH, halfW, halfH);

  return { name: `PinHeader_${rows}x${cols}`, pads, graphics };
}
