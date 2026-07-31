//! The import report: a face whose surface type is unsupported must be
//! recorded — entity id and surface type name — instead of vanishing
//! silently from the imported solid.

use vcad_kernel_step::{read_step_from_buffer, read_step_from_buffer_with_report};

/// A closed cylinder-shaped solid (r=10, h=20, axis Z) whose lateral face
/// surface (#40) is substituted per test. Same shape as the
/// extended_surfaces fixture.
const CYLINDER_TEMPLATE: &str = r#"ISO-10303-21;
HEADER;
FILE_DESCRIPTION((''), '2;1');
FILE_NAME('fixture.step', '2024-01-01', (''), (''), '', '', '');
FILE_SCHEMA(('AUTOMOTIVE_DESIGN'));
ENDSEC;
DATA;
#1 = CARTESIAN_POINT('', (10.0, 0.0, 0.0));
#2 = CARTESIAN_POINT('', (10.0, 0.0, 20.0));
#3 = VERTEX_POINT('', #1);
#4 = VERTEX_POINT('', #2);
#10 = CARTESIAN_POINT('', (0.0, 0.0, 0.0));
#11 = DIRECTION('', (0.0, 0.0, 1.0));
#12 = DIRECTION('', (1.0, 0.0, 0.0));
#13 = AXIS2_PLACEMENT_3D('', #10, #11, #12);
#14 = CIRCLE('', #13, 10.0);
#15 = CARTESIAN_POINT('', (0.0, 0.0, 20.0));
#16 = AXIS2_PLACEMENT_3D('', #15, #11, #12);
#17 = CIRCLE('', #16, 10.0);
#18 = VECTOR('', #11, 20.0);
#19 = LINE('', #1, #18);
#20 = EDGE_CURVE('', #3, #3, #14, .T.);
#21 = EDGE_CURVE('', #4, #4, #17, .T.);
#22 = EDGE_CURVE('', #3, #4, #19, .T.);
#23 = ORIENTED_EDGE('', *, *, #20, .T.);
#24 = ORIENTED_EDGE('', *, *, #22, .T.);
#25 = ORIENTED_EDGE('', *, *, #21, .F.);
#26 = ORIENTED_EDGE('', *, *, #22, .F.);
#27 = EDGE_LOOP('', (#23, #24, #25, #26));
#28 = FACE_OUTER_BOUND('', #27, .T.);
#29 = ADVANCED_FACE('', (#28), #40, .T.);
#30 = DIRECTION('', (0.0, 0.0, -1.0));
#31 = AXIS2_PLACEMENT_3D('', #10, #30, #12);
#32 = PLANE('', #31);
#33 = ORIENTED_EDGE('', *, *, #20, .F.);
#34 = EDGE_LOOP('', (#33));
#35 = FACE_OUTER_BOUND('', #34, .T.);
#36 = ADVANCED_FACE('', (#35), #32, .T.);
#37 = AXIS2_PLACEMENT_3D('', #15, #11, #12);
#38 = PLANE('', #37);
#39 = ORIENTED_EDGE('', *, *, #21, .T.);
#41 = EDGE_LOOP('', (#39));
#42 = FACE_OUTER_BOUND('', #41, .T.);
#43 = ADVANCED_FACE('', (#42), #38, .T.);
<LATERAL>
#44 = CLOSED_SHELL('', (#29, #36, #43));
#45 = MANIFOLD_SOLID_BREP('part', #44);
ENDSEC;
END-ISO-10303-21;
"#;

fn build_fixture(lateral: &str) -> String {
    CYLINDER_TEMPLATE.replace("<LATERAL>", lateral)
}

#[test]
fn unsupported_surface_is_reported_not_silent() {
    // DEGENERATE_TOROIDAL_SURFACE is a real AP214 entity the reader does not
    // support — the lateral face must be skipped AND reported.
    let step = build_fixture("#40 = DEGENERATE_TOROIDAL_SURFACE('', #13, 10.0, 10.0, .T.);");
    let result =
        read_step_from_buffer_with_report(step.as_bytes()).expect("import should still succeed");

    assert_eq!(result.solids.len(), 1);
    // Only the two planar caps survive.
    assert_eq!(result.solids[0].topology.faces.len(), 2);

    assert!(!result.report.is_clean());
    assert_eq!(result.report.total_skipped_faces(), 1);

    let solid_report = &result.report.solids[0];
    assert_eq!(solid_report.solid_id, 45);
    assert_eq!(solid_report.total_faces, 3);

    let skipped = &solid_report.skipped_faces[0];
    assert_eq!(skipped.face_id, 29);
    assert_eq!(skipped.surface_id, 40);
    assert!(
        skipped.reason.contains("DEGENERATE_TOROIDAL_SURFACE"),
        "reason should name the unsupported surface type, got: {}",
        skipped.reason
    );

    let summary = result.report.summary().expect("dirty import has a summary");
    assert!(summary.contains("DEGENERATE_TOROIDAL_SURFACE"));
    assert!(summary.contains("#29"));
}

#[test]
fn clean_import_has_clean_report() {
    let step = build_fixture("#40 = CYLINDRICAL_SURFACE('', #13, 10.0);");
    let result = read_step_from_buffer_with_report(step.as_bytes()).expect("import should succeed");

    assert_eq!(result.solids[0].topology.faces.len(), 3);
    assert!(result.report.is_clean());
    assert_eq!(result.report.summary(), None);
    // Curved rim edges are chord-subdivided — noted, not per-face.
    assert!(!result.report.solids[0].notes.is_empty());
}

#[test]
fn compat_wrapper_still_returns_bare_solids() {
    let step = build_fixture("#40 = CYLINDRICAL_SURFACE('', #13, 10.0);");
    let solids = read_step_from_buffer(step.as_bytes()).expect("import should succeed");
    assert_eq!(solids.len(), 1);
}
