//! Suite F (Fit) grader checks.
//!
//! Fit checks evaluate an accessory `.vcad` (the candidate) against a
//! host `.vcad` named in `task.inputs` as `kind: "host_geometry"`. The
//! grader assembles host and accessory in the host's declared frame and
//! evaluates four kinds of geometric/physical mate constraints:
//!
//! - [`check_envelope`] — accessory bbox extents.
//! - [`check_interference_volume`] — volume of (accessory ∩ host).
//! - [`check_contact_area`] — accessory surface area within ε of host.
//! - [`check_mate_clearance`] — minimum separation between the two.
//!
//! `gravity_hold` and `pull_force` are physics checks; they live as stubs
//! here until the phyz migration lands.

use crate::blob::CheckOutcome;
use crate::eval::{evaluate_vcad, EvalSnapshot};
use crate::task::{InputFrame, Task};
use serde_json::json;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use vcad_kernel::vcad_kernel_tessellate::TriangleMesh;
use vcad_kernel::Solid;

/// Tessellation density for fit checks. Higher than the kernel default
/// (32) because contact-area / clearance need a smooth mesh on cylinders.
const FIT_TESSELLATION_SEGMENTS: u32 = 64;

/// Resolves a host geometry input by `kind`, loads it from disk, and
/// evaluates it. Returns the assembled host solid and its placement
/// frame. The grader caches this per task across F-suite checks.
pub struct HostGeometry {
    /// Aggregated host solid (union of all evaluated roots), pre-placed
    /// in world space using the declared frame's origin. (Frame `axis`
    /// is currently informational — host meshes are authored in the
    /// declared frame already.)
    pub solid: Solid,
    /// Cached high-density mesh of the host for distance queries.
    pub mesh: TriangleMesh,
    /// Original placement frame from the task input.
    pub frame: InputFrame,
}

/// Errors returned when materializing the host geometry.
#[derive(Debug, thiserror::Error)]
pub enum HostError {
    /// Task did not declare a `host_geometry` input (or it was agent-visible).
    #[error("task has no `host_geometry` (private) input")]
    Missing,
    /// Host input had no `path`.
    #[error("`host_geometry` input has no path")]
    NoPath,
    /// Could not read the host file.
    #[error("could not read host file {0:?}: {1}")]
    Io(PathBuf, std::io::Error),
    /// Host `.vcad` failed to evaluate to any solid.
    #[error("host `.vcad` evaluated empty: {0}")]
    Empty(String),
}

/// Load + evaluate the task's host geometry. Resolves `path` relative to
/// `task_dir`. The placement frame defaults to origin / +Z if absent.
pub fn load_host(task: &Task, task_dir: &Path) -> Result<HostGeometry, HostError> {
    let input = task.private_input("host_geometry").ok_or(HostError::Missing)?;
    let rel = input.path.as_ref().ok_or(HostError::NoPath)?;
    let abs = task_dir.join(rel);
    let raw = std::fs::read_to_string(&abs).map_err(|e| HostError::Io(abs.clone(), e))?;
    let snap = evaluate_vcad(&raw);
    let solid = aggregate_solid(&snap)
        .ok_or_else(|| HostError::Empty(snap.fatal.clone().unwrap_or_else(|| "no solids".into())))?;
    let mesh = catch_unwind(AssertUnwindSafe(|| solid.to_mesh(FIT_TESSELLATION_SEGMENTS)))
        .map_err(|_| HostError::Empty("host tessellation panicked".into()))?;
    let frame = input.frame.clone().unwrap_or(InputFrame {
        origin: [0.0, 0.0, 0.0],
        axis: [0.0, 0.0, 1.0],
    });
    Ok(HostGeometry { solid, mesh, frame })
}

/// Sum-union every solid in the snapshot into one [`Solid`]. Returns
/// `None` if no positive-volume solids exist.
fn aggregate_solid(snap: &EvalSnapshot) -> Option<Solid> {
    let mut iter = snap
        .solids
        .iter()
        .filter(|s| s.volume() > 0.0 && s.num_triangles() > 0);
    let first = iter.next()?.clone();
    Some(iter.fold(first, |acc, s| acc.union(s)))
}

/// Same as [`aggregate_solid`] but for the candidate snapshot. Pulled
/// out so the grader's main loop can build it once and share across
/// every fit check.
pub fn aggregate_candidate(snap: &EvalSnapshot) -> Option<Solid> {
    aggregate_solid(snap)
}

// ---------- envelope ----------------------------------------------------

/// `envelope`: accessory bbox extents must not exceed `max_mm` per axis.
pub fn check_envelope(
    snap: &EvalSnapshot,
    max_mm: [f64; 3],
) -> (CheckOutcome, serde_json::Value) {
    let (lo, hi) = match snap.aggregate_bbox() {
        Some(b) => b,
        None => {
            return (
                CheckOutcome::Fail,
                json!({ "reason": "no valid solid to measure" }),
            );
        }
    };
    let extents = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
    let pass = (0..3).all(|i| extents[i] <= max_mm[i]);
    (
        if pass { CheckOutcome::Pass } else { CheckOutcome::Fail },
        json!({
            "extents_mm": extents,
            "max_mm": max_mm,
            "bbox_min": lo,
            "bbox_max": hi,
        }),
    )
}

// ---------- interference_volume ----------------------------------------

/// `interference_volume`: volume of (candidate ∩ host) must be ≤ `max_mm3`.
pub fn check_interference_volume(
    candidate: &Solid,
    host: &HostGeometry,
    max_mm3: f64,
) -> (CheckOutcome, serde_json::Value) {
    // AABB cull first — if the bounding boxes don't overlap, intersection
    // volume is exactly zero and we can short-circuit the (expensive)
    // boolean.
    if !candidate.aabb_overlaps(&host.solid) {
        return (
            CheckOutcome::Pass,
            json!({
                "interference_mm3": 0.0,
                "max_mm3": max_mm3,
                "short_circuit": "aabbs disjoint",
            }),
        );
    }
    let intersection = match catch_unwind(AssertUnwindSafe(|| candidate.intersection(&host.solid))) {
        Ok(s) => s,
        Err(_) => {
            return (
                CheckOutcome::Fail,
                json!({ "reason": "boolean intersection panicked" }),
            );
        }
    };
    let v = catch_unwind(AssertUnwindSafe(|| intersection.volume())).unwrap_or(f64::NAN);
    let v = if v.is_finite() && v >= 0.0 { v } else { 0.0 };
    let pass = v <= max_mm3;
    (
        if pass { CheckOutcome::Pass } else { CheckOutcome::Fail },
        json!({
            "interference_mm3": v,
            "max_mm3": max_mm3,
        }),
    )
}

// ---------- contact_area / mate_clearance helpers ----------------------

/// One triangle as three f64 corners — built from the kernel's flat
/// `vertices` (Vec<f32>) + `indices` (Vec<u32>) layout.
struct Tri {
    a: [f64; 3],
    b: [f64; 3],
    c: [f64; 3],
}

fn vert(mesh: &TriangleMesh, i: u32) -> [f64; 3] {
    let base = (i as usize) * 3;
    [
        mesh.vertices[base] as f64,
        mesh.vertices[base + 1] as f64,
        mesh.vertices[base + 2] as f64,
    ]
}

fn mesh_tris(mesh: &TriangleMesh) -> Vec<Tri> {
    let mut out = Vec::with_capacity(mesh.indices.len() / 3);
    for tri in mesh.indices.chunks_exact(3) {
        out.push(Tri {
            a: vert(mesh, tri[0]),
            b: vert(mesh, tri[1]),
            c: vert(mesh, tri[2]),
        });
    }
    out
}

fn tri_centroid(t: &Tri) -> [f64; 3] {
    [
        (t.a[0] + t.b[0] + t.c[0]) / 3.0,
        (t.a[1] + t.b[1] + t.c[1]) / 3.0,
        (t.a[2] + t.b[2] + t.c[2]) / 3.0,
    ]
}

fn tri_area(t: &Tri) -> f64 {
    let ab = sub(t.b, t.a);
    let ac = sub(t.c, t.a);
    let cx = ab[1] * ac[2] - ab[2] * ac[1];
    let cy = ab[2] * ac[0] - ab[0] * ac[2];
    let cz = ab[0] * ac[1] - ab[1] * ac[0];
    0.5 * (cx * cx + cy * cy + cz * cz).sqrt()
}

/// Squared distance from point `p` to triangle (a, b, c). Standard
/// Eberly clamping. Used by both contact-area and mate-clearance.
fn point_tri_distance_sq(p: [f64; 3], t: &Tri) -> f64 {
    let a = t.a;
    let b = t.b;
    let c = t.c;

    let ab = sub(b, a);
    let ac = sub(c, a);
    let ap = sub(p, a);

    let d1 = dot(ab, ap);
    let d2 = dot(ac, ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return dist_sq(p, a);
    }
    let bp = sub(p, b);
    let d3 = dot(ab, bp);
    let d4 = dot(ac, bp);
    if d3 >= 0.0 && d4 <= d3 {
        return dist_sq(p, b);
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        let q = [a[0] + v * ab[0], a[1] + v * ab[1], a[2] + v * ab[2]];
        return dist_sq(p, q);
    }
    let cp = sub(p, c);
    let d5 = dot(ab, cp);
    let d6 = dot(ac, cp);
    if d6 >= 0.0 && d5 <= d6 {
        return dist_sq(p, c);
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        let q = [a[0] + w * ac[0], a[1] + w * ac[1], a[2] + w * ac[2]];
        return dist_sq(p, q);
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        let q = [
            b[0] + w * (c[0] - b[0]),
            b[1] + w * (c[1] - b[1]),
            b[2] + w * (c[2] - b[2]),
        ];
        return dist_sq(p, q);
    }
    let denom = 1.0 / (va + vb + vc);
    let v = vb * denom;
    let w = vc * denom;
    let q = [
        a[0] + ab[0] * v + ac[0] * w,
        a[1] + ab[1] * v + ac[1] * w,
        a[2] + ab[2] * v + ac[2] * w,
    ];
    dist_sq(p, q)
}

#[inline]
fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
#[inline]
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
#[inline]
fn dist_sq(a: [f64; 3], b: [f64; 3]) -> f64 {
    let d = sub(a, b);
    dot(d, d)
}

// ---------- contact_area -----------------------------------------------

/// `contact_area`: total accessory triangle area whose centroid lies
/// within `epsilon_mm` of any host triangle. Approximate (centroid-only)
/// — refine with sub-sampling if false negatives appear in real tasks.
pub fn check_contact_area(
    candidate: &Solid,
    host: &HostGeometry,
    epsilon_mm: f64,
    min_mm2: f64,
) -> (CheckOutcome, serde_json::Value) {
    let cand_mesh = match catch_unwind(AssertUnwindSafe(|| {
        candidate.to_mesh(FIT_TESSELLATION_SEGMENTS)
    })) {
        Ok(m) => m,
        Err(_) => {
            return (
                CheckOutcome::Fail,
                json!({ "reason": "candidate tessellation panicked" }),
            );
        }
    };
    let cand_tris = mesh_tris(&cand_mesh);
    let host_tris = mesh_tris(&host.mesh);
    if host_tris.is_empty() || cand_tris.is_empty() {
        return (
            CheckOutcome::Fail,
            json!({ "reason": "empty mesh", "host_tri_count": host_tris.len(), "cand_tri_count": cand_tris.len() }),
        );
    }

    let eps_sq = epsilon_mm * epsilon_mm;
    let mut contact = 0.0_f64;
    for tri in &cand_tris {
        let centroid = tri_centroid(tri);
        let mut min_sq = f64::INFINITY;
        for ht in &host_tris {
            let d = point_tri_distance_sq(centroid, ht);
            if d < min_sq {
                min_sq = d;
                if min_sq <= eps_sq {
                    break;
                }
            }
        }
        if min_sq <= eps_sq {
            contact += tri_area(tri);
        }
    }

    let pass = contact >= min_mm2;
    (
        if pass { CheckOutcome::Pass } else { CheckOutcome::Fail },
        json!({
            "contact_area_mm2": contact,
            "min_mm2": min_mm2,
            "epsilon_mm": epsilon_mm,
            "candidate_triangles": cand_tris.len(),
            "host_triangles": host_tris.len(),
        }),
    )
}

// ---------- pull_retention_geometric -----------------------------------

/// `pull_retention_geometric`: translate the candidate by intermediate
/// fractions of `displacement_mm` along `direction` and measure peak
/// interference with the host. Pass if the peak exceeds the baseline
/// (as-designed pose) by at least `min_interference_gain_mm3`. This
/// is the deterministic stand-in for `pull_force` for snap-fit / form-
/// locked retention geometry.
///
/// Sampling is at 8 equispaced fractions in (0, 1] of the displacement.
pub fn check_pull_retention_geometric(
    candidate: &Solid,
    host: &HostGeometry,
    direction: [f64; 3],
    displacement_mm: f64,
    min_interference_gain_mm3: f64,
) -> (CheckOutcome, serde_json::Value) {
    let dir_mag = (direction[0] * direction[0]
        + direction[1] * direction[1]
        + direction[2] * direction[2])
        .sqrt();
    if dir_mag < 1e-9 {
        return (
            CheckOutcome::Fail,
            json!({ "reason": "pull direction is zero-length" }),
        );
    }
    let unit = [
        direction[0] / dir_mag,
        direction[1] / dir_mag,
        direction[2] / dir_mag,
    ];

    let baseline = match catch_unwind(AssertUnwindSafe(|| {
        candidate.intersection(&host.solid).volume()
    })) {
        Ok(v) if v.is_finite() && v >= 0.0 => v,
        _ => 0.0,
    };

    const N_SAMPLES: usize = 8;
    let mut peak = baseline;
    let mut samples = Vec::with_capacity(N_SAMPLES);
    for i in 1..=N_SAMPLES {
        let t = i as f64 / N_SAMPLES as f64;
        let dx = unit[0] * displacement_mm * t;
        let dy = unit[1] * displacement_mm * t;
        let dz = unit[2] * displacement_mm * t;
        let translated = candidate.translate(dx, dy, dz);
        let v = match catch_unwind(AssertUnwindSafe(|| {
            translated.intersection(&host.solid).volume()
        })) {
            Ok(v) if v.is_finite() && v >= 0.0 => v,
            _ => 0.0,
        };
        samples.push((t * displacement_mm, v));
        if v > peak {
            peak = v;
        }
    }

    let gain = peak - baseline;
    let pass = gain >= min_interference_gain_mm3;
    (
        if pass { CheckOutcome::Pass } else { CheckOutcome::Fail },
        json!({
            "baseline_interference_mm3": baseline,
            "peak_interference_mm3": peak,
            "interference_gain_mm3": gain,
            "min_interference_gain_mm3": min_interference_gain_mm3,
            "displacement_mm": displacement_mm,
            "direction": direction,
            "samples": samples.iter().map(|(d, v)| json!([d, v])).collect::<Vec<_>>(),
        }),
    )
}

// ---------- mate_clearance ---------------------------------------------

/// `mate_clearance`: minimum distance between any candidate vertex and
/// any host triangle. Negative values would indicate interpenetration,
/// but this implementation can't sign — pair with `interference_volume`
/// to detect overlap, then enforce `min_mm`/`max_mm` here.
pub fn check_mate_clearance(
    candidate: &Solid,
    host: &HostGeometry,
    min_mm: f64,
    max_mm: f64,
) -> (CheckOutcome, serde_json::Value) {
    let cand_mesh = match catch_unwind(AssertUnwindSafe(|| {
        candidate.to_mesh(FIT_TESSELLATION_SEGMENTS)
    })) {
        Ok(m) => m,
        Err(_) => {
            return (
                CheckOutcome::Fail,
                json!({ "reason": "candidate tessellation panicked" }),
            );
        }
    };
    let host_tris = mesh_tris(&host.mesh);
    if host_tris.is_empty() {
        return (
            CheckOutcome::Fail,
            json!({ "reason": "host mesh empty" }),
        );
    }

    let mut min_sq = f64::INFINITY;
    let n_verts = cand_mesh.vertices.len() / 3;
    for vi in 0..n_verts {
        let p = vert(&cand_mesh, vi as u32);
        for ht in &host_tris {
            let d = point_tri_distance_sq(p, ht);
            if d < min_sq {
                min_sq = d;
            }
        }
    }
    let actual = min_sq.sqrt();
    let pass = actual >= min_mm && actual <= max_mm;
    (
        if pass { CheckOutcome::Pass } else { CheckOutcome::Fail },
        json!({
            "min_separation_mm": actual,
            "min_mm": min_mm,
            "max_mm": max_mm,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::{StructuredInput, TaskInput};

    fn cube_vcad(size: f64, offset: [f64; 3]) -> String {
        // Cube primitive sits corner-at-origin per IR convention; translate
        // to place it deliberately.
        format!(
            r#"{{
                "version":"0.1",
                "nodes":{{
                    "1":{{"id":1,"name":"c","op":{{"type":"Cube","size":{{"x":{s},"y":{s},"z":{s}}}}}}},
                    "2":{{"id":2,"name":"t","op":{{"type":"Translate","child":1,"offset":{{"x":{ox},"y":{oy},"z":{oz}}}}}}}
                }},
                "materials":{{}},
                "part_materials":{{}},
                "roots":[{{"root":2,"material":"default"}}]
            }}"#,
            s = size,
            ox = offset[0],
            oy = offset[1],
            oz = offset[2],
        )
    }

    fn make_host_from_vcad(raw: &str) -> HostGeometry {
        let snap = evaluate_vcad(raw);
        let solid = aggregate_solid(&snap).expect("host has a solid");
        let mesh = solid.to_mesh(FIT_TESSELLATION_SEGMENTS);
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
    fn envelope_passes_when_within_caps() {
        let snap = evaluate_vcad(&cube_vcad(10.0, [0.0, 0.0, 0.0]));
        let (out, _details) = check_envelope(&snap, [10.5, 10.5, 10.5]);
        assert_eq!(out, CheckOutcome::Pass);
    }

    #[test]
    fn envelope_fails_when_too_big() {
        let snap = evaluate_vcad(&cube_vcad(20.0, [0.0, 0.0, 0.0]));
        let (out, _details) = check_envelope(&snap, [10.0, 10.0, 10.0]);
        assert_eq!(out, CheckOutcome::Fail);
    }

    #[test]
    fn interference_volume_zero_for_disjoint_solids() {
        // Host: 10mm cube at origin (0..10). Candidate: 10mm cube at
        // (20,0,0) — disjoint; AABBs don't overlap.
        let host = make_host_from_vcad(&cube_vcad(10.0, [0.0, 0.0, 0.0]));
        let snap = evaluate_vcad(&cube_vcad(10.0, [20.0, 0.0, 0.0]));
        let cand = aggregate_candidate(&snap).expect("candidate solid");
        let (out, details) = check_interference_volume(&cand, &host, 0.5);
        assert_eq!(out, CheckOutcome::Pass, "{:?}", details);
        assert_eq!(details["interference_mm3"].as_f64().unwrap(), 0.0);
    }

    #[test]
    fn interference_volume_detects_full_overlap() {
        // Two coincident 10mm cubes — intersection volume is 1000.
        let host = make_host_from_vcad(&cube_vcad(10.0, [0.0, 0.0, 0.0]));
        let snap = evaluate_vcad(&cube_vcad(10.0, [0.0, 0.0, 0.0]));
        let cand = aggregate_candidate(&snap).expect("candidate solid");
        let (out, details) = check_interference_volume(&cand, &host, 1.0);
        assert_eq!(out, CheckOutcome::Fail, "{:?}", details);
        let v = details["interference_mm3"].as_f64().unwrap();
        assert!(v > 990.0 && v < 1010.0, "expected ~1000, got {}", v);
    }

    #[test]
    fn mate_clearance_measures_disjoint_gap() {
        // Host at origin (0..10); candidate at (15, 0, 0) → 5mm gap.
        let host = make_host_from_vcad(&cube_vcad(10.0, [0.0, 0.0, 0.0]));
        let snap = evaluate_vcad(&cube_vcad(10.0, [15.0, 0.0, 0.0]));
        let cand = aggregate_candidate(&snap).expect("candidate solid");
        let (out, details) = check_mate_clearance(&cand, &host, 4.5, 5.5);
        assert_eq!(out, CheckOutcome::Pass, "{:?}", details);
        let m = details["min_separation_mm"].as_f64().unwrap();
        assert!(
            (m - 5.0).abs() < 0.1,
            "expected ~5mm clearance, got {}",
            m
        );
    }

    #[test]
    fn contact_area_finds_contact_on_touching_cubes() {
        // Two 10mm cubes touching face-to-face along x=10 plane.
        let host = make_host_from_vcad(&cube_vcad(10.0, [0.0, 0.0, 0.0]));
        let snap = evaluate_vcad(&cube_vcad(10.0, [10.0, 0.0, 0.0]));
        let cand = aggregate_candidate(&snap).expect("candidate solid");
        // Contact face is 10x10 = 100 mm² nominal. Centroid-only metric
        // typically recovers ≥ ~80% of this; require 60 to leave headroom.
        let (out, details) = check_contact_area(&cand, &host, 0.2, 60.0);
        assert_eq!(out, CheckOutcome::Pass, "{:?}", details);
    }

    /// Smoke test: load the f1-spacer-shaft-01 host file directly and
    /// verify it evaluates to a valid solid. Does not exercise the
    /// candidate-side checks (those need a hand-rolled spacer .vcad).
    #[test]
    fn loads_f1_spacer_host_geometry() {
        // Cargo runs tests from the crate root (mecheval/graders/).
        let task_dir = std::path::PathBuf::from("../tasks");
        let raw = std::fs::read_to_string(task_dir.join("assets/f1-spacer-shaft-01/host.vcad"))
            .expect("read host.vcad");
        let snap = evaluate_vcad(&raw);
        let solid = aggregate_solid(&snap).expect("host evaluates");
        let v = solid.volume();
        // Bottom flange (π·400·10) + waist (π·100·30) + top flange (π·400·10)
        // = π·(4000 + 3000 + 4000) = π·11000 ≈ 34,557.5 mm³.
        assert!(
            v > 34_000.0 && v < 35_000.0,
            "expected ~34,557 mm³, got {}",
            v
        );
    }

    /// Verify the structured input loader picks up the host_geometry
    /// entry from the f1-spacer-shaft-01 task.
    #[test]
    fn task_private_input_finds_host_geometry() {
        let task = crate::task::load_task(std::path::Path::new(
            "../tasks/f1-spacer-shaft-01.json",
        ))
        .expect("load task");
        let host_input = task
            .private_input("host_geometry")
            .expect("task has host_geometry");
        assert!(matches!(
            host_input,
            StructuredInput { agent_visible: false, .. }
        ));
        assert!(host_input.path.is_some());
        // Sanity: the agent_visible inputs (3 photos + 1 known dim) total 4.
        let visible: Vec<_> = task
            .inputs
            .iter()
            .filter(|i| matches!(i, TaskInput::Structured(s) if s.agent_visible))
            .collect();
        assert_eq!(visible.len(), 4);
    }
}
