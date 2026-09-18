//! A volume from an open shell is a guess, and it must say so.
//!
//! The divergence theorem needs a closed surface. Given an open one it still
//! returns a number, wrong by the flux through the hole — and the error
//! scales with the hole's DISTANCE FROM THE ORIGIN, not with its size, so a
//! sliver of missing wall far from centre is worth cubic millimetres. That is
//! not hypothetical: the rana-60 stator's near-tangent fillet lost a 0.98 mm²
//! wall at r 24 and its union reported 4734.78 mm³ against a true 4726.91,
//! +0.167 %, while the part's actual shape was right to 0.005 mm
//! (`docs/boolean-multilump-union-diagnosis.md`).
//!
//! `Solid::volume()` still returns the bare number — callers depend on it —
//! but `volume_report()` carries the closedness, and anything showing a user
//! a volume, a mass or a price is expected to print `caveat()` beside it.

use vcad_kernel::Solid;

/// A closed primitive needs no qualification, and the number is exact.
#[test]
fn a_closed_solid_reports_no_caveat() {
    let solid = Solid::cube(10.0, 8.0, 6.0);
    let report = solid.volume_report();
    assert!(report.closed(), "a cube's shell is closed");
    assert_eq!(report.open_edges, 0);
    assert_eq!(report.over_used_edges, 0);
    assert_eq!(report.caveat(), None, "nothing to warn about");
    assert!(
        (report.volume - 480.0).abs() < 1e-9,
        "volume {} against 480",
        report.volume
    );
    assert!((report.volume - solid.volume()).abs() < 1e-12);
}

/// Punch one triangle out and the report has to change its mind — and say
/// how far off the number now is, in the same breath as the number.
///
/// This reconstructs the near miss's defect in miniature: one missing wall
/// triangle, a shell that still looks like a part, and a volume that is
/// wrong by the flux through the hole. A 10 x 8 x 6 box at the origin loses
/// a 24 mm² half-face; the reported volume moves by tens of mm³, which is
/// exactly the size of error that used to travel with no warning attached.
#[test]
fn a_hole_makes_the_volume_approximate_and_says_so() {
    let solid = Solid::cube(10.0, 8.0, 6.0);
    let mut mesh = solid.to_mesh(16);
    let before = mesh.indices.len();
    // Drop the triangle furthest from the origin, where the flux is largest.
    let centroid = |t: usize| {
        let mut c = [0.0f64; 3];
        for k in 0..3 {
            let i = mesh.indices[t * 3 + k] as usize;
            for (a, slot) in c.iter_mut().enumerate() {
                *slot += mesh.vertices[i * 3 + a] as f64 / 3.0;
            }
        }
        c
    };
    let worst = (0..before / 3)
        .max_by(|&a, &b| {
            let n = |t: usize| {
                let c = centroid(t);
                c[0] * c[0] + c[1] * c[1] + c[2] * c[2]
            };
            n(a).partial_cmp(&n(b)).unwrap()
        })
        .expect("a triangle");
    mesh.indices.drain(worst * 3..worst * 3 + 3);

    let holed = Solid::from_mesh(mesh);
    let report = holed.volume_report();

    assert!(
        !report.closed(),
        "a box with a triangle missing is not closed"
    );
    assert!(
        report.open_edges >= 3,
        "a missing triangle leaves its three edges unpaired, got {}",
        report.open_edges
    );
    let caveat = report.caveat().expect("an open shell must carry a caveat");
    assert!(
        caveat.contains("not closed") && caveat.contains("approximate"),
        "unhelpful caveat: {caveat}"
    );
    // The number is still returned — it is the WARNING that is new — and it
    // is wrong, which is the whole point.
    assert!(
        (report.volume - 480.0).abs() > 1.0,
        "the hole should move the volume measurably; got {}",
        report.volume
    );
}
