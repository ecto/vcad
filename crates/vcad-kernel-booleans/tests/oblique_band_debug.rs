//! Diagnostic harness for oblique (sampled-curve) cylinder splits: the torr
//! B1 case — a thin rotated blade cube ~99% inside a cylinder, corner
//! slivers poking out. Prints per-face classification so band-splitter
//! regressions are debuggable; asserts only coarse invariants.

use vcad_kernel_booleans::classify::{classify_all_faces_with_mesh, FaceClassification};
use vcad_kernel_booleans::{boolean_op, BooleanOp, BooleanResult};
use vcad_kernel_math::Transform;
use vcad_kernel_primitives::{make_cube, make_cylinder, BRepSolid};
use vcad_kernel_tessellate::tessellate_brep;

fn transform_brep(brep: &mut BRepSolid, t: &Transform) {
    for (_id, v) in &mut brep.topology.vertices {
        v.point = t.apply_point(&v.point);
    }
    for s in &mut brep.geometry.surfaces {
        *s = s.transform(t);
    }
}

fn blade() -> BRepSolid {
    let mut b = make_cube(23.5, 0.5, 12.57);
    let rot = Transform::rotation_x(39.29_f64.to_radians());
    let tr = Transform::translation(21.5, 0.0, 0.0);
    let combined = tr.then(&rot);
    transform_brep(&mut b, &combined);
    b
}

fn mesh_volume(mesh: &vcad_kernel_tessellate::TriangleMesh) -> f64 {
    let mut vol = 0.0;
    for t in mesh.indices.chunks(3) {
        let p = |i: u32| {
            let b = i as usize * 3;
            (
                mesh.vertices[b] as f64,
                mesh.vertices[b + 1] as f64,
                mesh.vertices[b + 2] as f64,
            )
        };
        let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
        vol += (a.0 * (b.1 * c.2 - b.2 * c.1) - a.1 * (b.0 * c.2 - b.2 * c.0)
            + a.2 * (b.0 * c.1 - b.1 * c.0))
            / 6.0;
    }
    vol
}

#[test]
fn b1_intersection_diagnostics() {
    let cyl = make_cylinder(45.0, 13.0, 32);
    let bl = blade();

    let BooleanResult::BRep(result) =
        boolean_op(&cyl, &bl, BooleanOp::Intersection, 64).expect("boolean");
    let mesh = tessellate_brep(&result, 64);
    let vol = mesh_volume(&mesh);

    // Diagnostics: classify both operands' faces against each other the way
    // the pipeline does (post-split state is internal, so re-run classify on
    // the *inputs* for orientation sanity, then report the result volume).
    let mesh_cyl = tessellate_brep(&cyl, 64);
    let mesh_bl = tessellate_brep(&bl, 64);
    eprintln!("cyl volume {}", mesh_volume(&mesh_cyl));
    eprintln!("blade volume {}", mesh_volume(&mesh_bl));
    for (name, solid, other, other_mesh) in [
        ("cyl-vs-blade", &cyl, &bl, &mesh_bl),
        ("blade-vs-cyl", &bl, &cyl, &mesh_cyl),
    ] {
        let classes = classify_all_faces_with_mesh(solid, other, other_mesh);
        let mut counts = std::collections::HashMap::new();
        for (_, c) in &classes {
            *counts.entry(format!("{c:?}")).or_insert(0) += 1;
        }
        eprintln!("{name} face classes: {counts:?}");
    }

    eprintln!("intersection volume = {vol:.4} (expected ≈ 146.32, blade = 147.6975)");
    eprintln!(
        "result: {} faces, {} tris",
        result.topology.faces.len(),
        mesh.indices.len() / 3
    );
    let _ = FaceClassification::Inside;

    assert!(
        (vol - 146.32).abs() < 1.5,
        "blade ∩ cyl45 volume {vol:.4}, want ≈146.32"
    );
}

#[test]
fn b1_difference_diagnostics() {
    let cyl = make_cylinder(45.0, 13.0, 32);
    let bl = blade();
    let BooleanResult::BRep(result) =
        boolean_op(&cyl, &bl, BooleanOp::Difference, 64).expect("boolean");
    let mesh = tessellate_brep(&result, 64);
    let vol = mesh_volume(&mesh);
    let expected = std::f64::consts::PI * 45.0 * 45.0 * 13.0 - 146.32;
    eprintln!("difference volume = {vol:.4} (expected ≈ {expected:.2} at exact tess)");
    eprintln!(
        "result: {} faces, {} tris",
        result.topology.faces.len(),
        mesh.indices.len() / 3
    );
    // 64-segment tessellation reads the cylinder ~0.16% low; allow 0.5%.
    assert!(
        (vol - expected).abs() < expected * 0.005,
        "cyl45 − blade volume {vol:.4}, want ≈{expected:.2}"
    );
}

#[test]
fn b1_intersection_result_face_dump() {
    let cyl = make_cylinder(45.0, 13.0, 32);
    let bl = blade();
    let BooleanResult::BRep(result) =
        boolean_op(&cyl, &bl, BooleanOp::Intersection, 64).expect("boolean");
    for (fid, face) in result.topology.faces.iter() {
        let verts: Vec<_> = result
            .topology
            .loop_half_edges(face.outer_loop)
            .map(|he| result.topology.vertices[result.topology.half_edges[he].origin].point)
            .collect();
        let (mut lo, mut hi) = ([f64::MAX; 3], [f64::MIN; 3]);
        for p in &verts {
            for (k, val) in [p.x, p.y, p.z].into_iter().enumerate() {
                lo[k] = lo[k].min(val);
                hi[k] = hi[k].max(val);
            }
        }
        let surf = result.geometry.surfaces[face.surface_index].surface_type();
        eprintln!(
            "{fid:?} {surf:?} nverts={} bbox x[{:.2},{:.2}] y[{:.2},{:.2}] z[{:.2},{:.2}] orient={:?}",
            verts.len(),
            lo[0], hi[0], lo[1], hi[1], lo[2], hi[2],
            face.orientation
        );
    }
}

#[test]
fn top_face_trim_finds_crossing() {
    use vcad_kernel_booleans::{ssi, trim};
    let cyl_solid = make_cylinder(45.0, 13.0, 32);
    let bl = blade();
    let wall = cyl_solid
        .geometry
        .surfaces
        .iter()
        .find(|s| {
            s.as_any()
                .downcast_ref::<vcad_kernel_geom::CylinderSurface>()
                .is_some()
        })
        .unwrap();
    // Find the blade's top face (centroid z highest).
    let mut best = None;
    let mut best_z = f64::MIN;
    for (fid, face) in bl.topology.faces.iter() {
        let verts: Vec<_> = bl
            .topology
            .loop_half_edges(face.outer_loop)
            .map(|he| bl.topology.vertices[bl.topology.half_edges[he].origin].point)
            .collect();
        let cz = verts.iter().map(|p| p.z).sum::<f64>() / verts.len() as f64;
        if cz > best_z {
            best_z = cz;
            best = Some((fid, face.surface_index));
        }
    }
    let (top_face, top_surf_idx) = best.unwrap();
    let curve = ssi::intersect_surfaces(wall.as_ref(), bl.geometry.surfaces[top_surf_idx].as_ref())
        .expect("ssi");
    eprintln!(
        "curve kind: {}",
        match &curve {
            ssi::IntersectionCurve::Sampled(p) => format!("Sampled({})", p.len()),
            other => format!("{other:?}").chars().take(40).collect(),
        }
    );
    let segs = trim::trim_curve_to_face(&curve, top_face, &bl, 64);
    eprintln!("top-face trim segments: {segs:?}");
    assert!(
        !segs.is_empty(),
        "trim must find the corner crossing on the top face"
    );
}

#[test]
fn top_face_point_in_face_scan() {
    use vcad_kernel_booleans::{ssi, trim};
    let cyl_solid = make_cylinder(45.0, 13.0, 32);
    let bl = blade();
    let wall = cyl_solid
        .geometry
        .surfaces
        .iter()
        .find(|s| {
            s.as_any()
                .downcast_ref::<vcad_kernel_geom::CylinderSurface>()
                .is_some()
        })
        .unwrap();
    let mut best = None;
    let mut best_z = f64::MIN;
    for (fid, face) in bl.topology.faces.iter() {
        let verts: Vec<_> = bl
            .topology
            .loop_half_edges(face.outer_loop)
            .map(|he| bl.topology.vertices[bl.topology.half_edges[he].origin].point)
            .collect();
        let cz = verts.iter().map(|p| p.z).sum::<f64>() / verts.len() as f64;
        if cz > best_z {
            best_z = cz;
            best = Some((fid, face.surface_index));
        }
    }
    let (top_face, top_surf_idx) = best.unwrap();
    let curve = ssi::intersect_surfaces(wall.as_ref(), bl.geometry.surfaces[top_surf_idx].as_ref())
        .expect("ssi");
    let ssi::IntersectionCurve::Sampled(pts) = curve else {
        panic!()
    };
    // Scan interval 62..63 at fine resolution
    let (a, b) = (pts[62], pts[63]);
    let mut pattern = String::new();
    for k in 0..=100 {
        let f = k as f64 / 100.0;
        let p = a + f * (b - a);
        pattern.push(if trim::point_in_face(&bl, top_face, &p) {
            'I'
        } else {
            '.'
        });
    }
    eprintln!("interval62 scan: {pattern}");
    // Also print the two boundary-edge crossings for reference
    eprintln!("p62 = {:?}", a);
    eprintln!("p63 = {:?}", b);
}

#[test]
fn b1_intersection_mesh_area_by_kind() {
    let cyl = make_cylinder(45.0, 13.0, 32);
    let bl = blade();
    let BooleanResult::BRep(result) =
        boolean_op(&cyl, &bl, BooleanOp::Intersection, 64).expect("boolean");
    let mesh = tessellate_brep(&result, 64);
    let mut area_by_kind = std::collections::HashMap::new();
    for (t, kind) in mesh.indices.chunks(3).zip(mesh.face_kinds.iter()) {
        let p = |i: u32| {
            let b = i as usize * 3;
            vcad_kernel_math::Vec3::new(
                mesh.vertices[b] as f64,
                mesh.vertices[b + 1] as f64,
                mesh.vertices[b + 2] as f64,
            )
        };
        let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
        let ar = 0.5 * (b - a).cross(c - a).norm();
        *area_by_kind.entry(*kind).or_insert(0.0) += ar;
    }
    eprintln!("mesh area by face kind: {area_by_kind:?}");
    eprintln!("total tris: {}", mesh.indices.len() / 3);
}
