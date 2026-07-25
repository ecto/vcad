//! Validation ladder for the segment integrator.
//!
//! Every claim this crate makes rests on [`Segment::b_at`] and [`Segment::a_at`]
//! being right, so they are checked against three independent things: a closed
//! form derived by hand (infinite wire), an exact special function from a
//! *different codebase* (`vcad_kernel_particle::field::b_ring`, elliptic
//! integrals written for particle optics), and an internal consistency identity
//! (curl of A equals B, by finite differences).

use std::f64::consts::PI;

use vcad_kernel_magnetostatic::{Filament, Segment, Vec3, MU_0};
use vcad_kernel_particle::field::{b_ring, RingCoil};

/// A regular polygon approximating a circular loop in the z = `z0` plane.
fn ring_filament(radius_m: f64, z0: f64, current_a: f64, n: usize) -> Filament {
    let pts = (0..n)
        .map(|i| {
            let t = 2.0 * PI * (i as f64) / (n as f64);
            Vec3::cylindrical(radius_m, t, z0)
        })
        .collect();
    Filament::closed_loop(pts, current_a, 0.0)
}

#[test]
fn long_segment_reproduces_the_infinite_wire() {
    // B = μ₀I/(2πd) for an infinite wire; a segment ±L with L ≫ d approaches it.
    let i = 3.0;
    let d = 0.01;
    let l = 1.0e4 * d;
    let seg = Segment {
        a: Vec3::new(0.0, 0.0, -l),
        b: Vec3::new(0.0, 0.0, l),
        current_a: i,
        wire_radius_m: 0.0,
    };
    let b = seg.b_at(Vec3::new(d, 0.0, 0.0));
    let expected = MU_0 * i / (2.0 * PI * d);
    assert!(
        (b.norm() - expected).abs() / expected < 1e-6,
        "magnitude {} vs {}",
        b.norm(),
        expected
    );
    // Right-hand rule: current +z, field point +x ⇒ B along +y.
    assert!(b.y > 0.0 && b.x.abs() < 1e-12 * expected && b.z.abs() < 1e-12 * expected);
}

#[test]
fn on_axis_loop_matches_the_closed_form() {
    // B_z = μ₀ I R² / (2 (R² + z²)^{3/2}) on the axis of a circular loop.
    let r = 0.02;
    let i = 5.0;
    let loop_ = ring_filament(r, 0.0, i, 2048);
    for z in [0.0, 0.005, 0.02, 0.05] {
        let b = loop_.b_at(Vec3::z_axis(z));
        let expected = MU_0 * i * r * r / (2.0 * (r * r + z * z).powf(1.5));
        assert!(
            (b.z - expected).abs() / expected < 1e-5,
            "z={z}: {} vs {}",
            b.z,
            expected
        );
        assert!(b.x.abs().max(b.y.abs()) < 1e-9 * expected, "off-axis leakage at z={z}");
    }
}

#[test]
fn off_axis_loop_matches_particle_crates_elliptic_integrals() {
    // Independent-codebase check: `b_ring` solves the same loop with complete
    // elliptic integrals, written for a different crate and a different purpose.
    let r = 0.02;
    let i = 5.0;
    let coil = RingCoil { radius_m: r, z_m: 0.0, ampere_turns: i, wire_radius_m: 0.0 };
    let loop_ = ring_filament(r, 0.0, i, 4096);

    for &(rho, z) in &[
        (0.005, 0.0),
        (0.010, 0.004),
        (0.019, 0.010),
        (0.030, 0.006),
        (0.050, 0.030),
        (0.002, -0.008),
    ] {
        let (br_ref, bz_ref) = b_ring(&coil, rho, z);
        let p = Vec3::new(rho, 0.0, z);
        let b = loop_.b_at(p);
        // At y = 0 the radial direction is +x.
        let br = b.x;
        let bz = b.z;
        let scale = (br_ref * br_ref + bz_ref * bz_ref).sqrt().max(1e-12);
        assert!(
            (br - br_ref).abs() / scale < 2e-4,
            "B_r at (ρ={rho}, z={z}): {br} vs {br_ref}"
        );
        assert!(
            (bz - bz_ref).abs() / scale < 2e-4,
            "B_z at (ρ={rho}, z={z}): {bz} vs {bz_ref}"
        );
    }
}

#[test]
fn curl_of_a_equals_b() {
    // The two integrals are derived and implemented separately; ∇×A = B ties
    // them together and would catch a sign or factor error in either.
    let loop_ = ring_filament(0.02, 0.0, 4.0, 1024);
    let h = 1e-6;
    let curl_at = |p: Vec3| -> Vec3 {
        let d = |axis: usize, q: Vec3| -> Vec3 {
            let mut plus = q;
            let mut minus = q;
            match axis {
                0 => {
                    plus.x += h;
                    minus.x -= h;
                }
                1 => {
                    plus.y += h;
                    minus.y -= h;
                }
                _ => {
                    plus.z += h;
                    minus.z -= h;
                }
            }
            (loop_.a_at(plus) - loop_.a_at(minus)) * (1.0 / (2.0 * h))
        };
        let dx = d(0, p);
        let dy = d(1, p);
        let dz = d(2, p);
        Vec3::new(dy.z - dz.y, dz.x - dx.z, dx.y - dy.x)
    };

    for p in [
        Vec3::new(0.004, 0.003, 0.006),
        Vec3::new(0.0, 0.0, 0.01),
        Vec3::new(0.035, -0.01, 0.004),
    ] {
        let b = loop_.b_at(p);
        let c = curl_at(p);
        let scale = b.norm().max(1e-9);
        assert!(
            (c - b).norm() / scale < 1e-4,
            "curl A = {c:?} vs B = {b:?} at {p:?}"
        );
    }
}

#[test]
fn mutual_inductance_matches_maxwells_coaxial_formula() {
    // Two coaxial loops. λ = ∮A·dl from one loop through the other gives M,
    // which exercises `a_at` and `flux_linkage` on a case with a known answer.
    // Maxwell: M = μ₀√(R₁R₂)[(2/k − k)K(k²) − (2/k)E(k²)],
    // k² = 4R₁R₂/((R₁+R₂)² + d²). Checked here against the small-k dipole limit,
    // where M → μ₀πR₁²R₂²/(2 d³) for d ≫ R.
    let r1 = 0.01;
    let r2 = 0.012;
    let d = 0.5; // far apart ⇒ dipole limit is accurate
    let source = ring_filament(r1, 0.0, 1.0, 2048);
    let sensor = ring_filament(r2, d, 1.0, 2048);

    let m = sensor.flux_linkage(|p| source.a_at(p));
    let dipole = MU_0 * PI * r1 * r1 * r2 * r2 / (2.0 * d.powi(3));
    assert!(
        (m - dipole).abs() / dipole < 2e-3,
        "M = {m} vs dipole limit {dipole}"
    );
}

#[test]
fn wire_regularization_keeps_the_field_finite_and_odd() {
    // Approaching the conductor from either side must stay finite and reverse
    // sign through the centreline — the uniform-current-density model.
    let a_r = 1e-4;
    let seg = Segment {
        a: Vec3::new(0.0, 0.0, -1.0),
        b: Vec3::new(0.0, 0.0, 1.0),
        current_a: 10.0,
        wire_radius_m: a_r,
    };
    let inside = seg.b_at(Vec3::new(a_r * 0.5, 0.0, 0.0));
    let surface = seg.b_at(Vec3::new(a_r, 0.0, 0.0));
    assert!(inside.norm().is_finite() && surface.norm().is_finite());
    // Linear ramp to the surface value.
    assert!((inside.norm() / surface.norm() - 0.5).abs() < 1e-6);
    // Exactly on the axis the field vanishes by symmetry.
    assert!(seg.b_at(Vec3::ZERO).norm() < 1e-18);
    // Odd through the centreline.
    let other = seg.b_at(Vec3::new(-a_r * 0.5, 0.0, 0.0));
    assert!((other.y + inside.y).abs() < 1e-12 * inside.norm().max(1e-12));
}
