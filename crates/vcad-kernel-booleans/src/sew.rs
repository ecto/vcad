//! Topology reconstruction — sew selected faces into a result solid.
//!
//! After classification, we have sets of faces from both solids to keep.
//! This module copies those faces into a new BRepSolid, merging vertices
//! within tolerance and building proper shell/solid topology.
//!
//! # Important Implementation Note: Face Orientation Reversal
//!
//! When performing a boolean difference (A - B), faces from B that are kept
//! need their normals flipped to point into the resulting solid (they become
//! interior walls of holes/cavities).
//!
//! ## The Bug That Was Fixed
//!
//! There are two ways to flip a face's effective normal:
//! 1. Flip the `orientation` field (Forward ↔ Reversed)
//! 2. Reverse the loop vertex order (changes winding, which changes computed normal)
//!
//! The old code did BOTH when `reverse_b=true`:
//! - `copy_loop` reversed vertex order → normal flipped
//! - `copy_faces` flipped orientation → normal flipped again
//!
//! Two flips cancel out! The faces ended up with their original normal direction,
//! pointing OUTWARD from the hole instead of INWARD. This caused:
//! - Hole walls to contribute POSITIVE volume instead of negative
//! - Result volume was 29088 instead of expected 27936
//! - Visual artifacts (faces rendering incorrectly)
//!
//! ## The Fix
//!
//! Only flip the orientation field, NOT the loop vertex order. The orientation
//! field is the proper B-rep mechanism for controlling face normal direction
//! relative to the underlying surface.
//!
//! ```text
//! // WRONG (double flip = no flip):
//! copy_loop(..., reverse=true)  // flips winding
//! orientation = !orientation    // flips again → back to original
//!
//! // CORRECT (single flip):
//! copy_loop(..., reverse=false) // preserve winding
//! orientation = !orientation    // flips once → correct direction
//! ```

use vcad_kernel_geom::{GeometryStore, SurfaceKind};
use vcad_kernel_math::{Point3, Vec3};
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_topo::{FaceId, Orientation, ShellType, Topology};

use std::collections::HashMap;

use crate::repair;

/// Represents a plane equation: normal · point = d
#[derive(Debug, Clone)]
struct PlaneEq {
    normal: Vec3,
    d: f64,
}

impl PlaneEq {
    /// Create a plane equation from a point and normal.
    fn from_point_normal(point: &Point3, normal: &Vec3) -> Self {
        let d = normal.x * point.x + normal.y * point.y + normal.z * point.z;
        Self { normal: *normal, d }
    }

    /// Check if another plane is coplanar with this one (same plane, possibly opposite normal).
    /// Returns Some(true) if same normal direction, Some(false) if opposite, None if not coplanar.
    fn coplanar_with(&self, other: &PlaneEq, tol: f64) -> Option<bool> {
        // Check if normals are parallel (same or opposite direction)
        let dot = self.normal.dot(other.normal);
        let same_dir = dot > 1.0 - tol;
        let opp_dir = dot < -1.0 + tol;

        if !same_dir && !opp_dir {
            return None; // Not parallel
        }

        // Check if planes coincide (same distance from origin, accounting for sign)
        let d_diff = if same_dir {
            (self.d - other.d).abs()
        } else {
            (self.d + other.d).abs()
        };

        if d_diff > tol {
            return None; // Parallel but not coplanar
        }

        Some(same_dir)
    }
}

/// Sew selected faces from two solids into a new result solid.
///
/// - `faces_a`: Face IDs to keep from solid A (in A's topology)
/// - `faces_b`: Face IDs to keep from solid B (in B's topology)
/// - `reverse_b`: If true, flip the orientation of B's faces (for difference)
/// - `tolerance`: Vertex merge distance
pub fn sew_faces(
    a: &BRepSolid,
    faces_a: &[FaceId],
    b: &BRepSolid,
    faces_b: &[FaceId],
    reverse_b: bool,
    tolerance: f64,
) -> BRepSolid {
    let mut topo = Topology::new();
    let mut geom = GeometryStore::new();

    // Copy faces from A
    let _a_face_map = copy_faces(a, faces_a, false, &mut topo, &mut geom);

    // For difference operations, B faces that are coplanar with A faces should NOT
    // be reversed. These are typically end caps (like cylinder caps) that lie on
    // A's boundary and should face the same direction as the A faces they're adjacent to.
    //
    // Split B faces into two groups: coplanar faces (not reversed) and regular faces (reversed).
    if reverse_b {
        // Collect plane equations for all planar A faces
        let a_planes: Vec<PlaneEq> = faces_a
            .iter()
            .filter_map(|&face_id| {
                let face = &a.topology.faces[face_id];
                let surface = &a.geometry.surfaces[face.surface_index];
                if surface.surface_type() != SurfaceKind::Plane {
                    return None;
                }

                // Get a point on the face and its normal
                let plane = surface.as_any().downcast_ref::<vcad_kernel_geom::Plane>()?;
                let normal = plane.normal_dir.as_ref();
                // Account for face orientation
                let effective_normal = match face.orientation {
                    Orientation::Forward => *normal,
                    Orientation::Reversed => -*normal,
                };
                let plane_eq = PlaneEq::from_point_normal(&plane.origin, &effective_normal);
                Some(plane_eq)
            })
            .collect();

        // Separate B faces into coplanar-same-direction (don't reverse) and others (reverse)
        let mut b_faces_no_reverse: Vec<FaceId> = Vec::new();
        let mut b_faces_reverse: Vec<FaceId> = Vec::new();

        for &face_id in faces_b {
            let face = &b.topology.faces[face_id];
            let surface = &b.geometry.surfaces[face.surface_index];

            if surface.surface_type() == SurfaceKind::Plane {
                if let Some(plane) = surface.as_any().downcast_ref::<vcad_kernel_geom::Plane>() {
                    let b_normal = plane.normal_dir.as_ref();
                    let effective_normal = match face.orientation {
                        Orientation::Forward => *b_normal,
                        Orientation::Reversed => -*b_normal,
                    };
                    let b_plane = PlaneEq::from_point_normal(&plane.origin, &effective_normal);

                    // Check if this B face is coplanar with any A face
                    let mut is_coplanar_same = false;
                    for a_plane in &a_planes {
                        if let Some(same_dir) = a_plane.coplanar_with(&b_plane, 1e-6) {
                            if same_dir {
                                // Coplanar with same normal direction - don't reverse
                                is_coplanar_same = true;
                                break;
                            }
                        }
                    }

                    if is_coplanar_same {
                        b_faces_no_reverse.push(face_id);
                    } else {
                        b_faces_reverse.push(face_id);
                    }
                } else {
                    b_faces_reverse.push(face_id);
                }
            } else {
                // Non-planar faces (like cylinder walls) always get reversed
                b_faces_reverse.push(face_id);
            }
        }

        // Copy B faces: coplanar ones without reversal, others with reversal
        let _b_face_map_no_rev = copy_faces(b, &b_faces_no_reverse, false, &mut topo, &mut geom);
        let _b_face_map_rev = copy_faces(b, &b_faces_reverse, true, &mut topo, &mut geom);
    } else {
        // No reversal needed - copy all B faces normally
        let _b_face_map = copy_faces(b, faces_b, false, &mut topo, &mut geom);
    }

    // Merge vertices within tolerance
    merge_nearby_vertices(&mut topo, tolerance);

    // Repair topology issues after merges
    repair::repair_topology(&mut topo, tolerance);

    // Build shell from all faces
    let all_faces: Vec<FaceId> = topo.faces.keys().collect();
    if all_faces.is_empty() {
        let shell = topo.add_shell(Vec::new(), ShellType::Outer);
        let solid = topo.add_solid(shell);
        return BRepSolid {
            topology: topo,
            geometry: geom,
            solid_id: solid,
        };
    }

    let shell = topo.add_shell(all_faces, ShellType::Outer);
    let solid = topo.add_solid(shell);

    BRepSolid {
        topology: topo,
        geometry: geom,
        solid_id: solid,
    }
}

/// Copy selected faces from a source BRep into the target topology/geometry.
///
/// Returns a mapping from source FaceId to new FaceId.
fn copy_faces(
    source: &BRepSolid,
    face_ids: &[FaceId],
    reverse_orientation: bool,
    target_topo: &mut Topology,
    target_geom: &mut GeometryStore,
) -> HashMap<FaceId, FaceId> {
    use vcad_kernel_topo::HalfEdgeId;

    let mut face_map = HashMap::new();
    // Map from source VertexId position hash → target VertexId
    // We use positions since VertexIds from different topologies aren't comparable
    let mut vertex_map: HashMap<VertexPosKey, vcad_kernel_topo::VertexId> = HashMap::new();
    // Map from source surface index → target surface index
    let mut surface_map: HashMap<usize, usize> = HashMap::new();
    // Map from source HalfEdgeId → target HalfEdgeId (to preserve twin relationships)
    let mut he_map: HashMap<HalfEdgeId, HalfEdgeId> = HashMap::new();

    for &src_face_id in face_ids {
        let src_face = &source.topology.faces[src_face_id];
        let src_surface_idx = src_face.surface_index;

        // Copy surface if not already copied
        let tgt_surface_idx = *surface_map.entry(src_surface_idx).or_insert_with(|| {
            let surface = source.geometry.surfaces[src_surface_idx].clone();
            target_geom.add_surface(surface)
        });

        // Copy outer loop vertices and half-edges.
        //
        // CRITICAL: We do NOT reverse the loop vertices here, even when reverse_orientation=true.
        // The orientation field flip alone is sufficient to reverse the face normal.
        // If we also reversed the loop vertices, we'd double-flip and end up with the
        // original normal direction (see module docs for detailed explanation).
        //
        // This was a bug that caused boolean difference to produce wrong results:
        // hole walls pointed outward instead of inward, adding volume instead of subtracting.
        let tgt_outer_loop = copy_loop_with_he_map(
            source,
            src_face.outer_loop,
            false, // NEVER reverse loop - orientation flip handles normal reversal
            target_topo,
            &mut vertex_map,
            &mut he_map,
        );

        // Determine orientation
        let orientation = if reverse_orientation {
            match src_face.orientation {
                Orientation::Forward => Orientation::Reversed,
                Orientation::Reversed => Orientation::Forward,
            }
        } else {
            src_face.orientation
        };

        let tgt_face = target_topo.add_face(tgt_outer_loop, tgt_surface_idx, orientation);

        // Copy inner loops
        for &inner_loop in &src_face.inner_loops {
            let tgt_inner = copy_loop_with_he_map(
                source,
                inner_loop,
                false, // Never reverse loop for orientation flip
                target_topo,
                &mut vertex_map,
                &mut he_map,
            );
            target_topo.add_inner_loop(tgt_face, tgt_inner);
        }

        face_map.insert(src_face_id, tgt_face);
    }

    // Now link twins for half-edges that were twins in the source
    // This preserves topology for edges that are fully within the copied faces
    // Sorted: twin linking is first-come-wins (the guards below skip a
    // half-edge that already has a twin), so HashMap order would decide
    // which pairing survives — and with it the whole sewn result.
    let mut he_pairs: Vec<_> = he_map.iter().collect();
    he_pairs.sort_by_key(|(he, _)| **he);
    for (src_he, tgt_he) in he_pairs {
        // Skip if already has twin (might have been set by repair)
        if target_topo.half_edges[*tgt_he].twin.is_some() {
            continue;
        }

        // Check if source had a twin
        if let Some(src_twin) = source.topology.half_edges[*src_he].twin {
            // Check if the twin was also copied
            if let Some(&tgt_twin) = he_map.get(&src_twin) {
                // Only link if target twin also doesn't have a twin yet
                if target_topo.half_edges[tgt_twin].twin.is_none() {
                    target_topo.add_edge(*tgt_he, tgt_twin);
                }
            }
        }
    }

    face_map
}

/// Copy a loop from source to target topology, tracking half-edge mapping.
///
/// Returns the new LoopId in the target topology.
fn copy_loop_with_he_map(
    source: &BRepSolid,
    src_loop: vcad_kernel_topo::LoopId,
    reverse: bool,
    target: &mut Topology,
    vertex_map: &mut HashMap<VertexPosKey, vcad_kernel_topo::VertexId>,
    he_map: &mut HashMap<vcad_kernel_topo::HalfEdgeId, vcad_kernel_topo::HalfEdgeId>,
) -> vcad_kernel_topo::LoopId {
    let src_topo = &source.topology;

    // Collect half-edges in order
    let src_hes: Vec<_> = src_topo.loop_half_edges(src_loop).collect();

    // Get vertices in order
    let mut vert_ids: Vec<vcad_kernel_topo::VertexId> = src_hes
        .iter()
        .map(|&he| {
            let src_v = src_topo.half_edges[he].origin;
            let pos = src_topo.vertices[src_v].point;
            let key = VertexPosKey::from_point(&pos);

            *vertex_map
                .entry(key)
                .or_insert_with(|| target.add_vertex(pos))
        })
        .collect();

    if reverse {
        vert_ids.reverse();
    }

    // Create half-edges and track mapping
    let hes: Vec<_> = vert_ids.iter().map(|&v| target.add_half_edge(v)).collect();

    // Record the half-edge mapping (source → target)
    if reverse {
        // When reversed, the correspondence is reversed too
        for (i, &src_he) in src_hes.iter().enumerate() {
            let tgt_idx = src_hes.len() - 1 - i;
            he_map.insert(src_he, hes[tgt_idx]);
        }
    } else {
        for (src_he, &tgt_he) in src_hes.iter().zip(hes.iter()) {
            he_map.insert(*src_he, tgt_he);
        }
    }

    // Create loop
    target.add_loop(&hes)
}

/// Key for vertex position hashing (for deduplication).
///
/// Uses quantized coordinates to handle floating-point imprecision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct VertexPosKey {
    x: i64,
    y: i64,
    z: i64,
}

impl VertexPosKey {
    fn from_point(p: &Point3) -> Self {
        // Quantize to ~1e-8 resolution
        let scale = 1e8;
        Self {
            x: (p.x * scale).round() as i64,
            y: (p.y * scale).round() as i64,
            z: (p.z * scale).round() as i64,
        }
    }
}

/// Merge vertices that are within tolerance of each other.
///
/// After merging, half-edges pointing to the merged-away vertex
/// are updated to point to the surviving vertex.
fn merge_nearby_vertices(topo: &mut Topology, tolerance: f64) {
    let tol2 = tolerance * tolerance;

    // Collect all vertex IDs and positions
    let verts: Vec<(vcad_kernel_topo::VertexId, Point3)> =
        topo.vertices.iter().map(|(id, v)| (id, v.point)).collect();

    // Build merge map: vertex_to_remove → vertex_to_keep
    let mut merge_map: HashMap<vcad_kernel_topo::VertexId, vcad_kernel_topo::VertexId> =
        HashMap::new();

    for i in 0..verts.len() {
        if merge_map.contains_key(&verts[i].0) {
            continue;
        }
        for j in (i + 1)..verts.len() {
            if merge_map.contains_key(&verts[j].0) {
                continue;
            }
            let dist2 = (verts[i].1 - verts[j].1).norm_squared();
            if dist2 < tol2 {
                merge_map.insert(verts[j].0, verts[i].0);
            }
        }
    }

    if merge_map.is_empty() {
        return;
    }

    // Update all half-edges to use the surviving vertex
    let he_ids: Vec<_> = topo.half_edges.keys().collect();
    for he_id in he_ids {
        let origin = topo.half_edges[he_id].origin;
        if let Some(&target) = merge_map.get(&origin) {
            topo.half_edges[he_id].origin = target;
        }
    }

    // Remove merged vertices
    for v_id in merge_map.keys() {
        topo.vertices.remove(*v_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_kernel_primitives::make_cube;

    #[test]
    fn test_sew_non_overlapping() {
        // Two separate cubes — union should have all 12 faces
        let a = make_cube(10.0, 10.0, 10.0);
        let mut b = make_cube(10.0, 10.0, 10.0);
        for (_, v) in &mut b.topology.vertices {
            v.point.x += 100.0;
        }

        let faces_a: Vec<FaceId> = a.topology.faces.keys().collect();
        let faces_b: Vec<FaceId> = b.topology.faces.keys().collect();

        let result = sew_faces(&a, &faces_a, &b, &faces_b, false, 1e-6);
        assert_eq!(result.topology.faces.len(), 12); // 6 + 6
    }

    #[test]
    fn test_sew_with_reverse() {
        let a = make_cube(10.0, 10.0, 10.0);
        let faces_a: Vec<FaceId> = a.topology.faces.keys().collect();

        // Sew A's faces with reversed B (empty B)
        let result = sew_faces(&a, &faces_a, &a, &[], true, 1e-6);
        assert_eq!(result.topology.faces.len(), 6);
    }

    #[test]
    fn test_vertex_merge() {
        let mut topo = Topology::new();

        // Two vertices at nearly the same position
        let v1 = topo.add_vertex(Point3::new(1.0, 2.0, 3.0));
        let v2 = topo.add_vertex(Point3::new(1.0 + 1e-8, 2.0, 3.0));
        let v3 = topo.add_vertex(Point3::new(10.0, 20.0, 30.0));

        // Create half-edges so we can verify the merge
        let he1 = topo.add_half_edge(v1);
        let he2 = topo.add_half_edge(v2);
        let he3 = topo.add_half_edge(v3);

        merge_nearby_vertices(&mut topo, 1e-6);

        // v1 and v2 should have been merged
        assert_eq!(topo.vertices.len(), 2);

        // he1 and he2 should now point to the same vertex
        assert_eq!(topo.half_edges[he1].origin, topo.half_edges[he2].origin);

        // he3 should still point to v3
        assert_eq!(topo.half_edges[he3].origin, v3);
    }

    #[test]
    fn test_sew_empty() {
        let a = make_cube(10.0, 10.0, 10.0);
        let result = sew_faces(&a, &[], &a, &[], false, 1e-6);
        assert_eq!(result.topology.faces.len(), 0);
    }

    #[test]
    fn test_sew_preserves_edges() {
        // Two separate cubes — all half-edges should have parent edges after sewing
        let a = make_cube(10.0, 10.0, 10.0);
        let mut b = make_cube(10.0, 10.0, 10.0);
        for (_, v) in &mut b.topology.vertices {
            v.point.x += 100.0;
        }

        let faces_a: Vec<FaceId> = a.topology.faces.keys().collect();
        let faces_b: Vec<FaceId> = b.topology.faces.keys().collect();

        let result = sew_faces(&a, &faces_a, &b, &faces_b, false, 1e-6);

        // Count half-edges without parent edges
        let mut orphan_count = 0;
        for (he_id, he) in &result.topology.half_edges {
            if he.loop_id.is_some() && he.edge.is_none() {
                eprintln!(
                    "Orphan half-edge {:?}: origin={:?}, next={:?}, prev={:?}, twin={:?}",
                    he_id, he.origin, he.next, he.prev, he.twin
                );
                orphan_count += 1;
            }
        }

        assert_eq!(
            orphan_count, 0,
            "Found {} half-edges without parent edges",
            orphan_count
        );
    }

    #[test]
    fn test_sew_cylinders_preserves_edges() {
        use vcad_kernel_primitives::make_cylinder;

        // Two separate cylinders — all half-edges should have parent edges after sewing
        let a = make_cylinder(3.0, 10.0, 32);
        let mut b = make_cylinder(3.0, 10.0, 32);
        for (_, v) in &mut b.topology.vertices {
            v.point.x += 100.0;
        }

        let faces_a: Vec<FaceId> = a.topology.faces.keys().collect();
        let faces_b: Vec<FaceId> = b.topology.faces.keys().collect();

        eprintln!(
            "Cylinder A: {} faces, {} half-edges",
            a.topology.faces.len(),
            a.topology.half_edges.len()
        );
        eprintln!(
            "Cylinder B: {} faces, {} half-edges",
            b.topology.faces.len(),
            b.topology.half_edges.len()
        );

        let result = sew_faces(&a, &faces_a, &b, &faces_b, false, 1e-6);

        eprintln!(
            "Result: {} faces, {} half-edges, {} edges",
            result.topology.faces.len(),
            result.topology.half_edges.len(),
            result.topology.edges.len()
        );

        // Count half-edges without parent edges
        let mut orphan_count = 0;
        for (he_id, he) in &result.topology.half_edges {
            if he.loop_id.is_some() && he.edge.is_none() {
                let origin_pos = result.topology.vertices[he.origin].point;
                eprintln!(
                    "Orphan half-edge {:?}: origin={:?} pos=({:.2},{:.2},{:.2}), twin={:?}",
                    he_id, he.origin, origin_pos.x, origin_pos.y, origin_pos.z, he.twin
                );
                orphan_count += 1;
            }
        }

        assert_eq!(
            orphan_count, 0,
            "Found {} half-edges without parent edges",
            orphan_count
        );
    }
}
