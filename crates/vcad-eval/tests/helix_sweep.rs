//! Acceptance tests for the `helix` / `cam-path` path primitives and the
//! `sweep` solid built on them (issue #842).
//!
//! The motivating case is the `rana` 60c shell, whose J-slot cam tracks had
//! to be approximated twice — first as micro z-bands, then as a hand-written
//! per-0.5° sector-column sweep evaluator living in a project script. The
//! numbers below are that part's real dimensions: r34.5 tracks, a floor
//! rising 0.0667 mm/° over a 15° pin travel, and a rise–plateau–drop detent
//! at d9.6..11.6.
//!
//! Three things are checked, in the order they can break:
//!   (a) the swept solid is watertight on its own and lands on the path,
//!   (b) it still cuts a closed section out of a body when subtracted, and
//!   (c) the resulting floor is the analytic profile, not an approximation.

use std::collections::HashMap;

use vcad_eval::{evaluate_document, EvalOptions};
use vcad_kernel_tessellate::TriangleMesh as Mesh;
use vcad_loon::eval_vcad;

fn mesh_of(src: &str) -> Mesh {
    let doc = eval_vcad(src, None).expect("eval_vcad");
    let scene = evaluate_document(
        &doc,
        &EvalOptions {
            skip_clash_detection: true,
            clock: None,
            root_cache: None,
            mesh_segments: 0,
        },
    )
    .expect("evaluate_document");
    let solid = scene.parts[0].solid.as_ref().expect("root solid");
    let mesh = solid.to_mesh(0);
    assert!(!mesh.indices.is_empty(), "empty mesh");
    mesh
}

fn vert(mesh: &Mesh, i: u32) -> [f64; 3] {
    let v = &mesh.vertices[3 * i as usize..3 * i as usize + 3];
    [v[0] as f64, v[1] as f64, v[2] as f64]
}

/// Count undirected edges not shared by exactly two triangles. Zero means the
/// mesh is closed — the standalone-watertightness assertion.
fn non_manifold_edges(mesh: &Mesh) -> usize {
    let key = |i: u32| {
        let v = vert(mesh, i);
        [
            (v[0] * 1e4).round() as i64,
            (v[1] * 1e4).round() as i64,
            (v[2] * 1e4).round() as i64,
        ]
    };
    let mut edges: HashMap<_, usize> = HashMap::new();
    for t in mesh.indices.chunks_exact(3) {
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            let (ka, kb) = (key(a), key(b));
            let e = if ka <= kb { (ka, kb) } else { (kb, ka) };
            *edges.entry(e).or_default() += 1;
        }
    }
    edges.values().filter(|&&c| c != 2).count()
}

fn z_span(mesh: &Mesh) -> (f64, f64) {
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    for v in mesh.vertices.chunks_exact(3) {
        lo = lo.min(v[2] as f64);
        hi = hi.max(v[2] as f64);
    }
    (lo, hi)
}

/// Heights at which the vertical line through `(x, y)` passes through the
/// mesh, i.e. every triangle whose XY projection contains the point.
///
/// This is how the floor of a track is measured on the real part: drop a
/// probe down the slot at a known angle and read where it lands.
fn heights_above(mesh: &Mesh, x: f64, y: f64) -> Vec<f64> {
    let mut zs = Vec::new();
    for t in mesh.indices.chunks_exact(3) {
        let (a, b, c) = (vert(mesh, t[0]), vert(mesh, t[1]), vert(mesh, t[2]));
        let d = (b[1] - c[1]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[1] - c[1]);
        if d.abs() < 1e-12 {
            continue; // vertical triangle: no XY area to land in
        }
        let l0 = ((b[1] - c[1]) * (x - c[0]) + (c[0] - b[0]) * (y - c[1])) / d;
        let l1 = ((c[1] - a[1]) * (x - c[0]) + (a[0] - c[0]) * (y - c[1])) / d;
        let l2 = 1.0 - l0 - l1;
        if l0 < -1e-9 || l1 < -1e-9 || l2 < -1e-9 {
            continue;
        }
        zs.push(l0 * a[2] + l1 * b[2] + l2 * c[2]);
    }
    zs
}

fn floor_at(mesh: &Mesh, radius: f64, deg: f64) -> f64 {
    let r = deg.to_radians();
    let zs = heights_above(mesh, radius * r.cos(), radius * r.sin());
    assert!(
        !zs.is_empty(),
        "no material over the probe at r{radius} θ{deg}° — the sweep does not \
         reach the angle it was asked for"
    );
    zs.into_iter().fold(f64::INFINITY, f64::min)
}

// ---------------------------------------------------------------------------
// (a) The swept solid on its own
// ---------------------------------------------------------------------------

const RATE: f64 = 0.0667; // mm of rise per degree of arc
const ARC: f64 = 15.0; // the pin's full travel
const TRACK_R: f64 = 34.5;

/// A 3 mm × 2 mm channel section carried 15° around r34.5.
const RECT_SWEEP: &str = r#"[root
  [pipe [profile-rect 3.0 2.0] [sweep [helix 34.5 0.0667 0.0 15.0]]]
  "aluminum"]"#;

#[test]
fn rectangle_swept_along_a_helix_is_watertight_and_rises_at_the_stated_rate() {
    let mesh = mesh_of(RECT_SWEEP);

    assert_eq!(
        non_manifold_edges(&mesh),
        0,
        "the swept solid is not closed standing alone — it cannot be trusted \
         as a subtraction tool"
    );

    // 15° at the default 0.5° step is 30 path segments, so 31 rings of 4
    // profile vertices plus two caps. Anything near that is fine; the point
    // of the bound is to catch a sweep that silently collapsed to a couple of
    // rings or blew up into per-triangle vertices.
    let n_verts = mesh.vertices.len() / 3;
    assert!(
        (100..=4000).contains(&n_verts),
        "vertex count {n_verts} is not consistent with a 0.5°-faceted 15° sweep"
    );

    // The solid spans the profile's 2 mm axial height plus the helix's rise.
    let (lo, hi) = z_span(&mesh);
    let rise = (hi - lo) - 2.0;
    assert!(
        (rise - RATE * ARC).abs() < 0.01,
        "z-rise {rise:.4} does not match rate × arc = {:.4}",
        RATE * ARC
    );

    // ...and the floor itself follows the helix, not just the bounding box.
    // Probe just inside each end: the vertical line at exactly 0° or 15°
    // grazes a cap face rather than landing on the floor.
    let floor0 = floor_at(&mesh, TRACK_R, 0.05);
    let floor_end = floor_at(&mesh, TRACK_R, ARC - 0.05);
    assert!(
        ((floor_end - floor0) - RATE * (ARC - 0.1)).abs() < 0.01,
        "floor rise {:.4} does not match rate × arc",
        floor_end - floor0
    );
}

#[test]
fn a_negative_arc_sweeps_the_other_way_and_is_still_closed() {
    let mesh = mesh_of(
        r#"[root [pipe [profile-rect 3.0 2.0] [sweep [helix 34.5 0.0667 20.0 -15.0]]] "aluminum"]"#,
    );
    assert_eq!(
        non_manifold_edges(&mesh),
        0,
        "left-hand track is not closed"
    );
    let (lo, hi) = z_span(&mesh);
    assert!(((hi - lo) - 2.0 - RATE * ARC).abs() < 0.01);
}

#[test]
fn the_facet_step_is_configurable_and_self_consistent() {
    let coarse = mesh_of(
        r#"[root [pipe [profile-rect 3.0 2.0] [sweep [helix-res 5.0 34.5 0.0667 0.0 15.0]]] "aluminum"]"#,
    );
    let fine = mesh_of(
        r#"[root [pipe [profile-rect 3.0 2.0] [sweep [helix-res 0.1 34.5 0.0667 0.0 15.0]]] "aluminum"]"#,
    );
    assert_eq!(non_manifold_edges(&coarse), 0);
    assert_eq!(non_manifold_edges(&fine), 0);
    // 3 segments vs 150 — the step has to actually reach the mesh.
    assert!(
        fine.vertices.len() > 10 * coarse.vertices.len(),
        "seg-deg did not change the facet count: {} vs {}",
        coarse.vertices.len() / 3,
        fine.vertices.len() / 3
    );
    // Both land on the same helix, coarse faceting notwithstanding.
    for probe in [0.05, 14.95] {
        let d = floor_at(&coarse, TRACK_R, probe) - floor_at(&fine, TRACK_R, probe);
        // Mesh vertices are f32, so the bar is float noise, not exactness.
        assert!(d.abs() < 1e-4, "facet step moved the endpoints by {d}");
    }
}

// ---------------------------------------------------------------------------
// (b) Subtracted through a wall — the through-slot case
// ---------------------------------------------------------------------------

/// A tube of ID r33.3 / OD r35.7 with a 15° track cut clean through the wall:
/// the profile spans r32.5..r36.5, so the void exits both faces.
///
/// `difference` is subject-last — `[difference TOOL SUBJECT]` — so the tool
/// (the swept void) comes first, and the bore is the tool of the inner
/// difference. The bore overhangs the stock by 1 mm at each end so the two
/// cylinders do not share a coplanar cap; that is a standing workaround for
/// the boolean kernel, not something this feature introduced.
const TUBE_WITH_SLOT: &str = r#"[root
  [difference
    [translate 0.0 0.0 8.0
      [pipe [profile-polyline #[-2.0 0.0  2.0 0.0  2.0 2.4  -2.0 2.4]]
            [sweep [helix 34.5 0.0667 0.0 15.0]]]]
    [difference [translate 0.0 0.0 -1.0 [cylinder-n 33.3 22.0 96]]
                [cylinder-n 35.7 20.0 96]]]
  "aluminum"]"#;

#[test]
fn a_through_wall_slot_leaves_closed_sections() {
    let mesh = mesh_of(TUBE_WITH_SLOT);

    // Adapted from rana's tools/slice-check.py: a manifold check asks whether
    // the whole mesh is watertight; this asks the question that actually
    // stops a print — at height z, do the triangle/plane intersections join
    // up into closed loops? Every vertex of a closed section has even degree.
    let mut checked = 0;
    let mut open_sections = Vec::new();
    // Offset off the round numbers: a sample plane landing exactly on a
    // horizontal face gives a degenerate section, and a slicer nudges off it
    // too.
    let mut z = 1.037;
    while z < 19.0 {
        let (segs, odd) = section(&mesh, z);
        if segs > 0 {
            checked += 1;
            if odd > 0 {
                open_sections.push((z, odd));
            }
        }
        z += 0.4;
    }
    assert!(checked > 30, "only {checked} sections had any material");
    assert!(
        open_sections.is_empty(),
        "{} of {checked} sections do not close; first at z={:.2} with {} loose ends",
        open_sections.len(),
        open_sections[0].0,
        open_sections[0].1
    );
}

/// Intersect the mesh with the plane `z`, returning (segment count, number of
/// section vertices with odd degree). Odd degree = a loose end = an open loop.
fn section(mesh: &Mesh, z: f64) -> (usize, usize) {
    let q = |v: f64| (v * 100.0).round() as i64;
    let mut deg: HashMap<(i64, i64), usize> = HashMap::new();
    let mut segs = 0usize;
    for t in mesh.indices.chunks_exact(3) {
        let tri = [vert(mesh, t[0]), vert(mesh, t[1]), vert(mesh, t[2])];
        let mut pts = Vec::new();
        for i in 0..3 {
            let (a, b) = (tri[i], tri[(i + 1) % 3]);
            if (a[2] - z) * (b[2] - z) < 0.0 {
                let f = (z - a[2]) / (b[2] - a[2]);
                pts.push((q(a[0] + f * (b[0] - a[0])), q(a[1] + f * (b[1] - a[1]))));
            } else if (a[2] - z).abs() < 1e-9 {
                pts.push((q(a[0]), q(a[1])));
            }
        }
        pts.dedup();
        pts.sort_unstable();
        pts.dedup();
        if pts.len() == 2 {
            segs += 1;
            *deg.entry(pts[0]).or_default() += 1;
            *deg.entry(pts[1]).or_default() += 1;
        }
    }
    (segs, deg.values().filter(|d| *d % 2 == 1).count())
}

// ---------------------------------------------------------------------------
// (c) The detent floor against its analytic profile
// ---------------------------------------------------------------------------

/// rana's bottom-leg floor, verbatim: a 0.0667 mm/° ramp from -0.25 at d3.6 to
/// +0.15 at d9.6, a 0.1 mm rise–plateau–drop detent over d9.6..11.6, then a
/// flat pocket to the d18.4 stop. The profile section is the trapezoid land
/// the pin rides on.
const DETENT_TRACK: &str = r#"[root
  [pipe [profile-polyline #[-1.6 0.0  1.6 0.0  1.3 1.6  -1.3 1.6]]
        [sweep [cam-path 34.5 #[3.6 -0.25  9.6 0.15  10.3 0.25  11.3 0.25
                                11.6 0.15  18.4 0.15]]]]
  "aluminum"]"#;

/// The same numbers as a plain function, so the mesh is compared against the
/// drawing rather than against itself.
fn analytic_floor(d: f64) -> f64 {
    let bump = if d <= 9.6 || d >= 11.6 {
        0.0
    } else if d < 10.3 {
        0.1 * (d - 9.6) / 0.7
    } else if d <= 11.3 {
        0.1
    } else {
        0.1 * (11.6 - d) / 0.3
    };
    let ramp = (-0.25 + (d - 3.6) * 0.4 / 6.0).min(0.15);
    ramp + bump
}

#[test]
fn the_detent_floor_matches_the_analytic_profile() {
    let mesh = mesh_of(DETENT_TRACK);
    assert_eq!(
        non_manifold_edges(&mesh),
        0,
        "the detent track is not closed"
    );

    // Five angles: the lead-in ramp, just before the detent, the detent
    // plateau, the drop, and the pocket at the stop.
    for d in [4.5, 9.6, 10.8, 11.45, 17.0] {
        let got = floor_at(&mesh, TRACK_R, d);
        let want = analytic_floor(d);
        assert!(
            (got - want).abs() <= 0.02,
            "floor at d{d}° is {got:.4}, drawing says {want:.4} \
             (off by {:.4}, tolerance 0.02)",
            got - want
        );
    }
}

#[test]
fn a_non_monotonic_track_is_refused_rather_than_self_intersecting() {
    let err = eval_vcad(
        r#"[root [pipe [profile-rect 2.0 2.0]
             [sweep [cam-path 34.5 #[0.0 0.0  10.0 1.0  5.0 2.0]]]] "aluminum"]"#,
        None,
    )
    .unwrap_err();
    assert!(
        format!("{err}").contains("monotonic"),
        "expected a monotonicity complaint, got: {err}"
    );
}
