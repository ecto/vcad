//! The image model must reproduce the `μ→∞` boundary condition it claims.
//!
//! These are the checks that decide whether back-iron can be handled without a
//! grid: at an infinitely permeable surface the tangential field must vanish and
//! the normal field must double, and the two-plane image series must converge.

use std::f64::consts::PI;

use vcad_kernel_magnetostatic::{Filament, IronStack, Vec3};

fn ring(radius_m: f64, z0: f64, current_a: f64, n: usize) -> Filament {
    let pts = (0..n)
        .map(|i| Vec3::cylindrical(radius_m, 2.0 * PI * (i as f64) / (n as f64), z0))
        .collect();
    Filament::closed_loop(pts, current_a, 0.0)
}

/// Sum B over a source and its images.
fn b_with_iron(stack: &IronStack, src: &Filament, p: Vec3) -> Vec3 {
    stack.expand(src).iter().map(|f| f.b_at(p)).sum()
}

#[test]
fn a_single_plane_doubles_normal_field_and_cancels_tangential() {
    // The defining property of a μ→∞ mirror. A loop at z = h above a plane at
    // z = 0: on the plane, B_z must be exactly twice the free-space value and
    // the radial component must vanish.
    let h = 0.006;
    let loop_ = ring(0.02, h, 3.0, 1024);
    let stack = IronStack::single(0.0);

    for &rho in &[0.0, 0.005, 0.015, 0.025, 0.04] {
        let p = Vec3::new(rho, 0.0, 0.0);
        let free = loop_.b_at(p);
        let with = b_with_iron(&stack, &loop_, p);
        assert!(
            (with.z - 2.0 * free.z).abs() <= 1e-9 * free.z.abs().max(1e-12),
            "ρ={rho}: B_z {} vs 2×{}",
            with.z,
            free.z
        );
        // Tangential (radial, here +x) component cancels at the surface.
        let scale = with.z.abs().max(1e-12);
        assert!(
            with.x.abs() / scale < 1e-9,
            "ρ={rho}: tangential B_x = {} did not cancel",
            with.x
        );
    }
}

#[test]
fn image_of_a_loop_matches_a_hand_placed_mirror_loop() {
    // Independent construction: build the mirror loop explicitly and compare.
    let h = 0.004;
    let src = ring(0.015, h, 2.5, 512);
    let mirror = ring(0.015, -h, 2.5, 512);
    let stack = IronStack::single(0.0);

    for p in [
        Vec3::new(0.0, 0.0, 0.01),
        Vec3::new(0.012, 0.004, 0.006),
        Vec3::new(0.03, -0.01, 0.002),
    ] {
        let by_images = b_with_iron(&stack, &src, p);
        let by_hand = src.b_at(p) + mirror.b_at(p);
        let scale = by_hand.norm().max(1e-15);
        assert!(
            (by_images - by_hand).norm() / scale < 1e-12,
            "at {p:?}: {by_images:?} vs {by_hand:?}"
        );
    }
}

#[test]
fn back_iron_raises_airgap_flux_by_the_expected_factor() {
    // The engineering claim that motivated all of this: a back-iron behind the
    // magnet is worth roughly a factor of two in airgap flux. Modelled here with
    // a current loop standing in for the magnet's bound surface current.
    let magnet_z = 0.003;
    let src = ring(0.0075, magnet_z, 100.0, 512);
    let probe = Vec3::z_axis(0.0075); // in the airgap, beyond the magnet

    let free = src.b_at(probe).z;
    // Iron immediately behind the magnet.
    let backed = b_with_iron(&IronStack::single(0.0), &src, probe).z;

    assert!(backed > free, "back-iron must increase airgap flux");
    let ratio = backed / free;
    assert!(
        (1.3..2.0).contains(&ratio),
        "back-iron gain {ratio} outside the physically expected 1.3–2.0 band"
    );
}

/// Six alternating poles on a 22.5 mm pitch circle — a magnetically balanced
/// rotor, which is what every real multi-pole machine is.
fn alternating_rotor() -> Vec<Filament> {
    (0..6)
        .map(|k| {
            let t = 2.0 * PI * (k as f64) / 6.0;
            let c = Vec3::new(0.0225 * t.cos(), 0.0225 * t.sin(), 0.003);
            let sign = if k % 2 == 0 { 1.0 } else { -1.0 };
            let pts = (0..256)
                .map(|j| {
                    let u = 2.0 * PI * (j as f64) / 256.0;
                    Vec3::new(c.x + 0.0075 * u.cos(), c.y + 0.0075 * u.sin(), c.z)
                })
                .collect();
            Filament::closed_loop(pts, 100.0 * sign, 0.0)
        })
        .collect()
}

#[test]
fn balance_residual_separates_a_rotor_from_a_lone_magnet() {
    let rotor = alternating_rotor();
    assert!(
        IronStack::balance_residual(&rotor) < 1e-9,
        "an alternating rotor must be balanced, got {}",
        IronStack::balance_residual(&rotor)
    );
    let lone = vec![ring(0.0075, 0.003, 100.0, 256)];
    assert!(
        (IronStack::balance_residual(&lone) - 1.0).abs() < 1e-9,
        "a lone loop must be maximally unbalanced"
    );
}

#[test]
fn two_plane_series_converges_for_a_balanced_rotor() {
    // Facing mirrors generate an infinite series. It converges — and is only
    // usable — when the source set carries no net moment.
    let rotor = alternating_rotor();
    let probe = Vec3::new(0.0225, 0.0, 0.0045);
    let b_at = |depth: usize| -> Vec3 {
        let stack = IronStack::pair(0.0, 0.0056, depth);
        rotor
            .iter()
            .flat_map(|s| stack.expand(s))
            .map(|f| f.b_at(probe))
            .sum()
    };
    let shallow = b_at(8);
    let deep = b_at(IronStack::DEFAULT_REFLECTIONS);
    let deeper = b_at(32);

    // Converged: doubling the depth past the default barely moves it.
    assert!(
        (deeper - deep).norm() / deeper.norm() < 1e-4,
        "not converged at default depth: {} vs {}",
        deep.norm(),
        deeper.norm()
    );
    // And the approach is monotone in depth, not oscillating.
    assert!((deep - shallow).norm() > (deeper - deep).norm());
}

#[test]
fn an_unbalanced_source_is_flagged_as_non_convergent() {
    // The failure mode the precondition exists to catch: a single magnet between
    // two infinite plates never settles, and `tail_fraction` must say so loudly
    // rather than returning a plausible number.
    let lone = ring(0.0075, 0.003, 100.0, 256);
    let probe = Vec3::new(0.018, 0.0, 0.0045);
    let stack = IronStack::pair(0.0, 0.0056, IronStack::DEFAULT_REFLECTIONS);
    let tail = stack.tail_fraction(&lone, probe);
    assert!(
        tail > 0.05,
        "an unbalanced source must report a large truncation residual, got {tail}"
    );
}

#[test]
fn two_planes_beat_one_which_beats_none() {
    // Monotone ordering is the sanity check on the whole iron model: closing
    // more of the magnetic circuit can only raise the working flux.
    let src = ring(0.02, 0.003, 5.0, 256);
    let probe = Vec3::new(0.02, 0.0, 0.0045);

    let none = b_with_iron(&IronStack::none(), &src, probe).z.abs();
    let one = b_with_iron(&IronStack::single(0.0), &src, probe).z.abs();
    let two = b_with_iron(&IronStack::pair(0.0, 0.0056, 12), &src, probe)
        .z
        .abs();

    assert!(none < one, "one plane must beat none: {none} vs {one}");
    assert!(one < two, "two planes must beat one: {one} vs {two}");
}

#[test]
fn no_iron_is_a_pure_passthrough() {
    let src = ring(0.02, 0.003, 5.0, 128);
    let stack = IronStack::none();
    assert_eq!(stack.expand(&src).len(), 1);
    assert!(stack.is_empty());
    let p = Vec3::new(0.01, 0.002, 0.006);
    assert_eq!(b_with_iron(&stack, &src, p), src.b_at(p));
    assert_eq!(stack.tail_fraction(&src, p), 0.0);
}
