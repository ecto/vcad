# Cornerstone 1: the orderable verified tuning fork

Reality-tower cornerstone #1 (see ../reality-tower-spec.md). Goal: a 440 Hz
tuning fork that is (a) pitch-predicted from material properties, not copied
from a fork drawing, (b) verified in-session (geometry spec + DFM + fab
validation + receipt), and (c) orderable today via the sheet-metal handoff.

## Design

- Material: 304 stainless, E = 193 GPa, ρ = 8000 kg/m³ (handbook tier —
  upgrading these two numbers to homogenize-derived Quantities *is* milestone
  RT3; this document is the before/after benchmark).
- Stock: 0.25" (6.35 mm) plate, laser cut, flat — no bends, so the press-brake
  arrow stays out of scope for cornerstone 1 (it enters with the thermostat).
- Tine mode: **in-plane** cantilever. f₁ = (β₁²/2π)·(w/L²)·√(E/12ρ),
  β₁L = 1.8751. Pitch depends on tine *width* w and length L; sheet thickness
  only gates which mode is fundamental.
- Chosen: w = 6 mm → L = 104.0 mm for f₁ = 440.0 Hz.
  Check: √(193e9/(12·8000)) = 1417.9 m/s; 0.55958·1417.9·0.006/0.104² = 440 Hz.
- t = 6.35 mm > w = 6 mm ⇒ out-of-plane mode ≈ 440·(6.35/6) ≈ 466 Hz sits
  *above* the in-plane fundamental; strike direction selects in-plane.
- Geometry: tines 6×104, gap 8 (yoke top at y = 74), yoke 20×14, handle
  10×60. Overall 20 × 178 × 6.35 mm. Nominal volume 13 512.8 mm³
  (tines 7924.8 + yoke 1778.0 + handle 3810.0).

## Known model errors (state them, don't hide them)

- Clamped-root assumption: the yoke is compliant, which *lowers* f₁ by a few
  percent vs. ideal cantilever. Sharp inner corners raise local compliance
  further. Expected: fork arrives slightly flat (430–440 Hz band).
- Tuning protocol: file tine tips to raise pitch (shortens L); material
  removal near the root lowers it. One-directional recovery is why we bias
  none — the yoke error already biases flat, tip-filing corrects upward.
- Laser kerf: SendCutSend compensates kerf to nominal; edge taper on 6.35 mm
  stainless is sub-1% of w.

## Oracle protocol (glockenspiel-style)

1. Receive fork; suspend at yoke or hold handle; strike tine tip
   perpendicular to the sheet plane's *long* axis (in-plane excitation).
2. Measure with a chromatic tuner app (same oracle as glockenspiel).
3. Record via `record_measurement`: predicted 440 Hz (−0/+? with the stated
   flat bias), measured value, cents deviation.
4. The measurement is RT7 seed data: it constrains E/ρ and the yoke-
   compliance correction for the *next* fork.

## Verification ladder in-session

verify_spec (bbox + volume + watertight + part count) → dfm_check
(sheet_metal) → quote_manufacturing (sheet_metal, stainless, qty 1, includes
fab_handoff recipe) → receipt. "Orderable" = clean DFM + valid quote +
handoff instructions; actual order placement is a human step (Phase 0).
