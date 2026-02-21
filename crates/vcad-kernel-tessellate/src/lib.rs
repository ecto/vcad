#![warn(missing_docs)]

//! B-rep to triangle mesh tessellation for the vcad kernel.
//!
//! Converts B-rep faces into triangle meshes by:
//! 1. Sampling face boundaries in parameter space
//! 2. Generating interior sample points
//! 3. Triangulating via ear-clipping
//! 4. Mapping back to 3D via surface evaluation

use std::f64::consts::PI;
use vcad_kernel_geom::{BilinearSurface, GeometryStore, Surface, SurfaceKind};
use vcad_kernel_math::{Point2, Point3, Vec3};
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_topo::{FaceId, Orientation, Topology};

/// Output triangle mesh for rendering and export.
#[derive(Debug, Clone)]
pub struct TriangleMesh {
    /// Flat array of vertex positions: `[x0, y0, z0, x1, y1, z1, ...]` (f32).
    pub vertices: Vec<f32>,
    /// Flat array of triangle indices: `[i0, i1, i2, ...]` (u32).
    pub indices: Vec<u32>,
    /// Flat array of vertex normals: `[nx0, ny0, nz0, ...]` (f32). Same length as vertices.
    pub normals: Vec<f32>,
}

impl TriangleMesh {
    /// Create an empty mesh.
    pub fn new() -> Self {
        Self {
            vertices: Vec::new(),
            indices: Vec::new(),
            normals: Vec::new(),
        }
    }

    /// Number of triangles.
    pub fn num_triangles(&self) -> usize {
        self.indices.len() / 3
    }

    /// Number of vertices.
    pub fn num_vertices(&self) -> usize {
        self.vertices.len() / 3
    }

    /// Merge another mesh into this one.
    pub fn merge(&mut self, other: &TriangleMesh) {
        let offset = self.num_vertices() as u32;
        let other_num_verts = other.num_vertices();

        // Validate other mesh indices before merge
        #[cfg(debug_assertions)]
        for (i, &idx) in other.indices.iter().enumerate() {
            debug_assert!(
                (idx as usize) < other_num_verts,
                "Other mesh has invalid index {} at position {} (only {} vertices)",
                idx,
                i,
                other_num_verts
            );
        }

        // Validate normals match vertices in source mesh
        #[cfg(debug_assertions)]
        debug_assert!(
            other.normals.is_empty() || other.normals.len() == other.vertices.len(),
            "Normals/vertices mismatch: {} normals vs {} vertices",
            other.normals.len(),
            other.vertices.len()
        );

        self.vertices.extend_from_slice(&other.vertices);
        self.normals.extend_from_slice(&other.normals);
        self.indices
            .extend(other.indices.iter().map(|&i| i + offset));
    }
}

impl Default for TriangleMesh {
    fn default() -> Self {
        Self::new()
    }
}

/// Tessellation parameters controlling mesh quality.
#[derive(Debug, Clone, Copy)]
pub struct TessellationParams {
    /// Number of segments for circular features.
    pub circle_segments: u32,
    /// Number of segments along the height of cylindrical/conical features.
    pub height_segments: u32,
    /// Number of latitude bands for spherical features.
    pub latitude_segments: u32,
}

impl Default for TessellationParams {
    fn default() -> Self {
        Self {
            circle_segments: 32,
            height_segments: 1,
            latitude_segments: 16,
        }
    }
}

impl TessellationParams {
    /// Create params from a segment count hint (used for circular features).
    pub fn from_segments(segments: u32) -> Self {
        Self {
            circle_segments: segments.max(3),
            height_segments: 1,
            latitude_segments: (segments / 2).max(4),
        }
    }
}

/// Tessellate an entire B-rep solid into a triangle mesh.
pub fn tessellate_solid(brep: &BRepSolid, params: &TessellationParams) -> TriangleMesh {
    let mut mesh = TriangleMesh::new();
    let solid = &brep.topology.solids[brep.solid_id];
    let shell = &brep.topology.shells[solid.outer_shell];

    for &face_id in &shell.faces {
        let face_mesh = tessellate_face(&brep.topology, &brep.geometry, face_id, params);

        // Validate face mesh before merge
        #[cfg(debug_assertions)]
        {
            let num_verts = face_mesh.num_vertices();
            for (i, &idx) in face_mesh.indices.iter().enumerate() {
                debug_assert!(
                    (idx as usize) < num_verts,
                    "Face {:?} has invalid index {} at position {} (only {} vertices)",
                    face_id,
                    idx,
                    i,
                    num_verts
                );
            }
        }

        mesh.merge(&face_mesh);
    }

    // Validate final mesh
    #[cfg(debug_assertions)]
    {
        let num_verts = mesh.num_vertices();
        for (i, &idx) in mesh.indices.iter().enumerate() {
            debug_assert!(
                (idx as usize) < num_verts,
                "Final mesh has invalid index {} at position {} (only {} vertices)",
                idx,
                i,
                num_verts
            );
        }
    }

    mesh
}

/// Tessellate a single B-rep face.
fn tessellate_face(
    topo: &Topology,
    geom: &GeometryStore,
    face_id: FaceId,
    params: &TessellationParams,
) -> TriangleMesh {
    let face = &topo.faces[face_id];
    let surface = &geom.surfaces[face.surface_index];
    let reversed = face.orientation == Orientation::Reversed;

    match surface.surface_type() {
        SurfaceKind::Plane => tessellate_planar_face_with_geom(topo, geom, face_id, reversed),
        SurfaceKind::Cylinder => tessellate_cylindrical_face(topo, geom, face_id, params, reversed),
        SurfaceKind::Sphere => tessellate_spherical_face(topo, geom, face_id, params, reversed),
        SurfaceKind::Cone => tessellate_conical_face(topo, geom, face_id, params, reversed),
        SurfaceKind::Bilinear => tessellate_bilinear_face(topo, geom, face_id, params, reversed),
        SurfaceKind::Torus => tessellate_toroidal_face(topo, geom, face_id, params, reversed),
        SurfaceKind::BSpline => tessellate_bspline_face(topo, geom, face_id, params, reversed),
    }
}

/// Tessellate a planar face with geometry-aware winding detection.
///
/// This function detects when the loop vertex winding doesn't match the expected
/// face normal (surface normal * orientation), which can happen after boolean
/// operations that split faces. When mismatch is detected, the reversed flag
/// is flipped to ensure correct triangle orientation.
fn tessellate_planar_face_with_geom(
    topo: &Topology,
    geom: &GeometryStore,
    face_id: FaceId,
    reversed: bool,
) -> TriangleMesh {
    let face = &topo.faces[face_id];
    let surface = &geom.surfaces[face.surface_index];

    // Get the surface normal direction (at parameter origin)
    let surface_normal = surface.normal(Point2::new(0.0, 0.0));

    // Effective face normal accounts for orientation
    let expected_normal = if reversed {
        -surface_normal
    } else {
        surface_normal
    };

    let outer_verts: Vec<_> = topo
        .loop_half_edges(face.outer_loop)
        .map(|he| topo.vertices[topo.half_edges[he].origin].point)
        .collect();

    if outer_verts.len() < 3 {
        return TriangleMesh::new();
    }

    // Compute geometric winding normal from loop vertices using Newell's method.
    // This is robust for non-convex polygons and any orientation.
    let mut geom_normal = Vec3::zeros();
    for i in 0..outer_verts.len() {
        let curr = outer_verts[i];
        let next = outer_verts[(i + 1) % outer_verts.len()];
        geom_normal.x += (curr.y - next.y) * (curr.z + next.z);
        geom_normal.y += (curr.z - next.z) * (curr.x + next.x);
        geom_normal.z += (curr.x - next.x) * (curr.y + next.y);
    }

    // Check if geometric winding matches expected face normal
    let dot = geom_normal.dot(&expected_normal);
    let winding_matches = dot > 0.0;

    // If winding doesn't match, flip the reversed flag
    let effective_reversed = if winding_matches { reversed } else { !reversed };

    // Check if face has inner loops (holes)
    let mut mesh = if !face.inner_loops.is_empty() {
        tessellate_planar_face_with_holes(topo, face_id, effective_reversed)
    } else {
        tessellate_planar_face_core(&outer_verts, effective_reversed)
    };

    // Add constant analytical normal for all vertices (planar face has uniform normal)
    let face_normal = if effective_reversed {
        -surface_normal
    } else {
        surface_normal
    };
    let (nx, ny, nz) = (
        face_normal.x as f32,
        face_normal.y as f32,
        face_normal.z as f32,
    );
    for _ in 0..mesh.num_vertices() {
        mesh.normals.extend_from_slice(&[nx, ny, nz]);
    }

    mesh
}

/// Core tessellation logic for a planar polygon without holes.
fn tessellate_planar_face_core(outer_verts: &[Point3], reversed: bool) -> TriangleMesh {
    // Find the best fan center vertex index.
    // For faces with curved boundaries (like quarter disks), we need to pick a vertex
    // that's at the junction of straight edges, not on the curved portion.
    // Heuristic: find a vertex where consecutive edges form a significant angle (corner vertex).
    // Returns None if the polygon is too concave for fan triangulation.
    match find_best_fan_center(outer_verts) {
        Some(fan_center) => {
            // Fan triangulation is valid for this polygon
            let mut mesh = TriangleMesh::new();
            let n = outer_verts.len();

            // Add all vertices (rotated so fan_center is at index 0)
            for i in 0..n {
                let v = &outer_verts[(fan_center + i) % n];
                mesh.vertices.push(v.x as f32);
                mesh.vertices.push(v.y as f32);
                mesh.vertices.push(v.z as f32);
            }

            // Fan triangulation from vertex 0 (which is now the best fan center)
            for i in 1..(n - 1) {
                if reversed {
                    mesh.indices.push(0);
                    mesh.indices.push((i + 1) as u32);
                    mesh.indices.push(i as u32);
                } else {
                    mesh.indices.push(0);
                    mesh.indices.push(i as u32);
                    mesh.indices.push((i + 1) as u32);
                }
            }

            mesh
        }
        None => {
            // Polygon is too concave for fan triangulation - use ear clipping
            tessellate_concave_polygon(outer_verts, reversed)
        }
    }
}

/// Tessellate a concave polygon using ear clipping algorithm.
/// This is the fallback when fan triangulation cannot produce valid triangles.
fn tessellate_concave_polygon(verts: &[Point3], reversed: bool) -> TriangleMesh {
    let n = verts.len();
    if n < 3 {
        return TriangleMesh::new();
    }

    // Build 2D projection for ear clipping.
    // Compute the face plane from first 3 non-collinear vertices.
    let e1 = verts[1] - verts[0];
    let e2 = verts[2] - verts[0];
    let mut face_normal = e1.cross(&e2);
    for i in 3..n {
        if face_normal.norm() > 1e-12 {
            break;
        }
        let ei = verts[i] - verts[0];
        face_normal = e1.cross(&ei);
    }

    if face_normal.norm() < 1e-12 {
        // Degenerate polygon - all points collinear
        return TriangleMesh::new();
    }

    let u_axis = e1.normalize();
    let v_axis = face_normal.cross(&e1).normalize();
    let origin = verts[0];

    // Project 3D points to 2D
    let verts_2d: Vec<(f64, f64)> = verts
        .iter()
        .map(|p| {
            let d = *p - origin;
            (d.dot(&u_axis), d.dot(&v_axis))
        })
        .collect();

    // Build mesh with all 3D vertices
    let mut mesh = TriangleMesh::new();
    for v in verts {
        mesh.vertices.push(v.x as f32);
        mesh.vertices.push(v.y as f32);
        mesh.vertices.push(v.z as f32);
    }

    // Run ear clipping on the 2D projection
    let indices: Vec<usize> = (0..n).collect();
    ear_clip_triangulate(&verts_2d, &indices, &mut mesh.indices, reversed);

    mesh
}

/// Find the best vertex to use as a fan triangulation center.
/// Returns Some(index) if a valid fan center is found, None if the polygon is too concave.
///
/// For simple convex polygons, any vertex works. But for polygons with curved
/// sections (like quarter disks), we should pick a "corner" vertex where two
/// straight edges meet, not a vertex on the curved portion.
///
/// CRITICAL: For concave polygons, we must verify that fan triangulation from
/// the chosen vertex produces correctly-wound triangles. If a vertex is in a
/// concave region, its fan triangles may flip.
fn find_best_fan_center(verts: &[Point3]) -> Option<usize> {
    let n = verts.len();
    if n <= 4 {
        return Some(0); // Simple polygons are fine with vertex 0
    }

    // Compute polygon winding (signed area) to know expected triangle orientation
    let polygon_signed_area: f64 = (0..n)
        .map(|i| {
            let j = (i + 1) % n;
            verts[i].x * verts[j].y - verts[j].x * verts[i].y
        })
        .sum();

    // Helper: check if a fan center produces valid triangles
    // A valid fan center is one where ALL fan triangles have the same winding as the polygon
    let is_valid_fan_center = |center_idx: usize| -> bool {
        let center = &verts[center_idx];
        for i in 1..(n - 1) {
            let v1_idx = (center_idx + i) % n;
            let v2_idx = (center_idx + i + 1) % n;
            let v1 = &verts[v1_idx];
            let v2 = &verts[v2_idx];
            // Compute signed area of triangle (center, v1, v2)
            let tri_area = (v1.x - center.x) * (v2.y - center.y)
                - (v2.x - center.x) * (v1.y - center.y);
            // Triangle should have same sign as polygon (both positive or both negative)
            // Use a small tolerance to avoid issues with degenerate triangles
            if tri_area.abs() > 1e-10 && (tri_area > 0.0) != (polygon_signed_area > 0.0) {
                return false; // This fan center produces a flipped triangle
            }
        }
        true
    };

    // First, find candidates with good geometry (sharp angles, long edges)
    let mut candidates: Vec<(usize, f64)> = Vec::new();

    for i in 0..n {
        let prev = &verts[(i + n - 1) % n];
        let curr = &verts[i];
        let next = &verts[(i + 1) % n];

        // Vectors from current to neighbors
        let to_prev = *prev - *curr;
        let to_next = *next - *curr;

        let len_prev = to_prev.norm();
        let len_next = to_next.norm();

        if len_prev < 1e-10 || len_next < 1e-10 {
            continue;
        }

        // Compute angle using dot product
        let cos_angle = to_prev.dot(&to_next) / (len_prev * len_next);
        let angle = cos_angle.clamp(-1.0, 1.0).acos();

        // Also consider edge lengths - prefer vertices adjacent to longer edges
        // (curved portions tend to have many short edges)
        let edge_factor = 1.0 / (len_prev + len_next + 0.001);

        // Score: lower is better. Prefer sharp angles with longer adjacent edges.
        // Sharp angle = small angle value, so we want to minimize (angle * edge_factor).
        let score = angle * edge_factor;

        candidates.push((i, score));
    }

    // Sort candidates by score (lower is better)
    candidates.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

    // Find the first candidate that produces valid triangles
    if let Some(&(idx, _)) = candidates.iter().find(|(idx, _)| is_valid_fan_center(*idx)) {
        return Some(idx);
    }

    // If no candidate is valid, try all vertices as a fallback
    // No valid fan center exists (polygon is too concave for fan triangulation)
    (0..n).find(|&i| is_valid_fan_center(i))
}

/// Tessellate a bilinear surface face using the surface's normal method.
/// This enables smooth shading when corner normals are provided.
fn tessellate_bilinear_face(
    topo: &Topology,
    geom: &GeometryStore,
    face_id: FaceId,
    params: &TessellationParams,
    reversed: bool,
) -> TriangleMesh {
    let face = &topo.faces[face_id];
    let surface = &geom.surfaces[face.surface_index];

    // Try to downcast to BilinearSurface
    if let Some(bilinear) = surface.as_any().downcast_ref::<BilinearSurface>() {
        let n_u = params.circle_segments.max(2) as usize;
        let n_v = params.height_segments.max(2) as usize;

        let mut mesh = TriangleMesh::new();

        // Generate grid of vertices with surface normals
        for j in 0..=n_v {
            let v = j as f64 / n_v as f64;
            for i in 0..=n_u {
                let u = i as f64 / n_u as f64;
                let uv = Point2::new(u, v);
                let pt: Point3 = bilinear.evaluate(uv);
                let normal = bilinear.normal(uv);

                mesh.vertices.push(pt.x as f32);
                mesh.vertices.push(pt.y as f32);
                mesh.vertices.push(pt.z as f32);

                let (nx, ny, nz) = if reversed {
                    (-normal.x as f32, -normal.y as f32, -normal.z as f32)
                } else {
                    (normal.x as f32, normal.y as f32, normal.z as f32)
                };
                mesh.normals.push(nx);
                mesh.normals.push(ny);
                mesh.normals.push(nz);
            }
        }

        // Generate triangles
        let stride = (n_u + 1) as u32;
        for j in 0..n_v {
            for i in 0..n_u {
                let bl = j as u32 * stride + i as u32;
                let br = bl + 1;
                let tl = bl + stride;
                let tr = tl + 1;

                if reversed {
                    mesh.indices.extend_from_slice(&[bl, tl, br, br, tl, tr]);
                } else {
                    mesh.indices.extend_from_slice(&[bl, br, tl, br, tr, tl]);
                }
            }
        }

        mesh
    } else {
        // Fallback to simple quad tessellation
        TriangleMesh::new()
    }
}

/// Tessellate a planar face with inner loops (holes).
/// Uses a ring-based approach for better triangle quality: adds intermediate
/// Steiner points around each hole to prevent long thin triangles.
fn tessellate_planar_face_with_holes(
    topo: &Topology,
    face_id: FaceId,
    reversed: bool,
) -> TriangleMesh {
    let face = &topo.faces[face_id];

    // Get outer loop vertices
    let outer_verts: Vec<Point3> = topo
        .loop_half_edges(face.outer_loop)
        .map(|he| topo.vertices[topo.half_edges[he].origin].point)
        .collect();

    if outer_verts.len() < 3 {
        return TriangleMesh::new();
    }

    // Get all inner loop vertices
    let mut inner_loops: Vec<Vec<Point3>> = Vec::new();
    for &inner_loop in &face.inner_loops {
        let inner_verts: Vec<Point3> = topo
            .loop_half_edges(inner_loop)
            .map(|he| topo.vertices[topo.half_edges[he].origin].point)
            .collect();
        if inner_verts.len() >= 3 {
            inner_loops.push(inner_verts);
        }
    }

    if inner_loops.is_empty() {
        // No valid inner loops, fall back to simple triangulation
        return tessellate_simple_polygon(&outer_verts, reversed);
    }

    // Build a 2D projection for triangulation
    // Compute the face plane from first 3 vertices
    let e1 = outer_verts[1] - outer_verts[0];
    let e2 = outer_verts[2] - outer_verts[0];
    let face_normal = e1.cross(&e2);
    if face_normal.norm() < 1e-12 {
        return TriangleMesh::new();
    }

    let u_axis = e1.normalize();
    let v_axis = face_normal.cross(&e1).normalize();
    let origin = outer_verts[0];

    // Project 3D points to 2D
    let project = |p: &Point3| -> (f64, f64) {
        let d = *p - origin;
        (d.dot(&u_axis), d.dot(&v_axis))
    };

    // Unproject 2D points back to 3D
    let unproject = |uv: (f64, f64)| -> Point3 { origin + uv.0 * u_axis + uv.1 * v_axis };

    // Project outer loop
    let outer_2d: Vec<(f64, f64)> = outer_verts.iter().map(&project).collect();

    // Project inner loops
    let inner_2d: Vec<Vec<(f64, f64)>> = inner_loops
        .iter()
        .map(|loop_verts| loop_verts.iter().map(&project).collect())
        .collect();

    // Check if we need the ring-based approach (large face with small hole)
    let outer_area = polygon_area_2d(&outer_2d);
    let total_hole_area: f64 = inner_2d.iter().map(|h| polygon_area_2d(h).abs()).sum();

    // Use ring-based approach if holes are small relative to the face
    if total_hole_area < outer_area.abs() * 0.3 {
        return triangulate_with_rings(
            &outer_2d,
            &inner_2d,
            &outer_verts,
            &inner_loops,
            unproject,
            reversed,
        );
    }

    // Use ear-clipping with hole bridging for larger holes
    triangulate_polygon_with_holes(&outer_2d, &inner_2d, &outer_verts, &inner_loops, reversed)
}

/// Compute signed area of a 2D polygon.
fn polygon_area_2d(pts: &[(f64, f64)]) -> f64 {
    let mut area = 0.0;
    let n = pts.len();
    for i in 0..n {
        let j = (i + 1) % n;
        area += pts[i].0 * pts[j].1 - pts[j].0 * pts[i].1;
    }
    area / 2.0
}

/// Triangulate a face with holes using rings around each hole.
/// This creates better quality triangles by adding intermediate Steiner points.
fn triangulate_with_rings<F>(
    outer_2d: &[(f64, f64)],
    inner_2d: &[Vec<(f64, f64)>],
    outer_3d: &[Point3],
    inner_3d: &[Vec<Point3>],
    unproject: F,
    reversed: bool,
) -> TriangleMesh
where
    F: Fn((f64, f64)) -> Point3,
{
    let mut mesh = TriangleMesh::new();

    // For each hole, create a ring of points and triangulate hole-to-ring
    let mut ring_loops_2d: Vec<Vec<(f64, f64)>> = Vec::new();
    let mut ring_loops_3d: Vec<Vec<Point3>> = Vec::new();

    for (hole_2d, hole_3d) in inner_2d.iter().zip(inner_3d.iter()) {
        // Compute hole centroid
        let centroid: (f64, f64) = hole_2d
            .iter()
            .fold((0.0, 0.0), |acc, p| (acc.0 + p.0, acc.1 + p.1));
        let n = hole_2d.len() as f64;
        let centroid = (centroid.0 / n, centroid.1 / n);

        // Compute approximate hole radius
        let hole_radius: f64 = hole_2d
            .iter()
            .map(|p| ((p.0 - centroid.0).powi(2) + (p.1 - centroid.1).powi(2)).sqrt())
            .sum::<f64>()
            / n;

        // Compute the maximum safe ring radius (must stay inside outer polygon)
        // Find the minimum distance from hole centroid to any outer edge
        let max_ring_radius = {
            let mut min_dist = f64::INFINITY;
            let n_outer = outer_2d.len();
            for i in 0..n_outer {
                let j = (i + 1) % n_outer;
                let a = outer_2d[i];
                let b = outer_2d[j];
                // Distance from centroid to edge a-b
                let ab = (b.0 - a.0, b.1 - a.1);
                let len2 = ab.0 * ab.0 + ab.1 * ab.1;
                let dist = if len2 < 1e-12 {
                    // Degenerate edge
                    ((centroid.0 - a.0).powi(2) + (centroid.1 - a.1).powi(2)).sqrt()
                } else {
                    let ap = (centroid.0 - a.0, centroid.1 - a.1);
                    let t = (ap.0 * ab.0 + ap.1 * ab.1) / len2;
                    let t = t.clamp(0.0, 1.0);
                    let proj = (a.0 + t * ab.0, a.1 + t * ab.1);
                    ((centroid.0 - proj.0).powi(2) + (centroid.1 - proj.1).powi(2)).sqrt()
                };
                min_dist = min_dist.min(dist);
            }
            // Use 80% of the distance to the nearest edge as max ring radius
            min_dist * 0.8
        };

        // Create ring at 2x the hole radius, but capped at the max safe radius
        let desired_ring_radius = hole_radius * 2.5;
        let ring_radius = desired_ring_radius
            .min(max_ring_radius)
            .max(hole_radius * 1.2);

        // Create ring vertices aligned with hole vertices (same angle, larger radius)
        // This ensures proper 1-to-1 correspondence for triangle creation
        let ring_2d: Vec<(f64, f64)> = hole_2d
            .iter()
            .map(|h| {
                // Compute angle from centroid to this hole vertex
                let dx = h.0 - centroid.0;
                let dy = h.1 - centroid.1;
                let dist = (dx * dx + dy * dy).sqrt();
                if dist < 1e-12 {
                    // Degenerate case: hole vertex at centroid
                    (centroid.0 + ring_radius, centroid.1)
                } else {
                    // Scale to ring radius while preserving angle
                    let scale = ring_radius / dist;
                    (centroid.0 + dx * scale, centroid.1 + dy * scale)
                }
            })
            .collect();

        let ring_3d: Vec<Point3> = ring_2d.iter().map(|&uv| unproject(uv)).collect();

        // Triangulate from hole to ring
        let hole_start = mesh.num_vertices();
        for v in hole_3d {
            mesh.vertices.push(v.x as f32);
            mesh.vertices.push(v.y as f32);
            mesh.vertices.push(v.z as f32);
        }
        let ring_start = mesh.num_vertices();
        for v in &ring_3d {
            mesh.vertices.push(v.x as f32);
            mesh.vertices.push(v.y as f32);
            mesh.vertices.push(v.z as f32);
        }

        // Create triangles between hole and ring (quad strips)
        let count = hole_2d.len();
        for i in 0..count {
            let h0 = (hole_start + i) as u32;
            let h1 = (hole_start + (i + 1) % count) as u32;
            let r0 = (ring_start + i) as u32;
            let r1 = (ring_start + (i + 1) % count) as u32;

            // Two triangles per quad
            if reversed {
                mesh.indices.extend_from_slice(&[h0, r0, h1]);
                mesh.indices.extend_from_slice(&[h1, r0, r1]);
            } else {
                mesh.indices.extend_from_slice(&[h0, h1, r0]);
                mesh.indices.extend_from_slice(&[h1, r1, r0]);
            }
        }

        ring_loops_2d.push(ring_2d);
        ring_loops_3d.push(ring_3d);
    }

    // Now triangulate from rings to outer boundary
    // Treat rings as new inner loops and use ear-clipping
    let ring_outer_mesh = triangulate_polygon_with_holes(
        outer_2d,
        &ring_loops_2d,
        outer_3d,
        &ring_loops_3d,
        reversed,
    );

    // Merge ring-to-outer mesh into our mesh (with vertex offset)
    let offset = mesh.num_vertices() as u32;
    mesh.vertices.extend_from_slice(&ring_outer_mesh.vertices);
    mesh.indices
        .extend(ring_outer_mesh.indices.iter().map(|&i| i + offset));

    mesh
}

/// Add Steiner points to the outer polygon to improve triangulation quality.
///
/// This function:
/// 1. Subdivides long outer edges into smaller segments (max ~20 units)
/// 2. Adds additional points near each hole centroid
///
/// This prevents very long bridges that cause thin, degenerate triangles.
fn refine_outer_polygon_for_holes(
    outer_2d: &[(f64, f64)],
    outer_3d: &[Point3],
    inner_2d: &[Vec<(f64, f64)>],
) -> (Vec<(f64, f64)>, Vec<Point3>) {
    if outer_2d.len() < 3 {
        return (outer_2d.to_vec(), outer_3d.to_vec());
    }

    // Maximum edge length before subdivision.
    // Using a small value to ensure good quality triangles near holes.
    const MAX_EDGE_LENGTH: f64 = 8.0;

    // First pass: subdivide long edges
    let mut result_2d: Vec<(f64, f64)> = Vec::new();
    let mut result_3d: Vec<Point3> = Vec::new();

    for i in 0..outer_2d.len() {
        let j = (i + 1) % outer_2d.len();
        let a_2d = outer_2d[i];
        let b_2d = outer_2d[j];
        let a_3d = outer_3d[i];
        let b_3d = outer_3d[j];

        // Add the start vertex
        result_2d.push(a_2d);
        result_3d.push(a_3d);

        // Calculate edge length
        let edge_len = ((b_2d.0 - a_2d.0).powi(2) + (b_2d.1 - a_2d.1).powi(2)).sqrt();

        // If edge is long, subdivide it
        if edge_len > MAX_EDGE_LENGTH {
            let num_segments = (edge_len / MAX_EDGE_LENGTH).ceil() as usize;
            for k in 1..num_segments {
                let t = k as f64 / num_segments as f64;
                let new_2d = (
                    a_2d.0 + t * (b_2d.0 - a_2d.0),
                    a_2d.1 + t * (b_2d.1 - a_2d.1),
                );
                let new_3d = Point3::new(
                    a_3d.x + t * (b_3d.x - a_3d.x),
                    a_3d.y + t * (b_3d.y - a_3d.y),
                    a_3d.z + t * (b_3d.z - a_3d.z),
                );
                result_2d.push(new_2d);
                result_3d.push(new_3d);
            }
        }
    }

    // If no holes, we're done with just edge subdivision
    if inner_2d.is_empty() {
        return (result_2d, result_3d);
    }

    // Second pass: add points near each hole centroid
    // Collect insertion points: (edge_index, t_param, 2d_point, 3d_point)
    let mut insertions: Vec<(usize, f64, (f64, f64), Point3)> = Vec::new();

    for hole in inner_2d {
        if hole.is_empty() {
            continue;
        }

        // Find centroid of hole
        let centroid: (f64, f64) = hole
            .iter()
            .fold((0.0, 0.0), |acc, p| (acc.0 + p.0, acc.1 + p.1));
        let n = hole.len() as f64;
        let centroid = (centroid.0 / n, centroid.1 / n);

        // Find closest point on outer polygon edges to the hole centroid
        let mut best_edge = 0;
        let mut best_t = 0.5;
        let mut best_dist = f64::INFINITY;

        for i in 0..result_2d.len() {
            let j = (i + 1) % result_2d.len();
            let a = result_2d[i];
            let b = result_2d[j];

            // Project centroid onto edge a-b
            let ab = (b.0 - a.0, b.1 - a.1);
            let len2 = ab.0 * ab.0 + ab.1 * ab.1;
            if len2 < 1e-12 {
                continue;
            }

            let ap = (centroid.0 - a.0, centroid.1 - a.1);
            let t = (ap.0 * ab.0 + ap.1 * ab.1) / len2;

            // Only consider points on the edge interior (not at endpoints)
            if t <= 0.1 || t >= 0.9 {
                continue;
            }

            let proj = (a.0 + t * ab.0, a.1 + t * ab.1);
            let dist = ((centroid.0 - proj.0).powi(2) + (centroid.1 - proj.1).powi(2)).sqrt();

            if dist < best_dist {
                best_dist = dist;
                best_edge = i;
                best_t = t;
            }
        }

        // Check if the best point is significantly closer than existing vertices
        let mut min_vertex_dist = f64::INFINITY;
        for &v in &result_2d {
            let d = ((centroid.0 - v.0).powi(2) + (centroid.1 - v.1).powi(2)).sqrt();
            min_vertex_dist = min_vertex_dist.min(d);
        }

        // Only add if the edge point is at least 30% closer than any existing vertex
        if best_dist < min_vertex_dist * 0.7 && best_dist < f64::INFINITY {
            let j = (best_edge + 1) % result_2d.len();
            let a_2d = result_2d[best_edge];
            let b_2d = result_2d[j];
            let new_2d = (
                a_2d.0 + best_t * (b_2d.0 - a_2d.0),
                a_2d.1 + best_t * (b_2d.1 - a_2d.1),
            );

            let a_3d = result_3d[best_edge];
            let b_3d = result_3d[j];
            let new_3d = Point3::new(
                a_3d.x + best_t * (b_3d.x - a_3d.x),
                a_3d.y + best_t * (b_3d.y - a_3d.y),
                a_3d.z + best_t * (b_3d.z - a_3d.z),
            );

            insertions.push((best_edge, best_t, new_2d, new_3d));
        }
    }

    if insertions.is_empty() {
        return (result_2d, result_3d);
    }

    // Sort insertions by edge index (descending) then by t (descending within same edge)
    insertions.sort_by(|a, b| {
        if a.0 != b.0 {
            b.0.cmp(&a.0)
        } else {
            b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
        }
    });

    for (edge_idx, _, pt_2d, pt_3d) in insertions {
        result_2d.insert(edge_idx + 1, pt_2d);
        result_3d.insert(edge_idx + 1, pt_3d);
    }

    (result_2d, result_3d)
}

/// Triangulate a polygon with holes using ear-clipping with bridge construction.
fn triangulate_polygon_with_holes(
    outer_2d: &[(f64, f64)],
    inner_2d: &[Vec<(f64, f64)>],
    outer_3d: &[Point3],
    inner_3d: &[Vec<Point3>],
    reversed: bool,
) -> TriangleMesh {
    let mut mesh = TriangleMesh::new();

    // First, refine the outer polygon by adding Steiner points near each hole.
    // This prevents very long bridges that cause thin triangles.
    let (refined_outer_2d, refined_outer_3d) =
        refine_outer_polygon_for_holes(outer_2d, outer_3d, inner_2d);

    // Collect all vertices
    let mut all_verts_3d: Vec<Point3> = refined_outer_3d.clone();
    let mut all_verts_2d: Vec<(f64, f64)> = refined_outer_2d.clone();

    // Track where each inner loop starts
    let mut inner_starts: Vec<usize> = Vec::new();
    for (inner_loop_3d, inner_loop_2d) in inner_3d.iter().zip(inner_2d.iter()) {
        inner_starts.push(all_verts_3d.len());
        all_verts_3d.extend_from_slice(inner_loop_3d);
        all_verts_2d.extend_from_slice(inner_loop_2d);
    }

    // Add all vertices to mesh
    for v in &all_verts_3d {
        mesh.vertices.push(v.x as f32);
        mesh.vertices.push(v.y as f32);
        mesh.vertices.push(v.z as f32);
    }

    // Build a merged polygon by bridging outer to each inner loop
    let mut poly_indices: Vec<usize> = (0..refined_outer_2d.len()).collect();

    // Track which vertices have been used as bridge endpoints
    let mut used_bridge_vertices: std::collections::HashSet<usize> =
        std::collections::HashSet::new();

    for (hole_idx, inner_start) in inner_starts.iter().enumerate() {
        let inner_len = inner_2d[hole_idx].len();

        // Find the pair of (outer vertex, inner vertex) with minimum distance
        // Avoid vertices already used as bridge endpoints
        let mut candidates: Vec<(f64, usize, usize)> = Vec::new(); // (dist, inner_idx, outer_poly_idx)

        for i in 0..inner_len {
            let inner_pt = all_verts_2d[inner_start + i];
            for (j, &outer_idx) in poly_indices.iter().enumerate() {
                let outer_pt = all_verts_2d[outer_idx];
                let dist = (outer_pt.0 - inner_pt.0).powi(2) + (outer_pt.1 - inner_pt.1).powi(2);
                candidates.push((dist, i, j));
            }
        }

        // Sort by distance
        candidates.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

        // Find the best candidate that doesn't reuse a bridge vertex
        let mut best_inner = 0;
        let mut best_outer_idx = 0;

        for (_, inner_idx, outer_poly_idx) in &candidates {
            let outer_vertex_idx = poly_indices[*outer_poly_idx];
            // For the first hole, allow any vertex. For subsequent holes,
            // prefer vertices that haven't been used, but fall back if needed.
            if !used_bridge_vertices.contains(&outer_vertex_idx) || hole_idx == 0 {
                best_inner = *inner_idx;
                best_outer_idx = *outer_poly_idx;
                used_bridge_vertices.insert(outer_vertex_idx);
                break;
            }
        }

        // If all vertices are used (shouldn't happen with reasonable input), use closest anyway
        if candidates.is_empty() {
            continue;
        }

        let rightmost_inner = best_inner;

        // Insert bridge: outer -> hole -> back to outer
        let inner_global_start = *inner_start;
        let hole_indices: Vec<usize> = (0..inner_len)
            .map(|i| inner_global_start + ((rightmost_inner + i) % inner_len))
            .collect();

        // Insert after best_outer_idx:
        // poly[0..=best_outer_idx] + hole_indices + [hole_indices[0], poly[best_outer_idx]] + poly[best_outer_idx+1..]
        // Simplified: insert hole loop with bridge vertices
        let bridge_outer = poly_indices[best_outer_idx];
        let bridge_inner = hole_indices[0];

        let mut new_poly = Vec::new();
        new_poly.extend_from_slice(&poly_indices[..=best_outer_idx]);
        new_poly.extend_from_slice(&hole_indices);
        new_poly.push(bridge_inner);
        new_poly.push(bridge_outer);
        new_poly.extend_from_slice(&poly_indices[best_outer_idx + 1..]);

        poly_indices = new_poly;
    }

    // Now triangulate the merged polygon using ear clipping
    ear_clip_triangulate(&all_verts_2d, &poly_indices, &mut mesh.indices, reversed);

    mesh
}

/// Simple ear-clipping triangulation for a polygon (defined by indices into a vertex array).
fn ear_clip_triangulate(
    verts_2d: &[(f64, f64)],
    indices: &[usize],
    out_indices: &mut Vec<u32>,
    reversed: bool,
) {
    if indices.len() < 3 {
        return;
    }

    let mut remaining: Vec<usize> = indices.to_vec();

    while remaining.len() > 3 {
        let n = remaining.len();
        let mut found_ear = false;

        for i in 0..n {
            let prev = (i + n - 1) % n;
            let next = (i + 1) % n;

            let a = verts_2d[remaining[prev]];
            let b = verts_2d[remaining[i]];
            let c = verts_2d[remaining[next]];

            // Check if this is a convex vertex (ear candidate)
            let cross = (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
            let is_convex = if reversed { cross < 0.0 } else { cross > 0.0 };

            if !is_convex {
                continue;
            }

            // Check if any other vertex is inside this triangle
            let mut is_ear = true;
            for j in 0..n {
                if j == prev || j == i || j == next {
                    continue;
                }
                let p = verts_2d[remaining[j]];
                if point_in_triangle_2d(p, a, b, c) {
                    is_ear = false;
                    break;
                }
            }

            if is_ear {
                // Add triangle
                if reversed {
                    out_indices.push(remaining[prev] as u32);
                    out_indices.push(remaining[next] as u32);
                    out_indices.push(remaining[i] as u32);
                } else {
                    out_indices.push(remaining[prev] as u32);
                    out_indices.push(remaining[i] as u32);
                    out_indices.push(remaining[next] as u32);
                }
                remaining.remove(i);
                found_ear = true;
                break;
            }
        }

        if !found_ear {
            // Degenerate case - just triangulate remaining as fan
            break;
        }
    }

    // Final triangle
    if remaining.len() == 3 {
        if reversed {
            out_indices.push(remaining[0] as u32);
            out_indices.push(remaining[2] as u32);
            out_indices.push(remaining[1] as u32);
        } else {
            out_indices.push(remaining[0] as u32);
            out_indices.push(remaining[1] as u32);
            out_indices.push(remaining[2] as u32);
        }
    }
}

/// Check if a point is inside a triangle in 2D using barycentric coordinates.
fn point_in_triangle_2d(p: (f64, f64), a: (f64, f64), b: (f64, f64), c: (f64, f64)) -> bool {
    let v0 = (c.0 - a.0, c.1 - a.1);
    let v1 = (b.0 - a.0, b.1 - a.1);
    let v2 = (p.0 - a.0, p.1 - a.1);

    let dot00 = v0.0 * v0.0 + v0.1 * v0.1;
    let dot01 = v0.0 * v1.0 + v0.1 * v1.1;
    let dot02 = v0.0 * v2.0 + v0.1 * v2.1;
    let dot11 = v1.0 * v1.0 + v1.1 * v1.1;
    let dot12 = v1.0 * v2.0 + v1.1 * v2.1;

    let inv_denom = 1.0 / (dot00 * dot11 - dot01 * dot01);
    let u = (dot11 * dot02 - dot01 * dot12) * inv_denom;
    let v = (dot00 * dot12 - dot01 * dot02) * inv_denom;

    // Use small epsilon to avoid boundary issues
    let eps = 1e-10;
    u > eps && v > eps && (u + v) < 1.0 - eps
}

/// Simple fan triangulation for a convex polygon.
fn tessellate_simple_polygon(verts: &[Point3], reversed: bool) -> TriangleMesh {
    let mut mesh = TriangleMesh::new();

    for v in verts {
        mesh.vertices.push(v.x as f32);
        mesh.vertices.push(v.y as f32);
        mesh.vertices.push(v.z as f32);
    }

    for i in 1..(verts.len() - 1) {
        if reversed {
            mesh.indices.push(0);
            mesh.indices.push((i + 1) as u32);
            mesh.indices.push(i as u32);
        } else {
            mesh.indices.push(0);
            mesh.indices.push(i as u32);
            mesh.indices.push((i + 1) as u32);
        }
    }

    mesh
}

/// Tessellate a cylindrical face (lateral surface of a cylinder).
fn tessellate_cylindrical_face(
    topo: &Topology,
    geom: &GeometryStore,
    face_id: FaceId,
    params: &TessellationParams,
    reversed: bool,
) -> TriangleMesh {
    let face = &topo.faces[face_id];
    let surface = &geom.surfaces[face.surface_index];
    let n_circ = params.circle_segments.max(3) as usize;
    let mut n_height = params.height_segments.max(1) as usize;


    // Determine the v (height) parameter range by projecting seam vertices
    // onto the cylinder axis. This works correctly after any transform.
    let verts: Vec<_> = topo
        .loop_half_edges(face.outer_loop)
        .map(|he| topo.vertices[topo.half_edges[he].origin].point)
        .collect();


    let mut radius = None;
    let mut u_min = 0.0;
    let mut u_max = 2.0 * PI;
    let (v_min, v_max) = if let Some(cyl) = surface
        .as_any()
        .downcast_ref::<vcad_kernel_geom::CylinderSurface>()
    {
        radius = Some(cyl.radius.abs().max(1e-6));
        // Project vertices onto axis to get v parameter and compute U angles
        let mut vmin = f64::MAX;
        let mut vmax = f64::MIN;

        // Compute U (angle) for each vertex to find the angular range
        let ref_dir = cyl.ref_dir.as_ref();
        let y_dir = cyl.axis.as_ref().cross(ref_dir);
        let mut angles: Vec<f64> = Vec::new();

        for pt in &verts {
            let d = *pt - cyl.center;
            let v = d.dot(cyl.axis.as_ref());
            vmin = vmin.min(v);
            vmax = vmax.max(v);

            // Compute angle for this vertex
            let dot_y = d.dot(&y_dir);
            let dot_ref = d.dot(ref_dir);
            let u = dot_y.atan2(dot_ref);
            // Normalize to [0, 2π). Use a small epsilon to handle -0.0 and tiny negative values
            // that should be treated as 0. Also snap values very close to 2π back to 0.
            let u_normalized = if u < -1e-12 {
                u + 2.0 * PI
            } else if u < 1e-12 || (u - 2.0 * PI).abs() < 1e-12 {
                0.0 // Snap -0.0, tiny negatives, and ~2π to exactly 0
            } else {
                u
            };

            angles.push(u_normalized);
        }

        // Determine U range from the face vertices
        // For a partial face, we need to find the angular extent
        // Get unique angles (vertices at same angle but different heights)
        let mut unique_angles: Vec<f64> = Vec::new();
        for &a in &angles {
            if !unique_angles.iter().any(|&ua| (ua - a).abs() < 0.01) {
                unique_angles.push(a);
            }
        }


        if unique_angles.len() == 1 {
            // Full cylinder (all vertices at same seam angle)
            u_min = 0.0;
            u_max = 2.0 * PI;
        } else {
            // Determine angular direction from loop vertex order
            // Find the first significant angle change to detect winding direction
            let mut first_dir = 0.0;
            for i in 0..angles.len() {
                let a1 = angles[i];
                let a2 = angles[(i + 1) % angles.len()];
                let mut diff = a2 - a1;
                // Normalize to [-π, π]
                if diff > PI {
                    diff -= 2.0 * PI;
                } else if diff < -PI {
                    diff += 2.0 * PI;
                }
                // Use first significant change (> ~6 degrees)
                if diff.abs() > 0.1 {
                    first_dir = diff;
                    break;
                }
            }

            // Sort to find min/max
            unique_angles.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let a_min = unique_angles[0];
            let a_max = unique_angles[unique_angles.len() - 1];

            // Determine which arc the face covers based on the direct span vs wrap-around
            // Direct span: [a_min, a_max]
            // Wrap-around: [a_max, a_min + 2π] (goes through the seam at 0/2π)
            let direct_span = a_max - a_min;
            let wrap_span = 2.0 * PI - direct_span;

            // Use the smaller span, unless it's very close to half the circle
            // (in which case use the winding direction to disambiguate)
            if wrap_span < direct_span - 0.1 {
                // Wrap-around is smaller - face goes through the seam
                u_min = a_max;
                u_max = a_min + 2.0 * PI;
            } else if direct_span < wrap_span - 0.1 {
                // Direct span is smaller
                u_min = a_min;
                u_max = a_max;
            } else {
                // Spans are similar (nearly half-circle) - use winding direction
                if first_dir >= 0.0 {
                    // Counterclockwise in UV space means going from lower to higher U
                    u_min = a_min;
                    u_max = a_max;
                } else {
                    // Clockwise means wrap-around
                    u_min = a_max;
                    u_max = a_min + 2.0 * PI;
                }
            }
        }

        (vmin, vmax)
    } else {
        // Fallback: use z coordinates, full angle
        let z_min = verts.iter().map(|v| v.z).fold(f64::MAX, f64::min);
        let z_max = verts.iter().map(|v| v.z).fold(f64::MIN, f64::max);
        (z_min, z_max)
    };

    let height = v_max - v_min;
    let u_range = u_max - u_min;

    // Adjust segment count based on angular range
    let effective_n_circ = if u_range < 2.0 * PI - 0.01 {
        // Partial face - scale segments by angular fraction
        let fraction = u_range / (2.0 * PI);
        (n_circ as f64 * fraction).ceil().max(2.0) as usize
    } else {
        n_circ
    };

    if let Some(radius) = radius {
        let arc_length = radius * u_range;
        if arc_length > 1e-9 {
            let target = (height.abs() / arc_length) * effective_n_circ as f64;
            n_height = n_height.max(target.ceil() as usize).max(1);
        }
    }

    let mut mesh = TriangleMesh::new();

    // Generate grid of vertices using surface.evaluate
    // Respect the face's U range (angular extent)
    for j in 0..=n_height {
        let v = v_min + height * (j as f64 / n_height as f64);
        for i in 0..=effective_n_circ {
            // Map i to the face's U range, not full 2π
            let u = u_min + u_range * (i as f64 / effective_n_circ as f64);
            // Normalize u to [0, 2π) for surface evaluation
            let u_eval = u % (2.0 * PI);
            let uv = Point2::new(u_eval, v);
            let pt = surface.evaluate(uv);
            let normal = *surface.normal(uv);
            mesh.vertices.push(pt.x as f32);
            mesh.vertices.push(pt.y as f32);
            mesh.vertices.push(pt.z as f32);
            let (nx, ny, nz) = if reversed {
                (-normal.x as f32, -normal.y as f32, -normal.z as f32)
            } else {
                (normal.x as f32, normal.y as f32, normal.z as f32)
            };
            mesh.normals.extend_from_slice(&[nx, ny, nz]);
        }
    }

    // Generate triangles
    let stride = (effective_n_circ + 1) as u32;
    for j in 0..n_height {
        for i in 0..effective_n_circ {
            let bl = j as u32 * stride + i as u32;
            let br = bl + 1;
            let tl = bl + stride;
            let tr = tl + 1;

            if reversed {
                mesh.indices.extend_from_slice(&[bl, tl, br]);
                mesh.indices.extend_from_slice(&[br, tl, tr]);
            } else {
                mesh.indices.extend_from_slice(&[bl, br, tl]);
                mesh.indices.extend_from_slice(&[br, tr, tl]);
            }
        }
    }

    mesh
}

/// Tessellate a spherical face.
/// Uses a single vertex at each pole to avoid normal computation artifacts.
/// For split caps (from boolean operations), uses boundary-aware tessellation.
fn tessellate_spherical_face(
    topo: &Topology,
    geom: &GeometryStore,
    face_id: FaceId,
    params: &TessellationParams,
    reversed: bool,
) -> TriangleMesh {
    let face = &topo.faces[face_id];
    let surface = &geom.surfaces[face.surface_index];

    // Count edges in the face loop to detect split caps
    let loop_verts: Vec<Point3> = topo
        .loop_half_edges(face.outer_loop)
        .map(|he| topo.vertices[topo.half_edges[he].origin].point)
        .collect();

    // A normal sphere has exactly 4 edges from B-rep. Split caps have more.
    if loop_verts.len() > 4 {
        return tessellate_spherical_cap(surface.as_ref(), &loop_verts, reversed);
    }

    let n_lon = params.circle_segments as usize;
    let n_lat = params.latitude_segments as usize;

    let mut mesh = TriangleMesh::new();

    // Helper to push a normal
    let push_normal = |mesh: &mut TriangleMesh, normal: Vec3, reversed: bool| {
        let (nx, ny, nz) = if reversed {
            (-normal.x as f32, -normal.y as f32, -normal.z as f32)
        } else {
            (normal.x as f32, normal.y as f32, normal.z as f32)
        };
        mesh.normals.extend_from_slice(&[nx, ny, nz]);
    };

    // South pole - single vertex (index 0)
    let south_uv = Point2::new(0.0, -PI / 2.0);
    let south = surface.evaluate(south_uv);
    mesh.vertices.push(south.x as f32);
    mesh.vertices.push(south.y as f32);
    mesh.vertices.push(south.z as f32);
    push_normal(&mut mesh, *surface.normal(south_uv), reversed);

    // Middle latitude bands (j = 1 to n_lat - 1)
    for j in 1..n_lat {
        let v = -PI / 2.0 + PI * (j as f64 / n_lat as f64);
        for i in 0..=n_lon {
            let u = 2.0 * PI * (i as f64 / n_lon as f64);
            let uv = Point2::new(u, v);
            let pt = surface.evaluate(uv);
            mesh.vertices.push(pt.x as f32);
            mesh.vertices.push(pt.y as f32);
            mesh.vertices.push(pt.z as f32);
            push_normal(&mut mesh, *surface.normal(uv), reversed);
        }
    }

    // North pole - single vertex (last index)
    let north_uv = Point2::new(0.0, PI / 2.0);
    let north = surface.evaluate(north_uv);
    mesh.vertices.push(north.x as f32);
    mesh.vertices.push(north.y as f32);
    mesh.vertices.push(north.z as f32);
    push_normal(&mut mesh, *surface.normal(north_uv), reversed);

    let south_idx = 0u32;
    let north_idx = mesh.num_vertices() as u32 - 1;
    let stride = (n_lon + 1) as u32;

    // South pole triangles (fan from south pole to first latitude band)
    let first_band_start = 1u32;
    for i in 0..n_lon {
        let v1 = first_band_start + i as u32;
        let v2 = first_band_start + (i + 1) as u32;
        if reversed {
            mesh.indices.extend_from_slice(&[south_idx, v1, v2]);
        } else {
            mesh.indices.extend_from_slice(&[south_idx, v2, v1]);
        }
    }

    // Middle bands (quads between latitude bands)
    for j in 0..(n_lat - 2) {
        let band_start = 1 + j as u32 * stride;
        let next_band_start = band_start + stride;
        for i in 0..n_lon {
            let bl = band_start + i as u32;
            let br = band_start + (i + 1) as u32;
            let tl = next_band_start + i as u32;
            let tr = next_band_start + (i + 1) as u32;

            if reversed {
                mesh.indices.extend_from_slice(&[bl, tl, br]);
                mesh.indices.extend_from_slice(&[br, tl, tr]);
            } else {
                mesh.indices.extend_from_slice(&[bl, br, tl]);
                mesh.indices.extend_from_slice(&[br, tr, tl]);
            }
        }
    }

    // North pole triangles (fan from last latitude band to north pole)
    let last_band_start = 1 + (n_lat - 2) as u32 * stride;
    for i in 0..n_lon {
        let v1 = last_band_start + i as u32;
        let v2 = last_band_start + (i + 1) as u32;
        if reversed {
            mesh.indices.extend_from_slice(&[north_idx, v2, v1]);
        } else {
            mesh.indices.extend_from_slice(&[north_idx, v1, v2]);
        }
    }

    mesh
}

/// Tessellate a spherical cap defined by a boundary loop.
/// Used for split faces from boolean operations.
fn tessellate_spherical_cap(
    surface: &dyn vcad_kernel_geom::Surface,
    loop_verts: &[Point3],
    reversed: bool,
) -> TriangleMesh {
    use vcad_kernel_geom::SphereSurface;

    let mesh = TriangleMesh::new();

    if loop_verts.len() < 3 {
        return mesh;
    }

    // Get sphere center and radius
    let (center, radius) = if let Some(sphere) = surface.as_any().downcast_ref::<SphereSurface>() {
        (sphere.center, sphere.radius)
    } else {
        // Fallback: estimate center from boundary
        let centroid: Point3 = loop_verts.iter().fold(Point3::origin(), |acc, p| {
            Point3::new(acc.x + p.x, acc.y + p.y, acc.z + p.z)
        });
        let n = loop_verts.len() as f64;
        let centroid = Point3::new(centroid.x / n, centroid.y / n, centroid.z / n);
        let r = (loop_verts[0] - centroid).norm();
        (centroid, r)
    };

    // Compute centroid of boundary vertices for cap center
    let boundary_centroid: Point3 = loop_verts.iter().fold(Point3::origin(), |acc, p| {
        Point3::new(acc.x + p.x, acc.y + p.y, acc.z + p.z)
    });
    let n = loop_verts.len() as f64;
    let boundary_centroid = Point3::new(
        boundary_centroid.x / n,
        boundary_centroid.y / n,
        boundary_centroid.z / n,
    );

    // Direction from sphere center to cap center
    let cap_dir = (boundary_centroid - center).normalize();

    // Compute angle from cap direction to each boundary vertex
    let boundary_angles: Vec<f64> = loop_verts
        .iter()
        .map(|p| {
            let v = (*p - center).normalize();
            v.dot(&cap_dir).clamp(-1.0, 1.0).acos()
        })
        .collect();

    let min_angle = boundary_angles
        .iter()
        .cloned()
        .fold(f64::INFINITY, f64::min);
    let avg_angle = boundary_angles.iter().sum::<f64>() / boundary_angles.len() as f64;

    // Determine if this is a large cap (> ~90 degrees) or small cap
    let is_large_cap = avg_angle > PI / 2.0;

    if is_large_cap {
        tessellate_large_spherical_cap(loop_verts, center, radius, cap_dir, min_angle, reversed)
    } else {
        tessellate_small_spherical_cap(loop_verts, center, radius, cap_dir, reversed)
    }
}

/// Tessellate a small spherical cap using fan triangulation from the cap pole.
fn tessellate_small_spherical_cap(
    loop_verts: &[Point3],
    center: Point3,
    radius: f64,
    cap_dir: vcad_kernel_math::Vec3,
    reversed: bool,
) -> TriangleMesh {
    let mut mesh = TriangleMesh::new();

    // Helper to compute sphere normal at a point
    let sphere_normal = |pt: Point3| -> Vec3 {
        let n = (pt - center).normalize();
        if reversed { -n } else { n }
    };

    // Cap pole (point on sphere in cap direction)
    let pole = center + radius * cap_dir;
    mesh.vertices.push(pole.x as f32);
    mesh.vertices.push(pole.y as f32);
    mesh.vertices.push(pole.z as f32);
    let n = sphere_normal(pole);
    mesh.normals.extend_from_slice(&[n.x as f32, n.y as f32, n.z as f32]);

    // Add boundary vertices
    for p in loop_verts {
        mesh.vertices.push(p.x as f32);
        mesh.vertices.push(p.y as f32);
        mesh.vertices.push(p.z as f32);
        let n = sphere_normal(*p);
        mesh.normals.extend_from_slice(&[n.x as f32, n.y as f32, n.z as f32]);
    }

    // Fan triangulation from pole to boundary
    let pole_idx = 0u32;
    let n = loop_verts.len();
    for i in 0..n {
        let v1 = 1 + i as u32;
        let v2 = 1 + ((i + 1) % n) as u32;
        if reversed {
            mesh.indices.extend_from_slice(&[pole_idx, v2, v1]);
        } else {
            mesh.indices.extend_from_slice(&[pole_idx, v1, v2]);
        }
    }

    mesh
}

/// Tessellate a large spherical cap using latitude rings with boundary stitching.
fn tessellate_large_spherical_cap(
    loop_verts: &[Point3],
    center: Point3,
    radius: f64,
    cap_dir: vcad_kernel_math::Vec3,
    min_angle: f64,
    reversed: bool,
) -> TriangleMesh {
    let mut mesh = TriangleMesh::new();

    // Helper to compute sphere normal at a point
    let sphere_normal = |pt: Point3| -> vcad_kernel_math::Vec3 {
        let n = (pt - center).normalize();
        if reversed { -n } else { n }
    };

    // Antipodal pole (opposite to cap center)
    let anti_pole = center - radius * cap_dir;
    mesh.vertices.push(anti_pole.x as f32);
    mesh.vertices.push(anti_pole.y as f32);
    mesh.vertices.push(anti_pole.z as f32);
    let n = sphere_normal(anti_pole);
    mesh.normals.extend_from_slice(&[n.x as f32, n.y as f32, n.z as f32]);

    // Create local coordinate system for longitude
    let up = cap_dir;
    let right = if up.x.abs() < 0.9 {
        vcad_kernel_math::Vec3::new(1.0, 0.0, 0.0)
            .cross(&up)
            .normalize()
    } else {
        vcad_kernel_math::Vec3::new(0.0, 1.0, 0.0)
            .cross(&up)
            .normalize()
    };
    let forward = up.cross(&right);

    // Number of rings between pole and boundary
    let n_rings = 8;
    let n_lon = 32;

    // Generate latitude rings from antipodal pole toward boundary
    let ring_stop = min_angle * 0.98;
    for ring in 1..=n_rings {
        let t = ring as f64 / (n_rings + 1) as f64;
        let angle_from_pole = PI - (PI - ring_stop) * (1.0 - t);
        let sin_a = angle_from_pole.sin();
        let cos_a = angle_from_pole.cos();

        for i in 0..=n_lon {
            let lon = 2.0 * PI * (i as f64 / n_lon as f64);
            let x = sin_a * lon.cos();
            let y = sin_a * lon.sin();
            let z = cos_a;

            let local = x * right + y * forward - z * up;
            let pt = center + radius * local;
            mesh.vertices.push(pt.x as f32);
            mesh.vertices.push(pt.y as f32);
            mesh.vertices.push(pt.z as f32);
            let n = sphere_normal(pt);
            mesh.normals.extend_from_slice(&[n.x as f32, n.y as f32, n.z as f32]);
        }
    }

    // Add boundary vertices
    let boundary_start = mesh.num_vertices();
    for p in loop_verts {
        mesh.vertices.push(p.x as f32);
        mesh.vertices.push(p.y as f32);
        mesh.vertices.push(p.z as f32);
        let n = sphere_normal(*p);
        mesh.normals.extend_from_slice(&[n.x as f32, n.y as f32, n.z as f32]);
    }

    let pole_idx = 0u32;
    let stride = (n_lon + 1) as u32;

    // Pole fan to first ring
    let first_ring_start = 1u32;
    for i in 0..n_lon {
        let v1 = first_ring_start + i as u32;
        let v2 = first_ring_start + (i + 1) as u32;
        if reversed {
            mesh.indices.extend_from_slice(&[pole_idx, v1, v2]);
        } else {
            mesh.indices.extend_from_slice(&[pole_idx, v2, v1]);
        }
    }

    // Bands between rings
    for ring in 0..(n_rings - 1) {
        let ring_start = 1 + ring as u32 * stride;
        let next_ring_start = ring_start + stride;
        for i in 0..n_lon {
            let bl = ring_start + i as u32;
            let br = ring_start + (i + 1) as u32;
            let tl = next_ring_start + i as u32;
            let tr = next_ring_start + (i + 1) as u32;
            if reversed {
                mesh.indices.extend_from_slice(&[bl, br, tl]);
                mesh.indices.extend_from_slice(&[br, tr, tl]);
            } else {
                mesh.indices.extend_from_slice(&[bl, tl, br]);
                mesh.indices.extend_from_slice(&[br, tl, tr]);
            }
        }
    }

    // Stitch last ring to boundary
    let last_ring_start = 1 + (n_rings - 1) as u32 * stride;
    let boundary_start = boundary_start as u32;
    let boundary_len = loop_verts.len();

    let last_ring_angles: Vec<f64> = (0..=n_lon)
        .map(|i| 2.0 * PI * (i as f64 / n_lon as f64))
        .collect();

    let boundary_angles: Vec<f64> = loop_verts
        .iter()
        .map(|p| {
            let v = (*p - center).normalize();
            let x = v.dot(&right);
            let y = v.dot(&forward);
            y.atan2(x).rem_euclid(2.0 * PI)
        })
        .collect();

    stitch_ring_to_boundary(
        &mut mesh,
        last_ring_start,
        n_lon,
        &last_ring_angles,
        boundary_start,
        boundary_len,
        &boundary_angles,
        reversed,
    );

    mesh
}

/// Stitch a latitude ring to an arbitrary boundary loop.
#[allow(clippy::too_many_arguments)]
fn stitch_ring_to_boundary(
    mesh: &mut TriangleMesh,
    ring_start: u32,
    ring_len: usize,
    ring_angles: &[f64],
    boundary_start: u32,
    boundary_len: usize,
    boundary_angles: &[f64],
    reversed: bool,
) {
    // For each ring edge, connect to nearest boundary vertex
    for i in 0..ring_len {
        let ring_curr = ring_start + i as u32;
        let ring_next = ring_start + ((i + 1) % (ring_len + 1)) as u32;

        let ring_angle = (ring_angles[i] + ring_angles[(i + 1) % (ring_len + 1)]) / 2.0;
        let mut closest_boundary = 0usize;
        let mut closest_dist = f64::INFINITY;
        for (j, &ba) in boundary_angles.iter().enumerate() {
            let dist = (ba - ring_angle)
                .abs()
                .min(2.0 * PI - (ba - ring_angle).abs());
            if dist < closest_dist {
                closest_dist = dist;
                closest_boundary = j;
            }
        }
        let boundary_idx = boundary_start + closest_boundary as u32;

        if reversed {
            mesh.indices
                .extend_from_slice(&[ring_curr, boundary_idx, ring_next]);
        } else {
            mesh.indices
                .extend_from_slice(&[ring_curr, ring_next, boundary_idx]);
        }
    }

    // For each boundary edge, connect to nearest ring vertex
    for i in 0..boundary_len {
        let b_curr = boundary_start + i as u32;
        let b_next = boundary_start + ((i + 1) % boundary_len) as u32;

        let b_angle = (boundary_angles[i] + boundary_angles[(i + 1) % boundary_len]) / 2.0;
        let mut closest_ring = 0usize;
        let mut closest_dist = f64::INFINITY;
        for (j, &ra) in ring_angles.iter().enumerate().take(ring_len + 1) {
            let dist = (ra - b_angle).abs().min(2.0 * PI - (ra - b_angle).abs());
            if dist < closest_dist {
                closest_dist = dist;
                closest_ring = j;
            }
        }
        let ring_idx = ring_start + closest_ring as u32;

        if reversed {
            mesh.indices.extend_from_slice(&[b_curr, ring_idx, b_next]);
        } else {
            mesh.indices.extend_from_slice(&[b_curr, b_next, ring_idx]);
        }
    }
}

/// Tessellate a conical face (lateral surface of a cone/frustum).
fn tessellate_conical_face(
    topo: &Topology,
    geom: &GeometryStore,
    face_id: FaceId,
    params: &TessellationParams,
    reversed: bool,
) -> TriangleMesh {
    let face = &topo.faces[face_id];
    let surface = &geom.surfaces[face.surface_index];
    let n_circ = params.circle_segments as usize;
    let n_height = params.height_segments as usize;

    // Get seam vertices to determine the cone extent
    let verts: Vec<_> = topo
        .loop_half_edges(face.outer_loop)
        .map(|he| topo.vertices[topo.half_edges[he].origin].point)
        .collect();

    // Extract cone geometry for axis-aware parameterization
    let (axis, apex, ref_dir, half_angle) = if let Some(cone) = surface
        .as_any()
        .downcast_ref::<vcad_kernel_geom::ConeSurface>(
    ) {
        (
            *cone.axis.as_ref(),
            cone.apex,
            *cone.ref_dir.as_ref(),
            cone.half_angle,
        )
    } else {
        // Fallback: assume Z-axis cone at origin
        let z_min = verts.iter().map(|v| v.z).fold(f64::MAX, f64::min);
        let z_max = verts.iter().map(|v| v.z).fold(f64::MIN, f64::max);
        let r_min = verts
            .iter()
            .filter(|v| (v.z - z_min).abs() < 1e-6)
            .map(|v| (v.x * v.x + v.y * v.y).sqrt())
            .next()
            .unwrap_or(0.0);
        return tessellate_cone_direct(&verts, z_min, z_max, r_min, n_circ, n_height, reversed);
    };

    // Project vertices onto axis to get v parameter range (distance from apex)
    let mut v_min = f64::MAX;
    let mut v_max = f64::MIN;
    for pt in &verts {
        let d = pt - apex;
        let v = d.dot(&axis) / half_angle.cos();
        v_min = v_min.min(v);
        v_max = v_max.max(v);
    }

    // Generate mesh using surface.evaluate()
    let y_dir = axis.cross(&ref_dir);
    let mut mesh = TriangleMesh::new();
    let mut rows: Vec<Vec<u32>> = Vec::new();

    // Helper to push normal for cone surface
    let push_cone_normal =
        |mesh: &mut TriangleMesh, u: f64, ref_dir: Vec3, y_dir: Vec3, axis: Vec3, half_angle: f64, reversed: bool| {
            // Cone outward normal: radial direction rotated by (π/2 - half_angle) toward axis
            let radial = u.cos() * ref_dir + u.sin() * y_dir;
            let normal = half_angle.cos() * radial - half_angle.sin() * axis;
            let (nx, ny, nz) = if reversed {
                (-normal.x as f32, -normal.y as f32, -normal.z as f32)
            } else {
                (normal.x as f32, normal.y as f32, normal.z as f32)
            };
            mesh.normals.extend_from_slice(&[nx, ny, nz]);
        };

    for j in 0..=n_height {
        let t = j as f64 / n_height as f64;
        let v = v_min + (v_max - v_min) * t;
        let r = v * half_angle.sin();

        let mut row = Vec::new();

        if r.abs() < 1e-12 {
            // Apex point - use average normal (axis direction)
            let pt = apex + v * half_angle.cos() * axis;
            let idx = mesh.num_vertices() as u32;
            mesh.vertices.push(pt.x as f32);
            mesh.vertices.push(pt.y as f32);
            mesh.vertices.push(pt.z as f32);
            // At apex, normal is along the axis (degenerate point)
            let (nx, ny, nz) = if reversed {
                (axis.x as f32, axis.y as f32, axis.z as f32)
            } else {
                (-axis.x as f32, -axis.y as f32, -axis.z as f32)
            };
            mesh.normals.extend_from_slice(&[nx, ny, nz]);
            row.push(idx);
        } else {
            let center = apex + v * half_angle.cos() * axis;
            for i in 0..=n_circ {
                let u = 2.0 * PI * (i as f64 / n_circ as f64);
                let pt = center + r * (u.cos() * ref_dir + u.sin() * y_dir);
                let idx = mesh.num_vertices() as u32;
                mesh.vertices.push(pt.x as f32);
                mesh.vertices.push(pt.y as f32);
                mesh.vertices.push(pt.z as f32);
                push_cone_normal(&mut mesh, u, ref_dir, y_dir, axis, half_angle, reversed);
                row.push(idx);
            }
        }

        rows.push(row);
    }

    // Generate triangles between adjacent rows
    for j in 0..n_height {
        let bot = &rows[j];
        let top = &rows[j + 1];

        if bot.len() == 1 {
            let apex_idx = bot[0];
            for i in 0..(top.len() - 1) {
                if reversed {
                    mesh.indices
                        .extend_from_slice(&[apex_idx, top[i + 1], top[i]]);
                } else {
                    mesh.indices
                        .extend_from_slice(&[apex_idx, top[i], top[i + 1]]);
                }
            }
        } else if top.len() == 1 {
            let apex_idx = top[0];
            for i in 0..(bot.len() - 1) {
                if reversed {
                    mesh.indices
                        .extend_from_slice(&[bot[i], apex_idx, bot[i + 1]]);
                } else {
                    mesh.indices
                        .extend_from_slice(&[bot[i], bot[i + 1], apex_idx]);
                }
            }
        } else {
            for i in 0..n_circ {
                let bl = bot[i];
                let br = bot[i + 1];
                let tl = top[i];
                let tr = top[i + 1];
                if reversed {
                    mesh.indices.extend_from_slice(&[bl, tl, br]);
                    mesh.indices.extend_from_slice(&[br, tl, tr]);
                } else {
                    mesh.indices.extend_from_slice(&[bl, br, tl]);
                    mesh.indices.extend_from_slice(&[br, tr, tl]);
                }
            }
        }
    }

    mesh
}

/// Fallback cone tessellation using direct z-axis coordinates.
fn tessellate_cone_direct(
    verts: &[Point3],
    z_min: f64,
    z_max: f64,
    r_at_zmin: f64,
    n_circ: usize,
    n_height: usize,
    reversed: bool,
) -> TriangleMesh {
    let r_at_zmax = verts
        .iter()
        .filter(|v| (v.z - z_max).abs() < 1e-6)
        .map(|v| (v.x * v.x + v.y * v.y).sqrt())
        .next()
        .unwrap_or(0.0);

    let mut mesh = TriangleMesh::new();
    let mut rows: Vec<Vec<u32>> = Vec::new();

    for j in 0..=n_height {
        let t = j as f64 / n_height as f64;
        let z = z_min + (z_max - z_min) * t;
        let r = r_at_zmin + (r_at_zmax - r_at_zmin) * t;

        let mut row = Vec::new();
        if r < 1e-12 {
            let idx = mesh.num_vertices() as u32;
            mesh.vertices.extend_from_slice(&[0.0f32, 0.0f32, z as f32]);
            row.push(idx);
        } else {
            for i in 0..=n_circ {
                let u = 2.0 * PI * (i as f64 / n_circ as f64);
                let idx = mesh.num_vertices() as u32;
                mesh.vertices.extend_from_slice(&[
                    (r * u.cos()) as f32,
                    (r * u.sin()) as f32,
                    z as f32,
                ]);
                row.push(idx);
            }
        }
        rows.push(row);
    }

    for j in 0..n_height {
        let bot = &rows[j];
        let top = &rows[j + 1];
        if bot.len() == 1 {
            let a = bot[0];
            for i in 0..(top.len() - 1) {
                if reversed {
                    mesh.indices.extend_from_slice(&[a, top[i + 1], top[i]]);
                } else {
                    mesh.indices.extend_from_slice(&[a, top[i], top[i + 1]]);
                }
            }
        } else if top.len() == 1 {
            let a = top[0];
            for i in 0..(bot.len() - 1) {
                if reversed {
                    mesh.indices.extend_from_slice(&[bot[i], a, bot[i + 1]]);
                } else {
                    mesh.indices.extend_from_slice(&[bot[i], bot[i + 1], a]);
                }
            }
        } else {
            for i in 0..n_circ {
                let bl = bot[i];
                let br = bot[i + 1];
                let tl = top[i];
                let tr = top[i + 1];
                if reversed {
                    mesh.indices.extend_from_slice(&[bl, tl, br]);
                    mesh.indices.extend_from_slice(&[br, tl, tr]);
                } else {
                    mesh.indices.extend_from_slice(&[bl, br, tl]);
                    mesh.indices.extend_from_slice(&[br, tr, tl]);
                }
            }
        }
    }

    mesh
}

/// Tessellate a toroidal face.
///
/// Uses UV grid sampling similar to sphere tessellation.
fn tessellate_toroidal_face(
    topo: &Topology,
    geom: &GeometryStore,
    face_id: FaceId,
    params: &TessellationParams,
    reversed: bool,
) -> TriangleMesh {
    let face = &topo.faces[face_id];
    let surface = &geom.surfaces[face.surface_index];
    let n_u = params.circle_segments as usize;
    let n_v = params.circle_segments as usize;

    let mut mesh = TriangleMesh::new();

    // Get UV domain
    let ((u_min, u_max), (v_min, v_max)) = surface.domain();

    // Generate grid of vertices with analytical normals
    for j in 0..=n_v {
        let v = v_min + (v_max - v_min) * (j as f64 / n_v as f64);
        for i in 0..=n_u {
            let u = u_min + (u_max - u_min) * (i as f64 / n_u as f64);
            let uv = Point2::new(u, v);
            let pt = surface.evaluate(uv);
            let normal = *surface.normal(uv);
            mesh.vertices.push(pt.x as f32);
            mesh.vertices.push(pt.y as f32);
            mesh.vertices.push(pt.z as f32);
            let (nx, ny, nz) = if reversed {
                (-normal.x as f32, -normal.y as f32, -normal.z as f32)
            } else {
                (normal.x as f32, normal.y as f32, normal.z as f32)
            };
            mesh.normals.extend_from_slice(&[nx, ny, nz]);
        }
    }

    // Generate triangles
    let stride = (n_u + 1) as u32;
    for j in 0..n_v {
        for i in 0..n_u {
            let bl = j as u32 * stride + i as u32;
            let br = bl + 1;
            let tl = bl + stride;
            let tr = tl + 1;

            if reversed {
                mesh.indices.extend_from_slice(&[bl, tl, br, br, tl, tr]);
            } else {
                mesh.indices.extend_from_slice(&[bl, br, tl, br, tr, tl]);
            }
        }
    }

    mesh
}

/// Tessellate a B-spline or NURBS face.
///
/// Uses adaptive UV grid sampling.
fn tessellate_bspline_face(
    topo: &Topology,
    geom: &GeometryStore,
    face_id: FaceId,
    params: &TessellationParams,
    reversed: bool,
) -> TriangleMesh {
    let face = &topo.faces[face_id];
    let surface = &geom.surfaces[face.surface_index];

    // Use higher resolution for B-splines since they can be complex
    let n_u = (params.circle_segments * 2).max(16) as usize;
    let n_v = (params.circle_segments * 2).max(16) as usize;

    let mut mesh = TriangleMesh::new();

    // Get UV domain
    let ((u_min, u_max), (v_min, v_max)) = surface.domain();

    // Generate grid of vertices with normals
    for j in 0..=n_v {
        let v = v_min + (v_max - v_min) * (j as f64 / n_v as f64);
        for i in 0..=n_u {
            let u = u_min + (u_max - u_min) * (i as f64 / n_u as f64);
            let uv = Point2::new(u, v);
            let pt = surface.evaluate(uv);
            let normal = *surface.normal(uv);

            mesh.vertices.push(pt.x as f32);
            mesh.vertices.push(pt.y as f32);
            mesh.vertices.push(pt.z as f32);

            let (nx, ny, nz) = if reversed {
                (-normal.x as f32, -normal.y as f32, -normal.z as f32)
            } else {
                (normal.x as f32, normal.y as f32, normal.z as f32)
            };
            mesh.normals.push(nx);
            mesh.normals.push(ny);
            mesh.normals.push(nz);
        }
    }

    // Generate triangles
    let stride = (n_u + 1) as u32;
    for j in 0..n_v {
        for i in 0..n_u {
            let bl = j as u32 * stride + i as u32;
            let br = bl + 1;
            let tl = bl + stride;
            let tr = tl + 1;

            if reversed {
                mesh.indices.extend_from_slice(&[bl, tl, br, br, tl, tr]);
            } else {
                mesh.indices.extend_from_slice(&[bl, br, tl, br, tr, tl]);
            }
        }
    }

    mesh
}

/// Tessellate a planar disk with arbitrary orientation.
/// `x_dir` and `y_dir` define the disk plane.
fn tessellate_disk_general(
    center: Point3,
    radius: f64,
    x_dir: vcad_kernel_math::Vec3,
    y_dir: vcad_kernel_math::Vec3,
    segments: u32,
    flip: bool,
) -> TriangleMesh {
    let mut mesh = TriangleMesh::new();
    let n = segments as usize;

    // Center vertex
    mesh.vertices.push(center.x as f32);
    mesh.vertices.push(center.y as f32);
    mesh.vertices.push(center.z as f32);

    // Rim vertices
    for i in 0..=n {
        let u = 2.0 * PI * (i as f64 / n as f64);
        let pt = center + radius * (u.cos() * x_dir + u.sin() * y_dir);
        mesh.vertices.push(pt.x as f32);
        mesh.vertices.push(pt.y as f32);
        mesh.vertices.push(pt.z as f32);
    }

    // Fan triangles
    for i in 0..n {
        let v0 = 0u32;
        let v1 = (i + 1) as u32;
        let v2 = (i + 2) as u32;
        if flip {
            mesh.indices.extend_from_slice(&[v0, v2, v1]);
        } else {
            mesh.indices.extend_from_slice(&[v0, v1, v2]);
        }
    }

    mesh
}

/// Tessellate a planar disk (cap face) with a circular boundary.
/// Used for cylinder and cone caps.
pub fn tessellate_disk(
    center: Point3,
    radius: f64,
    z: f64,
    segments: u32,
    flip: bool,
) -> TriangleMesh {
    let mut mesh = TriangleMesh::new();
    let n = segments as usize;

    // Center vertex
    mesh.vertices.push(center.x as f32);
    mesh.vertices.push(center.y as f32);
    mesh.vertices.push(z as f32);

    // Rim vertices
    for i in 0..=n {
        let u = 2.0 * PI * (i as f64 / n as f64);
        mesh.vertices.push((radius * u.cos()) as f32);
        mesh.vertices.push((radius * u.sin()) as f32);
        mesh.vertices.push(z as f32);
    }

    // Fan triangles
    for i in 0..n {
        let v0 = 0u32; // center
        let v1 = (i + 1) as u32;
        let v2 = (i + 2) as u32;
        if flip {
            mesh.indices.extend_from_slice(&[v0, v2, v1]);
        } else {
            mesh.indices.extend_from_slice(&[v0, v1, v2]);
        }
    }

    mesh
}

/// Full tessellation of a B-rep solid, using `segments` as a quality hint.
///
/// This is the main entry point for converting a B-rep to a triangle mesh.
///
/// Output format:
/// - `vertices`: flat `Vec<f32>` of `[x, y, z, x, y, z, ...]`
/// - `indices`: flat `Vec<u32>` of triangle vertex indices
pub fn tessellate(brep: &BRepSolid, segments: u32) -> TriangleMesh {
    let params = TessellationParams::from_segments(segments);
    let solid = &brep.topology.solids[brep.solid_id];
    let shell = &brep.topology.shells[solid.outer_shell];

    let mut mesh = TriangleMesh::new();

    for &face_id in &shell.faces {
        let face = &brep.topology.faces[face_id];
        let surface = &brep.geometry.surfaces[face.surface_index];
        let reversed = face.orientation == Orientation::Reversed;

        match surface.surface_type() {
            SurfaceKind::Plane => {
                // Use winding-aware tessellation to handle faces with mismatched loop winding
                let face_mesh = tessellate_planar_face_with_geom(&brep.topology, &brep.geometry, face_id, reversed);
                mesh.merge(&face_mesh);
            }
            SurfaceKind::Cylinder => {
                let face_mesh = tessellate_cylindrical_face(
                    &brep.topology,
                    &brep.geometry,
                    face_id,
                    &params,
                    reversed,
                );
                mesh.merge(&face_mesh);

                // Also tessellate the caps
                // (Caps are separate faces and will be handled as planar faces
                //  if they have enough vertices. But our cylinder caps only have
                //  1 vertex in the loop, so we generate disks directly.)
            }
            SurfaceKind::Sphere => {
                let face_mesh = tessellate_spherical_face(
                    &brep.topology,
                    &brep.geometry,
                    face_id,
                    &params,
                    reversed,
                );
                mesh.merge(&face_mesh);
            }
            SurfaceKind::Cone => {
                let face_mesh = tessellate_conical_face(
                    &brep.topology,
                    &brep.geometry,
                    face_id,
                    &params,
                    reversed,
                );
                mesh.merge(&face_mesh);
            }
            _ => {
                // Fallback for tessellate(): use winding-aware tessellation
                let face_mesh = tessellate_planar_face_with_geom(&brep.topology, &brep.geometry, face_id, reversed);
                mesh.merge(&face_mesh);
            }
        }
    }

    mesh
}

/// Tessellate a B-rep solid with special handling for cap faces that
/// have degenerate (single-vertex) loops.
///
/// This is the primary tessellation function used by the facade crate.
pub fn tessellate_brep(brep: &BRepSolid, segments: u32) -> TriangleMesh {
    let params = TessellationParams::from_segments(segments);
    let solid = &brep.topology.solids[brep.solid_id];
    let shell = &brep.topology.shells[solid.outer_shell];

    let mut mesh = TriangleMesh::new();

    for &face_id in &shell.faces {
        let face = &brep.topology.faces[face_id];
        let surface = &brep.geometry.surfaces[face.surface_index];
        let reversed = face.orientation == Orientation::Reversed;
        let loop_len = brep.topology.loop_len(face.outer_loop);

        match surface.surface_type() {
            SurfaceKind::Plane => {
                if loop_len <= 1 && face.inner_loops.is_empty() {
                    // Degenerate cap face with ≤1 vertex and no holes — single
                    // degenerate edge forming a full circle.
                    // Tessellate as a disk using surface origin as center.
                    // Note: loop_len==2 faces are NOT safe to treat as disks —
                    // the 2 vertices may be far from the plane origin (e.g. two
                    // arcs forming a non-circular boundary).
                    let verts: Vec<_> = brep
                        .topology
                        .loop_half_edges(face.outer_loop)
                        .map(|he| brep.topology.vertices[brep.topology.half_edges[he].origin].point)
                        .collect();
                    if let Some(&v) = verts.first() {
                        let plane = &brep.geometry.surfaces[face.surface_index];
                        let center = plane.evaluate(Point2::origin());
                        let r = (v - center).norm();
                        let x_dir = if r > 1e-12 {
                            (v - center).normalize()
                        } else {
                            plane.d_du(Point2::origin()).normalize()
                        };
                        let normal = plane.normal(Point2::origin());
                        let y_dir = normal.as_ref().cross(&x_dir);

                        let disk = tessellate_disk_general(
                            center,
                            r,
                            x_dir,
                            y_dir,
                            params.circle_segments,
                            reversed,
                        );
                        mesh.merge(&disk);
                    }
                } else {
                    // Use winding-aware tessellation to handle faces with mismatched loop winding
                    let face_mesh = tessellate_planar_face_with_geom(&brep.topology, &brep.geometry, face_id, reversed);
                    mesh.merge(&face_mesh);
                }
            }
            SurfaceKind::Cylinder => {
                let face_mesh = tessellate_cylindrical_face(
                    &brep.topology,
                    &brep.geometry,
                    face_id,
                    &params,
                    reversed,
                );
                mesh.merge(&face_mesh);
            }
            SurfaceKind::Sphere => {
                let face_mesh = tessellate_spherical_face(
                    &brep.topology,
                    &brep.geometry,
                    face_id,
                    &params,
                    reversed,
                );
                mesh.merge(&face_mesh);
            }
            SurfaceKind::Cone => {
                let face_mesh = tessellate_conical_face(
                    &brep.topology,
                    &brep.geometry,
                    face_id,
                    &params,
                    reversed,
                );
                mesh.merge(&face_mesh);
            }
            _ => {
                // Fallback for tessellate_brep(): use winding-aware tessellation
                let face_mesh = tessellate_planar_face_with_geom(&brep.topology, &brep.geometry, face_id, reversed);
                mesh.merge(&face_mesh);
            }
        }
    }

    mesh
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_kernel_primitives::{make_cone, make_cube, make_cylinder, make_sphere};

    #[test]
    fn test_tessellate_cube() {
        let brep = make_cube(10.0, 10.0, 10.0);
        let mesh = tessellate_brep(&brep, 32);
        // A cube should have at least 12 triangles (2 per face × 6 faces)
        assert!(
            mesh.num_triangles() >= 12,
            "expected >= 12 triangles, got {}",
            mesh.num_triangles()
        );
        assert!(mesh.num_vertices() > 0);
    }

    #[test]
    fn test_tessellate_cylinder() {
        let brep = make_cylinder(5.0, 10.0, 32);
        let mesh = tessellate_brep(&brep, 32);
        // Cylinder: lateral (32 quads = 64 tris) + 2 caps (32 tris each) = ~128
        assert!(
            mesh.num_triangles() >= 64,
            "expected >= 64 triangles, got {}",
            mesh.num_triangles()
        );
    }

    #[test]
    fn test_tessellate_sphere() {
        let brep = make_sphere(10.0, 32);
        let mesh = tessellate_brep(&brep, 32);
        assert!(
            mesh.num_triangles() >= 100,
            "expected >= 100 triangles, got {}",
            mesh.num_triangles()
        );
    }

    #[test]
    fn test_tessellate_cone() {
        let brep = make_cone(5.0, 0.0, 10.0, 32);
        let mesh = tessellate_brep(&brep, 32);
        assert!(
            mesh.num_triangles() >= 32,
            "expected >= 32 triangles, got {}",
            mesh.num_triangles()
        );
    }

    #[test]
    fn test_cube_volume_from_mesh() {
        let brep = make_cube(10.0, 10.0, 10.0);
        let mesh = tessellate_brep(&brep, 32);
        let vol = compute_mesh_volume(&mesh);
        assert!((vol - 1000.0).abs() < 1.0, "expected ~1000, got {vol}");
    }

    #[test]
    fn test_cube_surface_area_from_mesh() {
        let brep = make_cube(10.0, 10.0, 10.0);
        let mesh = tessellate_brep(&brep, 32);
        let area = compute_mesh_surface_area(&mesh);
        assert!((area - 600.0).abs() < 1.0, "expected ~600, got {area}");
    }

    #[test]
    fn test_cylinder_volume_from_mesh() {
        let brep = make_cylinder(5.0, 10.0, 64);
        let mesh = tessellate_brep(&brep, 64);
        let expected = PI * 25.0 * 10.0; // π r² h
        let vol = compute_mesh_volume(&mesh);
        assert!(
            (vol - expected).abs() < expected * 0.05,
            "expected ~{expected}, got {vol}"
        );
    }

    #[test]
    fn test_sphere_volume_from_mesh() {
        let brep = make_sphere(10.0, 64);
        let mesh = tessellate_brep(&brep, 64);
        let expected = (4.0 / 3.0) * PI * 1000.0; // (4/3)πr³
        let vol = compute_mesh_volume(&mesh);
        // Sphere tessellation is less accurate, allow 5% error
        assert!(
            (vol - expected).abs() < expected * 0.05,
            "expected ~{expected}, got {vol}"
        );
    }

    /// Compute signed volume of a triangle mesh using the divergence theorem.
    fn compute_mesh_volume(mesh: &TriangleMesh) -> f64 {
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

    /// Compute surface area of a triangle mesh.
    fn compute_mesh_surface_area(mesh: &TriangleMesh) -> f64 {
        let verts = &mesh.vertices;
        let indices = &mesh.indices;
        let mut area = 0.0;
        for tri in indices.chunks(3) {
            let (i0, i1, i2) = (
                tri[0] as usize * 3,
                tri[1] as usize * 3,
                tri[2] as usize * 3,
            );
            let v0 = [verts[i0] as f64, verts[i0 + 1] as f64, verts[i0 + 2] as f64];
            let v1 = [verts[i1] as f64, verts[i1 + 1] as f64, verts[i1 + 2] as f64];
            let v2 = [verts[i2] as f64, verts[i2 + 1] as f64, verts[i2 + 2] as f64];
            let e1 = [v1[0] - v0[0], v1[1] - v0[1], v1[2] - v0[2]];
            let e2 = [v2[0] - v0[0], v2[1] - v0[1], v2[2] - v0[2]];
            let cross = [
                e1[1] * e2[2] - e1[2] * e2[1],
                e1[2] * e2[0] - e1[0] * e2[2],
                e1[0] * e2[1] - e1[1] * e2[0],
            ];
            area += (cross[0] * cross[0] + cross[1] * cross[1] + cross[2] * cross[2]).sqrt() / 2.0;
        }
        area
    }

    #[test]
    fn test_triangulate_square_with_circular_hole() {
        // Test the triangulation of a square with a circular hole in the center
        // This is what happens when a cylinder cuts through a planar face
        use vcad_kernel_math::Point3;

        // Square: 10x10 in XY plane at Z=0 (CCW winding)
        let outer_2d: Vec<(f64, f64)> = vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        let outer_3d: Vec<Point3> = vec![
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(10.0, 0.0, 0.0),
            Point3::new(10.0, 10.0, 0.0),
            Point3::new(0.0, 10.0, 0.0),
        ];

        // Circular hole: radius 2, center at (5, 5), 8 segments
        // CW winding (opposite to outer) - this is how B-rep stores inner loops
        let n_seg = 8usize;
        let hole_2d: Vec<(f64, f64)> = (0..n_seg)
            .rev() // CW winding: reverse the order
            .map(|i| {
                let theta = 2.0 * std::f64::consts::PI * (i as f64) / (n_seg as f64);
                (5.0 + 2.0 * theta.cos(), 5.0 + 2.0 * theta.sin())
            })
            .collect();
        let hole_3d: Vec<Point3> = hole_2d
            .iter()
            .map(|&(x, y)| Point3::new(x, y, 0.0))
            .collect();

        let inner_2d = vec![hole_2d];
        let inner_3d = vec![hole_3d];

        let mesh =
            triangulate_polygon_with_holes(&outer_2d, &inner_2d, &outer_3d, &inner_3d, false);

        println!(
            "Square with hole: {} triangles, {} vertices",
            mesh.num_triangles(),
            mesh.num_vertices()
        );

        // Should have triangles
        assert!(mesh.num_triangles() > 0, "Should produce triangles");

        // Compute mesh area - should be square area minus circle area
        let area = compute_mesh_surface_area(&mesh);
        let expected_area = 100.0 - std::f64::consts::PI * 4.0; // 100 - 4π ≈ 87.4
        println!("Mesh area: {:.2}, expected: {:.2}", area, expected_area);

        // Allow some tolerance due to polygon approximation of circle
        assert!(
            (area - expected_area).abs() < 5.0,
            "Area should be ~{:.1}, got {:.1}",
            expected_area,
            area
        );
    }

}
