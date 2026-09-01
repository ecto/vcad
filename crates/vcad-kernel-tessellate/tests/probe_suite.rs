//! Integration coverage for the solid query API (issue #843).
//!
//! The probe fixtures are eight assertions ported verbatim from the rana 60c
//! mule's suite (`rana/tools/probe-60c.py`, 142 probes) against the same
//! exported STLs: an airgap void through the assembled stack, a pocket
//! depth material/void pair, the castellation fingers of two rotors posed
//! face-to-face, and the sun/spider drive fit. The expected outcomes are the
//! ones the Python suite asserts — this is the "same answers, now a vcad
//! feature" check.

use std::path::PathBuf;

use vcad_kernel_tessellate::probe::{parse_binary_stl, run_probe_file};
use vcad_kernel_tessellate::{clearance, TriangleMesh};

fn fixture(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(rel)
}

#[test]
fn rana_60c_probe_excerpt_matches_the_python_suite() {
    let report = run_probe_file(&fixture("rana-60c/probes.json")).expect("probe file runs");
    assert_eq!(report.outcomes.len(), 9);
    assert!(report.ok(), "\n{}", report.render());
}

#[test]
fn a_wrong_expectation_fails_the_suite() {
    // Guard against the suite passing vacuously: flip one claim and the
    // runner must report exactly one failure. (rana's first shell check
    // summed winding with an inverted sign, saw no material anywhere, and
    // passed every void probe — this is the test that would have caught it.)
    let text = std::fs::read_to_string(fixture("rana-60c/probes.json")).unwrap();
    let mut suite: vcad_kernel_tessellate::ProbeSuite = serde_json::from_str(&text).unwrap();
    let p = suite
        .probes
        .iter_mut()
        .find(|p| p.name == "rear axial A void")
        .unwrap();
    p.want_material = true;
    let base = fixture("rana-60c");
    let asm = vcad_kernel_tessellate::probe::build_assembly(&suite, |rel| {
        let bytes = std::fs::read(base.join(rel)).unwrap();
        Ok(parse_binary_stl(&bytes).unwrap())
    })
    .unwrap();
    let report = vcad_kernel_tessellate::run_suite(&suite, &asm).unwrap();
    assert_eq!(report.failed(), 1, "\n{}", report.render());
}

/// Z-axis cylinder as a closed triangle mesh (outward winding).
fn cylinder(center_xy: [f64; 2], radius: f64, z0: f64, z1: f64, segments: usize) -> TriangleMesh {
    let mut vertices: Vec<f32> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let push = |p: [f64; 3], v: &mut Vec<f32>| {
        v.extend_from_slice(&[p[0] as f32, p[1] as f32, p[2] as f32]);
        (v.len() / 3 - 1) as u32
    };
    let bottom_center = push([center_xy[0], center_xy[1], z0], &mut vertices);
    let top_center = push([center_xy[0], center_xy[1], z1], &mut vertices);
    let mut ring = Vec::new();
    for i in 0..segments {
        let a = std::f64::consts::TAU * i as f64 / segments as f64;
        let (x, y) = (
            center_xy[0] + radius * a.cos(),
            center_xy[1] + radius * a.sin(),
        );
        let lo = push([x, y, z0], &mut vertices);
        let hi = push([x, y, z1], &mut vertices);
        ring.push((lo, hi));
    }
    for i in 0..segments {
        let (lo0, hi0) = ring[i];
        let (lo1, hi1) = ring[(i + 1) % segments];
        indices.extend_from_slice(&[lo0, lo1, hi0, lo1, hi1, hi0]);
        indices.extend_from_slice(&[bottom_center, lo1, lo0]);
        indices.extend_from_slice(&[top_center, hi0, hi1]);
    }
    TriangleMesh {
        normals: vec![0.0; vertices.len()],
        vertices,
        indices,
        face_kinds: Vec::new(),
    }
}

#[test]
fn clearance_between_two_cylinders_matches_the_known_gap() {
    // Radius 5 each, centers 13 apart -> 3.0 mm surface-to-surface. A fine
    // tessellation (720 segments) keeps the chord sag well under the 1e-3
    // tolerance the assertion is written to.
    let a = cylinder([0.0, 0.0], 5.0, 0.0, 10.0, 720);
    let b = cylinder([13.0, 0.0], 5.0, 0.0, 10.0, 720);
    let c = clearance(&a, &b).expect("both meshes have triangles");
    assert!(!c.intersecting, "{c:?}");
    assert!((c.distance - 3.0).abs() < 1e-3, "{c:?}");
}

#[test]
fn overlapping_cylinders_report_negative_clearance() {
    let a = cylinder([0.0, 0.0], 5.0, 0.0, 10.0, 128);
    let b = cylinder([8.0, 0.0], 5.0, 0.0, 10.0, 128);
    let c = clearance(&a, &b).unwrap();
    assert!(c.intersecting && c.distance <= 0.0, "{c:?}");
}
