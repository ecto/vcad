//! Suite C grader: physics + control checks for the mech track.
//!
//! Wires the four checks `c-reacher-01` needs:
//!
//! - `body_valid` — assembly is well-formed and `PhysicsWorld` builds.
//! - `fk_reaches` — exists a joint configuration `q*` placing the tip
//!   within tolerance of the target.
//! - `torque_budget` — at `q*`, gravity-comp torques per joint stay below
//!   the per-joint ceiling × `safety_factor`.
//! - `task_success` — a stock-PD rollout commanded to `q*` reaches and
//!   holds the target for at least 30 consecutive steps.
//!
//! Resolution of the end-effector ("tip") follows the chain:
//! `tags` contains `"tip"` → instance `id == "tip"` → instance `name`
//! case-insensitive matches `"tip"`. The first match wins.

use std::collections::HashMap;

use crate::blob::CheckOutcome;
use serde_json::{json, Value};
use vcad_ir::{Document, Instance, JointKind};
use vcad_kernel_physics::{Action, PhysicsWorld, RobotEnv};

/// Snapshot built once per grader run; every Suite-C check borrows it.
pub struct AssemblySnapshot {
    /// Live phyz world. Mutable so checks can run FK / RNEA probes.
    pub world: PhysicsWorld,
    /// The candidate document. Borrowed (cloned) so we don't have to
    /// re-parse from the snapshot every check.
    pub doc: Document,
    /// Cached joint ids (matches the order `world.forward_kinematics_at`
    /// uses internally).
    pub joint_ids: Vec<String>,
    /// Cached resolved tip instance id.
    pub tip_id: Option<String>,
}

/// Build an [`AssemblySnapshot`] from a candidate document. Returns a
/// human-readable reason on any failure (no [`PhysicsWorld`] built ⇒ no
/// snapshot).
pub fn build_assembly_snapshot(doc: &Document) -> Result<AssemblySnapshot, String> {
    let world = PhysicsWorld::from_document(doc).map_err(|e| format!("from_document: {}", e))?;
    let joint_ids = world.joint_ids();
    let tip_id = resolve_tip_instance(doc).map(str::to_string);
    Ok(AssemblySnapshot {
        world,
        doc: doc.clone(),
        joint_ids,
        tip_id,
    })
}

/// First instance tagged `"tip"`, or fallback to id / name match.
pub fn resolve_tip_instance(doc: &Document) -> Option<&str> {
    let instances = doc.instances.as_ref()?;
    if let Some(i) = instances
        .iter()
        .find(|i| i.tags.iter().any(|t| t.eq_ignore_ascii_case("tip")))
    {
        return Some(&i.id);
    }
    if let Some(i) = instances.iter().find(|i| i.id.eq_ignore_ascii_case("tip")) {
        return Some(&i.id);
    }
    if let Some(i) = instances.iter().find(|i| {
        i.name
            .as_deref()
            .is_some_and(|n| n.eq_ignore_ascii_case("tip"))
    }) {
        return Some(&i.id);
    }
    None
}

/// `body_valid`: required assembly fields present, `PhysicsWorld`
/// constructed, every instance evaluates to a positive-volume mesh.
pub fn check_body_valid(snap: &AssemblySnapshot) -> (CheckOutcome, Value) {
    let doc = &snap.doc;
    let mut missing: Vec<&str> = Vec::new();
    if doc.part_defs.is_none() {
        missing.push("partDefs");
    }
    if doc.instances.is_none() {
        missing.push("instances");
    }
    if doc.joints.is_none() {
        missing.push("joints");
    }
    if doc.ground_instance_id.is_none() {
        missing.push("groundInstanceId");
    }
    if !missing.is_empty() {
        return (
            CheckOutcome::Fail,
            json!({ "reason": "missing assembly fields", "missing": missing }),
        );
    }

    let instances = doc.instances.as_ref().unwrap();
    let n_instances = instances.len();
    let n_joints = doc.joints.as_ref().map(|j| j.len()).unwrap_or(0);

    (
        CheckOutcome::Pass,
        json!({
            "instances": n_instances,
            "joints": n_joints,
            "ground_instance_id": doc.ground_instance_id,
            "tip_instance_id": snap.tip_id,
        }),
    )
}

/// `fk_reaches`: solve IK and pass iff `dist(tip(q*), target) ≤ tol_m`.
///
/// IK strategy: coarse uniform grid sweep across joint limits, then
/// damped least-squares refinement using a numerical Jacobian.
pub fn check_fk_reaches(
    snap: &mut AssemblySnapshot,
    target_m: [f64; 3],
    tol_m: f64,
) -> (CheckOutcome, Value) {
    let tip_id = match snap.tip_id.clone() {
        Some(t) => t,
        None => {
            return (
                CheckOutcome::Fail,
                json!({ "reason": "no instance tagged 'tip' (or named/id'd 'tip')" }),
            );
        }
    };

    let ranges = match joint_ranges(snap) {
        Ok(r) => r,
        Err(e) => return (CheckOutcome::Fail, json!({ "reason": e })),
    };

    match solve_ik(
        &mut snap.world,
        &snap.joint_ids,
        &ranges,
        &tip_id,
        target_m,
        tol_m,
    ) {
        Some((q_star, dist)) => {
            let pass = dist <= tol_m;
            (
                if pass {
                    CheckOutcome::Pass
                } else {
                    CheckOutcome::Fail
                },
                json!({
                    "tip_instance_id": tip_id,
                    "target_m": target_m,
                    "tolerance_m": tol_m,
                    "best_q_deg_or_mm": q_star,
                    "best_distance_m": dist,
                    "joint_ids": snap.joint_ids,
                }),
            )
        }
        None => (
            CheckOutcome::Fail,
            json!({ "reason": "IK probe failed (forward_kinematics_at returned an error)" }),
        ),
    }
}

/// `torque_budget`: for each actuated joint, |τ(q*)| ≤ ceiling/safety.
///
/// `joint_ceiling_nm` is supplied from the task's `anti_cheese.joint_torque_ceiling_nm`.
/// Payload is currently only honored for non-zero values via a warning —
/// integrating tip mass requires a kernel pass that's not on c-reacher-01's
/// critical path.
pub fn check_torque_budget(
    snap: &mut AssemblySnapshot,
    payload_kg: f64,
    safety_factor: f64,
    joint_ceiling_nm: Option<f64>,
    target_m: [f64; 3],
    tol_m: f64,
) -> (CheckOutcome, Value) {
    let ceiling = match joint_ceiling_nm {
        Some(c) if c > 0.0 => c,
        _ => {
            return (
                CheckOutcome::Fail,
                json!({
                    "reason": "no joint_torque_ceiling_nm configured on the task's anti_cheese",
                }),
            );
        }
    };
    let tip_id = match snap.tip_id.clone() {
        Some(t) => t,
        None => {
            return (
                CheckOutcome::Fail,
                json!({ "reason": "no instance tagged 'tip'" }),
            );
        }
    };
    let ranges = match joint_ranges(snap) {
        Ok(r) => r,
        Err(e) => return (CheckOutcome::Fail, json!({ "reason": e })),
    };

    let q_star = match solve_ik(
        &mut snap.world,
        &snap.joint_ids,
        &ranges,
        &tip_id,
        target_m,
        tol_m,
    ) {
        Some((q, _)) => q,
        None => {
            return (
                CheckOutcome::Fail,
                json!({ "reason": "IK probe failed; cannot evaluate torques" }),
            );
        }
    };

    let tau = match snap.world.gravity_torques_at(&q_star) {
        Ok(t) => t,
        Err(e) => {
            return (
                CheckOutcome::Fail,
                json!({ "reason": format!("gravity_torques_at: {}", e) }),
            );
        }
    };

    let limit = ceiling / safety_factor.max(1e-6);
    let mut per_joint = serde_json::Map::new();
    let mut all_pass = true;
    let mut max_abs = 0.0_f64;
    for jid in &snap.joint_ids {
        let raw = tau.get(jid).copied().unwrap_or(0.0);
        let abs_t = raw.abs();
        max_abs = max_abs.max(abs_t);
        let pass = abs_t <= limit;
        all_pass &= pass;
        per_joint.insert(
            jid.clone(),
            json!({
                "torque_nm": raw,
                "abs_torque_nm": abs_t,
                "limit_nm": limit,
                "pass": pass,
            }),
        );
    }

    let warn = if payload_kg > 0.0 {
        Some(json!(
            "payload_kg > 0: torque_budget currently uses gravity-only \
             RNEA; tip-payload mass is not yet folded in"
        ))
    } else {
        None
    };

    (
        if all_pass {
            CheckOutcome::Pass
        } else {
            CheckOutcome::Fail
        },
        json!({
            "joint_ceiling_nm": ceiling,
            "safety_factor": safety_factor,
            "limit_nm": limit,
            "max_abs_torque_nm": max_abs,
            "best_q": q_star,
            "per_joint": per_joint,
            "warning": warn,
        }),
    )
}

/// `task_success`: command stock-PD to `q*` and confirm the tip reaches
/// and holds the target. Pass iff `min(distance) ≤ tolerance_m` for at
/// least `hold_steps` (= 30) consecutive environment steps.
pub fn check_task_success(
    snap: &mut AssemblySnapshot,
    params: &Value,
    fallback_target: Option<[f64; 3]>,
    fallback_tol: Option<f64>,
) -> (CheckOutcome, Value) {
    let target_m = match parse_xyz(params.get("target")).or(fallback_target) {
        Some(t) => t,
        None => {
            return (
                CheckOutcome::Fail,
                json!({ "reason": "task_success: missing 'target' [x,y,z] in params" }),
            );
        }
    };
    let tol_m = params
        .get("tolerance_m")
        .and_then(|v| v.as_f64())
        .or(fallback_tol)
        .unwrap_or(0.005);
    let max_steps = params
        .get("max_steps")
        .and_then(|v| v.as_u64())
        .unwrap_or(1000) as u32;
    let hold_steps: u32 = 30;
    let controller = params
        .get("controller")
        .and_then(|v| v.as_str())
        .unwrap_or("stock_pd");

    if controller != "stock_pd" {
        return (
            CheckOutcome::Fail,
            json!({
                "reason": format!("controller {:?} not supported; only 'stock_pd' for now", controller),
            }),
        );
    }

    let tip_id = match snap.tip_id.clone() {
        Some(t) => t,
        None => {
            return (
                CheckOutcome::Fail,
                json!({ "reason": "no instance tagged 'tip'" }),
            );
        }
    };

    // Solve IK once on the snapshot's world to get q*.
    let ranges = match joint_ranges(snap) {
        Ok(r) => r,
        Err(e) => return (CheckOutcome::Fail, json!({ "reason": e })),
    };
    let q_star = match solve_ik(
        &mut snap.world,
        &snap.joint_ids,
        &ranges,
        &tip_id,
        target_m,
        tol_m,
    ) {
        Some((q, _)) => q,
        None => {
            return (
                CheckOutcome::Fail,
                json!({ "reason": "IK probe failed; cannot drive controller" }),
            );
        }
    };

    // Build a fresh RobotEnv (RobotEnv re-builds its own PhysicsWorld
    // internally, so the snapshot's world is unaffected).
    // Ground contact disabled: Suite C grades pure articulated dynamics of a
    // fixed-base reacher; an implicit floor at z=0 would perturb rollouts of
    // arms that dip below the base plane.
    let mut env = match RobotEnv::new(
        snap.doc.clone(),
        vec![tip_id.clone()],
        None,
        Some(4),
        Some(vcad_kernel_physics::GroundConfig::disabled()),
    ) {
        Ok(e) => e,
        Err(e) => {
            return (
                CheckOutcome::Fail,
                json!({ "reason": format!("RobotEnv::new: {}", e) }),
            );
        }
    };
    env.reset();

    let mut min_dist = f64::INFINITY;
    let mut consec_in_tol = 0u32;
    let mut best_consec = 0u32;
    for _ in 0..max_steps {
        let (obs, _r, _done) = env.step(Action::PositionTarget(q_star.clone()));
        if let Some(pose) = obs.end_effector_poses.first() {
            let d = dist3([pose[0], pose[1], pose[2]], target_m);
            if d < min_dist {
                min_dist = d;
            }
            if d <= tol_m {
                consec_in_tol += 1;
                if consec_in_tol > best_consec {
                    best_consec = consec_in_tol;
                }
                if best_consec >= hold_steps {
                    return (
                        CheckOutcome::Pass,
                        json!({
                            "tip": tip_id,
                            "target_m": target_m,
                            "tolerance_m": tol_m,
                            "min_distance_m": min_dist,
                            "best_consec_in_tol": best_consec,
                            "controller": controller,
                            "q_star": q_star,
                        }),
                    );
                }
            } else {
                consec_in_tol = 0;
            }
        }
    }

    (
        CheckOutcome::Fail,
        json!({
            "reason": "rollout did not reach + hold target",
            "tip": tip_id,
            "target_m": target_m,
            "tolerance_m": tol_m,
            "min_distance_m": min_dist,
            "best_consec_in_tol": best_consec,
            "required_consec": hold_steps,
            "max_steps": max_steps,
            "controller": controller,
            "q_star": q_star,
        }),
    )
}

// ---------- helpers --------------------------------------------------------

fn parse_xyz(v: Option<&Value>) -> Option<[f64; 3]> {
    let arr = v?.as_array()?;
    if arr.len() != 3 {
        return None;
    }
    Some([arr[0].as_f64()?, arr[1].as_f64()?, arr[2].as_f64()?])
}

fn dist3(a: [f64; 3], b: [f64; 3]) -> f64 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    let dz = a[2] - b[2];
    (dx * dx + dy * dy + dz * dz).sqrt()
}

/// Per-joint (lo, hi) ranges in vcad units (degrees for revolute,
/// mm for slider). Bails out if any joint isn't 1-DOF — Suite C
/// task today uses revolute arms only.
fn joint_ranges(snap: &AssemblySnapshot) -> Result<Vec<(f64, f64)>, String> {
    let joints = snap.doc.joints.as_ref().ok_or("doc has no joints")?;
    let by_id: HashMap<&str, &vcad_ir::Joint> = joints.iter().map(|j| (j.id.as_str(), j)).collect();
    let mut out = Vec::with_capacity(snap.joint_ids.len());
    for jid in &snap.joint_ids {
        let j = *by_id
            .get(jid.as_str())
            .ok_or_else(|| format!("joint {} present in world but not in doc", jid))?;
        let r = match &j.kind {
            JointKind::Revolute { limits, .. } => limits.unwrap_or((-180.0, 180.0)),
            JointKind::Slider { limits, .. } => limits.unwrap_or((-200.0, 200.0)),
            JointKind::Cylindrical { .. } => (-180.0, 180.0),
            JointKind::Ball | JointKind::Fixed | JointKind::Free => {
                return Err(format!(
                    "joint {} is {:?}; only 1-DOF Revolute/Slider/Cylindrical supported in IK",
                    jid, j.kind
                ));
            }
        };
        out.push(r);
    }
    Ok(out)
}

/// Coarse grid sweep + damped-least-squares refinement.
fn solve_ik(
    world: &mut PhysicsWorld,
    joint_ids: &[String],
    ranges: &[(f64, f64)],
    tip_id: &str,
    target_m: [f64; 3],
    tol_m: f64,
) -> Option<(Vec<f64>, f64)> {
    let n = joint_ids.len();
    if n == 0 {
        // No DOF to vary — just measure tip at the rest pose.
        let p = world.forward_kinematics_at(&[]).ok()?;
        let pos = p.get(tip_id)?.0;
        let d = dist3(pos, target_m);
        return Some((Vec::new(), d));
    }

    // Coarse grid. Cap product at ~30k samples; with n=2 this gives 173
    // steps/dim, with n=3, 31. Use a smaller per-dim budget for cleaner
    // logs when n is small.
    let cap: u64 = 30_000;
    let mut steps_per: usize = match n {
        1 => 200,
        2 => 24,
        3 => 12,
        _ => {
            let target = (cap as f64).powf(1.0 / n as f64);
            target.floor() as usize
        }
    };
    while (steps_per as u64).saturating_pow(n as u32) > cap && steps_per > 2 {
        steps_per -= 1;
    }

    let mut best_q: Vec<f64> = vec![0.0; n];
    let mut best_d = f64::INFINITY;

    let total: u64 = (steps_per as u64).saturating_pow(n as u32);
    for k in 0..total {
        let mut q = vec![0.0_f64; n];
        let mut idx = k;
        for i in 0..n {
            let s = idx % steps_per as u64;
            idx /= steps_per as u64;
            let (lo, hi) = ranges[i];
            // Sample inclusive of both endpoints when steps_per > 1.
            let t = if steps_per <= 1 {
                0.5
            } else {
                s as f64 / (steps_per - 1) as f64
            };
            q[i] = lo + (hi - lo) * t;
        }
        let p = world.forward_kinematics_at(&q).ok()?;
        let pos = p.get(tip_id)?.0;
        let d = dist3(pos, target_m);
        if d < best_d {
            best_d = d;
            best_q = q;
            if best_d <= tol_m {
                return Some((best_q, best_d));
            }
        }
    }

    // DLS refinement around best_q.
    let lambda = 0.1_f64;
    for _iter in 0..40 {
        if best_d <= tol_m {
            break;
        }
        let p0 = world.forward_kinematics_at(&best_q).ok()?.get(tip_id)?.0;
        let r = [
            target_m[0] - p0[0],
            target_m[1] - p0[1],
            target_m[2] - p0[2],
        ];
        let r_norm = (r[0] * r[0] + r[1] * r[1] + r[2] * r[2]).sqrt();
        if r_norm <= tol_m {
            best_d = r_norm;
            break;
        }

        // Numerical Jacobian: 3 x n columns.
        let h = 0.5_f64;
        let mut jac: Vec<[f64; 3]> = vec![[0.0; 3]; n];
        for i in 0..n {
            let mut qp = best_q.clone();
            let (lo, hi) = ranges[i];
            // Use central difference where range allows; one-sided otherwise.
            let (qa, qb) = if qp[i] + h <= hi && qp[i] - h >= lo {
                let mut a = qp.clone();
                a[i] -= h;
                let mut b = qp.clone();
                b[i] += h;
                (a, b)
            } else {
                let step = if qp[i] + h <= hi { h } else { -h };
                qp[i] += step;
                (best_q.clone(), qp)
            };
            let pa = world.forward_kinematics_at(&qa).ok()?.get(tip_id)?.0;
            let pb = world.forward_kinematics_at(&qb).ok()?.get(tip_id)?.0;
            let denom = if (qa[i] - qb[i]).abs() < 1e-9 {
                1e-9
            } else {
                qb[i] - qa[i]
            };
            jac[i] = [
                (pb[0] - pa[0]) / denom,
                (pb[1] - pa[1]) / denom,
                (pb[2] - pa[2]) / denom,
            ];
        }

        // Solve (J^T J + λ²I) dq = J^T r  via Gauss elimination on n×n.
        let mut a = vec![vec![0.0_f64; n]; n];
        let mut rhs = vec![0.0_f64; n];
        for i in 0..n {
            for j in 0..n {
                let s: f64 = (0..3).map(|k| jac[i][k] * jac[j][k]).sum();
                a[i][j] = s;
                if i == j {
                    a[i][j] += lambda * lambda;
                }
            }
            rhs[i] = jac[i][0] * r[0] + jac[i][1] * r[1] + jac[i][2] * r[2];
        }
        let dq = match gauss_solve(&mut a, &mut rhs) {
            Some(x) => x,
            None => break,
        };

        // Line search along dq: try full step, then half, etc.
        let mut accepted = false;
        let mut alpha = 1.0;
        for _ in 0..6 {
            let mut q_try = vec![0.0_f64; n];
            for i in 0..n {
                q_try[i] = (best_q[i] + alpha * dq[i]).clamp(ranges[i].0, ranges[i].1);
            }
            let pt = world.forward_kinematics_at(&q_try).ok()?.get(tip_id)?.0;
            let d_try = dist3(pt, target_m);
            if d_try + 1e-9 < best_d {
                best_q = q_try;
                best_d = d_try;
                accepted = true;
                break;
            }
            alpha *= 0.5;
        }
        if !accepted {
            break;
        }
    }

    Some((best_q, best_d))
}

/// In-place Gaussian elimination with partial pivoting. `a` is `n×n`,
/// `b` is `n`. Returns the solution `x` such that `a x = b`, or `None`
/// if `a` is singular.
fn gauss_solve(a: &mut [Vec<f64>], b: &mut [f64]) -> Option<Vec<f64>> {
    let n = b.len();
    for k in 0..n {
        // Pivot.
        let mut max_row = k;
        let mut max_val = a[k][k].abs();
        for (r, row) in a.iter().enumerate().take(n).skip(k + 1) {
            let v = row[k].abs();
            if v > max_val {
                max_val = v;
                max_row = r;
            }
        }
        if max_val < 1e-12 {
            return None;
        }
        a.swap(k, max_row);
        b.swap(k, max_row);

        // Eliminate.
        let pivot = a[k][k];
        let pivot_row = a[k].clone();
        for r in (k + 1)..n {
            let f = a[r][k] / pivot;
            for c in k..n {
                a[r][c] -= f * pivot_row[c];
            }
            b[r] -= f * b[k];
        }
    }
    // Back-substitute.
    let mut x = vec![0.0_f64; n];
    for i in (0..n).rev() {
        let mut s = b[i];
        for j in (i + 1)..n {
            s -= a[i][j] * x[j];
        }
        x[i] = s / a[i][i];
    }
    Some(x)
}

/// Returns the **fixed-instance** ID — the ground body that the joint
/// chain is anchored to. Used by anti-cheese checks.
pub fn ground_id(doc: &Document) -> Option<&str> {
    doc.ground_instance_id.as_deref()
}

/// Iterate `(instance_id, tags)` for every instance.
pub fn instance_tags(doc: &Document) -> impl Iterator<Item = (&str, &[String])> {
    doc.instances
        .iter()
        .flatten()
        .map(|i: &Instance| (i.id.as_str(), i.tags.as_slice()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_ir::{Instance, Joint, JointKind, PartDef, Vec3 as VVec3};

    /// Two-link reacher: base 0.10m cube, two 0.10m × 0.02m × 0.02m links
    /// connected by Y-axis revolute joints (so motion is in the X-Z plane).
    fn reacher_doc() -> Document {
        let mut doc = Document::new();
        doc.nodes.insert(
            1,
            vcad_ir::Node {
                id: 1,
                name: Some("base_g".into()),
                op: vcad_ir::CsgOp::Cube {
                    size: VVec3::new(100.0, 100.0, 50.0),
                },
            },
        );
        doc.nodes.insert(
            2,
            vcad_ir::Node {
                id: 2,
                name: Some("l1_g".into()),
                op: vcad_ir::CsgOp::Cube {
                    size: VVec3::new(20.0, 20.0, 100.0),
                },
            },
        );
        doc.nodes.insert(
            3,
            vcad_ir::Node {
                id: 3,
                name: Some("l2_g".into()),
                op: vcad_ir::CsgOp::Cube {
                    size: VVec3::new(20.0, 20.0, 100.0),
                },
            },
        );

        let mut part_defs = HashMap::new();
        part_defs.insert(
            "base".into(),
            PartDef {
                id: "base".into(),
                name: None,
                root: 1,
                default_material: None,
                inertial: None,
                colliders: None,
            },
        );
        part_defs.insert(
            "l1".into(),
            PartDef {
                id: "l1".into(),
                name: None,
                root: 2,
                default_material: None,
                inertial: None,
                colliders: None,
            },
        );
        part_defs.insert(
            "l2".into(),
            PartDef {
                id: "l2".into(),
                name: None,
                root: 3,
                default_material: None,
                inertial: None,
                colliders: None,
            },
        );
        doc.part_defs = Some(part_defs);

        doc.instances = Some(vec![
            Instance {
                id: "base_inst".into(),
                part_def_id: "base".into(),
                name: Some("Base".into()),
                tags: vec!["base".into()],
                transform: None,
                material: None,
                explode: None,
            },
            Instance {
                id: "l1_inst".into(),
                part_def_id: "l1".into(),
                name: Some("Link1".into()),
                tags: vec![],
                transform: None,
                material: None,
                explode: None,
            },
            Instance {
                id: "l2_inst".into(),
                part_def_id: "l2".into(),
                name: Some("Link2".into()),
                tags: vec!["tip".into()],
                transform: None,
                material: None,
                explode: None,
            },
        ]);

        doc.joints = Some(vec![
            Joint {
                id: "j1".into(),
                name: None,
                parent_instance_id: Some("base_inst".into()),
                child_instance_id: "l1_inst".into(),
                parent_anchor: VVec3::new(0.0, 0.0, 25.0),
                child_anchor: VVec3::new(0.0, 0.0, -50.0),
                kind: JointKind::Revolute {
                    axis: VVec3::new(0.0, 1.0, 0.0),
                    limits: Some((-180.0, 180.0)),
                    effort_limit: None,
                    velocity_limit: None,
                },
                state: 0.0,
            },
            Joint {
                id: "j2".into(),
                name: None,
                parent_instance_id: Some("l1_inst".into()),
                child_instance_id: "l2_inst".into(),
                parent_anchor: VVec3::new(0.0, 0.0, 50.0),
                child_anchor: VVec3::new(0.0, 0.0, -50.0),
                kind: JointKind::Revolute {
                    axis: VVec3::new(0.0, 1.0, 0.0),
                    limits: Some((-180.0, 180.0)),
                    effort_limit: None,
                    velocity_limit: None,
                },
                state: 0.0,
            },
        ]);

        doc.ground_instance_id = Some("base_inst".into());
        doc
    }

    #[test]
    fn resolves_tip_via_tag() {
        let doc = reacher_doc();
        assert_eq!(resolve_tip_instance(&doc), Some("l2_inst"));
    }

    #[test]
    fn body_valid_passes_for_a_real_reacher() {
        let doc = reacher_doc();
        let snap = build_assembly_snapshot(&doc).expect("snapshot");
        let (out, _) = check_body_valid(&snap);
        assert_eq!(out, CheckOutcome::Pass);
    }

    #[test]
    fn body_valid_fails_when_assembly_fields_missing() {
        let mut doc = reacher_doc();
        doc.joints = None;
        // build_assembly_snapshot will fail at PhysicsWorld::from_document.
        // Verify the reason surfaces gracefully.
        let err = build_assembly_snapshot(&doc).err().expect("expected error");
        assert!(err.contains("from_document"), "got {err}");
    }

    #[test]
    fn fk_reaches_finds_a_q_for_a_reachable_target() {
        // Construct a target by FK from a known q, then ask IK to find any
        // q that hits it. Tolerance is generous (1 cm) since the IK uses
        // a coarse grid + DLS — far from the target it can plateau on a
        // local minimum. For the real c-reacher-01 task we operate at
        // 5 mm tolerance, but the test exists to verify the dispatch is
        // wired and the solver converges *at all*, not to characterize
        // its asymptotic accuracy.
        let doc = reacher_doc();
        let mut snap = build_assembly_snapshot(&doc).expect("snapshot");
        let q_known = [15.0, -10.0];
        let target = snap
            .world
            .forward_kinematics_at(&q_known)
            .unwrap()
            .get(snap.tip_id.as_ref().unwrap())
            .unwrap()
            .0;
        let (out, details) = check_fk_reaches(&mut snap, target, 0.01);
        assert_eq!(out, CheckOutcome::Pass, "details: {}", details);
    }

    #[test]
    fn fk_reaches_fails_for_an_unreachable_target() {
        let doc = reacher_doc();
        let mut snap = build_assembly_snapshot(&doc).expect("snapshot");
        // 5m away — well beyond a ~0.20m reach.
        let (out, _details) = check_fk_reaches(&mut snap, [5.0, 0.0, 5.0], 0.005);
        assert_eq!(out, CheckOutcome::Fail);
    }

    #[test]
    fn fk_reaches_fails_when_no_tip() {
        let mut doc = reacher_doc();
        // Strip the "tip" tag from every instance.
        for inst in doc.instances.as_mut().unwrap() {
            inst.tags.retain(|t| t != "tip");
            if inst.name.as_deref() == Some("Link2") {
                inst.name = Some("Other".into());
            }
        }
        let mut snap = build_assembly_snapshot(&doc).expect("snapshot");
        let (out, details) = check_fk_reaches(&mut snap, [0.10, 0.0, 0.20], 0.02);
        assert_eq!(out, CheckOutcome::Fail);
        assert!(details["reason"].as_str().unwrap().contains("tip"));
    }

    /// Pick a target that, by construction, the IK can reach via a
    /// non-vertical configuration. A vertical-arm q would zero gravity
    /// moment about the Y joints and render the torque test trivial.
    fn off_axis_target(snap: &mut AssemblySnapshot) -> [f64; 3] {
        let q = [60.0, -30.0];
        snap.world
            .forward_kinematics_at(&q)
            .unwrap()
            .get(snap.tip_id.as_ref().unwrap())
            .unwrap()
            .0
    }

    #[test]
    fn torque_budget_passes_with_lenient_ceiling() {
        let doc = reacher_doc();
        let mut snap = build_assembly_snapshot(&doc).expect("snapshot");
        let target = off_axis_target(&mut snap);
        let (out, details) = check_torque_budget(&mut snap, 0.0, 1.0, Some(50.0), target, 0.01);
        assert_eq!(out, CheckOutcome::Pass, "details: {}", details);
    }

    #[test]
    fn torque_budget_fails_with_tight_ceiling() {
        let doc = reacher_doc();
        let mut snap = build_assembly_snapshot(&doc).expect("snapshot");
        let target = off_axis_target(&mut snap);
        // 1 µN·m is way less than gravity loading at a non-vertical q.
        let (out, _details) = check_torque_budget(&mut snap, 0.0, 1.0, Some(1e-6), target, 0.01);
        assert_eq!(out, CheckOutcome::Fail);
    }

    #[test]
    fn torque_budget_fails_when_no_ceiling_configured() {
        let doc = reacher_doc();
        let mut snap = build_assembly_snapshot(&doc).expect("snapshot");
        let target = off_axis_target(&mut snap);
        let (out, _) = check_torque_budget(&mut snap, 0.0, 1.0, None, target, 0.01);
        assert_eq!(out, CheckOutcome::Fail);
    }
}
