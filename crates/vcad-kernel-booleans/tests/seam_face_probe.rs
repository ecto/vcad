//! Per-face seam probe for the blade∪cylinder open-edge investigation.

use std::collections::HashMap;

use vcad_kernel_booleans::{boolean_op, BooleanOp, BooleanResult};
use vcad_kernel_math::Transform;
use vcad_kernel_primitives::{make_cube, make_cylinder, BRepSolid};
use vcad_kernel_tessellate::{tessellate_brep_by_face, TessellationParams};

fn translate(brep: &mut BRepSolid, dx: f64, dy: f64, dz: f64) {
    let t = Transform::translation(dx, dy, dz);
    for (_, v) in &mut brep.topology.vertices {
        v.point = t.apply_point(&v.point);
    }
    brep.geometry.surfaces = brep
        .geometry
        .surfaces
        .drain(..)
        .map(|s| s.transform(&t))
        .collect();
}

#[test]
fn blade_union_cylinder_face_seams() {
    let cyl = make_cylinder(22.5, 12.57, 32);
    let mut blade = make_cube(23.5, 0.5, 12.57);
    translate(&mut blade, 21.5, 0.0, 0.0);
    let brep = match boolean_op(&cyl, &blade, BooleanOp::Union, 32).expect("boolean") {
        BooleanResult::BRep(b) => *b,
        _ => panic!("expected BRep"),
    };
    census(brep);
}

#[test]
fn blade_union_annulus_face_seams() {
    let big = make_cylinder(22.5, 12.57, 32);
    let small = make_cylinder(8.0, 12.57, 32);
    let annulus = match boolean_op(&big, &small, BooleanOp::Difference, 32).unwrap() {
        BooleanResult::BRep(b) => *b,
        _ => panic!(),
    };
    let mut blade = make_cube(23.5, 0.5, 12.57);
    translate(&mut blade, 21.5, 0.0, 0.0);
    let brep = match boolean_op(&annulus, &blade, BooleanOp::Union, 32).expect("boolean") {
        BooleanResult::BRep(b) => *b,
        _ => panic!("expected BRep"),
    };
    census(brep);
}

#[test]
fn staircase_hub_face_seams() {
    let big = make_cylinder(22.5, 12.57, 32);
    let small = make_cylinder(8.0, 12.57, 32);
    let annulus = match boolean_op(&big, &small, BooleanOp::Difference, 32).unwrap() {
        BooleanResult::BRep(b) => *b,
        _ => panic!(),
    };
    let mut cbore = make_cylinder(14.0, 4.0, 32);
    translate(&mut cbore, 0.0, 0.0, 8.57);
    let brep = match boolean_op(&annulus, &cbore, BooleanOp::Difference, 32).unwrap() {
        BooleanResult::BRep(b) => *b,
        _ => panic!(),
    };
    census(brep);
}

fn census(brep: BRepSolid) {

    // Topology conformity first
    {
        let topo = &brep.topology;
        let q = 1e-6;
        let key = |p: vcad_kernel_math::Point3| -> [i64; 3] {
            [
                (p.x / q).round() as i64,
                (p.y / q).round() as i64,
                (p.z / q).round() as i64,
            ]
        };
        let mut net: HashMap<([i64; 3], [i64; 3]), i64> = HashMap::new();
        let solid = &topo.solids[brep.solid_id];
        for &fid in &topo.shells[solid.outer_shell].faces {
            let face = &topo.faces[fid];
            let mut loops = vec![face.outer_loop];
            loops.extend(face.inner_loops.iter().copied());
            for lp in loops {
                let pts: Vec<_> = topo
                    .loop_half_edges(lp)
                    .map(|he| topo.vertices[topo.half_edges[he].origin].point)
                    .collect();
                for i in 0..pts.len() {
                    let a = key(pts[i]);
                    let b = key(pts[(i + 1) % pts.len()]);
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
        let open = net.values().filter(|n| **n != 0).count();
        println!("topology nonconforming loop-edges: {open}");
    }

    // Mesh-level: which faces contribute boundary (unmatched) edges?
    let params = TessellationParams::from_segments(32);
    let per_face = tessellate_brep_by_face(&brep, &params);
    let q = 1e-4;
    let mut net: HashMap<([i64; 3], [i64; 3]), i64> = HashMap::new();
    let mut edge_owner: HashMap<([i64; 3], [i64; 3]), Vec<usize>> = HashMap::new();
    for (fi, (_fid, _kind, mesh)) in per_face.iter().enumerate() {
        let vkey = |vi: usize| -> [i64; 3] {
            [
                (mesh.vertices[vi * 3] as f64 / q).round() as i64,
                (mesh.vertices[vi * 3 + 1] as f64 / q).round() as i64,
                (mesh.vertices[vi * 3 + 2] as f64 / q).round() as i64,
            ]
        };
        for t in 0..mesh.indices.len() / 3 {
            for k in 0..3 {
                let a = vkey(mesh.indices[t * 3 + k] as usize);
                let b = vkey(mesh.indices[t * 3 + (k + 1) % 3] as usize);
                if a == b {
                    continue;
                }
                let (key, dir) = if a < b { ((a, b), 1) } else { ((b, a), -1) };
                *net.entry(key).or_default() += dir;
                edge_owner.entry(key).or_default().push(fi);
            }
        }
    }
    let mut per_face_open: HashMap<usize, usize> = HashMap::new();
    for (key, n) in &net {
        if *n != 0 {
            for fi in &edge_owner[key] {
                *per_face_open.entry(*fi).or_default() += 1;
            }
        }
    }
    let mut rows: Vec<_> = per_face_open.iter().collect();
    rows.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
    for (fi, n) in rows.iter().take(12) {
        let (fid, kind, mesh) = &per_face[**fi];
        let face = &brep.topology.faces[*fid];
        println!(
            "face {fi} {kind:?} open={n} tris={} outer_loop_len={} inner_loops={}",
            mesh.indices.len() / 3,
            brep.topology.loop_len(face.outer_loop),
            face.inner_loops.len()
        );
    }
    let total: usize = net.values().filter(|n| **n != 0).count();
    println!("total open mesh edges: {total}");
    let mut shown = 0;
    for (key, n) in &net {
        if *n != 0 && shown < 40 {
            let p = |k: &[i64; 3]| [k[0] as f64 * q, k[1] as f64 * q, k[2] as f64 * q];
            println!(
                "  open net={n:+} {:?} -> {:?} owners={:?}",
                p(&key.0),
                p(&key.1),
                edge_owner[key]
            );
            shown += 1;
        }
    }
}
