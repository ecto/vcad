//! Temporary probe for D4 counterbore stage.
use vcad_kernel_booleans::{boolean_op, BooleanOp, BooleanResult};
use vcad_kernel_math::Transform;
use vcad_kernel_primitives::{make_cylinder, BRepSolid};
use vcad_kernel_tessellate::tessellate_brep;

fn translate_z(mut b: BRepSolid, dz: f64) -> BRepSolid {
    let t = Transform::translation(0.0, 0.0, dz);
    for (_id, v) in &mut b.topology.vertices {
        v.point = t.apply_point(&v.point);
    }
    for s in &mut b.geometry.surfaces {
        *s = s.transform(&t);
    }
    b
}

fn open_edges(mesh: &vcad_kernel_tessellate::TriangleMesh) -> usize {
    let quantum = 1e-5;
    let vkey = |vi: usize| -> [i64; 3] {
        let mut k = [0i64; 3];
        for c in 0..3 {
            k[c] = (mesh.vertices[vi * 3 + c] as f64 / quantum).round() as i64;
        }
        k
    };
    let mut net: std::collections::HashMap<([i64; 3], [i64; 3]), i64> =
        std::collections::HashMap::new();
    for t in 0..mesh.indices.len() / 3 {
        for k in 0..3 {
            let a = vkey(mesh.indices[t * 3 + k] as usize);
            let b = vkey(mesh.indices[t * 3 + (k + 1) % 3] as usize);
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
    net.values().map(|n| n.unsigned_abs() as usize).sum()
}

fn diff(a: &BRepSolid, b: &BRepSolid) -> BRepSolid {
    let BooleanResult::BRep(r) = boolean_op(a, b, BooleanOp::Difference, 64).expect("boolean");
    *r
}

#[test]
fn zz_d4_probe() {
    let hub = make_cylinder(30.0, 40.0, 32);
    let cavity = translate_z(make_cylinder(18.5, 30.0, 32), 10.0);
    let bore = make_cylinder(10.0, 10.0, 32);
    let s1 = diff(&hub, &cavity);
    let s2 = diff(&s1, &bore);
    for (fid, face) in s2.topology.faces.iter() {
        let kind = s2.geometry.surfaces[face.surface_index].surface_type();
        let hes: Vec<_> = s2.topology.loop_half_edges(face.outer_loop).collect();
        println!(
            "s2 {fid:?} {kind:?} loop_len={} inner={}",
            hes.len(),
            face.inner_loops.len()
        );
        if hes.len() <= 6 {
            for he in hes {
                let h = &s2.topology.half_edges[he];
                let p = s2.topology.vertices[h.origin].point;
                println!(
                    "   he origin=({:.3},{:.3},{:.3}) twin={:?}",
                    p.x,
                    p.y,
                    p.z,
                    h.twin.is_some()
                );
            }
        }
    }
    for seg in [32u32, 64, 256] {
        let m1 = tessellate_brep(&s1, seg);
        let m2 = tessellate_brep(&s2, seg);
        println!(
            "seg={seg}: s1 open={} s2 open={}",
            open_edges(&m1),
            open_edges(&m2)
        );
    }
}
