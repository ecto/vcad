# EM measurement pack: binding the 70 mm PCB motor's predictions to the bench

The M6 rung: every prediction in `docs/em-m0.md` §motor is a
`vcad.em-claims/1` claim (`cargo run --release -p vcad-kernel-em
--example motor_receipt` emits the JSON), and this document is the bench
procedure that binds real instruments to it through
`vcad_kernel_em::receipt::compare` — Holds / Violated / Unmeasured,
fail-closed. **Violated is a publishable result about the model**, not a
failure to hide: the 2D slice states its omissions (curvature, radial
end fringing, linear steel, statics), and the acceptance bands below are
sized by those omissions, not by optimism.

Hardware: the fabricated stack of `examples/pcb-motor` (stator-v3 board,
Y30 Ø15×3 discs on the 2.7 mm steel discs, 1.0 mm air gap, 608-ZZ
bearings).

## Predicted claims to bind

| claim | predicted | band | why the band |
|---|---|---|---|
| `torque_nm` @ 1.5 A peak, best angle | 4.64 mN·m (Kt ≈ 3.1 mN·m/A) | ×1.35 | slice omits curvature + radial end fringing (each ~10%); commutation alignment error on the bench |
| air-gap flux under a pole center | 0.201 T | ×1.25 | staircased magnets, Br tolerance of ceramic Y30 (0.38–0.40 T), gap tolerance ±0.05 mm |

(The solved B_gap also confirms the design pipeline's raw MEC 0.204 T to
1.5% — two models already agree before any measurement; the bench
decides whether *both* books are cooked.)

## Procedure 1 — Kt via back-EMF (primary; no torque instrumentation)

The cleanest Kt measurement needs a drill, a scope, and Ohm's law:
`Kt = Ke` in SI (N·m/A ≡ V·s/rad, same convention: peak sinusoidal
phase quantity).

1. Leave all phases open. Spin the rotor with a drill/hand driver
   through a rubber coupling at roughly constant speed.
2. Scope any two phase terminals (line–line). Measure the AC frequency
   `f_e` (Hz) and peak line-line voltage `V_ll_pk` over a few cycles.
   Speed comes from the waveform itself — no tachometer:
   `Ω = 2π·f_e / p`, p = 3 pole pairs.
3. `Ke = Kt = V_ll_pk / (√3 · Ω)` N·m/A.

Worked expectation at 1000 RPM (Ω = 104.7 rad/s, f_e = 50 Hz):
`V_ll_pk = √3 · 3.13e−3 · 104.7 ≈ 0.57 V` — comfortably measurable on
any scope; use the drill's top speed for more signal. Repeat at 3+
speeds; Kt is the slope of `V_ll_pk/√3` vs Ω (the intercept must pass
through zero — a nonzero intercept means waveform-reading error).

Bind it:

```rust
let m = Measurement {
    name: "torque_nm".into(),
    value: kt_measured * 1.5,          // claim is torque at 1.5 A peak
    uncertainty: kt_sigma * 1.5,
    instrument: "back-EMF, scope s/n …, drill spin".into(),
    band_factor: 1.35,
};
```

## Procedure 2 — Kt via stall torque (cross-check)

1. Lock the electrical angle: drive DC current `I` into phase A, out of
   phases B ∥ C (the classic locked-vector trick; this vector's torque
   constant equals the sinusoidal Kt at best angle to first order).
2. Arm on the rotor (a stiff 3D-printed lever, length `L` from axis)
   pressing on a 0.01 g kitchen/jewelry scale: `T = m·g·L`.
3. Sweep I = 0.5…2.5 A (2 oz copper: keep duty short above 2 A), fit
   the slope. The prediction says the line is straight (linear
   materials at these fields) — curvature in the data is itself a
   finding.

## Procedure 3 — air-gap flux with a Hall probe

A TLV493D / SS49E-class linear Hall sensor on a 0.8 mm flex or a bare
die fits the 1.0 mm gap (alternatively: measure at the magnet face with
the rotor removed and bind against a face-value solve). Log B while
slowly rotating the rotor one revolution: the peak under each pole
center binds `airgap flux`; the six peaks' spread measures magnet
lot-to-lot variation the model treats as uniform (0.39 T).

## Procedure 4 — spin-down (friction context, not a claim binder)

Open-circuit spin-down from ~2000 RPM, log the back-EMF envelope decay:
gives the friction torque curve that the self-start margin used
(bearing spec 608-ZZ vs 2RS mattered by 6.7× in the design record).
Shorted-phase spin-down decays faster by the `3·Kt²·Ω/(2R_ph)` braking
term — a second, instrument-free Kt estimate once `R_ph` is measured
with a milliohm-capable meter.

## What deliberately has no confident prediction

Phase inductance and resistance (LCR meter, 1 kHz): the 2D slice does
not model end turns or trace resistance, which dominate both numbers on
a PCB winding. Measure them for the record; binding them to a slice
prediction would manufacture a Violated verdict the model already
declares. They become bindable when the seam gains the board's real
trace geometry (kernel-side extraction, the flagged follow-up).

## Failure semantics

`compare()` is fail-closed: an unmeasured receipt never reads as
passing, a measurement that names no claim is an error, and a Violated
verdict with a clean procedure is the model telling us which omission
(curvature, fringing, Br tolerance) to price next — exactly the loop
the particle crate's experiment pack established.
