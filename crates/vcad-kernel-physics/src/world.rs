//! Physics world management using phyz.

use std::collections::HashMap;

use phyz::aba_with_external_forces;
use phyz::math::{Mat3, Quat, SpatialInertia, SpatialTransform, Vec3};
use phyz::model::{Model, ModelBuilder, State};
use phyz::{forward_kinematics, Geometry};
use vcad_ir::{Document, InertialProperties, JointKind};

use crate::colliders::{estimate_mass, mesh_to_collider, ColliderStrategy};
use crate::error::PhysicsError;
use crate::joints::{
    convert_state_from_physics, convert_state_to_physics, joint_ndof, vcad_joint_to_phyz,
    MotorMode, MotorTarget,
};

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
}

impl PhysicsWorld {
    /// Create a new physics world from a vcad Document.
    ///
    /// The document must have assembly data (instances and joints).
    pub fn from_document(doc: &Document) -> Result<Self, PhysicsError> {
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
            let mut mesh = Self::evaluate_part(doc, part_def.root)?;
            let mut authored = part_def.inertial;
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
            let geometry = mesh_to_collider(&mesh, ColliderStrategy::ConvexHull, &inst.id)?;
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
            instance_to_body,
            joint_to_index,
            joint_order,
            joint_kinds,
            joint_q_offsets,
            joint_v_offsets,
            body_part_frames,
            body_vels: vec![phyz::math::SpatialVec::zero(); nbodies],
            gain_scale: 1.0,
        };

        // Set initial joint states (zero-dof joints have no q slot to write)
        for joint in joints {
            if joint_ndof(&joint.kind) > 0 && joint.state.abs() > 1e-6 {
                world.set_joint_position(&joint.id, joint.state);
                // Also set the initial q value directly
                if let Some(&q_offset) = world.joint_q_offsets.get(&joint.id) {
                    let kind = &joint.kind;
                    let physics_val = convert_state_to_physics(kind, joint.state);
                    world.state.q[q_offset] = physics_val;
                }
            }
        }

        // Run initial FK
        world.refresh_kinematics();

        Ok(world)
    }

    /// Step the physics simulation by dt seconds.
    pub fn step(&mut self, dt: f32) {
        // Temporarily set the model timestep
        let original_dt = self.model.dt;
        self.model.dt = dt as f64;

        // Apply PD motor torques to state.ctrl
        self.apply_motor_torques();

        // Step: ABA forward dynamics + semi-implicit Euler integration
        {
            let qdd = aba_with_external_forces(&self.model, &self.state, None);

            let dt = self.model.dt;
            let nv = self.state.v.len();
            for i in 0..nv {
                self.state.v[i] += qdd[i] * dt;
            }

            let nq = self.state.q.len();
            for i in 0..nq {
                self.state.q[i] += self.state.v[i.min(nv - 1)] * dt;
            }

            self.enforce_joint_limits();

            self.refresh_kinematics();
        }

        self.model.dt = original_dt;
    }

    /// Recompute forward kinematics from the current `state.q` / `state.v`,
    /// refreshing the cached body transforms and spatial velocities. Call
    /// after mutating joint state directly (e.g. [`Self::perturb_joint_state`]).
    pub fn refresh_kinematics(&mut self) {
        let (xforms, vels) = forward_kinematics(&self.model, &self.state);
        self.state.body_xform = xforms;
        self.body_vels = vels;
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

    /// Apply PD motor torques from motor targets to state.ctrl.
    fn apply_motor_torques(&mut self) {
        // Zero out ctrl first
        for i in 0..self.state.ctrl.len() {
            self.state.ctrl[i] = 0.0;
        }

        for (joint_id, motor) in &self.motors {
            if let (Some(&q_offset), Some(&v_offset)) = (
                self.joint_q_offsets.get(joint_id),
                self.joint_v_offsets.get(joint_id),
            ) {
                let position = self.state.q[q_offset];
                let velocity = self.state.v[v_offset];
                let torque = motor.compute_torque(position, velocity);
                self.state.ctrl[v_offset] = torque;
            }
        }
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

                // For 1-DOF joints, read directly from state
                let position = self.state.q[q_offset];
                let velocity = self.state.v[v_offset];
                let effort = self.state.ctrl[v_offset];

                states.insert(
                    joint_id.clone(),
                    JointState {
                        position: convert_state_from_physics(kind, position),
                        velocity: convert_state_from_physics(kind, velocity),
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
            if joint_ndof(kind) == 0 {
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
            if joint_ndof(kind) == 0 {
                return;
            }
            let physics_target = convert_state_to_physics(kind, target);
            // Velocity servo: τ = kd (v* − v). Track within ~1/ω seconds and
            // clamp at the torque needed to reach the target from rest in one
            // time constant, scaled to the joint's reflected inertia.
            const OMEGA: f64 = 40.0;
            let i = self.reflected_inertia(joint_id);
            let kd = i * OMEGA * self.gain_scale;
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
            .is_none_or(|kind| joint_ndof(kind) == 0)
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
            self.state.q[q_offset] += convert_state_to_physics(kind, dpos);
            self.state.v[v_offset] += convert_state_to_physics(kind, dvel);
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
                let physics_val = convert_state_to_physics(kind, q[cursor + k]);
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
                self.state.q[q_offset + k] = convert_state_to_physics(kind, q[cursor + k]);
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
    /// actuated dof, so they are excluded here.
    pub fn actuated_joint_ids(&self) -> Vec<String> {
        self.joint_order
            .iter()
            .filter(|id| {
                self.joint_kinds
                    .get(*id)
                    .is_some_and(|kind| joint_ndof(kind) > 0)
            })
            .cloned()
            .collect()
    }

    /// Get list of all instance IDs.
    pub fn instance_ids(&self) -> Vec<String> {
        self.instance_to_body.keys().cloned().collect()
    }

    /// Evaluate a part's geometry to get a mesh.
    fn evaluate_part(
        doc: &Document,
        node_id: vcad_ir::NodeId,
    ) -> Result<vcad_kernel_tessellate::TriangleMesh, PhysicsError> {
        // This is a simplified evaluation - in practice would use the full engine
        let node = doc
            .nodes
            .get(&node_id)
            .ok_or_else(|| PhysicsError::Evaluation(format!("Node {} not found", node_id)))?;

        // STL meshes bypass the BRep solid path — load straight to a
        // triangle mesh in the IR's millimetre frame. If the path can't
        // be opened (e.g. browser-flow URDF imports keep the raw URDF
        // filename and have no filesystem behind it), fall back to a 1 cm
        // placeholder cube so authored inertials still anchor a body.
        if let vcad_ir::CsgOp::MeshImport { path, scale } = &node.op {
            match crate::stl::load_stl(std::path::Path::new(path), *scale) {
                Ok(mesh) => return Ok(mesh),
                Err(_) => return Ok(vcad_kernel::Solid::cube(10.0, 10.0, 10.0).to_mesh(32)),
            }
        }
        // Inline ImportedMesh (e.g. browser pre-parsed STL/DAE) ships its
        // triangle data inside the IR node — pull positions / indices /
        // optional normals straight across into the physics TriangleMesh
        // (units stay millimetres).
        if let vcad_ir::CsgOp::ImportedMesh {
            positions,
            indices,
            normals,
            ..
        } = &node.op
        {
            use vcad_kernel_tessellate::TriangleMesh;
            let n_verts = positions.len() / 3;
            let vertices: Vec<f32> = positions.iter().map(|v| *v as f32).collect();
            let normals_f32: Vec<f32> = normals
                .as_ref()
                .map(|n| n.iter().map(|v| *v as f32).collect())
                .unwrap_or_else(|| vec![0.0; n_verts * 3]);
            return Ok(TriangleMesh {
                vertices,
                indices: indices.clone(),
                normals: normals_f32,
                face_kinds: Vec::new(),
            });
        }

        // Create a simple mesh based on the primitive type
        let solid = match &node.op {
            vcad_ir::CsgOp::Cube { size } => vcad_kernel::Solid::cube(size.x, size.y, size.z),
            vcad_ir::CsgOp::Cylinder {
                radius,
                height,
                segments,
            } => vcad_kernel::Solid::cylinder(
                *radius,
                *height,
                if *segments == 0 { 32 } else { *segments },
            ),
            vcad_ir::CsgOp::Sphere { radius, segments } => {
                vcad_kernel::Solid::sphere(*radius, if *segments == 0 { 32 } else { *segments })
            }
            vcad_ir::CsgOp::Cone {
                radius_bottom,
                radius_top,
                height,
                segments,
            } => vcad_kernel::Solid::cone(
                *radius_bottom,
                *radius_top,
                *height,
                if *segments == 0 { 32 } else { *segments },
            ),
            vcad_ir::CsgOp::Torus {
                major_radius,
                minor_radius,
                segments,
            } => vcad_kernel::Solid::torus(
                *major_radius,
                *minor_radius,
                if *segments == 0 { 32 } else { *segments },
            ),
            vcad_ir::CsgOp::Wedge { size } => vcad_kernel::Solid::wedge(size.x, size.y, size.z),
            vcad_ir::CsgOp::Prism {
                sides,
                radius,
                height,
            } => vcad_kernel::Solid::prism(*sides, *radius, *height),
            _ => {
                // For other operations, create a small placeholder
                vcad_kernel::Solid::cube(10.0, 10.0, 10.0)
            }
        };

        Ok(solid.to_mesh(32))
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_ir::{Instance, Joint, JointKind, PartDef, Vec3 as VcadVec3};

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
}
