//! Suite D (Visual) grader checks.
//!
//! Visual grading is a deliberate philosophy shift from the deterministic
//! suites (A/B/C/F): the candidate is graded by how close its mesh comes
//! to a target shape, not by exact dimensional checks. Use for organic
//! / artistic forms (sea lions, figurines, sculpted shapes) that don't
//! admit closed-form mass-property targets.
//!
//! The single check today is bidirectional Chamfer distance between the
//! candidate's tessellated mesh and the target mesh, sampled at the same
//! tessellation density (64 segments). Symmetric Chamfer = average of
//! mean(min-dist candidate→target) and mean(min-dist target→candidate).
//!
//! Implementation is naive O(n²) — for the meshes the F-suite produces
//! (~5k vertices each side) that's tens of millions of distance ops,
//! which is fine for v0. KD-tree acceleration is a future improvement.

use crate::blob::CheckOutcome;
use crate::fit::HostGeometry;
use serde_json::json;
use std::panic::{catch_unwind, AssertUnwindSafe};
use vcad_kernel::vcad_kernel_tessellate::TriangleMesh;
use vcad_kernel::Solid;

/// Tessellation density for shape-similarity checks. Match the F-suite
/// `fit` module so we don't fight over density when both run.
const VISUAL_TESSELLATION_SEGMENTS: u32 = 64;

/// `shape_similarity_chamfer`: bidirectional Chamfer distance between
/// candidate and target meshes, in millimetres. Pass if the symmetric
/// average is at most `max_chamfer_mm`.
pub fn check_shape_similarity_chamfer(
    candidate: &Solid,
    target: &HostGeometry,
    max_chamfer_mm: f64,
) -> (CheckOutcome, serde_json::Value) {
    let cand_mesh = match catch_unwind(AssertUnwindSafe(|| {
        candidate.to_mesh(VISUAL_TESSELLATION_SEGMENTS)
    })) {
        Ok(m) => m,
        Err(_) => {
            return (
                CheckOutcome::Fail,
                json!({ "reason": "candidate tessellation panicked" }),
            );
        }
    };
    if cand_mesh.vertices.is_empty() || target.mesh.vertices.is_empty() {
        return (
            CheckOutcome::Fail,
            json!({ "reason": "empty mesh", "cand_verts": cand_mesh.vertices.len() / 3, "target_verts": target.mesh.vertices.len() / 3 }),
        );
    }

    let cand_pts = mesh_points(&cand_mesh);
    let tgt_pts = mesh_points(&target.mesh);

    let (cand_to_tgt_mean, cand_to_tgt_max) = nearest_distance_stats(&cand_pts, &tgt_pts);
    let (tgt_to_cand_mean, tgt_to_cand_max) = nearest_distance_stats(&tgt_pts, &cand_pts);

    let symmetric_chamfer = 0.5 * (cand_to_tgt_mean + tgt_to_cand_mean);
    let pass = symmetric_chamfer <= max_chamfer_mm;
    (
        if pass {
            CheckOutcome::Pass
        } else {
            CheckOutcome::Fail
        },
        json!({
            "chamfer_mm": symmetric_chamfer,
            "max_chamfer_mm": max_chamfer_mm,
            "cand_to_target_mean_mm": cand_to_tgt_mean,
            "cand_to_target_max_mm": cand_to_tgt_max,
            "target_to_cand_mean_mm": tgt_to_cand_mean,
            "target_to_cand_max_mm": tgt_to_cand_max,
            "cand_vertices": cand_pts.len(),
            "target_vertices": tgt_pts.len(),
        }),
    )
}

fn mesh_points(mesh: &TriangleMesh) -> Vec<[f64; 3]> {
    let n = mesh.vertices.len() / 3;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let base = i * 3;
        out.push([
            mesh.vertices[base] as f64,
            mesh.vertices[base + 1] as f64,
            mesh.vertices[base + 2] as f64,
        ]);
    }
    out
}

/// For each point in `a`, find the closest point in `b` (Euclidean).
/// Returns `(mean, max)` over all such nearest-neighbour distances, in mm.
fn nearest_distance_stats(a: &[[f64; 3]], b: &[[f64; 3]]) -> (f64, f64) {
    if a.is_empty() || b.is_empty() {
        return (0.0, 0.0);
    }
    let mut sum = 0.0_f64;
    let mut max = 0.0_f64;
    for p in a {
        let mut min_sq = f64::INFINITY;
        for q in b {
            let dx = p[0] - q[0];
            let dy = p[1] - q[1];
            let dz = p[2] - q[2];
            let d = dx * dx + dy * dy + dz * dz;
            if d < min_sq {
                min_sq = d;
            }
        }
        let d = min_sq.sqrt();
        sum += d;
        if d > max {
            max = d;
        }
    }
    (sum / (a.len() as f64), max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::evaluate_vcad;
    use crate::fit::aggregate_candidate;
    use crate::task::InputFrame;

    fn sphere_vcad(radius: f64) -> String {
        format!(
            r#"{{"version":"0.1","nodes":{{
                "1":{{"id":1,"op":{{"type":"Sphere","radius":{r},"segments":64}}}}
            }},"materials":{{}},"part_materials":{{}},"roots":[{{"root":1,"material":"default"}}]}}"#,
            r = radius
        )
    }

    fn target(raw: &str) -> HostGeometry {
        let snap = evaluate_vcad(raw);
        let solid = aggregate_candidate(&snap).expect("target solid");
        let mesh = solid.to_mesh(VISUAL_TESSELLATION_SEGMENTS);
        HostGeometry {
            solid,
            mesh,
            frame: InputFrame {
                origin: [0.0, 0.0, 0.0],
                axis: [0.0, 0.0, 1.0],
            },
        }
    }

    #[test]
    fn identical_spheres_have_near_zero_chamfer() {
        let tgt = target(&sphere_vcad(15.0));
        let snap = evaluate_vcad(&sphere_vcad(15.0));
        let cand = aggregate_candidate(&snap).expect("candidate");
        let (out, details) = check_shape_similarity_chamfer(&cand, &tgt, 0.5);
        assert_eq!(out, CheckOutcome::Pass, "{:?}", details);
        let chamfer = details["chamfer_mm"].as_f64().unwrap();
        assert!(
            chamfer < 0.01,
            "expected near-zero chamfer, got {}",
            chamfer
        );
    }

    #[test]
    fn different_size_spheres_have_proportional_chamfer() {
        // Two spheres differing by 2mm in radius should give chamfer ~2mm.
        let tgt = target(&sphere_vcad(15.0));
        let snap = evaluate_vcad(&sphere_vcad(13.0));
        let cand = aggregate_candidate(&snap).expect("candidate");
        let (_out, details) = check_shape_similarity_chamfer(&cand, &tgt, 0.5);
        let chamfer = details["chamfer_mm"].as_f64().unwrap();
        assert!(
            chamfer > 1.0 && chamfer < 3.0,
            "expected chamfer ~2mm, got {}",
            chamfer
        );
    }
}
