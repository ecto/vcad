# Calibration coupon — print-then-measure, zero vendors

The canonical part for vcad's **3DP private calibration loop** (see
[the design doc](../../docs/plans/2026-07-07-3dp-print-then-measure.md) and
[the domain atlas](../../docs/plans/2026-07-06-domain-atlas.md)): your own
printer is the effector, calipers and a kitchen scale are the oracle, and
every print captures a `(predicted, measured)` pair — the cheapest physical
ground truth vcad can buy.

One 80×32 mm plate, ~1 hour of print time, spanning the FDM failure modes
that matter:

| Feature | Measurables | What it calibrates |
|---|---|---|
| base plate 80×32×4 | `bbox_x`, `bbox_y` | XY scale over long spans |
| 4-step staircase, tops at Z 6/8/10/12 | `step_z_6…12`, `bbox_z` | Z scale at several heights |
| through-holes Ø3/Ø5/Ø8 | `hole_3mm/5mm/8mm` | hole undersize vs size |
| boss Ø6 | `boss_6mm` | outer vs inner diameter asymmetry |
| fins 0.8/1.2/2.0 mm | `fin_0_8/1_2/2` | thin-wall accuracy / flow |
| the whole part | `mass` | volume × density vs the scale |

[`geometry.mjs`](geometry.mjs) is the single source of truth: the same
`PARAMS` build the document **and** the declared measurables, so prediction
and part cannot drift apart.

## 1. Build and predict

```bash
# From the repo root (fresh worktree? run `npm ci` first):
npm run build --workspaces
node examples/calibration-coupon/build.mjs
```

Outputs land in `out/`:

- `coupon.stl` — slice and print. **Use 100% infill** or the mass row is
  meaningless (every dimensional row still works).
- `coupon.vcad` — editable parametric source.
- `prediction.json` — the pre-print snapshot from `predict_print`:
  kernel-evaluated bbox/volume/mass plus the 11 declared feature
  measurables, fingerprinted against the exact document that made them.
- `measurements.template.json` — the guided worksheet.

## 2. Print, then measure

Print `coupon.stl` (PLA assumed; any rigid filament works — adjust
`density_kg_m3` in `geometry.mjs` if you predict mass for something else).
Then:

```bash
cp out/measurements.template.json out/measurements.json
$EDITOR out/measurements.json
```

Each entry in `guide` tells you what to measure and where ("Ø3 through-hole
diameter (caliper inside jaws, front row) — predicted 3mm"); put your
numbers in `measurements`. Leave anything you didn't measure as `null` —
a partial worksheet is still data.

## 3. Record the delta

```bash
node examples/calibration-coupon/record.mjs
```

This joins your numbers against `prediction.json` via the same
`record_measurement` tool the MCP server exposes, prints the delta table,
and writes `out/calibration-report.json` — the receipt-vs-reality artifact,
stored alongside the document. Beyond per-feature deltas it reports the
aggregates a printer profile can act on:

```
X scale: 99.80%   Z scale: 100.29%   hole offset: -0.12mm   wall offset: +0.07mm
→ XY prints small by 0.21% — set shrinkage/scale compensation to 100.21%
→ holes print 0.12mm undersize — enable hole compensation or drill/ream to size
```

Axis scale fits use only span-like features (plate, steps); holes and walls
are excluded because their systematic offsets (undersize, over-extrusion)
would masquerade as shrinkage — they get their own aggregates instead.

## MCP flow (no scripts)

The same loop is available to any MCP agent against a live session:

1. `predict_print { document_id, material_density_kg_m3: 1240, measurables: […] }`
2. …human prints and measures…
3. `record_measurement { document_id, measurements: { bbox_x: 79.84, … },
   printer: "Bambu X1C" }` — or pass the saved `prediction` inline if the
   session is gone.
