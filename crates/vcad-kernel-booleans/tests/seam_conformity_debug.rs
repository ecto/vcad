//! Debug probe: dump face-loop conformity of a cube∪cube result.
//! A conforming topology has every loop edge (vertex-pair) traversed once
//! in each direction across the whole shell.

use std::collections::HashMap;

use vcad_kernel_booleans::{boolean_op, BooleanOp, BooleanResult};
use vcad_kernel_math::Transform;
use vcad_kernel_primitives::{make_cube, BRepSolid};

fn loop_conformity(brep: &BRepSolid, label: &str) {
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
    println!("{label}: faces={nfaces} loop-edges={} nonconforming={}", net.len(), open.len());
    for ((a, b), n) in open.iter().take(20) {
        let p = |k: &[i64; 3]| [k[0] as f64 * q, k[1] as f64 * q, k[2] as f64 * q];
        println!("  net={n:+} edge {:?} -> {:?}", p(a), p(b));
    }
}

#[test]
fn frozen_cylinder_renders() {
    use vcad_kernel_primitives::make_cylinder;
    use vcad_kernel_tessellate::tessellate_brep;
    let mut cyl = make_cylinder(22.5, 12.57, 32);
    vcad_kernel_booleans::freeze_circle_loops_for_test(&mut cyl, 32);
    loop_conformity(&cyl, "frozen cylinder topo");
    for segs in [32u32, 256] {
        let mesh = tessellate_brep(&cyl, segs);
        let mut net = std::collections::HashMap::new();
        for t in 0..mesh.indices.len() / 3 {
            for k in 0..3 {
                let a = mesh.indices[t * 3 + k];
                let b = mesh.indices[t * 3 + (k + 1) % 3];
                let key = (a.min(b), a.max(b));
                *net.entry(key).or_insert(0i64) += if a < b { 1 } else { -1 };
            }
        }
        let open = net.values().filter(|n| **n != 0).count();
        let mut vol = 0.0f64;
        for t in 0..mesh.indices.len() / 3 {
            let p = |i: usize| {
                let b = mesh.indices[t * 3 + i] as usize * 3;
                [mesh.vertices[b] as f64, mesh.vertices[b + 1] as f64, mesh.vertices[b + 2] as f64]
            };
            let (a, b, c) = (p(0), p(1), p(2));
            vol += (a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
                + a[2] * (b[0] * c[1] - b[1] * c[0])) / 6.0;
        }
        println!("frozen cyl segs={segs}: tris={} open={} vol={:.2} (analytic 19993.3)",
            mesh.indices.len() / 3, open, vol);
    }
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
    match boolean_op(&a, &b, BooleanOp::Union, 32).expect("boolean") {
        BooleanResult::BRep(brep) => loop_conformity(&brep, "cube ∪ cube"),
        _ => panic!("expected BRep result"),
    }
}
