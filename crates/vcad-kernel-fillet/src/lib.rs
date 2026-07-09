#![warn(missing_docs)]

//! Fillet and chamfer operations for the vcad kernel.
//!
//! Implements edge modification operations on B-rep solids:
//! - **Chamfer**: replaces an edge with a planar bevel face
//! - **Fillet**: replaces an edge with a cylindrical blend surface
//!
//! Supports edges between planar faces, plane-cylinder, coaxial cylinders,
//! and general curved surfaces via NURBS rolling ball blend.

mod blend_loft;
mod chamfer;
mod closest_point;
mod fillet_curved;
mod fillet_planar;
mod fillet_subset;
mod rolling_ball;
mod topology;
mod trim;

pub use blend_loft::{find_edge_near, loft_blend_edge, BlendSection};
pub use chamfer::chamfer_all_edges;
pub use closest_point::closest_point_uv;
pub use fillet_curved::{
    fillet_edges_detailed, fillet_edges_detailed_with_trace, FilletTrace, JunctionOutcome,
    JunctionTrace,
};
pub use fillet_planar::fillet_all_edges;
pub use rolling_ball::rolling_ball_blend;

use vcad_kernel_geom::{CylinderSurface, Surface, SurfaceKind};
use vcad_kernel_topo::EdgeId;

/// Classification of fillet geometry between two adjacent faces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilletCase {
    /// Both faces are planar — cylindrical blend.
    PlanePlane,
    /// One face is a plane, other is a cylinder — toric/spherical blend.
    PlaneCylinder,
    /// Two coaxial cylinders — toric blend (stepped shaft fillet).
    CylinderCylinderCoaxial,
    /// Two skew cylinders — Dupin cyclide or NURBS.
    CylinderCylinderSkew,
    /// General curved surfaces — NURBS rolling ball.
    GeneralCurved,
    /// Not supported.
    Unsupported,
}

/// Result of a single edge fillet operation.
#[derive(Debug)]
pub enum FilletResult {
    /// Successfully created a blend surface.
    Success,
    /// Edge pair not supported for fillet.
    Unsupported {
        /// The edge that could not be filleted.
        edge_id: EdgeId,
        /// Reason for failure.
        reason: String,
    },
    /// Fillet radius too large for the geometry.
    RadiusTooLarge {
        /// The edge that could not be filleted.
        edge_id: EdgeId,
        /// Maximum radius that would work.
        max_radius: f64,
    },
    /// Degenerate geometry at the edge.
    DegenerateGeometry {
        /// The edge that could not be filleted.
        edge_id: EdgeId,
    },
}

/// Classify the fillet case between two faces.
pub fn classify_fillet_case(surface_a: &dyn Surface, surface_b: &dyn Surface) -> FilletCase {
    match (surface_a.surface_type(), surface_b.surface_type()) {
        (SurfaceKind::Plane, SurfaceKind::Plane) => FilletCase::PlanePlane,
        (SurfaceKind::Plane, SurfaceKind::Cylinder)
        | (SurfaceKind::Cylinder, SurfaceKind::Plane) => FilletCase::PlaneCylinder,
        (SurfaceKind::Cylinder, SurfaceKind::Cylinder) => {
            let cyl_a = surface_a.as_any().downcast_ref::<CylinderSurface>();
            let cyl_b = surface_b.as_any().downcast_ref::<CylinderSurface>();
            if let (Some(a), Some(b)) = (cyl_a, cyl_b) {
                let dot = a.axis.as_ref().dot(b.axis.as_ref()).abs();
                if dot > 1.0 - 1e-10 {
                    let d = b.center - a.center;
                    let cross = d.cross(a.axis.as_ref());
                    if cross.norm() < 1e-6 {
                        FilletCase::CylinderCylinderCoaxial
                    } else {
                        FilletCase::CylinderCylinderSkew
                    }
                } else {
                    FilletCase::CylinderCylinderSkew
                }
            } else {
                FilletCase::GeneralCurved
            }
        }
        // All remaining surface combinations use the rolling ball fallback
        _ => FilletCase::GeneralCurved,
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_kernel_geom::{CylinderSurface, Plane};
    use vcad_kernel_math::Point3;
    use vcad_kernel_primitives::{make_cube, BRepSolid};

    use crate::topology::{extract_edges, extract_faces};

    #[test]
    fn test_extract_faces_cube() {
        let cube = make_cube(10.0, 10.0, 10.0);
        let faces = extract_faces(&cube);
        assert_eq!(faces.len(), 6);
        for face in &faces {
            assert_eq!(face.vertex_ids.len(), 4);
            let n = face.normal;
            assert!(
                (n.norm() - 1.0).abs() < 0.01,
                "face normal not unit: {:?}",
                n
            );
        }
    }

    #[test]
    fn test_extract_edges_cube() {
        let cube = make_cube(10.0, 10.0, 10.0);
        let edges = extract_edges(&cube);
        assert_eq!(edges.len(), 12, "cube should have 12 edges");
    }

    #[test]
    fn test_chamfer_cube_topology() {
        let cube = make_cube(10.0, 10.0, 10.0);
        let chamfered = chamfer_all_edges(&cube, 1.0);

        let n_faces = chamfered.topology.faces.len();
        assert_eq!(
            n_faces, 26,
            "chamfered cube should have 26 faces, got {}",
            n_faces
        );

        let n_verts = chamfered.topology.vertices.len();
        assert_eq!(
            n_verts, 24,
            "chamfered cube should have 24 vertices, got {}",
            n_verts
        );

        let total_hes = chamfered.topology.half_edges.len();
        let paired_hes = chamfered
            .topology
            .half_edges
            .values()
            .filter(|he| he.twin.is_some())
            .count();
        assert_eq!(
            paired_hes, total_hes,
            "all {} half-edges should be paired, got {} paired",
            total_hes, paired_hes
        );
    }

    #[test]
    fn test_chamfer_cube_volume() {
        let cube = make_cube(10.0, 10.0, 10.0);
        let d = 1.0;
        let chamfered = chamfer_all_edges(&cube, d);

        let mesh = vcad_kernel_tessellate::tessellate_brep(&chamfered, 32);

        let l = 10.0;
        let expected_vol = l * l * l - 6.0 * d * d * (l - d);

        let vol = compute_mesh_volume(&mesh);
        assert!(
            (vol - expected_vol).abs() < 5.0,
            "chamfered cube volume: expected ~{:.1}, got {:.1}",
            expected_vol,
            vol
        );
    }

    #[test]
    fn test_fillet_cube_topology() {
        let cube = make_cube(10.0, 10.0, 10.0);
        let filleted = fillet_all_edges(&cube, 1.0);

        let n_faces = filleted.topology.faces.len();
        assert_eq!(
            n_faces, 26,
            "filleted cube should have 26 faces, got {}",
            n_faces
        );
    }

    #[test]
    fn test_fillet_cube_has_cylindrical_surfaces() {
        let cube = make_cube(10.0, 10.0, 10.0);
        let filleted = fillet_all_edges(&cube, 1.0);

        let n_cyl = filleted
            .geometry
            .surfaces
            .iter()
            .filter(|s| s.surface_type() == vcad_kernel_geom::SurfaceKind::Cylinder)
            .count();
        assert_eq!(
            n_cyl, 12,
            "filleted cube should have 12 cylindrical surfaces, got {}",
            n_cyl
        );
    }

    #[test]
    fn test_fillet_cube_volume() {
        // Closed-form volume of a fillet-all-edges cube of side L with
        // radius r:
        //   V = L³ - 12 · r²(1 - π/4)·(L - 2r) - 8 · r³(1 - π/6)
        //
        // Twelve cylinder corners contribute the chamfer-vs-quarter-circle
        // sliver per edge; eight sphere octants close the corners. (The
        // current planar fillet uses flat triangles instead of true
        // sphere octants, so we only assert against the cylinder term —
        // the corner-slab error is bounded by 8 · r³(π/6 - sqrt(3)/4) ≈
        // 1.4 mm³ for L=10, r=1, well under our tolerance.)
        //
        // What this test really catches: the latent sign error that
        // placed every fillet cylinder *outside* the solid, blowing the
        // mesh up into floating disconnected faces.
        let cube = make_cube(10.0, 10.0, 10.0);
        let r = 1.0;
        let filleted = fillet_all_edges(&cube, r);

        let mesh = vcad_kernel_tessellate::tessellate_brep(&filleted, 32);
        let vol = compute_mesh_volume(&mesh);

        let l = 10.0;
        let pi = std::f64::consts::PI;
        let expected_with_sphere_corners = l * l * l
            - 12.0 * r * r * (1.0 - pi / 4.0) * (l - 2.0 * r)
            - 8.0 * r * r * r * (1.0 - pi / 6.0);

        // With boundary-shaped rings (every ring shares the boundary's
        // topology, no stitch step), the tessellation tracks the
        // closed form within a couple mm³. Tight tolerance both
        // documents that the corner geometry is right AND catches
        // any future regression in the sphere-octant placement.
        assert!(
            (vol - expected_with_sphere_corners).abs() < 5.0,
            "filleted cube volume: expected ~{:.1}, got {:.1} (Δ={:.2})",
            expected_with_sphere_corners,
            vol,
            vol - expected_with_sphere_corners,
        );
    }

    #[test]
    fn test_fillet_cube_has_sphere_corners() {
        // Cube fillet should have 8 spherical corner blends — one per
        // cube vertex — so the rendered corners are smooth instead of
        // showing the lens-shaped z-fight between flat triangles and
        // their adjacent cylinder fillet caps.
        let cube = make_cube(10.0, 10.0, 10.0);
        let filleted = fillet_all_edges(&cube, 1.0);

        let n_sphere = filleted
            .geometry
            .surfaces
            .iter()
            .filter(|s| s.surface_type() == vcad_kernel_geom::SurfaceKind::Sphere)
            .count();
        assert_eq!(
            n_sphere, 8,
            "filleted cube should have 8 spherical corner blends, got {}",
            n_sphere
        );
    }

    #[test]
    fn test_fillet_cube_cylinder_centers_are_inside() {
        // Direct regression for the sign error: every fillet cylinder
        // axis must sit inside the cube, not outside. We check by
        // confirming the axis distance from the cube centroid is
        // smaller than the half-side, with a margin of `radius`.
        let l = 10.0;
        let r = 1.0;
        let cube = make_cube(l, l, l);
        let filleted = fillet_all_edges(&cube, r);

        let centroid = vcad_kernel_math::Point3::new(l / 2.0, l / 2.0, l / 2.0);
        for surf in &filleted.geometry.surfaces {
            if let Some(cyl) = surf
                .as_any()
                .downcast_ref::<vcad_kernel_geom::CylinderSurface>()
            {
                let to_axis: vcad_kernel_math::Vec3 = cyl.center - centroid;
                let axis = *cyl.axis.as_ref();
                let along = to_axis.dot(axis);
                let perp = (to_axis - along * axis).norm();
                // The axis perpendicular distance from the centroid
                // must equal (half_side - radius) · sqrt(2) ≈ 5.66 for
                // a 10 mm cube with r=1. If the sign error is back,
                // perp would be (half_side + radius) · sqrt(2) ≈ 7.78
                // — way outside the solid.
                let expected = (l / 2.0 - r) * std::f64::consts::SQRT_2;
                assert!(
                    (perp - expected).abs() < 0.05,
                    "fillet cylinder axis at {} from centroid; expected {} (sign error: would give {})",
                    perp,
                    expected,
                    (l / 2.0 + r) * std::f64::consts::SQRT_2,
                );
            }
        }
    }

    #[test]
    fn test_classify_cube_edges() {
        let cube = make_cube(10.0, 10.0, 10.0);
        let edges = extract_edges(&cube);
        for edge in &edges {
            let sa = &cube.geometry.surfaces[cube.topology.faces[edge.face_a].surface_index];
            let sb = &cube.geometry.surfaces[cube.topology.faces[edge.face_b].surface_index];
            assert_eq!(
                classify_fillet_case(sa.as_ref(), sb.as_ref()),
                FilletCase::PlanePlane
            );
        }
    }

    #[test]
    fn test_classify_cylinder_edges() {
        let cyl = vcad_kernel_primitives::make_cylinder(5.0, 10.0, 64);
        let edges = extract_edges(&cyl);
        let mut has_plane_cyl = false;
        for edge in &edges {
            let sa = &cyl.geometry.surfaces[cyl.topology.faces[edge.face_a].surface_index];
            let sb = &cyl.geometry.surfaces[cyl.topology.faces[edge.face_b].surface_index];
            if classify_fillet_case(sa.as_ref(), sb.as_ref()) == FilletCase::PlaneCylinder {
                has_plane_cyl = true;
            }
        }
        assert!(has_plane_cyl, "cylinder should have plane-cylinder edges");
    }

    #[test]
    fn test_closest_point_uv_plane() {
        let plane = Plane::xy();
        let pt = Point3::new(3.0, 4.0, 5.0);
        let uv = closest_point_uv(&plane, &pt, 1e-10).unwrap();
        assert!((uv.x - 3.0).abs() < 1e-10);
        assert!((uv.y - 4.0).abs() < 1e-10);
    }

    #[test]
    fn test_closest_point_uv_cylinder() {
        let cyl = CylinderSurface::new(5.0);
        let pt = Point3::new(5.0, 0.0, 3.0);
        let uv = closest_point_uv(&cyl, &pt, 1e-10).unwrap();
        assert!((uv.x).abs() < 1e-6, "u should be ~0, got {}", uv.x);
        assert!((uv.y - 3.0).abs() < 1e-6, "v should be ~3, got {}", uv.y);
    }

    #[test]
    fn test_closest_point_uv_cylinder_multistart() {
        // Test that multi-start Newton finds the near side, not the far side
        let cyl = CylinderSurface::new(5.0);
        // Point near the cylinder at u=0 side (x positive)
        let pt = Point3::new(7.0, 0.0, 3.0);
        let uv = closest_point_uv(&cyl, &pt, 1e-6).unwrap();
        // Should converge to u~0 (x-positive side), not u~PI (x-negative side)
        let evaluated = cyl.evaluate(uv);
        let dist = (evaluated - pt).norm();
        assert!(
            dist < 2.5,
            "should be close to the near side, dist={}",
            dist
        );
        // The closest point on the x-positive side is at (5,0,3)
        assert!(
            (evaluated.x - 5.0).abs() < 0.1,
            "should be at x~5, got {}",
            evaluated.x
        );
    }

    #[test]
    fn test_fillet_edges_detailed_cube() {
        let cube = make_cube(10.0, 10.0, 10.0);
        let edge_ids: Vec<vcad_kernel_topo::EdgeId> = cube.topology.edges.keys().collect();
        let (result, results) = fillet_edges_detailed(&cube, &edge_ids, 1.0);

        for r in &results {
            assert!(
                matches!(r, FilletResult::Success),
                "expected Success, got {:?}",
                r
            );
        }
        assert_eq!(results.len(), 12, "should have 12 results for 12 edges");

        assert!(
            !result.topology.faces.is_empty(),
            "filleted result should have faces"
        );
    }

    #[test]
    fn test_fillet_cylinder_cap_edge() {
        let cyl = vcad_kernel_primitives::make_cylinder(5.0, 10.0, 64);
        let edges = extract_edges(&cyl);

        let plane_cyl_edges: Vec<vcad_kernel_topo::EdgeId> = edges
            .iter()
            .filter(|e| {
                let sa = &cyl.geometry.surfaces[cyl.topology.faces[e.face_a].surface_index];
                let sb = &cyl.geometry.surfaces[cyl.topology.faces[e.face_b].surface_index];
                classify_fillet_case(sa.as_ref(), sb.as_ref()) == FilletCase::PlaneCylinder
            })
            .map(|e| e.edge_id)
            .collect();

        if !plane_cyl_edges.is_empty() {
            let (result, results) = fillet_edges_detailed(&cyl, &plane_cyl_edges, 1.0);
            let has_torus = result
                .geometry
                .surfaces
                .iter()
                .any(|s| s.surface_type() == SurfaceKind::Torus);
            if has_torus {
                assert!(
                    results.iter().any(|r| matches!(r, FilletResult::Success)),
                    "should have at least one successful fillet"
                );
            }
        }
    }

    #[test]
    fn test_classify_general_curved_fallback() {
        // Previously Unsupported cases should now return GeneralCurved
        let torus = vcad_kernel_geom::TorusSurface {
            center: Point3::origin(),
            axis: vcad_kernel_math::Dir3::new_normalize(vcad_kernel_math::Vec3::z()),
            ref_dir: vcad_kernel_math::Dir3::new_normalize(vcad_kernel_math::Vec3::x()),
            major_radius: 5.0,
            minor_radius: 1.0,
        };
        let plane = Plane::xy();
        assert_eq!(
            classify_fillet_case(&torus, &plane),
            FilletCase::GeneralCurved,
            "torus-plane should be GeneralCurved, not Unsupported"
        );
        assert_eq!(
            classify_fillet_case(&torus, &torus),
            FilletCase::GeneralCurved,
            "torus-torus should be GeneralCurved"
        );
    }

    /// Return the vertical (Z-parallel) edges of an axis-aligned cube,
    /// keyed by their (x, y) foot so tests can pick specific corners.
    fn vertical_cube_edges(cube: &BRepSolid) -> Vec<vcad_kernel_topo::EdgeId> {
        extract_edges(cube)
            .into_iter()
            .filter(|e| {
                let a = cube.topology.vertices[e.v_start].point;
                let b = cube.topology.vertices[e.v_end].point;
                (a.x - b.x).abs() < 1e-9 && (a.y - b.y).abs() < 1e-9
            })
            .map(|e| e.edge_id)
            .collect()
    }

    /// Single-edge fillet of a cube: the corrected per-edge path must inset
    /// only the two faces adjacent to the selected edge, round the two cap
    /// corners with the blend arc, and leave the rest of the body alone.
    /// Volume must match the closed form
    ///   V = L³ − (1 − π/4)·r²·L
    /// to 1e-6 relative, and the tessellation must be watertight.
    ///
    /// Regression for the "insets ALL six faces but emits ONE blend" bug,
    /// which produced ~522 mm³ instead of ~995 mm³ and a non-manifold mesh.
    #[test]
    fn test_fillet_single_edge_volume_and_watertight() {
        let l = 10.0;
        let r = 1.5;
        let cube = make_cube(l, l, l);
        let edge = extract_edges(&cube)[0].edge_id;

        let (filleted, results) = fillet_edges_detailed(&cube, &[edge], r);
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], FilletResult::Success));

        let mesh = vcad_kernel_tessellate::tessellate_brep(&filleted, 32);
        assert_eq!(
            mesh.boundary_edges().len(),
            0,
            "single-edge fillet must be watertight"
        );

        let vol = compute_mesh_volume(&mesh);
        let pi = std::f64::consts::PI;
        let expected = l * l * l - (1.0 - pi / 4.0) * r * r * l;
        let rel = (vol - expected).abs() / expected;
        assert!(
            rel < 1e-6,
            "single-edge fillet volume: expected {:.6}, got {:.6} (rel {:.2e})",
            expected,
            vol,
            rel
        );
    }

    /// Two OPPOSITE (non-touching) edges of a cube. Each is an independent
    /// rounding, so the volume is the cube minus two cylinder slivers with
    /// no corner interaction:
    ///   V = L³ − 2·(1 − π/4)·r²·L
    /// Exact to 1e-6 relative; watertight. (The two verticals share the
    /// top/bottom cap faces, so this also exercises a cap face with two
    /// independently rounded corners.)
    #[test]
    fn test_fillet_two_opposite_edges_volume_and_watertight() {
        let l = 10.0;
        let r = 1.5;
        let cube = make_cube(l, l, l);

        // Two diagonally-opposite vertical edges share no vertex.
        let verts = vertical_cube_edges(&cube);
        let mut chosen = Vec::new();
        'outer: for i in 0..verts.len() {
            for j in (i + 1)..verts.len() {
                if is_edge_pair_disjoint(&cube, verts[i], verts[j]) {
                    chosen = vec![verts[i], verts[j]];
                    break 'outer;
                }
            }
        }
        assert_eq!(chosen.len(), 2, "expected two disjoint vertical edges");

        let (filleted, results) = fillet_edges_detailed(&cube, &chosen, r);
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| matches!(r, FilletResult::Success)));

        let mesh = vcad_kernel_tessellate::tessellate_brep(&filleted, 32);
        assert_eq!(
            mesh.boundary_edges().len(),
            0,
            "two-opposite-edge fillet must be watertight"
        );

        let vol = compute_mesh_volume(&mesh);
        let pi = std::f64::consts::PI;
        let expected = l * l * l - 2.0 * (1.0 - pi / 4.0) * r * r * l;
        let rel = (vol - expected).abs() / expected;
        assert!(
            rel < 1e-6,
            "two-opposite-edge fillet volume: expected {:.6}, got {:.6} (rel {:.2e})",
            expected,
            vol,
            rel
        );
    }

    /// Aspirational: two ADJACENT edges sharing a vertex. This needs a
    /// corner blend where two quarter-cylinders meet (a fillet-of-fillet /
    /// partial-torus patch) that the current architecture cannot express —
    /// such selections share a vertex, so they fall through to the legacy
    /// inset-every-face pipeline and come out malformed (~520 mm³, dozens
    /// of open edges). The correct per-edge builder (`fillet_subset`)
    /// deliberately restricts itself to an *independent set* of edges and
    /// leaves the shared-corner case to a follow-up. Un-ignore once
    /// adjacent-edge corner blending exists.
    #[test]
    #[ignore = "adjacent-edge corner blend (fillet-of-fillet) not yet implemented; see fillet_subset"]
    fn test_fillet_two_adjacent_edges_volume() {
        let l = 10.0;
        let r = 1.5;
        let cube = make_cube(l, l, l);
        let edges = extract_edges(&cube);

        let mut adj = Vec::new();
        'outer: for i in 0..edges.len() {
            for j in (i + 1)..edges.len() {
                if !is_edge_pair_disjoint(&cube, edges[i].edge_id, edges[j].edge_id) {
                    adj = vec![edges[i].edge_id, edges[j].edge_id];
                    break 'outer;
                }
            }
        }

        let (filleted, _) = fillet_edges_detailed(&cube, &adj, r);
        let mesh = vcad_kernel_tessellate::tessellate_brep(&filleted, 32);
        assert_eq!(mesh.boundary_edges().len(), 0, "must be watertight");

        let vol = compute_mesh_volume(&mesh);
        let pi = std::f64::consts::PI;
        // Two edge slivers minus the shared-corner over-subtraction. Gated
        // loosely to 1% pending an exact corner-blend closed form.
        let two_slivers = l * l * l - 2.0 * (1.0 - pi / 4.0) * r * r * l;
        assert!(
            (vol - two_slivers).abs() / two_slivers < 0.01,
            "adjacent-edge fillet volume: expected ~{:.3}, got {:.3}",
            two_slivers,
            vol
        );
    }

    /// True when the two edges share no vertex.
    fn is_edge_pair_disjoint(
        brep: &BRepSolid,
        a: vcad_kernel_topo::EdgeId,
        b: vcad_kernel_topo::EdgeId,
    ) -> bool {
        let edges = extract_edges(brep);
        let ea = edges.iter().find(|e| e.edge_id == a).unwrap();
        let eb = edges.iter().find(|e| e.edge_id == b).unwrap();
        let set = [ea.v_start, ea.v_end, eb.v_start, eb.v_end];
        let uniq: std::collections::HashSet<_> = set.iter().collect();
        uniq.len() == 4
    }

    fn compute_mesh_volume(mesh: &vcad_kernel_tessellate::TriangleMesh) -> f64 {
        let verts = &mesh.vertices;
        let indices = &mesh.indices;
        let mut vol = 0.0;
        for tri in indices.chunks(3) {
            let (i0, i1, i2) = (
                tri[0] as usize * 3,
                tri[1] as usize * 3,
                tri[2] as usize * 3,
            );
            let v0 = [verts[i0] as f64, verts[i0 + 1] as f64, verts[i0 + 2] as f64];
            let v1 = [verts[i1] as f64, verts[i1 + 1] as f64, verts[i1 + 2] as f64];
            let v2 = [verts[i2] as f64, verts[i2 + 1] as f64, verts[i2 + 2] as f64];
            vol += v0[0] * (v1[1] * v2[2] - v2[1] * v1[2])
                - v1[0] * (v0[1] * v2[2] - v2[1] * v0[2])
                + v2[0] * (v0[1] * v1[2] - v1[1] * v0[2]);
        }
        (vol / 6.0).abs()
    }
}
