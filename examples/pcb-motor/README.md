# PCB-Stator Axial-Flux Motor (rare-earth-free)

A 70 mm axial-flux motor whose stator is a 2-layer/2 oz PCB (9 slots / 6 poles,
3-phase wye) and whose field comes from ceramic Y30 ferrite discs — **zero
rare-earth content**. Designed, verified, and priced entirely through the vcad
MCP toolchain across three sessions; this example is the third (all-green) pass.

![stator v3](fab/stator-v3-board.png)

## What's here

| File | What |
|---|---|
| `stator-v3.vcad` | Winding board — DRC 0, receipt `31161edd73a2ca11` |
| `motor-assembly-v3.vcad` | Full assembly (board via `solid_from_board`, 11 parts) with named clearance assertions |
| `rotor-back-iron-v3.vcad`, `stator-back-iron-v3.vcad` | SendCutSend steel discs (2.7 mm mild steel) |
| `rotor-dragcup.vcad` | Optional all-copper induction rotor — **verified NOT to self-start** (margin 0.005 vs bearing friction); kept as an eddy-current demo |
| `motor-base.vcad` | 3D-printed bearing tower (2× 608-ZZ); export STL via `export_cad` |
| `fab/BOM-v3.md` / `.csv` | Order-ready BOM, $269.32 landed, quote-linked |
| `fab/VERIFICATION-v3.md` | Tool-emitted verification ledger (+ v2 ledger for history) |
| `fab/stator-v3-gerbers/`, `fab/rotor-gerbers/` | Fab files |
| `scripts/pipeline.py` | The whole build+verify pipeline as one script |
| `scripts/mcp-bridge.mjs` | stdio→HTTP bridge for driving a local MCP build |

## Numbers

> **No board in this example has been fabricated.** Every electromagnetic figure
> below is kernel-computed — a design target, not a measurement. Only the
> clearances are measured, and those from the assembly geometry rather than from
> hardware. Nothing here has been on a dyno.

- PM config: B_gap 0.155 T (fringing-derated MEC), **Kt 3.7 mN·m/A** (first-order
  closed form — see the cross-check below, which says this is optimistic),
  ~7 mN·m @ 1.5 A, self-start margin **9.25** with 608-ZZ shielded bearings (2RS
  contact seals drop it to 1.39 — the bearing spec matters).
- Induction (coils-only) config: locked-rotor 18.5 µN·m — ~100× under bearing friction.
  Magnet-free PCB induction at this scale is a physics demo, not a motor.
- Clearances (kernel-measured, re-runnable): air gap 1.000 mm, magnet↔screw-heads
  3.15 mm, shaft↔bearing slip fit 0.0498 mm, plus three more — all asserted ≥ spec.

### Independent cross-check — the torque constant is ~1.6× optimistic

`vcad-kernel-magnetostatic` solves this exact winding in 3D from Biot–Savart with
no grid and no mesh, and disagrees with the closed form:

| quantity | closed form | 3D oracle | verdict |
|---|---|---|---|
| `B_gap` | 0.155 T | 0.164 T | flux model holds up — within 6% |
| `Kt` | 3.7 mN·m/A | **2.26 mN·m/A** | closed form **1.6× optimistic** |

The flux model is not the problem; the torque formula is. `Kt = kw·N·p·B_gap·A_pole`
charges every pole the full annular sector, `π(r_out²−r_in²)/(2p)` = 3.39e-4 m². A
7.2 mm-radius circular coil actually encloses 1.63e-4 m² — **2.08× less**. Nine such
coils cover only 72% of the annulus, and each covers less than its own sector. The
formula is sound for a winding that genuinely fills its sector; these discrete
circular coils do not.

**What this changes for the build.** The self-start margin is a torque-vs-bearing-
friction ratio, so it scales with `Kt`. Derating by 1.6× takes it from 9.25 to ≈5.6
on 608-ZZ shielded bearings — still ample, the motor starts. But it takes the 2RS
contact-seal figure from 1.39 to **≈0.85, below 1**: with 2RS bearings this motor
likely would not start at all. Order the 608-ZZ shielded bearings the BOM specifies;
that line is now load-bearing, not a preference.

Reproduce with:

```bash
cargo run -p vcad-kernel-magnetostatic --example grade_calc_motor --release
```

## Regenerating fab outputs

Everything derives from the `.vcad` files: `export_gerber` for boards, `export_cad`
(STEP) for the sheet-metal irons, `quote_manufacturing` / `sheet_metal_cost` for
prices, `scripts/pipeline.py` for the full pass including regression checks.
