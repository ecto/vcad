//! Regression tests for the clash-detection AABB broadphase: the sweep must
//! still surface every genuinely overlapping pair, skip disjoint pairs
//! without invoking the boolean kernel, and stay fast on many-root scenes
//! (blind O(n²) pairwise booleans made a ~90k-root imported chip die
//! unrenderable).

use vcad_eval::{evaluate_document, EvalOptions};
use vcad_ir::{CsgOp, Document, Node, NodeId, SceneEntry, Vec3};

/// Document with one cube root per `(origin, size)` entry.
fn doc_with_cubes(cubes: &[([f64; 3], f64)]) -> Document {
    let mut doc = Document::new();
    let mut id: NodeId = 1;
    for &(origin, size) in cubes {
        let cube = id;
        doc.nodes.insert(
            cube,
            Node {
                id: cube,
                name: None,
                op: CsgOp::Cube {
                    size: Vec3::new(size, size, size),
                },
            },
        );
        let moved = id + 1;
        id += 2;
        doc.nodes.insert(
            moved,
            Node {
                id: moved,
                name: None,
                op: CsgOp::Translate {
                    child: cube,
                    offset: Vec3::new(origin[0], origin[1], origin[2]),
                },
            },
        );
        doc.roots.push(SceneEntry {
            root: moved,
            material: "default".to_string(),
            visible: None,
        });
    }
    doc
}

fn clash_count(doc: &Document) -> usize {
    let scene = evaluate_document(doc, &EvalOptions::default()).expect("evaluate");
    scene.clashes.len()
}

#[test]
fn overlapping_pair_still_detected() {
    // Two 10-cubes offset by 5 on x: genuine overlap.
    let doc = doc_with_cubes(&[([0.0, 0.0, 0.0], 10.0), ([5.0, 0.0, 0.0], 10.0)]);
    assert_eq!(clash_count(&doc), 1);
}

#[test]
fn overlap_found_regardless_of_root_order() {
    // Same pair, reversed order — the sweep sorts by min-x, so ordering of
    // roots must not matter.
    let doc = doc_with_cubes(&[([5.0, 0.0, 0.0], 10.0), ([0.0, 0.0, 0.0], 10.0)]);
    assert_eq!(clash_count(&doc), 1);
}

#[test]
fn overlap_on_y_and_z_only_still_detected() {
    // Identical x-extent (sweep axis), overlapping only via y/z checks.
    let doc = doc_with_cubes(&[([0.0, 0.0, 0.0], 10.0), ([0.0, 4.0, 4.0], 10.0)]);
    assert_eq!(clash_count(&doc), 1);
}

#[test]
fn disjoint_roots_produce_no_clashes() {
    let doc = doc_with_cubes(&[
        ([0.0, 0.0, 0.0], 10.0),
        ([100.0, 0.0, 0.0], 10.0),
        ([0.0, 100.0, 0.0], 10.0),
        ([0.0, 0.0, 100.0], 10.0),
    ]);
    assert_eq!(clash_count(&doc), 0);
}

#[test]
fn three_way_overlap_reports_each_pair() {
    // Three cubes chained: A∩B and B∩C overlap, A∩C do not.
    let doc = doc_with_cubes(&[
        ([0.0, 0.0, 0.0], 10.0),
        ([7.0, 0.0, 0.0], 10.0),
        ([14.0, 0.0, 0.0], 10.0),
    ]);
    assert_eq!(clash_count(&doc), 2);
}

#[test]
fn many_disjoint_roots_evaluate_quickly() {
    // 2,000 disjoint cubes = ~2M candidate pairs. With the broadphase this
    // is pure AABB arithmetic; without it, ~2M BRep booleans (minutes).
    // Keeping the wall-clock bound loose but real: fails hard on a
    // regression to blind pairwise booleans.
    let cubes: Vec<([f64; 3], f64)> = (0..2000)
        .map(|i| {
            let x = (i % 50) as f64 * 20.0;
            let y = ((i / 50) % 50) as f64 * 20.0;
            let z = (i / 2500) as f64 * 20.0;
            ([x, y, z], 10.0)
        })
        .collect();
    let doc = doc_with_cubes(&cubes);
    let t0 = std::time::Instant::now();
    assert_eq!(clash_count(&doc), 0);
    assert!(
        t0.elapsed() < std::time::Duration::from_secs(30),
        "clash broadphase too slow: {:?}",
        t0.elapsed()
    );
}
