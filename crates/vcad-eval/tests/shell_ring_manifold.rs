//! End-to-end regression for the shell-ring reproducer (found in the `rana`
//! project): a bored can with a multi-stage tool pipe — internal groove, a
//! circular pattern of axial slots, and a rotated notch — subtracted in one
//! `[difference TOOL SUBJECT]`.
//!
//! The historic failure had two faces:
//!   * later union stages of the tool pipe were silently dropped from the
//!     subtraction (only the first stage — the groove — cut), and
//!   * the result kept interior cap faces from the subject's construction, so
//!     the export had 1000+ edges not shared by exactly two triangles and
//!     sliced as "fused" even where cavity walls existed.
//!
//! The assertions are therefore (a) edge-manifoldness of the exported mesh and
//! (b) material absence at a probe point inside every cutter stage, plus
//! presence at control points in the wall.

use std::collections::HashMap;

use vcad_eval::{evaluate_document, EvalOptions};
use vcad_kernel_booleans::point_in_mesh;
use vcad_kernel_math::Point3;
use vcad_loon::eval_vcad;

const SHELL_RING: &str = r#"[root [difference
  [pipe [translate 0.0 0.0 -1.0 [cylinder-n 47.0 45.0 96]]
        [union [translate 0.0 0.0 31.0 [cylinder-n 49.5 5.0 96]]]
        [union [circular-pattern 0.0 0.0 0.0 0.0 0.0 1.0 3.0 360.0
                 [translate 44.0 -2.6 33.0 [cube 12.0 5.2 20.0]]]]
        [union [rotate 0.0 0.0 60.0 [translate 46.0 -2.25 37.0 [cube 10.0 4.5 12.0]]]]]
  [pipe [cylinder-n 53.0 4.0 96]
        [union [cylinder-n 50.5 41.0 96]]]] "aluminum"]"#;

fn rot_z(p: (f64, f64, f64), deg: f64) -> Point3 {
    let r = deg.to_radians();
    let (s, c) = r.sin_cos();
    Point3::new(c * p.0 - s * p.1, s * p.0 + c * p.1, p.2)
}

/// All three authored forms must produce the identical solid; each was a
/// verified-failing variant of the same bug report.
const SHELL_RING_FLAT_UNION: &str = r#"[root [difference
  [union [translate 0.0 0.0 -1.0 [cylinder-n 47.0 45.0 96]]
    [union [translate 0.0 0.0 31.0 [cylinder-n 49.5 5.0 96]]
      [union [circular-pattern 0.0 0.0 0.0 0.0 0.0 1.0 3.0 360.0
               [translate 44.0 -2.6 33.0 [cube 12.0 5.2 20.0]]]
        [rotate 0.0 0.0 60.0 [translate 46.0 -2.25 37.0 [cube 10.0 4.5 12.0]]]]]]
  [pipe [cylinder-n 53.0 4.0 96]
        [union [cylinder-n 50.5 41.0 96]]]] "aluminum"]"#;

const SHELL_RING_CHAINED: &str = r#"[root [difference
  [rotate 0.0 0.0 60.0 [translate 46.0 -2.25 37.0 [cube 10.0 4.5 12.0]]]
  [difference
    [circular-pattern 0.0 0.0 0.0 0.0 0.0 1.0 3.0 360.0
      [translate 44.0 -2.6 33.0 [cube 12.0 5.2 20.0]]]
    [difference
      [translate 0.0 0.0 31.0 [cylinder-n 49.5 5.0 96]]
      [difference
        [translate 0.0 0.0 -1.0 [cylinder-n 47.0 45.0 96]]
        [pipe [cylinder-n 53.0 4.0 96]
              [union [cylinder-n 50.5 41.0 96]]]]]]] "aluminum"]"#;

#[test]
fn shell_ring_all_stages_cut_and_mesh_is_edge_manifold() {
    check_shell_ring(SHELL_RING);
}

/// Reported variant 1: all cuts folded into one flat tool union.
#[test]
fn shell_ring_flat_union_tool() {
    check_shell_ring(SHELL_RING_FLAT_UNION);
}

/// Reported variant 2: chained single-tool differences — historically left
/// a D-shaped lune of leftover cap faces filling half the bore.
#[test]
fn shell_ring_chained_differences() {
    check_shell_ring(SHELL_RING_CHAINED);
}

fn check_shell_ring(src: &str) {
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

    // (a) Edge-manifoldness: every undirected edge shared by exactly two
    // triangles. Vertex positions are quantized so triangles that meet at the
    // same point through different indices still pair.
    let key = |i: u32| {
        let v = &mesh.vertices[3 * i as usize..3 * i as usize + 3];
        (
            (v[0] as f64 * 1e4).round() as i64,
            (v[1] as f64 * 1e4).round() as i64,
            (v[2] as f64 * 1e4).round() as i64,
        )
    };
    let mut edges: HashMap<_, usize> = HashMap::new();
    for t in mesh.indices.chunks_exact(3) {
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            let (ka, kb) = (key(a), key(b));
            let e = if ka <= kb { (ka, kb) } else { (kb, ka) };
            *edges.entry(e).or_default() += 1;
        }
    }
    let bad = edges.values().filter(|&&c| c != 2).count();
    assert_eq!(
        bad,
        0,
        "mesh is not edge-manifold: {bad} of {} undirected edges are not \
         shared by exactly two triangles",
        edges.len()
    );

    // (b) Material absence inside each cutter stage, presence in the wall.
    // Body: outer r50.5 (h41) ∪ r53 (h4), bore r47 → wall spans r47..50.5.
    let absent = [
        // bore
        ("bore axis", Point3::new(0.0, 0.0, 20.0)),
        // groove band r47..49.5, z31..36 (probe away from any slot)
        ("groove", rot_z((48.7, 0.0, 33.5), 200.0)),
        // slot pattern: cube x44..56, y±2.6, z33..53 at 0/120/240°
        ("slot @0°", Point3::new(48.75, 0.0, 39.0)),
        ("slot @120°", rot_z((48.75, 0.0, 39.0), 120.0)),
        ("slot @240°", rot_z((48.75, 0.0, 39.0), 240.0)),
        // notch: cube x46..56, y±2.25, z37..49 rotated 60°
        ("notch @60°", rot_z((48.0, 0.0, 40.0), 60.0)),
    ];
    for (name, p) in absent {
        assert!(
            !point_in_mesh(&p, &mesh),
            "material present inside the {name} cutter at {p:?} — that stage \
             of the tool pipe was dropped from the subtraction"
        );
    }
    let present = [
        ("wall @90°", rot_z((48.75, 0.0, 20.0), 90.0)),
        ("wall @30°", rot_z((49.5, 0.0, 10.0), 30.0)),
        ("outer flange", rot_z((52.0, 0.0, 2.0), 10.0)),
    ];
    for (name, p) in present {
        assert!(
            point_in_mesh(&p, &mesh),
            "material missing at control point {name} {p:?} — the subtraction \
             removed wall it should have kept"
        );
    }
}
