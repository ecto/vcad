//! C ABI for physics simulation and RL — the seam that lets a native app run
//! `vcad-kernel-physics` and `vcad-sim` directly.
//!
//! # Why this module exists
//!
//! The kernel half of this crate hands Swift *geometry*. This half hands it
//! *dynamics*: an owned [`RobotEnv`] behind an opaque handle, stepped from the
//! app's display loop, with body transforms coming back in exactly the layout
//! and units [`crate::vcad_scene_solve_fk`] already produces. That last point
//! is the whole design: a renderer that can draw kinematic playback can draw a
//! physics rollout with no changes at all, because both write column-major mm
//! 4×4s indexed by scene instance.
//!
//! # Unit boundary
//!
//! This is the sharpest edge in the module, because the two sides disagree:
//!
//! | quantity | physics (`PhysicsWorld`) | vcad scene / this ABI |
//! |----------|--------------------------|------------------------|
//! | body position | meters | **millimeters** |
//! | joint angle | degrees | degrees |
//! | joint travel | mm | mm |
//!
//! [`PhysicsWorld::get_instance_pose`] returns meters; every transform this
//! module writes is scaled to mm by [`M_TO_MM`] so it is drop-in with the
//! kinematic path. Observations are passed through untouched — they are the
//! policy's input space and must stay bit-identical to what training saw.
//!
//! # Ownership
//!
//! Same rules as the rest of the crate: the caller owns every returned handle
//! and frees it with the matching `*_free`. `*_view` structs borrow from the
//! owning handle's internal buffers and are invalidated by the next mutating
//! call (`step`/`reset`) on that handle — copy out before stepping again.
//! Failures return null/0 *and* set [`crate::vcad_last_error`].

use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;

use serde::{Deserialize, Serialize};
use vcad_ir::Document;
use vcad_kernel_physics::{Action, EnvConfig, Observation, RobotEnv, StepResult};
use vcad_sim::rl::{
    actuated_slots, feature_dim, features, ActionSpec, LinearPolicy, MlpPolicy, Policy,
};

use crate::err::{clear_error, ctx, set_error};

/// Physics works in meters, the vcad scene in millimeters.
const M_TO_MM: f64 = 1000.0;

// =========================================================================
// Spec
// =========================================================================

/// Everything needed to build a [`RobotEnv`], as JSON.
///
/// Passed as JSON rather than as a `#[repr(C)]` struct on purpose: the env
/// config is wide, nested, and still growing (randomization channels, noise
/// channels, termination predicates). Every added field would otherwise be a
/// breaking ABI change requiring a coordinated Swift rebuild. JSON makes field
/// growth additive, and `serde(default)` makes every field optional, so a
/// minimal `{}` builds a usable default env.
///
/// Mirrors [`vcad_sim::rl::EnvSpec`] field for field, minus the document
/// (which crosses separately, as bytes) — deliberately, so a spec authored for
/// in-app training and one authored for the `k1_stand` trainer are the same
/// object and cannot drift.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct GymSpec {
    /// Instance ids tracked as end effectors (e.g. the two feet). Order fixes
    /// the layout of the contact and pose channels in the observation.
    pub end_effector_ids: Vec<String>,
    /// Physics timestep in seconds.
    ///
    /// Defaults to 1 kHz, matching the trained K1 policies. **This is the
    /// single most consequential field here.** A policy trained at one `dt`
    /// and replayed at another sees a different plant: the stiff leg gains the
    /// K1 uses are near their explicit-integration stability limit at 1 kHz
    /// and diverge outright at 200 Hz. [`GymSpec::validate_against`] rejects
    /// the mismatch rather than letting it present as "the policy is bad".
    pub dt: f32,
    /// Physics substeps per env step. `dt * substeps` is the control period.
    pub substeps: u32,
    /// Episode length in env steps, after which the episode truncates.
    pub max_steps: u32,
    /// Explicit per-joint PD gains, `joint_id -> [kp, kd]`, reapplied on every
    /// reset. Joints absent here keep the kernel's inertia-scaled defaults,
    /// which are stable at any tick but too soft to hold a humanoid up.
    pub gains: HashMap<String, [f64; 2]>,
    /// Randomization / observation noise / termination / base instance.
    pub config: EnvConfig,
    /// Nominal base height in meters, subtracted in the policy feature vector.
    /// Must match the value the policy trained against.
    pub nominal_height_m: f64,
    /// When set, raises the document's `Free` base joint anchor to this height
    /// (mm) before building.
    ///
    /// A URDF import anchors the floating base at the world origin, so the
    /// robot spawns a metre below its own termination floor and every episode
    /// dies on step 1 — a silent degeneration that looks like a broken policy.
    /// Doing it here means the app cannot forget the step.
    pub spawn_z_mm: Option<f64>,
    /// Directory to resolve relative `MeshImport` paths against — the
    /// document's own location.
    ///
    /// A committed robot document references its vendored meshes relatively, so
    /// without this they resolve against the process working directory and come
    /// back empty. Physics still runs (mass properties are authored), but every
    /// collider degrades to a placeholder box, which is a quietly different
    /// robot rather than an error.
    pub base_dir: Option<String>,
    /// Require the built env to expose a floating base (a resolvable
    /// `base_pose`). Default true.
    ///
    /// Without a floating base no termination condition can fire and the
    /// height/tilt reward terms read constants: a fixed-base document reports
    /// a confident full-length episode while measuring nothing. Failing closed
    /// here converts that silent wrong answer into a startup error.
    pub require_floating_base: bool,
}

impl Default for GymSpec {
    fn default() -> Self {
        Self {
            end_effector_ids: Vec::new(),
            dt: 1.0 / 1000.0,
            substeps: 20,
            max_steps: 400,
            gains: HashMap::new(),
            config: EnvConfig::default(),
            nominal_height_m: 0.0,
            spawn_z_mm: None,
            base_dir: None,
            require_floating_base: true,
        }
    }
}

impl GymSpec {
    /// Control period in seconds — the wall-clock interval one `step`
    /// advances, and the rate a render loop should call it at for real time.
    pub fn control_dt(&self) -> f64 {
        self.dt as f64 * self.substeps.max(1) as f64
    }
}

// =========================================================================
// Handle
// =========================================================================

/// Snapshot of the most recent `step`/`reset`, kept alive so the borrowed
/// [`VcadGymStepView`] pointers stay valid until the next mutating call.
#[derive(Default)]
struct Snapshot {
    joint_positions: Vec<f64>,
    joint_velocities: Vec<f64>,
    /// 7 doubles per end effector: x, y, z, qw, qx, qy, qz.
    ee_poses: Vec<f64>,
    /// 5 doubles per end effector: in_contact (0/1), normal force, cop xyz.
    ee_contacts: Vec<f64>,
    base_pose: Option<[f64; 7]>,
    base_velocity: Option<[f64; 6]>,
    reward: f64,
    done: bool,
    terminated: bool,
    truncated: bool,
    step: u32,
    base_height_m: f64,
    base_tilt_deg: f64,
    has_base: bool,
    action_latency_substeps: u32,
    termination_reason: Option<String>,
}

impl Snapshot {
    fn absorb_observation(&mut self, obs: &Observation) {
        self.joint_positions.clear();
        self.joint_positions.extend_from_slice(&obs.joint_positions);
        self.joint_velocities.clear();
        self.joint_velocities
            .extend_from_slice(&obs.joint_velocities);

        self.ee_poses.clear();
        for p in &obs.end_effector_poses {
            self.ee_poses.extend_from_slice(p);
        }
        self.ee_contacts.clear();
        for c in &obs.end_effector_contacts {
            self.ee_contacts.push(if c.in_contact { 1.0 } else { 0.0 });
            self.ee_contacts.push(c.normal_force);
            // `point` is the impulse-weighted centroid of the contact
            // manifold — the center of pressure — in world meters.
            self.ee_contacts.extend_from_slice(&c.point);
        }

        self.base_pose = obs.base_pose;
        self.base_velocity = obs.base_velocity;
        self.has_base = obs.base_pose.is_some();
    }

    /// Absorb a reset: no reward/termination yet, so those read as a fresh
    /// episode rather than carrying the previous episode's values forward.
    fn absorb_reset(&mut self, obs: &Observation) {
        self.absorb_observation(obs);
        self.reward = 0.0;
        self.done = false;
        self.terminated = false;
        self.truncated = false;
        self.step = 0;
        self.base_height_m = obs.base_pose.map_or(0.0, |p| p[2]);
        self.base_tilt_deg = 0.0;
        self.action_latency_substeps = 0;
        self.termination_reason = None;
    }

    fn absorb_step(&mut self, r: &StepResult) {
        self.absorb_observation(&r.observation);
        self.reward = r.reward;
        self.done = r.done;
        self.terminated = r.info.terminated;
        self.truncated = r.info.truncated;
        self.step = r.info.step;
        self.base_height_m = r.info.base_height_m.unwrap_or(0.0);
        self.base_tilt_deg = r.info.base_tilt_deg.unwrap_or(0.0);
        self.action_latency_substeps = r.info.action_latency_substeps;
        self.termination_reason = r.info.termination_reason.clone();
    }
}

/// Opaque handle to a running physics environment.
pub struct VcadGym {
    env: RobotEnv,
    spec: GymSpec,
    /// Actuated-joint observation slot indices, cached — the policy feature
    /// builder needs them on every step and they never change for an env.
    slots: Vec<usize>,
    /// Policy feature vector length for this env.
    obs_dim: usize,
    /// Instance ids in a stable, sorted order (the physics world's own order).
    body_ids: Vec<String>,
    /// Scene-instance-index → `body_ids` index, installed by
    /// [`vcad_gym_bind_scene`]. Lets the render loop pull transforms in scene
    /// order with one call and no per-frame string lookups.
    scene_binding: Option<Vec<Option<usize>>>,
    snapshot: Snapshot,
    /// The most recent `step_full` result, kept so a client-side reward can be
    /// evaluated against the exact `StepResult` the step produced rather than
    /// against a lossy re-derivation of it.
    last_result: Option<StepResult>,
    /// Scratch reused by the transform writers so a 60 Hz render loop does not
    /// allocate.
    scratch_features: Vec<f64>,
    scratch_actions: Vec<f64>,
}

/// A borrowed view of the most recent step/reset.
///
/// Every pointer borrows from the owning [`VcadGym`] and is invalidated by the
/// next `step`/`reset` on that handle. Lengths are element counts.
///
/// `base_pose` / `base_velocity` are null when the document has no resolvable
/// floating base; check `has_base` rather than the pointer if you prefer.
#[repr(C)]
pub struct VcadGymStepView {
    /// Flattened joint positions, degrees for rotational DOFs and mm for
    /// translational, in the env's joint order.
    pub joint_positions: *const f64,
    pub joint_positions_len: usize,
    /// Flattened joint velocities, same layout and per-DOF units per second.
    pub joint_velocities: *const f64,
    pub joint_velocities_len: usize,
    /// 7 doubles per end effector: x, y, z (meters), qw, qx, qy, qz.
    pub end_effector_poses: *const f64,
    pub end_effector_poses_len: usize,
    /// 5 doubles per end effector: in_contact (0/1), normal force (N), and the
    /// 3-vector center of pressure.
    pub end_effector_contacts: *const f64,
    pub end_effector_contacts_len: usize,
    /// 7 doubles (x, y, z meters, then quaternion w, x, y, z), or null.
    pub base_pose: *const f64,
    /// 6 doubles (vx, vy, vz m/s, then wx, wy, wz rad/s), or null.
    pub base_velocity: *const f64,
    /// Task reward. Always 0 from the kernel — see [`vcad_gym_step`].
    pub reward: f64,
    /// Episode ended, for either reason.
    pub done: u8,
    /// Ended because a termination condition fired.
    pub terminated: u8,
    /// Ended because `max_steps` was reached.
    pub truncated: u8,
    /// True base pose was resolvable this step.
    pub has_base: u8,
    /// Steps since the last reset, this step included.
    pub step: u32,
    /// Episode's sampled actuator latency in physics substeps.
    pub action_latency_substeps: u32,
    /// **Noise-free** base height in meters — ground truth, deliberately not
    /// equal to `base_pose[2]` when observation noise is configured. Reward
    /// and termination read this; the policy reads `base_pose`.
    pub base_height_m: f64,
    /// Noise-free base tilt from upright, degrees. Same ground-truth caveat.
    pub base_tilt_deg: f64,
    /// UTF-8 termination reason, or null. Borrowed like the arrays.
    pub termination_reason: *const u8,
    pub termination_reason_len: usize,
}

impl VcadGymStepView {
    fn empty() -> Self {
        Self {
            joint_positions: ptr::null(),
            joint_positions_len: 0,
            joint_velocities: ptr::null(),
            joint_velocities_len: 0,
            end_effector_poses: ptr::null(),
            end_effector_poses_len: 0,
            end_effector_contacts: ptr::null(),
            end_effector_contacts_len: 0,
            base_pose: ptr::null(),
            base_velocity: ptr::null(),
            reward: 0.0,
            done: 0,
            terminated: 0,
            truncated: 0,
            has_base: 0,
            step: 0,
            action_latency_substeps: 0,
            base_height_m: 0.0,
            base_tilt_deg: 0.0,
            termination_reason: ptr::null(),
            termination_reason_len: 0,
        }
    }

    fn of(s: &Snapshot) -> Self {
        let (reason, reason_len) = match s.termination_reason.as_ref() {
            Some(r) => (r.as_ptr(), r.len()),
            None => (ptr::null(), 0),
        };
        Self {
            joint_positions: s.joint_positions.as_ptr(),
            joint_positions_len: s.joint_positions.len(),
            joint_velocities: s.joint_velocities.as_ptr(),
            joint_velocities_len: s.joint_velocities.len(),
            end_effector_poses: s.ee_poses.as_ptr(),
            end_effector_poses_len: s.ee_poses.len(),
            end_effector_contacts: s.ee_contacts.as_ptr(),
            end_effector_contacts_len: s.ee_contacts.len(),
            base_pose: s.base_pose.as_ref().map_or(ptr::null(), |p| p.as_ptr()),
            base_velocity: s.base_velocity.as_ref().map_or(ptr::null(), |v| v.as_ptr()),
            reward: s.reward,
            done: s.done as u8,
            terminated: s.terminated as u8,
            truncated: s.truncated as u8,
            has_base: s.has_base as u8,
            step: s.step,
            action_latency_substeps: s.action_latency_substeps,
            base_height_m: s.base_height_m,
            base_tilt_deg: s.base_tilt_deg,
            termination_reason: reason,
            termination_reason_len: reason_len,
        }
    }
}

/// Raise a document's `Free` base joint anchor to `z_mm`. Returns false when
/// the document has no floating base.
pub(crate) fn raise_base(doc: &mut Document, z_mm: f64) -> bool {
    doc.joints
        .iter_mut()
        .flat_map(|js| js.iter_mut())
        .find(|j| matches!(j.kind, vcad_ir::JointKind::Free))
        .map(|j| j.parent_anchor.z = z_mm)
        .is_some()
}

/// Build an env from an owned document plus spec, applying spawn height, PD
/// gains, episode length, and the floating-base check.
fn build_env(mut doc: Document, spec: &GymSpec) -> Option<RobotEnv> {
    if let Some(dir) = spec.base_dir.as_ref() {
        vcad_eval::resolve_mesh_paths(&mut doc, std::path::Path::new(dir));
    }
    if let Some(z) = spec.spawn_z_mm {
        if !raise_base(&mut doc, z) {
            set_error(
                "spec sets spawn_z_mm but the document has no Free base joint \
                 (import the floating-base URDF variant, not the fixed-base one)",
            );
            return None;
        }
    }

    let mut env = ctx("build robot env", || {
        RobotEnv::new_with_config(
            doc,
            spec.end_effector_ids.clone(),
            Some(spec.dt),
            Some(spec.substeps),
            None,
            spec.config.clone(),
        )
    })?;
    env.set_max_steps(spec.max_steps);

    // Reject gains addressed to joints that can't take them. Silently dropping
    // them is how a robot ends up running on the kernel's soft defaults while
    // the config file insists it is stiff.
    //
    // Checked against ACTUATED joints, not all joints: a Fixed joint has no
    // motor to tune, and installing gains on one used to reach a kernel path
    // that indexed one past the end of the control vector and panicked. The
    // check that reads more naturally — "is this a joint in the document?" —
    // is the one that lets that through.
    let actuated: std::collections::HashSet<&str> = env
        .actuated_joint_ids()
        .iter()
        .map(|s| s.as_str())
        .collect();
    for id in spec.gains.keys() {
        if !actuated.contains(id.as_str()) {
            let known = env.joint_ids().contains(id);
            set_error(if known {
                format!(
                    "spec sets PD gains for {id:?}, which has no actuated DOF \
                     (a Fixed joint has no motor). Actuated joints are {:?}",
                    env.actuated_joint_ids()
                )
            } else {
                format!(
                    "spec sets PD gains for unknown joint {id:?}; actuated joints \
                     are {:?}",
                    env.actuated_joint_ids()
                )
            });
            return None;
        }
    }
    for (id, [kp, kd]) in &spec.gains {
        env.set_joint_gains(id, *kp, *kd);
    }

    // Same fail-closed reasoning as the end-effector check, for the base.
    //
    // Both halves are needed and they catch different mistakes. `base_pose`
    // resolving proves there is *an* instance to read; `has_floating_base`
    // proves it can actually move. A fixed-base document passes the first and
    // fails the second — and it is the one that silently reports full-length
    // episodes while every height reading is a constant.
    if spec.require_floating_base {
        let obs = env.reset_with_seed(0);
        if obs.base_pose.is_none() {
            set_error(format!(
                "no base instance reachable from base_instance_id {:?} — every \
                 termination check would be disabled and every height/tilt reading \
                 meaningless. Set require_floating_base=false to simulate anyway.",
                spec.config.base_instance_id
            ));
            return None;
        }
        if !env.has_floating_base() {
            set_error(
                "document has no floating base (no 6-DOF Free joint): the robot is \
                 bolted to the world, so it cannot fall, no termination condition can \
                 fire, and height/tilt read constants. Import the floating-base URDF \
                 variant, or set require_floating_base=false for a genuinely \
                 fixed-base task."
                    .to_string(),
            );
            return None;
        }
    }

    // An end-effector id that doesn't resolve contributes a permanently
    // zero pose and never-in-contact state, which reads as a foot that is
    // always airborne — a policy trained against it learns nothing about
    // contact and the mistake is invisible in the returns.
    let instance_ids = env.instance_ids();
    for id in &spec.end_effector_ids {
        if !instance_ids.contains(id) {
            set_error(format!(
                "end effector {id:?} is not an instance in this document; \
                 its contact channel would be permanently zero"
            ));
            return None;
        }
    }

    Some(env)
}

fn new_handle(env: RobotEnv, spec: GymSpec) -> *mut VcadGym {
    let slots = actuated_slots(&env);
    let obs_dim = feature_dim(&env, &slots);
    let body_ids = env.instance_ids();
    let mut gym = VcadGym {
        env,
        spec,
        slots,
        obs_dim,
        body_ids,
        scene_binding: None,
        snapshot: Snapshot::default(),
        last_result: None,
        scratch_features: Vec::new(),
        scratch_actions: Vec::new(),
    };
    // Seed the snapshot so a caller may read transforms before its first step.
    let obs = gym.env.observe();
    gym.snapshot.absorb_reset(&obs);
    Box::into_raw(Box::new(gym))
}

/// Create an environment from a `.vcad` JSON document and a [`GymSpec`] JSON
/// blob (pass an empty `spec` to accept every default).
///
/// Returns null on failure and sets [`crate::vcad_last_error`] with the
/// reason.
#[no_mangle]
pub extern "C" fn vcad_gym_create(
    doc_json: *const u8,
    doc_json_len: usize,
    spec_json: *const u8,
    spec_json_len: usize,
) -> *mut VcadGym {
    clear_error();
    if doc_json.is_null() {
        set_error("vcad_gym_create: null document pointer");
        return ptr::null_mut();
    }
    catch_unwind(AssertUnwindSafe(|| {
        let doc_bytes = unsafe { std::slice::from_raw_parts(doc_json, doc_json_len) };
        let Some(doc_text) = ctx("document is not UTF-8", || std::str::from_utf8(doc_bytes)) else {
            return ptr::null_mut();
        };
        let Some(doc) = ctx("parse document", || Document::from_json(doc_text)) else {
            return ptr::null_mut();
        };

        let spec = if spec_json.is_null() || spec_json_len == 0 {
            GymSpec::default()
        } else {
            let bytes = unsafe { std::slice::from_raw_parts(spec_json, spec_json_len) };
            let Some(text) = ctx("spec is not UTF-8", || std::str::from_utf8(bytes)) else {
                return ptr::null_mut();
            };
            match ctx("parse gym spec", || serde_json::from_str::<GymSpec>(text)) {
                Some(s) => s,
                None => return ptr::null_mut(),
            }
        };

        match build_env(doc, &spec) {
            Some(env) => new_handle(env, spec),
            None => ptr::null_mut(),
        }
    }))
    .unwrap_or_else(|_| {
        set_error("vcad_gym_create: panic in kernel");
        ptr::null_mut()
    })
}

/// Free an environment. Null is a no-op.
#[no_mangle]
pub extern "C" fn vcad_gym_free(gym: *mut VcadGym) {
    if !gym.is_null() {
        drop(unsafe { Box::from_raw(gym) });
    }
}

/// Reset to the initial state, drawing randomization from `seed`.
///
/// Returns 1 on success, 0 on a null handle.
#[no_mangle]
pub extern "C" fn vcad_gym_reset(gym: *mut VcadGym, seed: u64) -> u8 {
    clear_error();
    if gym.is_null() {
        set_error("vcad_gym_reset: null handle");
        return 0;
    }
    let g: &mut VcadGym = unsafe { &mut *gym };
    catch_unwind(AssertUnwindSafe(|| {
        let obs = g.env.reset_with_seed(seed);
        g.snapshot.absorb_reset(&obs);
        g.last_result = None;
        g.scratch_actions.clear();
        1
    }))
    .unwrap_or_else(|_| {
        set_error("vcad_gym_reset: panic in physics");
        0
    })
}

/// Action encoding for [`vcad_gym_step`].
///
/// `0` torque (N·m / N), `1` position target (degrees / mm), `2` velocity
/// target (deg/s / mm/s). Anything else is rejected.
fn decode_action(kind: u32, values: Vec<f64>) -> Option<Action> {
    match kind {
        0 => Some(Action::Torque(values)),
        1 => Some(Action::PositionTarget(values)),
        2 => Some(Action::VelocityTarget(values)),
        other => {
            set_error(format!(
                "unknown action kind {other} (0=torque, 1=position, 2=velocity)"
            ));
            None
        }
    }
}

/// Step the environment once with `actions_len` action values.
///
/// The action vector indexes [`vcad_gym_actuated_joint_id`], i.e. the
/// document's actuated joints in document order, and its length must equal
/// [`vcad_gym_action_dim`].
///
/// The returned view's `reward` is always 0: the kernel computes no task
/// reward by design, since a reward is a task definition rather than a physics
/// fact. Compute it client-side from the view's ground-truth fields, or use
/// [`crate::train::vcad_gym_reward`] to evaluate the same standing reward the
/// bundled policies were trained against.
///
/// Returns 1 on success, 0 on failure (with the reason in
/// [`crate::vcad_last_error`]).
#[no_mangle]
pub extern "C" fn vcad_gym_step(
    gym: *mut VcadGym,
    actions: *const f64,
    actions_len: usize,
    action_kind: u32,
) -> u8 {
    clear_error();
    if gym.is_null() {
        set_error("vcad_gym_step: null handle");
        return 0;
    }
    let g: &mut VcadGym = unsafe { &mut *gym };
    let expected = g.env.action_dim();
    if actions_len != expected {
        set_error(format!(
            "vcad_gym_step: got {actions_len} action values, env has {expected} actuated DOFs"
        ));
        return 0;
    }
    let values = if actions_len == 0 {
        Vec::new()
    } else if actions.is_null() {
        set_error("vcad_gym_step: null action pointer with non-zero length");
        return 0;
    } else {
        unsafe { std::slice::from_raw_parts(actions, actions_len) }.to_vec()
    };
    let Some(action) = decode_action(action_kind, values) else {
        return 0;
    };
    catch_unwind(AssertUnwindSafe(|| {
        let result = g.env.step_full(action);
        g.snapshot.absorb_step(&result);
        g.last_result = Some(result);
        1
    }))
    .unwrap_or_else(|_| {
        set_error("vcad_gym_step: panic in physics");
        0
    })
}

/// Refresh the snapshot from the env's *true* (noise-free) state without
/// advancing time. Returns 1 on success.
#[no_mangle]
pub extern "C" fn vcad_gym_observe(gym: *mut VcadGym) -> u8 {
    clear_error();
    if gym.is_null() {
        set_error("vcad_gym_observe: null handle");
        return 0;
    }
    let g: &mut VcadGym = unsafe { &mut *gym };
    catch_unwind(AssertUnwindSafe(|| {
        let obs = g.env.observe();
        g.snapshot.absorb_observation(&obs);
        1
    }))
    .unwrap_or_else(|_| {
        set_error("vcad_gym_observe: panic in physics");
        0
    })
}

/// Borrow the most recent step/reset result. Valid until the next mutating
/// call on this handle. An empty view is returned for a null handle.
#[no_mangle]
pub extern "C" fn vcad_gym_step_view(gym: *const VcadGym) -> VcadGymStepView {
    if gym.is_null() {
        return VcadGymStepView::empty();
    }
    let g: &VcadGym = unsafe { &*gym };
    VcadGymStepView::of(&g.snapshot)
}

// =========================================================================
// Introspection
// =========================================================================

/// Number of actuated DOFs — the required action vector length.
#[no_mangle]
pub extern "C" fn vcad_gym_action_dim(gym: *const VcadGym) -> usize {
    if gym.is_null() {
        return 0;
    }
    unsafe { &*gym }.env.action_dim()
}

/// Length of the policy feature vector this env produces.
#[no_mangle]
pub extern "C" fn vcad_gym_obs_dim(gym: *const VcadGym) -> usize {
    if gym.is_null() {
        return 0;
    }
    unsafe { &*gym }.obs_dim
}

/// Full raw observation dimension: every joint's position and velocity slots
/// plus 12 per end effector (7 pose + 5 contact). Distinct from
/// [`vcad_gym_obs_dim`], which is the *policy feature* count.
#[no_mangle]
pub extern "C" fn vcad_gym_observation_dim(gym: *const VcadGym) -> usize {
    if gym.is_null() {
        return 0;
    }
    unsafe { &*gym }.env.observation_dim()
}

/// Control period in seconds (`dt * substeps`) — call `step` at this rate for
/// real-time playback.
#[no_mangle]
pub extern "C" fn vcad_gym_control_dt(gym: *const VcadGym) -> f64 {
    if gym.is_null() {
        return 0.0;
    }
    unsafe { &*gym }.spec.control_dt()
}

/// Episode length in env steps.
#[no_mangle]
pub extern "C" fn vcad_gym_max_steps(gym: *const VcadGym) -> u32 {
    if gym.is_null() {
        return 0;
    }
    unsafe { &*gym }.env.max_steps()
}

/// Borrow a UTF-8 string from a slice of owned strings, C-style.
fn borrow_str(list: &[String], index: usize, out_len: *mut usize) -> *const u8 {
    if !out_len.is_null() {
        unsafe { *out_len = 0 };
    }
    match list.get(index) {
        Some(s) => {
            if !out_len.is_null() {
                unsafe { *out_len = s.len() };
            }
            s.as_ptr()
        }
        None => ptr::null(),
    }
}

/// Number of actuated joints (equals [`vcad_gym_action_dim`] for
/// single-DOF actuators).
#[no_mangle]
pub extern "C" fn vcad_gym_actuated_joint_count(gym: *const VcadGym) -> usize {
    if gym.is_null() {
        return 0;
    }
    unsafe { &*gym }.env.actuated_joint_ids().len()
}

/// Borrow the `index`-th actuated joint id. Valid for the handle's lifetime.
#[no_mangle]
pub extern "C" fn vcad_gym_actuated_joint_id(
    gym: *const VcadGym,
    index: usize,
    out_len: *mut usize,
) -> *const u8 {
    if gym.is_null() {
        if !out_len.is_null() {
            unsafe { *out_len = 0 };
        }
        return ptr::null();
    }
    borrow_str(unsafe { &*gym }.env.actuated_joint_ids(), index, out_len)
}

/// Number of simulated bodies (document instances with a physics body).
#[no_mangle]
pub extern "C" fn vcad_gym_body_count(gym: *const VcadGym) -> usize {
    if gym.is_null() {
        return 0;
    }
    unsafe { &*gym }.body_ids.len()
}

/// Borrow the `index`-th body's instance id.
#[no_mangle]
pub extern "C" fn vcad_gym_body_id(
    gym: *const VcadGym,
    index: usize,
    out_len: *mut usize,
) -> *const u8 {
    if gym.is_null() {
        if !out_len.is_null() {
            unsafe { *out_len = 0 };
        }
        return ptr::null();
    }
    borrow_str(&unsafe { &*gym }.body_ids, index, out_len)
}

// =========================================================================
// Render seam
// =========================================================================

/// Write a position (meters) + quaternion `[w, x, y, z]` pose as a
/// column-major 4×4 in **millimeters**, matching `write_transform_col_major`
/// in the kinematic path exactly.
fn write_pose_col_major(pos: [f64; 3], quat: [f64; 4], out: &mut [f64]) {
    let [w, x, y, z] = quat;
    // Rotation matrix rows from a unit quaternion.
    let r = [
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y - w * z),
            2.0 * (x * z + w * y),
        ],
        [
            2.0 * (x * y + w * z),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z - w * x),
        ],
        [
            2.0 * (x * z - w * y),
            2.0 * (y * z + w * x),
            1.0 - 2.0 * (x * x + y * y),
        ],
    ];
    for col in 0..3 {
        for row in 0..3 {
            out[col * 4 + row] = r[row][col];
        }
        out[col * 4 + 3] = 0.0;
    }
    out[12] = pos[0] * M_TO_MM;
    out[13] = pos[1] * M_TO_MM;
    out[14] = pos[2] * M_TO_MM;
    out[15] = 1.0;
}

/// Write every body's current world transform into `out` as 16 doubles each,
/// column-major and in millimeters, in [`vcad_gym_body_id`] order.
///
/// `out_cap` is the capacity of `out` in doubles and must be at least
/// `16 * vcad_gym_body_count`. Returns the number of bodies written, or 0 on
/// a null handle or insufficient capacity.
#[no_mangle]
pub extern "C" fn vcad_gym_body_transforms(
    gym: *const VcadGym,
    out: *mut f64,
    out_cap: usize,
) -> usize {
    clear_error();
    if gym.is_null() || out.is_null() {
        return 0;
    }
    let g: &VcadGym = unsafe { &*gym };
    let n = g.body_ids.len();
    if out_cap < n * 16 {
        set_error(format!(
            "vcad_gym_body_transforms: need {} doubles, got {out_cap}",
            n * 16
        ));
        return 0;
    }
    let buf = unsafe { std::slice::from_raw_parts_mut(out, n * 16) };
    for (i, id) in g.body_ids.iter().enumerate() {
        match g.env.instance_pose(id) {
            Some((pos, quat)) => write_pose_col_major(pos, quat, &mut buf[i * 16..i * 16 + 16]),
            None => {
                // Identity rather than garbage: a body that momentarily fails
                // to resolve should render at the origin, not with whatever
                // the caller's buffer happened to hold.
                buf[i * 16..i * 16 + 16].copy_from_slice(&[
                    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
                ]);
            }
        }
    }
    n
}

/// Bind the env's bodies to a scene's instance ordering, so the render loop
/// can pull transforms in scene order with no per-frame string lookups.
///
/// Call once after creating the gym and the scene from the same document.
/// Returns the number of scene instances that matched a simulated body;
/// unmatched instances keep their authored transform in
/// [`vcad_gym_scene_transforms`]. Returns 0 on a null handle.
#[no_mangle]
pub extern "C" fn vcad_gym_bind_scene(gym: *mut VcadGym, scene: *const crate::VcadScene) -> usize {
    clear_error();
    if gym.is_null() || scene.is_null() {
        set_error("vcad_gym_bind_scene: null handle");
        return 0;
    }
    let g: &mut VcadGym = unsafe { &mut *gym };
    let s: &crate::VcadScene = unsafe { &*scene };
    let index_of: HashMap<&str, usize> = g
        .body_ids
        .iter()
        .enumerate()
        .map(|(i, id)| (id.as_str(), i))
        .collect();

    let instances = s.instances();
    let mut binding = Vec::with_capacity(instances.len());
    let mut matched = 0usize;
    for inst_id in instances {
        match index_of.get(inst_id.as_str()) {
            Some(&i) => {
                binding.push(Some(i));
                matched += 1;
            }
            None => binding.push(None),
        }
    }
    g.scene_binding = Some(binding);
    matched
}

/// Number of scene instances bound by [`vcad_gym_bind_scene`], or 0 when
/// nothing is bound.
#[no_mangle]
pub extern "C" fn vcad_gym_scene_binding_len(gym: *const VcadGym) -> usize {
    if gym.is_null() {
        return 0;
    }
    unsafe { &*gym }
        .scene_binding
        .as_ref()
        .map_or(0, |b| b.len())
}

/// Write transforms for every bound scene instance into `out` (16 doubles
/// each, column-major, millimeters) in **scene instance order** — the same
/// index space [`crate::vcad_scene_instance_mesh`] uses, so a renderer swaps
/// this in for `vcad_scene_solve_fk` with no other change.
///
/// Instances with no simulated body are left untouched, so a caller that
/// pre-fills `out` with authored transforms keeps them for static scenery.
///
/// Requires a prior [`vcad_gym_bind_scene`]. Returns the number of instances
/// written (including untouched ones), or 0 if unbound / undersized.
#[no_mangle]
pub extern "C" fn vcad_gym_scene_transforms(
    gym: *const VcadGym,
    out: *mut f64,
    out_cap: usize,
) -> usize {
    clear_error();
    if gym.is_null() || out.is_null() {
        return 0;
    }
    let g: &VcadGym = unsafe { &*gym };
    let Some(binding) = g.scene_binding.as_ref() else {
        set_error("vcad_gym_scene_transforms: call vcad_gym_bind_scene first");
        return 0;
    };
    let n = binding.len();
    if out_cap < n * 16 {
        set_error(format!(
            "vcad_gym_scene_transforms: need {} doubles, got {out_cap}",
            n * 16
        ));
        return 0;
    }
    let buf = unsafe { std::slice::from_raw_parts_mut(out, n * 16) };
    for (i, slot) in binding.iter().enumerate() {
        let Some(body) = slot else { continue };
        let Some(id) = g.body_ids.get(*body) else {
            continue;
        };
        if let Some((pos, quat)) = g.env.instance_pose(id) {
            write_pose_col_major(pos, quat, &mut buf[i * 16..i * 16 + 16]);
        }
    }
    n
}

// =========================================================================
// Policy inference
// =========================================================================

/// A trained policy, whichever architecture the file holds.
///
/// Inference lives on this side of the boundary rather than in Swift on
/// purpose: the forward pass has to match the one training used *exactly*
/// (whitening, output clamp, default-pose offset, degree conversion), and a
/// reimplementation that drifts by a clamp produces a robot that almost stands
/// — the hardest kind of bug to attribute.
pub enum VcadPolicy {
    /// Linear policy over whitened features.
    Linear(Box<LinearPolicy>),
    /// One-hidden-layer tanh policy.
    Mlp(Box<MlpPolicy>),
}

impl VcadPolicy {
    fn obs_dim(&self) -> usize {
        match self {
            Self::Linear(p) => p.obs_dim,
            Self::Mlp(p) => p.obs_dim,
        }
    }

    fn act_dim(&self) -> usize {
        match self {
            Self::Linear(p) => p.act_dim,
            Self::Mlp(p) => p.act_dim,
        }
    }

    fn act(&self, features: &[f64]) -> Vec<f64> {
        match self {
            Self::Linear(p) => Policy::act(p.as_ref(), features),
            Self::Mlp(p) => Policy::act(p.as_ref(), features),
        }
    }

    /// Parse from either a bare policy object or a `{"policy": {...}}`
    /// training bundle — the shape `k1_stand` writes.
    ///
    /// The architecture discriminator is the presence of `hidden`: only
    /// [`MlpPolicy`] has it. Guessing wrong is not silent (each type has a
    /// required field the other lacks) but it is still a failure, so it is
    /// decided here in one place rather than by each caller.
    fn from_json(text: &str) -> Result<Self, String> {
        let blob: serde_json::Value =
            serde_json::from_str(text).map_err(|e| format!("not valid JSON: {e}"))?;
        let pol = blob.get("policy").cloned().unwrap_or(blob);
        if pol.get("hidden").is_some() {
            serde_json::from_value::<MlpPolicy>(pol)
                .map(|p| Self::Mlp(Box::new(p)))
                .map_err(|e| format!("not a valid MLP policy: {e}"))
        } else {
            serde_json::from_value::<LinearPolicy>(pol)
                .map(|p| Self::Linear(Box::new(p)))
                .map_err(|e| format!("not a valid linear policy: {e}"))
        }
    }
}

/// Load a trained policy from JSON (a bare policy object, or a training
/// bundle with a `policy` field). Returns null and sets the last error on
/// failure.
#[no_mangle]
pub extern "C" fn vcad_policy_load(json: *const u8, json_len: usize) -> *mut VcadPolicy {
    clear_error();
    if json.is_null() {
        set_error("vcad_policy_load: null pointer");
        return ptr::null_mut();
    }
    catch_unwind(AssertUnwindSafe(|| {
        let bytes = unsafe { std::slice::from_raw_parts(json, json_len) };
        let Some(text) = ctx("policy is not UTF-8", || std::str::from_utf8(bytes)) else {
            return ptr::null_mut();
        };
        match ctx("load policy", || VcadPolicy::from_json(text)) {
            Some(p) => Box::into_raw(Box::new(p)),
            None => ptr::null_mut(),
        }
    }))
    .unwrap_or_else(|_| {
        set_error("vcad_policy_load: panic");
        ptr::null_mut()
    })
}

/// Free a policy. Null is a no-op.
#[no_mangle]
pub extern "C" fn vcad_policy_free(policy: *mut VcadPolicy) {
    if !policy.is_null() {
        drop(unsafe { Box::from_raw(policy) });
    }
}

/// Feature count the policy expects.
#[no_mangle]
pub extern "C" fn vcad_policy_obs_dim(policy: *const VcadPolicy) -> usize {
    if policy.is_null() {
        return 0;
    }
    unsafe { &*policy }.obs_dim()
}

/// Action count the policy emits.
#[no_mangle]
pub extern "C" fn vcad_policy_act_dim(policy: *const VcadPolicy) -> usize {
    if policy.is_null() {
        return 0;
    }
    unsafe { &*policy }.act_dim()
}

/// `1` when the policy is an MLP, `0` when linear (or on a null handle).
#[no_mangle]
pub extern "C" fn vcad_policy_is_mlp(policy: *const VcadPolicy) -> u8 {
    if policy.is_null() {
        return 0;
    }
    matches!(unsafe { &*policy }, VcadPolicy::Mlp(_)) as u8
}

/// Check that `policy` is dimensionally compatible with `gym`, setting a
/// descriptive last error if not. Returns 1 when compatible.
///
/// Worth its own entry point because the failure is otherwise a robot that
/// twitches: an action vector of the wrong length, or features built from a
/// different joint set, produce numbers rather than an error.
#[no_mangle]
pub extern "C" fn vcad_policy_check(gym: *const VcadGym, policy: *const VcadPolicy) -> u8 {
    clear_error();
    if gym.is_null() || policy.is_null() {
        set_error("vcad_policy_check: null handle");
        return 0;
    }
    let g: &VcadGym = unsafe { &*gym };
    let p: &VcadPolicy = unsafe { &*policy };
    if p.obs_dim() != g.obs_dim {
        set_error(format!(
            "policy expects {} features, this env produces {} — the policy was \
             trained on a different robot (or a different end-effector set)",
            p.obs_dim(),
            g.obs_dim
        ));
        return 0;
    }
    if p.act_dim() != g.env.action_dim() {
        set_error(format!(
            "policy emits {} actions, this env has {} actuated DOFs",
            p.act_dim(),
            g.env.action_dim()
        ));
        return 0;
    }
    1
}

/// Step the env by evaluating `policy` on the current observation.
///
/// This is the intended inference path. It builds the feature vector with the
/// *same* [`vcad_sim::rl::features`] the trainer used, applies the policy's
/// own forward pass, and issues a position-target action — the whole
/// observation→action chain stays inside Rust, so it cannot drift from
/// training. Reimplementing any link of it in Swift is how a policy silently
/// degrades.
///
/// Returns 1 on success. On a dimension mismatch it fails (0) rather than
/// stepping with garbage — call [`vcad_policy_check`] once at load time for
/// the descriptive message.
#[no_mangle]
pub extern "C" fn vcad_gym_policy_step(gym: *mut VcadGym, policy: *const VcadPolicy) -> u8 {
    clear_error();
    if gym.is_null() || policy.is_null() {
        set_error("vcad_gym_policy_step: null handle");
        return 0;
    }
    if vcad_policy_check(gym, policy) == 0 {
        return 0;
    }
    let g: &mut VcadGym = unsafe { &mut *gym };
    let p: &VcadPolicy = unsafe { &*policy };
    catch_unwind(AssertUnwindSafe(|| {
        // Features must come from the env's true observation, matching
        // `rl::rollout`, which feeds the observation returned by the previous
        // step (noisy when configured) — so read the snapshot the same way.
        let obs = g.env.observe();
        g.scratch_features.clear();
        g.scratch_features
            .extend_from_slice(&features(&obs, &g.slots, g.spec.nominal_height_m));
        g.scratch_actions = p.act(&g.scratch_features);
        let result = g
            .env
            .step_full(Action::PositionTarget(g.scratch_actions.clone()));
        g.snapshot.absorb_step(&result);
        g.last_result = Some(result);
        1
    }))
    .unwrap_or_else(|_| {
        set_error("vcad_gym_policy_step: panic in physics");
        0
    })
}

/// Borrow the action vector the most recent [`vcad_gym_policy_step`] issued
/// (joint position targets in degrees), for UI display. Null before the first
/// policy step.
#[no_mangle]
pub extern "C" fn vcad_gym_last_action(gym: *const VcadGym, out_len: *mut usize) -> *const f64 {
    if !out_len.is_null() {
        unsafe { *out_len = 0 };
    }
    if gym.is_null() {
        return ptr::null();
    }
    let g: &VcadGym = unsafe { &*gym };
    if g.scratch_actions.is_empty() {
        return ptr::null();
    }
    if !out_len.is_null() {
        unsafe { *out_len = g.scratch_actions.len() };
    }
    g.scratch_actions.as_ptr()
}

/// Write the current policy feature vector into `out` (for plotting the
/// policy's actual input). Returns the number of features written.
#[no_mangle]
pub extern "C" fn vcad_gym_features(gym: *mut VcadGym, out: *mut f64, out_cap: usize) -> usize {
    clear_error();
    if gym.is_null() || out.is_null() {
        return 0;
    }
    let g: &mut VcadGym = unsafe { &mut *gym };
    if out_cap < g.obs_dim {
        set_error(format!(
            "vcad_gym_features: need {} doubles, got {out_cap}",
            g.obs_dim
        ));
        return 0;
    }
    catch_unwind(AssertUnwindSafe(|| {
        let obs = g.env.observe();
        let f = features(&obs, &g.slots, g.spec.nominal_height_m);
        let buf = unsafe { std::slice::from_raw_parts_mut(out, f.len()) };
        buf.copy_from_slice(&f);
        f.len()
    }))
    .unwrap_or(0)
}

/// Evaluate `spec` against this gym's most recent step.
///
/// Returns 0 before the first step of an episode (a reset has produced no
/// step to score yet), and `NaN` on a null handle.
pub(crate) fn reward_of_last_step(gym: *const VcadGym, spec: &crate::train::RewardSpec) -> f64 {
    if gym.is_null() {
        return f64::NAN;
    }
    let g: &VcadGym = unsafe { &*gym };
    match g.last_result.as_ref() {
        Some(r) => spec.eval(r, &g.scratch_actions),
        None => 0.0,
    }
}

/// Shove the floating base: `d_omega` rad/s and `d_v` m/s added to its current
/// velocity, in the free joint's own frame (angular first, then body-frame
/// linear — see [`RobotEnv::nudge_base`]).
///
/// The "poke the robot" primitive. Returns 1 on success, 0 when the document
/// has no floating base to shove.
#[no_mangle]
pub extern "C" fn vcad_gym_nudge_base(
    gym: *mut VcadGym,
    dwx: f64,
    dwy: f64,
    dwz: f64,
    dvx: f64,
    dvy: f64,
    dvz: f64,
) -> u8 {
    clear_error();
    if gym.is_null() {
        set_error("vcad_gym_nudge_base: null handle");
        return 0;
    }
    let g: &mut VcadGym = unsafe { &mut *gym };
    catch_unwind(AssertUnwindSafe(|| {
        if g.env.nudge_base([dwx, dwy, dwz], [dvx, dvy, dvz]) {
            1
        } else {
            set_error("vcad_gym_nudge_base: document has no floating base");
            0
        }
    }))
    .unwrap_or_else(|_| {
        set_error("vcad_gym_nudge_base: panic in physics");
        0
    })
}

/// Build a zero policy (holds the rest pose) matched to this env — the
/// baseline every trained policy must beat, and a safe default for the app to
/// step with before anything is loaded.
///
/// `action_scale_deg` bounds how far one action may move a joint from the
/// default pose; the zero weights make it inert regardless.
#[no_mangle]
pub extern "C" fn vcad_policy_zeros(gym: *const VcadGym, action_scale_deg: f64) -> *mut VcadPolicy {
    clear_error();
    if gym.is_null() {
        set_error("vcad_policy_zeros: null handle");
        return ptr::null_mut();
    }
    let g: &VcadGym = unsafe { &*gym };
    let act_dim = g.env.action_dim();
    Box::into_raw(Box::new(VcadPolicy::Linear(Box::new(LinearPolicy::zeros(
        g.obs_dim,
        act_dim,
        ActionSpec {
            default_pose_deg: vec![0.0; act_dim],
            action_scale_deg,
        },
    )))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_defaults_are_the_k1_training_settings() {
        let spec: GymSpec = serde_json::from_str("{}").unwrap();
        assert_eq!(spec.dt, 1.0 / 1000.0);
        assert_eq!(spec.substeps, 20);
        assert_eq!(spec.max_steps, 400);
        assert!(spec.require_floating_base);
        // 1 kHz physics, 50 Hz control — the ratio the stiff gains need.
        // Tolerance is 1e-9, not exact: `dt` is `f32` (phyz's own timestep
        // type), so 1/1000 is not representable and the product carries ~1e-11
        // of relative error. That is far below anything dynamically
        // meaningful, but it is not zero.
        assert!(
            (spec.control_dt() - 0.02).abs() < 1e-9,
            "{}",
            spec.control_dt()
        );
    }

    #[test]
    fn spec_rejects_unknown_fields() {
        // A typo in a config file must not silently fall back to a default.
        let err = serde_json::from_str::<GymSpec>(r#"{"dtt": 0.001}"#).unwrap_err();
        assert!(err.to_string().contains("unknown field"), "{err}");
    }

    #[test]
    fn identity_pose_writes_an_identity_matrix_scaled_to_mm() {
        let mut out = [0.0f64; 16];
        write_pose_col_major([0.0, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0], &mut out);
        let expected = [
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ];
        assert_eq!(out, expected);
    }

    #[test]
    fn translation_is_converted_from_meters_to_millimeters() {
        let mut out = [0.0f64; 16];
        write_pose_col_major([0.5498, -0.25, 1.0], [1.0, 0.0, 0.0, 0.0], &mut out);
        assert!((out[12] - 549.8).abs() < 1e-9);
        assert!((out[13] + 250.0).abs() < 1e-9);
        assert!((out[14] - 1000.0).abs() < 1e-9);
        assert_eq!(out[15], 1.0);
    }

    #[test]
    fn quaternion_rotation_is_written_column_major() {
        // 90° about +Z: maps +X to +Y. Column-major means the first column
        // (out[0..3]) is the image of the X basis vector.
        let s = std::f64::consts::FRAC_1_SQRT_2;
        let mut out = [0.0f64; 16];
        write_pose_col_major([0.0; 3], [s, 0.0, 0.0, s], &mut out);
        assert!((out[0] - 0.0).abs() < 1e-12, "{out:?}");
        assert!((out[1] - 1.0).abs() < 1e-12, "{out:?}");
        assert!((out[4] + 1.0).abs() < 1e-12, "{out:?}");
        assert!((out[5] - 0.0).abs() < 1e-12, "{out:?}");
        assert!((out[10] - 1.0).abs() < 1e-12);
    }

    #[test]
    fn null_handles_are_inert() {
        assert_eq!(vcad_gym_action_dim(ptr::null()), 0);
        assert_eq!(vcad_gym_obs_dim(ptr::null()), 0);
        assert_eq!(vcad_gym_body_count(ptr::null()), 0);
        assert_eq!(vcad_gym_control_dt(ptr::null()), 0.0);
        assert_eq!(vcad_gym_step(ptr::null_mut(), ptr::null(), 0, 1), 0);
        assert_eq!(vcad_gym_reset(ptr::null_mut(), 0), 0);
        assert_eq!(vcad_policy_obs_dim(ptr::null()), 0);
        vcad_gym_free(ptr::null_mut());
        vcad_policy_free(ptr::null_mut());
        let v = vcad_gym_step_view(ptr::null());
        assert!(v.joint_positions.is_null());
        assert_eq!(v.done, 0);
    }

    #[test]
    fn create_reports_a_reason_for_a_malformed_document() {
        let bad = b"{ not json";
        let g = vcad_gym_create(bad.as_ptr(), bad.len(), ptr::null(), 0);
        assert!(g.is_null());
        let mut len = 0usize;
        let p = crate::vcad_last_error(&mut len);
        assert!(!p.is_null(), "a failed create must explain itself");
        let msg = std::str::from_utf8(unsafe { std::slice::from_raw_parts(p, len) }).unwrap();
        assert!(msg.contains("parse document"), "{msg}");
    }

    #[test]
    fn action_kinds_decode_and_reject() {
        assert!(matches!(
            decode_action(0, vec![1.0]),
            Some(Action::Torque(_))
        ));
        assert!(matches!(
            decode_action(1, vec![1.0]),
            Some(Action::PositionTarget(_))
        ));
        assert!(matches!(
            decode_action(2, vec![1.0]),
            Some(Action::VelocityTarget(_))
        ));
        assert!(decode_action(7, vec![1.0]).is_none());
    }

    #[test]
    fn policy_json_accepts_both_bare_and_bundled_shapes() {
        let linear = LinearPolicy::zeros(
            4,
            2,
            ActionSpec {
                default_pose_deg: vec![0.0; 2],
                action_scale_deg: 8.0,
            },
        );
        let bare = serde_json::to_string(&linear).unwrap();
        let bundled = format!(r#"{{"policy": {bare}, "kept": "last"}}"#);

        for (label, text) in [("bare", &bare), ("bundled", &bundled)] {
            let p = VcadPolicy::from_json(text).unwrap_or_else(|e| panic!("{label}: {e}"));
            assert_eq!(p.obs_dim(), 4);
            assert_eq!(p.act_dim(), 2);
            assert!(matches!(p, VcadPolicy::Linear(_)));
        }
    }

    #[test]
    fn policy_json_discriminates_mlp_by_the_hidden_field() {
        let mlp = MlpPolicy::new(
            6,
            8,
            3,
            ActionSpec {
                default_pose_deg: vec![0.0; 3],
                action_scale_deg: 8.0,
            },
            1,
        );
        let text = serde_json::to_string(&mlp).unwrap();
        let p = VcadPolicy::from_json(&text).unwrap();
        assert!(matches!(p, VcadPolicy::Mlp(_)));
        assert_eq!(p.obs_dim(), 6);
        assert_eq!(p.act_dim(), 3);
    }

    #[test]
    fn a_zero_policy_commands_exactly_the_default_pose() {
        let act_dim = 3;
        let p = LinearPolicy::zeros(
            5,
            act_dim,
            ActionSpec {
                default_pose_deg: vec![10.0, -5.0, 0.0],
                action_scale_deg: 8.0,
            },
        );
        let out = Policy::act(&p, &[1.0, 2.0, 3.0, 4.0, 5.0]);
        assert_eq!(out, vec![10.0, -5.0, 0.0]);
    }
}
