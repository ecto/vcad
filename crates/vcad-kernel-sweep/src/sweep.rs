//! Sweep operation: create a solid by moving a profile along a path.

use std::f64::consts::PI;

use rustc_hash::FxHashMap;
use vcad_kernel_geom::{BilinearSurface, Curve3d, CurveKind, GeometryStore, Plane};
use vcad_kernel_math::{Dir3, Point3, Vec3};
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_sketch::SketchProfile;
use vcad_kernel_topo::{HalfEdgeId, Orientation, ShellType, Topology, VertexId};

use crate::frenet::rotation_minimizing_frames;
use crate::SweepError;

/// Options for the sweep operation.
#[derive(Debug, Clone)]
pub struct SweepOptions {
    /// Total twist angle along the path (in radians). Default: 0.0
    pub twist_angle: f64,
    /// Number of segments along the path. 0 = auto (default 32).
    pub path_segments: u32,
    /// Scale factor at the start of the path. Default: 1.0
    pub scale_start: f64,
    /// Scale factor at the end of the path. Default: 1.0
    pub scale_end: f64,
    /// Number of line segments per arc in the profile. Default: 8.
    pub arc_segments: u32,
    /// Initial profile rotation around the path tangent (radians). Default: 0.0
    pub orientation_angle: f64,
}

impl Default for SweepOptions {
    fn default() -> Self {
        Self {
            twist_angle: 0.0,
            path_segments: 0,
            scale_start: 1.0,
            scale_end: 1.0,
            arc_segments: 8,
            orientation_angle: 0.0,
        }
    }
}

/// Sweep a closed profile along a path curve to create a B-rep solid.
///
/// # Arguments
///
/// * `profile` - The closed 2D profile to sweep
/// * `path` - The 3D path curve to sweep along
/// * `options` - Sweep options (twist, scaling, segments)
///
/// # Returns
///
/// A B-rep solid with:
/// * N lateral faces (one per profile segment × path segment)
/// * 2 cap faces (start and end)
///
/// # Errors
///
/// Returns an error if the path has zero length or the profile is invalid.
pub fn sweep(
    profile: &SketchProfile,
    path: &dyn Curve3d,
    options: SweepOptions,
) -> Result<BRepSolid, SweepError> {
    // Validate inputs
    let path_len = estimate_path_length(path);
    if path_len < 1e-12 {
        return Err(SweepError::ZeroLengthPath);
    }

    if profile.segments.is_empty() {
        return Err(SweepError::InvalidProfile("empty profile".into()));
    }

    let n_path_segments = if options.path_segments > 0 {
        options.path_segments as usize
    } else {
        path.suggested_segments() // auto-calculate based on curve
    };

    if n_path_segments < 2 {
        return Err(SweepError::TooFewSegments);
    }

    // Tessellate arcs in the profile for smooth curves
    let arc_segments = options.arc_segments.max(1) as usize;
    let tessellated_profile = profile.tessellate(arc_segments);
    let n_profile_verts = tessellated_profile.segments.len();
    let n_path_samples = n_path_segments + 1; // number of profile copies

    // Compute rotation-minimizing frames along the path
    let mut frames = rotation_minimizing_frames(path, n_path_samples);
    if frames.len() < 2 {
        return Err(SweepError::ZeroLengthPath);
    }

    // Apply initial orientation to all frames (rotates profile around path tangent)
    if options.orientation_angle.abs() > 1e-12 {
        for frame in &mut frames {
            *frame = frame.with_twist(options.orientation_angle);
        }
    }

    // Get profile vertices in 2D (from tessellated profile)
    let profile_verts_2d = tessellated_profile.vertices_2d();

    let quantize_pt = |p: Point3| -> [i64; 3] {
        [
            (p.x * 1e9).round() as i64,
            (p.y * 1e9).round() as i64,
            (p.z * 1e9).round() as i64,
        ]
    };

    let mut topo = Topology::new();
    let mut geom = GeometryStore::new();

    // Build vertex grid with parallel point/key caches to avoid slotmap lookups
    // in the hot lateral-face loop.
    let mut vertex_grid: Vec<Vec<VertexId>> = Vec::with_capacity(n_path_samples);
    let mut point_grid: Vec<Vec<Point3>> = Vec::with_capacity(n_path_samples);
    let mut key_grid: Vec<Vec<[i64; 3]>> = Vec::with_capacity(n_path_samples);

    let has_twist = options.twist_angle.abs() > 1e-12;
    let has_scale_variation = (options.scale_end - options.scale_start).abs() > 1e-12;

    for (path_idx, frame) in frames.iter().enumerate() {
        let t = path_idx as f64 / (n_path_samples - 1) as f64;

        // Compute twist and scale at this position
        let scale = if has_scale_variation {
            options.scale_start + t * (options.scale_end - options.scale_start)
        } else {
            options.scale_start
        };

        // Avoid clone + sin/cos when twist is zero
        let mut ring_verts = Vec::with_capacity(n_profile_verts);
        let mut ring_points = Vec::with_capacity(n_profile_verts);
        let mut ring_keys = Vec::with_capacity(n_profile_verts);
        if has_twist {
            let twisted_frame = frame.with_twist(options.twist_angle * t);
            for p2d in &profile_verts_2d {
                let p3d = twisted_frame.transform_point_scaled(*p2d, scale);
                let v_id = topo.add_vertex(p3d);
                ring_keys.push(quantize_pt(p3d));
                ring_points.push(p3d);
                ring_verts.push(v_id);
            }
        } else {
            for p2d in &profile_verts_2d {
                let p3d = frame.transform_point_scaled(*p2d, scale);
                let v_id = topo.add_vertex(p3d);
                ring_keys.push(quantize_pt(p3d));
                ring_points.push(p3d);
                ring_verts.push(v_id);
            }
        }
        vertex_grid.push(ring_verts);
        point_grid.push(ring_points);
        key_grid.push(ring_keys);
    }

    // Build faces — pre-allocate with exact capacity
    let n_lateral = n_path_segments * n_profile_verts;
    let mut all_faces = Vec::with_capacity(n_lateral + 2);
    let mut he_map: FxHashMap<([i64; 3], [i64; 3]), HalfEdgeId> =
        FxHashMap::with_capacity_and_hasher(n_lateral * 4 + n_profile_verts * 2, Default::default());

    // Radial normal helper (hoisted out of hot loop)
    let radial_normal = |pt: Point3, c: Point3| -> Dir3 {
        let d = pt - c;
        if d.norm() < 1e-12 {
            Dir3::new_normalize(Vec3::z())
        } else {
            Dir3::new_normalize(d)
        }
    };

    // Build lateral faces (one quad per profile edge × path segment)
    for path_idx in 0..n_path_segments {
        // Cache frame centers for the entire profile ring (only depends on path_idx)
        let center0 = frames[path_idx].position;
        let center1 = frames[path_idx + 1].position;

        for profile_idx in 0..n_profile_verts {
            let next_profile_idx = (profile_idx + 1) % n_profile_verts;

            // Quad vertices (winding for outward normal):
            // v0 (this ring, this profile) -> v1 (this ring, next profile)
            // -> v2 (next ring, next profile) -> v3 (next ring, this profile)
            let v0 = vertex_grid[path_idx][profile_idx];
            let v1 = vertex_grid[path_idx][next_profile_idx];
            let v2 = vertex_grid[path_idx + 1][next_profile_idx];
            let v3 = vertex_grid[path_idx + 1][profile_idx];

            // Use cached points instead of slotmap indirection
            let p0 = point_grid[path_idx][profile_idx];
            let p1 = point_grid[path_idx][next_profile_idx];
            let p2 = point_grid[path_idx + 1][next_profile_idx];
            let p3 = point_grid[path_idx + 1][profile_idx];

            // Use pre-computed quantized keys
            let k0 = key_grid[path_idx][profile_idx];
            let k1 = key_grid[path_idx][next_profile_idx];
            let k2 = key_grid[path_idx + 1][next_profile_idx];
            let k3 = key_grid[path_idx + 1][profile_idx];

            // Compute radial normals from path center to each vertex for smooth shading
            let n0 = radial_normal(p0, center0);
            let n1 = radial_normal(p1, center0);
            let n2 = radial_normal(p2, center1);
            let n3 = radial_normal(p3, center1);

            // BilinearSurface with corner normals: v0=p00, v1=p10, v2=p11, v3=p01
            let bilinear = BilinearSurface::with_normals(p0, p1, p3, p2, n0, n1, n3, n2);
            let surf_idx = if bilinear.is_planar() {
                geom.add_surface(Box::new(Plane::new(p0, p1 - p0, p3 - p0)))
            } else {
                geom.add_surface(Box::new(bilinear))
            };

            // Create half-edges
            let he0 = topo.add_half_edge(v0);
            let he1 = topo.add_half_edge(v1);
            let he2 = topo.add_half_edge(v2);
            let he3 = topo.add_half_edge(v3);

            let loop_id = topo.add_loop(&[he0, he1, he2, he3]);
            let face_id = topo.add_face(loop_id, surf_idx, Orientation::Forward);
            all_faces.push(face_id);

            // Record half-edges for twin pairing using pre-computed keys
            he_map.insert((k0, k1), he0);
            he_map.insert((k1, k2), he1);
            he_map.insert((k2, k3), he2);
            he_map.insert((k3, k0), he3);
        }
    }

    // Build start cap (first ring, reversed winding for outward normal)
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

    // Build end cap (last ring, forward winding)
    let end_ring = &vertex_grid[n_path_samples - 1];
    let end_face_id = build_cap_face(
        &mut topo,
        &mut geom,
        end_ring,
        false,
        &mut he_map,
        quantize_pt,
    );
    all_faces.push(end_face_id);

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

    // Create plane surface from first 3 vertices
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
    for (&(origin_key, dest_key), &he_id) in he_map {
        if topo.half_edges[he_id].twin.is_some() {
            continue;
        }
        if let Some(&twin_id) = he_map.get(&(dest_key, origin_key)) {
            if topo.half_edges[twin_id].twin.is_none() {
                topo.add_edge(he_id, twin_id);
            }
        }
    }
}

fn compute_polygon_normal(verts: &[Point3]) -> Vec3 {
    if verts.len() < 3 {
        return Vec3::z();
    }

    // Newell's method for computing polygon normal
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

fn estimate_path_length(path: &dyn Curve3d) -> f64 {
    let (t_min, t_max) = path.domain();
    let n_samples = 20;
    let dt = (t_max - t_min) / n_samples as f64;

    let mut length = 0.0;
    let mut prev = path.evaluate(t_min);

    for i in 1..=n_samples {
        let t = t_min + i as f64 * dt;
        let curr = path.evaluate(t);
        length += (curr - prev).norm();
        prev = curr;
    }

    length
}

// =============================================================================
// Helix curve implementation
// =============================================================================

/// A helical curve for sweep operations.
///
/// The helix is parameterized as:
/// ```text
/// x(t) = radius * cos(2π * turns * t)
/// y(t) = radius * sin(2π * turns * t)
/// z(t) = pitch * turns * t
/// ```
///
/// Where `t ∈ [0, 1]`.
#[derive(Debug, Clone)]
pub struct Helix {
    /// Center of the helix at the base.
    pub center: Point3,
    /// Radius of the helix.
    pub radius: f64,
    /// Pitch (height per turn).
    pub pitch: f64,
    /// Total height of the helix.
    pub height: f64,
    /// Number of turns.
    pub turns: f64,
}

impl Helix {
    /// Create a new helix.
    ///
    /// # Arguments
    ///
    /// * `radius` - Radius of the helix
    /// * `pitch` - Height per complete turn
    /// * `height` - Total height of the helix
    /// * `turns` - Number of complete turns (overrides pitch if both specified)
    pub fn new(radius: f64, pitch: f64, height: f64, turns: f64) -> Self {
        Self {
            center: Point3::origin(),
            radius,
            pitch,
            height,
            turns,
        }
    }

    /// Create a helix with specified center.
    pub fn with_center(mut self, center: Point3) -> Self {
        self.center = center;
        self
    }
}

impl Curve3d for Helix {
    fn evaluate(&self, t: f64) -> Point3 {
        let angle = 2.0 * PI * self.turns * t;
        let z = self.height * t;
        Point3::new(
            self.center.x + self.radius * angle.cos(),
            self.center.y + self.radius * angle.sin(),
            self.center.z + z,
        )
    }

    fn tangent(&self, t: f64) -> Vec3 {
        let angle = 2.0 * PI * self.turns * t;
        let d_angle = 2.0 * PI * self.turns;

        Vec3::new(
            -self.radius * d_angle * angle.sin(),
            self.radius * d_angle * angle.cos(),
            self.height,
        )
    }

    fn domain(&self) -> (f64, f64) {
        (0.0, 1.0)
    }

    fn curve_type(&self) -> CurveKind {
        CurveKind::Circle // Closest approximation
    }

    fn clone_box(&self) -> Box<dyn Curve3d> {
        Box::new(self.clone())
    }

    fn suggested_segments(&self) -> usize {
        // 48 segments per turn for smooth helix, minimum 64
        ((self.turns * 48.0).ceil() as usize).max(64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_kernel_geom::Line3d;

    fn create_rectangle_profile() -> SketchProfile {
        SketchProfile::rectangle(Point3::origin(), Vec3::x(), Vec3::y(), 4.0, 2.0)
    }

    fn create_circle_profile(radius: f64, n_arcs: u32) -> SketchProfile {
        SketchProfile::circle(Point3::origin(), Vec3::z(), radius, n_arcs)
    }

    #[test]
    fn test_sweep_straight_line() {
        // Sweep along a straight line should be equivalent to extrude
        let profile = create_rectangle_profile();
        let path = Line3d::from_points(Point3::origin(), Point3::new(0.0, 0.0, 10.0));

        let solid = sweep(&profile, &path, SweepOptions::default()).unwrap();

        // Should have proper topology
        assert!(!solid.topology.faces.is_empty());
        assert!(!solid.topology.vertices.is_empty());

        // Check all half-edges are paired
        let unpaired: Vec<_> = solid
            .topology
            .half_edges
            .values()
            .filter(|he| he.twin.is_none())
            .collect();
        assert!(
            unpaired.is_empty(),
            "found {} unpaired half-edges",
            unpaired.len()
        );
    }

    #[test]
    fn test_sweep_helix() {
        let profile = create_circle_profile(1.0, 8);
        let helix = Helix::new(5.0, 10.0, 20.0, 2.0);

        let solid = sweep(&profile, &helix, SweepOptions::default()).unwrap();

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
    fn test_sweep_with_twist() {
        let profile = create_rectangle_profile();
        let path = Line3d::from_points(Point3::origin(), Point3::new(0.0, 0.0, 10.0));

        let options = SweepOptions {
            twist_angle: PI / 2.0, // 90 degree twist
            ..Default::default()
        };

        let solid = sweep(&profile, &path, options).unwrap();
        assert!(!solid.topology.faces.is_empty());
    }

    #[test]
    fn test_sweep_with_scale() {
        let profile = create_rectangle_profile();
        let path = Line3d::from_points(Point3::origin(), Point3::new(0.0, 0.0, 10.0));

        let options = SweepOptions {
            scale_start: 1.0,
            scale_end: 0.5, // Taper
            ..Default::default()
        };

        let solid = sweep(&profile, &path, options).unwrap();
        assert!(!solid.topology.faces.is_empty());
    }

    #[test]
    fn test_sweep_with_orientation() {
        let profile = create_rectangle_profile();
        let path = Line3d::from_points(Point3::origin(), Point3::new(0.0, 0.0, 10.0));

        let options = SweepOptions {
            orientation_angle: PI / 4.0, // 45 degree initial rotation
            ..Default::default()
        };

        let solid = sweep(&profile, &path, options).unwrap();
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
    fn test_sweep_zero_length_path_error() {
        let profile = create_rectangle_profile();
        let path = Line3d::from_points(Point3::origin(), Point3::origin());

        let result = sweep(&profile, &path, SweepOptions::default());
        assert!(matches!(result, Err(SweepError::ZeroLengthPath)));
    }

    #[test]
    fn test_helix_evaluate() {
        let helix = Helix::new(10.0, 5.0, 10.0, 2.0);

        // At t=0, should be at (10, 0, 0)
        let p0 = helix.evaluate(0.0);
        assert!((p0.x - 10.0).abs() < 1e-6);
        assert!(p0.y.abs() < 1e-6);
        assert!(p0.z.abs() < 1e-6);

        // At t=1, should be at (10, 0, 10) (full 2 turns back to x-axis)
        let p1 = helix.evaluate(1.0);
        assert!((p1.x - 10.0).abs() < 1e-6);
        assert!(p1.y.abs() < 1e-6);
        assert!((p1.z - 10.0).abs() < 1e-6);
    }

    #[test]
    fn test_sweep_volume_straight() {
        // Sweep a 4x2 rectangle along 10 units should give volume ~80
        let profile = create_rectangle_profile();
        let path = Line3d::from_points(Point3::origin(), Point3::new(0.0, 0.0, 10.0));

        let solid = sweep(&profile, &path, SweepOptions::default()).unwrap();
        let mesh = vcad_kernel_tessellate::tessellate_brep(&solid, 32);

        let vol = compute_mesh_volume(&mesh);
        // Expected: 4 * 2 * 10 = 80
        assert!((vol - 80.0).abs() < 2.0, "expected volume ~80, got {vol}");
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
