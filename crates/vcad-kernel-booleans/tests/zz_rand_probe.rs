//! Temporary probes for torture-track regressions (rand-042/060/062/074, chain-03).
use vcad_kernel_booleans::{boolean_op, BooleanOp, BooleanResult};
use vcad_kernel_primitives::{make_cone, make_cylinder, BRepSolid};
use vcad_kernel_tessellate::tessellate_brep;

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

fn census(tag: &str, s: &BRepSolid) {
    for (fid, face) in s.topology.faces.iter() {
        let kind = s.geometry.surfaces[face.surface_index].surface_type();
        let hes: Vec<_> = s.topology.loop_half_edges(face.outer_loop).collect();
        let unpaired = hes
            .iter()
            .filter(|he| s.topology.half_edges[**he].twin.is_none())
            .count();
        // face centroid z for identification
        let mut cz = 0.0;
        for he in &hes {
            cz += s.topology.vertices[s.topology.half_edges[*he].origin]
                .point
                .z;
        }
        cz /= hes.len() as f64;
        if unpaired > 0 {
            println!(
                "{tag} {fid:?} {kind:?} nv={} inner={} unpaired={} cz={:.3}",
                hes.len(),
                face.inner_loops.len(),
                unpaired,
                cz
            );
        }
    }
}

#[test]
fn zz_rand_074() {
    // cone rb=5.284 rt=3.402 h=12.685, minus coaxial cylinder r=6.345 h=11.308
    let a = make_cone(5.284080202707185, 3.401886202216431, 12.685440060611493, 32);
    let b = make_cylinder(6.3445452567286695, 11.307838937227348, 32);
    let BooleanResult::BRep(r) = boolean_op(&a, &b, BooleanOp::Difference, 64).expect("boolean");
    census("074", &r);
    let m = tessellate_brep(&r, 64);
    println!("074 open_edges={}", open_edges(&m));
    let mut vol = 0.0;
    for t in 0..m.indices.len() / 3 {
        let p = |k: usize| {
            let i = m.indices[t * 3 + k] as usize;
            [
                m.vertices[i * 3] as f64,
                m.vertices[i * 3 + 1] as f64,
                m.vertices[i * 3 + 2] as f64,
            ]
        };
        let (a, b, c) = (p(0), p(1), p(2));
        vol += (a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
            + a[2] * (b[0] * c[1] - b[1] * c[0]))
            / 6.0;
    }
    println!("074 volume={vol:.3}");
}

#[test]
fn zz_rand_042() {
    // cone rb=8.693 rt=4.008 h=19.941 INTERSECT coaxial cylinder r=9.984 h=8.992
    let a = make_cone(8.692616967334407, 4.008220196660772, 19.940694870786217, 32);
    let b = make_cylinder(9.984136331872858, 8.991821915543659, 32);
    let BooleanResult::BRep(r) = boolean_op(&a, &b, BooleanOp::Intersection, 64).expect("boolean");
    census("042", &r);
    let m = tessellate_brep(&r, 64);
    println!("042 open_edges={}", open_edges(&m));
}

#[test]
fn zz_chain03_step0() {
    // cube 30³ minus vertical cylinder r=4.702 h=40, rot z=11°, translate (23,12,8)
    use vcad_kernel_math::Transform;
    let a = vcad_kernel_primitives::make_cube(30.0, 30.0, 30.0);
    let mut b = make_cylinder(4.7022185713160845, 40.0, 32);
    let t =
        Transform::rotation_z(11.0_f64.to_radians()).then(&Transform::translation(23.0, 12.0, 8.0));
    for (_id, v) in &mut b.topology.vertices {
        v.point = t.apply_point(&v.point);
    }
    for s in &mut b.geometry.surfaces {
        *s = s.transform(&t);
    }
    let BooleanResult::BRep(r) = boolean_op(&a, &b, BooleanOp::Difference, 32).expect("boolean");
    census("c03", &r);
    let m = tessellate_brep(&r, 32);
    println!("c03 open_edges={}", open_edges(&m));
}

#[test]
fn zz_rand_062() {
    // sphere r=3.096 INTERSECT prism(7, r=1.641, h=3.142), both at origin
    let a = vcad_kernel_primitives::make_sphere(3.0960184720233705, 32);
    let b = vcad_kernel_primitives::make_prism(7, 1.64063341728016, 3.1418857150735495);
    let BooleanResult::BRep(r) = boolean_op(&a, &b, BooleanOp::Intersection, 32).expect("boolean");
    census("062", &r);
    let m = tessellate_brep(&r, 32);
    let mut vol = 0.0;
    for t in 0..m.indices.len() / 3 {
        let p = |k: usize| {
            let i = m.indices[t * 3 + k] as usize;
            [
                m.vertices[i * 3] as f64,
                m.vertices[i * 3 + 1] as f64,
                m.vertices[i * 3 + 2] as f64,
            ]
        };
        let (a, b, c) = (p(0), p(1), p(2));
        vol += (a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
            + a[2] * (b[0] * c[1] - b[1] * c[0]))
            / 6.0;
    }
    println!("062 open_edges={} volume={vol:.3}", open_edges(&m));
}
