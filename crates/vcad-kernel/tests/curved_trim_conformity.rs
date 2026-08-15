//! Trimmed curved faces must share their boundary samples with whatever
//! is on the other side of the seam.
//!
//! A face a boolean leaves whole keeps its analytic seam loop, which
//! records nothing about how the split sampled the circle they now
//! share; the faces that close it off DO carry that ring. When the two
//! sides pick their own schedules the shell zippers open along every
//! such seam — no volume lost, just a mesh that is not closed. A bore
//! through a plain, unfilleted box was the smallest instance.

use vcad_kernel::Solid;

const SEGMENTS: u32 = 64;

fn open_edges(solid: &Solid) -> usize {
    solid.to_mesh(SEGMENTS).boundary_edges().len()
}

/// The smallest case: a through-bore in a plain box. Nothing curved is
/// filleted, nothing is re-split — the wall is one whole cylinder and
/// the two caps carry its ring as a hole loop.
#[test]
fn bore_through_plain_box_is_watertight() {
    let box_solid = Solid::cube(100.0, 100.0, 100.0);
    let bore = Solid::cylinder(10.0, 200.0, 48).translate(50.0, 50.0, -50.0);
    let result = box_solid.difference(&bore);

    assert_eq!(open_edges(&result), 0, "bore seam left the shell open");

    // A 48-gon bore removes slightly less than the true cylinder, so the
    // result sits just above the analytic value — that gap is chord
    // error, and it must stay that small.
    let analytic = 1.0e6 - std::f64::consts::PI * 100.0 * 100.0;
    let volume = result.volume();
    assert!(
        volume >= analytic && volume < analytic * 1.001,
        "{volume} vs analytic {analytic}"
    );
}

/// The same bore through a filleted box: the blends are untouched by the
/// cut, so this must be exactly as clean as the plain case.
#[test]
fn bore_through_filleted_box_is_watertight() {
    let filleted = Solid::cube(100.0, 100.0, 100.0)
        .fillet(12.0)
        .expect("r=12 fits a 100mm cube");
    let bore = Solid::cylinder(10.0, 200.0, 48).translate(50.0, 50.0, -50.0);
    let result = filleted.difference(&bore);

    assert_eq!(open_edges(&result), 0);
    // The bore removes π·r²·h from the filleted body; the blends are far
    // from it, so the whole difference is the bore.
    let expected = filleted.volume() - std::f64::consts::PI * 100.0 * 100.0;
    assert!(
        (result.volume() - expected).abs() < expected * 0.001,
        "{} vs {expected}",
        result.volume()
    );
}

/// Off-centre and non-axis-aligned bores exercise the same seam without
/// landing on the cylinder's u = 0 seam by luck.
#[test]
fn offset_bores_are_watertight() {
    let box_solid = Solid::cube(60.0, 60.0, 40.0);
    for (x, y, r) in [(20.0, 20.0, 6.0), (37.5, 22.5, 4.0), (30.0, 30.0, 12.5)] {
        let bore = Solid::cylinder(r, 200.0, 32).translate(x, y, -50.0);
        let result = box_solid.difference(&bore);
        assert_eq!(
            open_edges(&result),
            0,
            "bore r={r} at ({x},{y}) left the shell open"
        );
    }
}

/// Two bores through one box: each cap ends up with two hole rings, and
/// each wall must adopt its own.
#[test]
fn two_bores_share_a_cap_and_stay_watertight() {
    let box_solid = Solid::cube(80.0, 40.0, 20.0);
    let a = Solid::cylinder(5.0, 100.0, 32).translate(20.0, 20.0, -40.0);
    let b = Solid::cylinder(7.0, 100.0, 32).translate(60.0, 20.0, -40.0);
    let result = box_solid.difference(&a).difference(&b);

    assert_eq!(open_edges(&result), 0);
    let expected = 80.0 * 40.0 * 20.0 - std::f64::consts::PI * (25.0 + 49.0) * 20.0;
    assert!(
        (result.volume() - expected).abs() < expected * 0.01,
        "{} vs {expected}",
        result.volume()
    );
}

/// A holed planar face must not gain vertices of its own. The bridge +
/// ear-clip path used to drop a Steiner point every 8 mm along the
/// OUTER ring — a ring it shares with a neighbor that has no way to
/// know, so every one was an unweldable T-junction.
#[test]
fn holed_planar_face_keeps_its_outer_ring() {
    let plate = Solid::cube(200.0, 200.0, 10.0);
    let bore = Solid::cylinder(20.0, 100.0, 48).translate(100.0, 100.0, -40.0);
    let result = plate.difference(&bore);

    assert_eq!(
        open_edges(&result),
        0,
        "a 200mm face with a hole cracked along its own outer ring"
    );
}
