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
