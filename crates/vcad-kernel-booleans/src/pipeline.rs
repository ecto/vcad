//! B-rep boolean pipeline - face splitting, classification, and sewing.

use std::collections::HashMap;

use rayon::prelude::*;
use vcad_kernel_math::Point3;
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_tessellate::tessellate_brep;
use vcad_kernel_topo::FaceId;

use crate::api::{BooleanError, BooleanOp, BooleanResult};
use crate::mesh::empty_brep;
use crate::{bbox, classify, sew, split, ssi, trim};

/// Per-face split data: intersection curve with entry/exit points.
type FaceSplits = Vec<(ssi::IntersectionCurve, Point3, Point3)>;

/// Result of computing SSI for one face pair: splits for face A and face B,
/// plus whether the pair *really* crosses — i.e. the trimmed curve intervals
/// of both faces overlap on the same intersection curve. Splits are also
/// recorded when the carrier curve crosses only one face of the pair
/// (phantom splits from nearby-but-disjoint faces); those must not block
/// the no-crossing fast path.
type SsiPairResult = (FaceId, FaceSplits, FaceId, FaceSplits, bool);

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
            // Intersection of non-overlapping = empty B-rep.
            BooleanResult::BRep(Box::new(empty_brep()))
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
            // A degenerate one-sample curve has no segment to interpolate;
            // `len - 2` below would underflow.
            if points.len() == 1 {
                return snap_point(points[0]);
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
        ssi::IntersectionCurve::TwoSampled(points, _) => {
            // TwoSampled should be expanded before calling this function;
            // evaluate on the first curve as a defensive fallback.
            if points.is_empty() {
                return Point3::origin();
            }
            if points.len() == 1 {
                return snap_point(points[0]);
            }
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
    // Deterministic order: the splits are applied in sequence and each
    // one reshapes the face the next one sees, so iterating the HashMap
    // directly made the same boolean return a different solid (and a
    // different volume) run to run.
    let mut ordered: Vec<(FaceId, FaceSplits)> = splits.into_iter().collect();
    ordered.sort_by_key(|(fid, _)| *fid);
    for (face_id, split_list) in ordered {
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
                        let result = split::split_cylindrical_face(solid, fid, &curve, segments);
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
                        if let ssi::IntersectionCurve::Line(_l) = &curve {
                            debug_bool!(
                                "  Split {} planar face {:?} by Line at ({:.2},{:.2},{:.2}) dir ({:.2},{:.2},{:.2})",
                                solid_name,
                                fid,
                                _l.origin.x,
                                _l.origin.y,
                                _l.origin.z,
                                _l.direction.x,
                                _l.direction.y,
                                _l.direction.z
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
                                "    -> planar Line split result: {} sub-faces {:?}",
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

                    // Generic path: re-trim the curve to each (sub-)face and
                    // split along every trimmed segment. A curve can cross a
                    // face more than once (e.g. an ellipse clipping two
                    // corners), and the first listed segment can be a
                    // phantom that fails to split — so each face tries every
                    // segment, and successful splits re-queue their
                    // sub-faces for the remaining crossings.
                    let mut pending = vec![fid];
                    let mut passes = 0usize;
                    while let Some(cur) = pending.pop() {
                        passes += 1;
                        if passes > 32 {
                            // Defensive bound; no realistic curve crosses a
                            // face this many times.
                            new_faces.push(cur);
                            continue;
                        }
                        // A closed sampled curve whose parameter seam falls
                        // INSIDE this face yields two half-intervals, each
                        // with one endpoint floating mid-face at the seam —
                        // neither is a splittable cut. Rotate the sample
                        // list so it starts at a sample outside the face;
                        // the interval then reads as one seam-free segment.
                        let curve_for_face: ssi::IntersectionCurve =
                            if let ssi::IntersectionCurve::Sampled(pts) = &curve {
                                let closed = pts.len() > 2
                                    && (pts[0] - pts[pts.len() - 1]).norm() < 1e-9;
                                let seam_inside = closed
                                    && trim::point_in_face(solid, cur, &pts[0]);
                                if seam_inside {
                                    let m = pts.len() - 1; // unique samples
                                    match (0..m).find(|&k| {
                                        !trim::point_in_face(solid, cur, &pts[k])
                                    }) {
                                        Some(k) => {
                                            let mut rot: Vec<Point3> =
                                                Vec::with_capacity(m + 1);
                                            for j in 0..m {
                                                rot.push(pts[(k + j) % m]);
                                            }
                                            rot.push(pts[k]);
                                            ssi::IntersectionCurve::Sampled(rot)
                                        }
                                        None => curve.clone(),
                                    }
                                } else {
                                    curve.clone()
                                }
                            } else {
                                curve.clone()
                            };
                        let curve = &curve_for_face;
                        let segs = trim::trim_curve_to_face(curve, cur, solid, 64);
                        debug_bool!(
                            "  Split {} face {:?}: re-trim got {} segs",
                            solid_name,
                            cur,
                            segs.len()
                        );
                        let mut split_applied = false;
                        for seg in &segs {
                            let entry = evaluate_curve(curve, seg.t_start);
                            let exit = evaluate_curve(curve, seg.t_end);
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
                                continue;
                            }
                            let result =
                                split::split_face_by_curve(solid, cur, curve, &entry, &exit);
                            debug_bool!(
                                "    -> split result: {} sub-faces {:?}",
                                result.sub_faces.len(),
                                result.sub_faces
                            );
                            if result.sub_faces.len() >= 2 {
                                pending.extend(result.sub_faces);
                                split_applied = true;
                                break;
                            }
                        }
                        if !split_applied {
                            // Sub-resolution fallback: a curve crossing a
                            // THIN face (a 0.5 mm blade wall) can enter and
                            // leave between two samples — the trim sees
                            // nothing, but the face's boundary already
                            // carries the two exact crossing vertices
                            // (inserted by the neighboring faces' splits).
                            // Cut the chord between them; over a
                            // sub-sample span its deviation from the true
                            // curve is far below merge tolerance.
                            if let ssi::IntersectionCurve::Sampled(pts) = curve {
                                if let Some(r) =
                                    split::split_thin_face_at_curve_vertices(solid, cur, pts)
                                {
                                    if r.sub_faces.len() >= 2 {
                                        pending.extend(r.sub_faces);
                                        continue;
                                    }
                                }
                            }
                            new_faces.push(cur);
                        }
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
) -> Result<BooleanResult, BooleanError> {
    debug_bool!("\n========== BREP BOOLEAN START ==========");
    debug_bool!("Operation: {:?}", op);
    debug_bool!("Solid A: {} faces", solid_a.topology.faces.len());
    debug_bool!("Solid B: {} faces", solid_b.topology.faces.len());

    // Clone both solids so we can split them
    let mut a = solid_a.clone();
    let mut b = solid_b.clone();

    // Freeze analytic full-circle edges into canonical polylines so the
    // whole pipeline works on one concrete boundary representation (see
    // crate::freeze). Without this, results that keep an untouched analytic
    // face are watertight only by resolution coincidence, and any further
    // boolean on them cannot conform.
    if std::env::var("VCAD_NO_FREEZE").is_err() {
        crate::freeze::freeze_circle_loops(&mut a, segments);
        crate::freeze::freeze_circle_loops(&mut b, segments);
    }

    // 1. Find candidate face pairs via AABB filtering
    let pairs = bbox::find_candidate_face_pairs(&a, &b);
    debug_bool!("\n--- Stage 1: AABB filtering ---");
    debug_bool!("Candidate face pairs: {}", pairs.len());

    // 2. For each face pair, compute SSI and collect splits for both A and B.
    // `Ok(None)` means "no splits for this pair"; `Err` aborts the whole
    // boolean with a typed error instead of panicking mid-pipeline.
    let compute_ssi =
        |&(face_a, face_b): &(FaceId, FaceId)| -> Result<Option<SsiPairResult>, BooleanError> {
            // Get face data with bounds checking to avoid panics
            let Some(face_data_a) = a.topology.faces.get(face_a) else {
                return Ok(None);
            };
            let Some(face_data_b) = b.topology.faces.get(face_b) else {
                return Ok(None);
            };
            let Some(surf_a) = a.geometry.surfaces.get(face_data_a.surface_index) else {
                return Ok(None);
            };
            let Some(surf_b) = b.geometry.surfaces.get(face_data_b.surface_index) else {
                return Ok(None);
            };

            let curve = ssi::intersect_surfaces(surf_a.as_ref(), surf_b.as_ref())?;

            if matches!(curve, ssi::IntersectionCurve::Empty) {
                return Ok(None);
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
                // The circle counts as touching a face when ANY of several
                // rim probes lands in it (not center — center might not be in
                // face for cylindrical; not a single rim point — a circle that
                // partially overhangs the face's boundary, e.g. a boss hanging
                // over a disc edge, can have any one fixed angle land outside
                // even though the crossing is real and must be split).
                let circle_touches_face = |solid: &BRepSolid, fid: FaceId| -> bool {
                    for k in 0..8 {
                        let theta = 2.0 * std::f64::consts::PI * k as f64 / 8.0;
                        let dir = theta.cos() * circle.x_dir.into_inner()
                            + theta.sin() * circle.y_dir.into_inner();
                        if trim::point_in_face(solid, fid, &(circle.center + circle.radius * dir)) {
                            return true;
                        }
                    }
                    false
                };

                // A circle from a curved×planar SSI is built from the planar face's
                // *infinite* carrier plane. When that planar face is a small bounded
                // disk (e.g. a cylinder cap), the circle can be far larger than the
                // face and centered well outside it — a "phantom" circle. The
                // analytic splitters for curved faces (cylinder/sphere/cone) split
                // the *whole* surface by the circle, so applying a phantom circle to
                // a curved partner resurfaces geometry that a prior boolean already
                // trimmed away (e.g. a subtracted sphere reappearing above the part).
                //
                // Gate each curved-face split on the planar partner actually
                // anchoring the circle: its center must lie within the partner
                // face. The center test alone rejects annular partners whose
                // inner boundary IS the circle (a drum face butted against a
                // hub wall: the center sits in the annulus hole), leaving the
                // wall unsplit at the contact circle and the touching interface
                // untrimmed. Second chance: the circle counts as SEATED in the
                // face when rim probes at several angles land inside it. The
                // probes are nudged to both sides because a face whose hole
                // boundary is this circle stores it as an inscribed polygon (a
                // probe exactly on the rim falls in the hole by the chord sag),
                // and ≥2 distinct angles must hit so a circle that merely
                // grazes a face edge over a tiny arc — the phantom regime the
                // anchor gate exists for — still fails. Curved×curved pairs
                // have no planar partner and keep their existing behavior.
                let nudge = (circle.radius * 0.01).max(0.05);
                let seated = |solid: &vcad_kernel_primitives::BRepSolid, fid: FaceId| {
                    let mut hits = 0u32;
                    for k in 0..8 {
                        let theta = 2.0 * std::f64::consts::PI * k as f64 / 8.0;
                        let dir = theta.cos() * circle.x_dir.into_inner()
                            + theta.sin() * circle.y_dir.into_inner();
                        let out = circle.center + (circle.radius + nudge) * dir;
                        let inn = circle.center + (circle.radius - nudge) * dir;
                        if trim::point_in_face(solid, fid, &out)
                            || trim::point_in_face(solid, fid, &inn)
                        {
                            hits += 1;
                            if hits >= 2 {
                                return true;
                            }
                        }
                    }
                    false
                };
                let b_anchors_circle = !split::is_planar_face(&b, face_b)
                    || trim::point_in_face(&b, face_b, &circle.center)
                    || seated(&b, face_b);
                let a_anchors_circle = !split::is_planar_face(&a, face_a)
                    || trim::point_in_face(&a, face_a, &circle.center)
                    || seated(&a, face_a);

                // A circle that is already part of the planar face's sampled
                // boundary (the cap of an arc-extruded body paired with the
                // wall it borders) has nothing to split — feeding it to
                // split_planar_face shaves crescent slivers off the boundary
                // polygon, which then classify unreliably. Arc samples lie
                // exactly on their circle, and a circle that genuinely crosses
                // a polygon touches it at no more than two points, so ≥3
                // on-circle vertices means "this circle is one of my arcs".
                let circle_is_own_boundary = |solid: &BRepSolid, fid: FaceId| -> bool {
                    let face = &solid.topology.faces[fid];
                    let radius_tol = (circle.radius * 1e-4).max(1e-4);
                    let mut on_circle = 0u32;
                    let loops =
                        std::iter::once(face.outer_loop).chain(face.inner_loops.iter().copied());
                    for loop_id in loops {
                        for he in solid.topology.loop_half_edges(loop_id) {
                            let v =
                                solid.topology.vertices[solid.topology.half_edges[he].origin].point;
                            if ((v - circle.center).norm() - circle.radius).abs() <= radius_tol {
                                on_circle += 1;
                                if on_circle >= 3 {
                                    return true;
                                }
                            }
                        }
                    }
                    false
                };

                if split::is_planar_face(&a, face_a) && !circle_is_own_boundary(&a, face_a) {
                    // Check point_in_face to avoid creating inner loops inside existing holes.
                    // For degenerate cap faces (single-vertex outer loop), always check.
                    // For regular polygon faces, only check if they already have inner loops
                    // (existing holes from prior boolean ops). Without existing holes, the
                    // circle intersection is always valid and split_planar_face handles
                    // partial containment correctly.
                    let outer_len = a.topology.loop_len(a.topology.faces[face_a].outer_loop);
                    let has_inner_loops = !a.topology.faces[face_a].inner_loops.is_empty();
                    if outer_len <= 1 {
                        if circle_touches_face(&a, face_a) {
                            results_a.push((curve.clone(), circle.center, circle.center));
                        }
                    } else if has_inner_loops {
                        // Regular polygon face with existing holes: check that the circle
                        // is inside the face material (not inside an existing hole)
                        if circle_touches_face(&a, face_a) {
                            results_a.push((curve.clone(), circle.center, circle.center));
                        }
                    } else {
                        results_a.push((curve.clone(), circle.center, circle.center));
                    }
                }
                if b_anchors_circle && split::is_cylindrical_face(&a, face_a) {
                    results_a.push((curve.clone(), circle.center, circle.center));
                }
                if b_anchors_circle && split::is_spherical_face(&a, face_a) {
                    results_a.push((curve.clone(), circle.center, circle.center));
                }
                if b_anchors_circle && split::is_conical_face(&a, face_a) {
                    results_a.push((curve.clone(), circle.center, circle.center));
                }
                if split::is_planar_face(&b, face_b) && !circle_is_own_boundary(&b, face_b) {
                    let outer_len = b.topology.loop_len(b.topology.faces[face_b].outer_loop);
                    let has_inner_loops = !b.topology.faces[face_b].inner_loops.is_empty();
                    if outer_len <= 1 {
                        if circle_touches_face(&b, face_b) {
                            results_b.push((curve.clone(), circle.center, circle.center));
                        }
                    } else if has_inner_loops {
                        // Regular polygon face with existing holes: check that the circle
                        // is inside the face material (not inside an existing hole)
                        if circle_touches_face(&b, face_b) {
                            results_b.push((curve.clone(), circle.center, circle.center));
                        }
                    } else {
                        results_b.push((curve.clone(), circle.center, circle.center));
                    }
                }
                if a_anchors_circle && split::is_cylindrical_face(&b, face_b) {
                    results_b.push((curve.clone(), circle.center, circle.center));
                }
                if a_anchors_circle && split::is_spherical_face(&b, face_b) {
                    results_b.push((curve.clone(), circle.center, circle.center));
                }
                if a_anchors_circle && split::is_conical_face(&b, face_b) {
                    results_b.push((curve.clone(), circle.center, circle.center));
                }
                // Circle splits are not interval-trimmed; treat any hit as a
                // real crossing (conservative).
                let real = !results_a.is_empty() || !results_b.is_empty();
                return Ok(Some((face_a, results_a, face_b, results_b, real)));
            }

            // TwoSampled is the analytic cylinder × cylinder result (the
            // Steinmetz pair of ellipses). The curves are already on both
            // cylindrical surfaces; route them directly to the cylindrical
            // splitter without going through `trim_curve_to_face`, so the
            // figure-8 stays atomic and the 4-way split happens in one call.
            if let ssi::IntersectionCurve::TwoSampled(_, _) = &curve {
                debug_bool!(
                    "  TwoSampled: {:?}({:?}) x {:?}({:?})",
                    face_a,
                    surf_a.surface_type(),
                    face_b,
                    surf_b.surface_type()
                );
                if split::is_cylindrical_face(&a, face_a) {
                    results_a.push((curve.clone(), Point3::origin(), Point3::origin()));
                }
                if split::is_cylindrical_face(&b, face_b) {
                    results_b.push((curve.clone(), Point3::origin(), Point3::origin()));
                }
                let real = !results_a.is_empty() || !results_b.is_empty();
                return Ok(Some((face_a, results_a, face_b, results_b, real)));
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

            let mut real_crossing = false;
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

                // The pair genuinely crosses only when a positive-length stretch
                // of the curve lies on BOTH faces: their trimmed t-intervals
                // must overlap. One-sided segments are phantom splits from the
                // infinite carrier curve crossing a nearby (but disjoint) face.
                if !real_crossing {
                    'overlap: for sa in &segs_a {
                        for sb in &segs_b {
                            let lo = sa.t_start.max(sb.t_start);
                            let hi = sa.t_end.min(sb.t_end);
                            if hi > lo {
                                let p0 = evaluate_curve(single_curve, lo);
                                let p1 = evaluate_curve(single_curve, hi);
                                if (p1 - p0).norm() > 1e-6 {
                                    real_crossing = true;
                                    break 'overlap;
                                }
                            }
                        }
                    }
                }
            }

            Ok(Some((face_a, results_a, face_b, results_b, real_crossing)))
        };

    // Use Rayon parallelism only when there are enough pairs to amortize
    // thread overhead. The first SSI error aborts the whole boolean.
    let pair_results: Result<Vec<Option<SsiPairResult>>, BooleanError> = if pairs.len() >= 100 {
        pairs.par_iter().map(compute_ssi).collect()
    } else {
        pairs.iter().map(compute_ssi).collect()
    };
    let split_results: Vec<SsiPairResult> = pair_results?.into_iter().flatten().collect();

    // Merge results into HashMaps
    let mut splits_a: HashMap<FaceId, FaceSplits> = HashMap::new();
    let mut splits_b: HashMap<FaceId, FaceSplits> = HashMap::new();
    let mut any_real_crossing = false;

    for (face_a, results_a, face_b, results_b, real_crossing) in split_results {
        any_real_crossing |= real_crossing;
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

    // No real crossings: the boundaries never cross, so the operands are
    // disjoint or nested (or merely touching, which `boundaries_touch`
    // detects and routes to the general pipeline for coincident-face
    // dedup). Resolving containment with two point queries avoids the
    // O(|A|·|B|) classify-everything stage below — the difference between
    // hours and seconds when unioning thousands of disjoint solids.
    // One-sided (phantom) splits are dropped in this case: they come from
    // infinite carrier curves crossing a single face and would only
    // fragment the result.
    if !any_real_crossing && !crate::no_crossing::boundaries_touch(&a, &b, &pairs) {
        if let Some(rel) = crate::no_crossing::resolve_containment(&a, &b, segments) {
            debug_bool!("No-crossing fast path: {:?}", rel);
            return Ok(crate::no_crossing::no_crossing_result(a, b, op, rel));
        }
    }

    // Apply splits to both solids
    apply_splits_to_solid(&mut a, splits_a, segments, "A");
    // Heal the T-junctions splitting leaves behind (stacked band pieces
    // partition shared vertical boundaries at different heights, a pinch
    // tangency stops one side's rim where the other continues). The
    // classification stage ray-casts against these post-split meshes, and
    // rays escaping through pre-repair cracks misclassify whole faces.
    crate::repair::repair_topology(&mut a.topology, 1e-6);
    debug_bool!("\n--- Stage 2.5: After splits applied to A ---");
    debug_bool!("A now has {} faces", a.topology.faces.len());

    apply_splits_to_solid(&mut b, splits_b, segments, "B");
    crate::repair::repair_topology(&mut b.topology, 1e-6);

    // 3. Classify all faces (including split sub-faces)
    debug_bool!("\n--- Stage 3: Classification ---");
    debug_bool!("Solid A has {} faces after splits", a.topology.faces.len());
    debug_bool!("Solid B has {} faces after splits", b.topology.faces.len());

    // Tessellate each solid once and reuse for classification
    let mesh_b = tessellate_brep(&b, segments);
    let mesh_a = tessellate_brep(&a, segments);
    #[cfg(feature = "debug-boolean")]
    {
        let open = |mesh: &vcad_kernel_tessellate::TriangleMesh| -> usize {
            let quantum = 1e-5;
            let vkey = |vi: usize| -> [i64; 3] {
                let mut k = [0i64; 3];
                for c in 0..3 {
                    k[c] = (mesh.vertices[vi * 3 + c] as f64 / quantum).round() as i64;
                }
                k
            };
            let mut net: std::collections::HashMap<([i64; 3], [i64; 3]), i64> =
                std::collections::HashMap::new();
            for t in 0..mesh.indices.len() / 3 {
                for k in 0..3 {
                    let x = vkey(mesh.indices[t * 3 + k] as usize);
                    let y = vkey(mesh.indices[t * 3 + (k + 1) % 3] as usize);
                    if x == y {
                        continue;
                    }
                    if x < y {
                        *net.entry((x, y)).or_default() += 1;
                    } else {
                        *net.entry((y, x)).or_default() -= 1;
                    }
                }
            }
            let mut locs: Vec<_> = net
                .iter()
                .filter(|(_, &n)| n != 0)
                .map(|((x, y), _)| (*x, *y))
                .collect();
            locs.sort();
            for (x, y) in locs.iter().take(12) {
                debug_bool!(
                    "    open ({:.4},{:.4},{:.4})->({:.4},{:.4},{:.4})",
                    x[0] as f64 * quantum,
                    x[1] as f64 * quantum,
                    x[2] as f64 * quantum,
                    y[0] as f64 * quantum,
                    y[1] as f64 * quantum,
                    y[2] as f64 * quantum
                );
            }
            net.values().map(|n| n.unsigned_abs() as usize).sum()
        };
        debug_bool!(
            "post-split meshes: A open={} B open={}",
            open(&mesh_a),
            open(&mesh_b)
        );
        let params = vcad_kernel_tessellate::TessellationParams::from_segments(segments);
        for (fid, _kind, fmesh) in vcad_kernel_tessellate::tessellate_brep_by_face(&a, &params) {
            let step: Vec<f64> = fmesh
                .vertices
                .chunks(3)
                .take(4)
                .map(|c| (c[1] as f64).atan2(c[0] as f64))
                .collect();
            debug_bool!(
                "  A face {:?}: {} tris, first angles {:?}",
                fid,
                fmesh.indices.len() / 3,
                step
            );
        }
    }
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

    Ok(BooleanResult::BRep(Box::new(result)))
}
