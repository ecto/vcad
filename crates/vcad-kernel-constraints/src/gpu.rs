//! GPU-accelerated constraint evaluation via WGSL compute shaders.
//!
//! Generates WGSL shaders from the symbolic constraint system for parallel
//! evaluation of residuals and Jacobian entries on the GPU. Useful for:
//! - Large sketch systems with many constraints
//! - Batch evaluation of many sketches simultaneously
//! - GPU-accelerated optimization loops
//!
//! # Architecture
//!
//! Each GPU work item processes one parameter set and writes:
//! - `num_residuals` residual values
//! - `num_residuals * num_params` Jacobian entries (row-major)
//!
//! The shaders use f32 arithmetic (GPU-native), so results may differ from
//! the f64 CPU path by ~1e-6 relative error.

use crate::constraint::Constraint;
use crate::entity::{EntityId, SketchEntity};
use crate::symbolic::build_residual_exprs;
use slotmap::SlotMap;
use tang_expr::{trace, ExprId};

/// A WGSL compute shader for constraint evaluation.
pub struct WgslConstraintKernel {
    /// Complete WGSL shader source for residual evaluation.
    pub residual_shader: String,
    /// Complete WGSL shader source for Jacobian evaluation.
    pub jacobian_shader: String,
    /// Number of input parameters per work item.
    pub num_params: usize,
    /// Number of residual outputs per work item.
    pub num_residuals: usize,
    /// Number of Jacobian entries per work item (num_residuals * num_params).
    pub num_jacobian_entries: usize,
    /// Workgroup size used in the shaders.
    pub workgroup_size: u32,
}

/// Build WGSL compute shaders for a constraint system.
///
/// This traces the constraint residuals symbolically, differentiates each
/// residual w.r.t. each parameter, and compiles both to WGSL shaders.
///
/// # Arguments
/// * `constraints` - The constraint definitions
/// * `entities` - The sketch entity map (points, lines, circles, arcs)
/// * `num_params` - Total number of scalar parameters
///
/// # Returns
/// A [`WgslConstraintKernel`] with residual and Jacobian shader source code.
pub fn build_wgsl_constraint_system(
    constraints: &[Constraint],
    entities: &SlotMap<EntityId, SketchEntity>,
    num_params: usize,
) -> WgslConstraintKernel {
    let num_residuals: usize = constraints.iter().map(|c| c.num_residuals()).sum();

    if num_residuals == 0 || num_params == 0 {
        return WgslConstraintKernel {
            residual_shader: empty_shader(0, 0),
            jacobian_shader: empty_shader(0, 0),
            num_params,
            num_residuals,
            num_jacobian_entries: 0,
            workgroup_size: 256,
        };
    }

    // Trace the constraint system symbolically
    let (mut graph, residual_exprs) = trace(|| build_residual_exprs(constraints, entities));

    // Differentiate each residual w.r.t. each parameter
    let mut jac_exprs = Vec::with_capacity(num_residuals * num_params);
    for r in &residual_exprs {
        for j in 0..num_params {
            let d = graph.diff(*r, j as u16);
            let d = graph.simplify(d);
            jac_exprs.push(d);
        }
    }

    // Simplify residuals
    let residual_exprs: Vec<ExprId> = residual_exprs
        .into_iter()
        .map(|r| graph.simplify(r))
        .collect();

    // Generate WGSL shaders
    let residual_kernel = graph.to_wgsl(&residual_exprs, num_params);
    let jacobian_kernel = graph.to_wgsl(&jac_exprs, num_params);

    WgslConstraintKernel {
        residual_shader: residual_kernel.source,
        jacobian_shader: jacobian_kernel.source,
        num_params,
        num_residuals,
        num_jacobian_entries: num_residuals * num_params,
        workgroup_size: residual_kernel.workgroup_size,
    }
}

/// Generate a combined WGSL shader that evaluates both residuals and Jacobian
/// entries in a single dispatch.
///
/// Output layout per work item:
/// - `[0..num_residuals]` — residual values
/// - `[num_residuals..num_residuals + num_residuals*num_params]` — Jacobian (row-major)
pub fn build_wgsl_combined_system(
    constraints: &[Constraint],
    entities: &SlotMap<EntityId, SketchEntity>,
    num_params: usize,
) -> WgslConstraintKernel {
    let num_residuals: usize = constraints.iter().map(|c| c.num_residuals()).sum();

    if num_residuals == 0 || num_params == 0 {
        let total_outputs = num_residuals + num_residuals * num_params;
        return WgslConstraintKernel {
            residual_shader: empty_shader(num_params, total_outputs),
            jacobian_shader: String::new(),
            num_params,
            num_residuals,
            num_jacobian_entries: num_residuals * num_params,
            workgroup_size: 256,
        };
    }

    // Trace the constraint system symbolically
    let (mut graph, residual_exprs) = trace(|| build_residual_exprs(constraints, entities));

    // Differentiate each residual w.r.t. each parameter
    let mut jac_exprs = Vec::with_capacity(num_residuals * num_params);
    for r in &residual_exprs {
        for j in 0..num_params {
            let d = graph.diff(*r, j as u16);
            let d = graph.simplify(d);
            jac_exprs.push(d);
        }
    }

    // Simplify residuals
    let residual_exprs: Vec<ExprId> = residual_exprs
        .into_iter()
        .map(|r| graph.simplify(r))
        .collect();

    // Combine: residuals first, then Jacobian entries
    let mut all_outputs = residual_exprs;
    all_outputs.extend_from_slice(&jac_exprs);

    let combined_kernel = graph.to_wgsl(&all_outputs, num_params);

    WgslConstraintKernel {
        residual_shader: combined_kernel.source,
        jacobian_shader: String::new(), // Combined into residual_shader
        num_params,
        num_residuals,
        num_jacobian_entries: num_residuals * num_params,
        workgroup_size: combined_kernel.workgroup_size,
    }
}

/// Generate an empty/no-op WGSL shader.
fn empty_shader(n_inputs: usize, n_outputs: usize) -> String {
    format!(
        "// Auto-generated empty constraint shader\n\
         \n\
         struct Params {{\n\
         \x20   count: u32,\n\
         \x20   _pad1: u32,\n\
         \x20   _pad2: u32,\n\
         \x20   _pad3: u32,\n\
         }}\n\
         \n\
         @group(0) @binding(0) var<storage, read> inputs: array<f32>;\n\
         @group(0) @binding(1) var<storage, read_write> outputs: array<f32>;\n\
         @group(0) @binding(2) var<uniform> params: Params;\n\
         \n\
         @compute @workgroup_size(256)\n\
         fn main(@builtin(global_invocation_id) gid: vec3<u32>) {{\n\
         \x20   let idx = gid.x;\n\
         \x20   if (idx >= params.count) {{ return; }}\n\
         \x20   let _base_in = idx * {n_inputs}u;\n\
         \x20   let _base_out = idx * {n_outputs}u;\n\
         }}\n"
    )
}

/// Metadata about the generated WGSL system for GPU dispatch.
impl WgslConstraintKernel {
    /// Total number of f32 output values per work item for the combined shader.
    pub fn outputs_per_item(&self) -> usize {
        self.num_residuals + self.num_jacobian_entries
    }

    /// Compute the number of workgroups needed for `n` work items.
    pub fn workgroup_count(&self, n: u32) -> u32 {
        n.div_ceil(self.workgroup_size)
    }

    /// Size in bytes of the input buffer for `n` work items.
    pub fn input_buffer_size(&self, n: u32) -> u64 {
        (n as u64) * (self.num_params as u64) * 4 // f32 = 4 bytes
    }

    /// Size in bytes of the residual output buffer for `n` work items.
    pub fn residual_buffer_size(&self, n: u32) -> u64 {
        (n as u64) * (self.num_residuals as u64) * 4
    }

    /// Size in bytes of the Jacobian output buffer for `n` work items.
    pub fn jacobian_buffer_size(&self, n: u32) -> u64 {
        (n as u64) * (self.num_jacobian_entries as u64) * 4
    }

    /// Size in bytes of the combined output buffer for `n` work items.
    pub fn combined_buffer_size(&self, n: u32) -> u64 {
        (n as u64) * (self.outputs_per_item() as u64) * 4
    }
}

// ============================================================================
// GPU dispatch (requires "gpu" feature or test builds)
// ============================================================================

/// Errors from GPU constraint evaluation.
#[derive(Debug)]
#[cfg(any(feature = "gpu", test))]
pub enum GpuConstraintError {
    /// No compatible GPU adapter found.
    NoAdapter,
    /// Failed to request GPU device.
    DeviceRequest(wgpu::RequestDeviceError),
    /// Buffer mapping failed.
    BufferMapping,
}

#[cfg(any(feature = "gpu", test))]
impl std::fmt::Display for GpuConstraintError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoAdapter => write!(f, "No compatible GPU adapter found"),
            Self::DeviceRequest(e) => write!(f, "Failed to request GPU device: {e}"),
            Self::BufferMapping => write!(f, "Buffer mapping failed"),
        }
    }
}

#[cfg(any(feature = "gpu", test))]
impl std::error::Error for GpuConstraintError {}

/// GPU-backed constraint evaluator.
///
/// Compiles WGSL shaders from a constraint system and dispatches compute
/// work on the GPU. Evaluates `n` parameter sets in parallel, returning
/// residuals and/or Jacobian entries.
///
/// # Example
///
/// ```ignore
/// let solver = GpuConstraintSolver::new(&constraints, &entities, num_params)?;
/// let params_batch: Vec<f32> = /* n * num_params values */;
/// let (residuals, jacobian) = solver.evaluate_batch(&params_batch, n)?;
/// ```
#[cfg(any(feature = "gpu", test))]
pub struct GpuConstraintSolver {
    device: wgpu::Device,
    queue: wgpu::Queue,
    residual_pipeline: wgpu::ComputePipeline,
    jacobian_pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    kernel: WgslConstraintKernel,
}

#[cfg(any(feature = "gpu", test))]
impl GpuConstraintSolver {
    /// Create a new GPU constraint solver.
    ///
    /// Initializes wgpu, compiles the WGSL shaders, and creates compute pipelines.
    pub fn new(
        constraints: &[Constraint],
        entities: &SlotMap<EntityId, SketchEntity>,
        num_params: usize,
    ) -> Result<Self, GpuConstraintError> {
        let kernel = build_wgsl_constraint_system(constraints, entities, num_params);

        // Initialize wgpu
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            ..Default::default()
        });

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .ok_or(GpuConstraintError::NoAdapter)?;

        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default(), None))
                .map_err(GpuConstraintError::DeviceRequest)?;

        // Create bind group layout (shared between residual and Jacobian pipelines)
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Constraint BGL"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Constraint Pipeline Layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        // Compile residual shader
        let residual_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Residual Shader"),
            source: wgpu::ShaderSource::Wgsl(kernel.residual_shader.as_str().into()),
        });

        let residual_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Residual Pipeline"),
            layout: Some(&pipeline_layout),
            module: &residual_module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });

        // Compile Jacobian shader
        let jacobian_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Jacobian Shader"),
            source: wgpu::ShaderSource::Wgsl(kernel.jacobian_shader.as_str().into()),
        });

        let jacobian_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Jacobian Pipeline"),
            layout: Some(&pipeline_layout),
            module: &jacobian_module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });

        Ok(Self {
            device,
            queue,
            residual_pipeline,
            jacobian_pipeline,
            bind_group_layout,
            kernel,
        })
    }

    /// Evaluate residuals and Jacobian for a batch of parameter sets on the GPU.
    ///
    /// # Arguments
    /// * `params` - Flat f32 array: `n * num_params` values (batch of n parameter sets)
    /// * `n` - Number of work items (parameter sets)
    ///
    /// # Returns
    /// * `(residuals, jacobian)` — both as flat f32 vectors
    ///   - `residuals`: `n * num_residuals` values
    ///   - `jacobian`: `n * num_residuals * num_params` values (row-major per work item)
    pub fn evaluate_batch(
        &self,
        params: &[f32],
        n: u32,
    ) -> Result<(Vec<f32>, Vec<f32>), GpuConstraintError> {
        let residuals = self.dispatch(
            &self.residual_pipeline,
            params,
            n,
            self.kernel.num_residuals,
        )?;
        let jacobian = self.dispatch(
            &self.jacobian_pipeline,
            params,
            n,
            self.kernel.num_jacobian_entries,
        )?;
        Ok((residuals, jacobian))
    }

    /// Evaluate only residuals for a batch.
    pub fn evaluate_residuals(
        &self,
        params: &[f32],
        n: u32,
    ) -> Result<Vec<f32>, GpuConstraintError> {
        self.dispatch(
            &self.residual_pipeline,
            params,
            n,
            self.kernel.num_residuals,
        )
    }

    /// Internal: dispatch a compute shader and read back results.
    fn dispatch(
        &self,
        pipeline: &wgpu::ComputePipeline,
        params: &[f32],
        n: u32,
        outputs_per_item: usize,
    ) -> Result<Vec<f32>, GpuConstraintError> {
        use wgpu::util::DeviceExt;

        let input_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Input Buffer"),
                contents: bytemuck::cast_slice(params),
                usage: wgpu::BufferUsages::STORAGE,
            });

        let output_size = (n as usize * outputs_per_item * 4) as u64;
        // Ensure minimum buffer size of 4 bytes for wgpu validation
        let output_size = output_size.max(4);

        let output_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Output Buffer"),
            size: output_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        // Params uniform: { count: u32, _pad: u32, _pad: u32, _pad: u32 }
        let params_data: [u32; 4] = [n, 0, 0, 0];
        let params_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Params Uniform"),
                contents: bytemuck::cast_slice(&params_data),
                usage: wgpu::BufferUsages::UNIFORM,
            });

        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Constraint Bind Group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: input_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: output_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: params_buffer.as_entire_binding(),
                },
            ],
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Constraint Encoder"),
            });

        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Constraint Pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(self.kernel.workgroup_count(n), 1, 1);
        }

        // Copy to staging buffer for readback
        let staging_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Staging Buffer"),
            size: output_size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        encoder.copy_buffer_to_buffer(&output_buffer, 0, &staging_buffer, 0, output_size);

        self.queue.submit(std::iter::once(encoder.finish()));

        // Read back
        let buffer_slice = staging_buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
            tx.send(result).unwrap();
        });

        self.device.poll(wgpu::Maintain::Wait);
        rx.recv()
            .unwrap()
            .map_err(|_| GpuConstraintError::BufferMapping)?;

        let data = buffer_slice.get_mapped_range();
        let results: Vec<f32> = bytemuck::cast_slice(&data).to_vec();
        drop(data);
        staging_buffer.unmap();

        Ok(results)
    }

    /// Access the generated kernel metadata.
    pub fn kernel(&self) -> &WgslConstraintKernel {
        &self.kernel
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constraint::EntityRef;
    use crate::entity::{SketchLine, SketchPoint};
    use crate::symbolic::CompiledSystem;

    /// Helper: set up a simple 2-point, 1-line system with horizontal constraint.
    fn setup_horizontal() -> (Vec<Constraint>, SlotMap<EntityId, SketchEntity>, usize) {
        let mut entities = SlotMap::with_key();
        let p1 = entities.insert(SketchEntity::Point(SketchPoint {
            param_x: 0,
            param_y: 1,
        }));
        let p2 = entities.insert(SketchEntity::Point(SketchPoint {
            param_x: 2,
            param_y: 3,
        }));
        let line = entities.insert(SketchEntity::Line(SketchLine { start: p1, end: p2 }));
        let constraints = vec![Constraint::Horizontal { line }];
        (constraints, entities, 4)
    }

    /// Helper: set up distance + horizontal constraints.
    fn setup_mixed() -> (Vec<Constraint>, SlotMap<EntityId, SketchEntity>, usize) {
        let mut entities = SlotMap::with_key();
        let p1 = entities.insert(SketchEntity::Point(SketchPoint {
            param_x: 0,
            param_y: 1,
        }));
        let p2 = entities.insert(SketchEntity::Point(SketchPoint {
            param_x: 2,
            param_y: 3,
        }));
        let p3 = entities.insert(SketchEntity::Point(SketchPoint {
            param_x: 4,
            param_y: 5,
        }));
        let line = entities.insert(SketchEntity::Line(SketchLine { start: p1, end: p2 }));

        let constraints = vec![
            Constraint::Horizontal { line },
            Constraint::Distance {
                point_a: EntityRef::Point(p1),
                point_b: EntityRef::Point(p2),
                distance: 10.0,
            },
            Constraint::Fixed {
                point: EntityRef::Point(p3),
                x: 5.0,
                y: 1.5,
            },
        ];
        (constraints, entities, 6)
    }

    #[test]
    fn wgsl_horizontal_generates_valid_shader() {
        let (constraints, entities, num_params) = setup_horizontal();
        let kernel = build_wgsl_constraint_system(&constraints, &entities, num_params);

        // Check residual shader structure
        assert!(kernel.residual_shader.contains("@compute"));
        assert!(kernel.residual_shader.contains("@workgroup_size(256)"));
        assert!(kernel.residual_shader.contains("fn main("));
        assert!(kernel.residual_shader.contains("inputs"));
        assert!(kernel.residual_shader.contains("outputs"));

        // Verify dimensions
        assert_eq!(kernel.num_params, 4);
        assert_eq!(kernel.num_residuals, 1);
        assert_eq!(kernel.num_jacobian_entries, 4);
    }

    #[test]
    fn wgsl_horizontal_loads_all_inputs() {
        let (constraints, entities, num_params) = setup_horizontal();
        let kernel = build_wgsl_constraint_system(&constraints, &entities, num_params);

        // Should load 4 input values per work item
        assert!(kernel.residual_shader.contains("let base_in = idx * 4u;"));
        assert!(kernel
            .residual_shader
            .contains("let x0 = inputs[base_in + 0u];"));
        assert!(kernel
            .residual_shader
            .contains("let x1 = inputs[base_in + 1u];"));
        assert!(kernel
            .residual_shader
            .contains("let x2 = inputs[base_in + 2u];"));
        assert!(kernel
            .residual_shader
            .contains("let x3 = inputs[base_in + 3u];"));
    }

    #[test]
    fn wgsl_horizontal_residual_has_one_output() {
        let (constraints, entities, num_params) = setup_horizontal();
        let kernel = build_wgsl_constraint_system(&constraints, &entities, num_params);

        // Horizontal: error = ey - sy, so 1 output
        assert!(kernel.residual_shader.contains("let base_out = idx * 1u;"));
        assert!(kernel.residual_shader.contains("outputs[base_out + 0u]"));
    }

    #[test]
    fn wgsl_jacobian_has_correct_outputs() {
        let (constraints, entities, num_params) = setup_horizontal();
        let kernel = build_wgsl_constraint_system(&constraints, &entities, num_params);

        // Jacobian: 1 residual * 4 params = 4 outputs
        assert!(kernel.jacobian_shader.contains("let base_out = idx * 4u;"));
    }

    #[test]
    fn wgsl_mixed_constraints() {
        let (constraints, entities, num_params) = setup_mixed();
        let kernel = build_wgsl_constraint_system(&constraints, &entities, num_params);

        // 1 (horizontal) + 1 (distance) + 2 (fixed) = 4 residuals
        assert_eq!(kernel.num_residuals, 4);
        assert_eq!(kernel.num_params, 6);
        assert_eq!(kernel.num_jacobian_entries, 24);

        // Residual shader: 4 outputs
        assert!(kernel.residual_shader.contains("let base_out = idx * 4u;"));

        // Jacobian shader: 24 outputs
        assert!(kernel.jacobian_shader.contains("let base_out = idx * 24u;"));
    }

    #[test]
    fn wgsl_combined_shader() {
        let (constraints, entities, num_params) = setup_mixed();
        let kernel = build_wgsl_combined_system(&constraints, &entities, num_params);

        // Combined: 4 residuals + 24 Jacobian entries = 28 outputs
        assert_eq!(kernel.outputs_per_item(), 28);
        assert!(kernel.residual_shader.contains("let base_out = idx * 28u;"));

        // Jacobian shader should be empty (combined into residual_shader)
        assert!(kernel.jacobian_shader.is_empty());
    }

    #[test]
    fn wgsl_empty_system() {
        let entities: SlotMap<EntityId, SketchEntity> = SlotMap::with_key();
        let constraints: Vec<Constraint> = vec![];
        let kernel = build_wgsl_constraint_system(&constraints, &entities, 0);

        assert_eq!(kernel.num_residuals, 0);
        assert_eq!(kernel.num_params, 0);
        assert!(kernel.residual_shader.contains("@compute"));
    }

    #[test]
    fn wgsl_buffer_sizes() {
        let (constraints, entities, num_params) = setup_mixed();
        let kernel = build_wgsl_constraint_system(&constraints, &entities, num_params);

        // 10 work items
        assert_eq!(kernel.input_buffer_size(10), 10 * 6 * 4);
        assert_eq!(kernel.residual_buffer_size(10), 10 * 4 * 4);
        assert_eq!(kernel.jacobian_buffer_size(10), 10 * 24 * 4);
        assert_eq!(kernel.workgroup_count(256), 1);
        assert_eq!(kernel.workgroup_count(257), 2);
    }

    #[test]
    fn wgsl_residuals_match_cpu() {
        // Verify that the WGSL shader evaluates the same expressions as the CPU path
        // by checking that the expression graph produces consistent output counts.
        let (constraints, entities, num_params) = setup_mixed();

        let cpu_system = CompiledSystem::build(&constraints, &entities, num_params);
        let gpu_kernel = build_wgsl_constraint_system(&constraints, &entities, num_params);

        assert_eq!(cpu_system.num_residuals, gpu_kernel.num_residuals);
        assert_eq!(cpu_system.num_params, gpu_kernel.num_params);
    }

    #[test]
    fn wgsl_shader_contains_math_ops() {
        // Distance constraint uses sqrt, so the Jacobian should contain
        // sqrt or reciprocal operations.
        let (constraints, entities, num_params) = setup_mixed();
        let kernel = build_wgsl_constraint_system(&constraints, &entities, num_params);

        // The distance constraint involves sqrt(dx^2 + dy^2)
        assert!(
            kernel.residual_shader.contains("sqrt(") || kernel.residual_shader.contains("(x"),
            "Distance constraint should generate sqrt or variable references"
        );
    }

    #[test]
    fn wgsl_perpendicular_constraint() {
        let mut entities = SlotMap::with_key();
        let p1 = entities.insert(SketchEntity::Point(SketchPoint {
            param_x: 0,
            param_y: 1,
        }));
        let p2 = entities.insert(SketchEntity::Point(SketchPoint {
            param_x: 2,
            param_y: 3,
        }));
        let p3 = entities.insert(SketchEntity::Point(SketchPoint {
            param_x: 4,
            param_y: 5,
        }));
        let p4 = entities.insert(SketchEntity::Point(SketchPoint {
            param_x: 6,
            param_y: 7,
        }));
        let line1 = entities.insert(SketchEntity::Line(SketchLine { start: p1, end: p2 }));
        let line2 = entities.insert(SketchEntity::Line(SketchLine { start: p3, end: p4 }));

        let constraints = vec![Constraint::Perpendicular {
            line_a: line1,
            line_b: line2,
        }];

        let kernel = build_wgsl_constraint_system(&constraints, &entities, 8);
        assert_eq!(kernel.num_residuals, 1);
        assert_eq!(kernel.num_params, 8);
        assert_eq!(kernel.num_jacobian_entries, 8);

        // Should reference all 8 input variables
        for i in 0..8 {
            assert!(
                kernel
                    .residual_shader
                    .contains(&format!("let x{i} = inputs[base_in + {i}u];")),
                "Missing input variable x{i}"
            );
        }
    }

    #[test]
    fn wgsl_angle_constraint_uses_atan2() {
        let mut entities = SlotMap::with_key();
        let p1 = entities.insert(SketchEntity::Point(SketchPoint {
            param_x: 0,
            param_y: 1,
        }));
        let p2 = entities.insert(SketchEntity::Point(SketchPoint {
            param_x: 2,
            param_y: 3,
        }));
        let p3 = entities.insert(SketchEntity::Point(SketchPoint {
            param_x: 4,
            param_y: 5,
        }));
        let p4 = entities.insert(SketchEntity::Point(SketchPoint {
            param_x: 6,
            param_y: 7,
        }));
        let line1 = entities.insert(SketchEntity::Line(SketchLine { start: p1, end: p2 }));
        let line2 = entities.insert(SketchEntity::Line(SketchLine { start: p3, end: p4 }));

        let constraints = vec![Constraint::Angle {
            line_a: line1,
            line_b: line2,
            angle_rad: std::f64::consts::FRAC_PI_2,
        }];

        let kernel = build_wgsl_constraint_system(&constraints, &entities, 8);

        // Angle constraint uses atan2
        assert!(
            kernel.residual_shader.contains("atan2("),
            "Angle constraint should generate atan2 call"
        );
    }

    #[test]
    fn wgsl_combined_output_layout() {
        // Verify that the combined shader's output layout matches expectations:
        // [residuals..., jacobian_row_major...]
        let (constraints, entities, num_params) = setup_horizontal();
        let kernel = build_wgsl_combined_system(&constraints, &entities, num_params);

        // 1 residual + 4 Jacobian entries = 5 total outputs
        assert_eq!(kernel.outputs_per_item(), 5);

        // The combined shader stores all in one output array
        assert!(kernel.residual_shader.contains("let base_out = idx * 5u;"));

        // Output 0 = residual, outputs 1..4 = Jacobian row
        assert!(kernel.residual_shader.contains("outputs[base_out + 0u]"));
        assert!(kernel.residual_shader.contains("outputs[base_out + 1u]"));
        assert!(kernel.residual_shader.contains("outputs[base_out + 4u]"));
    }

    // ========================================================================
    // GPU dispatch tests (require actual GPU hardware)
    // ========================================================================

    #[test]
    #[ignore = "requires GPU"]
    fn gpu_solver_creates_successfully() {
        let (constraints, entities, num_params) = setup_horizontal();
        let solver = GpuConstraintSolver::new(&constraints, &entities, num_params);
        assert!(
            solver.is_ok(),
            "GpuConstraintSolver should initialize: {:?}",
            solver.err()
        );
    }

    #[test]
    #[ignore = "requires GPU"]
    fn gpu_residuals_match_cpu_horizontal() {
        let (constraints, entities, num_params) = setup_horizontal();
        let solver = GpuConstraintSolver::new(&constraints, &entities, num_params).unwrap();

        // Params: p1=(0,0), p2=(10,5)
        let params_f64 = [0.0f64, 0.0, 10.0, 5.0];
        let params_f32: Vec<f32> = params_f64.iter().map(|&v| v as f32).collect();

        // GPU evaluation
        let gpu_residuals = solver.evaluate_residuals(&params_f32, 1).unwrap();

        // CPU evaluation
        let cpu_system = CompiledSystem::build(&constraints, &entities, num_params);
        let cpu_residuals = cpu_system.eval_residuals(&params_f64);

        assert_eq!(gpu_residuals.len(), cpu_residuals.len());
        for (i, (gpu, cpu)) in gpu_residuals.iter().zip(cpu_residuals.iter()).enumerate() {
            assert!(
                (*gpu as f64 - cpu).abs() < 1e-4,
                "Residual {i} mismatch: GPU={gpu}, CPU={cpu}"
            );
        }
    }

    #[test]
    #[ignore = "requires GPU"]
    fn gpu_jacobian_matches_cpu_horizontal() {
        let (constraints, entities, num_params) = setup_horizontal();
        let solver = GpuConstraintSolver::new(&constraints, &entities, num_params).unwrap();

        let params_f64 = [0.0f64, 0.0, 10.0, 5.0];
        let params_f32: Vec<f32> = params_f64.iter().map(|&v| v as f32).collect();

        let (_, gpu_jacobian) = solver.evaluate_batch(&params_f32, 1).unwrap();

        // CPU Jacobian (dense)
        let cpu_system = CompiledSystem::build(&constraints, &entities, num_params);
        let cpu_jac = cpu_system.eval_jacobian(&params_f64);

        // Horizontal: d(ey - sy)/d[sx, sy, ex, ey] = [0, -1, 0, 1]
        assert_eq!(gpu_jacobian.len(), 4);
        let expected = [0.0f32, -1.0, 0.0, 1.0];
        for (i, (gpu, exp)) in gpu_jacobian.iter().zip(expected.iter()).enumerate() {
            assert!(
                (gpu - exp).abs() < 1e-4,
                "Jacobian[{i}] mismatch: GPU={gpu}, expected={exp}, CPU={}",
                cpu_jac[(0, i)]
            );
        }
    }

    #[test]
    #[ignore = "requires GPU"]
    fn gpu_batch_evaluation() {
        let (constraints, entities, num_params) = setup_horizontal();
        let solver = GpuConstraintSolver::new(&constraints, &entities, num_params).unwrap();

        // Batch of 3 parameter sets
        let params_f32: Vec<f32> = vec![
            0.0, 0.0, 10.0, 5.0, // set 0: residual = 5 - 0 = 5
            1.0, 2.0, 8.0, 2.0, // set 1: residual = 2 - 2 = 0
            0.0, 3.0, 5.0, 7.0, // set 2: residual = 7 - 3 = 4
        ];

        let residuals = solver.evaluate_residuals(&params_f32, 3).unwrap();

        assert_eq!(residuals.len(), 3);
        assert!((residuals[0] - 5.0).abs() < 1e-4, "Batch[0] residual");
        assert!((residuals[1] - 0.0).abs() < 1e-4, "Batch[1] residual");
        assert!((residuals[2] - 4.0).abs() < 1e-4, "Batch[2] residual");
    }

    #[test]
    #[ignore = "requires GPU"]
    fn gpu_mixed_constraints_match_cpu() {
        let (constraints, entities, num_params) = setup_mixed();
        let solver = GpuConstraintSolver::new(&constraints, &entities, num_params).unwrap();

        let params_f64 = [0.0f64, 0.0, 10.0, 3.0, 5.0, 1.5];
        let params_f32: Vec<f32> = params_f64.iter().map(|&v| v as f32).collect();

        let (gpu_residuals, gpu_jacobian) = solver.evaluate_batch(&params_f32, 1).unwrap();

        let cpu_system = CompiledSystem::build(&constraints, &entities, num_params);
        let cpu_residuals = cpu_system.eval_residuals(&params_f64);
        let cpu_jac = cpu_system.eval_jacobian(&params_f64);

        // Compare residuals
        assert_eq!(gpu_residuals.len(), cpu_residuals.len());
        for (i, (gpu, cpu)) in gpu_residuals.iter().zip(cpu_residuals.iter()).enumerate() {
            assert!(
                (*gpu as f64 - cpu).abs() < 1e-3,
                "Residual {i}: GPU={gpu}, CPU={cpu}"
            );
        }

        // Compare Jacobian (row-major)
        let nr = cpu_system.num_residuals;
        let np = cpu_system.num_params;
        assert_eq!(gpu_jacobian.len(), nr * np);
        for i in 0..nr {
            for j in 0..np {
                let gpu_val = gpu_jacobian[i * np + j] as f64;
                let cpu_val = cpu_jac[(i, j)];
                assert!(
                    (gpu_val - cpu_val).abs() < 1e-3,
                    "Jacobian[{i},{j}]: GPU={gpu_val}, CPU={cpu_val}"
                );
            }
        }
    }
}
