#!/usr/bin/env node
/**
 * The predicted-frequency table — the receipt's tuning claims, standalone.
 *
 * Pure math, no engine or build needed:
 *   node examples/glockenspiel/frequencies.mjs
 *
 * Prints note, target pitch, as-modeled bar length, nodal hole positions,
 * the frequency the closed-form beam model predicts for that length, and
 * the rounding error in cents. Anyone with a phone spectrogram can check
 * the "predicted" column against the physical bars.
 *
 * Material constants (E, ρ for 6061-T6) mirror the kernel registry in
 * crates/vcad-kernel-sheet/src/materials.rs; build.mjs asserts the two
 * agree at run time.
 */

import { BAR, BAR_MATERIAL, barConstantSI, barSpecs } from "./geometry.mjs";

const C = barConstantSI();
console.log(
  `free-free bar, ${BAR_MATERIAL.displayName}: E = ${BAR_MATERIAL.modulusGpa} GPa, ` +
    `ρ = ${BAR_MATERIAL.densityKgM3} kg/m³, t = ${BAR.thicknessMm} mm`,
);
console.log(
  `f₁ = ${C.toFixed(3)} / L²  (Hz, L in m) · nodal holes at 0.2242·L from each end\n`,
);

const header = ["note", "target Hz", "L (mm)", "holes @ (mm)", "predicted Hz", "err (¢)"];
const rows = barSpecs().map((b) => [
  b.note,
  b.targetHz.toFixed(2),
  b.lengthMm.toFixed(1),
  b.holeXsMm.map((x) => x.toFixed(2)).join(" / "),
  b.predictedHz.toFixed(2),
  b.errorCents.toFixed(2),
]);

const widths = header.map((h, i) =>
  Math.max(h.length, ...rows.map((r) => r[i].length)),
);
const fmt = (r) => r.map((c, i) => c.padStart(widths[i])).join("  ");
console.log(fmt(header));
console.log(widths.map((w) => "─".repeat(w)).join("  "));
for (const r of rows) console.log(fmt(r));
