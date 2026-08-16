//! Regression: hollow-box volume must track wall thickness, including through
//! a boolean cut (the vcad.io /prove demo path: cube → shell → difference).
//!
//! The analytical shell used to pre-reverse the inner loop *and* mark the
//! inner faces `Orientation::Reversed`; the tessellator reverses winding for
//! `Reversed` faces itself, so the two flips cancelled and the cavity was
//! wound outward — `volume()` reported outer + cavity, so a *thinner* wall
//! reported a *larger* volume.

use vcad_kernel::Solid;

fn analytic_shell_volume(w: f64) -> f64 {
    100.0 * 80.0 * 60.0 - (100.0 - 2.0 * w) * (80.0 - 2.0 * w) * (60.0 - 2.0 * w)
}

#[test]
fn shelled_box_volume_matches_wall_thickness() {
    for &w in &[0.8, 1.2, 2.0, 3.5, 5.0] {
        let hollow = Solid::cube(100.0, 80.0, 60.0).shell(w).expect("shell fits");
        let vol = hollow.volume();
        let expected = analytic_shell_volume(w);
        assert!(
            (vol - expected).abs() < expected * 1e-5,
            "w={w}: shelled volume {vol} != expected {expected}"
        );
    }
}

#[test]
fn shell_then_difference_volume_decreases_with_thinner_wall() {
    // Corner cutter from the /prove demo: overlaps the box at
    // x 55..100, y 45..80, z 5..60.
    let part_volume = |w: f64| -> f64 {
        let hollow = Solid::cube(100.0, 80.0, 60.0).shell(w).expect("shell fits");
        let cutter = Solid::cube(60.0, 50.0, 70.0).translate(55.0, 45.0, 5.0);
        hollow.difference(&cutter).volume()
    };

    let mut prev = 0.0;
    for &w in &[0.8, 1.2, 2.0, 3.5] {
        let vol = part_volume(w);
        // Wall material removed by the cutter = cutter∩outer − cutter∩cavity.
        let ov_outer = 45.0 * 35.0 * 55.0;
        let ov_cavity = (45.0 - w) * (35.0 - w) * (55.0 - w);
        let expected = analytic_shell_volume(w) - (ov_outer - ov_cavity);
        assert!(
            (vol - expected).abs() < expected * 1e-5,
            "w={w}: cut part volume {vol} != expected {expected}"
        );
        assert!(
            vol > prev,
            "w={w}: volume {vol} should exceed thinner-walled {prev}"
        );
        prev = vol;
    }
}
