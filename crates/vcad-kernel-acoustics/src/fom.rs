//! Figures of merit read off the solved field.
//!
//! Resonance frequencies, mode shapes, port volume velocity, and the driven
//! bass-reflex tuning — the numbers a loudspeaker or resonator designer acts
//! on, and the ones the receipt reports.

use crate::cavity::Cavity;
use crate::complex::Cplx;
use crate::helmholtz::{Field, NodeKind, Source};
use crate::sweep::{find_peaks, frequency_sweep, Probe, Resonance};

/// The axial mode shape: `|p|` along the axis `r = 0`, paired with the axial
/// coordinate (mm) at each fluid node. Normalized to a peak of 1. Solid/mouth
/// nodes off the fluid are skipped.
pub fn axial_mode_shape(field: &Field) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    let mut peak = 0.0_f64;
    for j in 0..field.nz {
        if field.kind_at(0, j) == NodeKind::Solid {
            continue;
        }
        let z = field.z_min_mm + j as f64 * field.dz_mm;
        let a = field.node(0, j).abs();
        peak = peak.max(a);
        out.push((z, a));
    }
    if peak > 0.0 {
        for p in out.iter_mut() {
            p.1 /= peak;
        }
    }
    out
}

/// Complex volume velocity through the port mouth (m³/s), from the axial
/// pressure gradient just inside the open mouth. Euler's relation gives
/// `v_z = (j/ωρ)·∂p/∂z`; integrating over the mouth cross-section yields the
/// throughput that actually radiates in a bass-reflex box.
pub fn port_volume_velocity(field: &Field, cavity: &Cavity) -> Cplx {
    let omega = std::f64::consts::TAU * field.f_hz;
    let rho = cavity.medium.rho;
    let dz = field.dz_mm * 1e-3;
    let dr = field.dr_mm * 1e-3;
    // Mouth is the top plane (j = nz−1). Use the one-sided gradient into the
    // last two interior rows.
    let jm = field.nz - 1;
    if jm < 2 {
        return Cplx::ZERO;
    }
    let mut acc = Cplx::ZERO;
    for i in 0..field.nr {
        if field.kind_at(i, jm) != NodeKind::Open {
            continue;
        }
        // ∂p/∂z at the mouth ≈ (p[jm] − p[jm−1]) / dz, with p[jm] = 0.
        let dpdz = (field.node(i, jm) - field.node(i, jm - 1)).scale(1.0 / dz);
        let vz = Cplx::J.scale(1.0 / (omega * rho)) * dpdz;
        // Annular area of node i (m²).
        let rr = (((i as f64) + 0.5) * dr).min(field.r_max_mm * 1e-3);
        let rl = (((i as f64) - 0.5) * dr).max(0.0);
        let area = std::f64::consts::PI * (rr * rr - rl * rl);
        acc += vz.scale(area);
    }
    acc
}

/// Extract the resonances of a driven cavity in `[f_lo, f_hi]` by sweeping and
/// peak-picking. A convenience wrapping [`frequency_sweep`] + [`find_peaks`].
#[allow(clippy::too_many_arguments)]
pub fn resonances(
    cavity: &Cavity,
    nr: usize,
    nz: usize,
    source: Source,
    probe: Probe,
    f_lo: f64,
    f_hi: f64,
    n: usize,
    max_peaks: usize,
) -> Vec<Resonance> {
    let sweep = frequency_sweep(cavity, nr, nz, source, probe, f_lo, f_hi, n);
    find_peaks(&sweep, max_peaks)
}

/// The field-solved bass-reflex tuning of a ported box, Hz: the lowest
/// interior-pressure resonance when the box is driven by its piston, found by
/// probing near the port base. Returns `None` if no peak is found in the band.
pub fn driven_tuning_hz(
    cavity: &Cavity,
    nr: usize,
    nz: usize,
    f_lo: f64,
    f_hi: f64,
    n: usize,
) -> Option<f64> {
    // Probe just below the port base, on axis (strong Helmholtz-mode pressure).
    let port = cavity.port_segment();
    let probe = Probe {
        r_mm: 0.0,
        z_mm: port.z0_mm - 1.0,
    };
    let peaks = resonances(
        cavity,
        nr,
        nz,
        Source::Piston {
            velocity: Cplx::ONE,
        },
        probe,
        f_lo,
        f_hi,
        n,
        4,
    );
    peaks.first().map(|r| r.f_hz)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::helmholtz::solve_driven;
    use crate::medium::Medium;

    #[test]
    fn axial_mode_shape_is_normalized() {
        let cav = Cavity::closed_cylinder(20.0, 200.0, Medium::air(20.0));
        let field = solve_driven(
            &cav,
            9,
            81,
            500.0,
            Source::Monopole {
                r_mm: 1.0,
                z_mm: 5.0,
                q: Cplx::ONE,
            },
        )
        .unwrap();
        let shape = axial_mode_shape(&field);
        assert!(!shape.is_empty());
        let peak = shape.iter().map(|p| p.1).fold(0.0_f64, f64::max);
        assert!((peak - 1.0).abs() < 1e-12);
    }
}
