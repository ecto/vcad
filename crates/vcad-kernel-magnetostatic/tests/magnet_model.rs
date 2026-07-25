//! The equivalent-surface-current magnet model, checked against the closed form
//! for an axially magnetized cylinder.

use std::f64::consts::PI;

use vcad_kernel_magnetostatic::{IronStack, MagnetRing, Polarity, PrismMagnet, Vec3};

fn disc(radius_m: f64, z0: f64, t: f64, br: f64, pol: Polarity, facets: usize) -> PrismMagnet {
    PrismMagnet {
        footprint: (0..facets)
            .map(|j| {
                let a = 2.0 * PI * (j as f64) / (facets as f64);
                (radius_m * a.cos(), radius_m * a.sin())
            })
            .collect(),
        z0_m: z0,
        thickness_m: t,
        remanence_t: br,
        polarity: pol,
    }
}

/// On-axis field of an axially magnetized cylinder, the standard closed form:
/// `B_z = (B_r/2)[ (z+L)/√((z+L)²+R²) − z/√(z²+R²) ]`, with `z` measured from
/// the near face.
fn cylinder_axial_closed_form(br: f64, radius: f64, length: f64, z_from_face: f64) -> f64 {
    let a = (z_from_face + length) / ((z_from_face + length).powi(2) + radius * radius).sqrt();
    let b = z_from_face / (z_from_face * z_from_face + radius * radius).sqrt();
    0.5 * br * (a - b)
}

#[test]
fn disc_magnet_matches_the_closed_form_on_axis() {
    // Y30 ferrite, the reference motor's magnet: D15 x 3 mm, Br 385 mT.
    let br = 0.385;
    let r = 0.0075;
    let t = 0.003;
    let m = disc(r, 0.0, t, br, Polarity::North, 720);
    let fils = m.to_filaments(48);

    for z_face in [0.0005, 0.001, 0.002, 0.004, 0.008] {
        let p = Vec3::z_axis(t + z_face); // above the top face
        let b: Vec3 = fils.iter().map(|f| f.b_at(p)).sum();
        let want = cylinder_axial_closed_form(br, r, t, z_face);
        assert!(
            (b.z - want).abs() / want.abs() < 0.02,
            "z={z_face}: {} vs closed form {}",
            b.z,
            want
        );
        assert!(
            b.x.abs().max(b.y.abs()) < 1e-6 * want.abs(),
            "off-axis leakage"
        );
    }
}

#[test]
fn the_reference_magnet_gives_the_free_space_airgap_number() {
    // The number quoted when choosing the solver path: a Y30 D15x3 disc produces
    // ~65 mT at 1 mm from its face in free space. This is the "no iron" anchor
    // that the back-iron gain is measured against.
    let m = disc(0.0075, 0.0, 0.003, 0.385, Polarity::North, 720);
    let fils = m.to_filaments(48);
    let b: Vec3 = fils.iter().map(|f| f.b_at(Vec3::z_axis(0.004))).sum();
    assert!(
        (0.060..0.070).contains(&b.z),
        "free-space airgap flux {} T outside the expected 60–70 mT",
        b.z
    );
}

#[test]
fn polarity_flips_the_field() {
    let n = disc(0.0075, 0.0, 0.003, 0.385, Polarity::North, 180);
    let s = disc(0.0075, 0.0, 0.003, 0.385, Polarity::South, 180);
    let p = Vec3::z_axis(0.005);
    let bn: Vec3 = n.to_filaments(8).iter().map(|f| f.b_at(p)).sum();
    let bs: Vec3 = s.to_filaments(8).iter().map(|f| f.b_at(p)).sum();
    assert!(bn.z > 0.0, "north-up must push +z flux above the magnet");
    assert!((bn.z + bs.z).abs() < 1e-12 * bn.z.abs());
}

#[test]
fn footprint_winding_order_does_not_change_the_field() {
    // Callers should not have to know the orientation convention.
    let ccw = disc(0.0075, 0.0, 0.003, 0.385, Polarity::North, 120);
    let mut cw = ccw.clone();
    cw.footprint.reverse();
    let p = Vec3::new(0.002, 0.001, 0.005);
    let a: Vec3 = ccw.to_filaments(6).iter().map(|f| f.b_at(p)).sum();
    let b: Vec3 = cw.to_filaments(6).iter().map(|f| f.b_at(p)).sum();
    assert!((a - b).norm() / a.norm() < 1e-12);
}

#[test]
fn an_alternating_rotor_ring_is_magnetically_balanced() {
    // The precondition the two-plane image series depends on.
    let ring = MagnetRing::discs(6, 0.0225, 0.015, 0.0, 0.003, 0.385, 64);
    let fils = ring.to_filaments(4);
    assert_eq!(ring.magnets.len(), 6);
    assert!(
        IronStack::balance_residual(&fils) < 1e-9,
        "alternating ring must be balanced, got {}",
        IronStack::balance_residual(&fils)
    );
}

#[test]
fn back_iron_raises_the_reference_airgap_flux_toward_the_mec_value() {
    // The decision that picked this solver path: for the reference geometry the
    // steel is worth ~2.4x, taking ~65 mT of free-space flux toward the ~155 mT
    // the reluctance model predicts for the full circuit. Checked here as a
    // gain band rather than a point value — the MEC number is itself only
    // first-order, and finite-disc fringing is not modelled.
    let ring = MagnetRing::discs(6, 0.0225, 0.015, 0.0, 0.003, 0.385, 96);
    let fils = ring.to_filaments(16);
    let probe = Vec3::new(0.0225, 0.0, 0.004); // 1 mm above a pole face

    let free: Vec3 = fils.iter().map(|f| f.b_at(probe)).sum();
    // Rotor back-iron sits directly behind the magnets (z = 0).
    let backed_stack = IronStack::single(0.0);
    let backed: Vec3 = fils
        .iter()
        .flat_map(|f| backed_stack.expand(f))
        .map(|f| f.b_at(probe))
        .sum();

    let gain = backed.z.abs() / free.z.abs();
    assert!(
        (1.4..2.1).contains(&gain),
        "single back-iron gain {gain} outside the expected 1.4–2.1 band"
    );
}
