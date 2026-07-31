//! Temporary probe: blade ∪ cylinder duplication hunt.
use vcad_kernel_booleans::{boolean_op, BooleanOp, BooleanResult};
use vcad_kernel_math::Transform;
use vcad_kernel_primitives::{make_cube, make_cylinder, BRepSolid};

fn transform_brep(brep: &mut BRepSolid, t: &Transform) {
    for (_id, v) in &mut brep.topology.vertices {
        v.point = t.apply_point(&v.point);
    }
    for s in &mut brep.geometry.surfaces {
        *s = s.transform(t);
    }
}

#[test]
fn zz_blade_union_no_duplicate_faces() {
    let cyl = make_cylinder(22.5, 13.0, 32);
    let mut blade = make_cube(23.5, 0.5, 12.57);
    let combined = Transform::translation(21.5, 0.0, 0.0)
        .then(&Transform::rotation_x(39.29_f64.to_radians()));
    transform_brep(&mut blade, &combined);
    let BooleanResult::BRep(result) =
        boolean_op(&blade, &cyl, BooleanOp::Union, 64).expect("boolean");
    // Hash each face's sorted vertex set; identical hashes = duplicates.
    let mut seen: std::collections::HashMap<Vec<[i64; 3]>, Vec<String>> =
        std::collections::HashMap::new();
    for (_fid, face) in result.topology.faces.iter() {
        let mut key: Vec<[i64; 3]> = result
            .topology
            .loop_half_edges(face.outer_loop)
            .map(|he| {
                let p = result.topology.vertices[result.topology.half_edges[he].origin].point;
                [
                    (p.x * 1e6) as i64,
                    (p.y * 1e6) as i64,
                    (p.z * 1e6) as i64,
                ]
            })
            .collect();
        key.sort();
        let kind = format!(
            "{_fid:?} {:?} nv={} first={:?}",
            result.geometry.surfaces[face.surface_index].surface_type(),
            key.len(),
            key.first().map(|k| [k[0] as f64 / 1e6, k[1] as f64 / 1e6, k[2] as f64 / 1e6])
        );
        seen.entry(key).or_default().push(kind);
    }
    let dups: Vec<&Vec<String>> = seen.values().filter(|v| v.len() > 1).collect();
    for d in &dups {
        eprintln!("DUP GROUP:");
        for m in d.iter() {
            eprintln!("   {m}");
        }
    }
    assert!(dups.is_empty(), "duplicate face groups: {}", dups.len());
}

#[test]
fn zz_frozen_caps_detected_as_disks() {
    let mut cyl = make_cylinder(22.5, 13.0, 32);
    // Same entry freeze as the pipeline (via a boolean no-op is awkward;
    // use a trivially overlapping union to trigger freeze, then inspect).
    vcad_kernel_booleans::freeze_circle_loops_for_test(&mut cyl, 64);
    let mut disks = 0;
    for (fid, face) in cyl.topology.faces.iter() {
        if cyl.geometry.surfaces[face.surface_index].surface_type()
            == vcad_kernel_geom::SurfaceKind::Plane
            && vcad_kernel_booleans::split::is_circular_disk_face(&cyl, fid)
        {
            disks += 1;
        }
    }
    assert_eq!(disks, 2, "analytic caps must read as disks");
    let _ = &mut cyl;
}

#[test]
fn zz_two_flat_blades_sequential() {
    let cyl = make_cylinder(22.5, 12.57, 32);
    let mk = |ang: f64| -> BRepSolid {
        let mut b = make_cube(23.5, 0.5, 12.57);
        let t = Transform::rotation_z(ang.to_radians())
            .then(&Transform::translation(21.5, 0.0, 0.0));
        transform_brep(&mut b, &t);
        b
    };
    let BooleanResult::BRep(u1) =
        boolean_op(&mk(0.0), &cyl, BooleanOp::Union, 64).expect("boolean 1");
    let BooleanResult::BRep(u2) =
        boolean_op(&mk(180.0), &u1, BooleanOp::Union, 64).expect("boolean 2");
    let mesh = vcad_kernel_tessellate::tessellate_brep(&u2, 64);
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
    let open = net.values().filter(|&&n| n != 0).count();
    assert_eq!(open, 0, "{open} open edges after two sequential flat unions");
}

#[test]
fn zz_two_rotated_blades_sequential() {
    let cyl = make_cylinder(22.5, 13.0, 32);
    let mk = |ang: f64| -> BRepSolid {
        let mut b = make_cube(23.5, 0.5, 12.57);
        let t = Transform::rotation_z(ang.to_radians())
            .then(&Transform::translation(21.5, 0.0, 0.0))
            .then(&Transform::rotation_x(39.29_f64.to_radians()));
        transform_brep(&mut b, &t);
        b
    };
    let BooleanResult::BRep(u1) =
        boolean_op(&mk(0.0), &cyl, BooleanOp::Union, 64).expect("boolean 1");
    for (_vid, v) in &u1.topology.vertices {
        assert!(
            v.point.z > -1.0 && v.point.z < 14.0,
            "phantom vertex in u1 at {:?}",
            v.point
        );
    }
    let BooleanResult::BRep(u2) =
        boolean_op(&mk(90.0), &u1, BooleanOp::Union, 64).expect("boolean 2");
    // No vertex may leave the model's z-range: phantom band geometry shows
    // up as wall vertices far outside [0, 13].
    let mut bad = std::collections::HashSet::new();
    for (vid, v) in &u2.topology.vertices {
        if !(v.point.z > -1.0 && v.point.z < 14.0) {
            bad.insert(vid);
        }
    }
    if !bad.is_empty() {
        for (fid, face) in u2.topology.faces.iter() {
            let hes: Vec<_> = u2.topology.loop_half_edges(face.outer_loop).collect();
            if hes.iter().any(|&he| bad.contains(&u2.topology.half_edges[he].origin)) {
                let pts: Vec<_> = hes
                    .iter()
                    .map(|&he| {
                        let p = u2.topology.vertices[u2.topology.half_edges[he].origin].point;
                        (
                            (p.x * 100.0).round() / 100.0,
                            (p.y * 100.0).round() / 100.0,
                            (p.z * 100.0).round() / 100.0,
                        )
                    })
                    .collect();
                eprintln!(
                    "PHANTOM FACE {fid:?} {:?} nv={} pts {:?}",
                    u2.geometry.surfaces[face.surface_index].surface_type(),
                    pts.len(),
                    &pts[..pts.len().min(12)]
                );
            }
        }
        panic!("{} phantom vertices", bad.len());
    }
}
