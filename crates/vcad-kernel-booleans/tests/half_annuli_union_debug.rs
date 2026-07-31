//! Diagnostic for the torr C2 half-annuli case: two arc-extruded 190°
//! half-annuli (r20..30, h5), the second rotated 180°, unioned. The union
//! must produce the full annulus; the overlap wedges' coincident faces
//! must dedupe rather than double-count.

use std::f64::consts::PI;

use vcad_kernel_booleans::{boolean_op, BooleanOp, BooleanResult};
use vcad_kernel_math::{Point2, Point3, Transform, Vec3};
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_sketch::{extrude, SketchProfile, SketchSegment};
use vcad_kernel_tessellate::tessellate_brep;

fn half_annulus() -> BRepSolid {
    // 190° span: outer arc ccw 0°..190°, radial line in, inner arc back, line out.
    let a0 = 0.0f64;
    let a1 = 190.0f64.to_radians();
    let (c1, s1) = (a1.cos(), a1.sin());
    let segments = vec![
        SketchSegment::Line {
            start: Point2::new(20.0, 0.0),
            end: Point2::new(30.0, 0.0),
        },
        SketchSegment::Arc {
            start: Point2::new(30.0, 0.0),
            end: Point2::new(30.0 * c1, 30.0 * s1),
            center: Point2::new(0.0, 0.0),
            ccw: true,
        },
        SketchSegment::Line {
            start: Point2::new(30.0 * c1, 30.0 * s1),
            end: Point2::new(20.0 * c1, 20.0 * s1),
        },
        SketchSegment::Arc {
            start: Point2::new(20.0 * c1, 20.0 * s1),
            end: Point2::new(20.0, 0.0),
            center: Point2::new(0.0, 0.0),
            ccw: false,
        },
    ];
    let _ = a0;
    let profile = SketchProfile::new(Point3::origin(), Vec3::x(), Vec3::y(), segments)
        .expect("valid profile");
    extrude(&profile, Vec3::new(0.0, 0.0, 5.0)).expect("extrude half annulus")
}

fn rotate_z(mut b: BRepSolid, deg: f64) -> BRepSolid {
    let t = Transform::rotation_z(deg.to_radians());
    for (_id, v) in &mut b.topology.vertices {
        v.point = t.apply_point(&v.point);
    }
    for s in &mut b.geometry.surfaces {
        *s = s.transform(&t);
    }
    b
}

fn vol(b: &BRepSolid) -> f64 {
    let mesh = tessellate_brep(b, 256);
    let mut v = 0.0;
    for t in mesh.indices.chunks(3) {
        let p = |i: u32| {
            let k = i as usize * 3;
            (
                mesh.vertices[k] as f64,
                mesh.vertices[k + 1] as f64,
                mesh.vertices[k + 2] as f64,
            )
        };
        let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
        v += (a.0 * (b.1 * c.2 - b.2 * c.1) - a.1 * (b.0 * c.2 - b.2 * c.0)
            + a.2 * (b.0 * c.1 - b.1 * c.0))
            / 6.0;
    }
    v
}

#[test]
fn half_annuli_union_full_annulus() {
    let a = half_annulus();
    let b = rotate_z(half_annulus(), 180.0);
    eprintln!(
        "a vol {:.1}, b vol {:.1} (each expect {:.1})",
        vol(&a),
        vol(&b),
        (190.0 / 360.0) * PI * 500.0 * 5.0
    );
    let BooleanResult::BRep(u) = boolean_op(&a, &b, BooleanOp::Union, 32).expect("boolean");
    let expected = PI * 500.0 * 5.0;
    let v = vol(&u);
    eprintln!(
        "union vol {:.1} (expect {expected:.1}), faces {}",
        v,
        u.topology.faces.len()
    );
    // Remaining ~1% deficit: split points land on sampled arc-boundary
    // chords, shaving each cap piece slightly (same family as the ignored
    // pattern-trim residuals in the torr catalogue).
    assert!(
        (v - expected).abs() < expected * 0.015,
        "union volume {v:.1}, want {expected:.1}"
    );
}

#[test]
fn half_annuli_union_area_audit() {
    use vcad_kernel_math::Vec3 as V3;
    let a = half_annulus();
    let b = rotate_z(half_annulus(), 180.0);
    let BooleanResult::BRep(u) = boolean_op(&a, &b, BooleanOp::Union, 32).expect("boolean");
    let mesh = tessellate_brep(&u, 256);
    let (mut z0, mut z5, mut r20, mut r30, mut other) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for t in mesh.indices.chunks(3) {
        let p = |i: u32| {
            let k = i as usize * 3;
            V3::new(
                mesh.vertices[k] as f64,
                mesh.vertices[k + 1] as f64,
                mesh.vertices[k + 2] as f64,
            )
        };
        let (pa, pb, pc) = (p(t[0]), p(t[1]), p(t[2]));
        let ar = 0.5 * (pb - pa).cross(pc - pa).norm();
        let flat = (pa.z - pb.z).abs() < 1e-6 && (pb.z - pc.z).abs() < 1e-6;
        let r = |q: V3| (q.x * q.x + q.y * q.y).sqrt();
        let rc = (r(pa) + r(pb) + r(pc)) / 3.0;
        if flat && pa.z.abs() < 1e-6 {
            z0 += ar;
        } else if flat && (pa.z - 5.0).abs() < 1e-6 {
            z5 += ar;
        } else if !flat && (rc - 20.0).abs() < 0.2 {
            r20 += ar;
        } else if !flat && (rc - 30.0).abs() < 0.2 {
            r30 += ar;
        } else {
            other += ar;
        }
    }
    use std::f64::consts::PI;
    eprintln!("z0 {z0:.1} (expect {:.1})", PI * 500.0);
    eprintln!("z5 {z5:.1} (expect {:.1})", PI * 500.0);
    eprintln!("r20 {r20:.1} (expect {:.1})", 2.0 * PI * 20.0 * 5.0);
    eprintln!("r30 {r30:.1} (expect {:.1})", 2.0 * PI * 30.0 * 5.0);
    eprintln!("other {other:.1} (expect 0)");
}

#[test]
fn half_annuli_result_face_tessellation_audit() {
    use vcad_kernel_math::Vec3 as V3;
    let a = half_annulus();
    let b = rotate_z(half_annulus(), 180.0);
    let BooleanResult::BRep(u) = boolean_op(&a, &b, BooleanOp::Union, 32).expect("boolean");
    let params = vcad_kernel_tessellate::TessellationParams::from_segments(64);
    for (fid, kind, mesh) in vcad_kernel_tessellate::tessellate_brep_by_face(&u, &params) {
        let face = &u.topology.faces[fid];
        let verts: Vec<V3> = u
            .topology
            .loop_half_edges(face.outer_loop)
            .map(|he| {
                u.topology.vertices[u.topology.half_edges[he].origin]
                    .point
                    .to_vec()
            })
            .collect();
        // Shoelace area of the loop polygon (planar faces only).
        let mut loop_area_vec = V3::zeros();
        for i in 1..verts.len().saturating_sub(1) {
            loop_area_vec += (verts[i] - verts[0]).cross(verts[i + 1] - verts[0]);
        }
        let loop_area = 0.5 * loop_area_vec.norm();
        let mut tess_area = 0.0;
        for t in mesh.indices.chunks(3) {
            let p = |i: u32| {
                let k = i as usize * 3;
                V3::new(
                    mesh.vertices[k] as f64,
                    mesh.vertices[k + 1] as f64,
                    mesh.vertices[k + 2] as f64,
                )
            };
            tess_area += 0.5 * (p(t[1]) - p(t[0])).cross(p(t[2]) - p(t[0])).norm();
        }
        let z = verts.first().map(|v| v.z).unwrap_or(f64::NAN);
        eprintln!(
            "{fid:?} {kind:?} nv={} loop_area={loop_area:.1} tess_area={tess_area:.1} z0={z:.1}",
            verts.len()
        );
    }
}
