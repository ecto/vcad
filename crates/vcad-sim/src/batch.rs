//! GPU batch simulation pipeline.

use phyz_gpu::GpuBatchSimulator;
use phyz_model::{Model, State};
use vcad_ir::Document;

use crate::error::SimError;
use crate::{Observation, StepResult};

/// GPU batch simulation pipeline running N parallel environments.
///
/// Uses `phyz_gpu::GpuBatchSimulator` for GPU-accelerated Featherstone ABA
/// across multiple independent environments simultaneously.
pub struct BatchSimPipeline {
    gpu_sim: GpuBatchSimulator,
    n_envs: usize,
    nv: usize,
    initial_state: State,
}

impl BatchSimPipeline {
    /// Create a batch simulation pipeline from a vcad Document.
    ///
    /// Builds the phyz Model from the assembly, then initializes `n_envs`
    /// parallel GPU environments.
    pub fn from_document(doc: &Document, n_envs: usize) -> Result<Self, SimError> {
        let model = Self::build_model(doc)?;
        let initial_state = model.default_state();
        let nv = model.nv;

        let gpu_sim = GpuBatchSimulator::new(model, n_envs).map_err(SimError::Gpu)?;

        Ok(Self {
            gpu_sim,
            n_envs,
            nv,
            initial_state,
        })
    }

    /// Step all environments with per-environment actions.
    ///
    /// `actions` is a flat slice of length `n_envs * nv`, where each
    /// contiguous block of `nv` values is the action for one environment.
    pub fn batch_step(&mut self, actions: &[f64]) -> Result<Vec<StepResult>, SimError> {
        let expected = self.n_envs * self.nv;
        if actions.len() != expected {
            return Err(SimError::ActionMismatch {
                expected,
                got: actions.len(),
            });
        }

        // Set controls for each env
        let ctrls: Vec<Vec<f64>> = actions.chunks(self.nv).map(|c| c.to_vec()).collect();
        self.gpu_sim.set_controls(&ctrls);

        // Step GPU simulation
        self.gpu_sim.step();

        // Readback states
        let states = self.gpu_sim.readback_states();

        let mut results = Vec::with_capacity(self.n_envs);
        for state in &states {
            results.push(StepResult {
                joint_positions: state.q.as_slice().to_vec(),
                joint_velocities: state.v.as_slice().to_vec(),
                done: false,
            });
        }

        Ok(results)
    }

    /// Observe all environments without stepping.
    pub fn batch_observe(&self) -> Vec<Observation> {
        let states = self.gpu_sim.readback_states();

        let mut observations = Vec::with_capacity(self.n_envs);
        for state in &states {
            observations.push(Observation {
                joint_positions: state.q.as_slice().to_vec(),
                joint_velocities: state.v.as_slice().to_vec(),
            });
        }

        observations
    }

    /// Reset all environments to the initial state.
    pub fn batch_reset(&mut self) {
        let states: Vec<State> = (0..self.n_envs)
            .map(|_| self.initial_state.clone())
            .collect();
        self.gpu_sim.load_states(&states);
    }

    /// Enable ground-plane contact detection.
    ///
    /// Objects will be repelled from the ground at `height` via penalty forces.
    pub fn enable_ground_contact(
        &mut self,
        height: f64,
        stiffness: f64,
        damping: f64,
        friction: f64,
    ) -> Result<(), SimError> {
        self.gpu_sim
            .enable_ground_contact(height, stiffness, damping, friction)
            .map_err(SimError::Gpu)?;
        Ok(())
    }

    /// Get the number of parallel environments.
    pub fn n_envs(&self) -> usize {
        self.n_envs
    }

    /// Get the number of action dimensions per environment.
    pub fn action_dim(&self) -> usize {
        self.nv
    }

    /// Build a phyz Model from a vcad Document.
    fn build_model(doc: &Document) -> Result<Model, SimError> {
        use phyz_math::{Mat3, SpatialInertia, SpatialTransform, Vec3};
        use phyz_model::ModelBuilder;
        use std::collections::{HashMap, HashSet};

        let instances = doc.instances.as_ref().ok_or(SimError::NoAssembly)?;
        let joints = doc.joints.as_ref().ok_or(SimError::NoAssembly)?;
        let part_defs = doc.part_defs.as_ref().ok_or(SimError::NoAssembly)?;
        let ground_id = doc
            .ground_instance_id
            .as_ref()
            .ok_or(SimError::NoAssembly)?;

        let mut builder = ModelBuilder::new()
            .gravity(Vec3::new(0.0, 0.0, -9.81))
            .dt(1.0 / 240.0);

        let mut instance_to_body: HashMap<String, usize> = HashMap::new();
        // Per-body part→body rotation `R_b` (`p_body = R_b * p_part`). phyz
        // body frames coincide with the joint frame, so a child hung off a
        // non-Z revolute has a rotated body frame — and its own children's
        // `parent_to_joint` is measured from that rotated frame. Identity for
        // ground and free bodies.
        let mut body_rotations: Vec<Mat3> = Vec::new();
        let mut body_count = 0usize;

        // Compute box inertia from a primitive node
        let compute_inertia = |node_id: u64| -> (f64, SpatialInertia) {
            let node = doc.nodes.get(&node_id);
            let (dx, dy, dz) = match node.map(|n| &n.op) {
                Some(vcad_ir::CsgOp::Cube { size }) => {
                    (size.x / 1000.0, size.y / 1000.0, size.z / 1000.0)
                }
                Some(vcad_ir::CsgOp::Cylinder { radius, height, .. }) => {
                    let d = 2.0 * radius / 1000.0;
                    (d, d, height / 1000.0)
                }
                Some(vcad_ir::CsgOp::Sphere { radius, .. }) => {
                    let d = 2.0 * radius / 1000.0;
                    (d, d, d)
                }
                _ => (0.01, 0.01, 0.01),
            };
            let density = 1000.0; // kg/m³
            let mass = density * dx * dy * dz;
            let ixx = mass / 12.0 * (dy * dy + dz * dz);
            let iyy = mass / 12.0 * (dx * dx + dz * dz);
            let izz = mass / 12.0 * (dx * dx + dy * dy);
            let inertia_mat = Mat3::new(ixx, 0.0, 0.0, 0.0, iyy, 0.0, 0.0, 0.0, izz);
            (mass, SpatialInertia::new(mass, Vec3::zeros(), inertia_mat))
        };

        let instance_transform = |inst: &vcad_ir::Instance| -> SpatialTransform {
            inst.transform
                .as_ref()
                .map(|t| {
                    let translation = Vec3::new(
                        t.translation.x / 1000.0,
                        t.translation.y / 1000.0,
                        t.translation.z / 1000.0,
                    );
                    SpatialTransform::new(Mat3::identity(), translation)
                })
                .unwrap_or(SpatialTransform::identity())
        };

        // 1. Ground body (fixed)
        let ground_inst = instances
            .iter()
            .find(|i| i.id == *ground_id)
            .ok_or(SimError::NoAssembly)?;
        {
            let part = part_defs
                .get(&ground_inst.part_def_id)
                .ok_or(SimError::NoAssembly)?;
            let (_, inertia) = compute_inertia(part.root);
            let xform = instance_transform(ground_inst);
            builder = builder.add_fixed_body(&ground_inst.id, -1, xform, inertia);
            instance_to_body.insert(ground_inst.id.clone(), body_count);
            body_rotations.push(Mat3::identity());
            body_count += 1;
        }

        // 2. BFS from ground through joints
        let mut queue = vec![ground_id.clone()];
        let mut visited: HashSet<String> = HashSet::new();
        visited.insert(ground_id.clone());

        while let Some(parent_id) = queue.pop() {
            for joint in joints {
                let joint_parent = joint.parent_instance_id.as_deref().unwrap_or(ground_id);
                if joint_parent != parent_id || visited.contains(&joint.child_instance_id) {
                    continue;
                }

                let child_inst = instances
                    .iter()
                    .find(|i| i.id == joint.child_instance_id)
                    .ok_or(SimError::NoAssembly)?;

                let parent_body_idx = *instance_to_body
                    .get(&parent_id)
                    .ok_or(SimError::NoAssembly)?;

                let part = part_defs
                    .get(&child_inst.part_def_id)
                    .ok_or(SimError::NoAssembly)?;
                let (_, mut inertia) = compute_inertia(part.root);

                // Body frame = joint frame, so the part-axis-aligned box
                // inertia has to be rotated into it.
                let r_part_to_body = joint_frame_rotation(&joint.kind).transpose();
                inertia.inertia = r_part_to_body
                    .mul_mat(&inertia.inertia)
                    .mul_mat(&r_part_to_body.transpose());

                let phyz_joint = build_phyz_model_joint(joint, &body_rotations[parent_body_idx])?;

                builder =
                    builder.add_body(&child_inst.id, parent_body_idx as i32, phyz_joint, inertia);

                instance_to_body.insert(child_inst.id.clone(), body_count);
                body_rotations.push(r_part_to_body);
                body_count += 1;
                visited.insert(child_inst.id.clone());
                queue.push(child_inst.id.clone());
            }
        }

        Ok(builder.build())
    }
}

/// Convert a vcad `Joint` into a `phyz_model::Joint`.
///
/// Mirrors `vcad_kernel_physics::joints::vcad_joint_to_phyz`, but targets the
/// `phyz_model` crate's `Joint` type (required by `phyz-gpu`'s `ModelBuilder`),
/// which is a separate Rust type from `phyz::model::Joint`.
///
/// `r_parent` is the parent body's part→body rotation: `parent_to_joint` is
/// measured from the parent *body* frame, which is itself rotated whenever
/// the parent hangs off a non-Z joint axis. Composing it out here is what
/// keeps the axis-alignment rotation from compounding down a serial chain.
fn build_phyz_model_joint(
    joint: &vcad_ir::Joint,
    r_parent: &phyz_math::Mat3,
) -> Result<phyz_model::Joint, SimError> {
    use phyz_math::{SpatialTransform, Vec3};
    use phyz_model::Joint as PhyzJoint;
    use vcad_ir::JointKind;

    // parent_anchor is in the parent *part*'s mm coordinates; phyz wants the
    // joint origin in the parent *body* frame, in meters.
    let anchor = r_parent.mul_vec(Vec3::new(
        joint.parent_anchor.x / 1000.0,
        joint.parent_anchor.y / 1000.0,
        joint.parent_anchor.z / 1000.0,
    ));

    // Plücker parent→joint coordinate map: the transpose of the joint frame's
    // axes expressed in the parent body frame.
    let rot = r_parent
        .mul_mat(&joint_frame_rotation(&joint.kind))
        .transpose();
    let xform = SpatialTransform::new(rot, anchor);

    match &joint.kind {
        JointKind::Fixed => Ok(PhyzJoint::fixed(xform)),
        JointKind::Revolute { limits, .. } => {
            let mut phyz_joint = PhyzJoint::revolute(xform);

            if let Some((lower, upper)) = limits {
                phyz_joint.limits = Some([lower.to_radians(), upper.to_radians()]);
            }

            Ok(phyz_joint)
        }
        JointKind::Slider { axis, limits, .. } => {
            // Slider joint frames are part-aligned, so the axis carries over
            // unchanged.
            let axis_vec = Vec3::new(axis.x, axis.y, axis.z).normalize();
            let mut phyz_joint = PhyzJoint::prismatic(xform, axis_vec);

            if let Some((lower, upper)) = limits {
                phyz_joint.limits = Some([*lower / 1000.0, *upper / 1000.0]);
            }

            Ok(phyz_joint)
        }
        JointKind::Cylindrical { .. } => Ok(PhyzJoint::revolute(xform)),
        JointKind::Ball => Ok(PhyzJoint::spherical(xform)),
    }
}

/// Rotation whose columns are the joint frame's axes in the part frame:
/// phyz revolute joints spin about the joint frame's Z, so that frame is
/// oriented to put Z on the declared axis.
fn joint_frame_rotation(kind: &vcad_ir::JointKind) -> phyz_math::Mat3 {
    use vcad_ir::JointKind;
    match kind {
        JointKind::Revolute { axis, .. } | JointKind::Cylindrical { axis } => {
            rotation_aligning_z_to(phyz_math::Vec3::new(axis.x, axis.y, axis.z).normalize())
        }
        JointKind::Slider { .. } | JointKind::Fixed | JointKind::Ball => {
            phyz_math::Mat3::identity()
        }
    }
}

/// Compute a rotation matrix that maps the Z unit vector to the given axis.
fn rotation_aligning_z_to(target: phyz_math::Vec3) -> phyz_math::Mat3 {
    use phyz_math::{Mat3, Vec3};

    let z = Vec3::new(0.0, 0.0, 1.0);
    let dot = z.dot(target);

    if dot > 0.9999 {
        return Mat3::identity();
    }
    if dot < -0.9999 {
        // 180° rotation about X
        return Mat3::new(1.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, -1.0);
    }

    let cross = z.cross(target);
    let s = cross.norm();
    let c = dot;
    let vx = phyz_math::skew(&cross.normalize());

    // Rodrigues: R = I + sin(θ) * [v]× + (1 - cos(θ)) * [v]×²
    Mat3::identity() + vx * s + vx * vx * (1.0 - c)
}
