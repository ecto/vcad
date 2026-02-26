//! Sub-face classification for B-rep boolean operations.
//!
//! After face splitting, each sub-face must be classified as IN, OUT,
//! ON_SAME, or ON_OPPOSITE relative to the other solid. The boolean
//! operation then selects which sub-faces to keep.

use vcad_kernel_geom::SurfaceKind;
use vcad_kernel_math::Point3;
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_tessellate::{tessellate_brep, TriangleMesh};
use vcad_kernel_topo::FaceId;

use crate::point_in_mesh;
use crate::split::point_to_segment_dist_2d;
use crate::BooleanOp;

/// Classification of a face relative to another solid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaceClassification {
    /// Face is outside the other solid.
    Outside,
    /// Face is inside the other solid.
    Inside,
    /// Face is on the boundary, normals agree.
    OnSame,
    /// Face is on the boundary, normals oppose.
    OnOpposite,
}

/// Compute a sample point in the interior of a face.
///
/// Returns a 3D point that lies on the face's surface, inside its boundary
/// but outside any holes (inner loops). Uses different strategies depending
/// on whether the face has holes.
pub fn face_sample_point(brep: &BRepSolid, face_id: FaceId) -> Point3 {
    let topo = &brep.topology;
    let face = &topo.faces[face_id];

    // Collect outer loop vertices
    let vertices: Vec<Point3> = topo
        .loop_half_edges(face.outer_loop)
        .map(|he_id| topo.vertices[topo.half_edges[he_id].origin].point)
        .collect();

    if vertices.is_empty() {
        return Point3::origin();
    }

    // Special case: circular disk face with a single seam vertex
    // (e.g., cylinder caps). The single vertex is on the circle's boundary,
    // not at its center.
    if vertices.len() == 1 {
        let surface = &brep.geometry.surfaces[face.surface_index];
        if let Some(plane) = surface.as_any().downcast_ref::<vcad_kernel_geom::Plane>() {
            let center = plane.origin;

            if face.inner_loops.is_empty() {
                // Simple disk — center is a safe sample point
                return center;
            }

            // Annular face (degenerate outer loop + inner hole).
            // Sample between the outer boundary and the nearest inner loop.
            let normal = plane.normal_dir.into_inner();
            let to_boundary = vertices[0] - center;
            let on_plane = to_boundary - to_boundary.dot(&normal) * normal;
            let outer_r = on_plane.norm();
            let x_dir = if outer_r > 1e-12 {
                on_plane.normalize()
            } else {
                return center;
            };

            // Collect all inner loop info (center and radius for each hole)
            let y_dir = normal.cross(&x_dir);
            let mut concentric_inner_r = 0.0f64;
            struct HoleInfo {
                center_2d: (f64, f64),
                radius: f64,
            }
            let mut holes: Vec<HoleInfo> = Vec::new();
            for &inner_loop in &face.inner_loops {
                let inner_verts: Vec<Point3> = topo
                    .loop_half_edges(inner_loop)
                    .map(|he_id| topo.vertices[topo.half_edges[he_id].origin].point)
                    .collect();
                if inner_verts.is_empty() {
                    continue;
                }
                // Compute hole center and radius
                let hole_center = if inner_verts.len() == 1 {
                    // Degenerate inner loop — it's a circle centered at the face center
                    let d = inner_verts[0] - center;
                    let r = (d - d.dot(&normal) * normal).norm();
                    concentric_inner_r = concentric_inner_r.max(r);
                    continue; // Concentric holes don't need extra checking
                } else {
                    // Multi-vertex inner loop — compute centroid
                    let mut cx = 0.0;
                    let mut cy = 0.0;
                    let mut max_r = 0.0f64;
                    for v in &inner_verts {
                        let d = v - center;
                        let on_plane = d - d.dot(&normal) * normal;
                        cx += on_plane.dot(&x_dir);
                        cy += on_plane.dot(&y_dir);
                    }
                    let n = inner_verts.len() as f64;
                    let hc = (cx / n, cy / n);
                    for v in &inner_verts {
                        let d = v - center;
                        let on_plane = d - d.dot(&normal) * normal;
                        let px = on_plane.dot(&x_dir);
                        let py = on_plane.dot(&y_dir);
                        let dr = ((px - hc.0).powi(2) + (py - hc.1).powi(2)).sqrt();
                        max_r = max_r.max(dr);
                    }
                    HoleInfo {
                        center_2d: hc,
                        radius: max_r,
                    }
                };
                holes.push(hole_center);
            }

            // Try multiple sample angles to find one that avoids all holes
            let mid_r = (outer_r + concentric_inner_r) / 2.0;
            let num_tries = 36;
            for try_idx in 0..num_tries {
                let angle = 2.0 * std::f64::consts::PI * (try_idx as f64) / (num_tries as f64);
                let (sin_a, cos_a) = angle.sin_cos();
                let sample_dir = cos_a * x_dir + sin_a * y_dir;
                let candidate = center + mid_r * sample_dir;
                let cx = mid_r * cos_a;
                let cy = mid_r * sin_a;

                // Check if candidate is inside any hole (concentric or off-center)
                let mut in_hole = false;
                // Check concentric holes: candidate must be outside the concentric inner radius
                if concentric_inner_r > 0.0 && mid_r < concentric_inner_r + 1e-6 {
                    in_hole = true;
                }
                // Check off-center holes
                if !in_hole {
                    for hole in &holes {
                        let dx = cx - hole.center_2d.0;
                        let dy = cy - hole.center_2d.1;
                        if dx * dx + dy * dy < (hole.radius + 1e-6) * (hole.radius + 1e-6) {
                            in_hole = true;
                            break;
                        }
                    }
                }
                if !in_hole {
                    return candidate;
                }
            }

            // Fallback: just use the first try
            return center + mid_r * x_dir;
        }
        // Fallback: return the single vertex
        return vertices[0];
    }

    // If face has inner loops (holes), we need a smarter sample point
    // that's outside the holes but inside the outer boundary.
    if !face.inner_loops.is_empty() {
        // Get surface for projection
        let surface = &brep.geometry.surfaces[face.surface_index];
        let is_planar = surface.surface_type() == SurfaceKind::Plane;

        if is_planar && vertices.len() >= 3 {
            // Build 2D coordinate system from the plane
            let v0 = vertices[0];
            let v1 = vertices[1];
            let v2 = vertices[2];
            let e1 = v1 - v0;
            let e2 = v2 - v0;
            let normal = e1.cross(&e2);
            let normal_len = normal.norm();

            if normal_len > 1e-12 {
                let u_axis = e1.normalize();
                let v_axis = normal.cross(&e1).normalize();

                // Project outer loop to 2D
                let project_2d = |p: &Point3| -> (f64, f64) {
                    let d = *p - v0;
                    (d.dot(&u_axis), d.dot(&v_axis))
                };

                let outer_2d: Vec<(f64, f64)> = vertices.iter().map(&project_2d).collect();

                // Collect all inner loop vertices in 2D
                let mut inner_loops_2d: Vec<Vec<(f64, f64)>> = Vec::new();
                for &inner_loop in &face.inner_loops {
                    let inner_verts: Vec<(f64, f64)> = topo
                        .loop_half_edges(inner_loop)
                        .map(|he_id| {
                            let pt = topo.vertices[topo.half_edges[he_id].origin].point;
                            project_2d(&pt)
                        })
                        .collect();
                    if !inner_verts.is_empty() {
                        inner_loops_2d.push(inner_verts);
                    }
                }

                // Try multiple candidate points along each edge of the outer boundary
                // Pick the one that's farthest from any hole
                let mut best_point: Option<Point3> = None;
                let mut best_dist = 0.0f64;

                for i in 0..vertices.len() {
                    let j = (i + 1) % vertices.len();
                    let p0 = vertices[i];
                    let p1 = vertices[j];
                    let p0_2d = outer_2d[i];
                    let p1_2d = outer_2d[j];

                    // Try points at 10%, 25%, 50%, 75%, 90% along this edge
                    for &t in &[0.1, 0.25, 0.5, 0.75, 0.9] {
                        let candidate_3d = p0 + t * (p1 - p0);
                        let candidate_2d = (
                            p0_2d.0 + t * (p1_2d.0 - p0_2d.0),
                            p0_2d.1 + t * (p1_2d.1 - p0_2d.1),
                        );

                        // Check distance to nearest hole
                        let mut min_hole_dist = f64::INFINITY;
                        for inner_loop_2d in &inner_loops_2d {
                            for k in 0..inner_loop_2d.len() {
                                let l = (k + 1) % inner_loop_2d.len();
                                let dist = point_to_segment_dist_2d(
                                    candidate_2d.0,
                                    candidate_2d.1,
                                    inner_loop_2d[k].0,
                                    inner_loop_2d[k].1,
                                    inner_loop_2d[l].0,
                                    inner_loop_2d[l].1,
                                );
                                min_hole_dist = min_hole_dist.min(dist);
                            }
                        }

                        if min_hole_dist > best_dist {
                            best_dist = min_hole_dist;
                            best_point = Some(candidate_3d);
                        }
                    }
                }

                if let Some(pt) = best_point {
                    return pt;
                }
            }
        }

        // Fallback for non-planar faces or if 2D approach fails:
        // Strategy: pick a point on the outer boundary's edge midpoint
        // and move slightly inward. This avoids the hole in the center.
        if vertices.len() >= 2 {
            // Take the midpoint of the first edge
            let edge_mid = Point3::new(
                (vertices[0].x + vertices[1].x) / 2.0,
                (vertices[0].y + vertices[1].y) / 2.0,
                (vertices[0].z + vertices[1].z) / 2.0,
            );

            // Compute face centroid
            let n = vertices.len() as f64;
            let cx = vertices.iter().map(|v| v.x).sum::<f64>() / n;
            let cy = vertices.iter().map(|v| v.y).sum::<f64>() / n;
            let cz = vertices.iter().map(|v| v.z).sum::<f64>() / n;
            let centroid = Point3::new(cx, cy, cz);

            // Move from edge_mid slightly toward centroid, but only 10% of the way
            // This keeps the sample point near the outer boundary, avoiding holes
            let dir = centroid - edge_mid;
            let sample = edge_mid + 0.1 * dir;

            return sample;
        }
    }

    // Standard case: no holes
    // For planar faces, try multiple sample points and use the one farthest from edges.
    // This helps with faces created by arc splits where the centroid might be
    // inside the cutting cylinder even though the face should be kept.
    let surface = &brep.geometry.surfaces[face.surface_index];
    let is_planar = surface.surface_type() == SurfaceKind::Plane;

    if is_planar && vertices.len() >= 3 {
        // Build 2D coordinate system from the plane
        let v0 = vertices[0];
        let v1 = vertices[1];
        let v2 = vertices[2];
        let e1 = v1 - v0;
        let e2 = v2 - v0;
        let normal = e1.cross(&e2);
        let normal_len = normal.norm();

        if normal_len > 1e-12 {
            let u_axis = e1.normalize();
            let v_axis = normal.cross(&e1).normalize();

            // Project vertices to 2D
            let project_2d = |p: &Point3| -> (f64, f64) {
                let d = *p - v0;
                (d.dot(&u_axis), d.dot(&v_axis))
            };

            let verts_2d: Vec<(f64, f64)> = vertices.iter().map(&project_2d).collect();

            // Compute face centroid in 2D
            let n_verts = vertices.len() as f64;
            let centroid_2d = (
                verts_2d.iter().map(|v| v.0).sum::<f64>() / n_verts,
                verts_2d.iter().map(|v| v.1).sum::<f64>() / n_verts,
            );

            // Try candidate points: edge midpoints moved slightly toward centroid
            // This ensures the sample is inside the face, not on its boundary
            // Pick the one farthest from all polygon edges
            let mut best_point: Option<Point3> = None;
            let mut best_dist = 0.0f64;

            for i in 0..vertices.len() {
                let j = (i + 1) % vertices.len();

                // Midpoint of this edge
                let mid_2d = (
                    verts_2d[i].0 + 0.5 * (verts_2d[j].0 - verts_2d[i].0),
                    verts_2d[i].1 + 0.5 * (verts_2d[j].1 - verts_2d[i].1),
                );

                // Move 20% toward centroid to get inside the face
                let candidate_2d = (
                    mid_2d.0 + 0.2 * (centroid_2d.0 - mid_2d.0),
                    mid_2d.1 + 0.2 * (centroid_2d.1 - mid_2d.1),
                );

                // Compute distance to nearest edge (all edges now)
                let mut min_dist = f64::INFINITY;
                for k in 0..vertices.len() {
                    let l = (k + 1) % vertices.len();
                    let dist = point_to_segment_dist_2d(
                        candidate_2d.0,
                        candidate_2d.1,
                        verts_2d[k].0,
                        verts_2d[k].1,
                        verts_2d[l].0,
                        verts_2d[l].1,
                    );
                    min_dist = min_dist.min(dist);
                }

                if min_dist > best_dist {
                    best_dist = min_dist;
                    // Convert back to 3D
                    let candidate_3d = v0 + candidate_2d.0 * u_axis + candidate_2d.1 * v_axis;
                    best_point = Some(candidate_3d);
                }
            }

            if let Some(pt) = best_point {
                // Snap small values
                let snap = |v: f64| if v.abs() < 1e-9 { 0.0 } else { v };
                return Point3::new(snap(pt.x), snap(pt.y), snap(pt.z));
            }
        }
    }

    // Fallback: use centroid
    let n = vertices.len() as f64;
    let cx = vertices.iter().map(|v| v.x).sum::<f64>() / n;
    let cy = vertices.iter().map(|v| v.y).sum::<f64>() / n;
    let cz = vertices.iter().map(|v| v.z).sum::<f64>() / n;
    let centroid = Point3::new(cx, cy, cz);

    // Snap small values to 0 to avoid floating point classification issues
    // at solid boundaries (e.g., -0.0 being treated as outside when it should be on-boundary)
    let snap = |v: f64| if v.abs() < 1e-9 { 0.0 } else { v };
    let centroid = Point3::new(snap(centroid.x), snap(centroid.y), snap(centroid.z));

    // For planar faces, the centroid is already on the surface.
    // For curved faces, project back to the surface using the closest UV.
    let surface = &brep.geometry.surfaces[face.surface_index];
    match surface.surface_type() {
        SurfaceKind::Plane => centroid,
        SurfaceKind::Cylinder => {
            // For cylindrical faces, compute a point ON the surface at the middle
            // of the face's U (angular) range. The boundary vertex centroid may
            // be inside the cylinder, not on its surface, leading to wrong classification.
            if let Some(cyl) = surface
                .as_any()
                .downcast_ref::<vcad_kernel_geom::CylinderSurface>()
            {
                use std::f64::consts::PI;

                let ref_dir = cyl.ref_dir.as_ref();
                let y_dir = cyl.y_dir();

                // Compute U angles for each boundary vertex
                let mut u_angles: Vec<f64> = vertices
                    .iter()
                    .map(|v| {
                        let d = *v - cyl.center;
                        let u = d.dot(&y_dir).atan2(d.dot(ref_dir));
                        if u < 0.0 { u + 2.0 * PI } else { u }
                    })
                    .collect();
                u_angles.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                u_angles.dedup_by(|a, b| (*a - *b).abs() < 0.01);

                if u_angles.len() >= 2 {
                    let u_min = u_angles[0];
                    let u_max = u_angles[u_angles.len() - 1];

                    // Check if face wraps around 2π (gap between max and min is small)
                    let direct_span = u_max - u_min;
                    let wrap_span = 2.0 * PI - direct_span;

                    let u_mid = if wrap_span < direct_span {
                        // Face wraps around: use midpoint of the wrap region
                        let mid = (u_max + u_min + 2.0 * PI) / 2.0;
                        if mid >= 2.0 * PI { mid - 2.0 * PI } else { mid }
                    } else {
                        // Normal face: use midpoint of direct span
                        (u_min + u_max) / 2.0
                    };

                    // Compute V (height) at centroid
                    let v_mid = (centroid - cyl.center).dot(cyl.axis.as_ref());

                    // Evaluate point on cylinder surface
                    let sin_u = u_mid.sin();
                    let cos_u = u_mid.cos();
                    let radial = cyl.radius * (cos_u * ref_dir + sin_u * y_dir);
                    let sample = cyl.center + radial + v_mid * cyl.axis.as_ref();
                    return sample;
                }
            }
            centroid
        }
        SurfaceKind::Sphere => {
            // For spherical faces, the centroid of boundary vertices (poles) is at
            // the sphere center, not on the surface. We must compute a point that
            // actually lies ON the sphere surface.
            if let Some(sph) = surface
                .as_any()
                .downcast_ref::<vcad_kernel_geom::SphereSurface>()
            {
                use std::f64::consts::PI;

                let ref_dir = sph.ref_dir.as_ref();
                let y_dir = sph.axis.as_ref().cross(ref_dir);

                // For a full sphere face (degenerate outer loop with <=2 vertices),
                // pick a point on the equator away from poles.
                if vertices.len() <= 2 {
                    // Try multiple directions to find one not inside any inner loop hole
                    let num_tries = 36;
                    for try_idx in 0..num_tries {
                        let u = 2.0 * PI * (try_idx as f64) / (num_tries as f64);
                        let (sin_u, cos_u) = u.sin_cos();
                        // Sample at equator (v=0)
                        let candidate = sph.center
                            + sph.radius * (cos_u * ref_dir + sin_u * y_dir);

                        // Check if candidate is inside any inner loop hole
                        let mut in_hole = false;
                        for &inner_loop in &face.inner_loops {
                            let inner_verts: Vec<Point3> = topo
                                .loop_half_edges(inner_loop)
                                .map(|he_id| {
                                    topo.vertices[topo.half_edges[he_id].origin].point
                                })
                                .collect();
                            if inner_verts.len() >= 3 {
                                let n_iv = inner_verts.len() as f64;
                                let hole_center = Point3::new(
                                    inner_verts.iter().map(|v| v.x).sum::<f64>() / n_iv,
                                    inner_verts.iter().map(|v| v.y).sum::<f64>() / n_iv,
                                    inner_verts.iter().map(|v| v.z).sum::<f64>() / n_iv,
                                );
                                let hole_radius = inner_verts
                                    .iter()
                                    .map(|v| (*v - hole_center).norm())
                                    .fold(0.0f64, f64::max);
                                let to_pt = (candidate - sph.center).normalize();
                                let to_hole = (hole_center - sph.center).normalize();
                                let angle =
                                    to_pt.dot(&to_hole).clamp(-1.0, 1.0).acos();
                                let hole_angle =
                                    (hole_radius / sph.radius).clamp(0.0, 1.0).asin();
                                if angle < hole_angle + 1e-6 {
                                    in_hole = true;
                                    break;
                                }
                            }
                        }
                        if !in_hole {
                            return candidate;
                        }
                    }
                    // Fallback: equator at u=0
                    return sph.center + sph.radius * ref_dir;
                }

                // For partial sphere faces with enough vertices, project centroid
                // onto the sphere surface
                let dir = centroid - sph.center;
                let dir_len = dir.norm();
                if dir_len > 1e-12 {
                    return sph.center + sph.radius * dir / dir_len;
                }
                // Fallback: equator point
                sph.center + sph.radius * ref_dir
            } else {
                centroid
            }
        }
        SurfaceKind::Torus => {
            // For toroidal faces, compute a point ON the surface at the midpoint
            // of the face's U and V ranges.
            if let Some(torus) = surface
                .as_any()
                .downcast_ref::<vcad_kernel_geom::TorusSurface>()
            {
                use std::f64::consts::PI;

                let ref_dir = torus.ref_dir.as_ref();
                let y_dir = torus.axis.as_ref().cross(ref_dir);

                let mut u_angles: Vec<f64> = vertices
                    .iter()
                    .map(|v| {
                        let d = *v - torus.center;
                        let d_axis = d.dot(torus.axis.as_ref());
                        let d_plane = d - d_axis * torus.axis.into_inner();
                        let d_plane_len = d_plane.norm();
                        if d_plane_len < 1e-12 {
                            return 0.0;
                        }
                        let d_plane_norm = d_plane / d_plane_len;
                        let u = d_plane_norm.dot(&y_dir).atan2(d_plane_norm.dot(ref_dir));
                        if u < 0.0 { u + 2.0 * PI } else { u }
                    })
                    .collect();
                u_angles.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                u_angles.dedup_by(|a, b| (*a - *b).abs() < 0.01);

                let mut v_angles: Vec<f64> = vertices
                    .iter()
                    .map(|v| {
                        let d = *v - torus.center;
                        let d_axis = d.dot(torus.axis.as_ref());
                        let d_plane = d - d_axis * torus.axis.into_inner();
                        let d_plane_len = d_plane.norm();
                        let radial = d_plane_len - torus.major_radius;
                        let v = d_axis.atan2(radial);
                        if v < 0.0 { v + 2.0 * PI } else { v }
                    })
                    .collect();
                v_angles.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                v_angles.dedup_by(|a, b| (*a - *b).abs() < 0.01);

                if u_angles.len() >= 2 && v_angles.len() >= 2 {
                    let u_min = u_angles[0];
                    let u_max = u_angles[u_angles.len() - 1];
                    let direct_u = u_max - u_min;
                    let wrap_u = 2.0 * PI - direct_u;
                    let u_mid = if wrap_u < direct_u {
                        let mid = (u_max + u_min + 2.0 * PI) / 2.0;
                        if mid >= 2.0 * PI { mid - 2.0 * PI } else { mid }
                    } else {
                        (u_min + u_max) / 2.0
                    };

                    let v_min = v_angles[0];
                    let v_max = v_angles[v_angles.len() - 1];
                    let direct_v = v_max - v_min;
                    let wrap_v = 2.0 * PI - direct_v;
                    let v_mid = if wrap_v < direct_v {
                        let mid = (v_max + v_min + 2.0 * PI) / 2.0;
                        if mid >= 2.0 * PI { mid - 2.0 * PI } else { mid }
                    } else {
                        (v_min + v_max) / 2.0
                    };

                    return surface.evaluate(vcad_kernel_math::Point2::new(u_mid, v_mid));
                }
            }
            centroid
        }
        _ => {
            // For other curved surfaces, the centroid of boundary vertices may not
            // lie on the surface. We use it as-is for classification since
            // we only need it to be "near" the face for ray-casting.
            centroid
        }
    }
}

/// Classify a face of one solid relative to another solid.
///
/// The `other_mesh` is the tessellated mesh of the other solid, used
/// for point-in-solid testing.
pub fn classify_face(
    brep: &BRepSolid,
    face_id: FaceId,
    other_mesh: &TriangleMesh,
) -> FaceClassification {
    let sample = face_sample_point(brep, face_id);

    // Offset the sample point slightly along the face normal
    // to avoid landing exactly on the boundary
    let face = &brep.topology.faces[face_id];
    let surface = &brep.geometry.surfaces[face.surface_index];

    // Compute the outward normal from the loop vertex winding.
    // By B-rep convention, loop vertices are ordered so that (v1-v0) × (v2-v0)
    // points outward from the solid. This is more reliable than using the
    // surface normal + orientation, which may not correctly indicate outward.
    let outer_verts: Vec<Point3> = brep
        .topology
        .loop_half_edges(face.outer_loop)
        .map(|he_id| brep.topology.vertices[brep.topology.half_edges[he_id].origin].point)
        .collect();

    let oriented_normal = if outer_verts.len() >= 3 {
        let e1 = outer_verts[1] - outer_verts[0];
        let e2 = outer_verts[2] - outer_verts[0];
        let n = e1.cross(&e2);
        if n.norm() > 1e-15 {
            n.normalize()
        } else {
            // Degenerate polygon — fall back to surface normal with orientation
            let sn = surface.normal(vcad_kernel_math::Point2::origin());
            let normal = *sn.as_ref();
            match face.orientation {
                vcad_kernel_topo::Orientation::Forward => normal,
                vcad_kernel_topo::Orientation::Reversed => -normal,
            }
        }
    } else {
        // Too few vertices — fall back to surface normal with orientation
        let sn = surface.normal(vcad_kernel_math::Point2::origin());
        let normal = *sn.as_ref();
        match face.orientation {
            vcad_kernel_topo::Orientation::Forward => normal,
            vcad_kernel_topo::Orientation::Reversed => -normal,
        }
    };

    // Test the sample point offset slightly inward (negative normal)
    let eps = 1e-4;
    let inward_point = sample - eps * oriented_normal;

    let is_inside = point_in_mesh(&inward_point, other_mesh);

    if is_inside {
        FaceClassification::Inside
    } else {
        FaceClassification::Outside
    }
}

/// Classify all faces of a solid relative to another solid.
pub fn classify_all_faces(
    brep: &BRepSolid,
    other: &BRepSolid,
    segments: u32,
) -> Vec<(FaceId, FaceClassification)> {
    let other_mesh = tessellate_brep(other, segments);
    classify_all_faces_with_mesh(brep, &other_mesh)
}

/// Classify all faces of a solid against a pre-tessellated mesh of the other solid.
///
/// This avoids re-tessellating when the same mesh is needed for multiple calls.
pub fn classify_all_faces_with_mesh(
    brep: &BRepSolid,
    other_mesh: &TriangleMesh,
) -> Vec<(FaceId, FaceClassification)> {
    brep.topology
        .faces
        .iter()
        .map(|(face_id, _)| {
            let class = classify_face(brep, face_id, other_mesh);
            (face_id, class)
        })
        .collect()
}

/// Select which faces to keep from each solid based on the boolean operation.
///
/// Returns `(faces_from_a, faces_from_b, reverse_b)`.
/// `reverse_b` indicates that B's kept faces should have their orientation flipped.
pub fn select_faces(
    op: BooleanOp,
    classes_a: &[(FaceId, FaceClassification)],
    classes_b: &[(FaceId, FaceClassification)],
) -> (Vec<FaceId>, Vec<FaceId>, bool) {
    let keep_a: Vec<FaceId> = classes_a
        .iter()
        .filter(|(_, c)| match op {
            BooleanOp::Union => {
                matches!(c, FaceClassification::Outside | FaceClassification::OnSame)
            }
            BooleanOp::Difference => {
                matches!(
                    c,
                    FaceClassification::Outside | FaceClassification::OnOpposite
                )
            }
            BooleanOp::Intersection => {
                matches!(c, FaceClassification::Inside | FaceClassification::OnSame)
            }
        })
        .map(|(f, _)| *f)
        .collect();

    let keep_b: Vec<FaceId> = classes_b
        .iter()
        .filter(|(_, c)| match op {
            BooleanOp::Union => matches!(c, FaceClassification::Outside),
            BooleanOp::Difference => matches!(c, FaceClassification::Inside),
            BooleanOp::Intersection => matches!(c, FaceClassification::Inside),
        })
        .map(|(f, _)| *f)
        .collect();

    let reverse_b = matches!(op, BooleanOp::Difference);

    (keep_a, keep_b, reverse_b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_kernel_primitives::make_cube;

    #[test]
    fn test_face_sample_point_cube() {
        let brep = make_cube(10.0, 10.0, 10.0);
        // Each face's sample point should be on one of the cube faces
        for (face_id, _) in &brep.topology.faces {
            let sample = face_sample_point(&brep, face_id);
            // The point should be within the cube's extent
            assert!(sample.x >= -0.1 && sample.x <= 10.1);
            assert!(sample.y >= -0.1 && sample.y <= 10.1);
            assert!(sample.z >= -0.1 && sample.z <= 10.1);
        }
    }

    #[test]
    fn test_classify_non_overlapping() {
        // Cube A at origin, cube B far away
        let a = make_cube(10.0, 10.0, 10.0);
        let mut b = make_cube(10.0, 10.0, 10.0);
        for (_, v) in &mut b.topology.vertices {
            v.point.x += 100.0;
        }

        let classes = classify_all_faces(&a, &b, 32);
        // All faces of A should be Outside relative to B
        for (_, class) in &classes {
            assert_eq!(*class, FaceClassification::Outside);
        }
    }

    #[test]
    fn test_classify_inside() {
        // Small cube inside a larger cube
        let small = make_cube(2.0, 2.0, 2.0);
        let mut big = make_cube(10.0, 10.0, 10.0);
        // Move big so small is inside it (small is at 0-2, big at -1 to 9)
        for (_, v) in &mut big.topology.vertices {
            v.point.x -= 1.0;
            v.point.y -= 1.0;
            v.point.z -= 1.0;
        }

        let classes = classify_all_faces(&small, &big, 32);
        // All faces of small should be Inside relative to big
        for (_, class) in &classes {
            assert_eq!(*class, FaceClassification::Inside);
        }
    }

    #[test]
    fn test_select_union() {
        let classes_a = vec![
            // Simulate: some faces outside, some inside
        ];
        let classes_b = vec![];
        let (keep_a, keep_b, reverse_b) = select_faces(BooleanOp::Union, &classes_a, &classes_b);
        assert!(keep_a.is_empty());
        assert!(keep_b.is_empty());
        assert!(!reverse_b);
    }

    #[test]
    fn test_select_difference_reverses_b() {
        let classes_a: Vec<(FaceId, FaceClassification)> = vec![];
        let classes_b: Vec<(FaceId, FaceClassification)> = vec![];
        let (_, _, reverse_b) = select_faces(BooleanOp::Difference, &classes_a, &classes_b);
        assert!(reverse_b);
    }
}
