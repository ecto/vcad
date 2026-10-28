//! A section through a solid built by several booleans must hatch every
//! closed region of the cut.
//!
//! Regression for the vcad.io drawing card (2026-10-27): an 80 × 50 × 8 plate,
//! filleted, then cut by an Ø18 bore and two Ø4.4 bolt holes, sectioned on
//! y = 25 through the bore. The cut is two rectangles either side of the
//! bore; the bolt holes never meet the plane. Before the fix, `section_mesh`
//! chained one of the two outlines open (a stray two-point curve split off
//! it) and so hatched only the other one.

use vcad_kernel::Solid;
use vcad_kernel_drafting::{section_mesh, HatchPattern, Point2D, SectionPlane, SectionView};

fn plane() -> SectionPlane {
    SectionPlane::from_arrays([40.0, 25.0, 4.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0])
}

fn hatch() -> HatchPattern {
    // exactly what the page sent: {"spacing":1.6,"angle":45}. angle is in
    // radians, so this is 45 rad, not 45° — kept because it is the input
    // that failed.
    HatchPattern {
        spacing: 1.6,
        angle: 45.0,
    }
}

fn plate_minus_bore() -> Solid {
    let plate = Solid::cube(80.0, 50.0, 8.0).fillet(3.0).expect("fillet");
    let bore = Solid::cylinder(9.0, 10.0, 32).translate(40.0, 25.0, -1.0);
    plate.difference(&bore)
}

fn bracket() -> Solid {
    let bolt = Solid::cylinder(2.2, 10.0, 32);
    plate_minus_bore()
        .difference(&bolt.translate(8.0, 8.0, -1.0))
        .difference(&bolt.translate(72.0, 42.0, -1.0))
}

/// Hatch lines whose midpoint falls on the given side of x = 0.
fn hatch_on_side(lines: &[(Point2D, Point2D)], left: bool) -> usize {
    lines
        .iter()
        .filter(|(a, b)| ((a.x + b.x) / 2.0 < 0.0) == left)
        .count()
}

fn describe(view: &SectionView) -> String {
    view.curves
        .iter()
        .map(|c| {
            let (lo, hi) = c.points.iter().fold((f64::MAX, f64::MIN), |(lo, hi), p| {
                (lo.min(p.x), hi.max(p.x))
            });
            format!(
                "[{} pts, closed={}, x {:.2}..{:.2}]",
                c.points.len(),
                c.is_closed,
                lo,
                hi
            )
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn control_two_booleans_hatches_both_regions() {
    let view = section_mesh(&plate_minus_bore().to_mesh(32), &plane(), Some(&hatch()));
    eprintln!("control: {}", describe(&view));
    assert!(
        hatch_on_side(&view.hatch_lines, true) > 0,
        "left region unhatched: {}",
        describe(&view)
    );
    assert!(
        hatch_on_side(&view.hatch_lines, false) > 0,
        "right region unhatched: {}",
        describe(&view)
    );
}

#[test]
fn four_booleans_hatches_both_regions() {
    let view = section_mesh(&bracket().to_mesh(32), &plane(), Some(&hatch()));
    eprintln!("bracket: {}", describe(&view));
    let closed = view.curves.iter().filter(|c| c.is_closed).count();
    assert_eq!(
        closed,
        2,
        "expected two closed outlines: {}",
        describe(&view)
    );
    assert!(
        view.curves.iter().all(|c| c.is_closed),
        "no open fragments expected in a cut through a closed solid: {}",
        describe(&view)
    );
    assert!(
        hatch_on_side(&view.hatch_lines, true) > 0,
        "left region unhatched: {}",
        describe(&view)
    );
    assert!(
        hatch_on_side(&view.hatch_lines, false) > 0,
        "right region unhatched: {}",
        describe(&view)
    );
}

#[test]
fn bracket_section_matches_plate_minus_bore() {
    // The bolt holes never meet y = 25, so the two sections are the same
    // geometry and should hatch the same amount, give or take a line.
    let a = section_mesh(&plate_minus_bore().to_mesh(32), &plane(), Some(&hatch()))
        .hatch_lines
        .len() as i64;
    let b = section_mesh(&bracket().to_mesh(32), &plane(), Some(&hatch()))
        .hatch_lines
        .len() as i64;
    assert!(
        (a - b).abs() <= 2,
        "plate−bore hatches {a} lines, bracket {b}"
    );
}
