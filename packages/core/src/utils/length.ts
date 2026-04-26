/**
 * Length unit conversion + formatting.
 *
 * The kernel works in millimeters everywhere. The UI lets the user view and
 * type values in mm, cm, or inches. Conversion happens at the display
 * boundary; nothing else in the codebase needs to know about units.
 */

export type LengthUnit = "mm" | "cm" | "in";

export const LENGTH_UNITS: LengthUnit[] = ["mm", "cm", "in"];

/** Display label rendered in chips and inputs. */
export const UNIT_LABEL: Record<LengthUnit, string> = {
  mm: "mm",
  cm: "cm",
  in: "in",
};

const MM_PER_UNIT: Record<LengthUnit, number> = {
  mm: 1,
  cm: 10,
  in: 25.4,
};

/** Convert a kernel-space (mm) value to the chosen display unit. */
export function fromMm(mm: number, unit: LengthUnit): number {
  return mm / MM_PER_UNIT[unit];
}

/** Convert a typed display-unit value back to kernel-space mm. */
export function toMm(value: number, unit: LengthUnit): number {
  return value * MM_PER_UNIT[unit];
}

/**
 * Step to the next unit in the cycle. Used by the click-to-cycle unit chip.
 * Order matches LENGTH_UNITS.
 */
export function nextUnit(unit: LengthUnit): LengthUnit {
  const i = LENGTH_UNITS.indexOf(unit);
  return LENGTH_UNITS[(i + 1) % LENGTH_UNITS.length] ?? "mm";
}

/**
 * Format a millimeter value for display. Inches need an extra decimal to feel
 * legible at typical CAD scales; mm/cm look right at 1 decimal.
 */
export function formatLength(mm: number, unit: LengthUnit, opts?: { decimals?: number }): string {
  const decimals = opts?.decimals ?? (unit === "in" ? 2 : 1);
  return fromMm(mm, unit).toFixed(decimals);
}
