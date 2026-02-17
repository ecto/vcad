//! Physics world management using phyz.

use std::collections::HashMap;

use phyz::phyz_math::{Mat3, Quat, SpatialInertia, SpatialTransform, Vec3};
use phyz::phyz_model::{Model, ModelBuilder, State};
use phyz::{ContactMaterial, Simulator, forward_kinematics, Geometry};
use vcad_ir::{Document, JointKind};

use crate::colliders::{estimate_mass, mesh_to_collider, ColliderStrategy};
use crate::error::PhysicsError;
use crate::joints::{
    MotorTarget, MotorMode, convert_state_from_physics, convert_state_to_physics,
    joint_ndof, vcad_joint_to_phyz,
};

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
    simulator: Simulator,
    contact_material: ContactMaterial,
    ground_height: f64,

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
            .gravity(Vec3::new(0.0, -9.81, 0.0))
            .dt(1.0 / 240.0);

        let mut instance_to_body: HashMap<String, usize> = HashMap::new();
        let mut joint_to_index: HashMap<String, usize> = HashMap::new();
        let mut joint_kinds: HashMap<String, JointKind> = HashMap::new();
        let mut body_geometries: Vec<Option<Geometry>> = Vec::new();
        let mut body_count = 0usize;

        // Helper: evaluate mesh and compute inertia for an instance
        let eval_instance = |inst: &vcad_ir::Instance| -> Result<(vcad_kernel_tessellate::TriangleMesh, f64, Geometry), PhysicsError> {
            let part_def = part_defs
                .get(&inst.part_def_id)
                .ok_or_else(|| PhysicsError::MissingPartDef(inst.part_def_id.clone()))?;
            let mesh = Self::evaluate_part(doc, part_def.root)?;
            let density = doc
                .materials
                .get(inst.material.as_deref().unwrap_or("default"))
                .and_then(|m| m.density)
                .unwrap_or(1000.0);
            let mass = estimate_mass(&mesh, density);
            let geometry = mesh_to_collider(&mesh, ColliderStrategy::ConvexHull, &inst.id)?;
            Ok((mesh, mass, geometry))
        };

        // Compute a box inertia from mass and mesh bounding box
        let compute_inertia = |mesh: &vcad_kernel_tessellate::TriangleMesh, mass: f64| -> SpatialInertia {
            // Compute bounding box to estimate inertia
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
            // Box inertia: I_xx = m/12 * (dy² + dz²), etc.
            let ixx = mass / 12.0 * (dy * dy + dz * dz);
            let iyy = mass / 12.0 * (dx * dx + dz * dz);
            let izz = mass / 12.0 * (dx * dx + dy * dy);
            let inertia_mat = Mat3::new(ixx, 0.0, 0.0, 0.0, iyy, 0.0, 0.0, 0.0, izz);
            SpatialInertia::new(mass, Vec3::zeros(), inertia_mat)
        };

        // 1. Add ground body (fixed, attached to world)
        let ground_inst = instances
            .iter()
            .find(|i| i.id == *ground_id)
            .ok_or_else(|| PhysicsError::MissingInstance(ground_id.clone()))?;
        {
            let (mesh, mass, geometry) = eval_instance(ground_inst)?;
            let inertia = compute_inertia(&mesh, mass);
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
                    .ok_or_else(|| PhysicsError::MissingInstance(joint.child_instance_id.clone()))?;

                let parent_body_idx = *instance_to_body
                    .get(&parent_id)
                    .ok_or_else(|| PhysicsError::MissingInstance(parent_id.clone()))?;

                let (mesh, mass, geometry) = eval_instance(child_inst)?;
                let inertia = compute_inertia(&mesh, mass);

                // Create phyz joint
                let phyz_joint = vcad_joint_to_phyz(joint)?;

                builder = builder.add_body(
                    &child_inst.id,
                    parent_body_idx as i32,
                    phyz_joint,
                    inertia,
                );

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

            let (mesh, mass, geometry) = eval_instance(inst)?;
            let inertia = compute_inertia(&mesh, mass);
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
            simulator: Simulator::new(),
            contact_material: ContactMaterial::default(),
            ground_height: 0.0,
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

        // Step with contact detection
        self.simulator.step_with_contacts(
            &self.model,
            &mut self.state,
            self.ground_height,
            &self.contact_material,
        );

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
        let pos = inv.translation_vector();
        let rot = inv.rotation_matrix();
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

        // Create a simple mesh based on the primitive type
        let solid = match &node.op {
            vcad_ir::CsgOp::Cube { size } => {
                vcad_kernel::Solid::cube(size.x, size.y, size.z)
            }
            vcad_ir::CsgOp::Cylinder { radius, height, segments } => {
                vcad_kernel::Solid::cylinder(*radius, *height, if *segments == 0 { 32 } else { *segments })
            }
            vcad_ir::CsgOp::Sphere { radius, segments } => {
                vcad_kernel::Solid::sphere(*radius, if *segments == 0 { 32 } else { *segments })
            }
            vcad_ir::CsgOp::Cone { radius_bottom, radius_top, height, segments } => {
                vcad_kernel::Solid::cone(*radius_bottom, *radius_top, *height, if *segments == 0 { 32 } else { *segments })
            }
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
            },
        );
        part_defs.insert(
            "arm".to_string(),
            PartDef {
                id: "arm".to_string(),
                name: Some("Arm".to_string()),
                root: 2,
                default_material: None,
            },
        );
        doc.part_defs = Some(part_defs);

        // Add instances
        doc.instances = Some(vec![
            Instance {
                id: "base_inst".to_string(),
                part_def_id: "base".to_string(),
                name: Some("Base".to_string()),
                transform: None,
                material: None,
            },
            Instance {
                id: "arm_inst".to_string(),
                part_def_id: "arm".to_string(),
                name: Some("Arm".to_string()),
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
}
