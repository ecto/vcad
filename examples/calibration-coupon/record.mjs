#!/usr/bin/env node
/**
 * Record the as-built measurements of a printed coupon and emit the
 * receipt-vs-reality delta report.
 *
 * Usage (after build.mjs, a print, and a session with calipers + scale):
 *   cp out/measurements.template.json out/measurements.json
 *   $EDITOR out/measurements.json         # fill in the numbers you measured
 *   node examples/calibration-coupon/record.mjs [path/to/measurements.json]
 *
 * Reads out/prediction.json (from build.mjs), joins it with your numbers via
 * the same record_measurement tool the MCP server exposes, prints the delta
 * table, and writes out/calibration-report.json — the (predicted, measured)
 * pair, stored alongside the document.
 */

import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { readFileSync, writeFileSync } from "node:fs";

import { recordMeasurement } from "../../packages/mcp/dist/tools/print-check.js";

const HERE = dirname(fileURLToPath(import.meta.url));
const OUT = join(HERE, "out");

const measurementsPath = process.argv[2] ?? join(OUT, "measurements.json");
const prediction = JSON.parse(readFileSync(join(OUT, "prediction.json"), "utf8"));
const worksheet = JSON.parse(readFileSync(measurementsPath, "utf8"));

// Nulls are unmeasured rows — drop them; the report lists them as missing.
const measurements = Object.fromEntries(
  Object.entries(worksheet.measurements ?? {}).filter(
    ([, v]) => typeof v === "number" && Number.isFinite(v),
  ),
);
if (Object.keys(measurements).length === 0) {
  console.error(
    `No measurements filled in at ${measurementsPath} — ` +
      `copy out/measurements.template.json to out/measurements.json and add your numbers.`,
  );
  process.exit(1);
}

const report = JSON.parse(
  recordMeasurement({
    measurements,
    prediction,
    ...(worksheet.printer && { printer: worksheet.printer }),
    ...(worksheet.material && { material: worksheet.material }),
    ...(worksheet.process && { process: worksheet.process }),
  }).content[0].text,
);

writeFileSync(join(OUT, "calibration-report.json"), JSON.stringify(report, null, 2));

// ── Console delta table ───────────────────────────────────────────────────
const rule = () => console.log("─".repeat(72));
rule();
console.log("  calibration report — predicted vs measured");
rule();
const pad = (s, n) => String(s).padEnd(n);
console.log(
  pad("  id", 16) + pad("predicted", 11) + pad("measured", 11) + pad("delta", 10) + "verdict",
);
for (const r of report.rows) {
  const mark = r.within_tolerance ? "ok" : "OUT";
  console.log(
    pad(`  ${r.id}`, 16) +
      pad(`${r.predicted}${r.unit}`, 11) +
      pad(`${r.measured}${r.unit}`, 11) +
      pad(`${r.delta > 0 ? "+" : ""}${r.delta}`, 10) +
      mark,
  );
}
if (report.missing.length > 0) console.log(`  (unmeasured: ${report.missing.join(", ")})`);
rule();
for (const s of report.aggregates.axis_scales) {
  console.log(`  ${s.axis} scale: ${(s.scale * 100).toFixed(2)}% (n=${s.n})`);
}
if (report.aggregates.hole_offset_mm !== undefined) {
  console.log(`  hole offset: ${report.aggregates.hole_offset_mm}mm`);
}
if (report.aggregates.wall_offset_mm !== undefined) {
  console.log(`  wall offset: ${report.aggregates.wall_offset_mm}mm`);
}
for (const s of report.suggestions) console.log(`  → ${s}`);
rule();
console.log(`  ${report.verdict.toUpperCase()} — ${report.summary}`);
console.log(`  report: examples/calibration-coupon/out/calibration-report.json`);
rule();

process.exit(report.verdict === "fail" ? 1 : 0);
