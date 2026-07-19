//! The paraxial y-u trace — the analytic cross-check for the exact tracer.
//!
//! First-order optics: refraction n′u′ = n·u − y·φ with surface power
//! φ = c(n′ − n), transfer y′ = y + t·u′ (real angles, real gaps). From a
//! marginal ray launched parallel to the axis this yields the effective
//! focal length and back focal distance in closed form. A second,
//! independent implementation via 2×2 ray-transfer matrices cross-checks
//! the recurrence (same mathematics, different code path — it catches
//! implementation bugs, not method errors; the *method* is checked against
//! the thin- and thick-lens closed forms in `tests/analytic.rs`).
//!
//! References: Hecht, *Optics* (5th ed.) §6.1 for the thick-lens closed
//! forms; any lens-design text for the y-u (y-nu) trace.

use crate::prescription::Prescription;

/// First-order properties of a prescription at one wavelength.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FirstOrder {
    /// Effective focal length, mm.
    pub efl_mm: f64,
    /// Back focal distance: last vertex → paraxial focus, mm.
    pub bfd_mm: f64,
    /// Paraxial image-plane z (global frame), mm.
    pub image_z_mm: f64,
}

/// Trace a paraxial ray (height `y`, angle `u`, object-space start)
/// through the prescription; returns final (y, u).
pub fn trace_yu(presc: &Prescription, lambda_um: f64, mut y: f64, mut u: f64) -> (f64, f64) {
    for (i, s) in presc.surfaces.iter().enumerate() {
        let n = presc.index_before(i, lambda_um);
        let np = presc.index_after(i, lambda_um);
        let phi = s.curvature() * (np - n);
        u = (n * u - y * phi) / np;
        y += s.thickness_mm * u;
    }
    (y, u)
}

/// First-order solve from an axis-parallel marginal ray.
///
/// Returns `None` (fail-closed) for an afocal or diverging system —
/// there is no finite paraxial focus to report.
pub fn first_order(presc: &Prescription, lambda_um: f64) -> Option<FirstOrder> {
    let mut y = 1.0;
    let mut u = 0.0;
    for (i, s) in presc.surfaces.iter().enumerate() {
        let n = presc.index_before(i, lambda_um);
        let np = presc.index_after(i, lambda_um);
        let phi = s.curvature() * (np - n);
        u = (n * u - y * phi) / np;
        y += s.thickness_mm * u;
    }
    if u >= 0.0 {
        return None; // diverging or afocal: no real focus after the lens
    }
    let y_exit = y - presc.surfaces.last().unwrap().thickness_mm * u; // height at last surface
    let efl = -1.0 / u;
    let bfd = -y_exit / u;
    Some(FirstOrder {
        efl_mm: efl,
        bfd_mm: bfd,
        image_z_mm: presc.last_vertex_z() + bfd,
    })
}

/// Independent 2×2 ray-transfer-matrix computation of (EFL, BFD).
///
/// State (y, ω) with reduced angle ω = n·u; refraction ω′ = ω − yφ,
/// transfer y′ = y + (t/n)ω. EFL = −1/C and BFD = −A/C of the system
/// matrix [[A, B], [C, D]].
pub fn first_order_matrix(presc: &Prescription, lambda_um: f64) -> Option<(f64, f64)> {
    let (mut a, mut b, mut c, mut d) = (1.0f64, 0.0f64, 0.0f64, 1.0f64);
    let mut mul = |m: [[f64; 2]; 2]| {
        let (na, nb) = (m[0][0] * a + m[0][1] * c, m[0][0] * b + m[0][1] * d);
        let (nc, nd) = (m[1][0] * a + m[1][1] * c, m[1][0] * b + m[1][1] * d);
        a = na;
        b = nb;
        c = nc;
        d = nd;
    };
    for (i, s) in presc.surfaces.iter().enumerate() {
        let n = presc.index_before(i, lambda_um);
        let np = presc.index_after(i, lambda_um);
        let phi = s.curvature() * (np - n);
        mul([[1.0, 0.0], [-phi, 1.0]]);
        if i + 1 < presc.surfaces.len() {
            mul([[1.0, s.thickness_mm / np], [0.0, 1.0]]);
        }
    }
    if c >= 0.0 {
        return None;
    }
    Some((-1.0 / c, -a / c))
}

/// Lagrange invariant H = n(u·ȳ − ū·y) for a marginal/chief ray pair,
/// evaluated in object space. The invariant is conserved surface-by-
/// surface in first-order optics; `lagrange_drift` measures the worst
/// relative violation across the system (an internal-consistency
/// diagnostic for the paraxial machinery).
pub fn lagrange_drift(
    presc: &Prescription,
    lambda_um: f64,
    marginal: (f64, f64),
    chief: (f64, f64),
) -> f64 {
    let (mut y, mut u) = marginal;
    let (mut yb, mut ub) = chief;
    let h0 = u * yb - ub * y; // n = 1 in object space
    let mut worst: f64 = 0.0;
    for (i, s) in presc.surfaces.iter().enumerate() {
        let n = presc.index_before(i, lambda_um);
        let np = presc.index_after(i, lambda_um);
        let phi = s.curvature() * (np - n);
        u = (n * u - y * phi) / np;
        ub = (n * ub - yb * phi) / np;
        let h = np * (u * yb - ub * y);
        worst = worst.max(((h - h0) / h0).abs());
        y += s.thickness_mm * u;
        yb += s.thickness_mm * ub;
    }
    worst
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glass::Glass;
    use crate::prescription::{Prescription, Surface};

    fn doublet() -> Prescription {
        Prescription::new(vec![
            Surface::sphere(62.8, 12.7, 4.0, Glass::n_bk7()),
            Surface::sphere(-45.7, 12.7, 2.5, Glass::sf5()),
            Surface::sphere(-128.2, 12.7, 0.0, Glass::Air),
        ])
        .unwrap()
    }

    #[test]
    fn recurrence_and_matrix_agree() {
        let p = doublet();
        let fo = first_order(&p, crate::lines::D).unwrap();
        let (efl_m, bfd_m) = first_order_matrix(&p, crate::lines::D).unwrap();
        assert!((fo.efl_mm - efl_m).abs() < 1e-9, "{} vs {efl_m}", fo.efl_mm);
        assert!((fo.bfd_mm - bfd_m).abs() < 1e-9, "{} vs {bfd_m}", fo.bfd_mm);
    }

    #[test]
    fn diverging_system_fails_closed() {
        let p = Prescription::new(vec![
            Surface::sphere(-50.0, 10.0, 3.0, Glass::n_bk7()),
            Surface::sphere(50.0, 10.0, 0.0, Glass::Air),
        ])
        .unwrap();
        assert!(first_order(&p, crate::lines::D).is_none());
        assert!(first_order_matrix(&p, crate::lines::D).is_none());
    }

    #[test]
    fn lagrange_invariant_is_conserved() {
        let p = doublet();
        let drift = lagrange_drift(&p, crate::lines::D, (1.0, 0.0), (0.0, 0.02));
        assert!(drift < 1e-12, "drift = {drift}");
    }
}
