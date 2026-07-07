//! Tracked regression: intersecting two deep union trees of coaxial
//! cylinders / annuli must be empty when every leaf pair is disjoint.
//!
//! Reported repro: a motor-style assembly split into a rotating group
//! (rotor discs + hub + shaft, all coaxial on Z) and a static group
//! (base + tower + bearings + plates + screws). Every rotating part
//! clears every static part — shaft r4 rides inside bearing bores
//! r4.05, plate holes r5/r8, and the tower bore r5.5; the rotor discs
//! clear the screw heads radially. Checked pairwise, every intersection
//! correctly returns zero. But intersecting the two *union trees*
//! reported ~661 mm³ of phantom volume with a geometrically impossible
//! surface-area-to-volume ratio (a phantom shaft segment, bbox r≤4,
//! z 11–28.7).
//!
//! Root cause (fixed): `split_planar_face_by_circle`'s degenerate-outer-
//! loop branch created the inner disk sub-face without routing the cap's
//! pre-existing inner loops to it. Unioning an annulus onto a body whose
//! bore meets the annulus face (bearing seated on the tower pocket, plate
//! stacked on plate) split the annular cap by the bore circle and dropped
//! the annulus's own hole — leaving a phantom membrane sealing the bore
//! mouth at z=11 and z=28.7. The membranes made the axial column between
//! them read as solid, so the final intersection fabricated disk faces
//! there (correctly classified Inside the shaft) and sewed them into a
//! 661 mm³ open shell.

use vcad_kernel::Solid;

/// Solid cylinder on the Z axis spanning [z0, z1].
fn cyl(r: f64, z0: f64, z1: f64) -> Solid {
    Solid::cylinder(r, z1 - z0, 0).translate(0.0, 0.0, z0)
}

/// Off-axis solid cylinder spanning [z0, z1] at (x, y).
fn cyl_at(r: f64, x: f64, y: f64, z0: f64, z1: f64) -> Solid {
    Solid::cylinder(r, z1 - z0, 0).translate(x, y, z0)
}

/// Annular disc: outer radius `ro`, concentric bore `ri`, spanning [z0, z1].
fn annulus(ro: f64, ri: f64, z0: f64, z1: f64) -> Solid {
    let disc = cyl(ro, z0, z1);
    // Over-long hole so the difference caps cleanly past both faces.
    let hole = cyl(ri, z0 - 1.0, z1 + 1.0);
    disc.difference(&hole)
}

/// Three cylinders at 120° spacing on a bolt circle of radius 32.5.
fn bolt_circle(r: f64, z0: f64, z1: f64) -> Solid {
    let mut result: Option<Solid> = None;
    for i in 0..3 {
        let a = 2.0 * std::f64::consts::PI * (i as f64) / 3.0;
        let c = cyl_at(r, 32.5 * a.cos(), 32.5 * a.sin(), z0, z1);
        result = Some(match result {
            Some(acc) => acc.union(&c),
            None => c,
        });
    }
    result.unwrap()
}

/// Rotating group: two rotor discs, the hub, and the shaft.
fn rotating_group() -> Solid {
    let disc1 = annulus(29.0, 4.2, 31.8, 33.4);
    let disc2 = annulus(29.0, 4.2, 33.4, 36.1);
    let hub = cyl(11.0, 36.1, 39.1)
        .union(&cyl(7.5, 39.1, 47.1))
        .difference(&cyl(4.05, 36.1 - 1.0, 47.1 + 1.0));
    let shaft = cyl(4.0, 8.0, 47.0);
    disc1.union(&disc2).union(&hub).union(&shaft)
}

/// Static group: base + tower (with bore and bearing pocket), standoff
/// screws, two stator plates, screw heads, and two bearings.
fn static_group() -> Solid {
    let base = cyl(40.0, 0.0, 6.0);
    let tower = cyl(12.0, 6.0, 25.0);
    let bore = cyl(5.5, 6.0 - 1.0, 25.0 + 1.0);
    let pocket = cyl(11.0, 11.0, 25.2);
    let base_tower = base.union(&tower).difference(&bore).difference(&pocket);

    let screws = bolt_circle(2.5, 6.0, 26.0);
    let plate_lower = annulus(35.0, 8.0, 26.0, 28.7);
    let plate_upper = annulus(35.0, 5.0, 28.7, 30.3);
    let screw_heads = bolt_circle(2.85, 30.3, 31.95);
    let bearing_lower = annulus(11.0, 4.05, 11.0, 18.0);
    let bearing_upper = annulus(11.0, 4.05, 18.0, 25.0);

    base_tower
        .union(&screws)
        .union(&plate_lower)
        .union(&plate_upper)
        .union(&screw_heads)
        .union(&bearing_lower)
        .union(&bearing_upper)
}

/// The exact reported failure: the full trees must intersect to nothing.
#[test]
fn coaxial_union_trees_intersect_to_empty() {
    let rot = rotating_group();
    let sta = static_group();

    // Sanity: both operands are substantial, sane solids.
    let (v_rot, v_sta) = (rot.volume(), sta.volume());
    assert!(v_rot > 1_000.0, "rotating group degenerate: vol={v_rot}");
    assert!(v_sta > 10_000.0, "static group degenerate: vol={v_sta}");

    let overlap = rot.intersection(&sta);
    let v = overlap.volume();
    let a = overlap.surface_area();
    assert!(
        v.abs() < 1e-3,
        "phantom intersection: volume={v:.3} mm³ (surface_area={a:.3} mm², want 0)"
    );
}

/// Guards: the pairwise checks that were already correct must stay zero.
#[test]
fn coaxial_pairwise_intersections_stay_empty() {
    let shaft = cyl(4.0, 8.0, 47.0);
    let bearing = annulus(11.0, 4.05, 11.0, 18.0);
    assert!(shaft.intersection(&bearing).volume().abs() < 1e-3);

    let base_tower = cyl(40.0, 0.0, 6.0)
        .union(&cyl(12.0, 6.0, 25.0))
        .difference(&cyl(5.5, 5.0, 26.0))
        .difference(&cyl(11.0, 11.0, 25.2));
    assert!(shaft.intersection(&base_tower).volume().abs() < 1e-3);

    let disc = annulus(29.0, 4.2, 31.8, 33.4);
    let heads = bolt_circle(2.85, 30.3, 31.95);
    assert!(disc.intersection(&heads).volume().abs() < 1e-3);
}
