//! Electromagnetics — the named domain, in one place.
//!
//! vcad's EM capability grew up scattered: spiral-inductance math landed with
//! the PCB-motor work, transmission-line impedance with the routing stack,
//! the air-gap solver with motor co-design. This module names the collection
//! **electromagnetics** and gives it a single front door. It re-exports the
//! existing modules unchanged — no paths move, nothing is deprecated — so
//! `vcad_ecad_sim::magnetics` and `vcad_ecad_sim::em::magnetics` are the same
//! module.
//!
//! # What the domain can claim today
//!
//! Every solver here is a closed-form, first-order model. Each one makes a
//! small set of quantitative *claims* — a predicted value for a named physical
//! quantity, from named inputs, by a citable method:
//!
//! | quantity | symbol | unit | solver | method |
//! |---|---|---|---|---|
//! | spiral inductance | L | H | [`magnetics::coil_inductance_henry`] | modified Wheeler (Mohan et al. 1999) |
//! | torque constant | Kt | N·m/A | [`magnetics::motor_torque_constant`] | kw·N·p·B·A_pole (first-order) |
//! | back-EMF constant | Ke | V·s/rad | [`motor::evaluate_motor`] | Ke = Kt (SI) |
//! | no-load speed | ω0 | rad/s | [`motor::evaluate_motor`] | V/Ke, linear DC model |
//! | stall torque | Ts | N·m | [`motor::evaluate_motor`] | Kt·V/R, linear DC model |
//! | air-gap flux density | B_gap | T | [`airgap::airgap_flux_density`] | MEC reluctance network |
//! | fringing derate | k_f | — | [`airgap::fringing_derate`] | Carter-like w/(w+2g) pole-edge fringe |
//! | induction gap field | B1 | T | [`induction::evaluate_thin_sheet_induction`] | rotating-MMF fundamental, μ0·F1/g |
//! | torque per unit slip | K | N·m | [`induction::evaluate_thin_sheet_induction`] | thin-sheet eddy torque, Russell–Norsworthy end effect |
//! | characteristic impedance | Z0 | Ω | [`impedance`] | IPC-2141 / Hammerstad–Jensen |
//! | differential impedance | Zdiff | Ω | [`impedance`] | 2·Z0·k edge-coupling factor |
//! | effective permittivity | εr_eff | — | [`impedance`] | Hammerstad–Jensen |
//! | propagation delay | td | ps | [`signal_integrity::propagation_delay`] | 3.336·√εr_eff ps/mm |
//! | crosstalk | NEXT/FEXT | dB | [`signal_integrity::estimate_crosstalk`] | empirical 1/(1+(s/h)²) coupling |
//!
//! The MCP calculators (`calc_coil`, `calc_impedance`, `size_coil`,
//! `size_impedance`, `winding_layout`) mirror these closed forms in
//! TypeScript; `calc_motor` (PM mode) calls [`motor::evaluate_motor`] and
//! [`airgap::airgap_flux_density`] through the kernel WASM, and mirrors the
//! [`airgap::fringing_derate`] and [`induction`] closed forms in TypeScript
//! (induction mode); `calc_rf` (RLC resonance/Q), `size_pdn` (IR-drop
//! resistor mesh), and `check_self_start` (torque-vs-bearing-friction margin,
//! mechanical rather than EM) are TS-side solvers with no Rust twin yet. Each calculator emits its predictions as **receipt
//! claims** — *quantity, predicted value, unit, method, inputs* (see
//! `packages/mcp/src/tools/em-claims.ts`). A claim is honest about being a
//! model — first-order, no slotting/fringing/saturation, no field solver — so
//! a future measurement or FEA pass can grade it rather than contradict it.
//!
//! # Differentiable by construction
//!
//! The geometry→performance leaves ([`magnetics`], the [`impedance`] Z0
//! functions) are generic over `tang::Scalar`, so the same code that computes
//! an `f64` builds a `tang_expr` graph and differentiates symbolically.
//! `vcad-ecad-diff` consumes these leaves for motor plant/controller
//! co-design; `size_impedance` solves trace geometry by gradient, not search.
//!
//! # Neighbors, deliberately outside the domain
//!
//! - [`crate::thermal`] — junction temperature and via thermal resistance:
//!   thermal, not electromagnetic (though copper geometry feeds both).
//! - [`crate::circuit`] — lumped-element MNA transient simulation: circuit
//!   behavior over time, not field/geometry claims.
//!
//! # Future
//!
//! Antenna synthesis, filter synthesis, and eventually a field solver join
//! here. The domain rides the PCB rail — coils are copper — so everything it
//! designs is already purchasable as a board.

pub use crate::airgap;
pub use crate::impedance;
pub use crate::induction;
pub use crate::magnetics;
pub use crate::motor;
pub use crate::signal_integrity;
