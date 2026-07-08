#!/usr/bin/env node
/**
 * Build the calibration coupon: evaluate the geometry, snapshot the
 * pre-print prediction (predict_print), and ship everything a print
 * session needs — the STL, the prediction, and the measurement worksheet.
 *
 * Run from the repo root after building the workspace:
 *   npm run build --workspaces
 *   node examples/calibration-coupon/build.mjs
 *
 * Outputs land in examples/calibration-coupon/out/:
 *   coupon.stl                  — slice this, print it (100% infill!)
 *   coupon.vcad                 — editable parametric source
 *   prediction.json             — what the design claims (predict_print)
 *   measurements.template.json  — the guided worksheet: copy to
 *                                 measurements.json, fill with calipers +
 *                                 scale, then run record.mjs
 */

import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { mkdirSync, writeFileSync } from "node:fs";

import { Engine } from "@vcad/engine";
import { openDocument } from "../../packages/mcp/dist/tools/session.js";
import { predictPrint } from "../../packages/mcp/dist/tools/print-check.js";
import { exportCad } from "../../packages/mcp/dist/tools/export.js";

import { couponDocument, measurables, PARAMS } from "./geometry.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const OUT = join(HERE, "out");
mkdirSync(OUT, { recursive: true });
process.env.VCAD_MCP_EXPORT_DIR = OUT;

const parse = (r) => JSON.parse(r.content[0].text);
const log = (...a) => console.log(...a);
const rule = () => log("─".repeat(64));

async function main() {
  const engine = await Engine.init();

  rule();
  log("  vcad calibration coupon — print-then-measure, zero vendors");
  rule();

  // 1. The part.
  const doc = couponDocument();
  const { document_id } = parse(openDocument({ initial: doc }));
  log(`\n[1] Coupon: ${PARAMS.base.x}×${PARAMS.base.y}mm plate — ` +
    `${PARAMS.stepHeights.length} Z steps, Ø${PARAMS.holes.map((h) => h.d).join("/Ø")} holes, ` +
    `Ø${PARAMS.boss.d} boss, ${PARAMS.fins.map((f) => f.t).join("/")}mm fins`);

  // 2. The prediction — what the design claims, before any plastic moves.
  const prediction = parse(
    predictPrint(
      {
        document_id,
        material_density_kg_m3: PARAMS.density_kg_m3,
        material_name: "PLA",
        measurables: measurables(),
      },
      engine,
    ),
  );
  log(`\n[2] Prediction: ${prediction.measurables.length} measurables — ` +
    `bbox ${prediction.bbox_mm.x}×${prediction.bbox_mm.y}×${prediction.bbox_mm.z}mm, ` +
    `${prediction.volume_mm3.toFixed(0)}mm³, ` +
    `${prediction.measurables.find((m) => m.id === "mass")?.predicted.toFixed(1)}g solid PLA`);
  for (const a of prediction.assumptions) log(`    · ${a}`);

  // 3. Ship: STL for the slicer, source, prediction, worksheet.
  const stl = parse(exportCad({ ir: doc, filename: "coupon.stl" }, engine));
  writeFileSync(join(OUT, "coupon.vcad"), JSON.stringify(doc, null, 2));
  writeFileSync(join(OUT, "prediction.json"), JSON.stringify(prediction, null, 2));

  const worksheet = {
    printer: "",
    material: "",
    process: "",
    guide: Object.fromEntries(
      prediction.measurables.map((m) => [
        m.id,
        `${m.label} — predicted ${m.predicted}${m.unit}`,
      ]),
    ),
    measurements: Object.fromEntries(prediction.measurables.map((m) => [m.id, null])),
  };
  writeFileSync(
    join(OUT, "measurements.template.json"),
    JSON.stringify(worksheet, null, 2),
  );

  log(`\n[3] Shipped → examples/calibration-coupon/out/`);
  log(`    coupon.stl (${(stl.bytes / 1024).toFixed(0)} KB) — print at 100% infill`);
  log(`    prediction.json + measurements.template.json`);
  rule();
  log("  Next: print coupon.stl, then");
  log("    cp out/measurements.template.json out/measurements.json");
  log("    (fill it in with calipers + scale)");
  log("    node examples/calibration-coupon/record.mjs");
  rule();
}

main().catch((e) => {
  console.error("FAILED:", e);
  process.exit(1);
});
