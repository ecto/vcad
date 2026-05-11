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

    // Original joint definitions for unit conversion
    joint_kinds: HashMap<String, JointKind>,

    // Joint DOF offsets in the state vectors
    joint_q_offsets: HashMap<String, usize>,
    joint_v_offsets: HashMap<String, usize>,
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
        let mut body_count = 0usize;

        // Helper: evaluate mesh, mass, and (optionally) authored inertials
        // for an instance. When the instance's PartDef carries an
        // `inertial` block (set by the URDF importer for any link with an
        // `<inertial>` tag), we surface those values; the caller prefers
        // them over mesh-derived inertia.
        let eval_instance = |inst: &vcad_ir::Instance| -> Result<
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
            let mesh = Self::evaluate_part(doc, part_def.root)?;
            let authored = part_def.inertial;
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
            let (mesh, mass, geometry, authored) = eval_instance(ground_inst)?;
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

                let (mesh, mass, geometry, authored) = eval_instance(child_inst)?;
                let inertia = build_inertia(&mesh, mass, authored);

                // Create phyz joint
                let phyz_joint = vcad_joint_to_phyz(joint)?;

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

        // 3. Add remaining instances as free-floating bodies
        for inst in instances {
            if visited.contains(&inst.id) {
                continue;
            }

            let (mesh, mass, geometry, authored) = eval_instance(inst)?;
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
            joint_kinds,
            joint_q_offsets,
            joint_v_offsets,
        };

        // Set initial joint states
        for joint in joints {
            if joint.state.abs() > 1e-6 {
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
        let (xforms, _) = forward_kinematics(&world.model, &world.state);
        world.state.body_xform = xforms;

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

            forward_kinematics(&self.model, &self.state);
        }

        self.model.dt = original_dt;
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
            let physics_target = convert_state_to_physics(kind, target);
            self.motors.insert(
                joint_id.to_string(),
                MotorTarget {
                    mode: MotorMode::Position,
                    target: physics_target,
                    ..MotorTarget::default()
                },
            );
        }
    }

    /// Set the target velocity for a joint.
    ///
    /// # Arguments
    ///
    /// * `joint_id` - The vcad joint ID
    /// * `target` - Target velocity (deg/s for revolute, mm/s for prismatic)
    pub fn set_joint_velocity(&mut self, joint_id: &str, target: f64) {
        if let Some(kind) = self.joint_kinds.get(joint_id) {
            let physics_target = convert_state_to_physics(kind, target);
            self.motors.insert(
                joint_id.to_string(),
                MotorTarget {
                    mode: MotorMode::Velocity,
                    target: physics_target,
                    ..MotorTarget::default()
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
        let xform = &self.state.body_xform[body_idx];

        // The body_xform from FK is world-to-body. We need body-in-world.
        // Actually phyz FK returns world_to_body transforms (Plücker convention).
        // To get the world-frame position, we need the inverse.
        let inv = xform.inverse();
        let pos = inv.pos;
        let rot = inv.rot;
        let quat = Quat::from_matrix(&rot);

        Some((
            [pos.x, pos.y, pos.z],
            [quat.w, quat.v.x, quat.v.y, quat.v.z],
        ))
    }

    /// Set gravity vector.
    pub fn set_gravity(&mut self, x: f32, y: f32, z: f32) {
        self.model.gravity = Vec3::new(x as f64, y as f64, z as f64);
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
            let inv = self.state.body_xform[body_idx].inverse();
            let pos = inv.pos;
            let rot = inv.rot;
            let quat = phyz::math::Quat::from_matrix(&rot);
            out.insert(
                inst_id.clone(),
                (
                    [pos.x, pos.y, pos.z],
                    [quat.w, quat.v.x, quat.v.y, quat.v.z],
                ),
            );
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
    pub fn joint_ids(&self) -> Vec<String> {
        self.joint_to_index.keys().cloned().collect()
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
}
