# Heat-conduction FEA M5+M6: benchmarks, convergence honesty, and the measurement pack

Final two rungs of the `vcad-kernel-thermal` M0 ladder.

## M5 — benchmark + convergence (`examples/convergence.rs`)

**Grid convergence, hot_chip** (anisotropic board, h = 10):

| grid | pitch (mm) | θ_ja (K/W) | Δ |
|---|---:|---:|---:|
| 25×25×13 | 4.0 | 20.80 | — |
| 50×50×13 | 2.0 | 19.78 | −4.9% |
| 100×100×13 | 1.0 | 22.45 | +13.5% |
| 100×100×26 | 1.0 | 22.17 | −1.3% |
| 200×200×26 | 0.5 | 21.88 | −1.3% |

The floor has a *name*: coarse pitches cannot represent the 10 mm die
footprint (center-containment mis-paints it — the ±5–13% jumps are the
footprint snapping into place, not solver noise). Once the footprint is
exact, θ drifts ~1.3% per further halving. The honest quote is the
1 mm/0.2 mm value **with a ~2% grid band**, and the receipt provenance
carries the grid so anyone can check.

**JEDEC-style consistency check**: a JESD51-7-shaped 2s2p board
(76.2×114.3×1.6 mm at effective [20, 20, 0.4] W/m·K from two buried
planes) with a 9×9 mm 1 W die lands at θ_ja = 24.6–28.5 K/W across
plausible still-air combined coefficients (h_eff = 8–15) — inside the
20–30 K/W band that datasheet θ_ja values for 9–10 mm exposed-pad
packages on JEDEC 2s2p boards commonly occupy. Labeled a **consistency
check, not a validation**: h_eff bundles convection *and* radiation
(JEDEC still-air chambers include both), and there is no package model
(the die couples straight to the board; junction-to-board resistance
~1–3 K/W unmodeled). Landing in-band says the geometry and copper
bookkeeping are sane; only measurements can say more.

Paper skeleton with all current numbers: `docs/thermal-paper-draft.md`.

## M6 — the measurement pack (`receipt::compare`)

`Measurement` binds bench data to predicted claims; `compare()` returns
Holds / Violated / Unmeasured per claim, fail-closed exactly like the
particle crate's: an **unmeasured receipt never passes**, a measurement
matching no claim is an error, and Violated is a publishable result about
the model, not an embarrassment to bury.

The instrument caveats are in the type's documentation because they are
where thermal measurements actually die:

- **Thermal cameras read radiance, not temperature.** ε ≈ 0.9 board
  surfaces read honestly; bare copper/solder at ε ≈ 0.05 reads the
  reflection of the room — a hot plane *shows cold*. Tape or paint a
  known-ε target on every compared spot; record the camera's ε setting in
  the instrument string.
- **Thermocouples read their own junction.** Contact resistance plus lead
  conduction pull a pressed-on junction toward ambient by several K on
  small hot spots. Glue or solder the junction; derate the band.
- **h is the third instrument and nobody calibrates it.** A prediction
  priced at h = 10 compared against a bench with a draft is not a model
  error. θ bands under natural convection honestly carry ±20–30%
  (`band_factor` ≈ 1.3); the energy residual gets a tight one — it is a
  solver property, not a bench property.

Bench protocol for the hot_chip demo board: power a 10×10 mm resistive
load at 2.000 W (4-wire), board horizontal in still air, ε-taped targets
at die center and two board corners, one epoxied TC under the die;
measure at thermal equilibrium (>5 time constants ≈ 30 min), bind
`t_max_c` (camera) and `theta_ja_c_per_w` (TC + wattmeter) and let the
receipt say Holds or Violated.

## Ladder complete

M0 solver → M1 transient/anisotropy → M2 adjoint → M3 seam → M4 claims →
M5 benchmarks → M6 measurements. Flagged follow-ups (cross-crate, next
PRs): `vcad-receipt` registration + `predict_thermal` MCP tool; PDN
joule-heating import from the ecad layer; radiation; the tessellated-part
voxelizer on the vcad side of the `VoxelMaterials` seam.
