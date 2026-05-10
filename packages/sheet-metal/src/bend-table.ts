/**
 * Bend tables: queryable `(material, t, R) → (K, BA)`.
 *
 * Replaces the "single global K-factor lie" with structured, provenanced
 * lookup. Every {@link import("./model.js").Bend} carries a
 * {@link KFactorSource} pointing back to the table row that produced its
 * allowance.
 */

/**
 * Where a K-factor was sourced from. Drives the colored provenance dot in
 * the property panel.
 */
export type KFactorSource =
  | { kind: "Builtin"; key: string }
  | { kind: "Shop"; shopId: string; key: string }
  | { kind: "Measured"; note: string }
  | { kind: "Manual" };

export function kFactorSourceLabel(s: KFactorSource): string {
  switch (s.kind) {
    case "Builtin":
      return `builtin:${s.key}`;
    case "Shop":
      return `shop:${s.shopId}/${s.key}`;
    case "Measured":
      return `measured:${s.note}`;
    case "Manual":
      return "manual";
  }
}

export interface BendTableRow {
  material: string;
  /** Material thickness (mm). */
  thickness: number;
  /** Inside bend radius (mm). */
  radius: number;
  kFactor: number;
}

export function rowRoverT(r: BendTableRow): number {
  return r.radius / r.thickness;
}

export interface BendTable {
  id: string;
  rows: BendTableRow[];
}

/**
 * Curated default table (Machinery's Handbook + DIN 6935 typical values).
 * Real shops calibrate against measured coupons and contribute back to the
 * open registry.
 */
export function builtinBendTable(): BendTable {
  return {
    id: "builtin",
    rows: [
      // Aluminum (soft, e.g. 1100, 3003)
      row("Al-soft", 1.0, 0.5, 0.33),
      row("Al-soft", 1.0, 1.0, 0.35),
      row("Al-soft", 1.0, 2.0, 0.37),
      row("Al-soft", 1.0, 3.0, 0.38),
      row("Al-soft", 1.5, 1.5, 0.35),
      row("Al-soft", 2.0, 2.0, 0.36),
      // Aluminum (hard, e.g. 6061-T6)
      row("Al-hard", 1.0, 1.0, 0.4),
      row("Al-hard", 1.0, 2.0, 0.42),
      row("Al-hard", 1.5, 1.5, 0.41),
      row("Al-hard", 2.0, 3.0, 0.44),
      // Mild steel (CRS, A36)
      row("Steel-mild", 1.0, 1.0, 0.4),
      row("Steel-mild", 1.0, 2.0, 0.43),
      row("Steel-mild", 1.5, 1.5, 0.42),
      row("Steel-mild", 2.0, 2.0, 0.44),
      row("Steel-mild", 3.0, 3.0, 0.45),
      // Stainless 304
      row("SS-304", 1.0, 1.0, 0.44),
      row("SS-304", 1.0, 2.0, 0.47),
      row("SS-304", 1.5, 1.5, 0.45),
      row("SS-304", 2.0, 2.0, 0.47),
    ],
  };
}

function row(
  material: string,
  thickness: number,
  radius: number,
  kFactor: number,
): BendTableRow {
  return { material, thickness, radius, kFactor };
}

/**
 * Look up the K-factor for `(material, thickness, radius)`. Falls back to
 * the closest row by `R/t` for that material when no exact match exists;
 * returns `null` if the material is unknown.
 */
export function lookupKFactor(
  table: BendTable,
  material: string,
  thickness: number,
  radius: number,
): { kFactor: number; source: KFactorSource } | null {
  const targetRt = radius / thickness;
  let best: { row: BendTableRow; dist: number } | null = null;
  for (const r of table.rows) {
    if (r.material !== material) continue;
    const dist =
      Math.abs(rowRoverT(r) - targetRt) +
      Math.abs(r.thickness - thickness) * 0.1;
    if (best === null || dist < best.dist) {
      best = { row: r, dist };
    }
  }
  if (best === null) return null;
  const key = `${best.row.material}/R${best.row.radius.toFixed(2)}t${best.row.thickness.toFixed(2)}`;
  return {
    kFactor: best.row.kFactor,
    source: { kind: "Builtin", key },
  };
}

/**
 * `BA = θ · (R + K · t)`. The sign of `θ` is ignored.
 */
export function bendAllowance(
  angleRad: number,
  radius: number,
  kFactor: number,
  thickness: number,
): number {
  return Math.abs(angleRad) * (radius + kFactor * thickness);
}

/**
 * `BD = 2(R + t) · tan(θ/2) - BA`.
 */
export function bendDeduction(
  angleRad: number,
  radius: number,
  kFactor: number,
  thickness: number,
): number {
  const ba = bendAllowance(angleRad, radius, kFactor, thickness);
  return 2 * (radius + thickness) * Math.tan(Math.abs(angleRad) / 2) - ba;
}
