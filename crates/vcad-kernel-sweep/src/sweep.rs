//! Sweep operation: create a solid by moving a profile along a path.

use std::f64::consts::PI;

use rustc_hash::FxHashMap;
use vcad_kernel_geom::{BilinearSurface, Curve3d, CurveKind, GeometryStore, Plane};
use vcad_kernel_math::{Dir3, Point3, Vec3};
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_sketch::SketchProfile;
use vcad_kernel_topo::{HalfEdgeId, Orientation, ShellType, Topology, VertexId};

use crate::frenet::{rotation_minimizing_frames, FrenetFrame};
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

    build_swept_solid(&tessellated_profile, &frames, options, false)
}

/// Stitch a tessellated profile into a closed solid along a prepared frame
/// sequence.
///
/// Split out of [`sweep`] so alternative framings — see
/// [`sweep_cylindrical`] — reuse the identical topology construction and
/// therefore inherit its capping and twin-pairing behaviour verbatim.
///
/// `normalize_winding` re-orders the profile ring so the lateral quads face
/// outward regardless of how the caller wound the profile and of whether the
/// frame basis is right- or left-handed with respect to the direction of
/// travel. [`sweep`] leaves it off (its rotation-minimizing frames are always
/// right-handed, and its callers already wind CCW); the cylindrical framing
/// below needs it, because a radial/axial basis is left-handed against
/// increasing angle.
pub(crate) fn build_swept_solid(
    tessellated_profile: &SketchProfile,
    frames: &[FrenetFrame],
    options: SweepOptions,
    normalize_winding: bool,
) -> Result<BRepSolid, SweepError> {
    let n_path_samples = frames.len();
    if n_path_samples < 2 {
        return Err(SweepError::ZeroLengthPath);
    }
    let n_path_segments = n_path_samples - 1;
    let n_profile_verts = tessellated_profile.segments.len();
    if n_profile_verts < 3 {
        return Err(SweepError::InvalidProfile(
            "a swept profile needs at least 3 vertices".into(),
        ));
    }

    // Get profile vertices in 2D (from tessellated profile)
    let mut profile_verts_2d = tessellated_profile.vertices_2d();

    if normalize_winding {
        // Signed area of the profile in frame (normal, binormal) coordinates.
        let mut area2 = 0.0;
        for i in 0..n_profile_verts {
            let a = profile_verts_2d[i];
            let b = profile_verts_2d[(i + 1) % n_profile_verts];
            area2 += a.x * b.y - b.x * a.y;
        }
        // Handedness of the frame basis against the direction of travel: the
        // lateral-quad winding below assumes (normal × binormal) points the
        // way the sweep is going.
        let f = &frames[0];
        let travel = &(frames[n_path_samples - 1].position - f.position);
        let across = f.normal.as_ref().cross(f.binormal.as_ref());
        let handedness = across.dot(travel).signum();
        if area2 * handedness < 0.0 {
            profile_verts_2d.reverse();
        }
    }

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
        FxHashMap::with_capacity_and_hasher(
            n_lateral * 4 + n_profile_verts * 2,
            Default::default(),
        );

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

// =============================================================================
// Cylindrical path — a z(θ) polyline wrapped on a cylinder
// =============================================================================

/// The default angular step, in degrees, between path samples.
pub const DEFAULT_SEG_DEG: f64 = 0.5;

/// A path that lives on the surface of a cylinder: constant radius, angle
/// sweeping from the first knot to the last, and height given by a
/// piecewise-linear function of the angle.
///
/// A plain helix is the two-knot case, which is what
/// [`CylindricalPath::helix`] builds. Extra knots express the features a cam
/// track actually has — a lead-in ramp, a rise–plateau–drop detent, a flat
/// pocket — without leaving the primitive, because the deviation from a
/// constant rate *is* the feature.
///
/// Knots are `(angle in degrees, height in mm)` and must be monotonic in
/// angle. Height is relative to the path's `center`, so `helix` starts at
/// `center.z` whatever `start_deg` is; place the result with a translate
/// rather than by arithmetic on the start angle.
#[derive(Debug, Clone)]
pub struct CylindricalPath {
    /// Point on the cylinder axis that heights are measured from.
    pub center: Point3,
    /// Cylinder radius.
    pub radius: f64,
    /// `(angle_deg, z)` knots, monotonic in angle, at least two.
    pub knots: Vec<(f64, f64)>,
    /// Angular step between path samples, in degrees.
    pub seg_deg: f64,
}

impl CylindricalPath {
    /// A constant-rate helical arc.
    ///
    /// `rate_mm_per_deg` is the rise per degree of arc — the way a cam or a
    /// bayonet track is actually dimensioned, and the number a 4th-axis
    /// toolpath is programmed from. `arc_deg` may be negative for a
    /// left-hand track.
    pub fn helix(radius: f64, rate_mm_per_deg: f64, start_deg: f64, arc_deg: f64) -> Self {
        Self {
            center: Point3::origin(),
            radius,
            knots: vec![
                (start_deg, 0.0),
                (start_deg + arc_deg, rate_mm_per_deg * arc_deg),
            ],
            seg_deg: DEFAULT_SEG_DEG,
        }
    }

    /// A path through arbitrary `(angle_deg, z)` knots.
    pub fn from_knots(radius: f64, knots: Vec<(f64, f64)>) -> Self {
        Self {
            center: Point3::origin(),
            radius,
            knots,
            seg_deg: DEFAULT_SEG_DEG,
        }
    }

    /// Override the angular sample step (degrees).
    pub fn with_seg_deg(mut self, seg_deg: f64) -> Self {
        if seg_deg > 0.0 {
            self.seg_deg = seg_deg;
        }
        self
    }

    /// Move the path's axis reference point.
    pub fn with_center(mut self, center: Point3) -> Self {
        self.center = center;
        self
    }

    /// First knot angle, in degrees.
    pub fn start_deg(&self) -> f64 {
        self.knots.first().map(|k| k.0).unwrap_or(0.0)
    }

    /// Last knot angle, in degrees.
    pub fn end_deg(&self) -> f64 {
        self.knots.last().map(|k| k.0).unwrap_or(0.0)
    }

    /// Total signed arc, in degrees.
    pub fn arc_deg(&self) -> f64 {
        self.end_deg() - self.start_deg()
    }

    /// Height of the path at `angle_deg`, by linear interpolation between
    /// the bracketing knots (clamped outside the knot range).
    ///
    /// This is the analytic surface the swept solid is built on, so a test
    /// can compare a probed floor against it directly.
    pub fn height_at_deg(&self, angle_deg: f64) -> f64 {
        let k = &self.knots;
        if k.is_empty() {
            return 0.0;
        }
        let ascending = self.arc_deg() >= 0.0;
        let inside = |a: f64, b: f64| {
            if ascending {
                angle_deg >= a && angle_deg <= b
            } else {
                angle_deg <= a && angle_deg >= b
            }
        };
        for w in k.windows(2) {
            let ((a0, z0), (a1, z1)) = (w[0], w[1]);
            if inside(a0, a1) {
                let span = a1 - a0;
                if span.abs() < 1e-12 {
                    return z1;
                }
                return z0 + (z1 - z0) * (angle_deg - a0) / span;
            }
        }
        if ascending == (angle_deg < self.start_deg()) {
            k[0].1
        } else {
            k[k.len() - 1].1
        }
    }

    /// The angles the path is sampled at: every knot, plus a `seg_deg` grid
    /// between them. Knots land exactly on samples, so a plateau's corners
    /// are never rounded off by the sampling.
    pub fn sample_angles(&self) -> Vec<f64> {
        let mut out = Vec::new();
        let step = if self.seg_deg > 0.0 {
            self.seg_deg
        } else {
            DEFAULT_SEG_DEG
        };
        for w in self.knots.windows(2) {
            let (a0, a1) = (w[0].0, w[1].0);
            let span = a1 - a0;
            let n = ((span.abs() / step).ceil() as usize).max(1);
            for i in 0..n {
                out.push(a0 + span * i as f64 / n as f64);
            }
        }
        out.push(self.end_deg());
        out
    }

    /// Frames for a cylindrical sweep: the cross-section plane contains the
    /// cylinder axis and the point on the path.
    ///
    /// Profile x is the **radial** offset from `radius` (outward positive)
    /// and profile y is the **axial** offset (up positive). The plane is
    /// deliberately *not* perpendicular to the tangent: a cam track's
    /// cross-section is what a radial section of the part shows, and holding
    /// the section vertical is what makes the swept floor land on
    /// [`height_at_deg`] exactly rather than to within a cosine. It also
    /// keeps the frame well-defined across a knot, where the tangent jumps.
    pub fn frames(&self) -> Vec<FrenetFrame> {
        self.sample_angles()
            .into_iter()
            .map(|deg| {
                let rad = deg.to_radians();
                let (s, c) = rad.sin_cos();
                let position = Point3::new(
                    self.center.x + self.radius * c,
                    self.center.y + self.radius * s,
                    self.center.z + self.height_at_deg(deg),
                );
                let normal = Dir3::new_normalize(Vec3::new(c, s, 0.0));
                let binormal = Dir3::new_normalize(Vec3::z());
                let arc = self.arc_deg();
                let eps = (arc.abs() * 1e-4).max(1e-6);
                let dz_ddeg =
                    (self.height_at_deg(deg + eps) - self.height_at_deg(deg - eps)) / (2.0 * eps);
                let tangent = Dir3::new_normalize(Vec3::new(
                    -self.radius * s * arc.to_radians(),
                    self.radius * c * arc.to_radians(),
                    dz_ddeg * arc,
                ));
                FrenetFrame {
                    position,
                    tangent,
                    normal,
                    binormal,
                }
            })
            .collect()
    }

    fn angle_at(&self, t: f64) -> f64 {
        self.start_deg() + self.arc_deg() * t
    }
}

impl Curve3d for CylindricalPath {
    fn evaluate(&self, t: f64) -> Point3 {
        let deg = self.angle_at(t);
        let rad = deg.to_radians();
        let (s, c) = rad.sin_cos();
        Point3::new(
            self.center.x + self.radius * c,
            self.center.y + self.radius * s,
            self.center.z + self.height_at_deg(deg),
        )
    }

    fn tangent(&self, t: f64) -> Vec3 {
        let deg = self.angle_at(t);
        let rad = deg.to_radians();
        let (s, c) = rad.sin_cos();
        // d/dt, with the height slope taken from the bracketing knots. A
        // one-sided step keeps the tangent finite at a knot instead of
        // averaging across the corner.
        let arc = self.arc_deg();
        let eps = (arc.abs() * 1e-4).max(1e-6);
        let dz_ddeg = (self.height_at_deg(deg + eps) - self.height_at_deg(deg - eps)) / (2.0 * eps);
        let darc_dt = arc.to_radians();
        Vec3::new(
            -self.radius * s * darc_dt,
            self.radius * c * darc_dt,
            dz_ddeg * arc,
        )
    }

    fn domain(&self) -> (f64, f64) {
        (0.0, 1.0)
    }

    fn curve_type(&self) -> CurveKind {
        CurveKind::Circle
    }

    fn clone_box(&self) -> Box<dyn Curve3d> {
        Box::new(self.clone())
    }

    fn suggested_segments(&self) -> usize {
        self.sample_angles().len().saturating_sub(1).max(2)
    }
}

/// Sweep a closed profile along a [`CylindricalPath`], holding the
/// cross-section in the radial/axial plane.
///
/// This is the primitive behind cam tracks, bayonet slots, J-slots and
/// lead-in ramps. Unlike [`sweep`], the profile is not rotated to stay
/// perpendicular to the tangent; see [`CylindricalPath::frames`] for why.
///
/// The result is a closed solid — lateral faces plus a cap at each end — so
/// it is watertight standing alone, which is what lets it be subtracted from
/// a body whose wall it passes clean through.
///
/// Profile coordinates are `(radial offset, axial offset)`, both relative to
/// the path, and the profile may be wound either way.
pub fn sweep_cylindrical(
    profile: &SketchProfile,
    path: &CylindricalPath,
    options: SweepOptions,
) -> Result<BRepSolid, SweepError> {
    if path.knots.len() < 2 {
        return Err(SweepError::ZeroLengthPath);
    }
    if path.arc_deg().abs() < 1e-9 {
        return Err(SweepError::ZeroLengthPath);
    }
    if profile.segments.is_empty() {
        return Err(SweepError::InvalidProfile("empty profile".into()));
    }

    let arc_segments = options.arc_segments.max(1) as usize;
    let tessellated = profile.tessellate(arc_segments);

    let frames = if options.path_segments > 0 {
        // An explicit segment count overrides the path's own angular step.
        let stepped = path
            .clone()
            .with_seg_deg(path.arc_deg().abs() / options.path_segments as f64);
        stepped.frames()
    } else {
        path.frames()
    };
    if frames.len() < 2 {
        return Err(SweepError::TooFewSegments);
    }

    build_swept_solid(&tessellated, &frames, options, true)
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

    fn signed_volume(solid: &BRepSolid) -> f64 {
        let mesh = vcad_kernel_tessellate::tessellate_brep(solid, 32);
        let v = &mesh.vertices;
        let mut vol = 0.0;
        for tri in mesh.indices.chunks(3) {
            let (a, b, c) = (
                tri[0] as usize * 3,
                tri[1] as usize * 3,
                tri[2] as usize * 3,
            );
            let p0 = [v[a] as f64, v[a + 1] as f64, v[a + 2] as f64];
            let p1 = [v[b] as f64, v[b + 1] as f64, v[b + 2] as f64];
            let p2 = [v[c] as f64, v[c + 1] as f64, v[c + 2] as f64];
            vol += p0[0] * (p1[1] * p2[2] - p2[1] * p1[2])
                - p1[0] * (p0[1] * p2[2] - p2[1] * p0[2])
                + p2[0] * (p0[1] * p1[2] - p1[1] * p0[2]);
        }
        vol / 6.0
    }

    /// Sweep places the profile on rotation-minimizing frames, so its
    /// winding depends on the profile's 2D handedness and the path
    /// direction rather than on the sketch normal (the extrude trap). Pin
    /// both signs: a sweep must never come back inside-out.
    #[test]
    fn sweep_is_positive_volume_for_either_frame_handedness() {
        for (x_dir, y_dir) in [(Vec3::x(), Vec3::y()), (Vec3::y(), Vec3::x())] {
            for end in [Point3::new(0.0, 0.0, 10.0), Point3::new(0.0, 0.0, -10.0)] {
                let profile = SketchProfile::rectangle(Point3::origin(), x_dir, y_dir, 4.0, 2.0);
                let path = Line3d::from_points(Point3::origin(), end);
                let vol = signed_volume(&sweep(&profile, &path, SweepOptions::default()).unwrap());
                assert!(
                    (vol - 80.0).abs() < 0.5,
                    "frame ({x_dir:?},{y_dir:?}) to {end:?}: expected +80, got {vol}"
                );
            }
        }
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

    // ---------------------------------------------------------------------
    // CylindricalPath
    // ---------------------------------------------------------------------

    /// The height function is what everything else is checked against, so it
    /// is checked against the drawing first.
    #[test]
    fn helix_height_is_rate_times_arc() {
        let h = CylindricalPath::helix(34.5, 0.0667, 0.0, 15.0);
        assert_eq!(h.height_at_deg(0.0), 0.0);
        assert!((h.height_at_deg(15.0) - 0.0667 * 15.0).abs() < 1e-12);
        assert!((h.height_at_deg(7.5) - 0.0667 * 7.5).abs() < 1e-12);
        // Clamped outside the knot range rather than extrapolated.
        assert_eq!(h.height_at_deg(-5.0), 0.0);
        assert!((h.height_at_deg(30.0) - 0.0667 * 15.0).abs() < 1e-12);
    }

    #[test]
    fn a_negative_arc_reads_the_same_way() {
        let h = CylindricalPath::helix(34.5, 0.0667, 20.0, -15.0);
        assert_eq!(h.height_at_deg(20.0), 0.0);
        assert!((h.height_at_deg(5.0) - 0.0667 * -15.0).abs() < 1e-12);
        assert!((h.height_at_deg(12.5) - 0.0667 * -7.5).abs() < 1e-12);
    }

    /// Knots must land exactly on samples: a plateau whose corners fall
    /// between samples is rounded off, which is the whole failure mode a
    /// detent has.
    #[test]
    fn knots_are_always_sampled_exactly() {
        let p = CylindricalPath::from_knots(
            34.5,
            vec![
                (3.6, -0.25),
                (9.6, 0.15),
                (10.3, 0.25),
                (11.3, 0.25),
                (11.6, 0.15),
            ],
        );
        let angles = p.sample_angles();
        for (knot, _) in &p.knots {
            assert!(
                angles.iter().any(|a| (a - knot).abs() < 1e-9),
                "knot at {knot}° is not a sample"
            );
        }
        // ...and the default step is honoured between them.
        for w in angles.windows(2) {
            assert!(w[1] - w[0] <= DEFAULT_SEG_DEG + 1e-9);
        }
    }

    #[test]
    fn seg_deg_sets_the_facet_count() {
        let coarse = CylindricalPath::helix(34.5, 0.0667, 0.0, 15.0).with_seg_deg(5.0);
        let fine = CylindricalPath::helix(34.5, 0.0667, 0.0, 15.0).with_seg_deg(0.1);
        assert_eq!(coarse.sample_angles().len(), 4); // 3 segments + endpoint
        assert_eq!(fine.sample_angles().len(), 151);
    }

    fn cyl_solid(path: &CylindricalPath, radial: f64, axial: f64) -> BRepSolid {
        let profile =
            SketchProfile::rectangle(Point3::origin(), Vec3::x(), Vec3::y(), radial, axial);
        sweep_cylindrical(&profile, path, SweepOptions::default()).unwrap()
    }

    /// A closed solid has zero net signed area over its faces; the cheaper
    /// proxy here is a non-zero volume plus a full complement of faces.
    #[test]
    fn a_cylindrical_sweep_is_capped_at_both_ends() {
        let path = CylindricalPath::helix(34.5, 0.0667, 0.0, 15.0);
        let solid = cyl_solid(&path, 3.0, 2.0);
        // 30 path segments x 4 profile edges, plus the two caps.
        assert_eq!(solid.topology.faces.len(), 30 * 4 + 2);
        assert!(signed_volume(&solid) > 0.0);
    }

    /// Winding is the caller's business, not the primitive's: a CW profile
    /// and a CCW one must give the same solid, not one inside out.
    #[test]
    fn profile_winding_does_not_flip_the_solid() {
        let path = CylindricalPath::helix(34.5, 0.0667, 0.0, 15.0);
        let ccw = SketchProfile::rectangle(Point3::origin(), Vec3::x(), Vec3::y(), 3.0, 2.0);
        let cw = ccw.reversed();
        let a = sweep_cylindrical(&ccw, &path, SweepOptions::default()).unwrap();
        let b = sweep_cylindrical(&cw, &path, SweepOptions::default()).unwrap();
        let (va, vb) = (signed_volume(&a), signed_volume(&b));
        assert!(
            va > 0.0 && vb > 0.0,
            "one winding came out inside out: {va} vs {vb}"
        );
        assert!((va - vb).abs() < 1e-6);
    }

    #[test]
    fn a_zero_arc_is_refused_rather_than_degenerate() {
        let path = CylindricalPath::helix(34.5, 0.0667, 10.0, 0.0);
        let profile = SketchProfile::rectangle(Point3::origin(), Vec3::x(), Vec3::y(), 3.0, 2.0);
        assert!(sweep_cylindrical(&profile, &path, SweepOptions::default()).is_err());
    }
}
