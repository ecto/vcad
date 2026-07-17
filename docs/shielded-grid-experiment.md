# Shielded-grid IEC experiment pack

The bench program that closes the loop on `vcad-kernel-particle`: build
the two-ring magnetically shielded cathode, measure the two numbers the
simulation predicts — **cathode interception current vs shield current**
(Phase A, no neutrons required) and **neutron rate** (Phase B) — and bind
them to the predicted claims with `receipt::compare` (verdicts: Holds /
Violated / Unmeasured, fail-closed).

All prices are order-of-magnitude estimates (used-market heavy) and
flagged as such, per vcad BOM convention. This document is a plan, not an
authorization; the safety section is load-bearing.

## The two headline measurements

1. **Phase A — the ammeter curve.** At −1…−3 kV bias and 0.5–5 kA·turns
   pulsed shield current, cathode interception current must fall with
   shield current along the simulated −3 kV curve (interception 1.00 →
   ~0.5 by ~40 kA·t; partial shielding is already unambiguous at a few
   kA·t at low bias). This is the entire shielded-grid hypothesis tested
   with an ammeter and an oscilloscope — no vacuum heroics, no neutron
   instrumentation, no high voltage.
2. **Phase B — neutrons.** At −30…−40 kV and 1–10 mA with D₂ at ~2 mTorr:
   the beam-on-background floor is ~10⁵ n/s-scale (chain and volume
   ionization should carry the measured value above it — the receipt's
   `band_factor` on the neutron claim is generous and one-sided-honest:
   the prediction is a floor).

## Bill of materials (estimates)

### Vacuum
| Item | Example | Est. |
|---|---|---|
| Chamber, ~150 mm radius equivalent | 8" spherical octagon / CF cross (Kurt J. Lesker or used) | $800–3,000 |
| Turbo pump station, ~70–80 L/s | Pfeiffer HiCube 80 (used) or turbo + dry backing | $2,000–6,000 |
| Wide-range gauge | MKS 972B DualMag or Pirani + cold cathode pair | $700–1,500 |
| D₂ supply | lecture bottle + regulator + precision leak valve (or MFC) | $600–1,500 |
| Fittings, viewport (leaded glass later), gaskets | — | $500–1,000 |

### High voltage and shield current
| Item | Example | Est. |
|---|---|---|
| HV supply, −40 kV / 10 mA, current-limited | Glassman/Spellman (used) | $1,000–3,000 |
| HV feedthrough, 40 kV CF | Lesker EFT series | $400–800 |
| Ballast resistor chain + HV divider probe | — | $300–600 |
| Shield-current pulser (Phase A) | 450 V electrolytic bank + IGBT/SCR + 100-turn ring formers → kA·turn ms pulses | $500–1,500 |
| **Floating isolation for the pulser** | isolation transformer rated ≥ 50 kV or battery pack + fiber-optic trigger | $500–2,000 |

The pulser floats at cathode potential — the rings are simultaneously the
−40 kV electrode and the coil. This is the hardest engineering item in
the machine and the reason Phase A runs at low bias first (at −1 kV the
isolation problem is trivial; at −40 kV it is the design). A REBCO
persistent-current variant is the Phase C upgrade path and changes this
table (cryocooler budget class).

### Cathode assembly
Two ring formers (copper or SS tube), multi-turn winding, alumina
standoffs, spot-welded joints; geometry from the optimizer output
(`optimize_shield`: ring radius 45 mm-class, spacing per the current
optimum — regenerate before freezing drawings). Design the assembly in
vcad; the drawings and the `DeviceSpec` JSON must come from the same
document so the sim and the hardware share provenance.

### Diagnostics
| Item | Example | Est. |
|---|---|---|
| **Cathode ammeter (the Phase A instrument)** | shunt + isolation amplifier, optically coupled, floating at cathode | $200–600 |
| Neutron counter | He-3 or B-10 proportional counter + preamp (used) | $1,000–3,000 |
| Bubble dosimeters (calibration + backup) | BD-PND type | ~$300 each |
| CR-39 track detectors (passive backup) | — | $100–300 |
| X-ray survey meter | Ludlum (used) | $300–800 |
| Scope, HV probes, thermocouples | — | $500–1,500 |

**Total: roughly $10k–25k**, used-market dependent, Phase A alone ≈ $2–5k
(chamber can be rough-vacuum for the ammeter curve at glow pressures).

## Safety (non-negotiable, incomplete by design — get review)

- **X-rays:** any run above ~20 kV produces bremsstrahlung; viewports are
  the leak path (leaded glass or steel blanks), survey meter on every
  voltage increase, interlocked enclosure.
- **HV:** single-point ground, grounding stick discipline, no lone
  operation, supply current limit set to the experiment's need.
- **Neutrons:** dose budget and distance plan before Phase B; detectors
  calibrated; logbook doses.
- **Gas:** D₂ is flammable; lecture-bottle handling, no accumulation
  paths.
- **Regulatory:** D-D amateur fusion is legal in the US (no special
  nuclear material), but radiation-dose and local rules apply — verify
  before Phase B; fusor.net norms are the community baseline.

## The receipt loop

1. Freeze the design: `DeviceSpec` JSON + drawings from the same vcad
   document; `receipt::predicted_claims` at the planned operating point,
   committed alongside the build.
2. Phase A: measure interception fraction vs shield current; bind with
   `receipt::compare` (tight `band_factor` ~1.5 on interception — it is a
   direct observable).
3. Phase B: neutron rate with calibrated counter; generous one-sided
   band on the floor claim; publish the comparison report either way —
   Violated is a publishable result about the model, Holds is a
   publishable result about the machine.
4. Every claim that stays `Unmeasured` is listed as such in anything we
   publish. Fail-closed applies to press releases too.
