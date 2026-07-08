//! End-to-end import of the extended surface entities that real
//! SolidWorks/Fusion/Catia exports rely on: SURFACE_OF_REVOLUTION,
//! SURFACE_OF_LINEAR_EXTRUSION, and OFFSET_SURFACE.
//!
//! Each fixture is a complete hand-written solid whose lateral face uses one
//! of the new entities. Before these were supported, the reader silently
//! skipped the face and the imported solid came back with a hole in it.
//! The tests check the full pipeline: import → solid → tessellate, with a
//! sane mesh volume and bounding box.

use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_step::read_step_from_buffer;
use vcad_kernel_tessellate::{tessellate_brep, tessellate_solid, TessellationParams, TriangleMesh};

/// A closed cylinder-shaped solid (r=10, h=20, axis Z): two planar caps and
/// one lateral face whose surface is substituted per fixture. The seam edge
/// curve is substituted alongside it (`<SEAM>`), since the vase fixture
/// sweeps a B-spline profile that doubles as its seam edge.
const CYLINDER_TEMPLATE: &str = r#"ISO-10303-21;
HEADER;
FILE_DESCRIPTION((''), '2;1');
FILE_NAME('fixture.step', '2024-01-01', (''), (''), '', '', '');
FILE_SCHEMA(('AUTOMOTIVE_DESIGN'));
ENDSEC;
DATA;
/* seam vertices */
#1 = CARTESIAN_POINT('', (10.0, 0.0, 0.0));
#2 = CARTESIAN_POINT('', (10.0, 0.0, 20.0));
#3 = VERTEX_POINT('', #1);
#4 = VERTEX_POINT('', #2);

/* rim circles */
#10 = CARTESIAN_POINT('', (0.0, 0.0, 0.0));
#11 = DIRECTION('', (0.0, 0.0, 1.0));
#12 = DIRECTION('', (1.0, 0.0, 0.0));
#13 = AXIS2_PLACEMENT_3D('', #10, #11, #12);
#14 = CIRCLE('', #13, 10.0);
#15 = CARTESIAN_POINT('', (0.0, 0.0, 20.0));
#16 = AXIS2_PLACEMENT_3D('', #15, #11, #12);
#17 = CIRCLE('', #16, 10.0);

/* seam line */
#18 = VECTOR('', #11, 20.0);
#19 = LINE('', #1, #18);

/* edges */
#20 = EDGE_CURVE('', #3, #3, #14, .T.);
#21 = EDGE_CURVE('', #4, #4, #17, .T.);
#22 = EDGE_CURVE('', #3, #4, <SEAM>, .T.);

/* lateral face: bottom rim, up the seam, top rim reversed, down the seam */
#23 = ORIENTED_EDGE('', *, *, #20, .T.);
#24 = ORIENTED_EDGE('', *, *, #22, .T.);
#25 = ORIENTED_EDGE('', *, *, #21, .F.);
#26 = ORIENTED_EDGE('', *, *, #22, .F.);
#27 = EDGE_LOOP('', (#23, #24, #25, #26));
#28 = FACE_OUTER_BOUND('', #27, .T.);
#29 = ADVANCED_FACE('', (#28), #40, .T.);

/* bottom cap (normal -Z) */
#30 = DIRECTION('', (0.0, 0.0, -1.0));
#31 = AXIS2_PLACEMENT_3D('', #10, #30, #12);
#32 = PLANE('', #31);
#33 = ORIENTED_EDGE('', *, *, #20, .F.);
#34 = EDGE_LOOP('', (#33));
#35 = FACE_OUTER_BOUND('', #34, .T.);
#36 = ADVANCED_FACE('', (#35), #32, .T.);

/* top cap (normal +Z) */
#37 = AXIS2_PLACEMENT_3D('', #15, #11, #12);
#38 = PLANE('', #37);
#39 = ORIENTED_EDGE('', *, *, #21, .T.);
#41 = EDGE_LOOP('', (#39));
#42 = FACE_OUTER_BOUND('', #41, .T.);
#43 = ADVANCED_FACE('', (#42), #38, .T.);

/* lateral surface (fixture-specific, defines #40) */
<LATERAL>

#44 = CLOSED_SHELL('', (#29, #36, #43));
#45 = MANIFOLD_SOLID_BREP('part', #44);
ENDSEC;
END-ISO-10303-21;
"#;

fn build_fixture(seam_curve: &str, lateral: &str) -> String {
    CYLINDER_TEMPLATE
        .replace("<SEAM>", seam_curve)
        .replace("<LATERAL>", lateral)
}

/// Signed mesh volume via the divergence theorem.
fn mesh_volume(mesh: &TriangleMesh) -> f64 {
    let pt = |i: u32| {
        let i = i as usize * 3;
        [
            mesh.vertices[i] as f64,
            mesh.vertices[i + 1] as f64,
            mesh.vertices[i + 2] as f64,
        ]
    };
    let mut vol = 0.0;
    for tri in mesh.indices.chunks(3) {
        let a = pt(tri[0]);
        let b = pt(tri[1]);
        let c = pt(tri[2]);
        vol += a[0] * (b[1] * c[2] - b[2] * c[1])
            + a[1] * (b[2] * c[0] - b[0] * c[2])
            + a[2] * (b[0] * c[1] - b[1] * c[0]);
    }
    vol / 6.0
}

/// Axis-aligned bounding box of a mesh as (min, max).
fn mesh_bbox(mesh: &TriangleMesh) -> ([f64; 3], [f64; 3]) {
    let mut min = [f64::MAX; 3];
    let mut max = [f64::MIN; 3];
    for v in mesh.vertices.chunks(3) {
        for k in 0..3 {
            min[k] = min[k].min(v[k] as f64);
            max[k] = max[k].max(v[k] as f64);
        }
    }
    (min, max)
}

/// Import the fixture and require all 3 faces to survive (no silent skips).
fn import_solid(step_text: &str) -> BRepSolid {
    let mut solids = read_step_from_buffer(step_text.as_bytes()).expect("import should succeed");
    assert_eq!(solids.len(), 1);
    let solid = solids.remove(0);
    assert_eq!(
        solid.topology.faces.len(),
        3,
        "no face may be skipped as unsupported"
    );
    solid
}

/// Tessellate through the app path (`tessellate_brep` is what the facade
/// crate's `to_mesh` uses) — appropriate for analytic surfaces.
fn app_mesh(solid: &BRepSolid) -> TriangleMesh {
    let mesh = tessellate_brep(solid, 32);
    assert!(
        mesh.num_triangles() > 0,
        "tessellation produced no triangles"
    );
    mesh
}

fn assert_cylinder_metrics(mesh: &TriangleMesh) {
    // Ideal cylinder: V = π r² h with r=10, h=20. Rim circles are sampled
    // polygons, so allow the volume to come in under the analytic value.
    let ideal = std::f64::consts::PI * 100.0 * 20.0;
    let vol = mesh_volume(mesh);
    assert!(
        vol > 0.85 * ideal && vol < 1.02 * ideal,
        "volume {vol} not within tolerance of ideal {ideal}"
    );
    let (min, max) = mesh_bbox(mesh);
    for k in 0..2 {
        assert!(
            (-10.5..=-9.0).contains(&min[k]),
            "bbox min[{k}] = {}",
            min[k]
        );
        assert!((9.0..=10.5).contains(&max[k]), "bbox max[{k}] = {}", max[k]);
    }
    assert!(min[2].abs() < 1e-3, "bbox z min = {}", min[2]);
    assert!((max[2] - 20.0).abs() < 1e-3, "bbox z max = {}", max[2]);
}

#[test]
fn import_surface_of_revolution_cylinder() {
    // Lateral face: the seam line (parallel to the axis) revolved about Z.
    // Must map to an analytic cylinder and import as a closed solid.
    let step = build_fixture(
        "#19",
        "#46 = AXIS1_PLACEMENT('', #10, #11);
#40 = SURFACE_OF_REVOLUTION('', #19, #46);",
    );
    let mesh = app_mesh(&import_solid(&step));
    assert_cylinder_metrics(&mesh);
}

#[test]
fn import_surface_of_linear_extrusion_cylinder() {
    // Lateral face: the bottom rim circle extruded 20mm along +Z.
    let step = build_fixture("#19", "#40 = SURFACE_OF_LINEAR_EXTRUSION('', #14, #18);");
    let mesh = app_mesh(&import_solid(&step));
    assert_cylinder_metrics(&mesh);
}

#[test]
fn import_offset_surface_cylinder() {
    // Lateral face: a radius-8 cylindrical surface offset outward by 2mm.
    let step = build_fixture(
        "#19",
        "#46 = CYLINDRICAL_SURFACE('', #13, 8.0);
#40 = OFFSET_SURFACE('', #46, 2.0, .F.);",
    );
    let mesh = app_mesh(&import_solid(&step));
    assert_cylinder_metrics(&mesh);
}

#[test]
fn import_surface_of_revolution_bspline_vase() {
    // Lateral face: a quadratic B-spline profile (bulging from r=10 to r=12
    // at mid-height) revolved about Z — exercises the exact rational NURBS
    // revolution fallback end to end. The profile doubles as the seam edge.
    let lateral = "#50 = CARTESIAN_POINT('', (12.0, 0.0, 10.0));
#51 = B_SPLINE_CURVE_WITH_KNOTS('', 2, (#1, #50, #2),
     .UNSPECIFIED., .F., .F.,
     (3, 3), (0.0, 1.0), .UNSPECIFIED.);
#52 = AXIS1_PLACEMENT('', #10, #11);
#40 = SURFACE_OF_REVOLUTION('', #51, #52);";
    let step = build_fixture("#51", lateral);
    let solid = import_solid(&step);
    // Use the per-face-faithful path: `tessellate_solid` evaluates B-spline
    // faces on the surface itself (`tessellate_brep`, the display path,
    // still flattens B-spline faces to their boundary loop — a pre-existing
    // limitation that applies to all imported B-spline surfaces alike).
    let mesh = tessellate_solid(&solid, &TessellationParams::from_segments(32));
    assert!(mesh.num_triangles() > 0);

    // Exact solid of revolution: V = π ∫ r(z)² dz with
    // r(t) = 10 + 4t(1-t), z = 20t  →  V = π·20·(100 + 80/6 + 16/30)
    let ideal = std::f64::consts::PI * 20.0 * (100.0 + 80.0 / 6.0 + 16.0 / 30.0);
    let vol = mesh_volume(&mesh);
    assert!(
        vol > 0.85 * ideal && vol < 1.03 * ideal,
        "volume {vol} not within tolerance of ideal {ideal}"
    );

    let (min, max) = mesh_bbox(&mesh);
    // Max radius 11 at mid-height.
    for k in 0..2 {
        assert!(
            (-11.5..=-10.0).contains(&min[k]),
            "bbox min[{k}] = {}",
            min[k]
        );
        assert!(
            (10.0..=11.5).contains(&max[k]),
            "bbox max[{k}] = {}",
            max[k]
        );
    }
    assert!(min[2].abs() < 1e-3, "bbox z min = {}", min[2]);
    assert!((max[2] - 20.0).abs() < 1e-3, "bbox z max = {}", max[2]);
}
