use std::time::Instant;
use vcad_kernel_booleans::{boolean_op, BooleanOp};
use vcad_kernel_math::Transform;
use vcad_kernel_primitives::{make_cube, make_cylinder, BRepSolid};

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

fn main() {
    let mut solid = make_cube(100.0, 100.0, 10.0);
    let t0 = Instant::now();
    for i in 0..24 {
        let mut hole = make_cylinder(2.0, 40.0, 24);
        let x = 10.0 + (i % 6) as f64 * 15.0;
        let y = 10.0 + (i / 6) as f64 * 20.0;
        translate(&mut hole, x, y, -10.0);
        solid = boolean_op(&solid, &hole, BooleanOp::Difference, 32)
            .unwrap()
            .into_brep()
            .expect("brep");
        println!("{:2} cuts: {:?}", i + 1, t0.elapsed());
    }
}
