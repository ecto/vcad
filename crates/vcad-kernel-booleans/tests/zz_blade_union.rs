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
    for (fid, face) in u1.topology.faces.iter() {
        if u1.geometry.surfaces[face.surface_index].surface_type()
            != vcad_kernel_geom::SurfaceKind::Cylinder
        {
            continue;
        }
        let pts: Vec<_> = u1
            .topology
            .loop_half_edges(face.outer_loop)
            .map(|he| u1.topology.vertices[u1.topology.half_edges[he].origin].point)
            .collect();
        let zmin = pts.iter().map(|p| p.z).fold(f64::MAX, f64::min);
        let zmax = pts.iter().map(|p| p.z).fold(f64::MIN, f64::max);
        let umin = pts
            .iter()
            .map(|p| p.y.atan2(p.x))
            .fold(f64::MAX, f64::min);
        let umax = pts
            .iter()
            .map(|p| p.y.atan2(p.x))
            .fold(f64::MIN, f64::max);
        eprintln!(
            "U1WALL {fid:?} nv={} z[{zmin:.3},{zmax:.3}] u[{umin:.3},{umax:.3}]",
            pts.len()
        );
        if pts.len() == 58 || pts.len() == 21 {
            let uv: Vec<_> = pts
                .iter()
                .map(|p| {
                    (
                        (p.y.atan2(p.x) * 1e3).round() / 1e3,
                        (p.z * 1e3).round() / 1e3,
                    )
                })
                .collect();
            eprintln!("U1LOOP {uv:?}");
        }
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
    let mut opens: Vec<_> = net
        .iter()
        .filter(|(_, &n)| n != 0)
        .map(|((a, b), _)| (*a, *b))
        .collect();
    opens.sort();
    for (a, b) in opens.iter().take(8) {
        eprintln!(
            "open ({:.4},{:.4},{:.4})->({:.4},{:.4},{:.4})",
            a[0] as f64 * quantum,
            a[1] as f64 * quantum,
            a[2] as f64 * quantum,
            b[0] as f64 * quantum,
            b[1] as f64 * quantum,
            b[2] as f64 * quantum
        );
    }
    if opens.len() > 4 {
        // Which faces' tessellations emit the phantom ring edges?
        let params = vcad_kernel_tessellate::TessellationParams::from_segments(64);
        for (fid, kind, fmesh) in
            vcad_kernel_tessellate::tessellate_brep_by_face(&u2, &params)
        {
            let hits = fmesh
                .vertices
                .chunks(3)
                .filter(|c| (c[2] as f64 - 10.8146).abs() < 1e-3 && (c[0] as f64) < -15.0)
                .count();
            if hits > 2 {
                let nv = u2.topology.loop_len(u2.topology.faces[fid].outer_loop);
                eprintln!("EMITTER {fid:?} {kind:?} nv={nv} ring-verts {hits}");
            }
        }
        for (fid, face) in u2.topology.faces.iter() {
            if u2.geometry.surfaces[face.surface_index].surface_type()
                != vcad_kernel_geom::SurfaceKind::Cylinder
            {
                continue;
            }
            let pts: Vec<_> = u2
                .topology
                .loop_half_edges(face.outer_loop)
                .map(|he| u2.topology.vertices[u2.topology.half_edges[he].origin].point)
                .collect();
            if pts.iter().any(|p| (p.z - 10.8146).abs() < 1e-3) {
                let uv: Vec<_> = pts
                    .iter()
                    .map(|p| {
                        let u = p.y.atan2(p.x);
                        ((u * 1e3).round() / 1e3, (p.z * 1e3).round() / 1e3)
                    })
                    .collect();
                if pts.len() > 100 {
                    eprintln!("RINGFACE {fid:?} nv={} FULL {uv:?}", pts.len());
                } else {
                    eprintln!("RINGFACE {fid:?} nv={} {:?}", pts.len(), &uv[..uv.len().min(14)]);
                }
            }
        }
    }
    assert!(opens.is_empty(), "{} mesh open edges", opens.len());
}

#[test]
fn zz_one_rotated_blade() {
    let cyl = make_cylinder(22.5, 13.0, 32);
    let mut b = make_cube(23.5, 0.5, 12.57);
    let t = Transform::translation(21.5, 0.0, 0.0)
        .then(&Transform::rotation_x(39.29_f64.to_radians()));
    transform_brep(&mut b, &t);
    let BooleanResult::BRep(u) = boolean_op(&b, &cyl, BooleanOp::Union, 32).expect("boolean");
    let mut unpaired = 0;
    for (_h, he) in &u.topology.half_edges {
        if he.loop_id.is_some() && he.twin.is_none() {
            unpaired += 1;
        }
    }
    assert_eq!(unpaired, 0, "{unpaired} unpaired half-edges");
}
