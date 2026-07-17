//! The method-of-moments EFIE solver: matrix fill, delta-gap drive, input
//! impedance, S11, and frequency sweeps.
//!
//! # Formulation (and why this one)
//!
//! We solve the thin-wire **electric-field integral equation** in
//! mixed-potential form with **triangular bases and Galerkin testing**.
//! With time convention `e^{+jωt}` and free-space Green's function
//! `G(R) = e^{−jkR}/(4πR)`, the tested EFIE reads `V_m = Σ_n Z_mn I_n`
//! with
//!
//! ```text
//! Z_mn = jkη ∬ f_m(l)·f_n(l′) (t̂·t̂′) G dl dl′  −  (jη/k) ∬ f_m′ f_n′ G dl dl′
//! ```
//!
//! — the first term is the magnetic vector potential of the basis currents,
//! the second the scalar potential of their charges (`q = −I′/jω`), with
//! both derivatives integrated by parts onto the *bases* (which vanish at
//! wire ends), so no derivative of `G` is ever taken numerically.
//!
//! Why triangular + Galerkin instead of NEC-2's three-term sinusoidal
//! point-matching:
//!
//! - **Reciprocity is structural.** Galerkin testing with a symmetric
//!   kernel gives an exactly symmetric `Z`, so `Z_21 = Z_12` holds to
//!   machine precision by construction (and is regression-tested), and the
//!   M2 adjoint is one extra solve against the same factorization.
//! - **Junctions come free at M1.** Triangular bases interpolate the nodal
//!   current, so KCL at a multi-wire junction is just more basis halves —
//!   the machinery NEC-2's basis makes famously delicate.
//! - **No second-derivative kernel.** Pocklington-style formulations
//!   differentiate `G` twice and are fragile near the self term; the
//!   mixed-potential form needs only `G` itself.
//!
//! The trade: sinusoidal bases are more accurate *per unknown* on straight
//! wires. Segments are cheap at N ≤ a few hundred — we buy robustness with
//! a slightly denser mesh, and the convergence study names the floor.
//!
//! # Thin-wire kernel
//!
//! Source current flows on the wire axis and the boundary condition is
//! matched on the surface: `R̃ = sqrt(|r − r′|² + ā²)` with
//! `ā² = (a_m² + a_n²)/2`. Symmetrizing the radii keeps `Z` exactly
//! symmetric for mixed-radius meshes; it matches the textbook same-wire
//! kernel identically (radii equal there) and differs from the
//! source-radius convention by `O((a/R)²)` across wires — far below
//! kernel validity. The gates in [`crate::geometry::Mesh::validate_for`]
//! (`Δ ≥ 4a`, `Δ ≤ λ/8`, `ka ≤ 0.1`) are hard errors, not warnings.
//!
//! # Quadrature
//!
//! Outer (test) integrals: Gauss–Legendre. Inner (source) integrals split
//! the kernel as
//!
//! ```text
//! e^{−jkR̃}/R̃ = [e^{−jkR̃} − 1 + jkR̃]/R̃  +  1/R̃  −  jk
//! ```
//!
//! — the bracket is smooth (O(k²R̃) with bounded curvature) and gets
//! Gauss–Legendre; `∫ w(t)/R̃ dt` has a closed form for linear weights on a
//! straight segment (`asinh` + hypotenuse terms); the constant integrates
//! trivially. Self and adjacent terms therefore carry their near-log
//! singularity analytically instead of asking a polynomial rule to chase
//! `1/R̃` with `R̃` down at the wire radius.

use crate::complex::Complex;
use crate::constants::{C0, ETA_0};
use crate::error::AntennaError;
use crate::geometry::{Mesh, Segment};
use crate::linalg::{lu_decompose, CMatrix};
use crate::quad::gauss_legendre;

/// Solver options: quadrature orders for the matrix fill.
#[derive(Debug, Clone, Copy)]
pub struct SolveOptions {
    /// Gauss–Legendre order for the outer (test) integral per segment.
    pub quad_outer: usize,
    /// Gauss–Legendre order for the inner (source) smooth remainder.
    pub quad_inner: usize,
}

impl Default for SolveOptions {
    fn default() -> Self {
        SolveOptions {
            quad_outer: 6,
            quad_inner: 6,
        }
    }
}

/// A solved delta-gap-driven mesh at one frequency.
#[derive(Debug, Clone)]
pub struct DrivenSolution {
    /// Drive frequency, Hz.
    pub freq_hz: f64,
    /// Free-space wavelength, m.
    pub wavelength_m: f64,
    /// Wavenumber `2π/λ`, 1/m.
    pub k: f64,
    /// Basis index of the delta-gap feed.
    pub feed_basis: usize,
    /// Gap voltage (1 V by convention).
    pub v0: Complex,
    /// Solved basis currents, amperes (current at each basis node).
    pub currents: Vec<Complex>,
    /// Input impedance `V₀ / I_feed`, Ω.
    pub z_in: Complex,
    /// Real input power `½ Re(V₀ I_feed*)`, W.
    pub input_power_w: f64,
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The four ramp-weighted double integrals `∬ w_a(s) w_b(t) G(R̃) dt ds`
/// over a (test p, source q) segment pair, indexed `[a][b]` with ramp 0
/// peaking at the segment start and ramp 1 at its end.
fn pair_integrals(
    p: &Segment,
    q: &Segment,
    k: f64,
    outer: &(Vec<f64>, Vec<f64>),
    inner: &(Vec<f64>, Vec<f64>),
) -> [[Complex; 2]; 2] {
    let a2 = 0.5 * (p.radius * p.radius + q.radius * q.radius);
    let inv4pi = 1.0 / (4.0 * std::f64::consts::PI);
    let mut m = [[Complex::ZERO; 2]; 2];

    for (si, &sx) in outer.0.iter().enumerate() {
        let s = 0.5 * p.len * (sx + 1.0);
        let sw = outer.1[si] * 0.5 * p.len;
        let x = [
            p.p0[0] + s * p.tangent[0],
            p.p0[1] + s * p.tangent[1],
            p.p0[2] + s * p.tangent[2],
        ];

        // Closed-form ∫ dt/R̃ and ∫ t dt/R̃ over the source segment, with
        // R̃(t) = sqrt((t − t0)² + c²), c² = h² + ā².
        let d = [x[0] - q.p0[0], x[1] - q.p0[1], x[2] - q.p0[2]];
        let t0 = dot(d, q.tangent);
        let h2 = (dot(d, d) - t0 * t0).max(0.0);
        let c2 = h2 + a2;
        let c = c2.sqrt();
        let u0 = -t0;
        let u1 = q.len - t0;
        let ia0 = (u1 / c).asinh() - (u0 / c).asinh();
        let r_u0 = (u0 * u0 + c2).sqrt();
        let r_u1 = (u1 * u1 + c2).sqrt();
        let ia_t = (r_u1 - r_u0) + t0 * ia0; // ∫ t/R̃ dt
                                             // Per-ramp analytic parts: w0 = 1 − t/L, w1 = t/L.
        let analytic = [ia0 - ia_t / q.len, ia_t / q.len];

        // Smooth remainder by Gauss–Legendre.
        let mut smooth = [Complex::ZERO; 2];
        for (ti, &tx) in inner.0.iter().enumerate() {
            let t = 0.5 * q.len * (tx + 1.0);
            let tw = inner.1[ti] * 0.5 * q.len;
            let y = [
                q.p0[0] + t * q.tangent[0],
                q.p0[1] + t * q.tangent[1],
                q.p0[2] + t * q.tangent[2],
            ];
            let dx = [x[0] - y[0], x[1] - y[1], x[2] - y[2]];
            let r = (dot(dx, dx) + a2).sqrt();
            let phi = k * r;
            // (e^{−jφ} − 1 + jφ)/R̃ — smooth through R̃ → 0.
            let g = Complex::new((phi.cos() - 1.0) / r, (phi - phi.sin()) / r);
            let w1 = t / q.len;
            smooth[0] += g.scale(tw * (1.0 - w1));
            smooth[1] += g.scale(tw * w1);
        }

        // Constant part: ∫ w_b (−jk) dt = −jk L/2 for either ramp.
        let const_part = Complex::new(0.0, -k * 0.5 * q.len);

        let w1s = s / p.len;
        let wa = [1.0 - w1s, w1s];
        for a in 0..2 {
            for b in 0..2 {
                let inner_b = (smooth[b] + Complex::real(analytic[b]) + const_part).scale(inv4pi);
                m[a][b] += inner_b.scale(sw * wa[a]);
            }
        }
    }
    m
}

/// Fill the Galerkin impedance matrix at wavenumber `k`.
///
/// Public so tests can assert structural symmetry; use [`solve_driven`]
/// for the full pipeline.
pub fn fill_impedance_matrix(mesh: &Mesh, k: f64, opts: &SolveOptions) -> CMatrix {
    let nb = mesh.bases.len();
    let ns = mesh.segments.len();
    let mut z = CMatrix::zeros(nb);

    // Which bases touch each segment: (basis, ramp index at the node end,
    // signed charge slope, signed ramp sign).
    struct Touch {
        basis: usize,
        end: u8,
        sign: f64,
        slope: f64, // f′ on this segment: sign · (end == 1 ? +1/L : −1/L)
    }
    let mut touch: Vec<Vec<Touch>> = (0..ns).map(|_| Vec::new()).collect();
    for (bi, b) in mesh.bases.iter().enumerate() {
        for h in &b.halves {
            let l = mesh.segments[h.seg].len;
            touch[h.seg].push(Touch {
                basis: bi,
                end: h.end,
                sign: h.sign,
                slope: h.sign * if h.end == 1 { 1.0 / l } else { -1.0 / l },
            });
        }
    }

    let outer = gauss_legendre(opts.quad_outer);
    let inner = gauss_legendre(opts.quad_inner);
    let jeta = Complex::new(0.0, ETA_0);

    // Each unordered segment pair is integrated ONCE and assembled into
    // both (test, source) orientations from the same numbers — the double
    // integral is one quantity, and reusing it makes the Galerkin matrix
    // symmetric to machine precision (not merely to quadrature error).
    let assemble = |z: &mut CMatrix, p: usize, q: usize, m: &[[Complex; 2]; 2], tt: f64| {
        let s_total = m[0][0] + m[0][1] + m[1][0] + m[1][1];
        for tm in &touch[p] {
            for tn in &touch[q] {
                // Ramp peaking at the basis node: index = end.
                let vector = m[tm.end as usize][tn.end as usize].scale(k * tt * tm.sign * tn.sign);
                let scalar = s_total.scale(tm.slope * tn.slope / k);
                let contrib = jeta * (vector - scalar);
                *z.at_mut(tm.basis, tn.basis) += contrib;
            }
        }
    };

    for p in 0..ns {
        for q in p..ns {
            if touch[p].is_empty() || touch[q].is_empty() {
                continue;
            }
            let sp = &mesh.segments[p];
            let sq = &mesh.segments[q];
            let m = pair_integrals(sp, sq, k, &outer, &inner);
            let tt = dot(sp.tangent, sq.tangent);
            assemble(&mut z, p, q, &m, tt);
            if p != q {
                let mt = [[m[0][0], m[1][0]], [m[0][1], m[1][1]]];
                assemble(&mut z, q, p, &mt, tt);
            }
        }
    }
    z
}

/// Solve a delta-gap-driven mesh at `freq_hz` with a 1 V gap at
/// `feed_basis`. Runs the fail-closed validity gates first.
pub fn solve_driven(
    mesh: &Mesh,
    feed_basis: usize,
    freq_hz: f64,
    opts: &SolveOptions,
) -> Result<DrivenSolution, AntennaError> {
    mesh.validate_for(freq_hz)?;
    if feed_basis >= mesh.bases.len() {
        return Err(AntennaError::FeedOutOfRange {
            feed: feed_basis,
            bases: mesh.bases.len(),
        });
    }
    let lambda = C0 / freq_hz;
    let k = 2.0 * std::f64::consts::PI / lambda;

    let z = fill_impedance_matrix(mesh, k, opts);
    let lu = lu_decompose(z)?;
    let v0 = Complex::ONE;
    let mut rhs = vec![Complex::ZERO; mesh.bases.len()];
    rhs[feed_basis] = v0;
    let currents = lu.solve(&rhs);

    let i_feed = currents[feed_basis];
    if i_feed.norm_sqr() <= 0.0 || !i_feed.is_finite() {
        return Err(AntennaError::SingularSystem);
    }
    let z_in = v0 / i_feed;
    // ½ Re(V₀ I*) with V₀ = 1 + 0j.
    let input_power_w = 0.5 * i_feed.re;

    Ok(DrivenSolution {
        freq_hz,
        wavelength_m: lambda,
        k,
        feed_basis,
        v0,
        currents,
        z_in,
        input_power_w,
    })
}

/// Reflection coefficient of impedance `z` against a real reference `z0`.
pub fn s11(z: Complex, z0: f64) -> Complex {
    (z - Complex::real(z0)) / (z + Complex::real(z0))
}

/// `20 log₁₀ |S11|`, dB (clamped at −120 dB for a perfect match).
pub fn s11_db(z: Complex, z0: f64) -> f64 {
    let mag = s11(z, z0).abs();
    (20.0 * mag.log10()).max(-120.0)
}

/// One point of a frequency sweep.
#[derive(Debug, Clone, Copy)]
pub struct SweepPoint {
    /// Frequency, Hz.
    pub freq_hz: f64,
    /// Input impedance, Ω.
    pub z_in: Complex,
    /// S11 against the sweep's reference impedance.
    pub s11: Complex,
    /// `20 log₁₀ |S11|`, dB.
    pub s11_db: f64,
}

/// Sweep the driven solve across `freqs`, reporting Z_in and S11 vs `z0`.
pub fn sweep(
    mesh: &Mesh,
    feed_basis: usize,
    freqs: &[f64],
    z0: f64,
    opts: &SolveOptions,
) -> Result<Vec<SweepPoint>, AntennaError> {
    freqs
        .iter()
        .map(|&f| {
            let sol = solve_driven(mesh, feed_basis, f, opts)?;
            Ok(SweepPoint {
                freq_hz: f,
                z_in: sol.z_in,
                s11: s11(sol.z_in, z0),
                s11_db: s11_db(sol.z_in, z0),
            })
        })
        .collect()
}

/// Find the resonance (`Im(Z_in) = 0`) by bisection over `[f_lo, f_hi]`.
///
/// The bracket must straddle the sign change; fails closed otherwise.
pub fn find_resonance(
    mesh: &Mesh,
    feed_basis: usize,
    f_lo: f64,
    f_hi: f64,
    opts: &SolveOptions,
) -> Result<f64, AntennaError> {
    let x_at = |f: f64| -> Result<f64, AntennaError> {
        Ok(solve_driven(mesh, feed_basis, f, opts)?.z_in.im)
    };
    let mut lo = f_lo;
    let mut hi = f_hi;
    let mut x_lo = x_at(lo)?;
    let x_hi = x_at(hi)?;
    if x_lo == 0.0 {
        return Ok(lo);
    }
    if x_hi == 0.0 {
        return Ok(hi);
    }
    if x_lo.signum() == x_hi.signum() {
        return Err(AntennaError::ResonanceNotBracketed { x_lo, x_hi });
    }
    for _ in 0..60 {
        let mid = 0.5 * (lo + hi);
        let x_mid = x_at(mid)?;
        if x_mid == 0.0 || (hi - lo) < 1e-7 * mid {
            return Ok(mid);
        }
        if x_mid.signum() == x_lo.signum() {
            lo = mid;
            x_lo = x_mid;
        } else {
            hi = mid;
        }
    }
    Ok(0.5 * (lo + hi))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::WireGrid;

    fn dipole_mesh(len_mm: f64, radius_mm: f64, nseg: usize) -> Mesh {
        let mut g = WireGrid::new();
        g.add_wire(
            [0.0, 0.0, -len_mm / 2.0],
            [0.0, 0.0, len_mm / 2.0],
            radius_mm,
            nseg,
        )
        .unwrap();
        Mesh::build(&g).unwrap()
    }

    #[test]
    fn impedance_matrix_is_structurally_symmetric() {
        // A bent chain with mixed radii — symmetry must survive both.
        let mut g = WireGrid::new();
        g.add_wire([0.0, 0.0, 0.0], [0.0, 0.0, 300.0], 1.0, 6)
            .unwrap();
        g.add_wire([0.0, 0.0, 300.0], [0.0, 200.0, 300.0], 0.5, 4)
            .unwrap();
        let mesh = Mesh::build(&g).unwrap();
        let k = 2.0 * std::f64::consts::PI / 3.0; // λ = 3 m
        let z = fill_impedance_matrix(&mesh, k, &SolveOptions::default());
        let scale = z.max_abs();
        let mut worst: f64 = 0.0;
        for i in 0..z.n() {
            for j in 0..z.n() {
                worst = worst.max((z.at(i, j) - z.at(j, i)).abs());
            }
        }
        assert!(
            worst < 1e-12 * scale,
            "asymmetry {worst:.3e} vs scale {scale:.3e}"
        );
    }

    #[test]
    fn short_dipole_is_capacitive_with_the_textbook_radiation_resistance() {
        // ℓ = 0.04 λ at 300 MHz (λ ≈ 0.9993 m): Balanis (4th ed., §4.3):
        // triangular-current short dipole has R_r = 20π²(ℓ/λ)², and its
        // reactance is strongly capacitive.
        let f = 300e6;
        let lambda = C0 / f;
        let len_mm = 0.04 * lambda * 1e3;
        let mesh = dipole_mesh(len_mm, len_mm / 2000.0, 8);
        let feed = mesh.nearest_basis([0.0, 0.0, 0.0]).unwrap();
        let sol = solve_driven(&mesh, feed, f, &SolveOptions::default()).unwrap();
        let r_expected = 20.0 * std::f64::consts::PI.powi(2) * 0.04_f64.powi(2);
        assert!(
            (sol.z_in.re - r_expected).abs() < 0.25 * r_expected,
            "R = {:.4} Ω, expected ≈ {:.4} Ω",
            sol.z_in.re,
            r_expected
        );
        assert!(
            sol.z_in.im < -500.0,
            "short dipole must be strongly capacitive, got X = {:.1} Ω",
            sol.z_in.im
        );
    }

    #[test]
    fn s11_of_matched_load_is_deep() {
        assert!(s11_db(Complex::real(50.0), 50.0) <= -120.0);
        let db = s11_db(Complex::new(73.0, 0.0), 50.0);
        // |Γ| = 23/123 → −14.6 dB
        assert!((db - 20.0 * (23.0_f64 / 123.0).log10()).abs() < 1e-9);
    }

    #[test]
    fn solve_rejects_out_of_range_feed() {
        let mesh = dipole_mesh(1000.0, 1.0, 8);
        match solve_driven(&mesh, 99, 150e6, &SolveOptions::default()) {
            Err(AntennaError::FeedOutOfRange { .. }) => {}
            other => panic!("expected feed error, got {other:?}"),
        }
    }
}
