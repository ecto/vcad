# Astrodynamics M0: exact Kepler + J2 propagation, and the sky as the bench

`vcad-kernel-orbit` starts the orbital-mechanics ladder. The incumbents are
STK (closed, expensive) and GMAT (capable, clunky, non-differentiable,
not agent-native). The domain's unique draw for the receipt program:
**ground truth is free and always on.** Public TLEs and JPL Horizons
ephemerides are a continuously-updating measurement stream, so the
predicted-vs-measured loop that every other solver crate needs bench
hardware for closes here with zero hardware. The `position_error_km`
claim in `vcad.orbit-claims/1` is the first claim in the workspace born
with `basis: measured` — measured against the actual ISS.

## M0 scope (and honesty)

**In scope:**

- Elements ↔ state (Vallado 4th ed., Algorithms 9/10), exact elliptic
  Kepler propagation (Newton on M = E − e·sin E, fail-closed on
  non-convergence).
- Fixed-step RK4 on Cartesian state, force model two-body or
  two-body + J2 (Vallado Eq. 8-30).
- Closed-form J2 secular rates (Eqs. 9-38/9-39), sun-synchronous
  inclination design, vis-viva, periods.
- GMST Earth rotation, sub-satellite points, WGS84 sites, topocentric
  elevation, rise/culminate/set pass prediction (scan + bisection).
- Fixture parsers: NORAD TLE (checksum-verified, fail-closed) and JPL
  Horizons vector tables (raw text retained → provenance checked in).
- `vcad.orbit-claims/1` receipts: predicted claims roll up Provisional,
  never Pass; the sky comparison is a Measured Pass/Fail against a stated
  error budget.

**Stated approximations** (each on the receipt's `frame_note`):

- ICRF treated as an inertial Earth-equator frame (2000↔2026 precession
  ≈ 0.36° ignored — visible in the data: the fixture's ICRF inclination
  differs from the TLE's true-of-date 51.6316° by ~0.07°, and the tests
  comment on it).
- GMST-only Earth rotation; no polar motion or equation of equinoxes.
- TDB − UTC = 69.184 s constant; UT1 − UTC ignored.
- **No drag, SRP, third-body, or harmonics beyond J2.** Pass times are
  honest to ±minutes, not ±seconds.
- TLE mean elements are never fed to the osculating propagator (that
  classic mistake costs tens of km immediately); the TLE fixture is used
  for cross-checks only until the M1 SGP4-compatibility mode.

**Units:** km, km/s, s, rad, `f64`, everywhere; degrees only in
`*_deg`-suffixed reporting fields. Loudly documented in the crate docs
because unit confusion is this field's classic bug.

## Validation ladder (all in `cargo test -p vcad-kernel-orbit`)

- Elements ↔ state round trip; hyperbolic states rejected.
- Geostationary period anchor (42 164 km → 86 164 s), LEO vis-viva speed.
- Kepler solver residual < 1e-12 across e ∈ [0, 0.95]; full-period return.
- Two-body RK4: energy + angular-momentum drift < 1e-9 relative over 10
  orbits (measured floor ≈ 3e-10); **analytic Kepler is the oracle** —
  RK4 at dt = 1 s agrees to < 1 m over 10 orbits.
- J2 RK4: the J2-inclusive energy and h_z are conserved (< 1e-9), the node
  measurably regresses.
- **Headline:** least-squares nodal drift over 10 orbits vs Vallado
  Eq. 9-38 to < 1% at i = 51.63°, 30°, 98.2°; sign flip across polar.
- Sun-synchronous anchors: **98.19° at 700 km**, 97.79° at 600 km.
  (Recon folklore said "~97.8° at 700 km" — that value belongs to
  ~600 km; the formula overruled the prompt, and the test asserts both.)
- Critical inclination 63.4349° zeroes the apsidal rate.
- GMST at J2000 = 280.46062°; zenith elevation = 90°; JD round trips.
- TLE: parses the checked-in ISS set, checksum corruption and truncation
  fail closed.
- Ephemeris: 865 rows parsed, every row a bound LEO state; TLE and
  Horizons agree on a to < 20 km and i to < 0.5° (TEME vs ICRF).

## The flagship: `cargo run --release -p vcad-kernel-orbit --example iss_pass`

Real ISS state (Horizons, 2026-07-17 00:00 TDB) propagated J2-only and
held against the real sky for 72 h:

| hours | J2 err (km) | two-body err (km) |
|---:|---:|---:|
| 1 | 0.44 | 11.5 |
| 6 | 2.33 | 142.7 |
| 12 | 4.53 | 216.0 |
| 24 | **9.77** | 486.8 |
| 48 | 24.8 | 1004.8 |
| 72 | 39.6 | 1524.8 |

Reading: J2 buys **50× at 24 h** over point-mass gravity against reality.
The remaining ~10 km/day of growth is the honest model gap — dominantly
drag (along-track), plus higher harmonics — and is exactly what M1 buys
back. The regression budgets are set at measured × ~2.5 (2 / 8 / 25 km at
1 / 6 / 24 h): if a change doubles the model gap, CI fails.

Pass prediction (San Francisco, mask 10°, next 24 h): 4 passes, e.g.
rise 2026-07-17 08:03:17 UTC, max elevation 75°, 6.8 min — ±minutes
honesty stated in-line.

Receipt: 6 predicted claims + 1 sky-measured claim
(`orbit.position_error_km_at_24h` = 9.8 km vs 25 km budget → Pass,
`basis: Measured`); overall verdict **Provisional** by design (predicted
claims never roll up Pass).

## Prior art

`sgp4` on crates.io (2.4.0, ~272k downloads) is a solid pure-Rust SGP4.
We deliberately did not reimplement SGP4 at M0 — the value here is the
exact/J2 propagator with receipts, headed differentiable, co-designed
with the thermal/antenna/neutronics crates already in the workspace (a
receipted smallsat: link budget from `vcad-kernel-antenna`, eclipse
thermal from `vcad-kernel-thermal`, radiation from
`vcad-kernel-neutronics`, all priced on one orbit).

## Milestone ladder

- **M0 — this.** Exact Kepler + J2 RK4, secular rates, passes, fixtures,
  sky-measured receipts.
- **M1 — the model gap.** Exponential-atmosphere drag, SGP4-compat mean
  element handling (or a seam to the `sgp4` crate), TEME↔ICRF rotation;
  target: ≤ ~2 km at 24 h vs the fixture, pass times to ±seconds.
- **M2 — differentiable.** ∂state/∂(elements, drag, thrust) via adjoint
  or tangent-linear RK4; station-keeping ΔV optimization is the payoff
  (the same optimizer seam every other crate uses).
- **M3 — co-design.** Orbit-aware receipts for the smallsat story above;
  MCP tools (`propagate_orbit`, `predict_passes`) registered in
  `vcad-receipt`/MCP like the wave-2 solver tools.

## Non-goals

No operational conjunction assessment, no maneuver planning against live
catalogs, no interplanetary — LEO/MEO/GEO bound orbits around one Earth,
honestly priced.
