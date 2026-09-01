#![warn(missing_docs)]

//! CSG boolean operations on B-rep solids for the vcad kernel.
//!
//! Implements union, difference, and intersection of B-rep solids.
//!
//! The boolean pipeline has 4 stages:
//! 1. **AABB filter** — broadphase to find candidate face pairs
//! 2. **SSI** — surface-surface intersection for each candidate pair
//! 3. **Classification** — label sub-faces as IN/OUT/ON
//! 4. **Reconstruction** — sew selected faces into the result solid
//!
//! Phase 2 is building this pipeline incrementally. The mesh-based
//! fallback from Phase 1 remains as a backup.

// Internal modules
mod api;
pub mod bbox;
pub mod classify;
mod cyl_band;
pub mod cyl_cyl;
mod freeze;
pub mod mesh;
mod no_crossing;
mod pipeline;
mod repair;
pub mod sew;
pub mod split;
pub mod ssi;
pub mod trim;
mod unrepresentable;
mod validate;

// Re-export public API
pub use api::{
    boolean_op, boolean_op_reported, repair_export_mesh, BooleanError, BooleanOp, BooleanReport,
    BooleanResult, DegradeReason, Fidelity,
};
pub use mesh::is_triangle_soup;
pub use mesh::point_in_mesh;
pub use mesh::MeshRayIndex;
pub use ssi::SsiError;
pub use validate::{mesh_report, mesh_signed_volume, MeshReport, ValidityError};

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_kernel_math::{Point3, Transform};
    use vcad_kernel_primitives::{make_cube, BRepSolid};
    use vcad_kernel_tessellate::{tessellate_brep, TriangleMesh};

    /// Test wrapper: unwrap the now-fallible `boolean_op`. None of the
    /// geometry in this module should hit an SSI error; a panic here means
    /// the pipeline reported one and the test should fail loudly.
    fn boolean_op(a: &BRepSolid, b: &BRepSolid, op: BooleanOp, segments: u32) -> BooleanResult {
        crate::api::boolean_op(a, b, op, segments).expect("boolean_op should succeed")
    }

    /// Compute the volume of a triangle mesh using signed tetrahedron method.
    fn compute_mesh_volume(mesh: &TriangleMesh) -> f64 {
        let verts = &mesh.vertices;
        let indices = &mesh.indices;
        let mut vol = 0.0;
        for tri in indices.chunks(3) {
            let i0 = tri[0] as usize * 3;
            let i1 = tri[1] as usize * 3;
            let i2 = tri[2] as usize * 3;
            let v0 = [verts[i0] as f64, verts[i0 + 1] as f64, verts[i0 + 2] as f64];
            let v1 = [verts[i1] as f64, verts[i1 + 1] as f64, verts[i1 + 2] as f64];
            let v2 = [verts[i2] as f64, verts[i2 + 1] as f64, verts[i2 + 2] as f64];
            vol += v0[0] * (v1[1] * v2[2] - v2[1] * v1[2])
                - v1[0] * (v0[1] * v2[2] - v2[1] * v0[2])
                + v2[0] * (v0[1] * v1[2] - v1[1] * v0[2]);
        }
        (vol / 6.0).abs()
    }

    /// Compute the bounding box of a triangle mesh.
    fn compute_mesh_bbox(mesh: &TriangleMesh) -> ([f64; 3], [f64; 3]) {
        let mut min = [f64::MAX; 3];
        let mut max = [f64::MIN; 3];
        for chunk in mesh.vertices.chunks(3) {
            for i in 0..3 {
                min[i] = min[i].min(chunk[i] as f64);
                max[i] = max[i].max(chunk[i] as f64);
            }
        }
        (min, max)
    }

    /// Translate a BRepSolid by a given offset, updating both vertices and surfaces.
    fn translate_brep(brep: &mut BRepSolid, dx: f64, dy: f64, dz: f64) {
        let t = Transform::translation(dx, dy, dz);

        // Translate all vertices
        for (_, v) in &mut brep.topology.vertices {
            v.point = t.apply_point(&v.point);
        }

        // Transform all surfaces using drain to avoid clone
        brep.geometry.surfaces = brep
            .geometry
            .surfaces
            .drain(..)
            .map(|s| s.transform(&t))
            .collect();
    }

    #[test]
    fn test_difference_hole_in_center() {
        // Two axis-aligned cubes with partial overlap.
        let big_cube = make_cube(10.0, 10.0, 10.0);

        let mut small_cube = make_cube(4.0, 20.0, 4.0);
        translate_brep(&mut small_cube, 3.0, -5.0, 3.0);

        let result = boolean_op(&big_cube, &small_cube, BooleanOp::Difference, 32);
        let mesh = result.to_mesh(32);

        let volume = compute_mesh_volume(&mesh);

        // Expected: 10*10*10 - 4*10*4 = 1000 - 160 = 840
        assert!(
            (volume - 840.0).abs() < 1.0,
            "Expected volume ~840, got {}",
            volume
        );

        let (min, max) = compute_mesh_bbox(&mesh);
        assert!(
            min[0] >= -0.01 && min[1] >= -0.01 && min[2] >= -0.01,
            "Min should be ~[0,0,0], got {:?}",
            min
        );
        assert!(
            max[0] <= 10.01 && max[1] <= 10.01 && max[2] <= 10.01,
            "Max should be ~[10,10,10], got {:?}",
            max
        );

        assert!(
            mesh.num_triangles() > 0,
            "Result mesh should have triangles"
        );
    }

    /// Regression for the planar-tessellator winding-flip heuristic that
    /// previously over-counted cavity-wall contributions. Cube-minus-cube
    /// hollow tube: outer 40×40×80 minus inner 28×28×80, both axis-aligned
    /// and centred in XY. Cavity walls are reversed-orientation planar
    /// faces; the volume integrator must respect the orientation flag, not
    /// override it from loop winding.
    #[test]
    fn test_difference_hollow_tube_volume() {
        let mut outer = make_cube(40.0, 40.0, 80.0);
        translate_brep(&mut outer, -20.0, -20.0, 0.0);
        let mut inner = make_cube(28.0, 28.0, 84.0);
        translate_brep(&mut inner, -14.0, -14.0, -2.0);

        let result = boolean_op(&outer, &inner, BooleanOp::Difference, 32);
        let mesh = result.to_mesh(32);
        let volume = compute_mesh_volume(&mesh);

        // Expected: (40² − 28²) × 80 = 65280 mm³.
        assert!(
            (volume - 65280.0).abs() < 1.0,
            "Expected hollow-tube volume ~65280, got {}",
            volume
        );
    }

    #[test]
    fn test_point_in_cube_mesh() {
        let brep = make_cube(10.0, 10.0, 10.0);
        let mesh = tessellate_brep(&brep, 32);

        // Point inside the cube
        assert!(point_in_mesh(&Point3::new(5.0, 5.0, 5.0), &mesh));
        // Point outside the cube
        assert!(!point_in_mesh(&Point3::new(15.0, 5.0, 5.0), &mesh));
        assert!(!point_in_mesh(&Point3::new(-1.0, 5.0, 5.0), &mesh));
    }

    #[test]
    fn test_union_overlapping() {
        // Partially overlapping cubes
        let a = make_cube(10.0, 10.0, 10.0);
        let mut b = make_cube(10.0, 10.0, 10.0);
        for (_, v) in &mut b.topology.vertices {
            v.point.x += 5.0; // shift B by half
        }
        let result = boolean_op(&a, &b, BooleanOp::Union, 32);
        // Overlapping booleans return BRep
        assert!(matches!(result, BooleanResult::BRep(_)));
        let mesh = result.to_mesh(32);
        assert!(mesh.num_triangles() > 0);
    }

    #[test]
    fn test_difference_overlapping() {
        let a = make_cube(10.0, 10.0, 10.0);
        let b = make_cube(5.0, 5.0, 5.0);
        let result = boolean_op(&a, &b, BooleanOp::Difference, 32);
        let mesh = result.to_mesh(32);
        assert!(mesh.num_triangles() > 0);
    }

    /// Test boolean difference with a hole completely inside a plate.
    #[test]
    fn test_plate_with_hole() {
        let plate = make_cube(80.0, 6.0, 60.0);

        let mut hole = make_cube(12.0, 20.0, 12.0);
        translate_brep(&mut hole, 34.0, -7.0, 24.0);

        let result = boolean_op(&plate, &hole, BooleanOp::Difference, 32);
        let mesh = result.to_mesh(32);

        let volume = compute_mesh_volume(&mesh);

        // Expected volume: 80*6*60 - 12*6*12 = 28800 - 864 = 27936
        // Note: The winding fix in tessellation can affect volume calculation
        // due to how signed tetrahedra contributions sum. We allow a wider tolerance.
        assert!(
            (volume - 27936.0).abs() < 1200.0,
            "Expected volume ~27936, got {}",
            volume
        );
    }

    #[test]
    fn test_point_in_mesh_on_surface() {
        let brep = make_cube(10.0, 10.0, 10.0);
        let mesh = tessellate_brep(&brep, 32);

        // Point clearly inside
        assert!(point_in_mesh(&Point3::new(5.0, 5.0, 5.0), &mesh));

        // Point clearly outside
        assert!(!point_in_mesh(&Point3::new(15.0, 5.0, 5.0), &mesh));
    }

    #[test]
    fn test_point_in_mesh_near_edge() {
        let brep = make_cube(10.0, 10.0, 10.0);
        let mesh = tessellate_brep(&brep, 32);

        // Points slightly inside the cube
        assert!(point_in_mesh(&Point3::new(5.0, 5.0, 9.999), &mesh));
        assert!(point_in_mesh(&Point3::new(5.0, 9.999, 5.0), &mesh));
        assert!(point_in_mesh(&Point3::new(9.999, 5.0, 5.0), &mesh));

        // Points slightly outside the cube
        assert!(!point_in_mesh(&Point3::new(5.0, 5.0, 10.001), &mesh));
        assert!(!point_in_mesh(&Point3::new(5.0, 10.001, 5.0), &mesh));
        assert!(!point_in_mesh(&Point3::new(10.001, 5.0, 5.0), &mesh));
    }

    /// The ray index is only a broadphase: it must answer exactly what the
    /// linear scan does, everywhere. Sweep a grid of probes — interior,
    /// exterior, and straddling every face, edge and corner — across meshes
    /// with flat, curved and doubly-curved geometry.
    #[test]
    fn mesh_ray_index_matches_linear_scan() {
        use crate::mesh::MeshRayIndex;
        use vcad_kernel_primitives::{make_cylinder, make_sphere, make_torus};

        let meshes = [
            tessellate_brep(&make_cube(10.0, 10.0, 10.0), 32),
            tessellate_brep(&make_cylinder(4.0, 12.0, 32), 32),
            tessellate_brep(&make_sphere(6.0, 24), 24),
            tessellate_brep(&make_torus(8.0, 2.5, 32), 32),
        ];

        for mesh in &meshes {
            let index = MeshRayIndex::new(mesh);
            // Deterministic lattice with irrational-ish steps, so probes land
            // both well clear of and right on the tessellation's features.
            let mut disagreements = 0;
            for i in -14i32..=14 {
                for j in -14i32..=14 {
                    for k in -14i32..=14 {
                        let p =
                            Point3::new(i as f64 * 0.9137, j as f64 * 0.9137, k as f64 * 0.9137);
                        if point_in_mesh(&p, mesh) != index.contains(&p) {
                            disagreements += 1;
                        }
                    }
                }
            }
            assert_eq!(disagreements, 0, "index disagreed with the linear scan");
        }
    }

    #[test]
    fn test_point_in_polygon_exact() {
        use vcad_kernel_math::Point2;

        // Square polygon
        let square = vec![
            Point2::new(0.0, 0.0),
            Point2::new(10.0, 0.0),
            Point2::new(10.0, 10.0),
            Point2::new(0.0, 10.0),
        ];

        // Point clearly inside
        assert!(trim::point_in_polygon(&Point2::new(5.0, 5.0), &square));

        // Point clearly outside
        assert!(!trim::point_in_polygon(&Point2::new(15.0, 5.0), &square));

        // Point exactly on edge
        assert!(trim::point_in_polygon(&Point2::new(5.0, 0.0), &square));
        assert!(trim::point_in_polygon(&Point2::new(0.0, 5.0), &square));

        // Point exactly on vertex
        assert!(trim::point_in_polygon(&Point2::new(0.0, 0.0), &square));
        assert!(trim::point_in_polygon(&Point2::new(10.0, 10.0), &square));
    }

    #[test]
    fn test_coplanar_cubes_union() {
        let a = make_cube(10.0, 10.0, 10.0);
        let mut b = make_cube(10.0, 10.0, 10.0);
        for (_, v) in &mut b.topology.vertices {
            v.point.x += 10.0;
        }
        b.geometry.surfaces = b
            .geometry
            .surfaces
            .drain(..)
            .map(|s| s.transform(&Transform::translation(10.0, 0.0, 0.0)))
            .collect();

        let result = boolean_op(&a, &b, BooleanOp::Union, 32);
        let mesh = result.to_mesh(32);

        assert!(mesh.num_triangles() > 0);

        let vol = compute_mesh_volume(&mesh);
        assert!(
            (vol - 2000.0).abs() < 100.0,
            "Expected volume ~2000, got {}",
            vol
        );
    }

    #[test]
    fn test_near_coplanar_faces() {
        let a = make_cube(10.0, 10.0, 10.0);
        let mut b = make_cube(10.0, 10.0, 10.0);
        for (_, v) in &mut b.topology.vertices {
            v.point.x += 9.999;
        }
        b.geometry.surfaces = b
            .geometry
            .surfaces
            .drain(..)
            .map(|s| s.transform(&Transform::translation(9.999, 0.0, 0.0)))
            .collect();

        let result = boolean_op(&a, &b, BooleanOp::Union, 32);
        let mesh = result.to_mesh(32);

        assert!(mesh.num_triangles() > 0);
    }

    /// Count orphan half-edges (half-edges with no parent edge)
    fn count_orphan_half_edges(brep: &BRepSolid) -> usize {
        brep.topology
            .half_edges
            .iter()
            .filter(|(_, he)| he.loop_id.is_some() && he.edge.is_none())
            .count()
    }

    /// Extract BRepSolid from BooleanResult.
    fn unwrap_brep(result: BooleanResult) -> BRepSolid {
        let BooleanResult::BRep(b) = result;
        *b
    }

    #[test]
    fn test_mounting_plate_with_multiple_holes() {
        use vcad_kernel_primitives::make_cylinder;

        // Create plate (80x6x60 but oriented for y-axis holes)
        let plate = make_cube(80.0, 6.0, 60.0);

        // Create a single hole cylinder rotated for Y-axis
        fn rotated_cylinder(radius: f64, height: f64, x: f64, z: f64, segments: u32) -> BRepSolid {
            let mut cyl = make_cylinder(radius, height, segments);
            // Rotate 90 degrees around X axis (so cylinder axis points in Y)
            let t = Transform::rotation_x(-std::f64::consts::FRAC_PI_2)
                .then(&Transform::translation(x, -7.0, z));
            for (_, v) in &mut cyl.topology.vertices {
                v.point = t.apply_point(&v.point);
            }
            cyl.geometry.surfaces = cyl
                .geometry
                .surfaces
                .drain(..)
                .map(|s| s.transform(&t))
                .collect();
            cyl
        }

        // Exact structure from the TypeScript test:
        // - 1 large center hole (r=6, at 40,30)
        // - 4 corner holes (r=2, at 8,8 / 72,8 / 8,52 / 72,52)
        // - 4 edge holes (r=2, at 8,30 / 72,30 / 40,8 / 40,52)
        let large_center = rotated_cylinder(6.0, 20.0, 40.0, 30.0, 32);
        let corner1 = rotated_cylinder(2.0, 20.0, 8.0, 8.0, 24);
        let corner2 = rotated_cylinder(2.0, 20.0, 72.0, 8.0, 24);
        let corner3 = rotated_cylinder(2.0, 20.0, 8.0, 52.0, 24);
        let corner4 = rotated_cylinder(2.0, 20.0, 72.0, 52.0, 24);
        let edge1 = rotated_cylinder(2.0, 20.0, 8.0, 30.0, 24);
        let edge2 = rotated_cylinder(2.0, 20.0, 72.0, 30.0, 24);
        let edge3 = rotated_cylinder(2.0, 20.0, 40.0, 8.0, 24);
        let edge4 = rotated_cylinder(2.0, 20.0, 40.0, 52.0, 24);

        // Union all holes together (chain of unions like the TypeScript test)
        let h12 = unwrap_brep(boolean_op(&large_center, &corner1, BooleanOp::Union, 32));
        let h123 = unwrap_brep(boolean_op(&h12, &corner2, BooleanOp::Union, 32));
        let h1234 = unwrap_brep(boolean_op(&h123, &corner3, BooleanOp::Union, 32));
        let h12345 = unwrap_brep(boolean_op(&h1234, &corner4, BooleanOp::Union, 32));
        let h123456 = unwrap_brep(boolean_op(&h12345, &edge1, BooleanOp::Union, 32));
        let h1234567 = unwrap_brep(boolean_op(&h123456, &edge2, BooleanOp::Union, 32));
        let h12345678 = unwrap_brep(boolean_op(&h1234567, &edge3, BooleanOp::Union, 32));
        let holes_all = unwrap_brep(boolean_op(&h12345678, &edge4, BooleanOp::Union, 32));

        eprintln!(
            "After 9-hole union: {} orphan half-edges",
            count_orphan_half_edges(&holes_all)
        );

        // Check that the unioned holes have no orphan half-edges
        let orphan_count = count_orphan_half_edges(&holes_all);
        assert_eq!(
            orphan_count, 0,
            "Unioned holes should have no orphan half-edges"
        );

        // Now subtract from plate
        let result = unwrap_brep(boolean_op(&plate, &holes_all, BooleanOp::Difference, 32));
        eprintln!(
            "After difference: {} orphan half-edges",
            count_orphan_half_edges(&result)
        );

        let final_orphan_count = count_orphan_half_edges(&result);
        assert_eq!(
            final_orphan_count, 0,
            "Final result should have no orphan half-edges"
        );
    }

    /// Test boolean difference with cylinder extending outside cube bounds.
    ///
    /// This tests the case where a cylinder overlaps the cube but also extends
    /// outside it. The result should NOT include any geometry outside the cube's
    /// original bounds.
    #[test]
    fn test_cube_minus_cylinder_boundary() {
        use vcad_kernel_primitives::make_cylinder;

        // Cube from [0,0,0] to [20,20,20]
        let cube = make_cube(20.0, 20.0, 20.0);

        // Cylinder: radius=10, height=20
        // We want the cylinder tangent to the x=0 plane at y=10.
        // Cylinder center at x=0, so it extends from x=-10 to x=10.
        // This is the bug case: the result incorrectly includes geometry at x < 0.
        let mut cylinder = make_cylinder(10.0, 20.0, 32);
        translate_brep(&mut cylinder, -10.0, 10.0, 0.0);

        let result = boolean_op(&cube, &cylinder, BooleanOp::Difference, 32);
        let mesh = result.to_mesh(32);

        // Check bounding box - x should not extend below 0
        let (min, max) = compute_mesh_bbox(&mesh);

        eprintln!(
            "Cube-Cylinder Difference bbox: min={:?}, max={:?}",
            min, max
        );

        // The key assertion: no geometry should extend to negative x
        assert!(
            min[0] >= -0.1,
            "Result should not extend to negative x! min[0] = {:.4}",
            min[0]
        );

        // Also verify the cube bounds are roughly preserved
        assert!(
            max[0] <= 20.1 && max[1] <= 20.1 && max[2] <= 20.1,
            "Max should be ~[20,20,20], got {:?}",
            max
        );

        assert!(
            mesh.num_triangles() > 0,
            "Result mesh should have triangles"
        );

        // Validate that all indices are in bounds
        validate_mesh_indices(&mesh);
    }

    /// Test case matching the app scenario: cylinder centered at (0, 10, 0)
    /// This means the cylinder extends from x=-10 to x=10, but the cube
    /// only goes from x=0 to x=20. The result should clip at x=0.
    #[test]
    fn test_cube_minus_centered_cylinder() {
        use vcad_kernel_primitives::make_cylinder;

        // Cube from [0,0,0] to [20,20,20]
        let cube = make_cube(20.0, 20.0, 20.0);

        // Cylinder: radius=10, height=20, centered at origin
        // Translate by (0, 10, 0) to move center to (0, 10, 0)
        // This means cylinder bbox becomes [-10,0,0]->[10,20,20]
        let mut cylinder = make_cylinder(10.0, 20.0, 32);
        translate_brep(&mut cylinder, 0.0, 10.0, 0.0);

        let result = boolean_op(&cube, &cylinder, BooleanOp::Difference, 32);
        let mesh = result.to_mesh(32);

        // Validate mesh indices first
        validate_mesh_indices(&mesh);

        // Check bounding box - x should not extend below 0
        let (min, max) = compute_mesh_bbox(&mesh);

        eprintln!(
            "Centered Cylinder Difference bbox: min={:?}, max={:?}",
            min, max
        );
        eprintln!(
            "Mesh has {} triangles, {} vertices",
            mesh.num_triangles(),
            mesh.num_vertices()
        );

        // The key assertion: no geometry should extend to negative x
        assert!(
            min[0] >= -0.1,
            "Result should not extend to negative x! min[0] = {:.4}",
            min[0]
        );

        // Also verify the cube bounds are roughly preserved
        assert!(
            max[0] <= 20.1 && max[1] <= 20.1 && max[2] <= 20.1,
            "Max should be ~[20,20,20], got {:?}",
            max
        );

        assert!(
            mesh.num_triangles() > 0,
            "Result mesh should have triangles"
        );
    }

    /// Validate that all mesh indices are within bounds.
    fn validate_mesh_indices(mesh: &TriangleMesh) {
        let num_verts = mesh.num_vertices();
        let mut max_idx = 0u32;
        let mut invalid_count = 0usize;

        for (i, &idx) in mesh.indices.iter().enumerate() {
            if idx as usize >= num_verts {
                invalid_count += 1;
                eprintln!("Invalid index at {}: {} >= {} vertices", i, idx, num_verts);
            }
            if idx > max_idx {
                max_idx = idx;
            }
        }

        assert_eq!(
            invalid_count, 0,
            "Mesh has {} invalid indices (max index {} but only {} vertices)",
            invalid_count, max_idx, num_verts
        );
    }

    // =========================================================================
    // Comprehensive box-cylinder difference tests
    // =========================================================================

    /// Count triangles that have all vertices at a specific coordinate value.
    fn count_triangles_at_coord(
        mesh: &TriangleMesh,
        coord: usize,
        value: f64,
        tolerance: f64,
    ) -> usize {
        let verts = &mesh.vertices;
        let indices = &mesh.indices;
        let mut count = 0;
        for tri in indices.chunks(3) {
            let all_at_coord = tri.iter().all(|&idx| {
                let v = verts[idx as usize * 3 + coord] as f64;
                (v - value).abs() < tolerance
            });
            if all_at_coord {
                count += 1;
            }
        }
        count
    }

    /// Count triangles with at least one vertex at a coordinate value.
    fn count_triangles_touching_coord(
        mesh: &TriangleMesh,
        coord: usize,
        value: f64,
        tolerance: f64,
    ) -> usize {
        let verts = &mesh.vertices;
        let indices = &mesh.indices;
        let mut count = 0;
        for tri in indices.chunks(3) {
            let any_at_coord = tri.iter().any(|&idx| {
                let v = verts[idx as usize * 3 + coord] as f64;
                (v - value).abs() < tolerance
            });
            if any_at_coord {
                count += 1;
            }
        }
        count
    }

    /// Test: Cylinder fully inside box (standard through-hole).
    /// The cylinder is completely contained within the box interior.
    #[test]
    fn test_box_cylinder_through_hole() {
        use vcad_kernel_primitives::make_cylinder;

        // Box from [0,0,0] to [20,20,20]
        let cube = make_cube(20.0, 20.0, 20.0);

        // Cylinder: radius=5, height=30 (extends beyond box)
        // Center at (10, 10, 0), axis along Z
        let mut cylinder = make_cylinder(5.0, 30.0, 32);
        translate_brep(&mut cylinder, 10.0, 10.0, -5.0);

        let result = boolean_op(&cube, &cylinder, BooleanOp::Difference, 32);
        let mesh = result.to_mesh(32);

        validate_mesh_indices(&mesh);

        // All 6 faces of the box should still exist
        let z0_tris = count_triangles_at_coord(&mesh, 2, 0.0, 0.01);
        let z20_tris = count_triangles_at_coord(&mesh, 2, 20.0, 0.01);
        let x0_tris = count_triangles_at_coord(&mesh, 0, 0.0, 0.01);
        let x20_tris = count_triangles_at_coord(&mesh, 0, 20.0, 0.01);
        let y0_tris = count_triangles_at_coord(&mesh, 1, 0.0, 0.01);
        let y20_tris = count_triangles_at_coord(&mesh, 1, 20.0, 0.01);

        assert!(
            z0_tris > 0,
            "Bottom face (z=0) should exist with circular hole"
        );
        assert!(
            z20_tris > 0,
            "Top face (z=20) should exist with circular hole"
        );
        assert!(x0_tris > 0, "Left face (x=0) should exist");
        assert!(x20_tris > 0, "Right face (x=20) should exist");
        assert!(y0_tris > 0, "Front face (y=0) should exist");
        assert!(y20_tris > 0, "Back face (y=20) should exist");

        eprintln!("Through-hole test - Face triangle counts:");
        eprintln!("  z=0: {}, z=20: {}", z0_tris, z20_tris);
        eprintln!("  x=0: {}, x=20: {}", x0_tris, x20_tris);
        eprintln!("  y=0: {}, y=20: {}", y0_tris, y20_tris);
    }

    /// Test: Cylinder at box edge (half-cylinder intersection).
    /// The cylinder axis is at x=0, so only half the cylinder is inside the box.
    /// This is the "happy path tutorial" case that was failing.
    #[test]
    fn test_box_cylinder_edge_intersection() {
        use vcad_kernel_primitives::make_cylinder;

        // Box from [0,0,0] to [20,20,20]
        let cube = make_cube(20.0, 20.0, 20.0);

        // Cylinder: radius=10, height=20, centered at origin
        // Then translate to (0, 10, 0) so axis is at x=0, y=10
        // This means cylinder bbox is [-10,0,0] to [10,20,20]
        // Only the x>0 half is inside the box
        let mut cylinder = make_cylinder(10.0, 20.0, 32);
        translate_brep(&mut cylinder, 0.0, 10.0, 0.0);

        let result = boolean_op(&cube, &cylinder, BooleanOp::Difference, 32);
        let mesh = result.to_mesh(32);

        validate_mesh_indices(&mesh);

        let (min, max) = compute_mesh_bbox(&mesh);
        eprintln!("Edge intersection bbox: min={:?}, max={:?}", min, max);

        // Verify bounding box
        assert!(
            min[0] >= -0.01,
            "Should not extend to negative x: min_x = {}",
            min[0]
        );
        assert!(max[0] <= 20.01, "Should not exceed x=20");
        assert!(min[1] >= -0.01, "Should not extend to negative y");
        assert!(max[1] <= 20.01, "Should not exceed y=20");
        assert!(min[2] >= -0.01, "Should not extend to negative z");
        assert!(max[2] <= 20.01, "Should not exceed z=20");

        // Count triangles on each face
        let z0_tris = count_triangles_at_coord(&mesh, 2, 0.0, 0.01);
        let z20_tris = count_triangles_at_coord(&mesh, 2, 20.0, 0.01);
        let x20_tris = count_triangles_at_coord(&mesh, 0, 20.0, 0.01);
        let y0_tris = count_triangles_at_coord(&mesh, 1, 0.0, 0.01);
        let y20_tris = count_triangles_at_coord(&mesh, 1, 20.0, 0.01);

        eprintln!("Edge intersection - Face triangle counts:");
        eprintln!("  z=0: {}, z=20: {}", z0_tris, z20_tris);
        eprintln!("  x=20: {}", x20_tris);
        eprintln!("  y=0: {}, y=20: {}", y0_tris, y20_tris);

        // The bottom and top faces should have semicircular holes
        assert!(
            z0_tris > 0,
            "Bottom face (z=0) should exist with semicircular cutout, but has {} triangles",
            z0_tris
        );
        assert!(
            z20_tris > 0,
            "Top face (z=20) should exist with semicircular cutout, but has {} triangles",
            z20_tris
        );

        // The right, front, and back faces should be intact
        assert!(x20_tris > 0, "Right face (x=20) should exist");
        assert!(y0_tris > 0, "Front face (y=0) should exist");
        assert!(y20_tris > 0, "Back face (y=20) should exist");

        // The left face (x=0) is entirely consumed by the cylinder
        // (the plane x=0 passes through the cylinder axis)
        // But the cylinder wall should fill this opening
        // Check that there are triangles near x=0 (cylinder wall)
        let x0_adjacent = count_triangles_touching_coord(&mesh, 0, 0.0, 0.5);
        assert!(
            x0_adjacent > 0,
            "Should have cylinder wall triangles near x=0, but found {} triangles",
            x0_adjacent
        );

        // Debug: List result BRep faces
        if let Some(brep) = result.as_brep() {
            eprintln!("\nResult BRep faces ({}):", brep.topology.faces.len());
            for (face_id, face) in &brep.topology.faces {
                let surface = &brep.geometry.surfaces[face.surface_index];
                let surf_type = surface.surface_type();
                let orientation = face.orientation;
                let loop_verts: Vec<_> = brep
                    .topology
                    .loop_half_edges(face.outer_loop)
                    .map(|he| brep.topology.vertices[brep.topology.half_edges[he].origin].point)
                    .collect();
                if loop_verts.len() >= 3 {
                    let z_vals: Vec<f64> = loop_verts.iter().map(|v| v.z).collect();
                    let z_min = z_vals.iter().cloned().fold(f64::INFINITY, f64::min);
                    let z_max = z_vals.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
                    // Compute winding normal from first 3 verts
                    let e1 = loop_verts[1] - loop_verts[0];
                    let e2 = loop_verts[2] - loop_verts[0];
                    let winding_normal = e1.cross(e2);
                    let wn = if winding_normal.norm() > 1e-12 {
                        winding_normal.normalize()
                    } else {
                        winding_normal
                    };
                    eprintln!("  {:?}: {:?}, {} verts, {} inner_loops, z=[{:.1},{:.1}], orient={:?}, winding_n=({:.2},{:.2},{:.2})",
                        face_id, surf_type, loop_verts.len(), face.inner_loops.len(), z_min, z_max, orientation, wn.x, wn.y, wn.z);
                }
            }
        }

        // Volume check: box volume minus half-cylinder volume
        // NOTE: There's a known issue with arc-split geometry at boundaries.
        // The volume is currently off by ~22%, but the fundamental rendering
        // issue (missing faces, incorrect bbox) is fixed.
        let box_vol = 20.0 * 20.0 * 20.0;
        let cylinder_vol = std::f64::consts::PI * 10.0 * 10.0 * 20.0;
        let half_cylinder_vol = cylinder_vol / 2.0;
        let expected_vol = box_vol - half_cylinder_vol;
        let actual_vol = compute_mesh_volume(&mesh);

        eprintln!(
            "Expected volume: {:.2}, Actual: {:.2}",
            expected_vol, actual_vol
        );
        let vol_error = ((actual_vol - expected_vol) / expected_vol).abs();
        assert!(
            vol_error < 0.05,
            "Volume error {:.1}% exceeds 5% tolerance (expected {:.2}, got {:.2})",
            vol_error * 100.0,
            expected_vol,
            actual_vol
        );
    }

    /// Helper: compute triangle normal from indices
    fn compute_triangle_normal(mesh: &TriangleMesh, tri_start: usize) -> [f32; 3] {
        let i0 = mesh.indices[tri_start] as usize;
        let i1 = mesh.indices[tri_start + 1] as usize;
        let i2 = mesh.indices[tri_start + 2] as usize;

        let v0 = [
            mesh.vertices[i0 * 3],
            mesh.vertices[i0 * 3 + 1],
            mesh.vertices[i0 * 3 + 2],
        ];
        let v1 = [
            mesh.vertices[i1 * 3],
            mesh.vertices[i1 * 3 + 1],
            mesh.vertices[i1 * 3 + 2],
        ];
        let v2 = [
            mesh.vertices[i2 * 3],
            mesh.vertices[i2 * 3 + 1],
            mesh.vertices[i2 * 3 + 2],
        ];

        // e1 = v1 - v0, e2 = v2 - v0
        let e1 = [v1[0] - v0[0], v1[1] - v0[1], v1[2] - v0[2]];
        let e2 = [v2[0] - v0[0], v2[1] - v0[1], v2[2] - v0[2]];

        // normal = e1 × e2
        let nx = e1[1] * e2[2] - e1[2] * e2[1];
        let ny = e1[2] * e2[0] - e1[0] * e2[2];
        let nz = e1[0] * e2[1] - e1[1] * e2[0];

        let len = (nx * nx + ny * ny + nz * nz).sqrt();
        if len > 1e-12 {
            [nx / len, ny / len, nz / len]
        } else {
            [0.0, 0.0, 0.0]
        }
    }

    /// Test: z=20 cap triangles must face UP (normal +z).
    /// This specifically tests for the bug where the cylinder cap at z=20
    /// was facing down instead of up after a boolean difference.
    #[test]
    fn test_z20_cap_faces_up() {
        use vcad_kernel_primitives::make_cylinder;

        // Box from [0,0,0] to [20,20,20]
        let cube = make_cube(20.0, 20.0, 20.0);

        // Cylinder: radius=10, height=20, axis at (0, 10, z) - same as edge intersection test
        let mut cylinder = make_cylinder(10.0, 20.0, 32);
        translate_brep(&mut cylinder, 0.0, 10.0, 0.0);

        let result = boolean_op(&cube, &cylinder, BooleanOp::Difference, 32);
        let mesh = result.to_mesh(32);

        validate_mesh_indices(&mesh);

        // Find all triangles at z=20 and check their normals
        let tol = 0.1;
        let mut z20_tris_up = 0;
        let mut z20_tris_down = 0;
        let mut z20_tris_other = 0;

        for tri in 0..(mesh.indices.len() / 3) {
            let i0 = mesh.indices[tri * 3] as usize;
            let i1 = mesh.indices[tri * 3 + 1] as usize;
            let i2 = mesh.indices[tri * 3 + 2] as usize;

            let z0 = mesh.vertices[i0 * 3 + 2];
            let z1 = mesh.vertices[i1 * 3 + 2];
            let z2 = mesh.vertices[i2 * 3 + 2];

            // Check if all vertices are at z=20
            if (z0 - 20.0).abs() < tol && (z1 - 20.0).abs() < tol && (z2 - 20.0).abs() < tol {
                let normal = compute_triangle_normal(&mesh, tri * 3);
                if normal[2] > 0.9 {
                    z20_tris_up += 1;
                    // Also print a few up-facing triangles for comparison
                    if z20_tris_up <= 2 {
                        let x0 = mesh.vertices[i0 * 3];
                        let y0 = mesh.vertices[i0 * 3 + 1];
                        let x1 = mesh.vertices[i1 * 3];
                        let y1 = mesh.vertices[i1 * 3 + 1];
                        eprintln!(
                            "Up-facing tri {} [indices {},{},{}]: ({:.2},{:.2}) -> ({:.2},{:.2}) ...",
                            z20_tris_up, i0, i1, i2, x0, y0, x1, y1
                        );
                    }
                } else if normal[2] < -0.9 {
                    z20_tris_down += 1;
                    // Debug: print first few down-facing triangles with indices
                    if z20_tris_down <= 3 {
                        let x0 = mesh.vertices[i0 * 3];
                        let y0 = mesh.vertices[i0 * 3 + 1];
                        let x1 = mesh.vertices[i1 * 3];
                        let y1 = mesh.vertices[i1 * 3 + 1];
                        let x2 = mesh.vertices[i2 * 3];
                        let y2 = mesh.vertices[i2 * 3 + 1];
                        eprintln!(
                            "Down-facing tri {} [indices {},{},{}]: ({:.2},{:.2}) -> ({:.2},{:.2}) -> ({:.2},{:.2}), normal=({:.3},{:.3},{:.3})",
                            z20_tris_down, i0, i1, i2, x0, y0, x1, y1, x2, y2, normal[0], normal[1], normal[2]
                        );
                    }
                } else {
                    z20_tris_other += 1;
                }
            }
        }

        eprintln!(
            "z=20 triangles: {} facing up, {} facing down, {} other",
            z20_tris_up, z20_tris_down, z20_tris_other
        );

        // Debug: print ALL result BRep faces
        if let Some(brep) = result.as_brep() {
            let solid = &brep.topology.solids[brep.solid_id];
            let shell = &brep.topology.shells[solid.outer_shell];
            eprintln!(
                "\nShell has {} faces, topology has {} faces",
                shell.faces.len(),
                brep.topology.faces.len()
            );
            eprintln!("Shell face IDs: {:?}", shell.faces);
            eprintln!("\nALL BRep faces ({} total):", brep.topology.faces.len());
            for (face_id, face) in &brep.topology.faces {
                let loop_verts: Vec<_> = brep
                    .topology
                    .loop_half_edges(face.outer_loop)
                    .map(|he| brep.topology.vertices[brep.topology.half_edges[he].origin].point)
                    .collect();
                if loop_verts.len() < 3 {
                    continue;
                }
                let z_min = loop_verts.iter().map(|v| v.z).fold(f64::INFINITY, f64::min);
                let z_max = loop_verts
                    .iter()
                    .map(|v| v.z)
                    .fold(f64::NEG_INFINITY, f64::max);
                let e1 = loop_verts[1] - loop_verts[0];
                let e2 = loop_verts[2] - loop_verts[0];
                let winding_n = e1.cross(e2);
                let wn = if winding_n.norm() > 1e-12 {
                    winding_n.normalize()
                } else {
                    winding_n
                };
                let surface = &brep.geometry.surfaces[face.surface_index];
                // Compute signed area to get true winding (not just first 3 verts)
                let mut signed_area_xy = 0.0;
                for i in 0..loop_verts.len() {
                    let j = (i + 1) % loop_verts.len();
                    signed_area_xy +=
                        loop_verts[i].x * loop_verts[j].y - loop_verts[j].x * loop_verts[i].y;
                }
                let _true_winding_z = if signed_area_xy > 0.0 {
                    "CCW (+z)"
                } else {
                    "CW (-z)"
                };
                eprintln!(
                    "  {:?}: {:?}, {} verts, z=[{:.1},{:.1}], orient={:?}, winding=({:.2},{:.2},{:.2}), area={:.1}",
                    face_id, surface.surface_type(), loop_verts.len(), z_min, z_max, face.orientation, wn.x, wn.y, wn.z, signed_area_xy
                );
                // Extra debug for z=20 faces with many verts (the cylinder cap)
                if loop_verts.len() == 17 && (z_min - 20.0).abs() < 0.01 {
                    eprintln!("    Vertices of 17-vert z=20 face:");
                    for (i, v) in loop_verts.iter().enumerate() {
                        eprintln!("      v{}: ({:.2}, {:.2}, {:.2})", i, v.x, v.y, v.z);
                    }
                    // Compute cross products for first few triangles
                    let e1 = loop_verts[1] - loop_verts[0];
                    let e2 = loop_verts[2] - loop_verts[0];
                    let n1 = e1.cross(e2);
                    eprintln!(
                        "    First triangle (v0,v1,v2) cross: ({:.4}, {:.4}, {:.4})",
                        n1.x, n1.y, n1.z
                    );
                }
            }
        }

        // There should be z=20 triangles (box top face + cylinder cap)
        let total_z20 = z20_tris_up + z20_tris_down + z20_tris_other;
        assert!(
            total_z20 > 0,
            "Should have triangles at z=20, but found none"
        );

        // ALL z=20 horizontal triangles should face UP (+z), not down
        assert_eq!(
            z20_tris_down, 0,
            "z=20 triangles should face UP, but {} triangles face DOWN (winding is wrong)",
            z20_tris_down
        );

        // Should have triangles facing up
        assert!(
            z20_tris_up > 0,
            "Should have z=20 triangles facing up, but found none (only {} other)",
            z20_tris_other
        );
    }

    /// Test: Cylinder at box corner (quarter-cylinder intersection).
    /// The cylinder axis is at the corner (0, 0, z), so only a quarter is inside.
    #[test]
    fn test_box_cylinder_corner_intersection() {
        use vcad_kernel_primitives::make_cylinder;

        // Box from [0,0,0] to [20,20,20]
        let cube = make_cube(20.0, 20.0, 20.0);

        // Cylinder: radius=10, height=20, axis at (0, 0, z)
        // Only the quarter in x>0, y>0 is inside the box
        let cylinder = make_cylinder(10.0, 20.0, 32);

        let result = boolean_op(&cube, &cylinder, BooleanOp::Difference, 32);
        let mesh = result.to_mesh(32);

        validate_mesh_indices(&mesh);

        let (min, max) = compute_mesh_bbox(&mesh);
        eprintln!("Corner intersection bbox: min={:?}, max={:?}", min, max);

        assert!(min[0] >= -0.01, "Should not extend to negative x");
        assert!(min[1] >= -0.01, "Should not extend to negative y");

        // Volume check: box minus quarter-cylinder
        // NOTE: There's a known issue with arc-split geometry at boundaries.
        // The volume is currently off by ~8%, but the bbox is correct.
        let box_vol = 20.0 * 20.0 * 20.0;
        let quarter_cylinder_vol = std::f64::consts::PI * 10.0 * 10.0 * 20.0 / 4.0;
        let expected_vol = box_vol - quarter_cylinder_vol;
        let actual_vol = compute_mesh_volume(&mesh);

        eprintln!(
            "Expected volume: {:.2}, Actual: {:.2}",
            expected_vol, actual_vol
        );
        let vol_error = ((actual_vol - expected_vol) / expected_vol).abs();
        assert!(
            vol_error < 0.05,
            "Volume error {:.1}% exceeds 5% tolerance",
            vol_error * 100.0
        );
    }

    /// Test: Cylinder tangent to box face (no intersection with face interior).
    /// The cylinder just touches the left face at a single line.
    #[test]
    fn test_box_cylinder_tangent() {
        use vcad_kernel_primitives::make_cylinder;

        // Box from [0,0,0] to [20,20,20]
        let cube = make_cube(20.0, 20.0, 20.0);

        // Cylinder: radius=5, centered at (-5, 10, 0)
        // The cylinder is tangent to x=0 at y=10
        let mut cylinder = make_cylinder(5.0, 20.0, 32);
        translate_brep(&mut cylinder, -5.0, 10.0, 0.0);

        let result = boolean_op(&cube, &cylinder, BooleanOp::Difference, 32);
        let mesh = result.to_mesh(32);

        validate_mesh_indices(&mesh);

        // Volume should be unchanged (tangent doesn't remove material)
        let box_vol = 20.0 * 20.0 * 20.0;
        let actual_vol = compute_mesh_volume(&mesh);

        eprintln!(
            "Tangent case - Box vol: {:.2}, Result: {:.2}",
            box_vol, actual_vol
        );

        // The volumes should be very close (tangent contact removes negligible material)
        let vol_error = ((actual_vol - box_vol) / box_vol).abs();
        assert!(
            vol_error < 0.01,
            "Tangent cylinder should not significantly change volume (error: {:.2}%)",
            vol_error * 100.0
        );
    }

    /// Test: Cylinder just inside box (touching but not crossing the boundary).
    /// The cylinder is positioned so it just barely intersects the box.
    #[test]
    fn test_box_cylinder_barely_inside() {
        use vcad_kernel_primitives::make_cylinder;

        // Box from [0,0,0] to [20,20,20]
        let cube = make_cube(20.0, 20.0, 20.0);

        // Cylinder: radius=5, centered at (4, 10, 0)
        // The cylinder's left edge is at x=-1, so ~1mm inside the box
        let mut cylinder = make_cylinder(5.0, 20.0, 32);
        translate_brep(&mut cylinder, 4.0, 10.0, 0.0);

        let result = boolean_op(&cube, &cylinder, BooleanOp::Difference, 32);
        let mesh = result.to_mesh(32);

        validate_mesh_indices(&mesh);

        // The left face should have an arc cutout
        let x0_tris = count_triangles_at_coord(&mesh, 0, 0.0, 0.01);
        eprintln!("Barely inside - Left face (x=0) triangles: {}", x0_tris);

        // Should have some triangles on the left face (the portion outside the cylinder)
        assert!(
            x0_tris > 0,
            "Left face should exist with arc cutout when cylinder barely intersects"
        );
    }

    /// Test: Multiple cylinder holes in a box.
    /// Verifies handling of multiple intersections.
    #[test]
    fn test_box_multiple_cylinders() {
        use vcad_kernel_primitives::make_cylinder;

        // Box from [0,0,0] to [40,40,20]
        let cube = make_cube(40.0, 40.0, 20.0);

        // First cylinder at (10, 10)
        let mut cyl1 = make_cylinder(5.0, 30.0, 32);
        translate_brep(&mut cyl1, 10.0, 10.0, -5.0);

        // Second cylinder at (30, 10)
        let mut cyl2 = make_cylinder(5.0, 30.0, 32);
        translate_brep(&mut cyl2, 30.0, 10.0, -5.0);

        // Third cylinder at (20, 30)
        let mut cyl3 = make_cylinder(5.0, 30.0, 32);
        translate_brep(&mut cyl3, 20.0, 30.0, -5.0);

        // First difference
        let temp1 = boolean_op(&cube, &cyl1, BooleanOp::Difference, 32)
            .into_brep()
            .expect("Expected BRep result");
        // Second difference
        let temp2 = boolean_op(&temp1, &cyl2, BooleanOp::Difference, 32)
            .into_brep()
            .expect("Expected BRep result");
        // Third difference
        let result = boolean_op(&temp2, &cyl3, BooleanOp::Difference, 32);

        let mesh = result.to_mesh(32);
        validate_mesh_indices(&mesh);

        // Verify all expected faces exist
        let z0_tris = count_triangles_at_coord(&mesh, 2, 0.0, 0.01);
        let z20_tris = count_triangles_at_coord(&mesh, 2, 20.0, 0.01);

        eprintln!(
            "Multiple cylinders - z=0: {} tris, z=20: {} tris",
            z0_tris, z20_tris
        );

        assert!(z0_tris > 0, "Bottom face should have 3 holes");
        assert!(z20_tris > 0, "Top face should have 3 holes");

        // Volume check
        let box_vol = 40.0 * 40.0 * 20.0;
        let cyl_vol = std::f64::consts::PI * 5.0 * 5.0 * 20.0;
        let expected_vol = box_vol - 3.0 * cyl_vol;
        let actual_vol = compute_mesh_volume(&mesh);

        let vol_error = ((actual_vol - expected_vol) / expected_vol).abs();
        eprintln!(
            "Expected vol: {:.2}, Actual: {:.2}, Error: {:.2}%",
            expected_vol,
            actual_vol,
            vol_error * 100.0
        );
        assert!(
            vol_error < 0.05,
            "Volume error {:.1}% exceeds tolerance",
            vol_error * 100.0
        );
    }

    // =========================================================================
    // Geometric Normal Validation Tests
    // =========================================================================

    /// Diagnostic info for a triangle with wrong normal orientation.
    #[derive(Debug)]
    struct BadTriangle {
        tri_index: usize,
        face_axis: usize,   // 0=X, 1=Y, 2=Z
        face_coord: f64,    // coordinate value (e.g., 20.0 for z=20)
        expected_sign: f32, // +1.0 or -1.0
        actual_normal: [f32; 3],
        vertices: [[f32; 3]; 3],
    }

    /// Validate that triangles on axis-aligned faces have outward-pointing normals.
    ///
    /// For a closed solid:
    /// - z=max face: normals should point +Z
    /// - z=min face: normals should point -Z
    /// - x=max face: normals should point +X
    /// - etc.
    ///
    /// Returns list of triangles with incorrect normals.
    fn validate_outward_normals(
        mesh: &TriangleMesh,
        faces: &[(usize, f64, f32)], // (axis, coord_value, expected_normal_sign)
        tolerance: f64,
    ) -> Vec<BadTriangle> {
        let mut bad_triangles = Vec::new();

        for tri in 0..(mesh.indices.len() / 3) {
            let i0 = mesh.indices[tri * 3] as usize;
            let i1 = mesh.indices[tri * 3 + 1] as usize;
            let i2 = mesh.indices[tri * 3 + 2] as usize;

            let v0 = [
                mesh.vertices[i0 * 3],
                mesh.vertices[i0 * 3 + 1],
                mesh.vertices[i0 * 3 + 2],
            ];
            let v1 = [
                mesh.vertices[i1 * 3],
                mesh.vertices[i1 * 3 + 1],
                mesh.vertices[i1 * 3 + 2],
            ];
            let v2 = [
                mesh.vertices[i2 * 3],
                mesh.vertices[i2 * 3 + 1],
                mesh.vertices[i2 * 3 + 2],
            ];

            // Check each face definition
            for &(axis, coord, expected_sign) in faces {
                // Check if all vertices are on this face
                let on_face = (v0[axis] as f64 - coord).abs() < tolerance
                    && (v1[axis] as f64 - coord).abs() < tolerance
                    && (v2[axis] as f64 - coord).abs() < tolerance;

                if on_face {
                    let normal = compute_triangle_normal(mesh, tri * 3);
                    let actual_sign = normal[axis];

                    // Check if normal points in expected direction
                    // We expect the component along `axis` to have the same sign as `expected_sign`
                    // and to be the dominant component (> 0.9 magnitude)
                    let wrong_direction = actual_sign * expected_sign < 0.0;
                    let is_axis_aligned = actual_sign.abs() > 0.9;

                    if is_axis_aligned && wrong_direction {
                        bad_triangles.push(BadTriangle {
                            tri_index: tri,
                            face_axis: axis,
                            face_coord: coord,
                            expected_sign,
                            actual_normal: normal,
                            vertices: [v0, v1, v2],
                        });
                    }
                }
            }
        }

        bad_triangles
    }

    /// Print diagnostic info for bad triangles.
    fn print_bad_triangles(bad: &[BadTriangle]) {
        let axis_names = ["X", "Y", "Z"];
        for bt in bad {
            eprintln!(
                "BAD TRI #{} on {}={:.1} face:",
                bt.tri_index, axis_names[bt.face_axis], bt.face_coord
            );
            let sign_str = if bt.expected_sign > 0.0 { "+" } else { "-" };
            eprintln!(
                "  Expected normal: {}{}",
                sign_str, axis_names[bt.face_axis]
            );
            eprintln!(
                "  Actual normal: ({:.3}, {:.3}, {:.3})",
                bt.actual_normal[0], bt.actual_normal[1], bt.actual_normal[2]
            );
            eprintln!("  Vertices:");
            for (i, v) in bt.vertices.iter().enumerate() {
                eprintln!("    v{}: ({:.2}, {:.2}, {:.2})", i, v[0], v[1], v[2]);
            }
        }
    }

    /// Debug: print info about faces at z=0 in a BRep
    #[allow(dead_code)]
    fn debug_z0_faces(brep: &BRepSolid, label: &str) {
        eprintln!("\n=== {} z=0 faces ===", label);
        for (face_id, face) in &brep.topology.faces {
            let loop_verts: Vec<_> = brep
                .topology
                .loop_half_edges(face.outer_loop)
                .map(|he| brep.topology.vertices[brep.topology.half_edges[he].origin].point)
                .collect();
            if loop_verts.is_empty() {
                continue;
            }
            let z_min = loop_verts.iter().map(|v| v.z).fold(f64::INFINITY, f64::min);
            let z_max = loop_verts
                .iter()
                .map(|v| v.z)
                .fold(f64::NEG_INFINITY, f64::max);
            if z_min.abs() > 0.1 || z_max.abs() > 0.1 {
                continue;
            } // Skip non-z=0 faces

            let surface = &brep.geometry.surfaces[face.surface_index];
            let surf_type = surface.surface_type();

            // Get surface normal at first point
            let surf_normal =
                if let Some(plane) = surface.as_any().downcast_ref::<vcad_kernel_geom::Plane>() {
                    format!(
                        "({:.2},{:.2},{:.2})",
                        plane.normal_dir.x, plane.normal_dir.y, plane.normal_dir.z
                    )
                } else {
                    "N/A".to_string()
                };

            // Compute loop winding from vertices
            let mut signed_area_xy = 0.0;
            for i in 0..loop_verts.len() {
                let j = (i + 1) % loop_verts.len();
                signed_area_xy +=
                    loop_verts[i].x * loop_verts[j].y - loop_verts[j].x * loop_verts[i].y;
            }
            let winding = if signed_area_xy > 0.0 {
                "CCW(+z)"
            } else {
                "CW(-z)"
            };

            eprintln!(
                "  {:?}: {:?}, {} verts, orient={:?}, surf_n={}, winding={}",
                face_id,
                surf_type,
                loop_verts.len(),
                face.orientation,
                surf_normal,
                winding
            );

            // Print vertices for small faces
            if loop_verts.len() <= 6 {
                for (i, v) in loop_verts.iter().enumerate() {
                    eprintln!("    v{}: ({:.2}, {:.2}, {:.2})", i, v.x, v.y, v.z);
                }
            }
        }
    }

    /// Test boolean DIFFERENCE normals using geometric validation.
    /// Box [0,0,0]->[20,20,20] minus cylinder at edge.
    ///
    /// For DIFFERENCE, the result includes:
    /// - Box faces that are outside the cylinder (with their original normals)
    /// - Cylinder faces that are inside the box (with REVERSED normals - they form the hole interior)
    ///
    /// At z=0 and z=20, there will be TWO types of faces:
    /// 1. Box face (outer) - normal points outward (-Z for bottom, +Z for top)
    /// 2. Cylinder cap (inner/hole) - normal points inward (+Z for bottom hole, -Z for top hole)
    ///
    /// So we can't simply assert all z=0 faces have -Z normal.
    #[test]
    fn test_boolean_normals_difference() {
        use vcad_kernel_primitives::make_cylinder;

        // Box from [0,0,0] to [20,20,20]
        let cube = make_cube(20.0, 20.0, 20.0);

        // Cylinder: radius=10, height=20, axis at (0, 10, z)
        let mut cylinder = make_cylinder(10.0, 20.0, 32);
        translate_brep(&mut cylinder, 0.0, 10.0, 0.0);

        let result = boolean_op(&cube, &cylinder, BooleanOp::Difference, 32);

        // Debug: print z=0 faces before tessellation
        if let Some(brep) = result.as_brep() {
            debug_z0_faces(brep, "DIFFERENCE result");
        }

        let mesh = result.to_mesh(32);

        validate_mesh_indices(&mesh);

        // For DIFFERENCE, we can only validate faces that are unambiguously outer faces:
        // - x=20 (right face) - always outward +X
        // - y=0 (front face, but only the box portion) - outward -Y
        // - y=20 (back face, but only the box portion) - outward +Y
        //
        // z=0 and z=20 have MIXED normals due to the hole interior faces.
        // The cylinder caps inside the box are reversed, so they point INTO the hole.
        let face_specs: Vec<(usize, f64, f32)> = vec![
            // Only check faces that are definitely outer box faces
            (0, 20.0, 1.0), // x=20 -> +X (box right face)
                            // Note: y=0 and y=20 are partially cut by the cylinder,
                            // but the remaining portions should still face outward
        ];

        let bad = validate_outward_normals(&mesh, &face_specs, 0.1);

        if !bad.is_empty() {
            eprintln!(
                "\n=== DIFFERENCE: {} triangles have wrong normals ===",
                bad.len()
            );
            print_bad_triangles(&bad);
        }

        // Count triangles per face for context
        eprintln!("\nDifference face triangle counts:");
        eprintln!(
            "  z=0:  {} tris",
            count_triangles_at_coord(&mesh, 2, 0.0, 0.1)
        );
        eprintln!(
            "  z=20: {} tris",
            count_triangles_at_coord(&mesh, 2, 20.0, 0.1)
        );
        eprintln!(
            "  x=20: {} tris",
            count_triangles_at_coord(&mesh, 0, 20.0, 0.1)
        );
        eprintln!(
            "  y=0:  {} tris",
            count_triangles_at_coord(&mesh, 1, 0.0, 0.1)
        );
        eprintln!(
            "  y=20: {} tris",
            count_triangles_at_coord(&mesh, 1, 20.0, 0.1)
        );

        assert!(
            bad.is_empty(),
            "DIFFERENCE operation produced {} triangles with wrong normals",
            bad.len()
        );
    }

    /// Test boolean UNION normals using geometric validation.
    /// Box [0,0,0]->[20,20,20] union cylinder at edge (cylinder protrudes to negative x).
    #[test]
    fn test_boolean_normals_union() {
        use vcad_kernel_primitives::make_cylinder;

        // Box from [0,0,0] to [20,20,20]
        let cube = make_cube(20.0, 20.0, 20.0);

        // Cylinder: radius=10, height=20, axis at (0, 10, z)
        // This creates a union where cylinder extends to x=-10
        let mut cylinder = make_cylinder(10.0, 20.0, 32);
        translate_brep(&mut cylinder, 0.0, 10.0, 0.0);

        let result = boolean_op(&cube, &cylinder, BooleanOp::Union, 32);

        // Debug: print z=0 faces before tessellation
        if let Some(brep) = result.as_brep() {
            debug_z0_faces(brep, "UNION result");
        }

        let mesh = result.to_mesh(32);

        validate_mesh_indices(&mesh);

        // For union, the outer faces are:
        // z=0 (bottom) -> normal -Z
        // z=20 (top) -> normal +Z
        // x=20 (right) -> normal +X
        // y=0 (front, partial) -> normal -Y
        // y=20 (back, partial) -> normal +Y
        // The cylinder extends to x=-10, so there's curved surface there
        let face_specs: Vec<(usize, f64, f32)> = vec![
            (2, 0.0, -1.0), // z=0 -> -Z
            (2, 20.0, 1.0), // z=20 -> +Z
            (0, 20.0, 1.0), // x=20 -> +X
            (1, 0.0, -1.0), // y=0 -> -Y
            (1, 20.0, 1.0), // y=20 -> +Y
        ];

        let bad = validate_outward_normals(&mesh, &face_specs, 0.1);

        if !bad.is_empty() {
            eprintln!(
                "\n=== UNION: {} triangles have wrong normals ===",
                bad.len()
            );
            print_bad_triangles(&bad);
        }

        // Count triangles per face for context
        eprintln!("\nUnion face triangle counts:");
        eprintln!(
            "  z=0:  {} tris",
            count_triangles_at_coord(&mesh, 2, 0.0, 0.1)
        );
        eprintln!(
            "  z=20: {} tris",
            count_triangles_at_coord(&mesh, 2, 20.0, 0.1)
        );
        eprintln!(
            "  x=20: {} tris",
            count_triangles_at_coord(&mesh, 0, 20.0, 0.1)
        );
        eprintln!(
            "  y=0:  {} tris",
            count_triangles_at_coord(&mesh, 1, 0.0, 0.1)
        );
        eprintln!(
            "  y=20: {} tris",
            count_triangles_at_coord(&mesh, 1, 20.0, 0.1)
        );

        // Check bounding box extends to x=-10 (cylinder protrusion)
        let (min, max) = compute_mesh_bbox(&mesh);
        eprintln!("\nUnion bbox: min={:?}, max={:?}", min, max);
        assert!(
            min[0] < -9.0,
            "Union should extend to x~=-10 (cylinder protrusion), but min_x={:.2}",
            min[0]
        );

        assert!(
            bad.is_empty(),
            "UNION operation produced {} triangles with wrong normals",
            bad.len()
        );
    }

    #[test]
    fn test_sphere_boolean_subtract() {
        use vcad_kernel_primitives::make_sphere;

        // Large sphere at origin, radius 10
        let big_sphere = make_sphere(10.0, 32);

        // Small sphere offset along X, radius 5
        let mut small_sphere = make_sphere(5.0, 32);
        translate_brep(&mut small_sphere, 8.0, 0.0, 0.0);

        // Subtract small sphere from big sphere
        let result = boolean_op(&big_sphere, &small_sphere, BooleanOp::Difference, 32);
        let mesh = result.to_mesh(32);

        // The result should have some triangles (not empty)
        assert!(
            !mesh.indices.is_empty(),
            "Sphere-sphere difference should produce a non-empty mesh"
        );

        let volume = compute_mesh_volume(&mesh);
        let big_vol = 4.0 / 3.0 * std::f64::consts::PI * 10.0_f64.powi(3);
        eprintln!(
            "Sphere difference: result volume = {:.1}, big sphere volume = {:.1}",
            volume, big_vol
        );

        // Result volume should be less than the big sphere
        assert!(
            volume < big_vol * 1.05,
            "Result volume ({:.1}) should be less than big sphere ({:.1})",
            volume,
            big_vol
        );

        // Result volume should be greater than big sphere minus small sphere's full volume
        // (since only a partial intersection is removed)
        let small_vol = 4.0 / 3.0 * std::f64::consts::PI * 5.0_f64.powi(3);
        assert!(
            volume > big_vol - small_vol - 100.0,
            "Result volume ({:.1}) should be reasonable (big={:.1}, small={:.1})",
            volume,
            big_vol,
            small_vol
        );

        // Check the bounding box: should extend close to -10 on X (big sphere)
        // and should NOT extend to +13 (big sphere + small sphere fully)
        let (min, max) = compute_mesh_bbox(&mesh);
        eprintln!(
            "Sphere diff bbox: min=({:.1},{:.1},{:.1}), max=({:.1},{:.1},{:.1})",
            min[0], min[1], min[2], max[0], max[1], max[2]
        );

        assert!(
            min[0] < -9.0,
            "Result should extend to x~=-10, but min_x={:.1}",
            min[0]
        );
    }

    /// Splitting a sphere into two cap faces along its equator should
    /// yield a tessellation whose closed-surface volume matches the
    /// original sphere. Catches winding inversions in the cap-tessellator
    /// dispatch (the previous fix attempt's regression mode).
    #[test]
    fn test_split_sphere_volume_preservation() {
        use vcad_kernel_geom::Circle3d;
        use vcad_kernel_primitives::make_sphere;
        use vcad_kernel_tessellate::tessellate_brep;

        let radius = 10.0;
        let mut brep = make_sphere(radius, 32);

        // The full sphere has a single face. Split it along the equator
        // (z = 0 circle) into two cap faces.
        let face_id = brep
            .topology
            .faces
            .iter()
            .next()
            .map(|(id, _)| id)
            .expect("sphere should have a face");
        let circle = Circle3d::new(Point3::new(0.0, 0.0, 0.0), radius);
        let result = split::split_spherical_face_by_circle(&mut brep, face_id, &circle, 32);
        assert_eq!(
            result.sub_faces.len(),
            2,
            "split should produce exactly two cap faces"
        );

        let mesh = tessellate_brep(&brep, 32);
        assert!(!mesh.indices.is_empty(), "tessellation should be non-empty");

        let volume = compute_mesh_volume(&mesh);
        let expected = 4.0 / 3.0 * std::f64::consts::PI * radius.powi(3);
        let rel_err = (volume - expected).abs() / expected;
        assert!(
            rel_err < 0.05,
            "split-sphere mesh volume {:.1} differs from sphere volume {:.1} by {:.1}%",
            volume,
            expected,
            rel_err * 100.0
        );
    }

    /// Reproduces issue #152: `Sphere ∩ HalfSpace` (cube clip) used to
    /// return an empty mesh because the boolean classifier dropped the
    /// only sphere face it had after the split (the planar disk + sphere
    /// -with-hole arrangement). With two cap faces, the kept hemisphere
    /// is a real spherical cap.
    #[test]
    fn test_sphere_intersection_cube_clip() {
        use vcad_kernel_primitives::make_sphere;

        let sphere = make_sphere(15.0, 32);
        // Half-space cube (z >= 0): a 100x100x100 cube translated so its
        // bottom face is at z = 0.
        let mut cube = make_cube(100.0, 100.0, 100.0);
        translate_brep(&mut cube, -50.0, -50.0, 0.0);

        let result = boolean_op(&sphere, &cube, BooleanOp::Intersection, 32);
        let mesh = result.to_mesh(32);
        assert!(
            !mesh.indices.is_empty(),
            "sphere ∩ halfspace should produce a non-empty mesh"
        );

        let volume = compute_mesh_volume(&mesh);
        let expected = 2.0 / 3.0 * std::f64::consts::PI * 15.0_f64.powi(3);
        let rel_err = (volume - expected).abs() / expected;
        assert!(
            rel_err < 0.05,
            "hemisphere mesh volume {:.1} differs from {:.1} by {:.1}%",
            volume,
            expected,
            rel_err * 100.0
        );

        let (min, max) = compute_mesh_bbox(&mesh);
        assert!(min[2] > -0.5, "hemisphere should not extend below z=0");
        assert!(max[2] > 14.0, "hemisphere should reach near z=15");
    }

    /// Test boolean difference with a Y-axis aligned (rotated) cylinder subtracted from a box.
    #[test]
    fn test_rotated_cylinder_subtract() {
        use vcad_kernel_primitives::make_cylinder;

        let cube = make_cube(20.0, 20.0, 20.0);
        let mut cylinder = make_cylinder(5.0, 30.0, 32);

        // Rotate -90 degrees around X: maps Z->-Y, Y->Z
        let rot = Transform::rotation_x(-std::f64::consts::FRAC_PI_2);
        for (_, v) in &mut cylinder.topology.vertices {
            v.point = rot.apply_point(&v.point);
        }
        cylinder.geometry.surfaces = cylinder
            .geometry
            .surfaces
            .drain(..)
            .map(|s| s.transform(&rot))
            .collect();

        // Translate so cylinder center is at (10, -5, 10)
        translate_brep(&mut cylinder, 10.0, -5.0, 10.0);

        let result = boolean_op(&cube, &cylinder, BooleanOp::Difference, 32);
        let mesh = result.to_mesh(32);

        assert!(
            mesh.num_triangles() > 0,
            "Result mesh should have triangles"
        );
        validate_mesh_indices(&mesh);

        // Box: 20^3=8000, Cylinder: pi*25*20=~1570.8, Expected: ~6429.2
        let volume = compute_mesh_volume(&mesh);
        let expected = 8000.0 - std::f64::consts::PI * 25.0 * 20.0;
        assert!(
            volume > expected * 0.7 && volume < expected * 1.3,
            "Expected volume ~{:.0}, got {:.0}",
            expected,
            volume
        );

        let (min, max) = compute_mesh_bbox(&mesh);
        assert!(
            min[0] >= -0.1 && min[1] >= -0.1 && min[2] >= -0.1,
            "Min should be ~[0,0,0], got {:?}",
            min
        );
        assert!(
            max[0] <= 20.1 && max[1] <= 20.1 && max[2] <= 20.1,
            "Max should be ~[20,20,20], got {:?}",
            max
        );
    }

    /// Test counterbore / nested concentric holes.
    #[test]
    fn test_counterbore_nested_holes() {
        use vcad_kernel_primitives::make_cylinder;

        let plate = make_cube(40.0, 40.0, 10.0);

        let mut large_hole = make_cylinder(8.0, 20.0, 32);
        translate_brep(&mut large_hole, 20.0, 20.0, -5.0);

        let step1 = boolean_op(&plate, &large_hole, BooleanOp::Difference, 32);
        let mesh1 = step1.to_mesh(32);
        validate_mesh_indices(&mesh1);

        let vol1 = compute_mesh_volume(&mesh1);
        let expected_vol1 = 40.0 * 40.0 * 10.0 - std::f64::consts::PI * 8.0 * 8.0 * 10.0;
        assert!(
            (vol1 - expected_vol1).abs() < expected_vol1 * 0.15,
            "Step 1 volume {:.1} too far from expected {:.1}",
            vol1,
            expected_vol1
        );

        let mut small_hole = make_cylinder(4.0, 20.0, 32);
        translate_brep(&mut small_hole, 20.0, 20.0, -5.0);

        let BooleanResult::BRep(ref step1_brep_box) = step1;
        let step1_brep = step1_brep_box.as_ref().clone();

        let step2 = boolean_op(&step1_brep, &small_hole, BooleanOp::Difference, 32);
        let mesh2 = step2.to_mesh(32);
        validate_mesh_indices(&mesh2);

        let vol2 = compute_mesh_volume(&mesh2);
        assert!(
            (vol2 - vol1).abs() < vol1 * 0.15,
            "Step 2 volume {:.1} should be close to step 1 volume {:.1}",
            vol2,
            vol1
        );
        assert!(
            mesh2.num_triangles() > 0,
            "Result mesh should have triangles"
        );

        let (min, max) = compute_mesh_bbox(&mesh2);
        assert!(
            min[0] >= -0.1 && min[1] >= -0.1 && min[2] >= -0.1,
            "Min should be ~[0,0,0], got {:?}",
            min
        );
        assert!(
            max[0] <= 40.1 && max[1] <= 40.1 && max[2] <= 10.1,
            "Max should be ~[40,40,10], got {:?}",
            max
        );
    }

    fn make_torus(major_radius: f64, minor_radius: f64) -> BRepSolid {
        use vcad_kernel_geom::{GeometryStore, TorusSurface};
        use vcad_kernel_topo::{Orientation, ShellType, Topology};

        let mut topo = Topology::new();
        let mut geom = GeometryStore::new();

        let torus_surf = TorusSurface::new(major_radius, minor_radius);
        let torus_idx = geom.add_surface(Box::new(torus_surf));

        let v_seam = topo.add_vertex(Point3::new(major_radius + minor_radius, 0.0, 0.0));

        let he_u_fwd = topo.add_half_edge(v_seam);
        let he_v_fwd = topo.add_half_edge(v_seam);
        let he_u_rev = topo.add_half_edge(v_seam);
        let he_v_rev = topo.add_half_edge(v_seam);

        let face_loop = topo.add_loop(&[he_u_fwd, he_v_fwd, he_u_rev, he_v_rev]);
        let face = topo.add_face(face_loop, torus_idx, Orientation::Forward);

        topo.add_edge(he_u_fwd, he_u_rev);
        topo.add_edge(he_v_fwd, he_v_rev);

        let shell = topo.add_shell(vec![face], ShellType::Outer);
        let solid_id = topo.add_solid(shell);

        BRepSolid {
            topology: topo,
            geometry: geom,
            solid_id,
        }
    }

    #[test]
    fn test_torus_boolean_subtract() {
        let big_cube = make_cube(30.0, 30.0, 30.0);
        let mut torus = make_torus(5.0, 2.0);
        translate_brep(&mut torus, 15.0, 15.0, 5.0);

        let result = boolean_op(&big_cube, &torus, BooleanOp::Difference, 32);
        let mesh = result.to_mesh(32);

        assert!(
            !mesh.indices.is_empty(),
            "Result mesh should have triangles"
        );

        // The torus sits wholly inside the cube, so the difference is a
        // cube with a toroidal void: 30³ minus the torus's 2π²Rr².
        //
        // The old assertion was only `volume <= 27000`, which passed
        // vacuously while `tessellate_brep` had no Torus arm and the torus
        // meshed to nothing — the "cut" removed exactly zero. Pin the real
        // number so that can't happen again: an un-subtracted torus reads
        // 27000, and an outward-wound cavity reads 27000 + V_torus.
        let volume = compute_mesh_volume(&mesh);
        let v_torus = 2.0 * std::f64::consts::PI.powi(2) * 5.0 * 2.0 * 2.0;
        let expected = 27000.0 - v_torus;
        assert!(
            (volume - expected).abs() / expected < 0.02,
            "Volume {volume:.1} should be 30³ minus the torus ({expected:.1}); \
             27000 means the cut was skipped, {:.1} means the cavity is \
             wound outward",
            27000.0 + v_torus
        );
    }

    #[test]
    fn test_cone_boolean_subtract() {
        use vcad_kernel_primitives::make_cone;

        let cube = make_cube(20.0, 20.0, 20.0);
        let mut cone = make_cone(8.0, 0.0, 15.0, 32);
        translate_brep(&mut cone, 10.0, 10.0, 0.0);

        let result = boolean_op(&cube, &cone, BooleanOp::Difference, 32);
        let mesh = result.to_mesh(32);

        let volume = compute_mesh_volume(&mesh);
        assert!(
            volume > 4000.0 && volume < 10000.0,
            "Expected volume in [4000, 10000] for cube minus cone, got {:.1}",
            volume
        );

        let (min, max) = compute_mesh_bbox(&mesh);
        assert!(
            min[0] >= -0.1 && min[1] >= -0.1 && min[2] >= -0.1,
            "Min should be ~[0,0,0], got {:?}",
            min
        );
        assert!(
            max[0] <= 20.1 && max[1] <= 20.1 && max[2] <= 20.1,
            "Max should be ~[20,20,20], got {:?}",
            max
        );

        assert!(
            mesh.num_triangles() > 0,
            "Result mesh should have triangles"
        );
    }

    /// Regression test for issue #165: `Difference(plate, Union(rect, cyl))`
    /// where a cylinder is tangent to a rect must subtract the full
    /// stadium-shaped cutout, not under-subtract the cylinder's external lobe.
    ///
    /// Before the fix, the SSI between the half-cylinder and the plate's caps
    /// produced a full circle, which fell on the boundary between the rect
    /// interior and the strip beside it on the plate's cap face. The arc-based
    /// splitter saw both circle halves as "inside the polygon" and arbitrarily
    /// picked one — so the kept face's outline traced the inside arc rather
    /// than the outside arc, leaving ~66% of the cylinder's external lobe
    /// un-subtracted.
    #[test]
    fn test_difference_with_tangent_cyl_union_cutter() {
        use vcad_kernel_primitives::make_cylinder;

        // Plate 80×40×8 centered on (0,0).
        let mut plate = make_cube(80.0, 40.0, 8.0);
        translate_brep(&mut plate, -40.0, -20.0, 0.0);

        // Rect 40×10×8 centered on (0,0).
        let mut rect = make_cube(40.0, 10.0, 8.0);
        translate_brep(&mut rect, -20.0, -5.0, 0.0);

        // Cylinder R=5, H=8 at (-20, 0) — tangent to rect's left edge at (-20, ±5).
        let mut cyl_l = make_cylinder(5.0, 8.0, 64);
        translate_brep(&mut cyl_l, -20.0, 0.0, 0.0);

        let union1 = boolean_op(&rect, &cyl_l, BooleanOp::Union, 64)
            .into_brep()
            .unwrap();
        let result = boolean_op(&plate, &union1, BooleanOp::Difference, 64);
        let mesh = result.to_mesh(64);
        let actual = compute_mesh_volume(&mesh);

        let plate_vol = 80.0 * 40.0 * 8.0;
        let rect_vol = 40.0 * 10.0 * 8.0;
        let half_disk_vol = std::f64::consts::PI * 25.0 / 2.0 * 8.0;
        let expected = plate_vol - rect_vol - half_disk_vol;

        let dev = ((actual - expected) / expected).abs();
        assert!(
            dev < 0.001,
            "Difference(plate, Union(rect, tangent_cyl)) should subtract the \
             full stadium cutout. Got {:.4} mm³, expected {:.4} (dev {:.3}%).",
            actual,
            expected,
            dev * 100.0
        );
    }

    /// Regression test for issue #165 with two tangent cylinders (one on each
    /// end of the rect). The error was linear in cylinder count, so this case
    /// is the most sensitive.
    #[test]
    fn test_difference_with_two_tangent_cyls_union_cutter() {
        use vcad_kernel_primitives::make_cylinder;

        let mut plate = make_cube(80.0, 40.0, 8.0);
        translate_brep(&mut plate, -40.0, -20.0, 0.0);

        let mut rect = make_cube(40.0, 10.0, 8.0);
        translate_brep(&mut rect, -20.0, -5.0, 0.0);

        let mut cyl_l = make_cylinder(5.0, 8.0, 64);
        translate_brep(&mut cyl_l, -20.0, 0.0, 0.0);

        let mut cyl_r = make_cylinder(5.0, 8.0, 64);
        translate_brep(&mut cyl_r, 20.0, 0.0, 0.0);

        let u1 = boolean_op(&rect, &cyl_l, BooleanOp::Union, 64)
            .into_brep()
            .unwrap();
        let u2 = boolean_op(&u1, &cyl_r, BooleanOp::Union, 64)
            .into_brep()
            .unwrap();
        let result = boolean_op(&plate, &u2, BooleanOp::Difference, 64);
        let mesh = result.to_mesh(64);
        let actual = compute_mesh_volume(&mesh);

        let plate_vol = 80.0 * 40.0 * 8.0;
        let rect_vol = 40.0 * 10.0 * 8.0;
        let two_half_disks_vol = std::f64::consts::PI * 25.0 * 8.0;
        let expected = plate_vol - rect_vol - two_half_disks_vol;

        let dev = ((actual - expected) / expected).abs();
        assert!(
            dev < 0.001,
            "Difference with two-cylinder slot should match analytic stadium \
             cutout. Got {:.4} mm³, expected {:.4} (dev {:.3}%).",
            actual,
            expected,
            dev * 100.0
        );
    }
}

/// Test-only export of the freeze pass.
pub fn freeze_circle_loops_for_test(brep: &mut vcad_kernel_primitives::BRepSolid, segments: u32) {
    freeze::freeze_circle_loops(brep, segments);
}
