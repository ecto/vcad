//! Regression tests for view orientation (un-mirroring, 2026-07).
//!
//! Until 2026-07 every `ViewDirection` basis was mirrored: `view_vector()`
//! pointed viewer → scene while the projection basis and the front-facing
//! test assumed scene → viewer, so each labeled view drew the *opposite*
//! side of the part as a mirror image. Fine for symmetric demos, unusable
//! for shop drawings. These tests pin the fixed behavior with an
//! asymmetric part.

use vcad_kernel_drafting::{project_mesh, project_point, ViewDirection, Visibility};
use vcad_kernel_math::Point3;
use vcad_kernel_tessellate::TriangleMesh;

/// An L-bracket (legs along +X and +Y from the corner at the origin) with
/// the center of an off-center hole in the X leg as a witness point.
///
/// Leg extents: X leg x∈[0,60], y∈[0,20]; Y leg x∈[0,20], y∈[0,60];
/// thickness z∈[0,10]. Hole center at (45, 10, 5) — far out on the X leg.
const HOLE: Point3 = Point3 {
    x: 45.0,
    y: 10.0,
    z: 5.0,
};
/// Center of the bracket's bounding box.
const CENTER: Point3 = Point3 {
    x: 30.0,
    y: 30.0,
    z: 5.0,
};

/// The hole must land on the correct side of the drawing in each of the 6
/// principal views (third-angle convention). With the pre-fix mirrored
/// bases, every horizontal expectation below flips sign.
#[test]
fn l_bracket_hole_lands_on_correct_side_in_all_views() {
    // (view, expected sign of hole-minus-center on drawing X,
    //  expected sign on drawing Y; 0 = don't care)
    let cases: &[(ViewDirection, f64, f64)] = &[
        // Hole is on the part's +X side, front half (-Y), mid-height.
        (ViewDirection::Front, 1.0, 0.0),  // +X maps right
        (ViewDirection::Back, -1.0, 0.0),  // +X maps left
        (ViewDirection::Top, 1.0, -1.0),   // +X right, -Y down
        (ViewDirection::Bottom, 1.0, 1.0), // +X right, -Y up
        (ViewDirection::Right, -1.0, 0.0), // -Y (front half) maps left
        (ViewDirection::Left, 1.0, 0.0),   // -Y (front half) maps right
    ];

    for &(view, want_x, want_y) in cases {
        let hole_2d = project_point(HOLE, view);
        let center_2d = project_point(CENTER, view);
        let dx = hole_2d.x - center_2d.x;
        let dy = hole_2d.y - center_2d.y;

        if want_x != 0.0 {
            assert!(
                dx * want_x > 1.0,
                "{view:?}: hole offset on drawing X is {dx}, expected sign {want_x} (mirrored view?)",
            );
        }
        if want_y != 0.0 {
            assert!(
                dy * want_y > 1.0,
                "{view:?}: hole offset on drawing Y is {dy}, expected sign {want_y} (mirrored view?)",
            );
        }
    }
}

/// Append an axis-aligned box to a triangle soup with outward-facing
/// (CCW-from-outside) triangles.
fn push_box(mesh: &mut TriangleMesh, min: [f32; 3], max: [f32; 3]) {
    let base = (mesh.vertices.len() / 3) as u32;
    let [x0, y0, z0] = min;
    let [x1, y1, z1] = max;
    #[rustfmt::skip]
    let verts: [[f32; 3]; 8] = [
        [x0, y0, z0], [x1, y0, z0], [x1, y1, z0], [x0, y1, z0],
        [x0, y0, z1], [x1, y0, z1], [x1, y1, z1], [x0, y1, z1],
    ];
    for v in verts {
        mesh.vertices.extend_from_slice(&v);
    }
    #[rustfmt::skip]
    let idx: [u32; 36] = [
        0, 2, 1, 0, 3, 2, // bottom (-Z)
        4, 5, 6, 4, 6, 7, // top (+Z)
        0, 1, 5, 0, 5, 4, // front (-Y)
        2, 3, 7, 2, 7, 6, // back (+Y)
        0, 4, 7, 0, 7, 3, // left (-X)
        1, 2, 6, 1, 6, 5, // right (+X)
    ];
    mesh.indices.extend(idx.iter().map(|i| i + base));
}

/// A small block sitting in front (-Y) of a larger slab. The Front view
/// must show the block's outline as visible; the Back view must hide it
/// entirely behind the slab. The pre-fix code rendered each view from the
/// opposite side, which inverts exactly this visibility.
#[test]
fn front_view_shows_near_geometry_and_back_view_hides_it() {
    let mut mesh = TriangleMesh {
        vertices: Vec::new(),
        indices: Vec::new(),
        normals: Vec::new(),
        face_kinds: Vec::new(),
    };
    // Big slab behind (y ∈ [10, 20]).
    push_box(&mut mesh, [0.0, 10.0, 0.0], [20.0, 20.0, 20.0]);
    // Small block in front of it (y ∈ [0, 10]), centered on the slab.
    push_box(&mut mesh, [5.0, 0.0, 5.0], [15.0, 10.0, 15.0]);

    // In the Front view the small block projects to the square
    // x ∈ [5, 15], y ∈ [5, 15]; the slab's own edges lie on x ∈ {0, 20},
    // y ∈ {0, 20}, so any edge strictly inside the square belongs to the
    // block and must be visible.
    let front = project_mesh(&mesh, ViewDirection::Front);
    let inside = |x: f64, y: f64, lo: f64, hi: f64| x > lo && x < hi && y > lo && y < hi;
    let block_edge_visible = front.edges.iter().any(|e| {
        e.visibility == Visibility::Visible
            && inside(e.start.x, e.start.y, 4.9, 15.1)
            && inside(e.end.x, e.end.y, 4.9, 15.1)
    });
    assert!(
        block_edge_visible,
        "Front view: near block should have visible edges (is the view rendered from the back?)",
    );

    // In the Back view the block sits behind the slab: same square but at
    // x ∈ [-15, -5] (world +X maps to drawing -X). Everything there must
    // be hidden.
    let back = project_mesh(&mesh, ViewDirection::Back);
    let block_edge_visible_from_back = back.edges.iter().any(|e| {
        e.visibility == Visibility::Visible
            && e.start.x > -15.1
            && e.start.x < -4.9
            && e.start.y > 4.9
            && e.start.y < 15.1
            && e.end.x > -15.1
            && e.end.x < -4.9
            && e.end.y > 4.9
            && e.end.y < 15.1
    });
    assert!(
        !block_edge_visible_from_back,
        "Back view: block is occluded by the slab and must be hidden",
    );
}
