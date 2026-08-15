//! Miter corners for adjacent-edge fillets — the follow-up the per-edge
//! fillet fix named: two selected plane-plane edges sharing a vertex.
//!
//! Closed forms (cube of side `a`, radius `r`): each filleted edge removes
//! a prism of cross-section `r² − πr²/4` along its length; where two
//! filleted edges meet at a corner the removed prisms overlap in a region
//! of volume `(5/3 − π/2)·r³` (∫₀ʳ (r − √(2rz − z²))² dz), which must be
//! added back exactly once per miter corner:
//!
//! ```text
//! V = a³ − c·Σlength + (5/3 − π/2)·r³·(#corners),   c = r²(1 − π/4)
//! ```
//!
//! The discrete mesh uses 128-segment arcs, so the tessellated volume
//! tracks these closed forms to ~1e-5 relative (the same band the
//! independent-edge gates use). Watertightness is gated structurally:
//! every undirected mesh edge must be shared by exactly two triangles.

use std::collections::HashMap;
use vcad_kernel_fillet::fillet_edges_detailed;
use vcad_kernel_math::Point3;
use vcad_kernel_primitives::{make_cube, BRepSolid};
use vcad_kernel_topo::EdgeId;

const A: f64 = 10.0;
const R: f64 = 1.5;

/// Cross-section removed per unit length of a 90° edge fillet.
fn c_edge() -> f64 {
    R * R * (1.0 - std::f64::consts::PI / 4.0)
}

/// Overlap of two removed prisms at a 90° miter corner.
fn c_corner() -> f64 {
    (5.0 / 3.0 - std::f64::consts::PI / 2.0) * R * R * R
}

/// Find the cube edge whose endpoints match `p` and `q` (any order).
fn edge_between(brep: &BRepSolid, p: Point3, q: Point3) -> EdgeId {
    let topo = &brep.topology;
    for (edge_id, edge) in &topo.edges {
        let he1 = edge.half_edge;
        let Some(he2) = topo.half_edges[he1].twin else {
            continue;
        };
        let a = topo.vertices[topo.half_edges[he1].origin].point;
        let b = topo.vertices[topo.half_edges[he2].origin].point;
        let close = |x: Point3, y: Point3| (x - y).norm() < 1e-9;
        if (close(a, p) && close(b, q)) || (close(a, q) && close(b, p)) {
            return edge_id;
        }
    }
    panic!("no edge between {p:?} and {q:?}");
}

fn mesh_volume(mesh: &vcad_kernel_tessellate::TriangleMesh) -> f64 {
    let verts = &mesh.vertices;
    let mut vol = 0.0;
    for tri in mesh.indices.chunks(3) {
        let (i0, i1, i2) = (
            tri[0] as usize * 3,
            tri[1] as usize * 3,
            tri[2] as usize * 3,
        );
        let v0 = [verts[i0] as f64, verts[i0 + 1] as f64, verts[i0 + 2] as f64];
        let v1 = [verts[i1] as f64, verts[i1 + 1] as f64, verts[i1 + 2] as f64];
        let v2 = [verts[i2] as f64, verts[i2 + 1] as f64, verts[i2 + 2] as f64];
        vol += v0[0] * (v1[1] * v2[2] - v2[1] * v1[2]) - v1[0] * (v0[1] * v2[2] - v2[1] * v0[2])
            + v2[0] * (v0[1] * v1[2] - v1[1] * v0[2]);
    }
    (vol / 6.0).abs()
}

/// Structural watertightness: every undirected edge of the triangle mesh
/// is shared by exactly two triangles (welded positions, so quantize).
fn assert_watertight(mesh: &vcad_kernel_tessellate::TriangleMesh, label: &str) {
    let key = |i: u32| -> [i64; 3] {
        let b = i as usize * 3;
        [
            (mesh.vertices[b] as f64 * 1e6).round() as i64,
            (mesh.vertices[b + 1] as f64 * 1e6).round() as i64,
            (mesh.vertices[b + 2] as f64 * 1e6).round() as i64,
        ]
    };
    let mut counts: HashMap<([i64; 3], [i64; 3]), i32> = HashMap::new();
    for tri in mesh.indices.chunks(3) {
        for e in 0..3 {
            let (p, q) = (key(tri[e]), key(tri[(e + 1) % 3]));
            if p == q {
                continue; // degenerate sliver along a welded seam
            }
            let k = if p < q { (p, q) } else { (q, p) };
            *counts.entry(k).or_insert(0) += 1;
        }
    }
    let boundary = counts.values().filter(|&&c| c % 2 != 0).count();
    assert_eq!(
        boundary, 0,
        "{label}: {boundary} boundary (odd-count) mesh edges — not watertight"
    );
}

fn run_case(edges: &[EdgeId], cube: &BRepSolid, total_len: f64, corners: usize, label: &str) {
    let (result, results) = fillet_edges_detailed(cube, edges, R);
    assert_eq!(results.len(), edges.len(), "{label}: one result per edge");
    for r in &results {
        assert!(r.is_success(), "{label}: {r:?}");
    }
    let mesh = vcad_kernel_tessellate::tessellate_brep(&result, 64);
    assert_watertight(&mesh, label);
    let vol = mesh_volume(&mesh);
    let expected = A * A * A - c_edge() * total_len + c_corner() * corners as f64;
    let rel = (vol - expected).abs() / expected;
    eprintln!("{label}: volume {vol:.6} vs closed form {expected:.6} (rel {rel:.3e})");
    assert!(
        rel <= 1e-4,
        "{label}: volume {vol} vs closed form {expected} (rel {rel:.3e})"
    );
}

#[test]
fn two_adjacent_edges_meet_in_a_miter() {
    // L-chain at the origin corner: edges along +X and +Y share (0,0,0).
    let cube = make_cube(A, A, A);
    let e_x = edge_between(&cube, Point3::new(0.0, 0.0, 0.0), Point3::new(A, 0.0, 0.0));
    let e_y = edge_between(&cube, Point3::new(0.0, 0.0, 0.0), Point3::new(0.0, A, 0.0));
    run_case(&[e_x, e_y], &cube, 2.0 * A, 1, "L-chain");
}

#[test]
fn three_edge_open_chain() {
    // U-chain around the bottom face: +X edge, then +Y, then the far X edge.
    let cube = make_cube(A, A, A);
    let e1 = edge_between(&cube, Point3::new(0.0, 0.0, 0.0), Point3::new(A, 0.0, 0.0));
    let e2 = edge_between(&cube, Point3::new(A, 0.0, 0.0), Point3::new(A, A, 0.0));
    let e3 = edge_between(&cube, Point3::new(A, A, 0.0), Point3::new(0.0, A, 0.0));
    run_case(&[e1, e2, e3], &cube, 3.0 * A, 2, "U-chain");
}

#[test]
fn closed_top_rim_ring() {
    // All four top edges: a closed chain, every vertex a miter, no caps.
    let cube = make_cube(A, A, A);
    let corners = [
        Point3::new(0.0, 0.0, A),
        Point3::new(A, 0.0, A),
        Point3::new(A, A, A),
        Point3::new(0.0, A, A),
    ];
    let edges: Vec<EdgeId> = (0..4)
        .map(|i| edge_between(&cube, corners[i], corners[(i + 1) % 4]))
        .collect();
    run_case(&edges, &cube, 4.0 * A, 4, "top rim");
}

#[test]
fn independent_pair_still_works() {
    // Regression: two opposite (non-adjacent) edges — the original
    // independent-set path, now a chain set with zero miter corners.
    let cube = make_cube(A, A, A);
    let e1 = edge_between(&cube, Point3::new(0.0, 0.0, 0.0), Point3::new(A, 0.0, 0.0));
    let e2 = edge_between(&cube, Point3::new(0.0, A, A), Point3::new(A, A, A));
    run_case(&[e1, e2], &cube, 2.0 * A, 0, "opposite pair");
}
