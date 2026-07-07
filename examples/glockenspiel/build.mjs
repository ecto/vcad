#!/usr/bin/env node
/**
 * The receipt you can hear — a laser-cut glockenspiel through the full
 * sheet-metal pipeline (docs/plans/2026-07-06-scs-closed-loop-demo.md).
 *
 * Eight free-free 6061 bars tuned to C6–C7 major (lengths solved from the
 * closed-form beam model, cord holes on the fundamental's nodal lines) and
 * a folded 5052 U-channel stand, all checked against SendCutSend's
 * published capabilities and shipped as fab-ready DXF + STEP.
 *
 * Run from the repo root after building the workspace:
 *   npm run build --workspaces
 *   node examples/glockenspiel/build.mjs
 *
 * Outputs land in examples/glockenspiel/out/.
 */

import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { mkdirSync, writeFileSync } from "node:fs";
import assert from "node:assert/strict";

import { Engine } from "@vcad/engine";
import {
  sheetMetalCreate,
  sheetMetalCheck,
  sheetMetalUnfold,
  sheetMetalCost,
  sheetMetalSequence,
  sheetMetalSuggestFix,
} from "../../packages/mcp/dist/tools/sheet-metal.js";
import { exportCad } from "../../packages/mcp/dist/tools/export.js";
import { getSession } from "../../packages/mcp/dist/tools/session.js";
import { femHz, simulateStrike } from "../../packages/mcp/dist/tools/acoustics.js";

import {
  BAR,
  BAR_MATERIAL,
  STAND,
  STAND_MATERIAL,
  acousticBar,
  barConstantSI,
  barCreateArgs,
  barSpecs,
  compensateBarSpecs,
  standCreateArgs,
} from "./geometry.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const OUT = join(HERE, "out");
mkdirSync(OUT, { recursive: true });
process.env.VCAD_MCP_EXPORT_DIR = OUT;

const parse = (r) => JSON.parse(r.content[0].text);
const log = (...a) => console.log(...a);
const rule = () => log("─".repeat(64));
const deg = (rad) => (rad * 180) / Math.PI;

async function main() {
  const engine = await Engine.init();

  rule();
  log("  vcad → SendCutSend — C6–C7 glockenspiel, physics to flat metal");
  rule();

  // [0] Provenance: the E and ρ behind every tuning claim are the kernel's
  // own registry values, not numbers typed into this script.
  const registry = engine.getSheetMetalMaterials();
  const alHard = registry.find((m) => m.name === BAR_MATERIAL.registryName);
  assert(alHard, "al-hard missing from the kernel material registry");
  assert.equal(alHard.modulus_gpa, BAR_MATERIAL.modulusGpa);
  assert.equal(alHard.density_kg_m3, BAR_MATERIAL.densityKgM3);
  log(
    `\n[0] Material registry: ${alHard.display_name} — E = ${alHard.modulus_gpa} GPa, ` +
      `ρ = ${alHard.density_kg_m3} kg/m³ (matches geometry.mjs)`,
  );

  // [1] The tuning table — solved from f₁ = 3.5608·(t/L²)·sqrt(E/12ρ),
  // then hole-compensated: the FEM says the Ø4.2 nodal holes flatten every
  // bar ~5 cents, so the cut lengths come from the hole-aware model.
  const closedForm = barSpecs();
  // The plan's published closed-form table — a regression guard.
  assert.deepEqual(
    closedForm.map((b) => b.lengthMm),
    [125.6, 118.5, 111.9, 108.7, 102.6, 96.8, 91.4, 88.8],
  );
  const holeShift =
    1200 *
    Math.log2(
      femHz(acousticBar(closedForm[0].lengthMm), 1)[0] /
        closedForm[0].predictedHz,
    );
  const bars = compensateBarSpecs(femHz, closedForm);
  log(
    `\n[1] Bars: f₁·L² = ${barConstantSI().toFixed(3)} Hz·m² for ` +
      `${BAR.thicknessMm} mm ${BAR_MATERIAL.displayName}`,
  );
  log(
    `    nodal holes cost ${holeShift.toFixed(1)} ¢ (hole-aware FEM) — cut lengths compensated:`,
  );
  log("    note  target Hz   closed-form L  cut L (mm)  FEM Hz    err (¢)");
  for (const b of bars) {
    log(
      `    ${b.note.padEnd(4)}  ${b.targetHz.toFixed(2).padStart(9)}  ` +
        `${b.closedFormLengthMm.toFixed(1).padStart(13)}  ${b.lengthMm.toFixed(1).padStart(10)}  ` +
        `${b.predictedHz.toFixed(2).padStart(8)}  ${b.errorCents.toFixed(2).padStart(7)}`,
    );
    // Residual = 0.1 mm cut-grid rounding on the compensated length.
    assert(Math.abs(b.errorCents) < 2, `${b.note} error over 2 cents after compensation`);
  }

  // Explicit cut-feature guard: the DFM checker's bend rules are vacuous for
  // a flat part, so state the cutting constraint we rely on out loud.
  assert(
    BAR.holeDiaMm >= 0.5 * BAR.thicknessMm,
    "cord hole below SCS's ~50%-of-thickness minimum for aluminum",
  );

  // [2] Each bar through create → check(SCS) → unfold → DXF → cost.
  log(`\n[2] Bar pipeline vs. SendCutSend (6061 is cut-only there — no bends):`);
  const barReports = [];
  let totalMassKg = 0;
  let totalEachUsd = 0;
  for (const spec of bars) {
    const created = parse(sheetMetalCreate(barCreateArgs(spec), engine));
    const check = parse(
      sheetMetalCheck(
        { document_id: created.document_id, shop_profile: "sendcutsend" },
        engine,
      ),
    );
    assert.equal(check.error_count, 0, `${spec.note}: DFM errors`);
    assert.equal(check.warning_count, 0, `${spec.note}: DFM warnings`);
    const unfolded = parse(
      sheetMetalUnfold({ document_id: created.document_id }, engine),
    );
    const dxfName = `bar-${spec.note}.dxf`;
    writeFileSync(join(OUT, dxfName), unfolded.dxf);
    writeFileSync(
      join(OUT, `bar-${spec.note}.vcad`),
      JSON.stringify(getSession(created.document_id), null, 2),
    );
    const cost = parse(
      sheetMetalCost({ document_id: created.document_id, quantity: 1 }, engine),
    );
    totalMassKg += cost.breakdown.mass_kg_each;
    totalEachUsd += cost.breakdown.total_each;
    barReports.push({
      note: spec.note,
      target_hz: spec.targetHz,
      length_mm: spec.lengthMm,
      closed_form_length_mm: spec.closedFormLengthMm,
      width_mm: BAR.widthMm,
      thickness_mm: BAR.thicknessMm,
      hole_dia_mm: BAR.holeDiaMm,
      holes_from_end_mm: spec.holeFromEndMm,
      hole_xs_mm: spec.holeXsMm,
      predicted_hz: spec.predictedHz,
      error_cents: spec.errorCents,
      flat_bbox_mm: unfolded.flat_pattern.bbox,
      mass_kg: cost.breakdown.mass_kg_each,
      shop_ready: check.shop_ready,
      dxf: dxfName,
      document_id: created.document_id,
    });
    log(
      `    ${spec.note.padEnd(4)} ${spec.lengthMm.toFixed(1).padStart(6)} × ${BAR.widthMm} mm` +
        `  holes @ ${spec.holeXsMm.map((x) => x.toFixed(2)).join("/")} mm` +
        `  ${(cost.breakdown.mass_kg_each * 1000).toFixed(1).padStart(5)} g  → ${dxfName}` +
        `  ${check.shop_ready ? "✓ shop-ready" : "✗"}`,
    );
  }

  // [3] The receipt you can hear — literally. Strike each bar in
  // simulation: hole-aware modal analysis → mallet-excited synthesis →
  // WAV → FFT peak extraction → cents verdict. The gate before the order.
  log(`\n[3] Strike simulation (center strike, hard mallet, 44.1 kHz):`);
  for (const report of barReports) {
    const sim = JSON.parse(
      simulateStrike(
        {
          document_id: report.document_id,
          note: report.note,
          tolerance_cents: 5,
          wav_filename: `bar-${report.note}.wav`,
        },
        engine,
      ).content[0].text,
    );
    assert(sim.verdict.pass, `${report.note}: audio verdict failed (${sim.verdict.cents_error.toFixed(2)} ¢)`);
    report.audio = {
      f1_fem_hz: sim.physics.f1_fem_with_holes_hz,
      measured_hz: sim.verdict.measured_hz,
      cents_error: sim.verdict.cents_error,
      overtone_ratios: sim.physics.overtone_ratios.slice(0, 3),
      modes: sim.modes,
      wav: `bar-${report.note}.wav`,
    };
    const ring = sim.modes[0]?.t60_s ?? 0;
    log(
      `    ${report.note.padEnd(4)} strike → ${sim.verdict.measured_hz.toFixed(2).padStart(8)} Hz ` +
        `(${sim.verdict.cents_error >= 0 ? "+" : ""}${sim.verdict.cents_error.toFixed(2)} ¢)  ` +
        `rings ${ring.toFixed(1)} s  → bar-${report.note}.wav  ✓`,
    );
  }
  log(`    all 8 bars within ±5 ¢ of target — order-gate passed`);

  // [4] The stand — where the DFM loop earns its keep. First as naively
  // designed (no reliefs): the chamfered deck corners put material at the
  // wall-bend ends, and the checker catches the tear-out.
  log(`\n[4] Stand: ${STAND.lengthMm}×${STAND.widthMm} mm ${STAND_MATERIAL.displayName} deck,`);
  log(
    `    two ${STAND.wallMm} mm walls + ${STAND.footMm} mm feet, 16 cord holes on the nodal rows`,
  );
  const naive = parse(sheetMetalCreate(standCreateArgs(false, bars), engine));
  const naiveCheck = parse(
    sheetMetalCheck(
      { document_id: naive.document_id, shop_profile: "sendcutsend" },
      engine,
    ),
  );
  log(`\n    naive fold (no reliefs) vs. SendCutSend: ${naiveCheck.violations.length} violation(s)`);
  for (const v of naiveCheck.violations) log(`      [${v.severity}] ${v.message}`);
  assert(naiveCheck.violations.length > 0, "expected the naive fold to fail DFM");
  const fixes = parse(
    sheetMetalSuggestFix(
      { document_id: naive.document_id, shop_profile: "sendcutsend" },
      engine,
    ),
  );
  const fixActions = [...new Set(fixes.suggestions.map((s) => s.fix.action))];
  log(`    suggested fix: ${fixActions.join(", ")}`);
  assert.deepEqual(fixActions, ["add_bend_relief"]);

  // Apply the fix the checker asked for — reliefs in the design, not a
  // skipped check — and re-verify.
  const stand = parse(sheetMetalCreate(standCreateArgs(true, bars), engine));
  const standCheck = parse(
    sheetMetalCheck(
      { document_id: stand.document_id, shop_profile: "sendcutsend" },
      engine,
    ),
  );
  assert.equal(standCheck.violations.length, 0, "stand still has DFM violations");
  log(`    with bend reliefs: 0 violations — shop-ready ✓`);
  log(
    `    shop row: fixed R ${standCheck.shop.fixed_bend_radius_mm} mm, die ${standCheck.shop.die_width_mm} mm, ` +
      `min flange ${standCheck.shop.min_flange_height_mm} mm, holes ≥ ${standCheck.shop.min_hole_to_bend_mm} mm from bends`,
  );

  const standUnfolded = parse(
    sheetMetalUnfold({ document_id: stand.document_id }, engine),
  );
  writeFileSync(join(OUT, "stand.dxf"), standUnfolded.dxf);
  const standDoc = getSession(stand.document_id);
  writeFileSync(join(OUT, "stand.vcad"), JSON.stringify(standDoc, null, 2));
  const seq = parse(sheetMetalSequence({ document_id: stand.document_id }, engine));
  log(`    bend sequence (springback-compensated):`);
  for (const s of seq.steps) {
    log(
      `      bend #${s.bend_id}: form to ${deg(s.compensated_angle_rad).toFixed(2)}° → ` +
        `springs back to ${deg(s.angle_rad).toFixed(1)}°`,
    );
  }
  const standCost = parse(
    sheetMetalCost({ document_id: stand.document_id, quantity: 1 }, engine),
  );
  totalMassKg += standCost.breakdown.mass_kg_each;
  totalEachUsd += standCost.breakdown.total_each;

  // Folded outputs: STEP (B-rep, zero data entry at the shop) + GLB (eyes).
  parse(exportCad({ ir: standDoc, filename: "stand.step" }, engine));
  parse(exportCad({ ir: standDoc, filename: "stand.glb" }, engine));
  const [fx0, fy0, fx1, fy1] = standUnfolded.flat_pattern.bbox;
  log(
    `    flat ${(fx1 - fx0).toFixed(1)} × ${(fy1 - fy0).toFixed(1)} mm ` +
      `(${(standUnfolded.flat_pattern.area_mm2 / 100).toFixed(0)} cm²), ` +
      `${(standCost.breakdown.mass_kg_each * 1000).toFixed(0)} g → stand.dxf, stand.step, stand.glb`,
  );

  // [5] The receipt — every claim in one JSON, with its oracle named.
  const receiptBars = barReports.map(({ document_id: _id, ...rest }) => rest);
  const receipt = {
    demo: "scs-glockenspiel",
    plan: "docs/plans/2026-07-06-scs-closed-loop-demo.md",
    physics: {
      model:
        "free-free Euler–Bernoulli bar; cut lengths from the hole-aware 1-D FEM (simulate_strike), closed form as the published baseline",
      formula: "f1 = 3.5608 * (t / L^2) * sqrt(E / (12 * rho))",
      f1_times_L2_hz_m2: barConstantSI(),
      nodal_fraction_from_end: 0.2242,
      hole_shift_cents_closed_form_c6: holeShift,
      material_source: "vcad-kernel-sheet materials registry (al-hard)",
      E_gpa: BAR_MATERIAL.modulusGpa,
      rho_kg_m3: BAR_MATERIAL.densityKgM3,
      caveats: [
        "±0.1 mm on 3.175 mm stock is ±3% on pitch — calibrate with the as-built thickness",
        "alloy tolerance on E moves all bars together",
        "1-D bending modes only; decay Q in the audio is a heuristic, the frequencies are not",
      ],
    },
    bars: receiptBars,
    stand: {
      material: STAND_MATERIAL.displayName,
      scs_material: STAND_MATERIAL.scsKey,
      thickness_mm: STAND.thicknessMm,
      deck_mm: [STAND.lengthMm, STAND.widthMm],
      wall_mm: STAND.wallMm,
      foot_mm: STAND.footMm,
      cord_holes: 16,
      dfm: {
        naive_violations: naiveCheck.violations,
        suggested_fix: fixActions,
        final_violations: standCheck.violations,
        shop: standCheck.shop,
      },
      bends: seq.steps.map((s) => ({
        bend_id: s.bend_id,
        target_deg: deg(s.angle_rad),
        form_to_deg: deg(s.compensated_angle_rad),
      })),
      flat_bbox_mm: standUnfolded.flat_pattern.bbox,
      mass_kg: standCost.breakdown.mass_kg_each,
      files: ["stand.dxf", "stand.step", "stand.glb", "stand.vcad"],
    },
    totals: {
      parts: bars.length + 1,
      mass_kg: totalMassKg,
      est_cost_usd: totalEachUsd,
      cost_note:
        "generic process-cost model at qty 1 — the SCS invoice is the real oracle",
    },
    order: {
      shop: "SendCutSend",
      uploads: [
        ...barReports.map((b) => ({
          file: b.dxf,
          material: "6061-T6 aluminum",
          thickness_in: 0.125,
          finish: "raw (coatings damp the ring)",
          qty: 1,
        })),
        {
          file: "stand.dxf (or stand.step for auto-detected bends)",
          material: "5052-H32 aluminum",
          thickness_in: 0.125,
          bends: "4 × 90°, per stand.dxf BEND layers / sheet_metal_sequence",
          finish: "powder coat optional (frame only)",
          qty: 1,
        },
      ],
      assembly:
        "thread cord through each bar's nodal holes and the matching deck holes; knots ride inside the channel",
    },
  };
  writeFileSync(join(OUT, "receipt.json"), JSON.stringify(receipt, null, 2));

  const table = [
    "| note | target Hz | closed-form L | cut L (mm) | holes @ (mm) | FEM Hz | strike FFT Hz | err (¢) |",
    "|------|-----------|---------------|------------|--------------|--------|---------------|---------|",
    ...barReports.map(
      (b) =>
        `| ${b.note} | ${b.target_hz.toFixed(2)} | ${b.closed_form_length_mm.toFixed(1)} | ` +
        `${b.length_mm.toFixed(1)} | ${b.hole_xs_mm.map((x) => x.toFixed(2)).join(" / ")} | ` +
        `${b.predicted_hz.toFixed(2)} | ${b.audio.measured_hz.toFixed(2)} | ` +
        `${b.audio.cents_error.toFixed(2)} |`,
    ),
  ].join("\n");
  writeFileSync(join(OUT, "frequency-table.md"), table + "\n");

  rule();
  log(
    `  ${bars.length} bars + 1 stand — all shop-ready · ${(totalMassKg * 1000).toFixed(0)} g · ` +
      `~USD ${totalEachUsd.toFixed(0)} (generic rates)`,
  );
  log(`  receipt.json, frequency-table.md, 9 DXF, 8 WAV, STEP, GLB → ${OUT}`);
  rule();
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
