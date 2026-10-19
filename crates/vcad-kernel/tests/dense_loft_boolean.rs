//! Regression: a boolean against a ruled loft of dense polyline sections
//! must finish in seconds, not minutes.
//!
//! A humanoid helmet (cube − (cylinder − loft), 25 sections × 56 line
//! segments → ~1400 planar quads) spent ~10 minutes in the B-rep pipeline's
//! face-pair stages (SSI trimming / point-in-face), after which the result
//! was rejected as "removed no volume" and re-cut with the mesh boolean
//! anyway. Dense faceted operands now go straight to the mesh boolean.

use std::time::Instant;

use vcad_kernel::Solid;
use vcad_kernel_math::{Point2, Point3, Vec3};
use vcad_kernel_sketch::{SketchProfile, SketchSegment};
use vcad_kernel_sweep::LoftOptions;

/// Rounded rectangle `w × h` with corner radius `r`, as a closed polyline of
/// `4 * per_corner` arc chords plus four straight sides (the shape the
/// humanoid shell sections use).
fn rounded_rect(z: f64, w: f64, h: f64, r: f64, per_corner: usize) -> SketchProfile {
    let (hx, hy) = (w / 2.0 - r, h / 2.0 - r);
    let mut pts = Vec::new();
    for (k, (cx, cy)) in [(hx, hy), (-hx, hy), (-hx, -hy), (hx, -hy)]
        .iter()
        .enumerate()
    {
        let a0 = k as f64 * std::f64::consts::FRAC_PI_2;
        for i in 0..=per_corner {
            let a = a0 + std::f64::consts::FRAC_PI_2 * i as f64 / per_corner as f64;
            pts.push(Point2::new(cx + r * a.cos(), cy + r * a.sin()));
        }
    }
    let n = pts.len();
    let segments = (0..n)
        .map(|i| SketchSegment::Line {
            start: pts[i],
            end: pts[(i + 1) % n],
        })
        .collect();
    SketchProfile::new(Point3::new(0.0, 0.0, z), Vec3::x(), Vec3::y(), segments).unwrap()
}

fn dense_loft() -> Solid {
    // 25 sections × 56 segments, tapering and swelling like a shell.
    let sections: Vec<SketchProfile> = (0..25)
        .map(|i| {
            let t = i as f64 / 24.0;
            let s = 1.0 + 0.25 * (t * std::f64::consts::PI).sin();
            rounded_rect(5.0 + 40.0 * t, 40.0 * s, 30.0 * s, 8.0, 13)
        })
        .collect();
    assert_eq!(sections[0].segments.len(), 56);
    Solid::loft(&sections, LoftOptions::default()).unwrap()
}

#[test]
fn difference_with_dense_loft_is_fast_and_correct() {
    let loft = dense_loft();
    let loft_vol = loft.volume();
    assert!(loft_vol > 0.0);

    // Loft sits entirely inside the block: the cut removes exactly its volume.
    let block = Solid::cube(80.0, 80.0, 60.0).translate(-40.0, -40.0, 0.0);
    let t = Instant::now();
    let cut = block.difference(&loft);
    let elapsed = t.elapsed();

    let expected = 80.0 * 80.0 * 60.0 - loft_vol;
    let vol = cut.volume();
    assert!(
        (vol - expected).abs() < expected * 1e-3,
        "cut volume {vol} != block − loft {expected}"
    );
    // Was minutes; the mesh path takes well under a second in release.
    // Generous so a debug build on a loaded CI runner still passes.
    assert!(
        elapsed.as_secs() < 60,
        "dense-loft difference took {elapsed:?}"
    );
}

#[test]
fn helmet_shaped_chain_is_fast() {
    // cube − (cylinder − loft): the exact chain from the field report.
    let loft = dense_loft();
    let cyl = Solid::cylinder(40.0, 80.0, 72)
        .rotate(-90.0, 0.0, 0.0)
        .translate(0.0, -40.0, 0.0);
    let block = Solid::cube(92.0, 80.0, 50.0).translate(-46.0, -40.0, -50.0);
    let t = Instant::now();
    let helmet = block.difference(&cyl.difference(&loft));
    let elapsed = t.elapsed();
    assert!(helmet.volume() > 0.0);
    assert!(elapsed.as_secs() < 60, "helmet chain took {elapsed:?}");
}
