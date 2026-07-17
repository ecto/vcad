# Antenna measurement pack: the $100 loop

The cheapest hardware validation in the vcad portfolio: a PCB monopole
through the repo's existing gerber pipeline, a ~$100 NanoVNA, and the
fail-closed `compare()` that turns the sweep into per-claim verdicts.
Everything below is already rehearsed in software
(`tests/measurement_pack.rs` runs the entire loop against a synthetic
sweep, including the violation the real board is expected to produce).

## The board

915 MHz ISM quarter-wave monopole, chosen because the free-space model
puts a 78 mm radiator *exactly on band* — and because the fabricated
board will not agree, which is the point (see "What we expect to
happen").

- **Substrate:** FR-4, 1.6 mm, 1 oz copper, ~25 × 90 mm.
- **Radiator:** 78.0 × 1.6 mm straight trace, top region of the board,
  no copper beneath it or beside it (≥ 8 mm keepout each side).
- **Ground:** solid pour on the bottom ~40 mm of both layers, stitched;
  the radiator base meets the pour edge.
- **Feed:** edge-launch SMA at the radiator base, launched against the
  pour (this is the calibration plane).
- **Emit path:** the repo's ecad pipeline (`set_board_outline` →
  `add_trace` → `add_zone` → `run_drc` → `export_gerber`); the wire
  model used for the claims comes from the same centerline through
  `ecad::add_trace_as_wire` (strip w → radius w/4).

## The predicted claims (free-space model, N = 12, band 700–1100 MHz)

`cargo run --release -p vcad-kernel-antenna --example pcb_monopole_claims > claims.json`

| claim | predicted | unit |
|---|---:|---|
| s11_db_at_band | −16.18 | dB vs 50 Ω |
| s11_min_freq | 925.0 | MHz |
| z_in_re / z_in_im | 37.5 / +5.5 | Ω |
| resonance_in_band | 1 | — |
| resonant_frequency | **913.13** | MHz |
| bandwidth_10db | 107.3 | MHz |
| gain_dbi | 5.16 | dBi (hemisphere, PEC) |
| radiation_efficiency | 1.000 | (energy-balance cross-check) |

Every claim carries the substrate caveat verbatim: **free-space PEC, no
FR-4** — these are M1-honest numbers, not board predictions.

## The measurement

1. NanoVNA (any NanoVNA-H/-F class unit): SOL calibrate at the SMA
   plane, 700–1100 MHz, ≥ 101 points.
2. Save the sweep as Touchstone `.s1p` (RI, MA, or DB — all parsed;
   NanoVNA-Saver's default RI/Hz works as-is).
3. Bind:

```rust
let claims: ClaimSet = serde_json::from_str(&fs::read_to_string("claims.json")?)?;
let sweep = nanovna::parse_s1p(&fs::read_to_string("board.s1p")?)?;
let ms = nanovna::measurements_from_s1p(&sweep, &claims, &NanoVnaTolerances::default())?;
let report = receipt::compare(&claims, &ms)?;
```

`compare` is fail-closed: a measurement naming no claim is an error, an
unmeasured claim reads Unmeasured, and `fully_verified` is true only
when everything is measured and holds. A one-port VNA cannot measure
gain or efficiency, so those two claims read **Unmeasured** — correctly,
loudly, every time.

## What we expect to happen (and why that's the win)

FR-4 under the radiator loads it: the measured dip should land roughly
**20–30% below** 913 MHz (ε_eff ≈ 1.5–2 for a trace at a pour edge).
The frequency-bearing claims will read **Violated**, exactly as
rehearsed in `substrate_downshift_reads_violated` (synthetic sweep at
0.72×: `s11_min_freq` Violated, `resonance_in_band` Violated, report
refuses to verify).

That violation is not a failure of the loop — it **is the M1.5
measurement**: the measured `f_meas/f_pred` ratio is `1/√ε_eff` for this
geometry, and it calibrates the quasi-static substrate correction that
turns PCB trends into PCB predictions. The second board spin, with M1.5
applied, is the one expected to read Holds across the frequency claims.

## BOM

| item | cost |
|---|---:|
| NanoVNA-H (or -F) with cal standards | ~$60–120 |
| PCB fab, 5 boards | ~$5–20 |
| Edge-launch SMA (×2 spares) | ~$5 |
| SMA cable (calibrated reference plane) | ~$10 |
| **total** | **≈ $100** |

## Fail-closed guarantees exercised by this pack

- `.s1p` parsing rejects unknown units/formats/columns by line number.
- Fewer than 2 sweep points inside the claimed band is an error.
- Stray measurement names are errors, never ignored.
- Unmeasured claims never pass; `fully_verified` requires all-measured,
  all-holding.
- A Violated verdict is a result about the model, and the expected first
  one is named in advance (the substrate downshift).
