//! Top-level grader entry point.
//!
//! Loads a task and a candidate `.vcad` file, dispatches each check, and
//! returns a [`RunBlob`]. v0.0 skeleton: every kernel-dependent check
//! returns [`CheckOutcome::NotImplemented`]. Wiring the dispatch to the
//! `vcad-kernel-*` crates is the next step.

use crate::blob::{CheckOutcome, CheckRecord, RunBlob, Summary, SCHEMA_VERSION};
use crate::check::CheckSpec;
use crate::eval::{evaluate_vcad, EvalSnapshot};
use crate::fillets::find_non_z_cylinders;
use crate::fit::{
    aggregate_candidate, check_contact_area, check_envelope, check_interference_volume,
    check_mate_clearance, check_pull_retention_geometric, load_host, HostError, HostGeometry,
};
use crate::fit_physics::{check_gravity_hold, check_pull_force};
use crate::holes::find_z_holes;
use crate::suite_c::{
    build_assembly_snapshot, check_body_valid, check_fk_reaches, check_task_success,
    check_torque_budget, AssemblySnapshot,
};
use crate::task::Task;
use serde_json::json;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use thiserror::Error;

/// Errors the grader can surface to its caller.
#[derive(Debug, Error)]
pub enum GraderError {
    /// Candidate `.vcad` file could not be read.
    #[error("could not read .vcad: {0}")]
    Io(#[from] std::io::Error),
    /// Candidate `.vcad` is not valid JSON.
    #[error("malformed .vcad: {0}")]
    Vcad(#[from] serde_json::Error),
    /// Task JSON could not be hashed.
    #[error("task hash failed: {0}")]
    HashFailed(String),
}

/// Grade a candidate `.vcad` file against a task.
///
/// `task_json_bytes` is the raw bytes of the task JSON the grader was loaded
/// from — used to compute `task_sha256` for forensic traceability. (We hash
/// the bytes the grader actually saw, not the file at re-read time, to
/// avoid a TOCTOU window.) `task_dir` is the directory the task file lives
/// in; used to resolve `inputs[].path` (e.g. Suite F host geometry).
pub fn grade(
    task: &Task,
    task_json_bytes: &[u8],
    candidate_vcad: &Path,
    task_dir: &Path,
) -> Result<RunBlob, GraderError> {
    // Read + sanity-check the candidate.
    let candidate_raw = std::fs::read_to_string(candidate_vcad)?;
    let _candidate_value: serde_json::Value = serde_json::from_str(&candidate_raw)?;

    // Evaluate once up front; every check shares the snapshot.
    let snapshot = evaluate_vcad(&candidate_raw);

    // Build a Suite-C assembly snapshot lazily — only if the task has
    // physics checks. Failures here surface as the per-check `Fail`
    // reason via [`AssemblyState`].
    let mut assembly = if task.checks.iter().any(CheckSpec::is_suite_c) {
        match snapshot.doc.as_ref() {
            Some(doc) => match build_assembly_snapshot(doc) {
                Ok(s) => AssemblyState::Built(Box::new(s)),
                Err(e) => AssemblyState::BuildError(e),
            },
            None => AssemblyState::BuildError("document failed to parse".into()),
        }
    } else {
        AssemblyState::NotNeeded
    };

    let task_target = first_fk_target(task);

    // Lazily build the host snapshot (Suite F) on first use; share across
    // every fit check that references the same host.
    let mut host_state = if task.checks.iter().any(CheckSpec::is_suite_f) {
        match load_host(task, task_dir) {
            Ok(h) => HostState::Loaded(Box::new(h)),
            Err(e) => HostState::Error(e.to_string()),
        }
    } else {
        HostState::NotNeeded
    };
    // Lazily build the target snapshot (Suite D); shared across visual checks.
    let mut target_state = if task.checks.iter().any(CheckSpec::is_suite_d) {
        match crate::fit::load_private_solid(task, task_dir, "target_mesh") {
            Ok(h) => HostState::Loaded(Box::new(h)),
            Err(e) => HostState::Error(e.to_string()),
        }
    } else {
        HostState::NotNeeded
    };
    // Build the candidate solid once (used by every fit/visual check that
    // operates on it). Cheap if no F or D checks present.
    let needs_candidate = task.checks.iter().any(CheckSpec::is_suite_f)
        || task.checks.iter().any(CheckSpec::is_suite_d);
    let candidate_solid = if needs_candidate {
        aggregate_candidate(&snapshot)
    } else {
        None
    };

    // Suite E: preload the grader-only golden netlist (by input kind) so
    // the dispatch stays synchronous and path-free.
    let golden_netlist: Option<Result<String, String>> = task
        .checks
        .iter()
        .find_map(|c| match c {
            CheckSpec::NetlistIsomorphic { golden } => Some(golden.clone()),
            _ => None,
        })
        .map(|kind| {
            let input = task
                .private_input(&kind)
                .ok_or_else(|| format!("no inputs[] entry of kind '{kind}'"))?;
            let rel = input
                .path
                .as_ref()
                .ok_or_else(|| format!("inputs[] entry '{kind}' has no path"))?;
            std::fs::read_to_string(task_dir.join(rel)).map_err(|e| e.to_string())
        });

    let mut records: Vec<CheckRecord> = Vec::with_capacity(task.checks.len());
    for (n, spec) in task.checks.iter().enumerate() {
        let (outcome, details) = run_check(
            spec,
            &snapshot,
            &mut assembly,
            task,
            task_target,
            candidate_solid.as_ref(),
            &mut host_state,
            &mut target_state,
            golden_netlist.as_ref(),
        );
        records.push(CheckRecord {
            n,
            r#type: spec.kind().to_string(),
            params: spec.clone(),
            result: outcome,
            details,
        });
    }

    // Limits enforcement (token counts, wall-clock, tool calls) is wired
    // by the harness, which has the trace data — that stays Vec::new()
    // here. Structural anti-cheese, on the other hand, is a function of
    // the candidate document only, so we run it.
    let (anti_cheese_violated, _ac_violations) = crate::anti_cheese::run(task, &snapshot, task_dir);
    let limits_exceeded: Vec<String> = Vec::new();

    let summary = Summary::from_records(&records, anti_cheese_violated, limits_exceeded);
    let task_sha256 = sha256_hex(task_json_bytes);

    Ok(RunBlob {
        schema_version: SCHEMA_VERSION,
        task_id: task.id.clone(),
        task_sha256,
        checks: records,
        summary,
    })
}

/// State of the lazily-built Suite-C assembly snapshot.
enum AssemblyState {
    NotNeeded,
    Built(Box<AssemblySnapshot>),
    BuildError(String),
}

/// State of the lazily-loaded Suite-F host geometry.
enum HostState {
    NotNeeded,
    Loaded(Box<HostGeometry>),
    Error(String),
}

#[allow(dead_code)]
impl From<HostError> for HostState {
    fn from(e: HostError) -> Self {
        HostState::Error(e.to_string())
    }
}

/// Pull the first `fk_reaches` check's target/tolerance out of the
/// task — `torque_budget` and `task_success` reuse it as the IK goal.
fn first_fk_target(task: &Task) -> Option<([f64; 3], f64)> {
    for c in &task.checks {
        if let CheckSpec::FkReaches {
            target,
            tolerance_m,
        } = c
        {
            return Some((*target, *tolerance_m));
        }
    }
    None
}

/// Dispatch one check against the prebuilt evaluation snapshot. Checks
/// that haven't been wired yet return [`CheckOutcome::NotImplemented`].
#[allow(clippy::too_many_arguments)]
fn run_check(
    spec: &CheckSpec,
    snapshot: &EvalSnapshot,
    assembly: &mut AssemblyState,
    task: &Task,
    task_target: Option<([f64; 3], f64)>,
    candidate_solid: Option<&vcad_kernel::Solid>,
    host_state: &mut HostState,
    target_state: &mut HostState,
    golden_netlist: Option<&Result<String, String>>,
) -> (CheckOutcome, serde_json::Value) {
    let stub_reason = "skeleton — kernel wiring pending";
    match spec {
        CheckSpec::ValidSolid => check_valid_solid(snapshot),

        CheckSpec::Bbox {
            min,
            max,
            tolerance_mm,
        } => check_bbox(snapshot, *min, *max, *tolerance_mm),

        CheckSpec::MassProps {
            volume_mm3,
            surface_area_mm2,
            center_of_mass,
            tolerance_pct,
        } => check_mass_props(
            snapshot,
            *volume_mm3,
            *surface_area_mm2,
            *center_of_mass,
            *tolerance_pct,
        ),

        CheckSpec::StepRoundtrip { tolerance_pct } => {
            check_step_roundtrip(snapshot, *tolerance_pct)
        }

        CheckSpec::HoleCount {
            diameter_mm,
            expected,
            diameter_tolerance_mm,
        } => check_hole_count(snapshot, *diameter_mm, *expected, *diameter_tolerance_mm),

        CheckSpec::HolePositions {
            diameter_mm,
            positions,
            tolerance_mm,
        } => check_hole_positions(snapshot, *diameter_mm, positions, *tolerance_mm),

        CheckSpec::FilletRadius {
            edge_class,
            radius_mm,
            tolerance_mm,
        } => check_fillet_radius(snapshot, edge_class, *radius_mm, *tolerance_mm),

        CheckSpec::Dfm {
            rules,
            process,
            max_severity,
        } => crate::dfm::check_dfm(snapshot, process.as_deref(), max_severity.as_deref(), rules),

        CheckSpec::DrcClean => crate::pcb::check_drc_clean(snapshot),
        CheckSpec::ErcClean => crate::pcb::check_erc_clean(snapshot),
        CheckSpec::NetsFullyConnected => crate::pcb::check_nets_fully_connected(snapshot),
        CheckSpec::BoardEnvelope { max_mm } => crate::pcb::check_board_envelope(snapshot, *max_mm),
        CheckSpec::ComponentCount { min } => crate::pcb::check_component_count(snapshot, *min),
        CheckSpec::DecouplingProximity {
            power_nets,
            ground_nets,
            max_mm,
            min_ic_pads,
        } => crate::pcb::check_decoupling_proximity(
            snapshot,
            power_nets,
            ground_nets,
            *max_mm,
            *min_ic_pads,
        ),
        CheckSpec::NetlistIsomorphic { .. } => {
            crate::pcb::check_netlist_isomorphic(snapshot, golden_netlist)
        }
        CheckSpec::FabReady => crate::pcb::check_fab_ready(snapshot),

        CheckSpec::RefactorInvariant { .. } => (
            CheckOutcome::NotImplemented,
            json!({ "reason": stub_reason }),
        ),

        CheckSpec::BodyValid => match assembly {
            AssemblyState::Built(snap) => check_body_valid(snap),
            AssemblyState::BuildError(e) => (
                CheckOutcome::Fail,
                json!({ "reason": "could not build physics world", "error": e }),
            ),
            AssemblyState::NotNeeded => (
                CheckOutcome::NotImplemented,
                json!({ "reason": "internal: body_valid dispatched without assembly" }),
            ),
        },
        CheckSpec::FkReaches {
            target,
            tolerance_m,
        } => match assembly {
            AssemblyState::Built(snap) => check_fk_reaches(snap, *target, *tolerance_m),
            AssemblyState::BuildError(e) => (
                CheckOutcome::Fail,
                json!({ "reason": "could not build physics world", "error": e }),
            ),
            AssemblyState::NotNeeded => (
                CheckOutcome::NotImplemented,
                json!({ "reason": "internal: fk_reaches dispatched without assembly" }),
            ),
        },
        CheckSpec::TorqueBudget {
            payload_kg,
            safety_factor,
        } => match assembly {
            AssemblyState::Built(snap) => {
                let (target, tol) = task_target.unwrap_or(([0.0, 0.0, 0.0], 0.005));
                check_torque_budget(
                    snap,
                    *payload_kg,
                    *safety_factor,
                    task.anti_cheese.joint_torque_ceiling_nm,
                    target,
                    tol,
                )
            }
            AssemblyState::BuildError(e) => (
                CheckOutcome::Fail,
                json!({ "reason": "could not build physics world", "error": e }),
            ),
            AssemblyState::NotNeeded => (
                CheckOutcome::NotImplemented,
                json!({ "reason": "internal: torque_budget dispatched without assembly" }),
            ),
        },
        CheckSpec::TaskSuccess { task: _, params } => match assembly {
            AssemblyState::Built(snap) => check_task_success(
                snap,
                params,
                task_target.map(|(t, _)| t),
                task_target.map(|(_, tol)| tol),
            ),
            AssemblyState::BuildError(e) => (
                CheckOutcome::Fail,
                json!({ "reason": "could not build physics world", "error": e }),
            ),
            AssemblyState::NotNeeded => (
                CheckOutcome::NotImplemented,
                json!({ "reason": "internal: task_success dispatched without assembly" }),
            ),
        },
        CheckSpec::StableDuringRollout { .. } => (
            CheckOutcome::NotImplemented,
            json!({ "reason": "no current task uses stable_during_rollout" }),
        ),

        CheckSpec::Envelope { max_mm } => check_envelope(snapshot, *max_mm),

        CheckSpec::InterferenceVolume { host: _, max_mm3 } => {
            dispatch_with_host(host_state, candidate_solid, |cand, host| {
                check_interference_volume(cand, host, *max_mm3)
            })
        }
        CheckSpec::ContactArea {
            host: _,
            epsilon_mm,
            min_mm2,
        } => dispatch_with_host(host_state, candidate_solid, |cand, host| {
            check_contact_area(cand, host, *epsilon_mm, *min_mm2)
        }),
        CheckSpec::MateClearance {
            host: _,
            min_mm,
            max_mm,
        } => dispatch_with_host(host_state, candidate_solid, |cand, host| {
            check_mate_clearance(cand, host, *min_mm, *max_mm)
        }),
        CheckSpec::GravityHold {
            host: _,
            host_mass_kg,
            gravity_dir,
            duration_sec,
            max_drift_mm,
        } => dispatch_with_host(host_state, candidate_solid, |cand, host| {
            check_gravity_hold(
                cand,
                host,
                *host_mass_kg,
                *gravity_dir,
                *duration_sec,
                *max_drift_mm,
            )
        }),
        CheckSpec::PullForce {
            host: _,
            force_n,
            direction,
            duration_sec,
            max_drift_mm,
        } => dispatch_with_host(host_state, candidate_solid, |cand, host| {
            check_pull_force(
                cand,
                host,
                *force_n,
                *direction,
                *duration_sec,
                *max_drift_mm,
            )
        }),
        CheckSpec::PullRetentionGeometric {
            host: _,
            direction,
            displacement_mm,
            min_interference_gain_mm3,
        } => dispatch_with_host(host_state, candidate_solid, |cand, host| {
            check_pull_retention_geometric(
                cand,
                host,
                *direction,
                *displacement_mm,
                *min_interference_gain_mm3,
            )
        }),
        CheckSpec::ShapeSimilarityChamfer {
            target: _,
            max_chamfer_mm,
        } => dispatch_with_host(target_state, candidate_solid, |cand, target| {
            crate::visual::check_shape_similarity_chamfer(cand, target, *max_chamfer_mm)
        }),
    }
}

/// Helper: gate a fit check on (a) candidate evaluating to a solid and
/// (b) host loading successfully. Closure runs the actual check.
fn dispatch_with_host<F>(
    host_state: &HostState,
    candidate_solid: Option<&vcad_kernel::Solid>,
    f: F,
) -> (CheckOutcome, serde_json::Value)
where
    F: FnOnce(&vcad_kernel::Solid, &HostGeometry) -> (CheckOutcome, serde_json::Value),
{
    let cand = match candidate_solid {
        Some(c) => c,
        None => {
            return (
                CheckOutcome::Fail,
                json!({ "reason": "candidate produced no valid solid" }),
            );
        }
    };
    let host = match host_state {
        HostState::Loaded(h) => h,
        HostState::Error(e) => {
            return (
                CheckOutcome::Fail,
                json!({ "reason": "host geometry could not be loaded", "error": e }),
            );
        }
        HostState::NotNeeded => {
            return (
                CheckOutcome::NotImplemented,
                json!({ "reason": "internal: fit check dispatched without host" }),
            );
        }
    };
    f(cand, host)
}

/// `valid_solid`: candidate evaluates cleanly, has at least one root, and
/// at least one root produces a non-empty solid with positive volume.
fn check_valid_solid(snap: &EvalSnapshot) -> (CheckOutcome, serde_json::Value) {
    if let Some(fatal) = &snap.fatal {
        return (
            CheckOutcome::Fail,
            json!({ "reason": "fatal evaluation error", "error": fatal }),
        );
    }
    if !snap.root_failures.is_empty() {
        // A partially-failed eval is still a fail, but distinguishable from
        // a clean-but-empty doc.
        return (
            CheckOutcome::Fail,
            json!({
                "reason": "one or more roots failed to evaluate",
                "root_failures": snap.root_failures,
                "solids_produced": snap.solids.len(),
            }),
        );
    }
    if snap.solids.is_empty() {
        return (
            CheckOutcome::Fail,
            json!({ "reason": "no solids produced", "root_count": snap.root_count }),
        );
    }
    if !snap.has_any_valid_solid() {
        return (
            CheckOutcome::Fail,
            json!({
                "reason": "evaluated but every solid is empty or zero-volume",
                "solids_produced": snap.solids.len(),
            }),
        );
    }
    (
        CheckOutcome::Pass,
        json!({
            "solids_produced": snap.solids.len(),
            "root_count": snap.root_count,
        }),
    )
}

/// `bbox`: aggregate AABB across all evaluated solids must match the spec
/// within `tolerance_mm` on every face (six bounds total).
fn check_bbox(
    snap: &EvalSnapshot,
    spec_min: [f64; 3],
    spec_max: [f64; 3],
    tolerance_mm: f64,
) -> (CheckOutcome, serde_json::Value) {
    let (actual_min, actual_max) = match snap.aggregate_bbox() {
        Some(b) => b,
        None => {
            return (
                CheckOutcome::Fail,
                json!({ "reason": "no valid solid to measure" }),
            );
        }
    };

    let dev_min: [f64; 3] = [
        actual_min[0] - spec_min[0],
        actual_min[1] - spec_min[1],
        actual_min[2] - spec_min[2],
    ];
    let dev_max: [f64; 3] = [
        actual_max[0] - spec_max[0],
        actual_max[1] - spec_max[1],
        actual_max[2] - spec_max[2],
    ];

    let max_abs_dev = dev_min
        .iter()
        .chain(dev_max.iter())
        .fold(0.0_f64, |acc, d| acc.max(d.abs()));

    let outcome = if max_abs_dev <= tolerance_mm {
        CheckOutcome::Pass
    } else {
        CheckOutcome::Fail
    };

    (
        outcome,
        json!({
            "actual_min": actual_min,
            "actual_max": actual_max,
            "deviation_min": dev_min,
            "deviation_max": dev_max,
            "max_abs_deviation_mm": max_abs_dev,
            "tolerance_mm": tolerance_mm,
        }),
    )
}

/// `mass_props`: any subset of (volume, surface_area, center_of_mass)
/// must match within `tolerance_pct`.
///
/// - Volume / surface_area: relative tolerance — `|actual - spec| / |spec| ≤ tolerance_pct/100`.
/// - Center of mass: per-component absolute tolerance derived as
///   `tolerance_pct/100 * actual_bbox_diagonal`, with a 0.01mm floor so
///   solids at the origin still admit a sensible deviation.
fn check_mass_props(
    snap: &EvalSnapshot,
    spec_volume: Option<f64>,
    spec_surface_area: Option<f64>,
    spec_com: Option<[f64; 3]>,
    tolerance_pct: f64,
) -> (CheckOutcome, serde_json::Value) {
    if snap.solids.is_empty() {
        return (
            CheckOutcome::Fail,
            json!({ "reason": "no valid solid to measure" }),
        );
    }

    let mut details = serde_json::Map::new();
    let mut all_pass = true;

    if let Some(spec) = spec_volume {
        match snap.aggregate_volume() {
            Some(actual) => {
                let dev_pct = ((actual - spec) / spec).abs() * 100.0;
                let pass = dev_pct <= tolerance_pct;
                all_pass &= pass;
                details.insert(
                    "volume".into(),
                    json!({
                        "spec_mm3": spec,
                        "actual_mm3": actual,
                        "deviation_pct": dev_pct,
                        "tolerance_pct": tolerance_pct,
                        "pass": pass,
                    }),
                );
            }
            None => {
                all_pass = false;
                details.insert(
                    "volume".into(),
                    json!({ "reason": "no positive volume produced", "pass": false }),
                );
            }
        }
    }

    if let Some(spec) = spec_surface_area {
        match snap.aggregate_surface_area() {
            Some(actual) => {
                let dev_pct = ((actual - spec) / spec).abs() * 100.0;
                let pass = dev_pct <= tolerance_pct;
                all_pass &= pass;
                details.insert(
                    "surface_area".into(),
                    json!({
                        "spec_mm2": spec,
                        "actual_mm2": actual,
                        "deviation_pct": dev_pct,
                        "tolerance_pct": tolerance_pct,
                        "pass": pass,
                    }),
                );
            }
            None => {
                all_pass = false;
                details.insert(
                    "surface_area".into(),
                    json!({ "reason": "no positive surface area produced", "pass": false }),
                );
            }
        }
    }

    if let Some(spec) = spec_com {
        let bbox_diag = snap
            .aggregate_bbox()
            .map(|(lo, hi)| {
                let d = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
                (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
            })
            .unwrap_or(0.0);
        let com_tol_mm = (tolerance_pct / 100.0 * bbox_diag).max(0.01);

        match snap.aggregate_center_of_mass() {
            Some(actual) => {
                let dev = [
                    actual[0] - spec[0],
                    actual[1] - spec[1],
                    actual[2] - spec[2],
                ];
                let max_abs = dev.iter().fold(0.0_f64, |a, d| a.max(d.abs()));
                let pass = max_abs <= com_tol_mm;
                all_pass &= pass;
                details.insert(
                    "center_of_mass".into(),
                    json!({
                        "spec": spec,
                        "actual": actual,
                        "deviation": dev,
                        "max_abs_deviation_mm": max_abs,
                        "tolerance_mm": com_tol_mm,
                        "bbox_diagonal_mm": bbox_diag,
                        "pass": pass,
                    }),
                );
            }
            None => {
                all_pass = false;
                details.insert(
                    "center_of_mass".into(),
                    json!({ "reason": "could not compute COM", "pass": false }),
                );
            }
        }
    }

    if details.is_empty() {
        // Spec asked for nothing — vacuously pass, but flag it as a likely
        // task-authoring mistake.
        return (
            CheckOutcome::Pass,
            json!({ "warning": "mass_props check had no spec subfields" }),
        );
    }

    (
        if all_pass {
            CheckOutcome::Pass
        } else {
            CheckOutcome::Fail
        },
        serde_json::Value::Object(details),
    )
}

/// `step_roundtrip`: write each evaluated solid to STEP, read it back,
/// and confirm volume + bbox extents agree within `tolerance_pct`.
///
/// Catches kernels that emit STEP they can't re-import, or that lose
/// material on export — both of which would otherwise pass A1/A2 in
/// vcad's own gym while breaking interop with every other CAD tool.
fn check_step_roundtrip(
    snap: &EvalSnapshot,
    tolerance_pct: f64,
) -> (CheckOutcome, serde_json::Value) {
    use vcad_kernel::Solid;
    use vcad_kernel_step::{read_step_from_buffer, write_step_to_buffer};

    if snap.solids.is_empty() {
        return (
            CheckOutcome::Fail,
            json!({ "reason": "no valid solid to round-trip" }),
        );
    }

    let mut details = serde_json::Map::new();
    let mut per_solid = Vec::new();
    let mut all_pass = true;

    for (idx, solid) in snap.solids.iter().enumerate() {
        let brep = match solid.as_brep() {
            Some(b) => b,
            None => {
                all_pass = false;
                per_solid.push(json!({
                    "index": idx,
                    "pass": false,
                    "reason": "solid is not BRep-backed (mesh-only after a boolean fallback?)",
                }));
                continue;
            }
        };

        let original_volume = catch_unwind_volume(solid);
        let original_bbox = catch_unwind_bbox(solid);

        // Export.
        let buffer = match catch_unwind(AssertUnwindSafe(|| write_step_to_buffer(brep))) {
            Ok(Ok(b)) => b,
            Ok(Err(e)) => {
                all_pass = false;
                per_solid.push(json!({
                    "index": idx,
                    "pass": false,
                    "stage": "export",
                    "error": format!("{}", e),
                }));
                continue;
            }
            Err(_) => {
                all_pass = false;
                per_solid.push(json!({
                    "index": idx,
                    "pass": false,
                    "stage": "export",
                    "error": "panic in write_step_to_buffer",
                }));
                continue;
            }
        };

        // Re-import.
        let imported_breps = match catch_unwind(AssertUnwindSafe(|| read_step_from_buffer(&buffer)))
        {
            Ok(Ok(b)) => b,
            Ok(Err(e)) => {
                all_pass = false;
                per_solid.push(json!({
                    "index": idx,
                    "pass": false,
                    "stage": "import",
                    "error": format!("{}", e),
                }));
                continue;
            }
            Err(_) => {
                all_pass = false;
                per_solid.push(json!({
                    "index": idx,
                    "pass": false,
                    "stage": "import",
                    "error": "panic in read_step_from_buffer",
                }));
                continue;
            }
        };

        if imported_breps.is_empty() {
            all_pass = false;
            per_solid.push(json!({
                "index": idx,
                "pass": false,
                "stage": "import",
                "error": "round-trip yielded zero solids",
            }));
            continue;
        }

        // Sum mass props across all imported BReps (a single STEP file may
        // round-trip into multiple solids if the writer split shells).
        let mut imp_volume = 0.0_f64;
        let mut imp_bbox: Option<([f64; 3], [f64; 3])> = None;
        for brep in &imported_breps {
            let s = Solid::from_brep(brep.clone());
            if let Some(v) = catch_unwind_volume(&s) {
                imp_volume += v;
            }
            if let Some(bb) = catch_unwind_bbox(&s) {
                imp_bbox = Some(match imp_bbox {
                    None => bb,
                    Some((al, ah)) => (
                        [al[0].min(bb.0[0]), al[1].min(bb.0[1]), al[2].min(bb.0[2])],
                        [ah[0].max(bb.1[0]), ah[1].max(bb.1[1]), ah[2].max(bb.1[2])],
                    ),
                });
            }
        }

        // Compare.
        let mut entry = serde_json::Map::new();
        entry.insert("index".into(), json!(idx));
        let mut this_pass = true;

        if let (Some(orig), imp) = (original_volume, imp_volume) {
            let dev_pct = if orig > 0.0 {
                ((imp - orig) / orig).abs() * 100.0
            } else {
                f64::INFINITY
            };
            let pass = dev_pct <= tolerance_pct;
            this_pass &= pass;
            entry.insert(
                "volume".into(),
                json!({
                    "original_mm3": orig,
                    "roundtripped_mm3": imp,
                    "deviation_pct": dev_pct,
                    "pass": pass,
                }),
            );
        }

        if let (Some((omin, omax)), Some((imin, imax))) = (original_bbox, imp_bbox) {
            let max_abs_dev = (0..3)
                .flat_map(|i| [(omin[i] - imin[i]).abs(), (omax[i] - imax[i]).abs()])
                .fold(0.0_f64, |a, d| a.max(d));
            // Use tolerance_pct of the original bbox diagonal as the absolute
            // mm tolerance for bbox extent shifts (with a 0.01mm floor).
            let diag = ((omax[0] - omin[0]).powi(2)
                + (omax[1] - omin[1]).powi(2)
                + (omax[2] - omin[2]).powi(2))
            .sqrt();
            let bbox_tol_mm = (tolerance_pct / 100.0 * diag).max(0.01);
            let pass = max_abs_dev <= bbox_tol_mm;
            this_pass &= pass;
            entry.insert(
                "bbox".into(),
                json!({
                    "original_min": omin, "original_max": omax,
                    "roundtripped_min": imin, "roundtripped_max": imax,
                    "max_abs_deviation_mm": max_abs_dev,
                    "tolerance_mm": bbox_tol_mm,
                    "pass": pass,
                }),
            );
        }

        entry.insert("pass".into(), json!(this_pass));
        all_pass &= this_pass;
        per_solid.push(serde_json::Value::Object(entry));
    }

    details.insert("per_solid".into(), serde_json::Value::Array(per_solid));
    details.insert("tolerance_pct".into(), json!(tolerance_pct));

    (
        if all_pass {
            CheckOutcome::Pass
        } else {
            CheckOutcome::Fail
        },
        serde_json::Value::Object(details),
    )
}

/// `fillet_radius`: pass if the body has at least one non-Z-axis
/// cylindrical surface whose radius matches the spec within
/// `tolerance_mm`. v0.0 ignores `edge_class` — any matching cylinder
/// counts. Tightening to actual tangency-checked fillets is future work.
fn check_fillet_radius(
    snap: &EvalSnapshot,
    edge_class: &str,
    radius_mm: f64,
    tolerance_mm: f64,
) -> (CheckOutcome, serde_json::Value) {
    let found = find_non_z_cylinders(snap, radius_mm, tolerance_mm);
    let outcome = if found.is_empty() {
        CheckOutcome::Fail
    } else {
        CheckOutcome::Pass
    };
    (
        outcome,
        json!({
            "edge_class": edge_class,
            "edge_class_note": "v0.0 grader: edge_class is not yet enforced",
            "radius_mm": radius_mm,
            "tolerance_mm": tolerance_mm,
            "matched": found.iter().map(|f| json!({
                "radius": f.radius,
                "axis": f.axis,
                "axis_point": f.axis_point,
            })).collect::<Vec<_>>(),
        }),
    )
}

/// `hole_count`: count Z-axis cylindrical features of the target diameter.
fn check_hole_count(
    snap: &EvalSnapshot,
    diameter_mm: f64,
    expected: u32,
    diameter_tolerance_mm: f64,
) -> (CheckOutcome, serde_json::Value) {
    let holes = find_z_holes(snap, diameter_mm, diameter_tolerance_mm);
    let actual = holes.len() as u32;
    let outcome = if actual == expected {
        CheckOutcome::Pass
    } else {
        CheckOutcome::Fail
    };
    (
        outcome,
        json!({
            "diameter_mm": diameter_mm,
            "diameter_tolerance_mm": diameter_tolerance_mm,
            "expected": expected,
            "actual": actual,
            "found": holes.iter().map(|h| json!([h.x, h.y, 2.0 * h.radius])).collect::<Vec<_>>(),
        }),
    )
}

/// `hole_positions`: every expected (x, y) must match a detected hole's
/// axis position within `tolerance_mm`. Order-independent. Each detected
/// hole may match at most one expected position.
fn check_hole_positions(
    snap: &EvalSnapshot,
    diameter_mm: f64,
    expected: &[[f64; 3]],
    tolerance_mm: f64,
) -> (CheckOutcome, serde_json::Value) {
    let holes = find_z_holes(snap, diameter_mm, tolerance_mm * 2.0);

    let mut matched: Vec<bool> = vec![false; holes.len()];
    let mut per_expected = Vec::with_capacity(expected.len());
    let mut all_matched = true;

    for exp in expected {
        let mut best: Option<(usize, f64)> = None;
        for (i, h) in holes.iter().enumerate() {
            if matched[i] {
                continue;
            }
            let dx = h.x - exp[0];
            let dy = h.y - exp[1];
            let d = (dx * dx + dy * dy).sqrt();
            if best.is_none_or(|(_, bd)| d < bd) {
                best = Some((i, d));
            }
        }
        match best {
            Some((idx, dist)) if dist <= tolerance_mm => {
                matched[idx] = true;
                per_expected.push(json!({
                    "spec_xy": [exp[0], exp[1]],
                    "matched_hole": [holes[idx].x, holes[idx].y],
                    "distance_mm": dist,
                    "pass": true,
                }));
            }
            Some((_, dist)) => {
                all_matched = false;
                per_expected.push(json!({
                    "spec_xy": [exp[0], exp[1]],
                    "nearest_distance_mm": dist,
                    "tolerance_mm": tolerance_mm,
                    "pass": false,
                }));
            }
            None => {
                all_matched = false;
                per_expected.push(json!({
                    "spec_xy": [exp[0], exp[1]],
                    "pass": false,
                    "reason": "no candidate cylindrical feature of this diameter remained unmatched",
                }));
            }
        }
    }

    let unmatched_extras: Vec<_> = holes
        .iter()
        .zip(matched.iter())
        .filter_map(|(h, &m)| if !m { Some(json!([h.x, h.y])) } else { None })
        .collect();

    let outcome = if all_matched {
        CheckOutcome::Pass
    } else {
        CheckOutcome::Fail
    };

    (
        outcome,
        json!({
            "diameter_mm": diameter_mm,
            "tolerance_mm": tolerance_mm,
            "per_expected": per_expected,
            "unmatched_extras": unmatched_extras,
        }),
    )
}

fn catch_unwind_volume(s: &vcad_kernel::Solid) -> Option<f64> {
    catch_unwind(AssertUnwindSafe(|| s.volume()))
        .ok()
        .filter(|v| v.is_finite() && *v > 0.0)
}

fn catch_unwind_bbox(s: &vcad_kernel::Solid) -> Option<([f64; 3], [f64; 3])> {
    let bb = catch_unwind(AssertUnwindSafe(|| s.bounding_box())).ok()?;
    if (0..3).any(|i| bb.0[i] > bb.1[i]) {
        return None;
    }
    Some(bb)
}

/// SHA-256 hex of arbitrary bytes. Tiny pure-Rust implementation so we
/// don't pull in a crypto crate just for this.
fn sha256_hex(bytes: &[u8]) -> String {
    let digest = sha256(bytes);
    let mut s = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(s, "{:02x}", byte);
    }
    s
}

// Minimal in-tree SHA-256 to avoid a crypto dep in v0.0. Replace with
// a vetted crate (`sha2`) once the grader gains real wiring.
fn sha256(input: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let bit_len = (input.len() as u64).wrapping_mul(8);
    let mut msg = input.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in msg.as_chunks::<64>().0 {
        let mut w = [0u32; 64];
        for (i, word) in chunk.as_chunks::<4>().0.iter().enumerate() {
            w[i] = u32::from_be_bytes(*word);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let mut a = h[0];
        let mut b = h[1];
        let mut c = h[2];
        let mut d = h[3];
        let mut e = h[4];
        let mut f = h[5];
        let mut g = h[6];
        let mut hh = h[7];
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }
    let mut out = [0u8; 32];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_known_vector() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    /// A 25mm cube .vcad as a JSON string — used by valid_solid + bbox +
    /// mass_props tests below.
    fn cube_vcad(size: f64) -> String {
        format!(
            r#"{{"version":"0.1","nodes":{{"1":{{"id":1,"name":"cube","op":{{"type":"Cube","size":{{"x":{s},"y":{s},"z":{s}}}}}}}}},"materials":{{}},"part_materials":{{}},"roots":[{{"root":1,"material":"default"}}]}}"#,
            s = size
        )
    }

    fn task_with(checks: Vec<CheckSpec>) -> (Task, Vec<u8>) {
        let task = Task {
            id: "test-1".into(),
            suite: crate::task::Suite::A,
            tier: "A1".into(),
            title: "t".into(),
            prompt: "p".into(),
            inputs: vec![],
            checks,
            anti_cheese: Default::default(),
            limits: Default::default(),
            pass_k: 5,
            tags: vec![],
        };
        let bytes = serde_json::to_vec(&task).unwrap();
        (task, bytes)
    }

    fn write_tmp_vcad(name: &str, contents: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(name);
        std::fs::write(&p, contents).unwrap();
        p
    }

    #[test]
    fn unwired_checks_still_return_not_implemented() {
        // RefactorInvariant is still unwired; this catches accidental
        // dispatch leaks.
        let (task, task_bytes) = task_with(vec![CheckSpec::RefactorInvariant {
            untouched_parts: vec![],
            tolerance_pct: 0.01,
        }]);
        let tmp = write_tmp_vcad("mecheval-unwired.vcad", &cube_vcad(10.0));
        let blob = grade(&task, &task_bytes, &tmp, std::path::Path::new(".")).expect("grade");
        assert_eq!(blob.checks.len(), 1);
        assert_eq!(blob.checks[0].result, CheckOutcome::NotImplemented);
    }

    #[test]
    fn drc_on_a_bare_cube_fails_closed() {
        // The DEFAULT_CUBE villain must fail every ECAD check — a solid
        // with no PCB cannot vacuously pass drc_clean.
        let (task, task_bytes) = task_with(vec![CheckSpec::DrcClean]);
        let tmp = write_tmp_vcad("mecheval-drc-cube.vcad", &cube_vcad(10.0));
        let blob = grade(&task, &task_bytes, &tmp, std::path::Path::new(".")).expect("grade");
        assert_eq!(blob.checks.len(), 1);
        assert_eq!(blob.checks[0].result, CheckOutcome::Fail);
        assert!(!blob.summary.passed);
    }

    #[test]
    fn valid_solid_passes_for_a_real_cube() {
        let (task, task_bytes) = task_with(vec![CheckSpec::ValidSolid]);
        let tmp = write_tmp_vcad("mecheval-valid-cube.vcad", &cube_vcad(10.0));
        let blob = grade(&task, &task_bytes, &tmp, std::path::Path::new(".")).expect("grade");
        assert_eq!(blob.checks[0].result, CheckOutcome::Pass);
        assert!(blob.summary.passed);
    }

    #[test]
    fn valid_solid_fails_for_empty_doc() {
        let (task, task_bytes) = task_with(vec![CheckSpec::ValidSolid]);
        let tmp = write_tmp_vcad(
            "mecheval-empty-doc.vcad",
            r#"{"version":"0.1","nodes":{},"materials":{},"part_materials":{},"roots":[]}"#,
        );
        let blob = grade(&task, &task_bytes, &tmp, std::path::Path::new(".")).expect("grade");
        assert_eq!(blob.checks[0].result, CheckOutcome::Fail);
        assert!(!blob.summary.passed);
    }

    #[test]
    fn bbox_passes_when_within_tolerance() {
        // Cube primitive's corner is at origin per the IR convention (a
        // 10mm cube spans 0..10 on each axis).
        let (task, task_bytes) = task_with(vec![CheckSpec::Bbox {
            min: [0.0, 0.0, 0.0],
            max: [10.0, 10.0, 10.0],
            tolerance_mm: 0.05,
        }]);
        let tmp = write_tmp_vcad("mecheval-bbox-pass.vcad", &cube_vcad(10.0));
        let blob = grade(&task, &task_bytes, &tmp, std::path::Path::new(".")).expect("grade");
        assert_eq!(
            blob.checks[0].result,
            CheckOutcome::Pass,
            "{:?}",
            blob.checks[0].details
        );
    }

    #[test]
    fn bbox_fails_when_off() {
        // Same 10mm cube, but spec says 50mm — should fail with deviations.
        let (task, task_bytes) = task_with(vec![CheckSpec::Bbox {
            min: [0.0, 0.0, 0.0],
            max: [50.0, 50.0, 50.0],
            tolerance_mm: 0.1,
        }]);
        let tmp = write_tmp_vcad("mecheval-bbox-fail.vcad", &cube_vcad(10.0));
        let blob = grade(&task, &task_bytes, &tmp, std::path::Path::new(".")).expect("grade");
        assert_eq!(blob.checks[0].result, CheckOutcome::Fail);
        let dev = blob.checks[0].details["max_abs_deviation_mm"]
            .as_f64()
            .unwrap();
        assert!(dev > 30.0, "expected large deviation, got {}", dev);
    }

    #[test]
    fn mass_props_passes_for_correct_volume() {
        // 10mm cube → volume = 1000.
        let (task, task_bytes) = task_with(vec![CheckSpec::MassProps {
            volume_mm3: Some(1000.0),
            surface_area_mm2: None,
            center_of_mass: None,
            tolerance_pct: 1.0,
        }]);
        let tmp = write_tmp_vcad("mecheval-mp-vol-pass.vcad", &cube_vcad(10.0));
        let blob = grade(&task, &task_bytes, &tmp, std::path::Path::new(".")).expect("grade");
        assert_eq!(
            blob.checks[0].result,
            CheckOutcome::Pass,
            "{:?}",
            blob.checks[0].details
        );
    }

    #[test]
    fn mass_props_fails_for_wrong_volume() {
        let (task, task_bytes) = task_with(vec![CheckSpec::MassProps {
            volume_mm3: Some(125_000.0), // expects a 50mm cube
            surface_area_mm2: None,
            center_of_mass: None,
            tolerance_pct: 0.5,
        }]);
        let tmp = write_tmp_vcad("mecheval-mp-vol-fail.vcad", &cube_vcad(10.0));
        let blob = grade(&task, &task_bytes, &tmp, std::path::Path::new(".")).expect("grade");
        assert_eq!(blob.checks[0].result, CheckOutcome::Fail);
    }

    #[test]
    fn mass_props_passes_for_com_at_cube_center() {
        // 10mm cube primitive corner-at-origin → COM at [5,5,5].
        let (task, task_bytes) = task_with(vec![CheckSpec::MassProps {
            volume_mm3: None,
            surface_area_mm2: None,
            center_of_mass: Some([5.0, 5.0, 5.0]),
            tolerance_pct: 0.5,
        }]);
        let tmp = write_tmp_vcad("mecheval-mp-com-pass.vcad", &cube_vcad(10.0));
        let blob = grade(&task, &task_bytes, &tmp, std::path::Path::new(".")).expect("grade");
        assert_eq!(
            blob.checks[0].result,
            CheckOutcome::Pass,
            "{:?}",
            blob.checks[0].details
        );
    }

    #[test]
    fn mass_props_combined_subfields() {
        // All three: volume + surface area (6 * 100 = 600) + COM.
        let (task, task_bytes) = task_with(vec![CheckSpec::MassProps {
            volume_mm3: Some(1000.0),
            surface_area_mm2: Some(600.0),
            center_of_mass: Some([5.0, 5.0, 5.0]),
            tolerance_pct: 1.0,
        }]);
        let tmp = write_tmp_vcad("mecheval-mp-combo.vcad", &cube_vcad(10.0));
        let blob = grade(&task, &task_bytes, &tmp, std::path::Path::new(".")).expect("grade");
        assert_eq!(
            blob.checks[0].result,
            CheckOutcome::Pass,
            "{:?}",
            blob.checks[0].details
        );
    }

    #[test]
    fn step_roundtrip_passes_for_a_cube() {
        let (task, task_bytes) = task_with(vec![CheckSpec::StepRoundtrip { tolerance_pct: 1.0 }]);
        let tmp = write_tmp_vcad("mecheval-step-cube.vcad", &cube_vcad(20.0));
        let blob = grade(&task, &task_bytes, &tmp, std::path::Path::new(".")).expect("grade");
        assert_eq!(
            blob.checks[0].result,
            CheckOutcome::Pass,
            "{:?}",
            blob.checks[0].details
        );
    }

    #[test]
    fn task_sha256_has_64_hex_chars() {
        let (task, task_bytes) = task_with(vec![CheckSpec::ValidSolid]);
        let tmp = write_tmp_vcad("mecheval-hash-len.vcad", &cube_vcad(5.0));
        let blob = grade(&task, &task_bytes, &tmp, std::path::Path::new(".")).expect("grade");
        assert_eq!(blob.task_sha256.len(), 64);
    }

    /// End-to-end Suite-C smoke: a small reacher .vcad goes through
    /// the full dispatch (body_valid + fk_reaches + torque_budget +
    /// task_success). Targets are picked at the rest pose so IK has a
    /// trivial solution; this test exists to verify the dispatch
    /// plumbing is intact, not the IK / controller solvers (those are
    /// covered in `suite_c::tests`).
    fn reacher_vcad_str() -> String {
        // Mirror suite_c::tests::reacher_doc — a base + two-link Y-axis
        // revolute reacher, with the second link tagged "tip".
        r#"{
            "version":"0.1",
            "nodes":{
                "1":{"id":1,"name":"base_g","op":{"type":"Cube","size":{"x":100,"y":100,"z":50}}},
                "2":{"id":2,"name":"l1_g","op":{"type":"Cube","size":{"x":20,"y":20,"z":100}}},
                "3":{"id":3,"name":"l2_g","op":{"type":"Cube","size":{"x":20,"y":20,"z":100}}}
            },
            "materials":{},
            "part_materials":{},
            "roots":[],
            "partDefs":{
                "base":{"id":"base","root":1},
                "l1":{"id":"l1","root":2},
                "l2":{"id":"l2","root":3}
            },
            "instances":[
                {"id":"base_inst","partDefId":"base","name":"Base","tags":["base"]},
                {"id":"l1_inst","partDefId":"l1","name":"L1"},
                {"id":"l2_inst","partDefId":"l2","name":"Tip","tags":["tip"]}
            ],
            "joints":[
                {"id":"j1","parentInstanceId":"base_inst","childInstanceId":"l1_inst",
                 "parentAnchor":{"x":0,"y":0,"z":25},"childAnchor":{"x":0,"y":0,"z":-50},
                 "kind":{"type":"Revolute","axis":{"x":0,"y":1,"z":0},"limits":[-180,180]},
                 "state":0},
                {"id":"j2","parentInstanceId":"l1_inst","childInstanceId":"l2_inst",
                 "parentAnchor":{"x":0,"y":0,"z":50},"childAnchor":{"x":0,"y":0,"z":-50},
                 "kind":{"type":"Revolute","axis":{"x":0,"y":1,"z":0},"limits":[-180,180]},
                 "state":0}
            ],
            "groundInstanceId":"base_inst"
        }"#
        .to_string()
    }

    #[test]
    fn end_to_end_suite_c_runs_real_pass_fail() {
        // Discover the tip rest pose and use it as the target — that way
        // IK trivially finds q=0 and we exercise the full dispatch chain.
        use crate::eval::evaluate_vcad;
        use crate::suite_c::{build_assembly_snapshot, resolve_tip_instance};
        let raw = reacher_vcad_str();
        let snap = evaluate_vcad(&raw);
        let doc = snap.doc.as_ref().expect("doc parsed");
        let mut asm = build_assembly_snapshot(doc).expect("assembly built");
        let tip = resolve_tip_instance(doc).unwrap().to_string();
        let rest = asm
            .world
            .forward_kinematics_at(&[0.0, 0.0])
            .unwrap()
            .get(&tip)
            .unwrap()
            .0;

        let task = Task {
            id: "c-smoke".into(),
            suite: crate::task::Suite::C,
            tier: "C-smoke".into(),
            title: "smoke".into(),
            prompt: "p".into(),
            inputs: vec![],
            checks: vec![
                CheckSpec::BodyValid,
                CheckSpec::FkReaches {
                    target: rest,
                    tolerance_m: 0.005,
                },
                CheckSpec::TorqueBudget {
                    payload_kg: 0.0,
                    safety_factor: 1.5,
                },
                CheckSpec::TaskSuccess {
                    task: "reach_target".into(),
                    params: serde_json::json!({
                        "target": rest,
                        "tolerance_m": 0.05,
                        "max_steps": 200,
                        "controller": "stock_pd"
                    }),
                },
            ],
            anti_cheese: crate::task::AntiCheese {
                min_rigid_bodies: Some(3),
                min_actuated_joints: Some(2),
                joint_torque_ceiling_nm: Some(50.0),
                required_links: vec!["base".into(), "tip".into()],
                ..Default::default()
            },
            limits: Default::default(),
            pass_k: 1,
            tags: vec![],
        };
        let task_bytes = serde_json::to_vec(&task).unwrap();
        let tmp = write_tmp_vcad("mecheval-suite-c-e2e.vcad", &raw);
        let blob = grade(&task, &task_bytes, &tmp, std::path::Path::new(".")).expect("grade");

        // Each check has a real outcome — never NotImplemented now.
        for r in &blob.checks {
            assert_ne!(
                r.result,
                CheckOutcome::NotImplemented,
                "check {} returned NotImplemented: {:?}",
                r.r#type,
                r.details
            );
        }
        assert!(!blob.summary.anti_cheese_violated);
        assert_eq!(blob.checks.len(), 4);
    }
}
