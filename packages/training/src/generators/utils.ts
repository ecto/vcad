/**
 * Utility functions for part generators.
 */

import type { ParamRange } from "./types.js";

/** Random integer in range [min, max] inclusive. */
export function randInt(range: ParamRange): number {
  const { min, max, step = 1 } = range;
  const steps = Math.floor((max - min) / step);
  return min + Math.floor(Math.random() * (steps + 1)) * step;
}

/** Random float in range [min, max]. */
export function randFloat(range: ParamRange, decimals = 1): number {
  const { min, max } = range;
  const value = min + Math.random() * (max - min);
  const factor = Math.pow(10, decimals);
  return Math.round(value * factor) / factor;
}

/** Random choice from an array. */
export function randChoice<T>(choices: T[]): T {
  return choices[Math.floor(Math.random() * choices.length)];
}

/** Random boolean with optional probability of true. */
export function randBool(prob = 0.5): boolean {
  return Math.random() < prob;
}

/** Format a number for VCode (remove unnecessary decimals). */
export function fmt(n: number): string {
  // Round to avoid floating point issues
  const rounded = Math.round(n * 1000) / 1000;
  // Convert to string and remove trailing zeros after decimal
  const str = rounded.toString();
  if (str.includes(".")) {
    return str.replace(/\.?0+$/, "");
  }
  return str;
}

/**
 * Compute positions for a hole pattern.
 */
export interface HolePosition {
  x: number;
  y: number;
}

/** Compute corner hole positions for a rectangular shape. */
export function cornerHoles(
  width: number,
  depth: number,
  inset: number,
): HolePosition[] {
  return [
    { x: inset, y: inset },
    { x: width - inset, y: inset },
    { x: width - inset, y: depth - inset },
    { x: inset, y: depth - inset },
  ];
}

/** Compute edge hole positions (midpoints of edges). */
export function edgeHoles(
  width: number,
  depth: number,
  inset: number,
): HolePosition[] {
  return [
    { x: width / 2, y: inset },
    { x: width - inset, y: depth / 2 },
    { x: width / 2, y: depth - inset },
    { x: inset, y: depth / 2 },
  ];
}

/** Compute grid hole positions. */
export function gridHoles(
  width: number,
  depth: number,
  inset: number,
  cols: number,
  rows: number,
): HolePosition[] {
  const holes: HolePosition[] = [];
  const xSpacing = (width - 2 * inset) / (cols - 1);
  const ySpacing = (depth - 2 * inset) / (rows - 1);

  for (let i = 0; i < cols; i++) {
    for (let j = 0; j < rows; j++) {
      holes.push({
        x: inset + i * xSpacing,
        y: inset + j * ySpacing,
      });
    }
  }

  return holes;
}

/** Compute circular bolt pattern positions. */
export function circularHoles(
  centerX: number,
  centerY: number,
  radius: number,
  count: number,
  startAngle = 0,
): HolePosition[] {
  const holes: HolePosition[] = [];
  const angleStep = (2 * Math.PI) / count;

  for (let i = 0; i < count; i++) {
    const angle = startAngle + i * angleStep;
    holes.push({
      x: centerX + radius * Math.cos(angle),
      y: centerY + radius * Math.sin(angle),
    });
  }

  return holes;
}

/** Compute center hole position. */
export function centerHole(width: number, depth: number): HolePosition[] {
  return [{ x: width / 2, y: depth / 2 }];
}
