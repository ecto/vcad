//! Discrete adjoint of a mode-overlap transmission objective (TM).
//!
//! Computes exact-to-discretization gradients of
//!
//! ```text
//! J = |A|²,   A = Σ_m w_m·Êz_m(ω),   Êz_m = Σ_n Ez_m^n·e^{iω·n·dt}·dt
//! ```
//!
//! (the DFT mode-overlap amplitude on a vertical monitor line — the
//! transmission objective of inverse design, up to a fixed input-power
//! normalization) with respect to the permittivity of **every Ez sample
//! in a design region**, at the cost of one extra FDTD run.
//!
//! # Why the adjoint run is just another `Simulation`
//!
//! Write one leapfrog step as `u^{n+1} = M·u^n + s`, u = (E, H). The
//! gradient chain λ^n = Mᵀ·λ^{n+1} + ∂J/∂u^n runs the *transposed* system
//! backward. On the Yee grid with PEC walls the staggered curls satisfy
//! `C_H = C_Eᵀ` in plain ℓ² (the same identity behind the exact energy
//! invariant of [`crate::sim::Simulation::step_measuring_energy`]), and
//! substituting `φ = ε⁻¹·λ_E`, `Ψ = −λ_H`, `m = N − n` turns the
//! transposed recursion into **exactly the forward stepper** — H update,
//! then E update with 1/ε — driven by soft E sources at the monitor line:
//!
//! ```text
//! s_adj,m(step q) = (2·dt/ε_m)·w_m·Re[conj(A)·e^{iω·(N−q)·dt}]
//! ```
//!
//! The gradient is then the exact time-domain pairing of the adjoint
//! field with the stored forward increments:
//!
//! ```text
//! dJ/dε_i = −Σ_{q=0}^{N−1} Φ_i^q · (Ez_i^{N−q} − Ez_i^{N−q−1})
//! ```
//!
//! (Φ^q = adjoint Ez after q adjoint steps; forward Ez snapshots are
//! stored over the design region only.)
//!
//! # Exactness and its edges (stated, not hidden)
//!
//! - The interior transposition is exact to rounding. The CPML slabs are
//!   **not** transposed — the adjoint reuses the same absorber, standard
//!   practice justified by the measured −95 dB reflection floor; the
//!   induced gradient error is at that order and is absorbed into the
//!   finite-difference validation tolerance.
//! - The design region must not overlap sources, monitors, or CPML
//!   (asserted): the increment formula drops source terms and CPML ψ
//!   corrections inside the region.
//! - The run length is **frozen**: the same `steps` for forward, adjoint,
//!   and every finite-difference probe — comparing runs whose windows
//!   differ contaminates gradients (the `vcad-kernel-particle` lesson,
//!   encoded here as a single shared parameter).
//!
//! Validated against central differences cell by cell in
//! `tests/validation.rs::adjoint_gradient_matches_finite_differences`.

use crate::grid::Field2;
use crate::monitor::Cplx;
use crate::sim::{Polarization, Simulation};
use crate::source::SourcePlacement;

/// A rectangular design region of Ez nodes, inclusive on all sides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DesignRegion {
    /// First node column.
    pub i0: usize,
    /// Last node column (inclusive).
    pub i1: usize,
    /// First node row.
    pub j0: usize,
    /// Last node row (inclusive).
    pub j1: usize,
}

impl DesignRegion {
    /// Node count along x.
    pub fn ns_x(&self) -> usize {
        self.i1 - self.i0 + 1
    }

    /// Node count along y.
    pub fn ns_y(&self) -> usize {
        self.j1 - self.j0 + 1
    }

    /// Total cell count.
    pub fn len(&self) -> usize {
        self.ns_x() * self.ns_y()
    }

    /// True if the region is empty (never, by construction).
    pub fn is_empty(&self) -> bool {
        false
    }
}

/// The mode-overlap objective: `J = |Σ_m w_m·Êz(i, j0+m)(freq)|²` on a
/// vertical line of Ez nodes.
#[derive(Debug, Clone, PartialEq)]
pub struct ModeOverlap {
    /// Monitor node column.
    pub i: usize,
    /// First Ez row.
    pub j0: usize,
    /// Per-row overlap weights (typically the eigenmode profile).
    pub weights: Vec<f64>,
    /// Objective frequency.
    pub freq: f64,
}

/// Everything the adjoint pass returns.
#[derive(Debug, Clone)]
pub struct GradientResult {
    /// Objective value J = |A|².
    pub objective: f64,
    /// The overlap amplitude A.
    pub amplitude: Cplx,
    /// dJ/dε at each design-region Ez sample (region-local row-major,
    /// `ns_x × ns_y`).
    pub grad: Field2,
    /// The region the gradient covers.
    pub region: DesignRegion,
}

/// Run a forward simulation for `steps` and return `(J, A)` for the
/// objective — the plain evaluation used by finite-difference probes.
pub fn run_objective(mut sim: Simulation, obj: &ModeOverlap, steps: usize) -> (f64, Cplx) {
    assert_eq!(sim.polarization(), Polarization::Tm);
    let dt = sim.dt();
    let omega = 2.0 * std::f64::consts::PI * obj.freq;
    let mut a = Cplx::ZERO;
    for n in 0..steps {
        sim.step();
        let t = (n as f64 + 1.0) * dt;
        let ph = Cplx::cis(omega * t).scale(dt);
        for (m, w) in obj.weights.iter().enumerate() {
            a = a + ph.scale(w * sim.ez_at(obj.i, obj.j0 + m));
        }
    }
    (a.abs2(), a)
}

/// Compute the objective and its exact discrete gradient over `region`.
///
/// `build(with_sources)` must return an identically configured simulation
/// (grid, Courant, walls, CPML, materials) every call — forward sources
/// included only when `with_sources` is true. The forward pass stores Ez
/// snapshots over the region (`steps × region.len()` f64s); the adjoint
/// pass reuses the same stepper with monitor-line sources.
pub fn objective_and_gradient(
    build: &mut dyn FnMut(bool) -> Simulation,
    region: &DesignRegion,
    obj: &ModeOverlap,
    steps: usize,
) -> GradientResult {
    objectives_and_gradients(build, region, std::slice::from_ref(obj), steps)
        .pop()
        .expect("one objective in, one result out")
}

/// Multi-objective form: **one shared forward pass** (the expensive
/// series storage is reused), then one adjoint pass per objective —
/// exactly what a splitter needs (one gradient per output arm, combined
/// by the optimizer with whatever weighting its figure of merit implies).
pub fn objectives_and_gradients(
    build: &mut dyn FnMut(bool) -> Simulation,
    region: &DesignRegion,
    objs: &[ModeOverlap],
    steps: usize,
) -> Vec<GradientResult> {
    assert!(!objs.is_empty());
    // ---- Forward pass: record every A and the region's Ez time series.
    let mut fwd = build(true);
    assert_eq!(fwd.polarization(), Polarization::Tm);
    for obj in objs {
        validate_geometry(&fwd, region, obj);
    }
    let dt = fwd.dt();
    let (rx, ry) = (region.ns_x(), region.ns_y());
    let cells = rx * ry;
    // series[n·cells + c] = Ez at region cell c after forward step n;
    // one extra leading frame of zeros represents Ez^0.
    let mut series = vec![0.0f64; (steps + 1) * cells];
    let mut amps = vec![Cplx::ZERO; objs.len()];
    for n in 0..steps {
        fwd.step();
        let t = (n as f64 + 1.0) * dt;
        for (k, obj) in objs.iter().enumerate() {
            let omega = 2.0 * std::f64::consts::PI * obj.freq;
            let ph = Cplx::cis(omega * t).scale(dt);
            for (m, w) in obj.weights.iter().enumerate() {
                amps[k] = amps[k] + ph.scale(w * fwd.ez_at(obj.i, obj.j0 + m));
            }
        }
        let base = (n + 1) * cells;
        for di in 0..rx {
            for dj in 0..ry {
                series[base + di * ry + dj] = fwd.ez_at(region.i0 + di, region.j0 + dj);
            }
        }
    }

    // ---- One adjoint pass per objective: same stepper, monitor-line
    // sources, reversed pairing with the stored forward increments.
    objs.iter()
        .zip(amps.iter())
        .map(|(obj, &a)| {
            let mut adj = build(false);
            assert_eq!(adj.polarization(), Polarization::Tm);
            assert_eq!(
                adj.dt(),
                dt,
                "adjoint must share the forward discretization"
            );
            let omega = 2.0 * std::f64::consts::PI * obj.freq;
            // 1/ε at the monitor samples (the D⁻¹ in the transformed
            // sources).
            let inv_eps_mon: Vec<f64> = (0..obj.weights.len())
                .map(|m| 1.0 / adj.epsilon().0.at(obj.i, obj.j0 + m))
                .collect();
            let mut grad = Field2::new(rx, ry);
            for q in 0..steps {
                adj.step();
                // Adjoint source for this step:
                // (2·dt/ε_m)·w_m·Re[conj(A)·e^{iω·t_k}] with
                // t_k = (N−q)·dt — adjoint step q produces the transformed
                // λ^{N−q}, whose recursion carries g^{N−q}; the first
                // injection (q = 0) realizes the terminal condition
                // λ^N = ∂J/∂u^N.
                let t_k = (steps - q) as f64 * dt;
                let osc = Cplx::cis(omega * t_k);
                let re = a.conj() * osc;
                for (m, w) in obj.weights.iter().enumerate() {
                    let s = 2.0 * dt * inv_eps_mon[m] * w * re.re;
                    if s != 0.0 {
                        adj.inject_ez(obj.i, obj.j0 + m, s);
                    }
                }
                // Pair φ^{N−q} — the POST-step, POST-injection adjoint
                // field (λ^{n+1} includes its own ∂J/∂u^{n+1} term) —
                // with the forward increment ΔE^n at n = N−q−1.
                let n = steps - 1 - q;
                let (b1, b0) = ((n + 1) * cells, n * cells);
                for di in 0..rx {
                    for dj in 0..ry {
                        let phi = adj.ez_at(region.i0 + di, region.j0 + dj);
                        let de = series[b1 + di * ry + dj] - series[b0 + di * ry + dj];
                        *grad.at_mut(di, dj) -= phi * de;
                    }
                }
            }
            GradientResult {
                objective: a.abs2(),
                amplitude: a,
                grad,
                region: *region,
            }
        })
        .collect()
}

/// Region/monitor placement rules the gradient formula depends on.
fn validate_geometry(sim: &Simulation, region: &DesignRegion, obj: &ModeOverlap) {
    let g = sim.grid();
    assert!(region.i0 <= region.i1 && region.j0 <= region.j1);
    assert!(region.i1 <= g.nx && region.j1 <= g.ny, "region out of grid");
    let c = sim.cpml_spec();
    assert!(
        region.i0 > c.x_lo
            && region.i1 < g.nx - c.x_hi
            && region.j0 > c.y_lo
            && region.j1 < g.ny - c.y_hi,
        "design region must not overlap the CPML slabs"
    );
    assert!(
        obj.i < region.i0 || obj.i > region.i1,
        "objective monitor must sit outside the design region"
    );
    // The adjoint injects sources on the monitor rows; inside a CPML slab
    // the untransposed-ψ dynamics make that injection first-order wrong
    // (measured: 17 % gradient error when a monitor line dipped 2 rows
    // into the y-PML — the bug this assert now makes impossible).
    let j_last = obj.j0 + obj.weights.len() - 1;
    assert!(
        obj.i > c.x_lo && obj.i < g.nx - c.x_hi && obj.j0 > c.y_lo && j_last < g.ny - c.y_hi,
        "objective monitor rows must lie strictly outside the CPML slabs"
    );
    for src in sim.sources() {
        let cols: (usize, usize) = match &src.placement {
            SourcePlacement::Point { i, .. } => (*i, *i),
            SourcePlacement::VerticalLine { i, .. } => (*i, *i),
            SourcePlacement::TfsfVerticalLine { i0, .. } => (*i0, *i0 + 1),
        };
        assert!(
            cols.1 < region.i0 || cols.0 > region.i1,
            "forward sources must not overlap the design region"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_arithmetic() {
        let r = DesignRegion {
            i0: 10,
            i1: 14,
            j0: 3,
            j1: 5,
        };
        assert_eq!(r.ns_x(), 5);
        assert_eq!(r.ns_y(), 3);
        assert_eq!(r.len(), 15);
        assert!(!r.is_empty());
    }
}
