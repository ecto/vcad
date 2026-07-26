#![warn(missing_docs)]

//! Static structural FEA for the vcad kernel (M0–M2).
//!
//! Answers the everyday question — *will this bracket break?* — with a
//! defensible number: a linear-elastic solve on the part's real geometry,
//! returning the maximum von Mises stress, the maximum displacement, and
//! (given a yield strength) a safety factor.
//!
//! The pipeline:
//!
//! 1. [`mesh::tet_fill`] — the part's tessellated boundary
//!    ([`vcad_kernel_tessellate::TriangleMesh`]) is filled with a uniform
//!    lattice of linear tetrahedra: interior voxels are found by per-column
//!    ray parity against the surface mesh, and each interior cell is split
//!    into six tets (Kuhn decomposition, face-conforming across cells).
//!    This is a *structured* fill — the boundary is staircase-approximated
//!    at the lattice pitch, which is exactly what the convergence gate in
//!    step 3 prices. A boundary-conforming Delaunay mesh is the M3+
//!    upgrade (see `docs/fea-m0.md`).
//! 2. [`solve::solve_static`] — constant-strain-tet linear elasticity,
//!    assembled matrix-free (per-element shape gradients, ~100 B/tet) and
//!    solved with Jacobi-preconditioned conjugate gradients at unit
//!    Young's modulus (linearity lets displacement rescale by 1/E while
//!    stress is E-independent for force-driven loads). Loads and supports
//!    are axis-aligned box regions selecting boundary nodes — fail-closed:
//!    a region that selects no nodes is an error, never a silent no-op.
//! 3. [`convergence::analyze_converged`] — the same solve at two or more
//!    lattice refinements. QoIs must agree across levels within stated
//!    tolerances or the result is **Unverifiable** (fail-closed, per the
//!    thermal/particle house contract); the reported discretization-error
//!    estimates are the inter-level relative changes.
//! 4. [`receipt`] — `vcad.fea-claims/1` predicted claims with full solver
//!    provenance, translated onto the unified `vcad.receipt/1` schema with
//!    `basis: predicted` — a receipt built from them rolls up
//!    **Provisional, never Pass**, because these describe hardware nobody
//!    has tested yet.
//!
//! **Scope and honesty:** small-displacement linear elasticity of a single
//! isotropic material. No plasticity, no buckling, no contact, no dynamic
//! loads; constant-strain tets smear stress concentrations, so
//! `max_von_mises` at a re-entrant corner is a lower bound that grows with
//! refinement (the true elastic solution is singular there). Every claim
//! note says so. Adjoint gradients are deliberately out of scope for this
//! pass (M3+).
//!
//! **Thin walls are a different discretization, not a finer one.** A
//! staircase lattice needs several cells through the thinnest load-bearing
//! section, and a sheet-metal or tube-frame member does not give it that at
//! any resolution the cost cap allows: a 2 mm wall on a 312 mm member wants
//! a 0.33 mm pitch, i.e. ~950 cells along the longest axis. So the lattice
//! path *measures* the part ([`mesh::wall_thickness`]), and when the pitch
//! cannot resolve the section it fails closed with the cell arithmetic and
//! a route forward already spelled out ([`mesh::diagnose_thin_wall`]) —
//! failing without a route is what costs the caller an afternoon.
//!
//! The route forward for a prismatic member is [`section`]: closed-form
//! section properties and beam/torsion checks (Bredt thin-wall torsion, the
//! Saint-Venant series for solid rectangles, Euler buckling), gated on
//! their own applicability and carrying the same `vcad.fea-claims/1`
//! predicted-basis claims. For a constant cross-section that is not a
//! fallback — it is the more accurate answer, and it costs microseconds.
//! Shell and beam *elements*, which would cover non-prismatic thin-walled
//! parts, remain the honest M3+ upgrade.
//!
//! Units: geometry in **millimeters**, forces in Newtons, moduli and
//! stresses in MPa (N/mm²) — the consistent mm-N-MPa system.

pub mod convergence;
pub mod mesh;
pub mod receipt;
pub mod section;
pub mod solve;
pub mod spec;
