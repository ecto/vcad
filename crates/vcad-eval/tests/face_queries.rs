//! Document-level face queries: the exact path the `inspectDocumentFaces`
//! WASM binding (and therefore the `inspect_faces` / `measure_outer_diameter`
//! MCP tools) takes — parse a document, evaluate its roots to kernel solids,
//! then read the B-rep faces.
//!
//! The scenario is a motor in miniature, reproducing the failure that
//! motivated the tools: an 80 mm-diameter body with a small radial connector
//! boss, so the bounding box overstates the diameter, and a body axis of
//! **Y**, so anything that assumes Z gets the axis wrong.

use vcad_kernel::faces::SurfaceInfo;
use vcad_kernel_math::Vec3;

/// Body Ø80 about Y + a Ø12 boss sticking out along +X.
fn motor_document_json() -> String {
    serde_json::json!({
        "version": "0.1",
        "nodes": {
            "1": { "id": 1, "name": "body-raw",
                   "op": { "type": "Cylinder", "radius": 40.0, "height": 30.0, "segments": 64 } },
            "2": { "id": 2, "name": "body-y",
                   "op": { "type": "Rotate", "child": 1, "angles": { "x": -90.0, "y": 0.0, "z": 0.0 } } },
            "3": { "id": 3, "name": "boss-raw",
                   "op": { "type": "Cylinder", "radius": 6.0, "height": 30.0, "segments": 32 } },
            "4": { "id": 4, "name": "boss-x",
                   "op": { "type": "Rotate", "child": 3, "angles": { "x": 0.0, "y": 90.0, "z": 0.0 } } },
            "5": { "id": 5, "name": "boss",
                   "op": { "type": "Translate", "child": 4, "offset": { "x": 24.0, "y": -15.0, "z": 0.0 } } },
            "6": { "id": 6, "name": "motor",
                   "op": { "type": "Union", "left": 2, "right": 5 } }
        },
        "materials": {},
        "part_materials": {},
        "roots": [{ "root": 6, "material": "aluminum" }]
    })
    .to_string()
}

fn motor_report() -> vcad_kernel::faces::FaceReport {
    let doc: vcad_ir::Document = serde_json::from_str(&motor_document_json()).unwrap();
    let roots = vcad_eval::evaluate_root_solids(&doc).expect("document evaluates");
    assert_eq!(roots.len(), 1, "one visible root");
    roots[0]
        .solid
        .as_ref()
        .expect("root produced a kernel solid")
        .inspect_faces()
        .expect("the union keeps B-rep")
}

#[test]
fn body_outer_diameter_is_exact_where_the_bbox_overstates_it() {
    let report = motor_report();

    let group = report
        .largest_coaxial(None)
        .expect("the part has a dominant axis");

    // The whole point: exactly 80.0, not the bbox's inflated reading.
    assert!(
        (group.max_diameter_mm - 80.0).abs() < 1e-9,
        "OD {}",
        group.max_diameter_mm
    );

    // Dominant axis is Y. A tool that assumed Z would report the boss, or
    // nothing at all.
    assert!(
        (group.axis[1].abs() - 1.0).abs() < 1e-9,
        "axis {:?}",
        group.axis
    );

    // The bounding box across X is inflated well past 80 by the boss.
    let x_span = report
        .faces
        .iter()
        .map(|f| f.bbox_max_mm[0])
        .fold(f64::MIN, f64::max)
        - report
            .faces
            .iter()
            .map(|f| f.bbox_min_mm[0])
            .fold(f64::MAX, f64::min);
    assert!(
        x_span > 90.0,
        "bbox X span {x_span} should overstate the OD"
    );
}

#[test]
fn a_requested_axis_selects_its_own_cylinder() {
    let report = motor_report();

    let about_y = report
        .largest_coaxial(Some(Vec3::new(0.0, 1.0, 0.0)))
        .expect("a Y group");
    assert!((about_y.max_diameter_mm - 80.0).abs() < 1e-9);

    let about_x = report
        .largest_coaxial(Some(Vec3::new(1.0, 0.0, 0.0)))
        .expect("an X group (the boss)");
    assert!(
        (about_x.max_diameter_mm - 12.0).abs() < 1e-9,
        "boss OD {}",
        about_x.max_diameter_mm
    );

    // An axis nothing is built about resolves to nothing, rather than
    // silently falling back to the dominant axis.
    assert!(report
        .largest_coaxial(Some(Vec3::new(0.6, 0.8, 0.0)))
        .is_none());
}

#[test]
fn faces_carry_document_scoped_stable_names() {
    let report = motor_report();
    assert!(report.face_count > 0);

    // Evaluated documents rescope names to the node id, so ids stay unique
    // across a multi-primitive document and survive a parameter change.
    let named = report.faces.iter().filter(|f| f.stable).count();
    assert!(
        named > 0,
        "an evaluated boolean should keep some named faces: {:?}",
        report.faces.iter().map(|f| &f.id).collect::<Vec<_>>()
    );

    // Every id is unique — a caller can round-trip one back as a filter.
    let mut ids: Vec<&String> = report.faces.iter().map(|f| &f.id).collect();
    ids.sort();
    let before = ids.len();
    ids.dedup();
    assert_eq!(before, ids.len(), "face ids collide");
}

#[test]
fn planar_faces_report_outward_normals() {
    let report = motor_report();

    // The body's two end caps are planes with normals along ±Y.
    let caps: Vec<[f64; 3]> = report
        .faces
        .iter()
        .filter_map(|f| match f.surface {
            SurfaceInfo::Plane { normal, .. } if normal[1].abs() > 0.999 => Some(normal),
            _ => None,
        })
        .collect();
    assert!(caps.len() >= 2, "two Y-facing caps, got {}", caps.len());
    assert!(
        caps.iter().any(|n| n[1] > 0.0) && caps.iter().any(|n| n[1] < 0.0),
        "caps face opposite ways: {caps:?}"
    );
}

#[test]
fn the_split_body_wall_still_answers_as_one_diameter() {
    // The boss union trims the body wall into several faces on the same
    // cylinder — normal boolean behaviour, and precisely why a raw face list
    // is a poor answer to "what is the OD". The coaxial grouping reassembles
    // them: several faces in, one diameter and one full-height extent out.
    let report = motor_report();

    let walls: Vec<&vcad_kernel::faces::FaceInfo> = report
        .faces
        .iter()
        .filter(|f| match f.surface {
            SurfaceInfo::Cylinder { diameter_mm, .. } => (diameter_mm - 80.0).abs() < 1e-9,
            _ => false,
        })
        .collect();
    assert!(
        walls.len() > 1,
        "the union splits the wall, so a raw list fragments it"
    );
    assert!(
        walls.iter().all(|f| match f.surface {
            SurfaceInfo::Cylinder { convex, .. } => convex,
            _ => false,
        }),
        "every body-wall piece is an outer surface"
    );

    let group = report.largest_coaxial(None).expect("a dominant axis");
    assert_eq!(
        group.face_ids.len(),
        walls.len(),
        "the group gathers exactly the wall pieces"
    );
    let extent = group.axial_range_mm[1] - group.axial_range_mm[0];
    assert!(
        (extent - 30.0).abs() < 1e-6,
        "the group spans the full body height: {extent}"
    );
}
