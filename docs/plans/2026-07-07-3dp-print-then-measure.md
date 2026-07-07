# 3D printing: the print-then-measure calibration loop

*The 3DP slice of [the domain atlas](2026-07-06-domain-atlas.md): "the only
domain where the loop closes with zero vendor dependency — the user's printer
is the effector. Cheapest physical ground truth per datum." Gap named there:
**print-then-measure receipt flow; dimensional accuracy oracles per printer
profile.** This doc designs and ships the minimal version. 2026-07-07.*

## Why this loop

Every other rail (SCS, PCB fabs) puts a vendor between the design and the
ground truth. A Bambu on the desk doesn't. That makes 3DP the **volume**
source of sim2real data: every print is a chance to capture a
`(predicted, measured)` pair for near-zero cost, and those pairs are exactly
what the Receipt needs to stop being a promise and start being a track
record.

The loop, minimally:

```
design ──► predict_print ──► slice + print (human + Bambu) ──► calipers/scale ──► record_measurement ──► delta report
              │                                                                        │
              └── prediction snapshot (stored with the session) ───────────────────────┘
```

Deliberately **not** in scope: printer integration beyond what
`vcad-slicer`/`vcad-slicer-bambu` already do. The human carries the part to
the printer. The value is the captured pairs, not automation of the carry.

## The two artifacts

Both are small JSON documents. Pure computation lives in
`@vcad/core` (`packages/core/src/utils/print-calibration.ts`) — no I/O, no
kernel, no MCP — mirroring the receipt engine's layering. The MCP tools wrap
it; example scripts write the JSON to disk alongside the document.

### 1. `PrintPrediction` — recorded *before* printing

Produced by the `predict_print` MCP tool from an open session document:

```jsonc
{
  "version": 1,
  "document_id": "abc123",
  "doc_fingerprint": "fnv1a-128 hex of the canonicalized document IR",
  "created_at": "2026-07-07T…",
  "material": { "name": "PLA", "density_kg_m3": 1240 },
  "volume_mm3": 21847.3,          // kernel-evaluated, not hand math
  "bbox_mm": { "x": 80, "y": 32, "z": 12 },
  "assumptions": [
    "mass assumes 100% infill (solid) — print the coupon solid or ignore the mass row",
    "dimensions are model-space; no shrinkage compensation applied"
  ],
  "measurables": [
    { "id": "bbox_x", "label": "Overall length (X)", "kind": "dimension",
      "axis": "X", "feature": "overall", "predicted": 80, "unit": "mm" },
    { "id": "hole_3mm", "label": "Small hole diameter", "kind": "diameter",
      "axis": "XY", "feature": "hole", "predicted": 3, "unit": "mm" },
    { "id": "mass", "label": "Part mass", "kind": "mass",
      "predicted": 27.09, "unit": "g", "tolerance": 1.35 }
  ]
}
```

Measurables come from two sources, merged:

- **Auto** (computed by the tool from the evaluated kernel mesh): `bbox_x/y/z`
  and `mass` (when a density is known — from the `material_density_kg_m3` arg
  or the document's material table).
- **Declared** (passed by the caller): named features the mesh can't name —
  step heights, hole diameters, wall thicknesses. For the calibration coupon
  these are *derived from the same parameters that built the geometry*, the
  f405 pattern: prediction and part can't drift apart by construction.

`doc_fingerprint` hashes the canonicalized document IR with the receipt's
existing `hashHex`/`canonicalize` (fnv1a-128), so `record_measurement` can
detect "you edited the design after predicting" and flag the pairing stale.

### 2. `CalibrationReport` — the receipt-vs-reality delta

Produced by `record_measurement` from a prediction + measured values:

```jsonc
{
  "version": 1,
  "doc_fingerprint": "…", "stale": false,
  "context": { "printer": "Bambu X1C", "material": "PLA Basic black",
               "process": "0.2mm layers, 100% infill" },
  "rows": [
    { "id": "bbox_x", "kind": "dimension", "axis": "X",
      "predicted": 80, "measured": 79.82, "unit": "mm",
      "delta": -0.18, "delta_pct": -0.225,
      "tolerance": 0.16, "within_tolerance": false }
  ],
  "missing": ["step_z_12"],        // predicted but not measured
  "unknown": [],                   // measured but never predicted
  "aggregates": {
    "axis_scales": [                // least-squares measured ≈ scale·predicted
      { "axis": "X", "n": 3, "scale": 0.99775 },
      { "axis": "Y", "n": 2, "scale": 0.99810 },
      { "axis": "Z", "n": 5, "scale": 1.00120 }
    ],
    "hole_offset_mm": -0.14,        // mean(measured−predicted), holes only
    "wall_offset_mm": 0.06,         // …thin walls only (flow signature)
    "mass": { "predicted_g": 27.09, "measured_g": 26.4, "delta_pct": -2.5 }
  },
  "suggestions": [
    "XY prints ~0.21% small — set shrinkage/scale compensation to 100.21%",
    "holes print 0.14mm undersize — enable hole compensation or drill to size"
  ],
  "verdict": "attention",           // pass | attention | fail
  "summary": "9/12 within tolerance; XY scale 99.78%; holes −0.14mm"
}
```

The aggregates are the point. Raw deltas are data; **axis scale factors,
hole undersize, and wall offset are the actual knobs** a printer profile
exposes (shrinkage compensation, hole compensation, flow ratio). One coupon
print yields the numbers to turn. Accumulating reports per
(printer, material) is the future "dimensional accuracy oracle per printer
profile" — out of scope here, but the `context` block is shaped so the
aggregation needs no schema change.

Default tolerances when a measurable doesn't carry one:
`±max(0.1mm, 0.2% of nominal)` for dimensions/diameters (a well-tuned FDM
machine's realistic envelope), `±5%` for mass (infill/flow variance).
Verdict: `pass` (all rows within tolerance), `fail` (any row out by more
than 3× its tolerance, or a majority out), else `attention`.

## Tool contracts

New MCP pack `print` (opt-out like the others, in `TOOL_PACKS`):

- **`predict_print`** `{ document_id, material_density_kg_m3?, material_name?,
  measurables? }` → the `PrintPrediction`, returned inline AND cached
  in-process keyed by `document_id` (same warm-instance lifetime as sessions
  and the artifact registry — see `session.ts`). Small payload; the caller
  is expected to keep it (the example writes it to disk next to the doc).
- **`record_measurement`** `{ document_id?, measurements, printer?, material?,
  process?, prediction? }` → the `CalibrationReport`, inline. `measurements`
  is `{ id: value }`. The prediction is resolved from the inline `prediction`
  arg first, then the warm cache — the inline path makes the tool usable
  even after a cold serverless restart, and is how the example's
  `record.mjs` replays a prediction file. Reports are also appended to the
  warm cache for the session.

Storage stays deliberately simple, per the artifact-store precedent: inline
JSON + warm-instance cache + files-on-disk in the example. No new tables.
When the unified Receipt grows a claims spine, `CalibrationReport` becomes a
signed claim ("as-built within tolerance of as-designed") — the shapes here
were chosen so that's an envelope change, not a rewrite.

## The canonical coupon — `examples/calibration-coupon/`

One plate, printable in ~1h, spanning the failure modes that matter:

| Feature | Measurable(s) | What it calibrates |
|---|---|---|
| base plate 80×32×4 | `bbox_x`, `bbox_y` | XY scale (long spans) |
| 4-step staircase, tops at Z 6/8/10/12 | `step_z_6…12` | Z scale at multiple heights |
| through-holes Ø3/Ø5/Ø8 | `hole_3mm…8mm` | hole undersize vs size |
| boss Ø6 | `boss_6mm` | outer vs inner diameter asymmetry |
| thin fins 0.8/1.2/2.0 | `fin_0_8…2_0` | flow / wall accuracy |
| whole part | `mass` | density × volume vs scale |

Geometry rules: rectilinear unions with parts sunk 1 mm into the base (no
coincident-face union seams), plain cylinder differences through a flat
plate (the best-tested boolean path in the kernel), no fillets, no text.
`geometry.mjs` exports both the document and `measurables()` derived from
the same `PARAMS`.

Scripts, following the f405-enclosure pattern (import MCP tools from
`packages/mcp/dist`):

- `build.mjs` — evaluate, `predict_print`, export STL, write
  `out/prediction.json` + `out/measurements.template.json` (the guided
  worksheet: every measurable with its label and a `null` to fill in).
- `record.mjs <measurements.json>` — `record_measurement`, write
  `out/calibration-report.json`, print the delta table.

The worksheet **is** the "guided measurement step": each row tells the human
what to measure and where ("Step 2 top face height off the bed"), in
measurement order. No app UI in v1.

## Follow-ups (not this change)

- Aggregate reports per (printer, material) into a persistent calibration
  profile; feed compensation back into slicer settings (`smart_defaults`).
- Auto-derive hole/wall measurables from `vcad-slicer`'s BRep
  `analyze_for_printing` (it already detects holes and wall thicknesses).
- Infill-aware mass prediction via `vcad-kernel-cost`'s
  `estimate_fdm_from_volume` when the print won't be solid.
- `vcad measure` CLI subcommand wrapping the same core engine.
- Receipt unification: emit the report as a signed receipt claim.
