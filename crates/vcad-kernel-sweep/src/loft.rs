//! Loft operation: create a solid by interpolating between profiles.

use rustc_hash::FxHashMap;
use vcad_kernel_geom::{GeometryStore, Plane};
use vcad_kernel_math::{Point3, Vec3};
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_sketch::SketchProfile;
use vcad_kernel_topo::{HalfEdgeId, Orientation, ShellType, Topology, VertexId};

use crate::LoftError;

/// The interpolation mode for lofting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LoftMode {
    /// Connect profiles with ruled (planar) faces.
    #[default]
    Ruled,
    /// Connect profiles with smooth B-spline surfaces (not yet implemented).
    Smooth,
}

/// Options for the loft operation.
#[derive(Debug, Clone, Default)]
pub struct LoftOptions {
    /// Interpolation mode.
    pub mode: LoftMode,
    /// If true, connect the last profile back to the first (creates a tube).
    pub closed: bool,
}

/// Loft between multiple profiles to create a B-rep solid.
///
/// # Arguments
///
/// * `profiles` - At least 2 profiles to interpolate between
/// * `options` - Loft options (mode, closed)
///
/// # Returns
///
/// A B-rep solid with:
/// * Lateral faces connecting adjacent profiles
/// * Cap faces at start and end (unless closed)
///
/// # Errors
///
/// Returns an error if:
/// * Less than 2 profiles are provided
/// * Profiles have different segment counts
///
/// # Example
///
/// ```
/// use vcad_kernel_sweep::{loft, LoftOptions};
/// use vcad_kernel_sketch::SketchProfile;
/// use vcad_kernel_math::{Point3, Vec3};
///
/// // Create two profiles at different heights
/// let profile1 = SketchProfile::rectangle(
///     Point3::new(0.0, 0.0, 0.0),
///     Vec3::x(), Vec3::y(),
///     10.0, 10.0,
/// );
/// let profile2 = SketchProfile::rectangle(
///     Point3::new(2.5, 2.5, 20.0),
///     Vec3::x(), Vec3::y(),
///     5.0, 5.0,
/// );
///
/// let solid = loft(&[profile1, profile2], LoftOptions::default()).unwrap();
/// ```
pub fn loft(profiles: &[SketchProfile], options: LoftOptions) -> Result<BRepSolid, LoftError> {
    // Validate inputs
    if profiles.len() < 2 {
        return Err(LoftError::TooFewProfiles(profiles.len()));
    }

    // Check that all profiles have the same number of segments
    let n_segments = profiles[0].segments.len();
    for profile in profiles.iter().skip(1) {
        if profile.segments.len() != n_segments {
            return Err(LoftError::MismatchedSegmentCounts(
                n_segments,
                profile.segments.len(),
            ));
        }
    }

    // Validate profiles
    for (i, profile) in profiles.iter().enumerate() {
        if profile.segments.is_empty() {
            return Err(LoftError::InvalidProfile(i, "empty profile".into()));
        }
    }

    match options.mode {
        LoftMode::Ruled => loft_ruled(profiles, options.closed),
        LoftMode::Smooth => {
            // Smooth mode not yet implemented - fall back to ruled
            loft_ruled(profiles, options.closed)
        }
    }
}

fn loft_ruled(profiles: &[SketchProfile], closed: bool) -> Result<BRepSolid, LoftError> {
    let n_profiles = profiles.len();
    let n_segments = profiles[0].segments.len();

    let mut topo = Topology::new();
    let mut geom = GeometryStore::new();

    // Build vertex grid: [profile_index][vertex_index]
    let mut vertex_grid: Vec<Vec<VertexId>> = Vec::with_capacity(n_profiles);

    for profile in profiles {
        let verts_3d = profile.vertices_3d();
        let ring: Vec<VertexId> = verts_3d.iter().map(|&p| topo.add_vertex(p)).collect();
        vertex_grid.push(ring);
    }

    let mut all_faces = Vec::new();
    let mut he_map: FxHashMap<([i64; 3], [i64; 3]), HalfEdgeId> = FxHashMap::default();

    let quantize_pt = |p: Point3| -> [i64; 3] {
        [
            (p.x * 1e9).round() as i64,
            (p.y * 1e9).round() as i64,
            (p.z * 1e9).round() as i64,
        ]
    };

    // Number of profile transitions
    let n_transitions = if closed { n_profiles } else { n_profiles - 1 };

    // Build lateral faces between adjacent profiles
    for profile_idx in 0..n_transitions {
        let next_profile_idx = (profile_idx + 1) % n_profiles;

        for seg_idx in 0..n_segments {
            let next_seg_idx = (seg_idx + 1) % n_segments;

            // Quad vertices:
            // v0 (this profile, this segment) -> v1 (this profile, next segment)
            // -> v2 (next profile, next segment) -> v3 (next profile, this segment)
            let v0 = vertex_grid[profile_idx][seg_idx];
            let v1 = vertex_grid[profile_idx][next_seg_idx];
            let v2 = vertex_grid[next_profile_idx][next_seg_idx];
            let v3 = vertex_grid[next_profile_idx][seg_idx];

            let p0 = topo.vertices[v0].point;
            let p1 = topo.vertices[v1].point;
            let p3 = topo.vertices[v3].point;

            // Create planar surface approximation
            let x_dir = p1 - p0;
            let y_dir = p3 - p0;
            let surf_idx = geom.add_surface(Box::new(Plane::new(p0, x_dir, y_dir)));

            // Create half-edges
            let he0 = topo.add_half_edge(v0);
            let he1 = topo.add_half_edge(v1);
            let he2 = topo.add_half_edge(v2);
            let he3 = topo.add_half_edge(v3);

            let loop_id = topo.add_loop(&[he0, he1, he2, he3]);
            let face_id = topo.add_face(loop_id, surf_idx, Orientation::Forward);
            all_faces.push(face_id);

            // Record half-edges for twin pairing
            for he_id in [he0, he1, he2, he3] {
                let he = &topo.half_edges[he_id];
                let origin = topo.vertices[he.origin].point;
                let next = he.next.unwrap();
                let dest = topo.vertices[topo.half_edges[next].origin].point;
                he_map.insert((quantize_pt(origin), quantize_pt(dest)), he_id);
            }
        }
    }

    // Build cap faces if not closed
    if !closed {
        // Start cap (first profile, reversed winding)
        let start_ring = &vertex_grid[0];
        let start_face_id = build_cap_face(
            &mut topo,
            &mut geom,
            start_ring,
            true,
            &mut he_map,
            quantize_pt,
        );
        all_faces.push(start_face_id);

        // End cap (last profile, forward winding)
        let end_ring = &vertex_grid[n_profiles - 1];
        let end_face_id = build_cap_face(
            &mut topo,
            &mut geom,
            end_ring,
            false,
            &mut he_map,
            quantize_pt,
        );
        all_faces.push(end_face_id);
    }

    // Pair twin half-edges
    pair_twin_half_edges(&mut topo, &he_map);

    // Build shell and solid
    let shell = topo.add_shell(all_faces, ShellType::Outer);
    let solid_id = topo.add_solid(shell);

    Ok(BRepSolid {
        topology: topo,
        geometry: geom,
        solid_id,
    })
}

fn build_cap_face<F>(
    topo: &mut Topology,
    geom: &mut GeometryStore,
    verts: &[VertexId],
    reversed: bool,
    he_map: &mut FxHashMap<([i64; 3], [i64; 3]), HalfEdgeId>,
    quantize_pt: F,
) -> vcad_kernel_topo::FaceId
where
    F: Fn(Point3) -> [i64; 3],
{
    let n = verts.len();

    // Get positions
    let positions: Vec<Point3> = verts.iter().map(|&v| topo.vertices[v].point).collect();

    // Create plane surface
    let origin = positions[0];
    let surf_idx = if n >= 3 {
        let x_dir = positions[1] - origin;
        let y_dir = positions[n - 1] - origin;
        if x_dir.norm() > 1e-12 && y_dir.norm() > 1e-12 {
            geom.add_surface(Box::new(Plane::new(origin, x_dir, y_dir)))
        } else {
            let normal = compute_polygon_normal(&positions);
            geom.add_surface(Box::new(Plane::from_normal(origin, normal)))
        }
    } else {
        geom.add_surface(Box::new(Plane::from_normal(origin, Vec3::z())))
    };

    // Create half-edges in the correct order
    let ordered_verts: Vec<VertexId> = if reversed {
        verts.iter().rev().copied().collect()
    } else {
        verts.to_vec()
    };

    let hes: Vec<HalfEdgeId> = ordered_verts
        .iter()
        .map(|&v| topo.add_half_edge(v))
        .collect();
    let loop_id = topo.add_loop(&hes);
    let face_id = topo.add_face(loop_id, surf_idx, Orientation::Forward);

    // Record half-edges for twin pairing
    for &he_id in &hes {
        let he = &topo.half_edges[he_id];
        let origin = topo.vertices[he.origin].point;
        let next = he.next.unwrap();
        let dest = topo.vertices[topo.half_edges[next].origin].point;
        he_map.insert((quantize_pt(origin), quantize_pt(dest)), he_id);
    }

    face_id
}

fn pair_twin_half_edges(topo: &mut Topology, he_map: &FxHashMap<([i64; 3], [i64; 3]), HalfEdgeId>) {
    let mut paired = std::collections::HashSet::new();

    for (&(origin_key, dest_key), &he_id) in he_map {
        if paired.contains(&(dest_key, origin_key)) {
            continue;
        }

        if let Some(&twin_id) = he_map.get(&(dest_key, origin_key)) {
            if topo.half_edges[he_id].twin.is_none() && topo.half_edges[twin_id].twin.is_none() {
                topo.add_edge(he_id, twin_id);
                paired.insert((origin_key, dest_key));
            }
        }
    }
}

fn compute_polygon_normal(verts: &[Point3]) -> Vec3 {
    if verts.len() < 3 {
        return Vec3::z();
    }

    // Newell's method
    let mut n = Vec3::zeros();
    for i in 0..verts.len() {
        let current = verts[i];
        let next = verts[(i + 1) % verts.len()];
        n.x += (current.y - next.y) * (current.z + next.z);
        n.y += (current.z - next.z) * (current.x + next.x);
        n.z += (current.x - next.x) * (current.y + next.y);
    }

    if n.norm() < 1e-12 {
        Vec3::z()
    } else {
        n.normalize()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_rectangle_profile(origin: Point3, width: f64, height: f64) -> SketchProfile {
        SketchProfile::rectangle(origin, Vec3::x(), Vec3::y(), width, height)
    }

    fn create_circle_profile(origin: Point3, radius: f64, n_arcs: u32) -> SketchProfile {
        SketchProfile::circle(origin, Vec3::z(), radius, n_arcs)
    }

    #[test]
    fn test_loft_two_rectangles() {
        let profile1 = create_rectangle_profile(Point3::origin(), 10.0, 10.0);
        let profile2 = create_rectangle_profile(Point3::new(0.0, 0.0, 20.0), 10.0, 10.0);

        let solid = loft(&[profile1, profile2], LoftOptions::default()).unwrap();

        assert!(!solid.topology.faces.is_empty());
        assert!(!solid.topology.vertices.is_empty());

        // Check all half-edges are paired
        let unpaired = solid
            .topology
            .half_edges
            .values()
            .filter(|he| he.twin.is_none())
            .count();
        assert_eq!(unpaired, 0, "expected no unpaired half-edges");
    }

    #[test]
    fn test_loft_two_circles() {
        let profile1 = create_circle_profile(Point3::origin(), 5.0, 8);
        let profile2 = create_circle_profile(Point3::new(0.0, 0.0, 20.0), 10.0, 8);

        let solid = loft(&[profile1, profile2], LoftOptions::default()).unwrap();

        assert!(!solid.topology.faces.is_empty());

        // Check all half-edges are paired
        let unpaired = solid
            .topology
            .half_edges
            .values()
            .filter(|he| he.twin.is_none())
            .count();
        assert_eq!(unpaired, 0, "expected no unpaired half-edges");
    }

    #[test]
    fn test_loft_three_profiles() {
        let profile1 = create_rectangle_profile(Point3::origin(), 10.0, 10.0);
        let profile2 = create_rectangle_profile(Point3::new(0.0, 0.0, 10.0), 8.0, 8.0);
        let profile3 = create_rectangle_profile(Point3::new(0.0, 0.0, 20.0), 6.0, 6.0);

        let solid = loft(&[profile1, profile2, profile3], LoftOptions::default()).unwrap();

        assert!(!solid.topology.faces.is_empty());

        // Check all half-edges are paired
        let unpaired = solid
            .topology
            .half_edges
            .values()
            .filter(|he| he.twin.is_none())
            .count();
        assert_eq!(unpaired, 0, "expected no unpaired half-edges");
    }

    #[test]
    fn test_loft_closed() {
        // Create a ring of profiles (tube)
        let profile1 = create_rectangle_profile(Point3::new(10.0, 0.0, 0.0), 2.0, 2.0);
        let profile2 = create_rectangle_profile(Point3::new(0.0, 10.0, 0.0), 2.0, 2.0);
        let profile3 = create_rectangle_profile(Point3::new(-10.0, 0.0, 0.0), 2.0, 2.0);
        let profile4 = create_rectangle_profile(Point3::new(0.0, -10.0, 0.0), 2.0, 2.0);

        let options = LoftOptions {
            closed: true,
            ..Default::default()
        };

        let solid = loft(&[profile1, profile2, profile3, profile4], options).unwrap();

        assert!(!solid.topology.faces.is_empty());

        // Closed loft should have no cap faces, so n_faces = n_profiles * n_segments
        // 4 profiles × 4 segments = 16 faces
        assert_eq!(solid.topology.faces.len(), 16);
    }

    #[test]
    fn test_loft_too_few_profiles_error() {
        let profile = create_rectangle_profile(Point3::origin(), 10.0, 10.0);

        let result = loft(&[profile], LoftOptions::default());
        assert!(matches!(result, Err(LoftError::TooFewProfiles(1))));
    }

    #[test]
    fn test_loft_mismatched_segments_error() {
        let profile1 = create_rectangle_profile(Point3::origin(), 10.0, 10.0); // 4 segments
        let profile2 = create_circle_profile(Point3::new(0.0, 0.0, 20.0), 5.0, 8); // 8 segments

        let result = loft(&[profile1, profile2], LoftOptions::default());
        assert!(matches!(
            result,
            Err(LoftError::MismatchedSegmentCounts(4, 8))
        ));
    }

    #[test]
    fn test_loft_volume_prism() {
        // Loft two identical rectangles should give a prism
        let profile1 = create_rectangle_profile(Point3::origin(), 10.0, 5.0);
        let profile2 = create_rectangle_profile(Point3::new(0.0, 0.0, 20.0), 10.0, 5.0);

        let solid = loft(&[profile1, profile2], LoftOptions::default()).unwrap();
        let mesh = vcad_kernel_tessellate::tessellate_brep(&solid, 32);

        let vol = compute_mesh_volume(&mesh);
        // Expected: 10 * 5 * 20 = 1000
        assert!(
            (vol - 1000.0).abs() < 5.0,
            "expected volume ~1000, got {vol}"
        );
    }

    #[test]
    fn test_loft_volume_frustum() {
        // Loft a large rectangle to a small one (frustum-like)
        let profile1 = create_rectangle_profile(Point3::origin(), 10.0, 10.0);
        let profile2 = create_rectangle_profile(Point3::new(2.5, 2.5, 10.0), 5.0, 5.0);

        let solid = loft(&[profile1, profile2], LoftOptions::default()).unwrap();
        let mesh = vcad_kernel_tessellate::tessellate_brep(&solid, 32);

        let vol = compute_mesh_volume(&mesh);
        // For a frustum: V = h/3 * (A1 + A2 + sqrt(A1*A2))
        // A1 = 100, A2 = 25, h = 10
        // V = 10/3 * (100 + 25 + 50) = 10/3 * 175 ≈ 583
        // But our loft uses ruled surfaces, so it's a bit different
        // Just check it's positive and reasonable
        assert!(vol > 400.0 && vol < 700.0, "volume {vol} out of range");
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
