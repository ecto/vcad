//! Physics world management using phyz.

use std::collections::HashMap;

use phyz::aba_with_external_forces;
use phyz::math::{Mat3, Quat, SpatialInertia, SpatialTransform, Vec3};
use phyz::model::{Model, ModelBuilder, State};
use phyz::{
    forward_kinematics, Collision, ContactMaterial, ContactProblem, ContactSolverConfig, Geometry,
};
use serde::{Deserialize, Serialize};
use vcad_ir::{Document, InertialProperties, JointKind};

use crate::colliders::{estimate_mass, mesh_to_collider, ColliderStrategy};
use crate::error::PhysicsError;
use crate::joints::{
    convert_q_dof_to_physics, convert_state_from_physics, convert_state_to_physics,
    convert_v_dof_to_physics, joint_ndof, vcad_joint_to_phyz, MotorMode, MotorTarget,
};

/// Largest ω·dt an explicit PD servo may run at before it is reported as
/// unstable, where ω = √(kp / I_reflected) is the closed-loop natural
/// frequency of the joint.
///
/// The divergence boundary for critically-damped explicit PD sits near
/// ω·dt ≈ 1 (measured in `tests/servo_stability.rs`: the servo tracks its
/// target cleanly through 0.8, overshoots at 1.0, and is thrown off the
/// target entirely by 1.3). 0.3 leaves the margin the nonlinear multi-body case
/// actually needs — coupling, contact impulses and gain randomization all
/// push a marginal joint over.
pub const GAIN_STABILITY_LIMIT: f64 = 0.3;

/// One joint whose explicit PD gains are too stiff for the integrator at a
/// given timestep. Produced by [`PhysicsWorld::check_gain_stability`].
#[derive(Debug, Clone, PartialEq)]
pub struct GainWarning {
    /// The vcad joint ID.
    pub joint_id: String,
    /// The effective proportional gain (after domain-randomization scaling).
    pub kp: f64,
    /// Measured reflected inertia of the joint's DOF (kg·m² or kg).
    pub reflected_inertia: f64,
    /// ω·dt at the checked timestep — unstable above
    /// [`GAIN_STABILITY_LIMIT`].
    pub omega_dt: f64,
    /// The largest `kp` that stays inside the limit at this timestep.
    pub max_stable_kp: f64,
    /// Substep count needed to bring ω·dt under the limit. From
    /// [`PhysicsWorld::check_gain_stability`] this is a *multiplier* on
    /// whatever substep count the caller runs (the world doesn't know it);
    /// [`crate::RobotEnv::check_gain_stability`] rescales it to the absolute
    /// substep count for that env.
    pub min_substeps: u32,
}

impl std::fmt::Display for GainWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "joint '{}': kp={:.4} with reflected inertia {:.3e} gives omega*dt = {:.2} \
             (unstable above ~{:.1}); raise substeps to >= {}x or lower kp below {:.4}",
            self.joint_id,
            self.kp,
            self.reflected_inertia,
            self.omega_dt,
            GAIN_STABILITY_LIMIT,
            self.min_substeps,
            self.max_stable_kp,
        )
    }
}

/// Ground-plane contact configuration for a physics world.
///
/// The ground is an infinite horizontal plane at `z = height` (metres,
/// world frame). When enabled, every *movable* body's collision geometry is
/// tested against it each substep and resolved through phyz's convex contact
/// solver (Coulomb friction cone, restitution as a target normal velocity).
/// Bodies welded to the world through nothing but Fixed joints are skipped —
/// their contacts could exert no force and would only pad the Delassus
/// system.
///
/// Body-body (robot self-) collision is not handled here yet.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GroundConfig {
    /// Whether ground contact is active.
    pub enabled: bool,
    /// Ground plane height, metres in the world frame (plane is z = height).
    pub height: f64,
    /// Coulomb friction coefficient of the ground.
    pub friction: f64,
    /// Restitution (0 = inelastic rest, 1 = elastic bounce).
    pub restitution: f64,
}

impl Default for GroundConfig {
    /// Ground on at z = 0 with friction 0.8 and inelastic contact.
    fn default() -> Self {
        Self {
            enabled: true,
            height: 0.0,
            friction: 0.8,
            restitution: 0.0,
        }
    }
}

impl GroundConfig {
    /// A disabled ground plane — the pre-contact, contact-free dynamics.
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Self::default()
        }
    }
}

/// Per-instance world-frame pose: `(position_m, quaternion_wxyz)`.
pub type Pose = ([f64; 3], [f64; 4]);

/// Map of instance id → world-frame pose, returned from
/// [`PhysicsWorld::forward_kinematics_at`].
pub type PoseMap = HashMap<String, Pose>;

/// State of a single joint.
#[derive(Debug, Clone, Default)]
pub struct JointState {
    /// Joint position (degrees for revolute, mm for prismatic).
    pub position: f64,
    /// Joint velocity (deg/s for revolute, mm/s for prismatic).
    pub velocity: f64,
    /// Joint effort/torque (Nm for revolute, N for prismatic).
    pub effort: f64,
}

/// Physics simulation world.
pub struct PhysicsWorld {
    // phyz components
    model: Model,
    state: State,

    // Motor targets for PD control
    motors: HashMap<String, MotorTarget>,

    // Explicit per-joint PD gains `(kp, kd)`. When present they override the
    // inertia-scaled defaults for position/velocity servos on that joint.
    joint_gains: HashMap<String, (f64, f64)>,

    // Mapping from vcad IDs to phyz indices
    instance_to_body: HashMap<String, usize>,
    joint_to_index: HashMap<String, usize>,

    // Joint ids in document order — the canonical ordering for observation
    // vectors and positional q arguments. Never iterate joint_to_index for
    // ordering: HashMap iteration order permutes run-to-run.
    joint_order: Vec<String>,

    // Original joint definitions for unit conversion
    joint_kinds: HashMap<String, JointKind>,

    // Joint DOF offsets in the state vectors
    joint_q_offsets: HashMap<String, usize>,
    joint_v_offsets: HashMap<String, usize>,

    // Per-body part-local → body-frame transform (rotation, translation in
    // meters): `p_body = R * p_part_m + t`. phyz body frames coincide with
    // the joint frame (Featherstone), which for a jointed child is rotated by
    // the axis-alignment rotation and anchored at the child anchor. Identity
    // for ground/free bodies. Needed to report part poses to callers.
    body_part_frames: Vec<(Mat3, Vec3)>,

    // Spatial velocities per body (body frame), refreshed alongside
    // `state.body_xform` by [`Self::step`] / [`Self::refresh_kinematics`].
    body_vels: Vec<phyz::math::SpatialVec>,

    // Multiplier applied to the auto-derived PD gains (kp, kd) of position
    // and velocity motors. Domain randomization scales this per episode to
    // model actuator strength/controller mismatch.
    gain_scale: f64,

    // Ground-plane contact configuration. Disabled by default at this level;
    // RobotEnv turns it on for gym use.
    ground: GroundConfig,

    // Per-body collision geometry used for ground contact, `None` for bodies
    // that cannot move (welded to the world through Fixed joints only) —
    // contacts on those would contribute empty Jacobian rows.
    contact_geometries: Vec<Option<Geometry>>,

    // Per-body, body-frame support points for mesh colliders — the only
    // vertices that can be the deepest point of a plane contact. Precomputed
    // once (see `colliders::support_points`) so the per-substep candidate
    // scan doesn't walk the whole tessellation. `None` for bodies with no
    // contact geometry or a non-mesh collider.
    contact_support: Vec<Option<Vec<Vec3>>>,

    // True when any joint is Free — i.e. the document describes a
    // floating-base robot. Gates gravity-compensation feedforward (see
    // `apply_motor_torques`).
    has_floating_base: bool,

    // Per-body ground-contact state from the most recent `step`, indexed like
    // `model.bodies`. Overwritten every step (so after a multi-substep env
    // step it reports the final substep), and cleared to "no contact" when a
    // step finds no manifold or the ground is disabled.
    body_contacts: Vec<ContactState>,
}

/// Ground-contact state of one body, as of the most recent
/// [`PhysicsWorld::step`].
///
/// This is the physical sensor a foot force plate / ankle F/T would read: a
/// touch flag, the total normal load, and where it acts.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ContactState {
    /// True when the body's collider had at least one point penetrating the
    /// ground plane during the last step — touching, whether or not the
    /// solver had to push (a grazing touch reports `normal_force == 0`).
    pub in_contact: bool,
    /// Total normal force over the body's contact manifold, in newtons
    /// (the solved normal impulse divided by the step's `dt`). A body at rest
    /// reads its supported weight; airborne reads `0`.
    pub normal_force: f64,
    /// Impulse-weighted centroid of the manifold in world meters — the
    /// center of pressure. `[0, 0, 0]` when not in contact; falls back to the
    /// unweighted centroid when the manifold carries no normal impulse.
    pub point: [f64; 3],
}

impl Default for ContactState {
    fn default() -> Self {
        Self {
            in_contact: false,
            normal_force: 0.0,
            point: [0.0; 3],
        }
    }
}

impl PhysicsWorld {
    /// Create a new physics world from a vcad Document.
    ///
    /// The document must have assembly data (instances and joints).
    pub fn from_document(doc: &Document) -> Result<Self, PhysicsError> {
        Self::from_document_with_colliders(doc, ColliderStrategy::ConvexHull)
    }

    /// Build a world using a specific collider strategy.
    ///
    /// The strategy is not cosmetic across backends. phyz's **GPU** contact
    /// pipeline packs eight floats per body and understands only `Sphere`,
    /// `Box`, `Capsule` and `Cylinder`; anything else — including the
    /// `Geometry::Mesh` that [`ColliderStrategy::ConvexHull`] produces — falls
    /// through its match to "type 0", which means *no collision*, silently.
    /// A vcad robot built the default way is therefore invisible to GPU
    /// contact and drops through the floor, while the same robot on the CPU
    /// (which handles meshes) stands on it.
    ///
    /// [`ColliderStrategy::Aabb`] emits `Geometry::Box`, which the GPU does
    /// understand. That is a real fidelity trade — a box is not a hull — so it
    /// is opt-in rather than the default, and a batch that wants contact has
    /// to ask for it.
    pub fn from_document_with_colliders(
        doc: &Document,
        collider_strategy: ColliderStrategy,
    ) -> Result<Self, PhysicsError> {
        let instances = doc.instances.as_ref().ok_or(PhysicsError::NoAssembly)?;
        let joints = doc.joints.as_ref().ok_or(PhysicsError::NoAssembly)?;
        let part_defs = doc.part_defs.as_ref().ok_or(PhysicsError::NoAssembly)?;
        let ground_id = doc
            .ground_instance_id
            .as_ref()
            .ok_or(PhysicsError::NoGroundInstance)?;

        // Build the articulated tree.
        // Strategy: ground instance is a fixed root body. Each joint connects
        // parent → child as in the vcad assembly. Instances without joints that
        // aren't ground become free-floating bodies.
        //
        // We first need to figure out the ordering. phyz requires that parent
        // bodies come before children. We'll process joints in dependency order.

        // We'll build bodies in order: ground first, then joint-connected instances
        // in topological order, then free-floating instances.
        let mut builder = ModelBuilder::new()
            .gravity(Vec3::new(0.0, 0.0, -9.81))
            .dt(1.0 / 240.0);

        let mut instance_to_body: HashMap<String, usize> = HashMap::new();
        let mut joint_to_index: HashMap<String, usize> = HashMap::new();
        let mut joint_kinds: HashMap<String, JointKind> = HashMap::new();
        let mut body_geometries: Vec<Option<Geometry>> = Vec::new();
        let mut body_part_frames: Vec<(Mat3, Vec3)> = Vec::new();
        let mut body_count = 0usize;

        // Tessellated part geometry, keyed by PartDef root node. Instances
        // sharing a PartDef (every leg of a pattern, every identical link)
        // evaluate its boolean tree once.
        let mesh_cache: std::cell::RefCell<
            HashMap<vcad_ir::NodeId, vcad_kernel_tessellate::TriangleMesh>,
        > = std::cell::RefCell::new(HashMap::new());

        // Helper: evaluate mesh, mass, and (optionally) authored inertials
        // for an instance. When the instance's PartDef carries an
        // `inertial` block (set by the URDF importer for any link with an
        // `<inertial>` tag), we surface those values; the caller prefers
        // them over mesh-derived inertia.
        // `part_frame`: optional part-local → body-frame map `(R, anchor_mm)`
        // with `p_body = R * (p_part - anchor)`. When present, the mesh (and
        // any authored inertial) is re-expressed in the body frame before
        // mass/collider/inertia are computed, because phyz body frames
        // coincide with the joint frame, not the part's local frame.
        let eval_instance = |inst: &vcad_ir::Instance,
                             part_frame: Option<(&Mat3, &vcad_ir::Vec3)>|
         -> Result<
            (
                vcad_kernel_tessellate::TriangleMesh,
                f64,
                Geometry,
                Option<InertialProperties>,
            ),
            PhysicsError,
        > {
            let part_def = part_defs
                .get(&inst.part_def_id)
                .ok_or_else(|| PhysicsError::MissingPartDef(inst.part_def_id.clone()))?;
            let cached = mesh_cache.borrow().get(&part_def.root).cloned();
            let mut authored = part_def.inertial;
            let mut mesh = match cached {
                Some(m) => m,
                None => {
                    let m = match Self::evaluate_part(doc, part_def.root)? {
                        Some(m) => m,
                        // No resolvable geometry. Acceptable only when the
                        // PartDef carries an authored `inertial` block (the
                        // URDF path, whose `package://` meshes often aren't on
                        // disk): mass, COM and inertia then come from the
                        // authored values and the placeholder only stands in
                        // as a collider. Without authored inertials we would
                        // be inventing the mass properties the dynamics run
                        // on — exactly the failure this fallback used to hide
                        // — so refuse instead.
                        None if authored.is_some() => placeholder_collider_mesh(),
                        None => {
                            return Err(PhysicsError::Evaluation(format!(
                                "part '{}' (node {}) has no resolvable geometry and no \
                                 authored inertial block — cannot derive mass properties",
                                part_def.id, part_def.root
                            )))
                        }
                    };
                    mesh_cache.borrow_mut().insert(part_def.root, m.clone());
                    m
                }
            };
            if let Some((rot, anchor_mm)) = part_frame {
                for v in mesh.vertices.chunks_mut(3) {
                    let p = Vec3::new(
                        v[0] as f64 - anchor_mm.x,
                        v[1] as f64 - anchor_mm.y,
                        v[2] as f64 - anchor_mm.z,
                    );
                    let q = rot.mul_vec(p);
                    v[0] = q.x as f32;
                    v[1] = q.y as f32;
                    v[2] = q.z as f32;
                }
                if let Some(props) = authored.as_mut() {
                    // COM is in mm, inertia about COM in kg·m² — rotate both
                    // into the body frame.
                    let com = Vec3::new(
                        props.com_mm.x - anchor_mm.x,
                        props.com_mm.y - anchor_mm.y,
                        props.com_mm.z - anchor_mm.z,
                    );
                    let com_b = rot.mul_vec(com);
                    props.com_mm = vcad_ir::Vec3::new(com_b.x, com_b.y, com_b.z);
                    let [ixx, iyy, izz, ixy, ixz, iyz] = props.inertia_kg_m2;
                    let i = Mat3::new(ixx, ixy, ixz, ixy, iyy, iyz, ixz, iyz, izz);
                    let i_b = rot.mul_mat(&i).mul_mat(&rot.transpose());
                    props.inertia_kg_m2 = [
                        i_b[(0, 0)],
                        i_b[(1, 1)],
                        i_b[(2, 2)],
                        i_b[(0, 1)],
                        i_b[(0, 2)],
                        i_b[(1, 2)],
                    ];
                }
            }
            let mass = match authored {
                Some(props) => props.mass_kg,
                None => {
                    let density = doc
                        .materials
                        .get(inst.material.as_deref().unwrap_or("default"))
                        .and_then(|m| m.density)
                        .unwrap_or(1000.0);
                    estimate_mass(&mesh, density)
                }
            };
            let geometry = mesh_to_collider(&mesh, collider_strategy, &inst.id)?;
            Ok((mesh, mass, geometry, authored))
        };

        // Build a SpatialInertia, preferring authored mass/inertia/COM
        // (e.g. straight from URDF `<inertial>`) over a mesh-derived
        // estimate. Without authored data we fall back to a box inertia
        // computed from the mesh bounding box, with the COM placed at the
        // bbox center — without this offset, RNEA / ABA see a body whose
        // mass acts at the joint origin and gravity exerts no moment.
        let build_inertia = |mesh: &vcad_kernel_tessellate::TriangleMesh,
                             mass: f64,
                             authored: Option<InertialProperties>|
         -> SpatialInertia {
            if let Some(props) = authored {
                // Authored COM is in mm; phyz uses metres.
                let com = Vec3::new(
                    props.com_mm.x / 1000.0,
                    props.com_mm.y / 1000.0,
                    props.com_mm.z / 1000.0,
                );
                let [ixx, iyy, izz, ixy, ixz, iyz] = props.inertia_kg_m2;
                let inertia_mat = Mat3::new(ixx, ixy, ixz, ixy, iyy, iyz, ixz, iyz, izz);
                return SpatialInertia::new(props.mass_kg, com, inertia_mat);
            }
            let mut min = Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
            let mut max = Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
            for v in mesh.vertices.chunks(3) {
                let x = v[0] as f64 / 1000.0;
                let y = v[1] as f64 / 1000.0;
                let z = v[2] as f64 / 1000.0;
                min.x = min.x.min(x);
                min.y = min.y.min(y);
                min.z = min.z.min(z);
                max.x = max.x.max(x);
                max.y = max.y.max(y);
                max.z = max.z.max(z);
            }
            let dx = max.x - min.x;
            let dy = max.y - min.y;
            let dz = max.z - min.z;
            let com = Vec3::new(
                0.5 * (min.x + max.x),
                0.5 * (min.y + max.y),
                0.5 * (min.z + max.z),
            );
            let ixx = mass / 12.0 * (dy * dy + dz * dz);
            let iyy = mass / 12.0 * (dx * dx + dz * dz);
            let izz = mass / 12.0 * (dx * dx + dy * dy);
            let inertia_mat = Mat3::new(ixx, 0.0, 0.0, 0.0, iyy, 0.0, 0.0, 0.0, izz);
            SpatialInertia::new(mass, com, inertia_mat)
        };

        // 1. Add ground body (fixed, attached to world)
        let ground_inst = instances
            .iter()
            .find(|i| i.id == *ground_id)
            .ok_or_else(|| PhysicsError::MissingInstance(ground_id.clone()))?;
        {
            let (mesh, mass, geometry, authored) = eval_instance(ground_inst, None)?;
            body_part_frames.push((Mat3::identity(), Vec3::zero()));
            let inertia = build_inertia(&mesh, mass, authored);
            let xform = instance_transform(ground_inst);
            builder = builder.add_fixed_body(&ground_inst.id, -1, xform, inertia);
            instance_to_body.insert(ground_inst.id.clone(), body_count);
            body_geometries.push(Some(geometry));
            body_count += 1;
        }

        // 2. Add joint-connected instances in topological order
        // Simple BFS from ground through joints
        let mut queue: Vec<String> = vec![ground_id.clone()];
        let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
        visited.insert(ground_id.clone());

        while let Some(parent_id) = queue.pop() {
            // Find all joints where this instance is the parent
            for joint in joints {
                let joint_parent = joint.parent_instance_id.as_deref().unwrap_or(ground_id);
                if joint_parent != parent_id || visited.contains(&joint.child_instance_id) {
                    continue;
                }

                let child_inst = instances
                    .iter()
                    .find(|i| i.id == joint.child_instance_id)
                    .ok_or_else(|| {
                        PhysicsError::MissingInstance(joint.child_instance_id.clone())
                    })?;

                let parent_body_idx = *instance_to_body
                    .get(&parent_id)
                    .ok_or_else(|| PhysicsError::MissingInstance(parent_id.clone()))?;

                // Body frame = joint frame: rotated so the motion axis is Z,
                // origin at the child anchor.
                let r_part_to_body = crate::joints::joint_frame_rotation(&joint.kind).transpose();
                let (mesh, mass, geometry, authored) =
                    eval_instance(child_inst, Some((&r_part_to_body, &joint.child_anchor)))?;
                let anchor_m = Vec3::new(
                    joint.child_anchor.x / 1000.0,
                    joint.child_anchor.y / 1000.0,
                    joint.child_anchor.z / 1000.0,
                );
                body_part_frames.push((r_part_to_body, r_part_to_body.mul_vec(-anchor_m)));
                let inertia = build_inertia(&mesh, mass, authored);

                // Create phyz joint. `parent_to_joint` is measured from the
                // parent *body* frame, which for a jointed parent is itself
                // rotated by that joint's axis alignment — pass it in so the
                // rotation does not accumulate down the chain.
                let (r_parent, t_parent) = body_part_frames[parent_body_idx];
                let phyz_joint = vcad_joint_to_phyz(joint, (&r_parent, &t_parent))?;

                builder =
                    builder.add_body(&child_inst.id, parent_body_idx as i32, phyz_joint, inertia);

                // Store geometry on the body
                instance_to_body.insert(child_inst.id.clone(), body_count);
                body_geometries.push(Some(geometry));

                // Track joint mapping
                joint_to_index.insert(joint.id.clone(), body_count);
                joint_kinds.insert(joint.id.clone(), joint.kind.clone());

                body_count += 1;
                visited.insert(child_inst.id.clone());
                queue.push(child_inst.id.clone());
            }
        }

        // Canonical joint ordering: document order, restricted to joints the
        // BFS actually realized (a joint whose child is unreachable from
        // ground, or already claimed by an earlier joint, is skipped above).
        let joint_order: Vec<String> = joints
            .iter()
            .filter(|j| joint_to_index.contains_key(&j.id))
            .map(|j| j.id.clone())
            .collect();

        // 3. Add remaining instances as free-floating bodies
        for inst in instances {
            if visited.contains(&inst.id) {
                continue;
            }

            let (mesh, mass, geometry, authored) = eval_instance(inst, None)?;
            body_part_frames.push((Mat3::identity(), Vec3::zero()));
            let inertia = build_inertia(&mesh, mass, authored);
            let xform = instance_transform(inst);

            builder = builder.add_free_body(&inst.id, -1, xform, inertia);
            instance_to_body.insert(inst.id.clone(), body_count);
            body_geometries.push(Some(geometry));
            body_count += 1;
            visited.insert(inst.id.clone());
        }

        // Build model
        let mut model = builder.build();

        // Attach geometries to model bodies
        for (i, geom) in body_geometries.into_iter().enumerate() {
            if i < model.bodies.len() {
                model.bodies[i].geometry = geom;
            }
        }

        let nbodies = model.bodies.len();

        // A body can move iff some joint between it and the world root has a
        // degree of freedom. Only movable bodies get contact geometry — the
        // fixed base (and anything welded to it) can't respond to contact
        // impulses, so testing it against the ground is pure waste.
        let mut movable = vec![false; nbodies];
        for i in 0..nbodies {
            let body = &model.bodies[i];
            let own_dof = model.joints[body.joint_idx].ndof() > 0;
            movable[i] = own_dof || (body.parent >= 0 && movable[body.parent as usize]);
        }
        let contact_geometries: Vec<Option<Geometry>> = model
            .bodies
            .iter()
            .enumerate()
            .map(|(i, b)| if movable[i] { b.geometry.clone() } else { None })
            .collect();
        let contact_support: Vec<Option<Vec<Vec3>>> = contact_geometries
            .iter()
            .map(|g| match g {
                Some(Geometry::Mesh { vertices, .. }) => {
                    Some(crate::colliders::support_points(vertices))
                }
                _ => None,
            })
            .collect();

        let has_floating_base = joint_kinds.values().any(|k| matches!(k, JointKind::Free));

        let state = model.default_state();

        // Pre-compute joint DOF offsets
        let mut joint_q_offsets = HashMap::new();
        let mut joint_v_offsets = HashMap::new();
        for (joint_id, &body_idx) in &joint_to_index {
            let joint_idx = model.bodies[body_idx].joint_idx;
            joint_q_offsets.insert(joint_id.clone(), model.q_offsets[joint_idx]);
            joint_v_offsets.insert(joint_id.clone(), model.v_offsets[joint_idx]);
        }

        let mut world = Self {
            model,
            state,
            motors: HashMap::new(),
            joint_gains: HashMap::new(),
            instance_to_body,
            joint_to_index,
            joint_order,
            joint_kinds,
            joint_q_offsets,
            joint_v_offsets,
            body_part_frames,
            body_vels: vec![phyz::math::SpatialVec::zero(); nbodies],
            gain_scale: 1.0,
            ground: GroundConfig::disabled(),
            contact_geometries,
            contact_support,
            has_floating_base,
            body_contacts: vec![ContactState::default(); nbodies],
        };

        // Seed the initial configuration from the authored joint states.
        //
        // This writes `q` only — it must NOT install a motor. `joint.state` is
        // the pose the assembly starts in, not a target to be held: calling
        // `set_joint_position` here left a PD servo latched onto every joint
        // authored at a nonzero angle, so a "passive" rollout was really a
        // servo fighting gravity and no unactuated joint ever swung freely.
        for joint in joints {
            // The scalar `state` is meaningless for a 6-DOF Free joint — its
            // pose lives entirely in the physics q, and writing a single q
            // slot from it would corrupt that pose — so skip it here.
            if matches!(joint.kind, JointKind::Free) {
                continue;
            }
            if joint_ndof(&joint.kind) == 0 || joint.state.abs() <= 1e-6 {
                continue;
            }
            if let Some(&q_offset) = world.joint_q_offsets.get(&joint.id) {
                world.state.q[q_offset] = convert_state_to_physics(&joint.kind, joint.state);
            }
        }

        // Run initial FK
        world.refresh_kinematics();

        Ok(world)
    }

    /// The underlying phyz model, as built from the document — authored
    /// inertials, joint frames, limits, the lot.
    ///
    /// This is the seam batched backends build on: clone it into a
    /// `phyz_gpu::GpuBatchSimulator` or a `phyz_env::BatchEnv` and every
    /// environment inherits exactly the physics this world runs, instead of a
    /// re-derived approximation. (An earlier GPU pipeline rebuilt the model
    /// from the document with density-guessed box inertias; keeping a single
    /// builder is the fix.)
    pub fn model(&self) -> &Model {
        &self.model
    }

    /// The current phyz state (q, v) — pair of [`Self::model`] for seeding a
    /// batched backend with this world's exact initial conditions.
    pub fn phyz_state(&self) -> &State {
        &self.state
    }

    /// A joint's (q offset, v offset, dof count) in the flat phyz state, plus
    /// its authored effort limit (N·m or N) when it has one.
    ///
    /// This is the addressing a batched backend needs to servo the same
    /// joints this world servos — a GPU PD pass indexes `q`/`v` directly and
    /// cannot go through vcad joint ids.
    pub fn joint_addressing(&self, joint_id: &str) -> Option<(usize, usize, usize, Option<f64>)> {
        let q = *self.joint_q_offsets.get(joint_id)?;
        let v = *self.joint_v_offsets.get(joint_id)?;
        let ndof = self.joint_dof_count(joint_id);
        let effort = self
            .joint_kinds
            .get(joint_id)
            .and_then(Self::joint_effort_limit);
        Some((q, v, ndof, effort))
    }

    /// Configure the ground plane. Takes effect on the next [`Self::step`].
    pub fn set_ground(&mut self, ground: GroundConfig) {
        self.ground = ground;
    }

    /// Current ground-plane configuration.
    pub fn ground(&self) -> GroundConfig {
        self.ground
    }

    /// Step the physics simulation by dt seconds.
    ///
    /// With the ground enabled ([`Self::set_ground`]) the step is
    /// FK → ground contact detection → ABA → convex contact solve at the
    /// velocity level → joint-aware semi-implicit Euler, mirroring phyz's
    /// `Simulator::step_with_contacts`. The velocity-level impulse solve
    /// (rather than a penalty force) keeps the explicit integrator stable at
    /// gym timesteps: contact can only remove approach velocity, never
    /// inject energy.
    pub fn step(&mut self, dt: f32) {
        // Temporarily set the model timestep
        let original_dt = self.model.dt;
        self.model.dt = dt as f64;

        // Apply PD motor torques to state.ctrl
        self.apply_motor_torques();

        {
            let dt = self.model.dt;
            let nv = self.state.v.len();

            // Contact detection reads world-frame body transforms — refresh
            // them so contacts are found where the bodies are *now*, not
            // where the last caller left body_xform.
            let (xforms, _) = forward_kinematics(&self.model, &self.state);
            self.state.body_xform = xforms;

            // Free velocity: where the system lands after this step with
            // every force except contact (ABA reads state.ctrl internally).
            let qdd = aba_with_external_forces(&self.model, &self.state, None);
            let free_v = &self.state.v + &(&qdd * dt);

            let contacts = if self.ground.enabled {
                self.ground_contacts()
            } else {
                Vec::new()
            };

            // Contact sensing is per-step, not cumulative: last step's
            // manifold says nothing about this one.
            for c in &mut self.body_contacts {
                *c = ContactState::default();
            }

            if contacts.is_empty() {
                self.state.v = free_v;
            } else {
                // Contact solve: all contacts coupled through the Delassus
                // operator, Coulomb friction disc with stiction.
                let material = ContactMaterial {
                    friction: self.ground.friction,
                    restitution: self.ground.restitution,
                    ..ContactMaterial::default()
                };
                let config = ContactSolverConfig::simulation();
                // Position stabilization is phyz's job now: `assemble` takes
                // `dt` and applies the MuJoCo-style solref bias from each
                // contact material (ContactMaterial::default() → timeconst
                // 0.02, dampratio 1.0). That replaces the capped Baumgarte
                // push vcad used to apply here by hand — keeping both would
                // stack two recovery biases on the same penetration.
                let asm = phyz::contact::assemble(
                    &self.model,
                    &self.state,
                    &contacts,
                    &[material],
                    &free_v,
                    dt,
                    &config,
                );
                let impulses = solve_contacts_pgs(&asm.problem);

                // Publish the per-body manifold as a foot-force sensor. The
                // solve returns impulses in each contact's local frame with
                // the normal first, so force = impulse_n / dt, and the center
                // of pressure is the impulse-weighted point centroid.
                for (ci, c) in contacts.iter().enumerate() {
                    let s = &mut self.body_contacts[c.body_i];
                    let fn_ = impulses[ci].x.max(0.0) / dt;
                    let p = c.contact_point;
                    if !s.in_contact {
                        *s = ContactState {
                            in_contact: true,
                            normal_force: 0.0,
                            point: [0.0; 3],
                        };
                    }
                    s.normal_force += fn_;
                    // Accumulate weighted point; normalized below.
                    s.point[0] += fn_ * p.x;
                    s.point[1] += fn_ * p.y;
                    s.point[2] += fn_ * p.z;
                }
                for (bi, s) in self.body_contacts.iter_mut().enumerate() {
                    if !s.in_contact {
                        continue;
                    }
                    if s.normal_force > 1e-12 {
                        for v in &mut s.point {
                            *v /= s.normal_force;
                        }
                    } else {
                        // Grazing touch: no impulse to weight by, so report
                        // the plain centroid of this body's manifold.
                        let pts: Vec<&Collision> =
                            contacts.iter().filter(|c| c.body_i == bi).collect();
                        let n = pts.len().max(1) as f64;
                        let mut acc = Vec3::new(0.0, 0.0, 0.0);
                        for c in pts {
                            acc += c.contact_point;
                        }
                        s.point = [acc.x / n, acc.y / n, acc.z / n];
                    }
                }

                // v' = v_free + M⁻¹ Jᵀ f.
                self.state.v = &free_v + &asm.velocity_delta(&impulses);
            }

            // Velocity is already updated; integrate positions only. Do NOT
            // hand-roll `q += v·dt` here: q and v use different
            // parameterisations for ball joints (exp-coords vs body angular
            // velocity — composition, not addition) and for Free
            // (floating-base) joints (q is [pos, rot] while v is [angular,
            // linear], so a flat elementwise add integrates angular velocity
            // into *position*). phyz's `semi_implicit_euler` is the single
            // place that knows the mapping.
            let zero_qdd = vec![0.0; nv];
            phyz::rigid::semi_implicit_euler(&self.model, &mut self.state, &zero_qdd, dt);

            self.enforce_joint_limits();

            // Publish the new body poses. Dropping this result left
            // `state.body_xform` pinned at the construction-time pose, so
            // `get_instance_pose` (and every `end_effector_poses` built on
            // it) reported the rest configuration no matter how far the
            // joints had moved. This also caches the spatial velocities that
            // base-velocity observations read.
            self.refresh_kinematics();
        }

        self.model.dt = original_dt;
    }

    /// Adopt an externally produced state — a GPU batch readback — and
    /// recompute the derived kinematics, so every pose and velocity query
    /// afterwards answers about *that* state.
    ///
    /// This exists so the GPU path never re-derives a conversion. Base pose,
    /// base velocity, joint units and end-effector poses all have exact
    /// definitions here (world-frame rotation of a body-frame spatial
    /// velocity, angular-first free-joint slots, degrees and millimetres);
    /// reimplementing any of them against a raw `q`/`v` buffer is how the two
    /// backends end up describing different robots. Load the state, then ask
    /// the same questions.
    ///
    /// Contacts are **not** part of `State`, so they are unchanged by this
    /// call — a decoder that has never stepped reports no contact.
    pub fn load_phyz_state(&mut self, state: &State) -> Result<(), PhysicsError> {
        if state.q.len() != self.state.q.len() || state.v.len() != self.state.v.len() {
            return Err(PhysicsError::Evaluation(format!(
                "state has {} q / {} v, this model has {} q / {} v — decoding it \
                 would silently read another robot's DOFs",
                state.q.len(),
                state.v.len(),
                self.state.q.len(),
                self.state.v.len()
            )));
        }
        self.state
            .q
            .as_mut_slice()
            .copy_from_slice(state.q.as_slice());
        self.state
            .v
            .as_mut_slice()
            .copy_from_slice(state.v.as_slice());
        self.refresh_kinematics();
        Ok(())
    }

    /// Recompute forward kinematics from the current `state.q` / `state.v`,
    /// refreshing the cached body transforms and spatial velocities. Call
    /// after mutating joint state directly (e.g. [`Self::perturb_joint_state`]).
    pub fn refresh_kinematics(&mut self) {
        let (xforms, vels) = forward_kinematics(&self.model, &self.state);
        self.state.body_xform = xforms;
        self.body_vels = vels;
    }

    /// Contacts between movable bodies' collision geometry and the ground
    /// plane at `z = self.ground.height`.
    ///
    /// This intentionally does not use `phyz::find_ground_contacts`: that
    /// routine multiplies vertices by `body_xform.rot`, but `body_xform` is
    /// the world→body Plücker rotation `E` (`p_body = E (p − r)`), so
    /// body→world needs `Eᵀ` — for any body whose frame is rotated (every
    /// vcad joint whose axis isn't already Z) it places the candidates
    /// wrongly and a swinging link passes straight through the floor. It
    /// also truncates the manifold by depth *before* deduplicating, and
    /// vcad's tessellated colliders duplicate each corner vertex per
    /// incident face, so a flat box impact could spend the whole manifold
    /// budget on copies of a single corner and lose its support polygon.
    fn ground_contacts(&self) -> Vec<Collision> {
        // Same manifold cap as phyz_collision::MAX_MANIFOLD_POINTS.
        const MAX_POINTS: usize = 4;
        let h = self.ground.height;
        let mut contacts = Vec::new();

        for (i, geom) in self.contact_geometries.iter().enumerate() {
            let Some(geom) = geom else { continue };
            let xform = &self.state.body_xform[i];
            let (pos, e_t) = (xform.pos, xform.rot.transpose());
            if !(pos.x.is_finite() && pos.y.is_finite() && pos.z.is_finite()) {
                continue;
            }

            // World-frame candidate support points.
            let candidates: Vec<Vec3> = match geom {
                Geometry::Mesh { vertices, .. } => {
                    // Precomputed support points when available (every mesh
                    // collider built here); the raw vertex cloud otherwise.
                    let src = self.contact_support[i].as_deref().unwrap_or(vertices);
                    src.iter().map(|v| e_t.mul_vec(*v) + pos).collect()
                }
                Geometry::Box { half_extents } => {
                    let he = half_extents;
                    let mut v = Vec::with_capacity(8);
                    for sx in [-1.0, 1.0] {
                        for sy in [-1.0, 1.0] {
                            for sz in [-1.0, 1.0] {
                                v.push(
                                    e_t.mul_vec(Vec3::new(sx * he.x, sy * he.y, sz * he.z)) + pos,
                                );
                            }
                        }
                    }
                    v
                }
                Geometry::Sphere { radius } => {
                    vec![pos - Vec3::new(0.0, 0.0, *radius)]
                }
                // Colliders built by this crate are Mesh or Box; anything
                // else has no ground support here yet.
                _ => continue,
            };

            // Penetrating points, deduplicated (tessellated meshes repeat
            // each corner per incident face), deepest first, capped.
            let mut hits: Vec<(f64, Vec3)> = Vec::new();
            'cand: for p in candidates {
                if !p.z.is_finite() || p.z >= h {
                    continue;
                }
                for (_, q) in &hits {
                    if (p - *q).norm() < 1e-9 {
                        continue 'cand;
                    }
                }
                hits.push((h - p.z, p));
            }
            hits.sort_by(|a, b| b.0.total_cmp(&a.0));
            hits.truncate(MAX_POINTS);

            for (depth, p) in hits {
                contacts.push(Collision {
                    body_i: i,
                    body_j: usize::MAX, // ground is not a body
                    // Midsurface between the vertex and the plane.
                    contact_point: Vec3::new(p.x, p.y, h - depth * 0.5),
                    contact_normal: Vec3::new(0.0, 0.0, 1.0),
                    penetration_depth: depth,
                });
            }
        }

        contacts
    }

    /// The currently installed motor target for a joint, if any (tests).
    #[cfg(test)]
    pub(crate) fn motor(&self, joint_id: &str) -> Option<&MotorTarget> {
        self.motors.get(joint_id)
    }

    /// Effort limit (N·m or N) authored on a joint kind, if any.
    fn effort_limit_of(&self, joint_id: &str) -> Option<f64> {
        self.joint_kinds
            .get(joint_id)
            .and_then(Self::joint_effort_limit)
    }

    /// Clamp single-DOF joints to their limits after integration.
    ///
    /// phyz carries `Joint::limits` but its integrators never read them —
    /// without this, an unactuated slider free-falls through its stops
    /// forever (a mm-scale piston ends up hundreds of meters below the
    /// floor within a minute of sim time). Hard clamp + zero the DOF
    /// velocity at the stop: inelastic, but stable at any scale.
    fn enforce_joint_limits(&mut self) {
        for joint_id in &self.joint_order {
            let Some(&q_offset) = self.joint_q_offsets.get(joint_id) else {
                continue;
            };
            let Some(&v_offset) = self.joint_v_offsets.get(joint_id) else {
                continue;
            };
            let Some(&body_idx) = self.joint_to_index.get(joint_id) else {
                continue;
            };
            let joint_idx = self.model.bodies[body_idx].joint_idx;
            let Some([lo, hi]) = self.model.joints[joint_idx].limits else {
                continue;
            };
            let q = self.state.q[q_offset];
            if q < lo {
                self.state.q[q_offset] = lo;
                if self.state.v[v_offset] < 0.0 {
                    self.state.v[v_offset] = 0.0;
                }
            } else if q > hi {
                self.state.q[q_offset] = hi;
                if self.state.v[v_offset] > 0.0 {
                    self.state.v[v_offset] = 0.0;
                }
            }
        }
    }

    /// Actuator effort limit (N·m for revolute, N for prismatic) authored on a
    /// joint kind, if any. Already in physics units.
    fn joint_effort_limit(kind: &JointKind) -> Option<f64> {
        match kind {
            JointKind::Revolute { effort_limit, .. } | JointKind::Slider { effort_limit, .. } => {
                *effort_limit
            }
            _ => None,
        }
    }

    /// Actuator velocity limit converted to physics units (rad/s for revolute,
    /// m/s for prismatic). The IR stores it in vcad units (deg/s / mm/s).
    fn joint_velocity_limit_physics(kind: &JointKind) -> Option<f64> {
        match kind {
            JointKind::Revolute { velocity_limit, .. } => velocity_limit.map(f64::to_radians),
            JointKind::Slider { velocity_limit, .. } => velocity_limit.map(|v| v / 1000.0),
            _ => None,
        }
    }

    /// Apply PD motor torques from motor targets to state.ctrl.
    ///
    /// Position/Velocity motors get gravity-compensation feedforward: one
    /// RNEA pass at the current configuration with `v = qdd = 0` yields the
    /// static holding torque per DOF, which is added inside the motor's
    /// clamp. Without it the pure PD servo carries a `τ_g / kp` steady-state
    /// droop — tens of degrees for a hanging link at the default gains.
    fn apply_motor_torques(&mut self) {
        // Zero out ctrl first
        for i in 0..self.state.ctrl.len() {
            self.state.ctrl[i] = 0.0;
        }
        if self.motors.is_empty() {
            return;
        }

        // Gravity feedforward is only meaningful on a *fixed*-base model.
        // `rnea` with `qdd = 0` solves for the torques that hold the robot
        // static assuming the root is bolted to the world; on a floating
        // base those torques also carry the 6-DOF base wrench the ground is
        // supposed to supply. Feeding them to the joints of a robot that is
        // actually free (or airborne) injects that wrench as internal
        // torque, and the base tumbles — a K1 held at its rest pose spun to
        // 90° of tilt in 0.22 s. Real floating-base controllers (MuJoCo /
        // Isaac PD actuators, on which every published locomotion policy is
        // trained) run plain PD for this reason.
        let needs_gravity_comp = !self.has_floating_base
            && self
                .motors
                .values()
                .any(|m| !matches!(m.mode, MotorMode::Torque));
        let tau_g = if needs_gravity_comp {
            let saved_v = self.state.v.clone();
            for i in 0..self.state.v.len() {
                self.state.v[i] = 0.0;
            }
            let qdd = phyz::math::DVec::zeros(self.state.v.len());
            let tau = phyz::rnea(&self.model, &self.state, &qdd);
            self.state.v = saved_v;
            Some(tau)
        } else {
            None
        };

        for (joint_id, motor) in &self.motors {
            if let (Some(&q_offset), Some(&v_offset)) = (
                self.joint_q_offsets.get(joint_id),
                self.joint_v_offsets.get(joint_id),
            ) {
                let position = self.state.q[q_offset];
                let velocity = self.state.v[v_offset];
                let ff = tau_g.as_ref().map_or(0.0, |t| t[v_offset]);
                let mut torque = motor.compute_torque_with_feedforward(position, velocity, ff);
                // Actuator effort saturation: no control mode (direct torque
                // included) can exceed the joint's authored effort limit.
                // Applied last, after gravity feedforward — the actuator has
                // to produce the whole commanded torque, feedforward and all,
                // so the ceiling binds the total rather than just the PD part.
                if let Some(effort) = self.effort_limit_of(joint_id) {
                    torque = torque.clamp(-effort, effort);
                }
                self.state.ctrl[v_offset] = torque;
            }
        }
    }

    /// Set explicit PD gains for a joint, overriding the inertia-scaled
    /// defaults for position and velocity servos. Gains are in physics units
    /// (N·m/rad and N·m·s/rad for revolute; N/m and N·s/m for prismatic).
    ///
    /// Gains published for Isaac-style simulators assume an *implicit*
    /// actuator integration and can be far outside this crate's explicit
    /// stability region — check them with [`Self::check_gain_stability`]
    /// once the timestep is known.
    pub fn set_joint_gains(&mut self, joint_id: &str, kp: f64, kd: f64) {
        self.joint_gains.insert(joint_id.to_string(), (kp, kd));
    }

    /// Explicit-integrator stability check for the currently-set PD gains.
    ///
    /// This crate integrates explicitly (`phyz::rigid::semi_implicit_euler`),
    /// so a PD servo is only stable while the closed-loop natural frequency
    /// ω = √(kp / I_reflected) is small against the substep: ω·dt must stay
    /// below ~[`GAIN_STABILITY_LIMIT`]. Isaac and MuJoCo integrate their
    /// actuators implicitly and have no such bound, so gains copied from a
    /// published config (booster_gym's K1 ships kp = 200 on 1e-3 kg·m²
    /// joints) can diverge here within a few control steps — the robot tears
    /// itself apart while still airborne, which reads exactly like a contact
    /// solver bug.
    ///
    /// Returns one [`GainWarning`] per offending joint. Nothing is clamped:
    /// the caller may be about to raise `substeps` instead, which is the fix
    /// that keeps the published gains intact.
    pub fn check_gain_stability(&mut self, dt: f64) -> Vec<GainWarning> {
        if !(dt.is_finite() && dt > 0.0) {
            return Vec::new();
        }
        let joint_ids: Vec<String> = self.joint_gains.keys().cloned().collect();
        let mut warnings = Vec::new();
        for joint_id in joint_ids {
            let Some(&(kp, _kd)) = self.joint_gains.get(&joint_id) else {
                continue;
            };
            let kp = kp * self.gain_scale;
            if !(kp.is_finite() && kp > 0.0) {
                continue;
            }
            let inertia = self.reflected_inertia(&joint_id);
            let omega = (kp / inertia).sqrt();
            let omega_dt = omega * dt;
            if omega_dt <= GAIN_STABILITY_LIMIT {
                continue;
            }
            // Largest kp that lands exactly on the limit at this dt.
            let max_kp = inertia * (GAIN_STABILITY_LIMIT / dt).powi(2) / self.gain_scale.max(1e-12);
            // Substep multiplier needed to bring ω·dt back under the limit.
            let substep_factor = (omega_dt / GAIN_STABILITY_LIMIT).ceil() as u32;
            warnings.push(GainWarning {
                joint_id,
                kp,
                reflected_inertia: inertia,
                omega_dt,
                max_stable_kp: max_kp,
                min_substeps: substep_factor,
            });
        }
        warnings.sort_by(|a, b| {
            b.omega_dt
                .partial_cmp(&a.omega_dt)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        warnings
    }

    /// Get the current state of all joints.
    pub fn get_joint_states(&self) -> HashMap<String, JointState> {
        let mut states = HashMap::new();

        for (joint_id, &_body_idx) in &self.joint_to_index {
            let kind = self.joint_kinds.get(joint_id).unwrap();

            if let (Some(&q_offset), Some(&v_offset)) = (
                self.joint_q_offsets.get(joint_id),
                self.joint_v_offsets.get(joint_id),
            ) {
                let ndof = joint_ndof(kind);
                if ndof == 0 {
                    states.insert(joint_id.clone(), JointState::default());
                    continue;
                }

                // Scalar summary: the first DOF only. For multi-DOF joints
                // (Ball, Free) use `get_joint_dofs` for the full layout.
                let position = self.state.q[q_offset];
                let velocity = self.state.v[v_offset];
                let effort = self.state.ctrl[v_offset];

                states.insert(
                    joint_id.clone(),
                    JointState {
                        position: crate::joints::convert_q_dof_from_physics(kind, 0, position),
                        velocity: crate::joints::convert_v_dof_from_physics(kind, 0, velocity),
                        effort,
                    },
                );
            }
        }

        states
    }

    /// Set the target position for a joint.
    ///
    /// # Arguments
    ///
    /// * `joint_id` - The vcad joint ID
    /// * `target` - Target position (degrees for revolute, mm for prismatic)
    pub fn set_joint_position(&mut self, joint_id: &str, target: f64) {
        if let Some(kind) = self.joint_kinds.get(joint_id) {
            // Free joints are passive floating bases — no motor to drive.
            if joint_ndof(kind) == 0 || matches!(kind, JointKind::Free) {
                return;
            }
            let physics_target = convert_state_to_physics(kind, target);
            let (kp, kd, max_force) = self.position_gains(joint_id);
            self.motors.insert(
                joint_id.to_string(),
                MotorTarget {
                    mode: MotorMode::Position,
                    target: physics_target,
                    kp,
                    kd,
                    max_force,
                },
            );
        }
    }

    /// Critically-damped PD gains scaled to the joint's reflected inertia.
    ///
    /// The old fixed defaults (`kp = 1000 Nm/rad`, clamp `±1000 Nm`) are
    /// tuned for meter/kilogram robots; a mm-scale part has a reflected
    /// inertia around 1e-8 kg·m², so a saturated 1000 Nm torque produced
    /// ~1e11 rad/s² and the explicit integrator diverged in one substep.
    /// Scaling by the measured inertia keeps the closed-loop natural
    /// frequency fixed (ω = 20 rad/s, ζ = 1) at every scale.
    fn position_gains(&mut self, joint_id: &str) -> (f64, f64, f64) {
        if let Some(&(kp, kd)) = self.joint_gains.get(joint_id) {
            // Explicit gains still ride domain randomization's gain_scale —
            // randomizing actuator gains is meaningless if authoring them
            // opts out of it.
            let (kp, kd) = (kp * self.gain_scale, kd * self.gain_scale);
            // Bound the clamp by the effort limit when the joint has one,
            // else by the full-scale error torque.
            let max_force = self
                .effort_limit_of(joint_id)
                .unwrap_or((kp * std::f64::consts::PI).max(1e-12))
                .max(1e-12);
            return (kp, kd, max_force);
        }
        const OMEGA: f64 = 20.0;
        let i = self.reflected_inertia(joint_id);
        let kp = i * OMEGA * OMEGA * self.gain_scale;
        let kd = 2.0 * i * OMEGA * self.gain_scale;
        // Full-scale (π rad / 1 m) error torque bounds the clamp.
        let max_force = (kp * std::f64::consts::PI).max(1e-12);
        (kp, kd, max_force)
    }

    /// Reflected inertia (kg·m² or kg) of a joint's DOF, measured by probing
    /// forward dynamics: apply a unit generalized force and read the change
    /// in acceleration. Falls back to the old meter-scale assumption (1.0)
    /// when the probe degenerates.
    fn reflected_inertia(&mut self, joint_id: &str) -> f64 {
        let Some(&v_offset) = self.joint_v_offsets.get(joint_id) else {
            return 1.0;
        };
        // A zero-DOF (Fixed) joint still has an entry in `joint_v_offsets` —
        // pointing at where its DOFs *would* start, which for the last joint in
        // the model is one past the end of `ctrl`. Probing it panicked with an
        // out-of-bounds index. A joint with no DOF has no reflected inertia to
        // measure and no motor to tune, so the neutral fallback is the answer.
        if v_offset >= self.state.ctrl.len() {
            return 1.0;
        }
        let saved_ctrl = self.state.ctrl.clone();
        for c in self.state.ctrl.as_mut_slice() {
            *c = 0.0;
        }
        let qdd0 = aba_with_external_forces(&self.model, &self.state, None);
        self.state.ctrl[v_offset] = 1.0;
        let qdd1 = aba_with_external_forces(&self.model, &self.state, None);
        self.state.ctrl = saved_ctrl;

        let delta = qdd1[v_offset] - qdd0[v_offset];
        if !delta.is_finite() || delta.abs() < 1e-12 {
            return 1.0;
        }
        (1.0 / delta).abs().clamp(1e-12, 1e9)
    }

    /// Set the target velocity for a joint.
    ///
    /// # Arguments
    ///
    /// * `joint_id` - The vcad joint ID
    /// * `target` - Target velocity (deg/s for revolute, mm/s for prismatic)
    pub fn set_joint_velocity(&mut self, joint_id: &str, target: f64) {
        if let Some(kind) = self.joint_kinds.get(joint_id) {
            // Free joints are passive floating bases — no motor to drive.
            if joint_ndof(kind) == 0 || matches!(kind, JointKind::Free) {
                return;
            }
            let mut physics_target = convert_state_to_physics(kind, target);
            // Actuator velocity saturation: the servo can only chase a target
            // inside the joint's authored velocity limit.
            if let Some(vmax) = Self::joint_velocity_limit_physics(kind) {
                physics_target = physics_target.clamp(-vmax, vmax);
            }
            // Velocity servo: τ = kd (v* − v). Track within ~1/ω seconds and
            // clamp at the torque needed to reach the target from rest in one
            // time constant, scaled to the joint's reflected inertia.
            const OMEGA: f64 = 40.0;
            // Explicit per-joint gain when set, else inertia-scaled. Domain
            // randomization's gain_scale multiplies either one: randomizing
            // actuator gains is meaningless if authoring them opts out of it.
            let kd = match self.joint_gains.get(joint_id) {
                Some(&(_, kd)) => kd,
                None => self.reflected_inertia(joint_id) * OMEGA,
            } * self.gain_scale;
            let max_force = (kd * physics_target.abs().max(1.0) * 2.0).max(1e-12);
            self.motors.insert(
                joint_id.to_string(),
                MotorTarget {
                    mode: MotorMode::Velocity,
                    target: physics_target,
                    kp: 0.0,
                    kd,
                    max_force,
                },
            );
        }
    }

    /// Apply torque/force to a joint.
    ///
    /// # Arguments
    ///
    /// * `joint_id` - The vcad joint ID
    /// * `torque` - Torque/force (Nm for revolute, N for prismatic)
    pub fn apply_joint_torque(&mut self, joint_id: &str, torque: f64) {
        if self
            .joint_kinds
            .get(joint_id)
            .is_none_or(|kind| joint_ndof(kind) == 0 || matches!(kind, JointKind::Free))
        {
            return;
        }
        self.motors.insert(
            joint_id.to_string(),
            MotorTarget {
                mode: MotorMode::Torque,
                target: torque,
                max_force: torque.abs().max(1.0),
                ..MotorTarget::default()
            },
        );
    }

    /// Get the pose of an instance in world coordinates.
    ///
    /// Returns (position, orientation) where position is in meters and
    /// orientation is a unit quaternion [w, x, y, z].
    pub fn get_instance_pose(&self, instance_id: &str) -> Option<([f64; 3], [f64; 4])> {
        let &body_idx = self.instance_to_body.get(instance_id)?;
        Some(self.part_pose(body_idx))
    }

    /// Ground-contact state of an instance as of the most recent
    /// [`Self::step`] — `None` for an unknown instance id.
    ///
    /// A body whose collider never reaches the ground (or any body at all,
    /// with the ground disabled) reports the default "not in contact, zero
    /// force" state.
    pub fn get_instance_contact(&self, instance_id: &str) -> Option<ContactState> {
        let &body_idx = self.instance_to_body.get(instance_id)?;
        self.body_contacts.get(body_idx).copied()
    }

    /// World pose of the PART frame for a body: composes the body-in-world
    /// pose from FK with the stored part→body frame, so callers see the
    /// part's local origin/axes (the thing renderers pose), not the internal
    /// joint frame.
    fn part_pose(&self, body_idx: usize) -> Pose {
        let xform = &self.state.body_xform[body_idx];
        // phyz body_xform is world→body in Plücker form `p_body = E (p - r)`:
        // body origin in world is `r`, body→world rotation is `Eᵀ`.
        let e_t = xform.rot.transpose();
        let (r_pb, t_pb) = &self.body_part_frames[body_idx];
        // part→world: p_w = Eᵀ (R_pb p + t_pb) + r
        let rot_world = e_t.mul_mat(r_pb);
        let origin = e_t.mul_vec(*t_pb) + xform.pos;
        let quat = Quat::from_matrix(&rot_world);
        (
            [origin.x, origin.y, origin.z],
            [quat.w, quat.v.x, quat.v.y, quat.v.z],
        )
    }

    /// Set gravity vector.
    pub fn set_gravity(&mut self, x: f32, y: f32, z: f32) {
        self.model.gravity = Vec3::new(x as f64, y as f64, z as f64);
    }

    /// An instance's body mass in kilograms — `None` for an unknown id.
    pub fn get_instance_mass(&self, instance_id: &str) -> Option<f64> {
        let &body_idx = self.instance_to_body.get(instance_id)?;
        Some(self.model.bodies[body_idx].inertia.mass)
    }

    /// Scale an instance's mass (and rotational inertia) by `scale`.
    ///
    /// Domain-randomization seam: multiplying the whole spatial inertia by a
    /// scalar models a uniformly denser/lighter link with unchanged geometry.
    pub fn scale_instance_mass(&mut self, instance_id: &str, scale: f64) {
        let Some(&body_idx) = self.instance_to_body.get(instance_id) else {
            return;
        };
        let inertia = &mut self.model.bodies[body_idx].inertia;
        inertia.mass *= scale;
        let i = inertia.inertia;
        inertia.inertia = Mat3::new(
            i[(0, 0)] * scale,
            i[(0, 1)] * scale,
            i[(0, 2)] * scale,
            i[(1, 0)] * scale,
            i[(1, 1)] * scale,
            i[(1, 2)] * scale,
            i[(2, 0)] * scale,
            i[(2, 1)] * scale,
            i[(2, 2)] * scale,
        );
    }

    /// Scale a joint's dry-friction loss (and viscous damping) by `scale`.
    ///
    /// Both enter the dynamics through phyz's passive-force path
    /// (`Joint::passive_force` inside ABA's generalized forces).
    ///
    /// TODO(contact): once the contact ground-plane task lands, surface
    /// (foot-ground) friction should be randomized here too — this seam only
    /// covers *joint* friction because `PhysicsWorld` currently runs a
    /// contact-free articulated rollout.
    pub fn scale_joint_friction(&mut self, joint_id: &str, scale: f64) {
        let Some(&body_idx) = self.joint_to_index.get(joint_id) else {
            return;
        };
        let joint_idx = self.model.bodies[body_idx].joint_idx;
        let joint = &mut self.model.joints[joint_idx];
        joint.friction_loss *= scale;
        joint.damping *= scale;
    }

    /// Set the multiplier applied to auto-derived PD motor gains (kp, kd).
    ///
    /// Domain-randomization seam for actuator-strength / controller mismatch.
    /// Applies to motors installed *after* the call.
    pub fn set_gain_scale(&mut self, scale: f64) {
        self.gain_scale = scale;
    }

    /// The current multiplier applied to PD motor gains.
    pub fn gain_scale(&self) -> f64 {
        self.gain_scale
    }

    /// Add `dpos` / `dvel` (vcad units: degrees or mm) to a 1-DOF joint's
    /// position and velocity. No-op for Fixed joints. Call
    /// [`Self::refresh_kinematics`] after the last perturbation.
    pub fn perturb_joint_state(&mut self, joint_id: &str, dpos: f64, dvel: f64) {
        let Some(kind) = self.joint_kinds.get(joint_id) else {
            return;
        };
        if joint_ndof(kind) == 0 {
            return;
        }
        if let (Some(&q_offset), Some(&v_offset)) = (
            self.joint_q_offsets.get(joint_id),
            self.joint_v_offsets.get(joint_id),
        ) {
            // Every DOF, not just the first: a Ball joint carries 3 and a Free
            // joint 6, and perturbing only DOF 0 would silently apply a
            // fraction of the requested randomization. The per-DOF converters
            // also handle Free's angular-first q/v layouts (q = [rotation, linear],
            // v = [angular, linear]).
            for k in 0..joint_ndof(kind) {
                self.state.q[q_offset + k] += convert_q_dof_to_physics(kind, k, dpos);
                self.state.v[v_offset + k] += convert_v_dof_to_physics(kind, k, dvel);
            }
        }
    }

    /// World-frame velocity of an instance's body: `[vx, vy, vz, wx, wy, wz]`
    /// (linear m/s of the body-frame origin, then angular rad/s). Zero for
    /// fixed bodies. Reflects the last [`Self::step`] /
    /// [`Self::refresh_kinematics`] call.
    pub fn get_instance_velocity(&self, instance_id: &str) -> Option<[f64; 6]> {
        let &body_idx = self.instance_to_body.get(instance_id)?;
        let v = self.body_vels.get(body_idx)?;
        // body_xform.rot maps world → body; transpose back to world.
        let e_t = self.state.body_xform[body_idx].rot.transpose();
        let lin = e_t.mul_vec(v.linear);
        let ang = e_t.mul_vec(v.angular);
        Some([lin.x, lin.y, lin.z, ang.x, ang.y, ang.z])
    }

    /// A 1-DOF joint's limits in vcad units (degrees / mm), if any.
    pub fn joint_limits_vcad(&self, joint_id: &str) -> Option<(f64, f64)> {
        let &body_idx = self.joint_to_index.get(joint_id)?;
        let joint_idx = self.model.bodies[body_idx].joint_idx;
        let [lo, hi] = self.model.joints[joint_idx].limits?;
        let kind = self.joint_kinds.get(joint_id)?;
        Some((
            convert_state_from_physics(kind, lo),
            convert_state_from_physics(kind, hi),
        ))
    }

    /// Pose every instance in world coordinates for a given joint configuration.
    ///
    /// Computes forward kinematics at `q` (per-joint values in **vcad units**:
    /// degrees for revolute / cylindrical / ball, mm for sliders) and returns
    /// `(position_m, quat_wxyz)` per instance id. The mutation of `state.q`
    /// is rolled back before returning, so this can be called repeatedly
    /// during IK search without disturbing the simulation.
    ///
    /// `q` is interpreted positionally against [`Self::joint_ids`]: `q[i]`
    /// applies to `joint_ids()[i]`. Multi-DOF joints (Ball, Free) consume
    /// multiple consecutive entries from `q`.
    pub fn forward_kinematics_at(&mut self, q: &[f64]) -> Result<PoseMap, PhysicsError> {
        let joint_ids = self.joint_ids();
        let saved_q = self.state.q.clone();
        let saved_xform = self.state.body_xform.clone();

        // Walk joint_ids in order and write q values into state.q at the
        // right offsets. We reject the call if `q` is too short.
        let mut cursor = 0usize;
        for joint_id in &joint_ids {
            let kind = self
                .joint_kinds
                .get(joint_id)
                .ok_or_else(|| PhysicsError::MissingJoint(joint_id.clone()))?;
            let ndof = joint_ndof(kind);
            if ndof == 0 {
                continue;
            }
            if cursor + ndof > q.len() {
                self.state.q = saved_q;
                self.state.body_xform = saved_xform;
                return Err(PhysicsError::Evaluation(format!(
                    "forward_kinematics_at: q has {} entries, joints need {}",
                    q.len(),
                    cursor + ndof
                )));
            }
            let q_offset = self
                .joint_q_offsets
                .get(joint_id)
                .copied()
                .ok_or_else(|| PhysicsError::MissingJoint(joint_id.clone()))?;
            for k in 0..ndof {
                let physics_val = crate::joints::convert_q_dof_to_physics(kind, k, q[cursor + k]);
                self.state.q[q_offset + k] = physics_val;
            }
            cursor += ndof;
        }

        let (xforms, _) = forward_kinematics(&self.model, &self.state);
        self.state.body_xform = xforms;

        let mut out = HashMap::new();
        for (inst_id, &body_idx) in &self.instance_to_body {
            out.insert(inst_id.clone(), self.part_pose(body_idx));
        }

        // Restore prior state — caller wanted a kinematic probe, not a step.
        self.state.q = saved_q;
        self.state.body_xform = saved_xform;
        Ok(out)
    }

    /// Joint torques required to hold configuration `q` at rest under
    /// gravity. Uses RNEA inverse dynamics with `v = 0` and `qdd = 0`,
    /// so the result is pure gravity-comp + any spring/limit terms.
    ///
    /// Result is keyed by joint id, in **N·m** for revolute joints,
    /// **N** for sliders. For multi-DOF joints (Ball, Free) only the
    /// first DOF's torque is reported — Suite C cares about per-actuator
    /// effort and the reacher uses single-DOF joints exclusively.
    ///
    /// State is restored before returning (same contract as
    /// [`Self::forward_kinematics_at`]).
    pub fn gravity_torques_at(&mut self, q: &[f64]) -> Result<HashMap<String, f64>, PhysicsError> {
        let joint_ids = self.joint_ids();
        let saved_q = self.state.q.clone();
        let saved_v = self.state.v.clone();

        // Reuse the FK-at machinery to write q into state.q. We don't need
        // its return value here — RNEA reads state directly.
        self.forward_kinematics_at(q)?;
        // forward_kinematics_at restores state at the end; rewrite q again
        // for the inverse-dyn pass.
        let mut cursor = 0usize;
        for joint_id in &joint_ids {
            let kind = self.joint_kinds.get(joint_id).unwrap();
            let ndof = joint_ndof(kind);
            if ndof == 0 {
                continue;
            }
            let q_offset = self.joint_q_offsets[joint_id];
            for k in 0..ndof {
                self.state.q[q_offset + k] =
                    crate::joints::convert_q_dof_to_physics(kind, k, q[cursor + k]);
            }
            cursor += ndof;
        }

        // Zero out velocities so RNEA gives the static (gravity-only) torques.
        for i in 0..self.state.v.len() {
            self.state.v[i] = 0.0;
        }

        let qdd = phyz::math::DVec::zeros(self.state.v.len());
        let tau = phyz::rnea(&self.model, &self.state, &qdd);

        let mut out = HashMap::new();
        for (joint_id, &v_offset) in &self.joint_v_offsets {
            if v_offset < tau.len() {
                out.insert(joint_id.clone(), tau[v_offset]);
            }
        }

        // Restore.
        self.state.q = saved_q;
        self.state.v = saved_v;
        Ok(out)
    }

    /// Debug: `(mass_kg, perpendicular COM distance from the joint axis,
    /// I_com about the joint axis)` for a body, as handed to phyz.
    #[doc(hidden)]
    pub fn debug_body_props(&self, body_idx: usize) -> (f64, f64, f64) {
        let si = &self.model.bodies[body_idx].inertia;
        // phyz revolute joints rotate about body-frame Z.
        let d = (si.com.x * si.com.x + si.com.y * si.com.y).sqrt();
        (si.mass, d, si.inertia[(2, 2)])
    }

    /// Read a joint's velocity DOFs in **physics units** (rad/s, m/s),
    /// unconverted.
    ///
    /// The symmetric partner of [`Self::set_joint_velocity_raw`]. It exists
    /// because the natural-looking pairing — read with
    /// [`Self::get_joint_dofs`], write with `set_joint_velocity_raw` — mixes
    /// unit systems: the getter converts to vcad units (deg/s, mm/s) and the
    /// setter does not convert back. Round-tripping through that pair scales
    /// an angular velocity by 180/π and a linear one by 1000, silently.
    #[doc(hidden)]
    pub fn get_joint_velocity_raw(&self, joint_id: &str) -> Option<Vec<f64>> {
        let kind = self.joint_kinds.get(joint_id)?;
        let &v_offset = self.joint_v_offsets.get(joint_id)?;
        let ndof = joint_ndof(kind);
        Some((0..ndof).map(|k| self.state.v[v_offset + k]).collect())
    }

    /// Directly write a joint's velocity DOFs (physics units: rad/s or m/s),
    /// without installing a motor. Test/tooling hook — actions should go
    /// through the motor API.
    #[doc(hidden)]
    pub fn set_joint_velocity_raw(&mut self, joint_id: &str, v: &[f64]) {
        let Some(&v_offset) = self.joint_v_offsets.get(joint_id) else {
            return;
        };
        let Some(kind) = self.joint_kinds.get(joint_id) else {
            return;
        };
        let ndof = joint_ndof(kind).min(v.len());
        for (k, &val) in v.iter().enumerate().take(ndof) {
            self.state.v[v_offset + k] = val;
        }
    }

    /// Directly write a free-floating instance's angular velocity (rad/s,
    /// body frame). Test/tooling hook.
    #[doc(hidden)]
    pub fn set_free_body_spin_raw(&mut self, instance_id: &str, omega: [f64; 3]) {
        let Some(&body_idx) = self.instance_to_body.get(instance_id) else {
            return;
        };
        let joint_idx = self.model.bodies[body_idx].joint_idx;
        if self.model.joints[joint_idx].ndof() != 6 {
            return; // not a free body
        }
        let v_offset = self.model.v_offsets[joint_idx];
        for (k, w) in omega.iter().enumerate() {
            self.state.v[v_offset + k] = *w;
        }
    }

    /// Get list of all joint IDs.
    ///
    /// Order is deterministic: document order (`doc.joints`), restricted to
    /// joints realized in the physics model. Observation vectors
    /// ([`crate::Observation`]) and the positional `q` arguments of
    /// [`Self::forward_kinematics_at`] / [`Self::gravity_torques_at`] all
    /// index against this order.
    pub fn joint_ids(&self) -> Vec<String> {
        self.joint_order.clone()
    }

    /// Joint ids (document order) with at least one degree of freedom.
    ///
    /// Fixed joints weld their child to the parent body and contribute no
    /// actuated dof, so they are excluded here. Free joints (floating
    /// bases) are excluded too: a floating base is passive — it has no
    /// actuator, matching the URDF/MuJoCo convention.
    pub fn actuated_joint_ids(&self) -> Vec<String> {
        self.joint_order
            .iter()
            .filter(|id| {
                self.joint_kinds
                    .get(*id)
                    .is_some_and(|kind| joint_ndof(kind) > 0 && !matches!(kind, JointKind::Free))
            })
            .cloned()
            .collect()
    }

    /// Number of DOFs of a joint (0 for unknown joint ids).
    pub fn joint_dof_count(&self, joint_id: &str) -> usize {
        self.joint_kinds.get(joint_id).map_or(0, joint_ndof)
    }

    /// Per-DOF positions and velocities of a joint, in vcad units.
    ///
    /// Layouts (see [`crate::joints::convert_q_dof_from_physics`] /
    /// [`crate::joints::convert_v_dof_from_physics`]):
    /// - 1-DOF kinds: `([pos], [vel])` — degrees / deg/s or mm / mm/s
    /// - Ball: 3 rotation exp-coords in degrees; 3 angular vel in deg/s
    /// - Free: positions `[rx, ry, rz (exp-coords, deg), x, y, z (mm)]`,
    ///   velocities `[wx, wy, wz (deg/s), vx, vy, vz (body-frame mm/s)]` —
    ///   both angular-first, matching phyz's `SpatialVec` order
    /// - Fixed: `([], [])`
    pub fn get_joint_dofs(&self, joint_id: &str) -> Option<(Vec<f64>, Vec<f64>)> {
        let kind = self.joint_kinds.get(joint_id)?;
        let ndof = joint_ndof(kind);
        let &q_offset = self.joint_q_offsets.get(joint_id)?;
        let &v_offset = self.joint_v_offsets.get(joint_id)?;
        let mut positions = Vec::with_capacity(ndof);
        let mut velocities = Vec::with_capacity(ndof);
        for k in 0..ndof {
            positions.push(crate::joints::convert_q_dof_from_physics(
                kind,
                k,
                self.state.q[q_offset + k],
            ));
            velocities.push(crate::joints::convert_v_dof_from_physics(
                kind,
                k,
                self.state.v[v_offset + k],
            ));
        }
        Some((positions, velocities))
    }

    /// Get list of all instance IDs, sorted.
    ///
    /// The backing map is a `HashMap`, whose iteration order varies per
    /// process. Sorting keeps callers that consume a seeded RNG per instance
    /// (domain randomization) reproducible across runs.
    pub fn instance_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.instance_to_body.keys().cloned().collect();
        ids.sort();
        ids
    }

    /// Resolve a triangle mesh directly out of `node_id` when the node *is*
    /// a mesh — a fast path that skips wrapping the soup in a `Solid` and
    /// tessellating it straight back out.
    ///
    /// Only a bare mesh node qualifies. Anything else (including a mesh under
    /// a transform) returns `Ok(None)` and goes to the canonical evaluator,
    /// which handles mesh-backed solids properly — including flipping triangle
    /// winding when a transform has negative determinant, which this path has
    /// no way to know about.
    fn evaluate_mesh_leaf(
        doc: &Document,
        node_id: vcad_ir::NodeId,
    ) -> Result<Option<vcad_kernel_tessellate::TriangleMesh>, PhysicsError> {
        use vcad_kernel_tessellate::TriangleMesh;

        let node = doc
            .nodes
            .get(&node_id)
            .ok_or_else(|| PhysicsError::Evaluation(format!("Node {} not found", node_id)))?;

        match &node.op {
            vcad_ir::CsgOp::MeshImport { path, scale } => {
                // `.ok()` deliberately flattens a load failure — missing file,
                // unparseable STL — into "no geometry" rather than erroring.
                // A browser-flow URDF import keeps the raw URDF filename with
                // no filesystem behind it, so an unopenable reference is an
                // expected state here, not a fault.
                //
                // This is *not* a silent swallow at the level that matters:
                // the decision is deferred, not discarded. `eval_instance` is
                // where it gets made, and it fails closed — a part with no
                // resolvable geometry and no authored `<inertial>` is a hard
                // error there ("cannot derive mass properties"), because the
                // alternative is inventing the mass properties the dynamics
                // run on. Only a part that *has* authored inertials is allowed
                // to fall back to a placeholder collider.
                //
                // A future caller that needs the underlying I/O error should
                // call `crate::stl::load_stl` directly rather than reaching
                // for this helper and re-deriving the cause from `None`.
                Ok(crate::stl::load_stl(std::path::Path::new(path), *scale).ok())
            }
            // Inline ImportedMesh (e.g. browser pre-parsed STL/DAE) ships its
            // triangle data inside the IR node — pull positions / indices /
            // optional normals straight across into the physics TriangleMesh
            // (units stay millimetres).
            vcad_ir::CsgOp::ImportedMesh {
                positions,
                indices,
                normals,
                ..
            } => {
                let n_verts = positions.len() / 3;
                let vertices: Vec<f32> = positions.iter().map(|v| *v as f32).collect();
                let normals_f32: Vec<f32> = normals
                    .as_ref()
                    .map(|n| n.iter().map(|v| *v as f32).collect())
                    .unwrap_or_else(|| vec![0.0; n_verts * 3]);
                Ok(Some(TriangleMesh {
                    vertices,
                    indices: indices.clone(),
                    normals: normals_f32,
                    face_kinds: Vec::new(),
                }))
            }
            _ => Ok(None),
        }
    }

    /// Evaluate a part's geometry to get a mesh, or `None` when the tree
    /// carries no resolvable geometry (an unresolved external mesh reference,
    /// an `Empty` node). Callers decide whether that is fatal — see
    /// `eval_instance`.
    ///
    /// Uses the canonical document evaluator (`vcad_eval`), so a part built
    /// from booleans, transforms, sketches or any other non-primitive op gets
    /// its real geometry. This path used to match on primitives only and fall
    /// back to a 10 mm placeholder cube for everything else, which silently
    /// replaced every composed part with a 1 g box whose centre of mass sat
    /// *above* the joint anchor — inverting the sign of the gravitational
    /// torque and shrinking the inertia by orders of magnitude.
    fn evaluate_part(
        doc: &Document,
        node_id: vcad_ir::NodeId,
    ) -> Result<Option<vcad_kernel_tessellate::TriangleMesh>, PhysicsError> {
        // STL meshes bypass the BRep solid path — load straight to a
        // triangle mesh in the IR's millimetre frame. If the path can't
        // be opened (e.g. browser-flow URDF imports keep the raw URDF
        // filename and have no filesystem behind it), report "no geometry".
        //
        // This only catches a *bare* mesh node. A mesh under a transform —
        // what a URDF `<visual>` with an `origin` offset imports as — goes
        // through the evaluator below, which returns it as a mesh-backed
        // `Solid` with the transform applied.
        if let Some(mesh) = Self::evaluate_mesh_leaf(doc, node_id)? {
            return Ok(Some(mesh));
        }
        // Everything else goes through the canonical evaluator, which
        // understands booleans, transforms, sketches, sweeps and the rest.
        let mut cache = std::collections::HashMap::new();
        let solid = vcad_eval::evaluate_node(node_id, &doc.nodes, &mut cache)
            .map_err(|e| PhysicsError::Evaluation(format!("node {node_id}: {e}")))?;

        Ok(solid.map(|s| s.to_mesh(32)))
    }
}

/// A 1 cm cube centred on the body origin, used as a stand-in collider for a
/// link whose geometry could not be resolved. Centred deliberately: a
/// corner-at-origin cube would place the centre of mass 5 mm off the joint
/// anchor and fabricate a gravitational lever arm.
fn placeholder_collider_mesh() -> vcad_kernel_tessellate::TriangleMesh {
    let mut mesh = vcad_kernel::Solid::cube(10.0, 10.0, 10.0).to_mesh(4);
    for v in mesh.vertices.chunks_mut(3) {
        v[0] -= 5.0;
        v[1] -= 5.0;
        v[2] -= 5.0;
    }
    mesh
}

/// Compute the SpatialTransform from an instance's transform.
fn instance_transform(inst: &vcad_ir::Instance) -> SpatialTransform {
    inst.transform
        .as_ref()
        .map(|t| {
            // Convert from mm to meters
            let translation = Vec3::new(
                t.translation.x / 1000.0,
                t.translation.y / 1000.0,
                t.translation.z / 1000.0,
            );
            // Euler angles to rotation matrix
            let rx = t.rotation.x.to_radians();
            let ry = t.rotation.y.to_radians();
            let rz = t.rotation.z.to_radians();
            let rot = euler_to_mat3(rx, ry, rz);
            SpatialTransform::new(rot, translation)
        })
        .unwrap_or(SpatialTransform::identity())
}

/// Convert Euler angles (XYZ convention) to rotation matrix.
fn euler_to_mat3(rx: f64, ry: f64, rz: f64) -> Mat3 {
    let (sx, cx) = rx.sin_cos();
    let (sy, cy) = ry.sin_cos();
    let (sz, cz) = rz.sin_cos();

    Mat3::new(
        cy * cz,
        -cy * sz,
        sy,
        sx * sy * cz + cx * sz,
        -sx * sy * sz + cx * cz,
        -sx * cy,
        -cx * sy * cz + sx * sz,
        cx * sy * sz + sx * cz,
        cx * cy,
    )
}

/// Scalar projected Gauss-Seidel over an assembled contact problem.
///
/// Replaces `phyz::solve_contacts` for the gym step: phyz's staged
/// per-contact block solve drops the within-block normal↔tangential
/// coupling (its residual excludes the contact's own impulse, then solves
/// the normal from `A_nn` alone and the tangential 2×2 without the
/// `A_tn·f_n` term). For a multi-corner manifold — where a normal impulse
/// at one corner induces tangential velocity at the others through the
/// body's rotation — its fixed point satisfies the wrong equations and a
/// 4 m/s box impact bounces off the floor at 25 m/s. Verified empirically:
/// same assembly, phyz's solve diverges, this one rests.
///
/// This is the textbook sequential-impulse iteration on the PSD Delassus
/// operator: each scalar row relaxed against the *full* current residual,
/// normal impulses projected to `≥ 0`, tangential pairs clamped to the
/// isotropic friction disc `‖f_t‖ ≤ μ f_n`.
fn solve_contacts_pgs(problem: &ContactProblem) -> Vec<Vec3> {
    let n = problem.n;
    let dim = 3 * n;
    let a = &problem.delassus;
    let at = |i: usize, j: usize| a[i * dim + j];
    let mut f = vec![0.0f64; dim];

    for _ in 0..200 {
        let mut max_move: f64 = 0.0;
        for c in 0..n {
            let base = 3 * c;

            // Normal row: full residual over every impulse but its own.
            let mut r = problem.free_velocity[base];
            for (j, fj) in f.iter().enumerate() {
                if j != base {
                    r += at(base, j) * fj;
                }
            }
            let a_nn = at(base, base).max(1e-12);
            let f_n = (-r / a_nn).max(0.0);
            max_move = max_move.max((f_n - f[base]).abs());
            f[base] = f_n;

            // Tangential rows, relaxed one scalar at a time, then projected
            // onto the friction disc the fresh normal admits.
            for t in 1..3 {
                let i = base + t;
                let mut r = problem.free_velocity[i];
                for (j, fj) in f.iter().enumerate() {
                    if j != i {
                        r += at(i, j) * fj;
                    }
                }
                let a_ii = at(i, i).max(1e-12);
                let f_t = -r / a_ii;
                max_move = max_move.max((f_t - f[i]).abs());
                f[i] = f_t;
            }
            let limit = problem.rows[c].mu * f[base];
            let t_norm = (f[base + 1] * f[base + 1] + f[base + 2] * f[base + 2]).sqrt();
            if t_norm > limit {
                let s = if t_norm > 0.0 { limit / t_norm } else { 0.0 };
                f[base + 1] *= s;
                f[base + 2] *= s;
            }
        }
        if max_move < 1e-10 {
            break;
        }
    }

    (0..n)
        .map(|c| Vec3::new(f[3 * c], f[3 * c + 1], f[3 * c + 2]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_ir::{Instance, Joint, JointKind, PartDef, Vec3 as VcadVec3};

    /// A unit tetrahedron as an inline `ImportedMesh`, in millimetres.
    fn tetra_mesh_op(size: f64) -> vcad_ir::CsgOp {
        vcad_ir::CsgOp::ImportedMesh {
            positions: vec![
                0.0, 0.0, 0.0, size, 0.0, 0.0, 0.0, size, 0.0, 0.0, 0.0, size,
            ],
            indices: vec![0, 2, 1, 0, 1, 3, 0, 3, 2, 1, 2, 3],
            normals: None,
            source: None,
        }
    }

    /// A part whose mesh sits under a transform, with **no** authored
    /// inertial — so its mass properties can only come from the geometry.
    ///
    /// Regression: the BRep evaluator returns no solid for a mesh op, so the
    /// physics builder's mesh fast-path was the only thing that could resolve
    /// one — and it matched a *bare* mesh node only. A URDF `<visual>` with an
    /// `origin` offset imports as `Translate { MeshImport }`, which fell
    /// through to the evaluator and reported "no resolvable geometry", failing
    /// the whole build. XLeRobot's arm-camera links are exactly this shape.
    fn create_transformed_mesh_document() -> Document {
        let mut doc = Document::new();
        doc.nodes.insert(
            1,
            vcad_ir::Node {
                id: 1,
                name: Some("anchor_geom".to_string()),
                op: vcad_ir::CsgOp::Cube {
                    size: VcadVec3::new(100.0, 100.0, 50.0),
                },
            },
        );
        doc.nodes.insert(
            2,
            vcad_ir::Node {
                id: 2,
                name: Some("cam_geom".to_string()),
                op: tetra_mesh_op(40.0),
            },
        );
        doc.nodes.insert(
            3,
            vcad_ir::Node {
                id: 3,
                name: Some("cam_translate".to_string()),
                op: vcad_ir::CsgOp::Translate {
                    child: 2,
                    offset: VcadVec3::new(0.0, 0.0, 30.0),
                },
            },
        );

        let mut part_defs = HashMap::new();
        for (id, root) in [("anchor", 1), ("cam", 3)] {
            part_defs.insert(
                id.to_string(),
                PartDef {
                    id: id.to_string(),
                    name: Some(id.to_string()),
                    root,
                    default_material: None,
                    inertial: None,
                },
            );
        }
        doc.part_defs = Some(part_defs);
        doc.instances = Some(vec![
            Instance {
                id: "anchor_inst".to_string(),
                part_def_id: "anchor".to_string(),
                name: None,
                tags: Vec::new(),
                transform: None,
                material: None,
            },
            Instance {
                id: "cam_inst".to_string(),
                part_def_id: "cam".to_string(),
                name: None,
                tags: Vec::new(),
                transform: None,
                material: None,
            },
        ]);
        doc.joints = Some(vec![Joint {
            id: "cam_joint".to_string(),
            name: None,
            parent_instance_id: Some("anchor_inst".to_string()),
            child_instance_id: "cam_inst".to_string(),
            parent_anchor: VcadVec3::new(0.0, 0.0, 25.0),
            child_anchor: VcadVec3::new(0.0, 0.0, 0.0),
            kind: JointKind::Revolute {
                axis: VcadVec3::new(0.0, 0.0, 1.0),
                limits: None,
                effort_limit: None,
                velocity_limit: None,
            },
            state: 0.0,
        }]);
        doc.ground_instance_id = Some("anchor_inst".to_string());
        doc
    }

    #[test]
    fn resolves_a_mesh_under_a_transform() {
        let doc = create_transformed_mesh_document();
        PhysicsWorld::from_document(&doc).expect(
            "a mesh wrapped in a Translate must resolve — the BRep evaluator \
             yields no solid for a mesh op, so this is the only path to \
             geometry for a part with no authored inertial",
        );
    }

    #[test]
    fn transform_under_mesh_moves_the_centre_of_mass() {
        // Resolving the mesh is not enough: the transform has to actually be
        // applied. A fast-path that returned the *untransformed* leaf would
        // pass the test above while placing the collider 30 mm out of place.
        let mut doc = create_transformed_mesh_document();
        let shifted = PhysicsWorld::evaluate_part(&doc, 3)
            .unwrap()
            .expect("transformed mesh resolves");
        doc.nodes.get_mut(&3).unwrap().op = vcad_ir::CsgOp::Translate {
            child: 2,
            offset: VcadVec3::new(0.0, 0.0, 0.0),
        };
        let unshifted = PhysicsWorld::evaluate_part(&doc, 3)
            .unwrap()
            .expect("untransformed mesh resolves");

        let mean_z = |m: &vcad_kernel_tessellate::TriangleMesh| {
            let zs: Vec<f32> = m.vertices.chunks(3).map(|v| v[2]).collect();
            zs.iter().sum::<f32>() / zs.len() as f32
        };
        let delta = mean_z(&shifted) - mean_z(&unshifted);
        assert!(
            (delta - 30.0).abs() < 1e-3,
            "the 30 mm Translate must move the mesh, got {delta} mm"
        );
    }

    fn create_test_document() -> Document {
        let mut doc = Document::new();

        // Add cube nodes
        doc.nodes.insert(
            1,
            vcad_ir::Node {
                id: 1,
                name: Some("base_geom".to_string()),
                op: vcad_ir::CsgOp::Cube {
                    size: VcadVec3::new(100.0, 100.0, 50.0),
                },
            },
        );
        doc.nodes.insert(
            2,
            vcad_ir::Node {
                id: 2,
                name: Some("arm_geom".to_string()),
                op: vcad_ir::CsgOp::Cube {
                    size: VcadVec3::new(20.0, 20.0, 100.0),
                },
            },
        );

        // Add part definitions
        let mut part_defs = HashMap::new();
        part_defs.insert(
            "base".to_string(),
            PartDef {
                id: "base".to_string(),
                name: Some("Base".to_string()),
                root: 1,
                default_material: None,
                inertial: None,
            },
        );
        part_defs.insert(
            "arm".to_string(),
            PartDef {
                id: "arm".to_string(),
                name: Some("Arm".to_string()),
                root: 2,
                default_material: None,
                inertial: None,
            },
        );
        doc.part_defs = Some(part_defs);

        // Add instances
        doc.instances = Some(vec![
            Instance {
                id: "base_inst".to_string(),
                part_def_id: "base".to_string(),
                name: Some("Base".to_string()),
                tags: Vec::new(),
                transform: None,
                material: None,
            },
            Instance {
                id: "arm_inst".to_string(),
                part_def_id: "arm".to_string(),
                name: Some("Arm".to_string()),
                tags: Vec::new(),
                transform: None,
                material: None,
            },
        ]);

        // Add joint
        doc.joints = Some(vec![Joint {
            id: "joint1".to_string(),
            name: Some("Base-Arm".to_string()),
            parent_instance_id: Some("base_inst".to_string()),
            child_instance_id: "arm_inst".to_string(),
            parent_anchor: VcadVec3::new(0.0, 0.0, 25.0),
            child_anchor: VcadVec3::new(0.0, 0.0, -50.0),
            kind: JointKind::Revolute {
                axis: VcadVec3::new(0.0, 0.0, 1.0),
                limits: Some((-90.0, 90.0)),
                effort_limit: None,
                velocity_limit: None,
            },
            state: 0.0,
        }]);

        doc.ground_instance_id = Some("base_inst".to_string());

        doc
    }

    #[test]
    fn test_create_world() {
        let doc = create_test_document();
        let world = PhysicsWorld::from_document(&doc).unwrap();

        assert_eq!(world.instance_ids().len(), 2);
        assert_eq!(world.joint_ids().len(), 1);
    }

    /// `perturb_joint_state` must move every DOF. Writing only `q[offset]`
    /// left a Ball joint with a third of the requested initial-state
    /// randomization and a Free joint with a sixth — silently, since the
    /// caller sees no error and the episode just starts less varied than
    /// asked for.
    #[test]
    fn perturb_moves_every_dof_of_a_multi_dof_joint() {
        let mut doc = create_test_document();
        doc.joints.as_mut().unwrap()[0].kind = JointKind::Ball;
        let mut world = PhysicsWorld::from_document(&doc).unwrap();

        let before = world.get_joint_dofs("joint1").expect("ball dofs").0;
        assert_eq!(before.len(), 3, "Ball should expose 3 position DOFs");

        world.perturb_joint_state("joint1", 4.0, 0.0);
        let after = world.get_joint_dofs("joint1").expect("ball dofs").0;

        for (k, (b, a)) in before.iter().zip(&after).enumerate() {
            assert!(
                (a - b - 4.0).abs() < 1e-9,
                "DOF {k} moved by {}, expected 4.0 — perturbation applied to only part of the joint",
                a - b
            );
        }
    }

    #[test]
    fn test_step_simulation() {
        let doc = create_test_document();
        let mut world = PhysicsWorld::from_document(&doc).unwrap();

        // Step a few times
        for _ in 0..10 {
            world.step(1.0 / 60.0);
        }

        // Should have some joint states
        let states = world.get_joint_states();
        assert!(states.contains_key("joint1"));
    }

    #[test]
    fn test_joint_control() {
        let doc = create_test_document();
        let mut world = PhysicsWorld::from_document(&doc).unwrap();

        // Set joint position target
        world.set_joint_position("joint1", 45.0);

        // Step simulation
        for _ in 0..100 {
            world.step(1.0 / 60.0);
        }

        // Joint should have moved (exact position depends on dynamics)
        let states = world.get_joint_states();
        let state = states.get("joint1").unwrap();
        // Position should be non-zero after commanding 45 degrees
        assert!(state.position.abs() > 0.0 || state.velocity.abs() > 0.0);
    }

    /// Rewrite the fixture's revolute joint with K1-knee actuator limits
    /// (40 N·m effort, 12.5 rad/s velocity).
    fn with_k1_knee_limits(mut doc: Document) -> Document {
        let joints = doc.joints.as_mut().unwrap();
        if let JointKind::Revolute {
            effort_limit,
            velocity_limit,
            ..
        } = &mut joints[0].kind
        {
            *effort_limit = Some(40.0);
            *velocity_limit = Some(12.5_f64.to_degrees());
        }
        doc
    }

    #[test]
    fn test_torque_action_saturates_at_effort_limit() {
        // Booster K1 knee reference: a 1e6 N·m torque command must saturate
        // at the joint's 40 N·m effort limit.
        let doc = with_k1_knee_limits(create_test_document());
        let mut world = PhysicsWorld::from_document(&doc).unwrap();

        world.apply_joint_torque("joint1", 1e6);
        world.step(1.0 / 240.0);

        let states = world.get_joint_states();
        let effort = states.get("joint1").unwrap().effort;
        assert!(
            (effort - 40.0).abs() < 1e-9,
            "expected effort saturated at 40 N·m, got {effort}"
        );

        // And symmetric on the negative side.
        world.apply_joint_torque("joint1", -1e6);
        world.step(1.0 / 240.0);
        let effort = world.get_joint_states().get("joint1").unwrap().effort;
        assert!(
            (effort + 40.0).abs() < 1e-9,
            "expected effort saturated at -40 N·m, got {effort}"
        );
    }

    #[test]
    fn test_position_pd_output_respects_effort_limit() {
        let doc = with_k1_knee_limits(create_test_document());
        let mut world = PhysicsWorld::from_document(&doc).unwrap();

        // Huge explicit gains guarantee an unsaturated PD output far above
        // the effort limit at this position error.
        world.set_joint_gains("joint1", 1e6, 1e3);
        world.set_joint_position("joint1", 90.0);
        world.step(1.0 / 240.0);

        let effort = world.get_joint_states().get("joint1").unwrap().effort;
        assert!(
            effort.abs() <= 40.0 + 1e-9,
            "PD output must be clamped to the 40 N·m effort limit, got {effort}"
        );
        assert!(effort.abs() > 39.0, "expected the clamp to be active");
    }

    #[test]
    fn test_velocity_target_clamped_to_velocity_limit() {
        let doc = with_k1_knee_limits(create_test_document());
        let mut world = PhysicsWorld::from_document(&doc).unwrap();

        // Command far beyond the 12.5 rad/s limit (in deg/s).
        world.set_joint_velocity("joint1", 1e6);
        let motor = world.motors.get("joint1").unwrap();
        assert!(
            (motor.target - 12.5).abs() < 1e-9,
            "velocity target must clamp to 12.5 rad/s, got {}",
            motor.target
        );
    }

    #[test]
    fn test_explicit_joint_gains_override_defaults() {
        let doc = create_test_document();
        let mut world = PhysicsWorld::from_document(&doc).unwrap();

        world.set_joint_gains("joint1", 200.0, 5.0);
        world.set_joint_position("joint1", 10.0);

        let motor = world.motors.get("joint1").unwrap();
        assert_eq!(motor.kp, 200.0);
        assert_eq!(motor.kd, 5.0);
    }

    #[test]
    fn test_forward_kinematics_at_does_not_mutate_state() {
        let doc = create_test_document();
        let mut world = PhysicsWorld::from_document(&doc).unwrap();
        let q_before: Vec<f64> = (0..world.state.q.len()).map(|i| world.state.q[i]).collect();

        let poses = world.forward_kinematics_at(&[45.0]).unwrap();
        // Test fixture has two instances, one of which is the rotating arm
        // — its world-z must be positive (lifted by parent_anchor) and FK
        // restores state at the end.
        assert!(poses.contains_key("base_inst"));
        assert!(poses.contains_key("arm_inst"));

        let q_after: Vec<f64> = (0..world.state.q.len()).map(|i| world.state.q[i]).collect();
        assert_eq!(
            q_before, q_after,
            "forward_kinematics_at must restore state.q"
        );
    }

    #[test]
    fn test_gravity_torques_at_returns_per_joint_torque() {
        let doc = create_test_document();
        let mut world = PhysicsWorld::from_document(&doc).unwrap();

        let tau = world.gravity_torques_at(&[0.0]).unwrap();
        // Test fixture has joint1 (revolute about Z). At q=0 with gravity
        // along -Z, a Z-axis revolute joint sees zero gravity moment about
        // its axis (lever arm parallel to gravity). The map must still
        // contain the key with a finite value.
        assert!(tau.contains_key("joint1"));
        assert!(tau["joint1"].is_finite());
    }

    /// Y-axis revolute under -Z gravity should produce a non-zero gravity
    /// torque whenever the link is off vertical. Smoke-tests
    /// [`PhysicsWorld::gravity_torques_at`].
    #[test]
    fn test_gravity_torques_at_nonzero_for_y_axis_revolute() {
        let mut doc = Document::new();
        doc.nodes.insert(
            1,
            vcad_ir::Node {
                id: 1,
                name: Some("base_g".into()),
                op: vcad_ir::CsgOp::Cube {
                    size: VcadVec3::new(100.0, 100.0, 50.0),
                },
            },
        );
        doc.nodes.insert(
            2,
            vcad_ir::Node {
                id: 2,
                name: Some("link_g".into()),
                op: vcad_ir::CsgOp::Cube {
                    size: VcadVec3::new(20.0, 20.0, 100.0),
                },
            },
        );
        let mut part_defs = HashMap::new();
        part_defs.insert(
            "base".into(),
            vcad_ir::PartDef {
                id: "base".into(),
                name: None,
                root: 1,
                default_material: None,
                inertial: None,
            },
        );
        part_defs.insert(
            "arm".into(),
            vcad_ir::PartDef {
                id: "arm".into(),
                name: None,
                root: 2,
                default_material: None,
                inertial: None,
            },
        );
        doc.part_defs = Some(part_defs);
        doc.instances = Some(vec![
            vcad_ir::Instance {
                id: "base_inst".into(),
                part_def_id: "base".into(),
                name: None,
                tags: Vec::new(),
                transform: None,
                material: None,
            },
            vcad_ir::Instance {
                id: "arm_inst".into(),
                part_def_id: "arm".into(),
                name: None,
                tags: Vec::new(),
                transform: None,
                material: None,
            },
        ]);
        doc.joints = Some(vec![vcad_ir::Joint {
            id: "j".into(),
            name: None,
            parent_instance_id: Some("base_inst".into()),
            child_instance_id: "arm_inst".into(),
            parent_anchor: VcadVec3::new(0.0, 0.0, 25.0),
            child_anchor: VcadVec3::new(0.0, 0.0, -50.0),
            kind: JointKind::Revolute {
                axis: VcadVec3::new(0.0, 1.0, 0.0),
                limits: Some((-180.0, 180.0)),
                effort_limit: None,
                velocity_limit: None,
            },
            state: 0.0,
        }]);
        doc.ground_instance_id = Some("base_inst".into());

        let mut world = PhysicsWorld::from_document(&doc).unwrap();
        // 60° off vertical → non-zero gravity moment about the Y joint axis.
        let tau = world.gravity_torques_at(&[60.0]).unwrap();
        let t = tau["j"];
        assert!(
            t.abs() > 1e-4,
            "expected nonzero gravity torque at q=60° about Y axis, got {}",
            t
        );
    }

    /// Build the three-link hanging chain from the depth-compounding repro:
    /// ground at world z=500, then three identical 20x20x100 segments each
    /// hanging -Z from its own origin, joined tip-to-tip by revolute joints
    /// about `axis`. At zero joint angle every link must sit exactly 100 mm
    /// below the previous one with an identity orientation, for *any* axis.
    fn hanging_chain(axis: VcadVec3) -> Document {
        let mut doc = Document::new();

        // seg geometry: 20x20x100 box translated so it hangs down -Z.
        doc.nodes.insert(
            1,
            vcad_ir::Node {
                id: 1,
                name: None,
                op: vcad_ir::CsgOp::Cube {
                    size: VcadVec3::new(20.0, 20.0, 100.0),
                },
            },
        );
        doc.nodes.insert(
            2,
            vcad_ir::Node {
                id: 2,
                name: None,
                op: vcad_ir::CsgOp::Translate {
                    child: 1,
                    offset: VcadVec3::new(-10.0, -10.0, -100.0),
                },
            },
        );
        // base geometry: a small block for the ground instance.
        doc.nodes.insert(
            3,
            vcad_ir::Node {
                id: 3,
                name: None,
                op: vcad_ir::CsgOp::Cube {
                    size: VcadVec3::new(40.0, 40.0, 20.0),
                },
            },
        );

        let mut part_defs = HashMap::new();
        part_defs.insert(
            "seg".to_string(),
            PartDef {
                id: "seg".to_string(),
                name: None,
                root: 2,
                default_material: None,
                inertial: None,
            },
        );
        part_defs.insert(
            "base".to_string(),
            PartDef {
                id: "base".to_string(),
                name: None,
                root: 3,
                default_material: None,
                inertial: None,
            },
        );
        doc.part_defs = Some(part_defs);

        let inst = |id: &str, part: &str, z: f64| Instance {
            id: id.to_string(),
            part_def_id: part.to_string(),
            name: None,
            tags: Vec::new(),
            transform: Some(vcad_ir::Transform3D {
                translation: VcadVec3::new(0.0, 0.0, z),
                ..Default::default()
            }),
            material: None,
        };
        doc.instances = Some(vec![
            inst("i-base", "base", 500.0),
            inst("i-1", "seg", 500.0),
            inst("i-2", "seg", 400.0),
            inst("i-3", "seg", 300.0),
        ]);

        let joint = |id: &str, parent: &str, child: &str, pz: f64| Joint {
            id: id.to_string(),
            name: None,
            parent_instance_id: Some(parent.to_string()),
            child_instance_id: child.to_string(),
            parent_anchor: VcadVec3::new(0.0, 0.0, pz),
            child_anchor: VcadVec3::new(0.0, 0.0, 0.0),
            kind: JointKind::Revolute {
                axis,
                limits: Some((-90.0, 90.0)),
                effort_limit: None,
                velocity_limit: None,
            },
            state: 0.0,
        };
        doc.joints = Some(vec![
            joint("j1", "i-base", "i-1", 0.0),
            joint("j2", "i-1", "i-2", -100.0),
            joint("j3", "i-2", "i-3", -100.0),
        ]);
        doc.ground_instance_id = Some("i-base".to_string());

        doc
    }

    /// Poses of the chain at zero joint angle must be axis-independent:
    /// links at z = 0.5, 0.4, 0.3 m, all with identity orientation. The
    /// axis-alignment rotation used to point phyz's Z-only revolute at an
    /// arbitrary axis must not leak into the child body's world frame — it
    /// used to, invisibly at depth 1 and compounding at every depth beyond.
    fn assert_chain_poses(axis: VcadVec3) {
        let doc = hanging_chain(axis);
        let world = PhysicsWorld::from_document(&doc).unwrap();

        for (inst, expect_z) in [("i-1", 0.5), ("i-2", 0.4), ("i-3", 0.3)] {
            let (pos, quat) = world.get_instance_pose(inst).unwrap();
            assert!(
                (pos[0]).abs() < 1e-9 && (pos[1]).abs() < 1e-9 && (pos[2] - expect_z).abs() < 1e-9,
                "axis {:?}: {} at {:?}, expected [0, 0, {}]",
                (axis.x, axis.y, axis.z),
                inst,
                pos,
                expect_z
            );
            // Identity quaternion, up to sign.
            assert!(
                (quat[0].abs() - 1.0).abs() < 1e-9
                    && quat[1].abs() < 1e-9
                    && quat[2].abs() < 1e-9
                    && quat[3].abs() < 1e-9,
                "axis {:?}: {} orientation {:?}, expected identity",
                (axis.x, axis.y, axis.z),
                inst,
                quat
            );
        }
    }

    #[test]
    fn test_chain_zero_pose_z_axis() {
        assert_chain_poses(VcadVec3::new(0.0, 0.0, 1.0));
    }

    #[test]
    fn test_chain_zero_pose_y_axis() {
        assert_chain_poses(VcadVec3::new(0.0, 1.0, 0.0));
    }

    #[test]
    fn test_chain_zero_pose_x_axis() {
        assert_chain_poses(VcadVec3::new(1.0, 0.0, 0.0));
    }

    /// Mixed-axis chain, the humanoid-leg shape: yaw/roll/pitch alternating.
    /// Same invariant — the chain hangs straight down at zero angle.
    #[test]
    fn test_chain_zero_pose_mixed_axes() {
        let mut doc = hanging_chain(VcadVec3::new(0.0, 0.0, 1.0));
        let axes = [
            VcadVec3::new(0.0, 0.0, 1.0),
            VcadVec3::new(0.0, 1.0, 0.0),
            VcadVec3::new(1.0, 0.0, 0.0),
        ];
        for (joint, axis) in doc.joints.as_mut().unwrap().iter_mut().zip(axes) {
            joint.kind = JointKind::Revolute {
                axis,
                limits: Some((-90.0, 90.0)),
                effort_limit: None,
                velocity_limit: None,
            };
        }
        let world = PhysicsWorld::from_document(&doc).unwrap();
        for (inst, expect_z) in [("i-1", 0.5), ("i-2", 0.4), ("i-3", 0.3)] {
            let (pos, quat) = world.get_instance_pose(inst).unwrap();
            assert!(
                pos[0].abs() < 1e-9 && pos[1].abs() < 1e-9 && (pos[2] - expect_z).abs() < 1e-9,
                "mixed axes: {} at {:?}, expected [0, 0, {}]",
                inst,
                pos,
                expect_z
            );
            assert!(
                (quat[0].abs() - 1.0).abs() < 1e-9,
                "mixed axes: {} orientation {:?}, expected identity",
                inst,
                quat
            );
        }
    }

    /// Physics FK must agree with the CAD-side assembly evaluator at every
    /// joint configuration, not just zero — the shared-oracle check. The CAD
    /// evaluator is the reference: it was correct while the physics
    /// conversion was not.
    #[test]
    fn test_physics_fk_matches_cad_fk_mixed_axes() {
        let mut doc = hanging_chain(VcadVec3::new(0.0, 1.0, 0.0));
        let axes = [
            VcadVec3::new(1.0, 0.0, 0.0),
            VcadVec3::new(0.0, 1.0, 0.0),
            VcadVec3::new(0.0, 0.0, 1.0),
        ];
        let angles = [17.0, -35.0, 48.0];
        for ((joint, axis), state) in doc
            .joints
            .as_mut()
            .unwrap()
            .iter_mut()
            .zip(axes)
            .zip(angles)
        {
            joint.kind = JointKind::Revolute {
                axis,
                limits: Some((-90.0, 90.0)),
                effort_limit: None,
                velocity_limit: None,
            };
            joint.state = state;
        }

        let mut world = PhysicsWorld::from_document(&doc).unwrap();
        let poses = world.forward_kinematics_at(&angles).unwrap();

        // CAD-side reference: walk the chain by hand with the same
        // convention the assembly evaluator uses — each joint places its
        // child at parent_anchor, rotated by `state` about `axis`.
        let mut ref_rot = Mat3::identity();
        let mut ref_pos = Vec3::new(0.0, 0.0, 0.5);
        for (i, joint) in doc.joints.as_ref().unwrap().iter().enumerate() {
            let JointKind::Revolute { axis, .. } = joint.kind else {
                unreachable!()
            };
            let anchor = Vec3::new(
                joint.parent_anchor.x / 1000.0,
                joint.parent_anchor.y / 1000.0,
                joint.parent_anchor.z / 1000.0,
            );
            ref_pos += ref_rot.mul_vec(anchor);
            let a = Vec3::new(axis.x, axis.y, axis.z).normalize();
            ref_rot = ref_rot.mul_mat(&phyz::math::Mat3::rotation_axis(a, angles[i].to_radians()));

            let inst = format!("i-{}", i + 1);
            let (pos, _) = poses[&inst];
            for k in 0..3 {
                let expect = [ref_pos.x, ref_pos.y, ref_pos.z][k];
                assert!(
                    (pos[k] - expect).abs() < 1e-9,
                    "{} component {}: physics {} vs CAD {}",
                    inst,
                    k,
                    pos[k],
                    expect
                );
            }
        }
    }

    /// Movability propagation keys off `Joint::ndof() > 0`, so a body with
    /// no parent (added via `add_free_body`) is movable only because phyz
    /// gives a free joint 6 DOF. If that contract ever changes, free bodies
    /// silently lose their contact geometry and fall through the floor — a
    /// failure that surfaces as a confusing tunneling assertion elsewhere.
    /// Pin it here so the breakage names itself.
    #[test]
    fn phyz_free_joint_has_dof_so_unparented_bodies_are_movable() {
        let free = phyz::model::Joint::free(phyz::math::SpatialTransform::identity());
        assert_eq!(
            free.ndof(),
            6,
            "phyz free joint no longer reports 6 DOF; PhysicsWorld's movability \
             propagation would exclude free bodies from ground contact"
        );

        // End-to-end: an instance with no joints becomes a free body and must
        // come out of construction with contact geometry attached.
        let mut doc = Document::new();
        doc.nodes.insert(
            1,
            vcad_ir::Node {
                id: 1,
                name: None,
                op: vcad_ir::CsgOp::Cube {
                    size: VcadVec3::new(100.0, 100.0, 100.0),
                },
            },
        );
        let mut part_defs = HashMap::new();
        for id in ["crate", "anchor"] {
            part_defs.insert(
                id.to_string(),
                PartDef {
                    id: id.to_string(),
                    name: None,
                    root: 1,
                    default_material: None,
                    inertial: None,
                },
            );
        }
        doc.part_defs = Some(part_defs);
        doc.instances = Some(vec![
            Instance {
                id: "anchor_inst".to_string(),
                part_def_id: "anchor".to_string(),
                name: None,
                tags: std::vec::Vec::new(),
                transform: None,
                material: None,
            },
            Instance {
                id: "crate_inst".to_string(),
                part_def_id: "crate".to_string(),
                name: None,
                tags: std::vec::Vec::new(),
                transform: None,
                material: None,
            },
        ]);
        doc.joints = Some(std::vec::Vec::new());
        doc.ground_instance_id = Some("anchor_inst".to_string());

        let world = PhysicsWorld::from_document(&doc).unwrap();
        assert!(
            world.contact_geometries.iter().any(|g| g.is_some()),
            "free body was built without contact geometry — it can never touch the ground"
        );
    }
}
