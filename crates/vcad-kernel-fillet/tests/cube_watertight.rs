//! Standing invariant: `fillet_all_edges` on a box produces a closed manifold
//! solid whose volume matches the analytic rounded box.
//!
//! These assertions passed before the ray-tracer corner-trim fix and still
//! pass after it — the corner hole lived in the ray tracer's trim test, not in
//! the B-rep — so they are an invariant, not that fix's regression test (see
//! `vcad-kernel-raytrace/tests/fillet_corner_trim.rs`). They are worth pinning
//! anyway: this pipeline rebuilds the whole shell from scratch, and a cracked
//! shell is exactly the failure mode that a trim-level test cannot see.

use vcad_kernel_fillet::fillet_all_edges;
use vcad_kernel_primitives::make_cube;
use vcad_kernel_tessellate::{tessellate_solid, TessellationParams};

const DIMS: (f64, f64, f64) = (40.0, 30.0, 6.0);
const RADIUS: f64 = 2.0;

/// Volume of a box of `dims` with all edges rounded at radius `r`: the inner
/// box, plus a slab over each of the 6 faces, plus a quarter-cylinder along
/// each of the 12 edges, plus 8 sphere octants (one whole sphere).
fn analytic_rounded_box_volume(dims: (f64, f64, f64), r: f64) -> f64 {
    let (a, b, c) = (dims.0 - 2.0 * r, dims.1 - 2.0 * r, dims.2 - 2.0 * r);
    let core = a * b * c;
    let slabs = 2.0 * r * (a * b + b * c + a * c);
    let edges = std::f64::consts::PI * r * r * (a + b + c);
    let corners = 4.0 / 3.0 * std::f64::consts::PI * r * r * r;
    core + slabs + edges + corners
}

#[test]
fn filleted_cube_brep_is_a_closed_manifold() {
    let filleted = fillet_all_edges(&make_cube(DIMS.0, DIMS.1, DIMS.2), RADIUS);

    // The fillet must actually have happened — a refused fillet returns the
    // input unchanged, and a 6-face box would trivially pass the checks below.
    assert_eq!(
        filleted.topology.faces.len(),
        26,
        "6 trimmed faces + 12 edge cylinders + 8 corner blends"
    );

    let boundary: Vec<_> = filleted
        .topology
        .half_edges
        .iter()
        .filter(|(_, he)| he.twin.is_none())
        .map(|(id, _)| id)
        .collect();
    assert!(
        boundary.is_empty(),
        "{} half-edges have no twin — the rebuilt shell is cracked: {boundary:?}",
        boundary.len()
    );
}

#[test]
fn filleted_cube_mesh_is_watertight() {
    let filleted = fillet_all_edges(&make_cube(DIMS.0, DIMS.1, DIMS.2), RADIUS);
    let mesh = tessellate_solid(&filleted, &TessellationParams::default());

    // Weld by position, then require every undirected edge to carry one
    // triangle in each direction. A hole shows up as a non-zero imbalance.
    let quantized = |i: usize| -> [i64; 3] {
        [0, 1, 2].map(|k| ((mesh.vertices[i * 3 + k] as f64) * 1e4).round() as i64)
    };
    let mut balance: std::collections::HashMap<([i64; 3], [i64; 3]), i32> = Default::default();
    for tri in mesh.indices.chunks(3) {
        let p = [
            quantized(tri[0] as usize),
            quantized(tri[1] as usize),
            quantized(tri[2] as usize),
        ];
        for i in 0..3 {
            let (a, b) = (p[i], p[(i + 1) % 3]);
            let (key, dir) = if a < b { ((a, b), 1) } else { ((b, a), -1) };
            *balance.entry(key).or_insert(0) += dir;
        }
    }
    let unpaired = balance.values().filter(|v| **v != 0).count();
    assert_eq!(
        unpaired, 0,
        "{unpaired} mesh edges are not shared by two oppositely-wound triangles"
    );
}

#[test]
fn filleted_cube_volume_matches_the_analytic_rounded_box() {
    let filleted = fillet_all_edges(&make_cube(DIMS.0, DIMS.1, DIMS.2), RADIUS);
    let mesh = tessellate_solid(&filleted, &TessellationParams::default());
    let props =
        vcad_kernel_tessellate::mesh_props::compute_mesh_properties(&mesh.vertices, &mesh.indices);

    let expected = analytic_rounded_box_volume(DIMS, RADIUS);
    // The mesh is a chordal approximation, so it under-reports slightly;
    // 0.5% covers the default tessellation density.
    let rel_err = (props.volume - expected).abs() / expected;
    assert!(
        rel_err < 5e-3,
        "filleted volume {} differs from the analytic rounded box {expected} \
         by {:.3}%",
        props.volume,
        rel_err * 100.0
    );
}
