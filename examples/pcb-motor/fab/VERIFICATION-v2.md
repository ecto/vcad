# Motor v2 — Verification Ledger

Rare-earth-free PCB-stator axial-flux motor, second pass (2026-07-08), built end-to-end
on the post-ship vcad toolchain. Every claim below is tool-emitted, not hand-derived.

## Electromagnetics (design-first, all `calc_motor` / `check_self_start`)

| Claim | Value | Verdict |
|---|---|---|
| Induction (drag-cup) locked-rotor torque @1.9A/100Hz | 18.5 µN·m (28.5 raw, ×0.65 end-effect) | — |
| Induction self-start vs 608 pair (fail-closed) | margin **0.0046** | **REJECTED** |
| PM B_gap (MEC, fringing-derated, pole 15mm/gap 2.6mm) | 0.1555 T | ✓ |
| PM Kt | 3.7 mN·m/A | ✓ |
| PM self-start @1.5A, **608-ZZ** shielded | margin **9.25** | **PASS** |
| PM self-start @1.5A, 608-2RS contact seals | margin 1.39 | avoided → BOM specs ZZ |

## Stator PCB (`stator-v2.vcad`, receipt board_hash `798e30816f7d06f4`)

- Realized by the rebuilt `add_motor_winding` (planar bore rings + rim neutral loop);
  its `drc_delta` self-reported 2 NetIslands (PHB/PHC feeds) — repaired with 2
  hand-routed feed stitches, each mutation verify-on-write clean.
- **DRC: 0 violations** (SameNetBypass rule active). Gerbers: `fab/stator-v2-gerbers/`.
- DFM `pcb_jlcpcb`: pass except (a) min_clearance ×4 at the wye net-tie junctions —
  net-blind DFM rule, waived (net-aware DRC passes; fix chipped), (b) acid_trap ×20
  spiral polyline joints at 87.8° — cosmetic, waived.
- Mounting moved to **bore-mount** (3× M3 @ r=8) after DFM copper_to_edge caught the
  rim-arc collision with v1-style rim holes; bore zone verified copper-free.

## Mechanical (`motor-assembly-v2.vcad`, board via `solid_from_board`, 11.63 g)

All named `check_clearance` assertions PASS:

| Label | Required | Measured |
|---|---|---|
| air-gap (magnets ↔ stator PCB) | ≥ 0.9 | **1.000 mm** |
| magnet-vs-heads | ≥ 0.5 | 3.15 mm |
| shaft-vs-stator bore | ≥ 0.8 | 0.995 mm |
| hub-vs-heads | ≥ 2.0 | 5.05 mm |
| rotor-iron-vs-board | ≥ 2.0 | 4.00 mm |
| shaft-vs-bearing (slip fit) | ≥ 0.02 | 0.0498 mm |

## Back irons (SendCutSend, `*-back-iron-v2.vcad`)

- Rotor: Ø58×2.7, bore 8.4, **4× M4 taps @ BCD22** (matched to catalog flange coupling).
- Stator: Ø70×2.7, Ø16 center, 3× Ø3.4 @ r=8 (bore-mount).
- `verify_spec` geometric grading: bbox/part-count PASS; volume+watertight FAIL due to a
  now-chipped flange-with-holes tessellation bug — independently cross-checked correct via
  `sheet_metal_cost` mass (54 g ≈ 6870 mm³) and STEP export. Fail-closed, cause known.

## BOM (`fab/BOM-v2.md` / `.csv`, bom `2d2a1cbe`)

15 lines, 5 manufactured (each linked to a persisted quote id) + 10 COTS (catalog-resolved
specs incl. Y30 Ø15×3 ferrite discs — zero rare earths). **Grand total $271.94 landed**
(incl. $42 optional induction-demo PCBs and spares); emits `bom.cost.total` receipt claim.

## Product bugs found & chipped this round

1. DFM min_clearance is net/tie-blind (flags wye junctions DRC passes).
2. MCP create/apply_edits leaves intermediate nodes as live roots (ghost geometry).
3. quote_manufacturing sheet-metal model 3× above the calibrated laser cost model.
4. Sheet-metal flange-with-holes mesh non-watertight, volume ~⅓ truth (verify_spec catch).
