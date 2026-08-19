# rose-pro

Original 22-DOF humanoid, 1130 mm, 17.58 kg. Not a K1 clone — the K1's joint axes
are too tightly packed for off-the-shelf actuators (its hip roll and yaw axes
intersect at a point), so this is an original layout designed around MyActuator
X-series envelopes.

## Build

```
cargo build --release -p vcad-cli -p vcad-render   # needs sibling tang/ and phyz/
target/release/vcad info   hardware/rose-pro/rose-pro.loon
target/release/vcad export hardware/rose-pro/rose-pro.loon /tmp/rose-pro.stl
target/release/vcad-render hardware/rose-pro/rose-pro.loon --view iso --size 1400 \
    --fill 0.92 --auto-aspect -o /tmp/rose-pro.png
```

`vcad-render` takes `.loon` directly. Evaluated root meshes are cached under
`~/.cache/vcad` keyed on each root's expression and the kernel build, so a
second render is ~1 s and editing one plate re-evaluates one plate
(`--no-cache` / `VCAD_CACHE=0` to opt out).

## Files

| file | what |
|---|---|
| `rose-pro.loon` | **generated** — 137 roots (115 plates, 22 actuator envelopes). Do not hand-edit; regenerate with `rose/pro/rose_pro_loon.py`. |
| `plates.loon` | hand-maintained loon **module**: `plate-x/y/z` rounded-rect builders, `bore-x/y/z` through-bores, `act-x/y/z` actuator envelopes. `rose-pro.loon` imports it with `[use plates [...]]`; vcad resolves it next to the file. |

## Actuators

| joint group | part | envelope | stator | output |
|---|---|---|---|---|
| hip pitch/roll/yaw, knee | X6-60 | Ø80 × 67.5 | 12×M3 on Ø72.5, Ø53 spigot | 12×M4 on Ø44, Ø32 pilot |
| ankles, arms | X4-36 | Ø55 × 61.0 | 6×M3 on Ø48, Ø3 dowels | 6×M3 on Ø26, Ø35 boss |
| head | X4-10 | Ø55 × 55.5 | as X4-36 | as X4-36 |

All figures from the vendor installation drawings (X-series V4 manual), cross-checked
against the vendor STEP files.

## Structure

3 mm 5052 sheet, cut and bent. Bend radius 1.5 (0.5t). Couplings are one blank where
the bend line clears both bolt circles, two bends with a corner relief where it
crosses one, and two lapped pieces where it lands inside a circle. See
`ipse/scripts/rose_pro_bent.py` for the per-coupling determination.

## Source of truth

Geometry is generated from `ipse/scripts/rose_pro_links.py`, which carries the
clearance, hole-edge and bend-rule checks. Regenerate with
`ipse/scripts/rose_pro_loon.py`. Do not hand-edit `rose-pro.loon`.

## Verification

- volume cross-check against the independent analytic plate model:
  `ipse/scripts/rose_pro_vcad_check.py` — vcad 1379.0 cm³ / 3696 g vs Fusion 3768 g,
  1.9% across two kernels.
- the robot description (`ipse/scripts/rose_pro_export.py`) loads in phyz and stands
  at +0.0% ground-force error.

## Notes for whoever touches this next

- Union the bores into ONE tool before differencing. Cutting a 13-hole plate one
  bore at a time costs 27 s versus 14 s for identical output — every chained cut
  re-processes the whole accumulated mesh.
- Keep bores exiting planar faces. vcad's boolean-fidelity matrix degrades on
  through-cuts breaking out of round walls, which silently yields faceted STEP.
- The `floating_floor` DFM warning on export is a 3D-printing orientation check and
  does not apply to sheet parts.

## vcad CLI notes

- Both `vcad render` (the CLI subcommand) and `vcad-render` are Z-up now
  (`vcad render --up` defaults to `z`); the old "robot fell over" trap is gone.
- Per-plate boolean cost is NOT monotonic in corner radius (r=16 and r=25 are
  5-10x slower than r=20 on the same plate), and unioning lapped/stacked plates
  sent one mirrored link past 400 s while its mirror took 10 s — hence one root
  per plate and r=20. Filed as ecto/vcad#821 and #822 (numbers in the rose generator's
  `corner_radius` docstring).
