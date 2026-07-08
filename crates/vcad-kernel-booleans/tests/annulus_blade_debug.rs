//! Debug: annulus (boolean result) ∪ blade — the F2 regression driver.

use vcad_kernel_booleans::{boolean_op, BooleanOp, BooleanResult};
use vcad_kernel_math::Transform;
use vcad_kernel_primitives::{make_cube, make_cylinder, BRepSolid};
use vcad_kernel_tessellate::tessellate_brep;

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

fn mesh_volume(mesh: &vcad_kernel_tessellate::TriangleMesh) -> f64 {
    let mut vol = 0.0f64;
    for t in 0..mesh.indices.len() / 3 {
        let p = |i: usize| {
            let b = mesh.indices[t * 3 + i] as usize * 3;
            [
                mesh.vertices[b] as f64,
                mesh.vertices[b + 1] as f64,
                mesh.vertices[b + 2] as f64,
            ]
        };
        let (a, b, c) = (p(0), p(1), p(2));
        vol += (a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
            + a[2] * (b[0] * c[1] - b[1] * c[0]))
            / 6.0;
    }
    vol
}

#[test]
fn annulus_union_blade() {
    let big = make_cylinder(22.5, 12.57, 32);
    let small = make_cylinder(8.0, 12.57, 32);
    let annulus = match boolean_op(&big, &small, BooleanOp::Difference, 32).unwrap() {
        BooleanResult::BRep(b) => *b,
        _ => panic!(),
    };
    let mut blade = make_cube(23.5, 0.5, 12.57);
    translate(&mut blade, 21.5, 0.0, 0.0);
    let v_ann = mesh_volume(&tessellate_brep(&annulus, 32));
    let result = match boolean_op(&annulus, &blade, BooleanOp::Union, 32).unwrap() {
        BooleanResult::BRep(b) => *b,
        _ => panic!(),
    };
    let v = mesh_volume(&tessellate_brep(&result, 32));
    println!("annulus={v_ann:.2} annulus∪blade={v:.2} (expected ≈ {:.2})", v_ann + 147.7 - 6.27);
}
