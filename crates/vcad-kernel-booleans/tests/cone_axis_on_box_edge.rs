//! Regression: a cone whose axis lies exactly on a box corner edge — the
//! box planes x=0 and y=0 both pass through the cone axis, so plane∩cone
//! degenerates to two straight rulings instead of a conic. The lateral wall
//! band used to be dropped entirely (torture cases rand-002/030/086/130:
//! open rim circles + 34-41% volume deficit).
//!
//! Assertions are on volume against the analytic value, not just
//! watertightness — see curved_face_classification.rs for why.

use std::f64::consts::PI;
use vcad_kernel_booleans::{boolean_op, BooleanOp};
use vcad_kernel_primitives::{make_cone, make_cube};

const SEGMENTS: u32 = 32;

fn frustum_volume(rb: f64, rt: f64, h: f64) -> f64 {
    PI * h / 3.0 * (rb * rb + rb * rt + rt * rt)
}

/// Volume of the frustum's +x,+y quarter clipped to z <= zmax (the box is
/// wide enough in x/y to contain the quarter for every case below).
fn quarter_frustum_below(rb: f64, rt: f64, h: f64, zmax: f64) -> f64 {
    if zmax >= h {
        return frustum_volume(rb, rt, h) / 4.0;
    }
    let rm = rb + (rt - rb) * (zmax / h);
    frustum_volume(rb, rm, zmax) / 4.0
}

fn mesh_volume(mesh: &vcad_kernel_tessellate::TriangleMesh) -> f64 {
    let verts = &mesh.vertices;
    let mut vol = 0.0_f64;
    for tri in mesh.indices.chunks(3) {
        let i0 = tri[0] as usize * 3;
        let i1 = tri[1] as usize * 3;
        let i2 = tri[2] as usize * 3;
        let v0 = [verts[i0] as f64, verts[i0 + 1] as f64, verts[i0 + 2] as f64];
        let v1 = [verts[i1] as f64, verts[i1 + 1] as f64, verts[i1 + 2] as f64];
        let v2 = [verts[i2] as f64, verts[i2 + 1] as f64, verts[i2 + 2] as f64];
        vol += v0[0] * (v1[1] * v2[2] - v2[1] * v1[2]) - v1[0] * (v0[1] * v2[2] - v2[1] * v0[2])
            + v2[0] * (v0[1] * v1[2] - v1[1] * v0[2]);
    }
    vol / 6.0
}

fn check(case: &str, sx: f64, sy: f64, sz: f64, rb: f64, rt: f64, h: f64, union_op: bool) {
    let cube = make_cube(sx, sy, sz);
    let cone = make_cone(rb, rt, h, SEGMENTS);
    let v_cube = sx * sy * sz;
    let v_overlap = quarter_frustum_below(rb, rt, h, sz.min(h));
    let (op, expected) = if union_op {
        (
            BooleanOp::Union,
            v_cube + frustum_volume(rb, rt, h) - v_overlap,
        )
    } else {
        (BooleanOp::Difference, v_cube - v_overlap)
    };
    let result = boolean_op(&cube, &cone, op, SEGMENTS).expect("boolean should succeed");
    let mesh = result.to_mesh(SEGMENTS);
    let open = mesh.boundary_edges().len();
    assert_eq!(open, 0, "{case}: {open} open boundary edges");
    let v = mesh_volume(&mesh);
    let rel = (v - expected).abs() / expected;
    assert!(
        rel < 0.02,
        "{case}: volume {v:.4} vs analytic {expected:.4} (rel err {:.2}%)",
        rel * 100.0
    );
}

#[test]
fn rand_002_cube_minus_cone_axis_on_corner() {
    check(
        "rand-002",
        10.90671106413945,
        17.75451124540674,
        4.615343499803716,
        9.910569812730584,
        1.1726645375261058,
        2.4735394613289343,
        false,
    );
}

#[test]
fn rand_030_cube_minus_cone_axis_on_corner() {
    check(
        "rand-030",
        19.359394782885566,
        9.650830329754319,
        16.076476317416457,
        6.875037331366226,
        0.45694795134827615,
        14.784346481906827,
        false,
    );
}

#[test]
fn rand_086_cube_union_cone_axis_on_corner() {
    check(
        "rand-086",
        10.1041605926844,
        13.684462663150608,
        17.498866474834283,
        6.475596216765858,
        2.9404818972270004,
        6.444204498945038,
        true,
    );
}

#[test]
fn rand_130_cube_union_cone_axis_on_corner() {
    check(
        "rand-130",
        13.049185556942604,
        14.95758620279308,
        4.791990137455045,
        8.62772952972415,
        3.802442143234791,
        13.515303284875346,
        true,
    );
}

/// Clean round numbers, same degeneracy.
#[test]
fn simple_cube_minus_cone_axis_on_corner() {
    check("simple", 10.0, 10.0, 5.0, 8.0, 2.0, 4.0, false);
}
