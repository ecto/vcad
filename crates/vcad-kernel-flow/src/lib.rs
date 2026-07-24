#![warn(missing_docs)]

//! Internal-flow CFD for the vcad kernel (M0).
//!
//! Answers the questions every duct, manifold, and cooled enclosure asks —
//! *how much flow do I get, and what pressure does it cost?* — with two
//! independent routes to the same numbers instead of one unexamined mesh:
//!
//! 1. [`model::FlowModel`] — a bounding box divided into a uniform cubic
//!    voxel grid; every voxel is solid, fluid, inlet, or outlet. Geometry
//!    arrives either as painted regions ([`spec`]) or as an
//!    externally-voxelized occupancy vector (the same seam
//!    `vcad-kernel-thermal` uses, so a conjugate solve shares one grid).
//! 2. [`solve::solve_steady`] — a D3Q19 lattice-Boltzmann solver: BGK
//!    collision with Guo forcing, half-way bounce-back walls (second-order
//!    wall placement), velocity inlets via moving-wall bounce-back, and
//!    anti-bounce-back pressure outlets. Run to a steady state detected on
//!    the velocity field, never assumed.
//! 3. [`lumped`] — the closed-form route: Poiseuille and rectangular-duct
//!    pressure drop, Darcy–Weisbach with laminar `f = 64/Re`, Borda–Carnot
//!    expansion loss, hydraulic diameter. Both a feature (instant answers
//!    at design time) and the field solver's conscience: the receipt
//!    carries the gap between the two routes as `cross_route_residual`.
//! 4. [`receipt`] — `vcad.flow-claims/1`: pressure drop, flow rate, and a
//!    mass-balance audit, all `basis: "predicted"`, with full solver
//!    provenance (grid, relaxation time, lattice Mach number, Reynolds
//!    number and its envelope).
//!
//! **Scope and honesty (M0):** single-phase, incompressible, isothermal,
//! **laminar** internal flow. The solver refuses — fails closed with the
//! computed Reynolds number in the error — above the validated laminar
//! envelope rather than producing a plausible turbulent-looking wrong
//! answer. LBM is weakly compressible: pressure fields carry acoustic
//! noise of order Ma², which the scaling keeps below the claim tolerance.
//! No turbulence models, no free surfaces, no non-Newtonian fluids, no
//! external aerodynamics. Thermal transport in the fluid is M1; the
//! conjugate seam to `solve_thermal` is M2; buoyancy is M3; adjoints are
//! M4. See `docs/flow-m0.md`.
//!
//! Units: geometry in millimeters (house convention), fluid properties
//! and results in SI (Pa, m³/s, m/s).

pub mod lattice;
pub mod lumped;
pub mod model;
pub mod receipt;
pub mod solve;
pub mod spec;
