# Differentiable circuit simulation — M0

Circuit simulation that can tell you **d(output)/d(every component value)**
from one extra linear solve, with fail-closed receipts. The incumbent is
SPICE (Nagel & Pederson 1973; SPICE2: Nagel, UCB ERL-M520, 1975) — superb,
ubiquitous, and non-differentiable. The gap this work fills is *not*
simulation quality (ngspice exists and is good); it is:

1. **Exact adjoint sensitivities** — the transposed-network method
   (Director & Rohrer, IEEE Trans. Circuit Theory CT-16, 1969), so a whole
   gradient costs one solve instead of one-solve-per-component.
2. **Fail-closed receipts** — `vcad.spice-claims/1`, predicted basis,
   Provisional rollup, provenance on every number.
3. **Agent-native workspace integration** — the module lives in
   `vcad-ecad-sim`, next to the PCB stack that produces the netlists.

## Where it lives (the extend-vs-new decision)

`vcad_ecad_sim::circuit` — the pre-existing MNA transient module was
extended, not wrapped in a new `vcad-kernel-spice` crate. The reasons are
logged in `docs/research-log.md` (2026-07-17): the module already had the
MNA core, companion models, Newton + `pnjlim`, and a live WASM consumer
(`vcad-kernel-wasm::circuit_sim`) that inherits every improvement.

## What M0 ships

| piece | module | notes |
|---|---|---|
| DC operating point | `circuit::dc` | Newton–Raphson, gmin stepping (1e-3 → 1e-12 → **0**: the aid is removed before the answer is reported), C open / L short, fail-closed on non-convergence |
| Trapezoidal integration | `circuit::Integrator` | SPICE2's default method, opt-in (`set_integrator`); BE stays the default for existing consumers. First step always BE (startup consistency). Verified 2nd-order: halve dt → quarter error |
| AC analysis | `circuit::ac` | complex MNA, hand-rolled `(re, im)` (`Cplx`), inductors in branch form so ω = 0 degrades to the DC short; diodes linearized at the DC op point |
| **Adjoint sensitivities** | `circuit::adjoint` | DC (implicit function theorem through the converged Newton system, incl. d/dIs of the diode) and AC (transposed complex system — plain transpose, not conjugate). FD-validated on every element kind |
| Tellegen gate | `CircuitEnv::power_balance` | Σ v·i at every timestep < 1e-9 of source power, both integrators, in CI |
| Receipts | `circuit::receipt` | `vcad.spice-claims/1`: dc_node_voltages, cutoff_hz, q_factor, power_balance_residual; `design_claims` adapter → unified receipt, Provisional rollup |
| Flagship | `examples/filter_autotune` | adjoint-driven RLC design to a 10 kHz / Q = 1/√2 Butterworth target; J: 1.16e-1 → 5.8e-17, cutoff and Q exact to < 0.1% |

## Validation ladder (`tests/circuit_validation.rs`)

| rung | oracle | result |
|---|---|---|
| voltage divider | Ohm's law | exact to 1e-13 |
| RC step | V·(1 − e^{−t/RC}) | < 5 µV on 5 V at dt = τ/1000; error ratios 4.0/4.0 on dt halving (trap), 2.0 (BE) — both orders verified |
| RLC ringdown | ω_d = ω₀√(1 − 1/4Q²), α = R/2L | frequency < 1e-3 rel, envelope < 2e-2 rel |
| diode + R | Lambert-W closed form (Corless et al. 1996), Halley iteration | operating-point current < 1e-9 rel |
| Tellegen | Σ v·i = 0 | < 1e-9 rel at every one of 2000 steps × 2 integrators |
| adjoint vs FD | central differences, frozen network | < 1e-5 rel on every element kind (DC and AC), < 1e-4 through the diode Newton system |

## M1 progress (2026-07-17): transistors

M1 item 1 landed. `Device::Mosfet` (Shichman–Hodges level-1: cutoff /
triode / saturation, kp / vt0 / λ, source-drain swap for vds < 0 — SPICE2,
UCB ERL-M520, 1975, §2) and `Device::Bjt` (Ebers–Moll transport form:
Is / βF / βR), both polarities (NMOS/PMOS, NPN/PNP) via the sign
transformation. Wired everywhere: transient Newton (`pnjlim` on both BJT
junctions, step-clamped FET voltages), DC (gmin ladder verified on a real
CMOS inverter transfer curve), AC (gm/gds/gπ/gµ at the DC op point), and
the DC adjoint (d/dkp + d/dvt0, d/dIs + d/dβF via `gradient_aux`,
FD-validated to < 1e-4). The Tellegen gate learned multi-terminal power
(`Device::power`) — a BJT's base current carries power the (c, e) pair
alone would miss — and holds < 1e-9 with transistors in the network.
The M0 AC-diode placeholder is closed: `AcSensitivity` diode slots now
carry the full operating-point chain term (∂H/∂g_d from the AC adjoint ×
dv_d/dp from the DC adjoint), FD-validated; `deferred` now lists exactly
the transistors (their AC sensitivities need model second derivatives).
New validation rungs: MOSFET saturation vs the square law (1e-9),
common-source gain vs −gm·(Rd ∥ ro) (1e-9), BJT current-mirror ratio vs
1/(1 + 2/βF).

## Honesty (what is not claimed)

- **Transistor models are level-1 / Ebers–Moll only.** No body effect, no
  subthreshold, no capacitances (transistors are memoryless here), no
  Early effect (BJT), no Gummel–Poon. Good for topology-level design and
  gradients, not for nanometer accuracy.
- **Transistor AC sensitivities are deferred placeholders** (flagged in
  `AcSensitivity::deferred`): they need d(gm, gds)/d(op point) — second
  derivatives of the models. Diode AC sensitivities are now computed,
  including the operating-point chain term.
- **No transient adjoint** — DC and AC gradients only. Transient adjoint
  (reverse sweep over companion states) is M1.
- **Fixed timestep at M0** — LTE-based adaptive stepping flagged for M1.
- **No noise, no Monte Carlo.** `vcad-kernel-tolerance` is the natural
  partner: its stackup engine consumes exactly the sensitivities the
  adjoint produces — component-tolerance yield of a circuit by gradient
  instead of sampling is the killer combo, unbuilt.
- **Motor devices are rejected** by DC/AC analysis (their DC state couples
  to a mechanical equilibrium; transient handles them as before).

## Bench-closing instruments

The claims are `basis: "predicted"` and roll up Provisional. The closing
instruments are deliberately cheap: a ~$30 USB oscilloscope + signal
generator measure DC operating points (multimeter), step responses (scope),
and |H| Bode sweeps (generator + scope) directly. The measurement pack that
binds them is M-next, mirroring the antenna/EM measurement packs.

## M1 ladder

1. ~~MOSFET level-1 (Shichman–Hodges) + BJT Ebers–Moll, with the same
   FD-validated adjoint treatment.~~ **Done** (see "M1 progress" above);
   transistor *AC* sensitivities remain deferred.
2. Transient adjoint (reverse sweep) → time-domain objectives (settling
   time, overshoot) become differentiable.
3. LTE-based adaptive timestep (with the frozen-discretization caveat for
   gradient runs, per the particle crate's scar tissue).
4. ~~Netlist-from-ecad seam~~ — **DONE** (`circuit::netlist`, 2026-07-17):
   `circuit_from_schematic` maps a `SchematicSheet` (components + nets via
   `vcad_ecad_schematic::generate_netlist`) onto `Circuit` nodes/devices.
   Refdes-prefix convention (R/C/L/V/I/D), SI value parsing (`4.7k`,
   `100n`, `4R7`, case-sensitive `m`/`M`), ground-family nets → node 0,
   fail-closed per-component blockers for ICs/connectors with an explicit
   `stub_as_open` allowlist. Round-trip validated against the ladder's
   closed forms (DC divider, AC RC corner, both to 1e-12). **No layout
   parasitics** — trace R/L/C extraction from the routed board is M2.
   The `simulate_circuit`/`tune_circuit` MCP tools (item 6) should accept
   `{document_id}` and ride this seam.
5. Tolerance-yield bridge to `vcad-kernel-tolerance`.
6. MCP tools (`simulate_circuit`, `tune_circuit`) riding the WASM binding.
