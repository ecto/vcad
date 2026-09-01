//! The acceptance protocol: the checker must FAIL each known-defective mesh
//! with the RIGHT diagnosis, and pass the known-good ones. A checker that
//! cannot demonstrate failure proves nothing.

mod fixtures;

use vcad_printcheck::{check_file, FindingKind, Options};

fn opts() -> Options {
    Options {
        pitch: 0.25,
        ..Default::default()
    }
}

#[test]
fn good_cube_passes() {
    let r = check_file(&fixtures::good_cube(), &opts()).unwrap();
    assert!(r.ok, "plain cube should be clean:\n{:#?}", r.findings);
    assert_eq!(r.manifold.bad_edges, 0);
    assert_eq!(r.columns.cracks, 0);
    assert_eq!(r.columns.floating_regions, 0);
}

#[test]
fn good_bridge_passes_and_is_reported_as_a_bridge() {
    let r = check_file(&fixtures::good_bridge(), &opts()).unwrap();
    assert!(r.ok, "3 mm span is inside convention:\n{:#?}", r.findings);
    let b = r
        .columns
        .bridges
        .iter()
        .find(|b| b.z > 5.0 && b.z < 7.0)
        .expect("the roof should be reported as a bridge span");
    assert!(
        b.span_mm > 2.0 && b.span_mm <= 4.0,
        "span {} mm should measure the ~3 mm clear gap",
        b.span_mm
    );
    assert!(b.anchored);
}

#[test]
fn crack_of_005mm_fails_as_a_crack() {
    let r = check_file(&fixtures::crack_005(), &opts()).unwrap();
    assert!(!r.ok, "a 0.05 mm interior gap must be dirty");
    assert!(
        r.has(FindingKind::Crack),
        "expected a Crack diagnosis, got {:#?}",
        r.findings
    );
    let gap = r.columns.thinnest_gap_mm.unwrap();
    assert!(
        (gap - 0.05).abs() < 0.01,
        "should measure the 0.05 mm gap, measured {gap}"
    );
}

#[test]
fn crack_is_never_whitelistable() {
    let o = Options {
        allow_bridges: vec![(0.0, 100.0)],
        ..opts()
    };
    let r = check_file(&fixtures::crack_005(), &o).unwrap();
    assert!(
        !r.ok && r.has(FindingKind::Crack),
        "a bridge whitelist must not silence a crack"
    );
}

#[test]
fn floating_island_fails_as_a_floating_region() {
    let r = check_file(&fixtures::floating_island(), &opts()).unwrap();
    assert!(!r.ok, "a cube hanging in mid-air must be dirty");
    assert!(
        r.has(FindingKind::FloatingRegion),
        "expected a FloatingRegion diagnosis, got {:#?}",
        r.findings
    );
    let f = r
        .failures()
        .find(|f| f.kind == FindingKind::FloatingRegion)
        .unwrap();
    let z = f.location.unwrap()[2];
    assert!(
        (z - 5.0).abs() < 0.5,
        "should locate the island at its underside z=5, reported {z}"
    );
}

#[test]
fn thin_wall_of_02mm_fails_against_a_04_nozzle() {
    let r = check_file(&fixtures::thin_wall_02(), &opts()).unwrap();
    assert!(!r.ok, "a 0.2 mm wall at a 0.4 nozzle must be dirty");
    assert!(
        r.has(FindingKind::ThinWall),
        "expected a ThinWall diagnosis, got {:#?}",
        r.findings
    );
    let t = r.walls.min_feature_mm.unwrap();
    assert!(
        (t - 0.2).abs() < 0.05,
        "should measure the 0.2 mm wall, measured {t}"
    );
}

#[test]
fn thin_wall_passes_with_a_02_nozzle() {
    // The same mesh is printable on a finer nozzle — proof the check reads the
    // nozzle rather than a hard-coded floor.
    let o = Options {
        nozzle: 0.15,
        ..opts()
    };
    let r = check_file(&fixtures::thin_wall_02(), &o).unwrap();
    assert!(
        !r.has(FindingKind::ThinWall),
        "0.2 mm clears a 0.15 mm nozzle:\n{:#?}",
        r.findings
    );
}

#[test]
fn overlong_bridge_fails_with_its_span() {
    let r = check_file(&fixtures::overlong_bridge(), &opts()).unwrap();
    assert!(!r.ok, "a 12 mm span at a 4 mm limit must be dirty");
    assert!(
        r.has(FindingKind::OverlongBridge),
        "expected an OverlongBridge diagnosis, got {:#?}",
        r.findings
    );
    let f = r
        .failures()
        .find(|f| f.kind == FindingKind::OverlongBridge)
        .unwrap();
    let span = f.value_mm.unwrap();
    assert!(
        span > 10.0,
        "should report the ~12 mm span, reported {span}"
    );
}

#[test]
fn overlong_bridge_can_be_whitelisted_by_height() {
    let o = Options {
        allow_bridges: vec![(5.5, 6.5)],
        ..opts()
    };
    let r = check_file(&fixtures::overlong_bridge(), &o).unwrap();
    assert!(
        !r.has(FindingKind::OverlongBridge),
        "a documented bridge zone should clear the span verdict:\n{:#?}",
        r.findings
    );
}

#[test]
fn holed_cube_fails_manifold_and_sections() {
    let r = check_file(&fixtures::holed_cube(), &opts()).unwrap();
    assert!(!r.ok);
    assert!(r.manifold.bad_edges > 0);
    assert!(r.has(FindingKind::NonManifold));
}

#[test]
fn orientation_changes_the_verdict() {
    // The 0.2 mm wall stands along Z. Lay the part on its side and the wall is
    // still 0.2 mm — the check must find it from any orientation, which is the
    // whole reason it casts along all three axes.
    let o = Options {
        orientation: vcad_printcheck::Orientation::XUp,
        ..opts()
    };
    let r = check_file(&fixtures::thin_wall_02(), &o).unwrap();
    assert!(
        r.has(FindingKind::ThinWall),
        "the wall must be found with the part rotated:\n{:#?}",
        r.findings
    );
}
