//! B-rep boolean pipeline - face splitting, classification, and sewing.

use std::collections::HashMap;

use rayon::prelude::*;
use vcad_kernel_math::Point3;
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_tessellate::{tessellate_brep, TriangleMesh};
use vcad_kernel_topo::FaceId;

use crate::api::{BooleanOp, BooleanResult};
use crate::{bbox, classify, sew, split, ssi, trim};

/// Per-face split data: intersection curve with entry/exit points.
type FaceSplits = Vec<(ssi::IntersectionCurve, Point3, Point3)>;

/// Result of computing SSI for one face pair: splits for face A and face B.
type SsiPairResult = (FaceId, FaceSplits, FaceId, FaceSplits);

/// Debug logging macro - only prints when debug-boolean feature is enabled
#[allow(unused_macros)]
#[cfg(feature = "debug-boolean")]
macro_rules! debug_bool {
    ($($arg:tt)*) => {
        eprintln!($($arg)*)
    };
}

/// No-op version when debug-boolean feature is disabled
#[allow(unused_macros)]
#[cfg(not(feature = "debug-boolean"))]
macro_rules! debug_bool {
    ($($arg:tt)*) => {};
}

/// Handle boolean operations on non-overlapping solids.
pub(crate) fn non_overlapping_boolean(
    solid_a: &BRepSolid,
    solid_b: &BRepSolid,
    op: BooleanOp,
    _segments: u32,
) -> BooleanResult {
    match op {
        BooleanOp::Union => {
            // Union of non-overlapping = both solids combined
            let faces_a: Vec<_> = solid_a.topology.faces.keys().collect();
            let faces_b: Vec<_> = solid_b.topology.faces.keys().collect();
            let result = sew::sew_faces(solid_a, &faces_a, solid_b, &faces_b, false, 1e-6);
            BooleanResult::BRep(Box::new(result))
        }
        BooleanOp::Difference => {
            // Difference with non-overlapping = just A (nothing to subtract)
            let faces_a: Vec<_> = solid_a.topology.faces.keys().collect();
            let result = sew::sew_faces(solid_a, &faces_a, solid_b, &[], false, 1e-6);
            BooleanResult::BRep(Box::new(result))
        }
        BooleanOp::Intersection => {
            // Intersection of non-overlapping = empty
            BooleanResult::Mesh(TriangleMesh {
                vertices: Vec::new(),
                indices: Vec::new(),
                normals: Vec::new(),
                face_kinds: Vec::new(),
            })
        }
    }
}

/// Snap a value to 0 if it's within epsilon of 0.
/// This prevents floating point errors like -0.0000001 from affecting classification.
fn snap_to_zero(v: f64, eps: f64) -> f64 {
    if v.abs() < eps {
        0.0
    } else {
        v
    }
}

/// Snap a point's coordinates to 0 if they're very close to 0.
fn snap_point(p: Point3) -> Point3 {
    const EPS: f64 = 1e-9;
    Point3::new(
        snap_to_zero(p.x, EPS),
        snap_to_zero(p.y, EPS),
        snap_to_zero(p.z, EPS),
    )
}

/// Evaluate a point on an intersection curve at parameter t.
fn evaluate_curve(curve: &ssi::IntersectionCurve, t: f64) -> Point3 {
    let p = match curve {
        ssi::IntersectionCurve::Line(line) => line.origin + t * line.direction,
        ssi::IntersectionCurve::TwoLines(line1, _line2) => {
            // For TwoLines, evaluate on the first line by default
            // Caller should expand TwoLines before calling this
            line1.origin + t * line1.direction
        }
        ssi::IntersectionCurve::Circle(c) => {
            let (sin_t, cos_t) = t.sin_cos();
            c.center + c.radius * (cos_t * c.x_dir.into_inner() + sin_t * c.y_dir.into_inner())
        }
        ssi::IntersectionCurve::Point(p) => *p,
        ssi::IntersectionCurve::Sampled(points) => {
            if points.is_empty() {
                return Point3::origin();
            }
            // Linear interpolation along sampled curve
            let idx = ((t * (points.len() - 1) as f64).floor() as usize).min(points.len() - 2);
            let frac = t * (points.len() - 1) as f64 - idx as f64;
            let p0 = points[idx];
            let p1 = points[idx + 1];
            Point3::new(
                p0.x + frac * (p1.x - p0.x),
                p0.y + frac * (p1.y - p0.y),
                p0.z + frac * (p1.z - p0.z),
            )
        }
        ssi::IntersectionCurve::Empty => Point3::origin(),
    };
    // Snap small values to exactly 0 to avoid floating point classification issues
    snap_point(p)
}

/// Apply splits from intersection curves to solid A.
fn apply_splits_to_solid(
    solid: &mut BRepSolid,
    splits: HashMap<FaceId, FaceSplits>,
    segments: u32,
    #[allow(unused_variables)] solid_name: &str,
) {
    for (face_id, split_list) in splits {
        let mut current_faces = vec![face_id];
        for (curve, _entry, _exit) in split_list {
            let mut new_faces = Vec::new();
            for &fid in &current_faces {
                if solid.topology.faces.contains_key(fid) {
                    // Check if this is a conical face - route to appropriate splitting
                    if split::is_conical_face(solid, fid) {
                        debug_bool!(
                            "  Split {} conical face {:?} by {:?}",
                            solid_name,
                            fid,
                            match &curve {
                                ssi::IntersectionCurve::Line(l) => format!(
                                    "Line at ({:.2},{:.2},{:.2})",
                                    l.origin.x, l.origin.y, l.origin.z
                                ),
                                ssi::IntersectionCurve::Circle(c) => format!(
                                    "Circle at ({:.2},{:.2},{:.2}) r={:.2}",
                                    c.center.x, c.center.y, c.center.z, c.radius
                                ),
                                _ => format!("{:?}", curve),
                            }
                        );
                        let result = split::split_conical_face(solid, fid, &curve);
                        debug_bool!(
                            "    -> Conical split result: {} sub-faces {:?}",
                            result.sub_faces.len(),
                            result.sub_faces
                        );
                        if result.sub_faces.len() >= 2 {
                            new_faces.extend(result.sub_faces);
                        } else {
                            new_faces.push(fid);
                        }
                        continue;
                    }

                    // Check if this is a cylindrical face - use specialized split
                    if split::is_cylindrical_face(solid, fid) {
                        debug_bool!(
                            "  Split {} cylindrical face {:?} by {:?}",
                            solid_name,
                            fid,
                            match &curve {
                                ssi::IntersectionCurve::Line(l) => format!(
                                    "Line at ({:.2},{:.2},{:.2})",
                                    l.origin.x, l.origin.y, l.origin.z
                                ),
                                ssi::IntersectionCurve::Circle(c) => format!(
                                    "Circle at ({:.2},{:.2},{:.2}) r={:.2}",
                                    c.center.x, c.center.y, c.center.z, c.radius
                                ),
                                _ => format!("{:?}", curve),
                            }
                        );
                        let result = split::split_cylindrical_face(solid, fid, &curve);
                        debug_bool!(
                            "    -> Cylindrical split result: {} sub-faces {:?}",
                            result.sub_faces.len(),
                            result.sub_faces
                        );
                        if result.sub_faces.len() >= 2 {
                            new_faces.extend(result.sub_faces);
                        } else {
                            new_faces.push(fid);
                        }
                        continue;
                    }

                    // Check if this is a spherical face with a circle curve.
                    // Sphere-plane and sphere-sphere intersections produce circles.
                    // Add the circle as an inner loop (hole) on the sphere face,
                    // and create an inner disk face for classification.
                    if split::is_spherical_face(solid, fid) {
                        if let ssi::IntersectionCurve::Circle(_circle) = &curve {
                            debug_bool!(
                                "  Split {} spherical face {:?} by Circle at ({:.2},{:.2},{:.2}) r={:.2}",
                                solid_name,
                                fid,
                                _circle.center.x, _circle.center.y, _circle.center.z, _circle.radius
                            );
                            let result = split::split_spherical_face_by_circle(
                                solid, fid, _circle, segments,
                            );
                            debug_bool!(
                                "    -> Sphere split result: {} sub-faces {:?}",
                                result.sub_faces.len(),
                                result.sub_faces
                            );
                            if result.sub_faces.len() >= 2 {
                                new_faces.extend(result.sub_faces);
                            } else {
                                new_faces.push(fid);
                            }
                            continue;
                        }
                        // Non-circle curves on sphere faces: keep unchanged
                        new_faces.push(fid);
                        continue;
                    }

                    // Check if this is a toroidal face - use general trim+split path
                    // Torus faces have periodic UV in both u and v. The SSI
                    // (plane-torus or sampled) already produces appropriate curves.
                    // We route through the general re-trim path which now handles
                    // toroidal UV wrapping in point_in_face.
                    if split::is_toroidal_face(solid, fid) {
                        let segs = trim::trim_curve_to_face(&curve, fid, solid, 64);
                        debug_bool!(
                            "  Split {} toroidal face {:?}: re-trim got {} segs",
                            solid_name,
                            fid,
                            segs.len()
                        );
                        if segs.is_empty() {
                            new_faces.push(fid);
                            continue;
                        }
                        let seg = &segs[0];
                        let entry = evaluate_curve(&curve, seg.t_start);
                        let exit = evaluate_curve(&curve, seg.t_end);
                        let len = (exit - entry).norm();
                        if len < 1e-6 {
                            new_faces.push(fid);
                            continue;
                        }
                        let result = split::split_face_by_curve(solid, fid, &curve, &entry, &exit);
                        debug_bool!(
                            "    -> Toroidal split result: {} sub-faces {:?}",
                            result.sub_faces.len(),
                            result.sub_faces
                        );
                        if result.sub_faces.len() >= 2 {
                            new_faces.extend(result.sub_faces);
                        } else {
                            new_faces.push(fid);
                        }
                        continue;
                    }

                    // Check if this is a circular disk face (cylinder cap) with a line curve
                    if split::is_circular_disk_face(solid, fid) {
                        if let ssi::IntersectionCurve::Line(_line) = &curve {
                            debug_bool!(
                                "  Split {} circular disk face {:?} by Line at ({:.2},{:.2},{:.2})",
                                solid_name,
                                fid,
                                _line.origin.x,
                                _line.origin.y,
                                _line.origin.z
                            );
                            let result =
                                split::split_circular_disk_face(solid, fid, &curve, segments);
                            debug_bool!(
                                "    -> Disk split result: {} sub-faces {:?}",
                                result.sub_faces.len(),
                                result.sub_faces
                            );
                            if result.sub_faces.len() >= 2 {
                                new_faces.extend(result.sub_faces);
                            } else {
                                new_faces.push(fid);
                            }
                            continue;
                        }
                    }

                    // Check if this is a planar face with a circle curve - use specialized split
                    if split::is_planar_face(solid, fid) {
                        #[allow(unused_variables)]
                        if let ssi::IntersectionCurve::Circle(circle) = &curve {
                            debug_bool!(
                                "  Split {} face {:?}: planar + Circle at ({:.1},{:.1},{:.1}) r={:.1}",
                                solid_name,
                                fid,
                                circle.center.x,
                                circle.center.y,
                                circle.center.z,
                                circle.radius
                            );
                            let result = split::split_planar_face(
                                solid,
                                fid,
                                &curve,
                                &Point3::origin(),
                                &Point3::origin(),
                                segments,
                            );
                            debug_bool!(
                                "    -> Circle split result: {} sub-faces {:?}",
                                result.sub_faces.len(),
                                result.sub_faces
                            );
                            if result.sub_faces.len() >= 2 {
                                new_faces.extend(result.sub_faces);
                            } else {
                                new_faces.push(fid);
                            }
                            continue;
                        }

                        // Handle line curves on planar faces
                        if let ssi::IntersectionCurve::Line(_) = &curve {
                            let result = split::split_planar_face(
                                solid,
                                fid,
                                &curve,
                                &Point3::origin(),
                                &Point3::origin(),
                                segments,
                            );
                            if result.sub_faces.len() >= 2 {
                                new_faces.extend(result.sub_faces);
                            } else {
                                new_faces.push(fid);
                            }
                            continue;
                        }
                    }

                    // Re-trim the curve to THIS sub-face's boundary
                    let segs = trim::trim_curve_to_face(&curve, fid, solid, 64);
                    debug_bool!(
                        "  Split {} face {:?}: re-trim got {} segs",
                        solid_name,
                        fid,
                        segs.len()
                    );
                    if segs.is_empty() {
                        // Curve doesn't cross this face, keep it unchanged
                        debug_bool!("    -> empty segs, keeping face unchanged");
                        new_faces.push(fid);
                        continue;
                    }
                    // Use the first segment's trimmed entry/exit
                    let seg = &segs[0];
                    let entry = evaluate_curve(&curve, seg.t_start);
                    let exit = evaluate_curve(&curve, seg.t_end);
                    let len = (exit - entry).norm();
                    debug_bool!(
                        "    -> entry=({:.2},{:.2},{:.2}) exit=({:.2},{:.2},{:.2}) len={:.4}",
                        entry.x,
                        entry.y,
                        entry.z,
                        exit.x,
                        exit.y,
                        exit.z,
                        len
                    );
                    if len < 1e-6 {
                        debug_bool!("    -> too short, keeping face unchanged");
                        new_faces.push(fid);
                        continue;
                    }
                    let result = split::split_face_by_curve(solid, fid, &curve, &entry, &exit);
                    debug_bool!(
                        "    -> split result: {} sub-faces {:?}",
                        result.sub_faces.len(),
                        result.sub_faces
                    );
                    if result.sub_faces.len() >= 2 {
                        new_faces.extend(result.sub_faces);
                    } else {
                        new_faces.push(fid);
                    }
                }
            }
            if !new_faces.is_empty() {
                current_faces = new_faces;
            }
        }
    }
}

/// B-rep boolean pipeline for overlapping solids.
///
/// Handles general boolean operations by:
/// 1. Finding candidate face pairs via AABB
/// 2. Computing surface-surface intersections
/// 3. Splitting both A and B faces along intersection curves
/// 4. Classifying split sub-faces
/// 5. Selecting and sewing result faces
pub(crate) fn brep_boolean(
    solid_a: &BRepSolid,
    solid_b: &BRepSolid,
    op: BooleanOp,
    segments: u32,
) -> BooleanResult {
    debug_bool!("\n========== BREP BOOLEAN START ==========");
    debug_bool!("Operation: {:?}", op);
    debug_bool!("Solid A: {} faces", solid_a.topology.faces.len());
    debug_bool!("Solid B: {} faces", solid_b.topology.faces.len());

    // Clone both solids so we can split them
    let mut a = solid_a.clone();
    let mut b = solid_b.clone();

    // 1. Find candidate face pairs via AABB filtering
    let pairs = bbox::find_candidate_face_pairs(&a, &b);
    debug_bool!("\n--- Stage 1: AABB filtering ---");
    debug_bool!("Candidate face pairs: {}", pairs.len());

    // 2. For each face pair, compute SSI and collect splits for both A and B
    let compute_ssi = |&(face_a, face_b): &(FaceId, FaceId)| -> Option<SsiPairResult> {
        // Get face data with bounds checking to avoid panics
        let face_data_a = a.topology.faces.get(face_a)?;
        let face_data_b = b.topology.faces.get(face_b)?;
        let surf_a = a.geometry.surfaces.get(face_data_a.surface_index)?;
        let surf_b = b.geometry.surfaces.get(face_data_b.surface_index)?;

        let curve = ssi::intersect_surfaces(surf_a.as_ref(), surf_b.as_ref());

        if matches!(curve, ssi::IntersectionCurve::Empty) {
            return None;
        }

        let mut results_a = Vec::new();
        let mut results_b = Vec::new();

        // For circle curves (from plane-cylinder intersection), skip trimming —
        // the circle is already the intersection. Add split instructions to
        // BOTH the planar face (gets an inner loop) and the cylindrical face
        // (gets a new edge splitting the cylinder).
        // Note: face_a and face_b can be from either solid, so check both.
        // Also verify the circle center is in the face's material region to
        // avoid creating nested inner loops (e.g. bore circle inside hub hole).
        if let ssi::IntersectionCurve::Circle(circle) = &curve {
            // Sample a point on the circle (not center — center might not be in face for cylindrical)
            let sample_pt = circle.center + circle.radius * circle.x_dir.into_inner();
            if split::is_planar_face(&a, face_a) {
                // Check point_in_face to avoid creating inner loops inside existing holes.
                // For degenerate cap faces (single-vertex outer loop), always check.
                // For regular polygon faces, only check if they already have inner loops
                // (existing holes from prior boolean ops). Without existing holes, the
                // circle intersection is always valid and split_planar_face handles
                // partial containment correctly.
                let outer_len = a.topology.loop_len(a.topology.faces[face_a].outer_loop);
                let has_inner_loops = !a.topology.faces[face_a].inner_loops.is_empty();
                if outer_len <= 1 {
                    if trim::point_in_face(&a, face_a, &sample_pt) {
                        results_a.push((curve.clone(), circle.center, circle.center));
                    }
                } else if has_inner_loops {
                    // Regular polygon face with existing holes: check that the circle
                    // is inside the face material (not inside an existing hole)
                    if trim::point_in_face(&a, face_a, &sample_pt) {
                        results_a.push((curve.clone(), circle.center, circle.center));
                    }
                } else {
                    results_a.push((curve.clone(), circle.center, circle.center));
                }
            }
            if split::is_cylindrical_face(&a, face_a) {
                results_a.push((curve.clone(), circle.center, circle.center));
            }
            if split::is_spherical_face(&a, face_a) {
                results_a.push((curve.clone(), circle.center, circle.center));
            }
            if split::is_conical_face(&a, face_a) {
                results_a.push((curve.clone(), circle.center, circle.center));
            }
            if split::is_planar_face(&b, face_b) {
                let outer_len = b.topology.loop_len(b.topology.faces[face_b].outer_loop);
                let has_inner_loops = !b.topology.faces[face_b].inner_loops.is_empty();
                if outer_len <= 1 {
                    if trim::point_in_face(&b, face_b, &sample_pt) {
                        results_b.push((curve.clone(), circle.center, circle.center));
                    }
                } else if has_inner_loops {
                    // Regular polygon face with existing holes: check that the circle
                    // is inside the face material (not inside an existing hole)
                    if trim::point_in_face(&b, face_b, &sample_pt) {
                        results_b.push((curve.clone(), circle.center, circle.center));
                    }
                } else {
                    results_b.push((curve.clone(), circle.center, circle.center));
                }
            }
            if split::is_cylindrical_face(&b, face_b) {
                results_b.push((curve.clone(), circle.center, circle.center));
            }
            if split::is_spherical_face(&b, face_b) {
                results_b.push((curve.clone(), circle.center, circle.center));
            }
            if split::is_conical_face(&b, face_b) {
                results_b.push((curve.clone(), circle.center, circle.center));
            }
            return Some((face_a, results_a, face_b, results_b));
        }

        // Expand TwoLines into individual Line curves for processing
        let curves_to_process: Vec<ssi::IntersectionCurve> = match &curve {
            ssi::IntersectionCurve::TwoLines(line1, line2) => {
                debug_bool!(
                    "  TwoLines: {:?}({:?}) x {:?}({:?})",
                    face_a,
                    surf_a.surface_type(),
                    face_b,
                    surf_b.surface_type()
                );
                debug_bool!(
                    "    Line1: origin=({:.2},{:.2},{:.2}) dir=({:.2},{:.2},{:.2})",
                    line1.origin.x,
                    line1.origin.y,
                    line1.origin.z,
                    line1.direction.x,
                    line1.direction.y,
                    line1.direction.z
                );
                debug_bool!(
                    "    Line2: origin=({:.2},{:.2},{:.2}) dir=({:.2},{:.2},{:.2})",
                    line2.origin.x,
                    line2.origin.y,
                    line2.origin.z,
                    line2.direction.x,
                    line2.direction.y,
                    line2.direction.z
                );
                vec![
                    ssi::IntersectionCurve::Line(line1.clone()),
                    ssi::IntersectionCurve::Line(line2.clone()),
                ]
            }
            _ => vec![curve.clone()],
        };

        for single_curve in &curves_to_process {
            // Trim curve to A's face boundary (for non-circle curves)
            let segs_a = trim::trim_curve_to_face(single_curve, face_a, &a, 64);
            debug_bool!(
                "    Trim to face A ({:?}): {} segments",
                face_a,
                segs_a.len()
            );
            for seg in &segs_a {
                let entry = evaluate_curve(single_curve, seg.t_start);
                let exit = evaluate_curve(single_curve, seg.t_end);
                let len = (exit - entry).norm();
                debug_bool!(
                    "      Segment: entry=({:.2},{:.2},{:.2}) exit=({:.2},{:.2},{:.2}) len={:.4}",
                    entry.x,
                    entry.y,
                    entry.z,
                    exit.x,
                    exit.y,
                    exit.z,
                    len
                );
                if len > 1e-6 {
                    results_a.push((single_curve.clone(), entry, exit));
                }
            }

            // Trim curve to B's face boundary (for non-circle curves)
            let segs_b = trim::trim_curve_to_face(single_curve, face_b, &b, 64);
            debug_bool!(
                "    Trim to face B ({:?}): {} segments",
                face_b,
                segs_b.len()
            );
            for seg in &segs_b {
                let entry = evaluate_curve(single_curve, seg.t_start);
                let exit = evaluate_curve(single_curve, seg.t_end);
                let len = (exit - entry).norm();
                debug_bool!(
                    "      Segment: entry=({:.2},{:.2},{:.2}) exit=({:.2},{:.2},{:.2}) len={:.4}",
                    entry.x,
                    entry.y,
                    entry.z,
                    exit.x,
                    exit.y,
                    exit.z,
                    len
                );
                if len > 1e-6 {
                    results_b.push((single_curve.clone(), entry, exit));
                }
            }
        }

        Some((face_a, results_a, face_b, results_b))
    };

    // Use Rayon parallelism only when there are enough pairs to amortize thread overhead
    let split_results: Vec<_> = if pairs.len() >= 100 {
        pairs.par_iter().filter_map(compute_ssi).collect()
    } else {
        pairs.iter().filter_map(compute_ssi).collect()
    };

    // Merge results into HashMaps
    let mut splits_a: HashMap<FaceId, FaceSplits> = HashMap::new();
    let mut splits_b: HashMap<FaceId, FaceSplits> = HashMap::new();

    for (face_a, results_a, face_b, results_b) in split_results {
        if !results_a.is_empty() {
            splits_a.entry(face_a).or_default().extend(results_a);
        }
        if !results_b.is_empty() {
            splits_b.entry(face_b).or_default().extend(results_b);
        }
    }

    debug_bool!("\n--- Stage 2: SSI results ---");
    debug_bool!("Faces of A to split: {}", splits_a.len());
    debug_bool!("Faces of B to split: {}", splits_b.len());

    // Apply splits to both solids
    apply_splits_to_solid(&mut a, splits_a, segments, "A");
    debug_bool!("\n--- Stage 2.5: After splits applied to A ---");
    debug_bool!("A now has {} faces", a.topology.faces.len());

    apply_splits_to_solid(&mut b, splits_b, segments, "B");

    // 3. Classify all faces (including split sub-faces)
    debug_bool!("\n--- Stage 3: Classification ---");
    debug_bool!("Solid A has {} faces after splits", a.topology.faces.len());
    debug_bool!("Solid B has {} faces after splits", b.topology.faces.len());

    // Tessellate each solid once and reuse for classification
    let mesh_b = tessellate_brep(&b, segments);
    let mesh_a = tessellate_brep(&a, segments);
    let classes_a = classify::classify_all_faces_with_mesh(&a, &b, &mesh_b);
    let classes_b = classify::classify_all_faces_with_mesh(&b, &a, &mesh_a);

    debug_bool!("\nClassification of A faces:");
    for (fid, _class) in &classes_a {
        let _sample = classify::face_sample_point(&a, *fid);
        let face = &a.topology.faces[*fid];
        let _surf = &a.geometry.surfaces[face.surface_index];
        debug_bool!(
            "  {:?}: {:?} sample=({:.2},{:.2},{:.2}) -> {:?}",
            fid,
            _surf.surface_type(),
            _sample.x,
            _sample.y,
            _sample.z,
            _class
        );
    }
    debug_bool!("\nClassification of B faces:");
    for (fid, _class) in &classes_b {
        let _sample = classify::face_sample_point(&b, *fid);
        debug_bool!(
            "  {:?}: sample=({:.2},{:.2},{:.2}) -> {:?}",
            fid,
            _sample.x,
            _sample.y,
            _sample.z,
            _class
        );
    }

    // 4. Select and sew
    let (keep_a, keep_b, reverse_b) = classify::select_faces(op, &classes_a, &classes_b);

    debug_bool!("\n--- Stage 4: Selection (op={:?}) ---", op);
    debug_bool!("Keep {} A faces:", keep_a.len());
    for fid in &keep_a {
        let face = &a.topology.faces[*fid];
        let _surf = &a.geometry.surfaces[face.surface_index];
        let _sample = classify::face_sample_point(&a, *fid);
        debug_bool!(
            "  {:?}: {:?} sample=({:.2},{:.2},{:.2})",
            fid,
            _surf.surface_type(),
            _sample.x,
            _sample.y,
            _sample.z
        );
    }
    debug_bool!("Keep {} B faces (reverse_b={}):", keep_b.len(), reverse_b);
    for fid in &keep_b {
        let face = &b.topology.faces[*fid];
        let _surf = &b.geometry.surfaces[face.surface_index];
        let _sample = classify::face_sample_point(&b, *fid);
        debug_bool!(
            "  {:?}: {:?} sample=({:.2},{:.2},{:.2})",
            fid,
            _surf.surface_type(),
            _sample.x,
            _sample.y,
            _sample.z
        );
    }

    let result = sew::sew_faces(&a, &keep_a, &b, &keep_b, reverse_b, 1e-6);

    debug_bool!("\n--- Stage 5: Result ---");
    debug_bool!("Result solid has {} faces", result.topology.faces.len());
    debug_bool!("========== BREP BOOLEAN END ==========\n");

    BooleanResult::BRep(Box::new(result))
}
