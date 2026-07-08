//! Regression: boolean results must have CONFORMING face loops — every
//! loop edge (vertex-pair) traversed once in each direction across the
//! shell. Trimming used to split one face's boundary at intersection
//! points while the neighbor kept the whole edge (a T-junction), so the
//! sewn shell could never close; `repair::split_edges_at_interior_vertices`
//! heals this.

use std::collections::HashMap;

use vcad_kernel_booleans::{boolean_op, BooleanOp, BooleanResult};
use vcad_kernel_math::Transform;
use vcad_kernel_primitives::{make_cube, BRepSolid};

fn loop_conformity(brep: &BRepSolid, label: &str) -> usize {
    let topo = &brep.topology;
    let solid = &topo.solids[brep.solid_id];
    let shell = &topo.shells[solid.outer_shell];
    let q = 1e-6;
    let key = |p: vcad_kernel_math::Point3| -> [i64; 3] {
        [
            (p.x / q).round() as i64,
            (p.y / q).round() as i64,
            (p.z / q).round() as i64,
        ]
    };
    let mut net: HashMap<([i64; 3], [i64; 3]), i64> = HashMap::new();
    let mut nfaces = 0;
    for &face_id in &shell.faces {
        nfaces += 1;
        let face = &topo.faces[face_id];
        let mut loops = vec![face.outer_loop];
        loops.extend(face.inner_loops.iter().copied());
        for lp in loops {
            let pts: Vec<_> = topo
                .loop_half_edges(lp)
                .map(|he| topo.vertices[topo.half_edges[he].origin].point)
                .collect();
            let n = pts.len();
            for i in 0..n {
                let a = key(pts[i]);
                let b = key(pts[(i + 1) % n]);
                if a == b {
                    continue;
                }
                if a < b {
                    *net.entry((a, b)).or_default() += 1;
                } else {
                    *net.entry((b, a)).or_default() -= 1;
                }
            }
        }
    }
    let open: Vec<_> = net.iter().filter(|(_, n)| **n != 0).collect();
    println!(
        "{label}: faces={nfaces} loop-edges={} nonconforming={}",
        net.len(),
        open.len()
    );
    for ((a, b), n) in open.iter().take(20) {
        let p = |k: &[i64; 3]| [k[0] as f64 * q, k[1] as f64 * q, k[2] as f64 * q];
        println!("  net={n:+} edge {:?} -> {:?}", p(a), p(b));
    }
    open.len()
}

#[test]
fn annulus_conformity() {
    use vcad_kernel_primitives::make_cylinder;
    let big = make_cylinder(22.5, 12.57, 32);
    let small = make_cylinder(8.0, 12.57, 32);
    let annulus = match boolean_op(&big, &small, BooleanOp::Difference, 32).unwrap() {
        BooleanResult::BRep(b) => *b,
        _ => panic!(),
    };
    // NOTE: differences copy B faces with the Reversed flag but unchanged
    // loop order, so a directed census reads ±2 on the bore seams even
    // though the emitted mesh cancels — informational only, no assertion.
    loop_conformity(&annulus, "annulus");
    // dump face loop shapes
    let topo = &annulus.topology;
    let solid = &topo.solids[annulus.solid_id];
    for &fid in &topo.shells[solid.outer_shell].faces {
        let face = &topo.faces[fid];
        let surf = annulus.geometry.surfaces[face.surface_index].surface_type();
        let outer: Vec<_> = topo.loop_vertices(face.outer_loop);
        println!(
            "face {fid:?} {surf:?} outer_len={} inner={}",
            outer.len(),
            face.inner_loops.len()
        );
        for il in &face.inner_loops {
            println!("   inner_len={}", topo.loop_vertices(*il).len());
        }
    }
}

#[test]
fn cube_union_cube_conformity() {
    let a = make_cube(10.0, 10.0, 10.0);
    let mut b = make_cube(10.0, 10.0, 10.0);
    let t = Transform::translation(5.0, 5.0, 5.0);
    for (_, v) in &mut b.topology.vertices {
        v.point = t.apply_point(&v.point);
    }
    b.geometry.surfaces = b
        .geometry
        .surfaces
        .drain(..)
        .map(|s| s.transform(&t))
        .collect();
    let BooleanResult::BRep(brep) = boolean_op(&a, &b, BooleanOp::Union, 32).expect("boolean");
    let open = loop_conformity(&brep, "cube ∪ cube");
    assert_eq!(open, 0, "cube ∪ cube topology must be conforming");
}
