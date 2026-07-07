//! Diagnostic for torr D4: three chained coaxial differences where the
//! counterbore's inner removal boundary coincides with the existing cavity
//! wall radius. Volume error appears on the FOURTH boolean.

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

fn diff(a: &BRepSolid, b: &BRepSolid) -> BRepSolid {
    let BooleanResult::BRep(r) = boolean_op(a, b, BooleanOp::Difference, 64);
    *r
}

#[test]
fn d4_chained_counterbore() {
    use std::f64::consts::PI;
    let hub = make_cylinder(30.0, 40.0, 32);
    let cavity = translate_z(make_cylinder(18.5, 30.0, 32), 10.0);
    let bore = make_cylinder(10.0, 10.0, 32);
    let cbore = translate_z(make_cylinder(24.0, 10.0, 32), 30.0);

    let s1 = diff(&hub, &cavity);
    eprintln!(
        "after cavity: vol {:.2} (expect {:.2})",
        vol(&s1),
        PI * (900.0 * 40.0 - 342.25 * 30.0)
    );
    let s2 = diff(&s1, &bore);
    eprintln!(
        "after bore:   vol {:.2} (expect {:.2})",
        vol(&s2),
        PI * (900.0 * 40.0 - 342.25 * 30.0 - 100.0 * 10.0)
    );
    let s3 = diff(&s2, &cbore);
    let expected = PI * (900.0 * 40.0 - 342.25 * 30.0 - 100.0 * 10.0 - (576.0 - 342.25) * 10.0);
    eprintln!("after cbore:  vol {:.2} (expect {:.2})", vol(&s3), expected);
    eprintln!("result faces: {}", s3.topology.faces.len());
    for (fid, face) in s3.topology.faces.iter() {
        let verts: Vec<_> = s3
            .topology
            .loop_half_edges(face.outer_loop)
            .map(|he| s3.topology.vertices[s3.topology.half_edges[he].origin].point)
            .collect();
        let (mut lo, mut hi) = ([f64::MAX; 3], [f64::MIN; 3]);
        for p in &verts {
            for (k, val) in [p.x, p.y, p.z].into_iter().enumerate() {
                lo[k] = lo[k].min(val);
                hi[k] = hi[k].max(val);
            }
        }
        let kind = s3.geometry.surfaces[face.surface_index].surface_type();
        eprintln!(
            "  {fid:?} {kind:?} nv={} inner={} r~[{:.1},{:.1}] z[{:.1},{:.1}] {:?}",
            verts.len(),
            face.inner_loops.len(),
            lo[0]
                .abs()
                .max(lo[1].abs())
                .min(hi[0].abs().max(hi[1].abs())),
            hi[0].abs().max(hi[1].abs()),
            lo[2],
            hi[2],
            face.orientation
        );
    }
    let v = vol(&s3);
    assert!(
        (v - expected).abs() < expected * 0.005,
        "d4 volume {v:.2}, want {expected:.2}"
    );
}

#[test]
fn d4_per_face_areas() {
    let hub = make_cylinder(30.0, 40.0, 32);
    let cavity = translate_z(make_cylinder(18.5, 30.0, 32), 10.0);
    let bore = make_cylinder(10.0, 10.0, 32);
    let cbore = translate_z(make_cylinder(24.0, 10.0, 32), 30.0);
    let s3 = diff(&diff(&diff(&hub, &cavity), &bore), &cbore);

    let params = vcad_kernel_tessellate::TessellationParams::from_segments(256);
    for (fid, kind, mesh) in vcad_kernel_tessellate::tessellate_brep_by_face(&s3, &params) {
        let mut area = 0.0;
        let mut zmin = f64::MAX;
        let mut zmax = f64::MIN;
        for t in mesh.indices.chunks(3) {
            let p = |i: u32| {
                let k = i as usize * 3;
                vcad_kernel_math::Vec3::new(
                    mesh.vertices[k] as f64,
                    mesh.vertices[k + 1] as f64,
                    mesh.vertices[k + 2] as f64,
                )
            };
            let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
            area += 0.5 * (b - a).cross(c - a).norm();
            for v in [a, b, c] {
                zmin = zmin.min(v.z);
                zmax = zmax.max(v.z);
            }
        }
        eprintln!("  {fid:?} {kind:?} area={area:.1} z[{zmin:.1},{zmax:.1}]");
    }
}

#[test]
fn d4_cap_areas_by_plane() {
    use std::f64::consts::PI;
    let hub = make_cylinder(30.0, 40.0, 32);
    let cavity = translate_z(make_cylinder(18.5, 30.0, 32), 10.0);
    let bore = make_cylinder(10.0, 10.0, 32);
    let cbore = translate_z(make_cylinder(24.0, 10.0, 32), 30.0);
    let s3 = diff(&diff(&diff(&hub, &cavity), &bore), &cbore);

    let mesh = tessellate_brep(&s3, 256);
    let mut by_z: std::collections::BTreeMap<i64, f64> = Default::default();
    for t in mesh.indices.chunks(3) {
        let p = |i: u32| {
            let k = i as usize * 3;
            vcad_kernel_math::Vec3::new(
                mesh.vertices[k] as f64,
                mesh.vertices[k + 1] as f64,
                mesh.vertices[k + 2] as f64,
            )
        };
        let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
        if (a.z - b.z).abs() < 1e-6 && (b.z - c.z).abs() < 1e-6 {
            let area = 0.5 * (b - a).cross(c - a).norm();
            *by_z.entry((a.z * 10.0).round() as i64).or_insert(0.0) += area;
        }
    }
    eprintln!("cap area by z (z*10):");
    for (z, a) in &by_z {
        eprintln!("  z={:.1}: {a:.1}", *z as f64 / 10.0);
    }
    eprintln!(
        "expected: z0 {:.1}, z10 {:.1}, z30 {:.1}, z40 {:.1}",
        PI * (900.0 - 100.0),
        PI * (342.25 - 100.0),
        PI * (576.0 - 342.25),
        PI * (900.0 - 576.0)
    );
}

#[test]
fn d2_locality_union_shared_wall() {
    use std::f64::consts::PI;
    // hub r20 h30; drum = annulus r20..28 z in [20,30]; bore r4 h30.
    let hub = make_cylinder(20.0, 30.0, 32);
    let drum_outer = translate_z(make_cylinder(28.0, 10.0, 32), 20.0);
    let drum_inner = translate_z(make_cylinder(20.0, 10.0, 32), 20.0);
    let drum = diff(&drum_outer, &drum_inner);
    let bore = make_cylinder(4.0, 30.0, 32);

    eprintln!(
        "drum vol {:.1} (expect {:.1})",
        vol(&drum),
        PI * (784.0 - 400.0) * 10.0
    );

    let hub_minus_bore = diff(&hub, &bore);
    eprintln!(
        "hub-bore vol {:.1} (expect {:.1})",
        vol(&hub_minus_bore),
        PI * (400.0 - 16.0) * 30.0
    );

    let BooleanResult::BRep(u) = boolean_op(&hub_minus_bore, &drum, BooleanOp::Union, 64);
    let expected = PI * (400.0 * 30.0 - 16.0 * 30.0 + (784.0 - 400.0) * 10.0);
    eprintln!("union vol {:.1} (expect {expected:.1})", vol(&u));
    for (fid, face) in u.topology.faces.iter() {
        let verts: Vec<_> = u
            .topology
            .loop_half_edges(face.outer_loop)
            .map(|he| u.topology.vertices[u.topology.half_edges[he].origin].point)
            .collect();
        let (mut lo, mut hi) = ([f64::MAX; 3], [f64::MIN; 3]);
        for p in &verts {
            for (k, val) in [p.x, p.y, p.z].into_iter().enumerate() {
                lo[k] = lo[k].min(val);
                hi[k] = hi[k].max(val);
            }
        }
        let kind = u.geometry.surfaces[face.surface_index].surface_type();
        eprintln!(
            "  {fid:?} {kind:?} nv={} r<= {:.1} z[{:.1},{:.1}] {:?}",
            verts.len(),
            hi[0].abs().max(hi[1].abs()),
            lo[2],
            hi[2],
            face.orientation
        );
    }
}
