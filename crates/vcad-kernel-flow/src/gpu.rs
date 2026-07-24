//! M5: the preview lattice — LBM streaming/collision as WGSL compute.
//!
//! Runs the same D3Q19 step as [`crate::solve`] (isothermal path) on
//! the GPU via `vcad-kernel-gpu`'s shared wgpu context, native and
//! browser (WebGPU). This is the engine behind live smoke-in-viewport.
//!
//! **Preview lattice, not claim-grade.** The kernel computes in f32 and
//! runs a caller-chosen number of steps with no steadiness detection —
//! every result carries a `note` saying so. The f64 CPU solver remains
//! the oracle; the parity test in this module pins the two lattices to
//! each other at steady state, which is what licenses the preview to
//! *look* right while the claims come from the CPU path. Thermal
//! transport and Brinkman terms are CPU-only for now.

use wgpu::util::DeviceExt;

use vcad_kernel_gpu::{GpuContext, GpuError};

use crate::lattice::{Scaling, CS2, Q, W};
use crate::model::{norm, Cell, FlowModel};
use crate::solve::SolveError;

/// Preview run options.
#[derive(Debug, Clone, Copy)]
pub struct PreviewOptions {
    /// Lattice steps to run (no steadiness detection — previews render
    /// motion, they don't certify convergence).
    pub steps: usize,
    /// Smoothstep inlet ramp length, steps (same convention as the CPU
    /// solver).
    pub ramp_steps: usize,
    /// Reference speed override, m/s (same contract as the CPU solver).
    pub u_ref_m_s: Option<f64>,
}

impl Default for PreviewOptions {
    fn default() -> Self {
        PreviewOptions {
            steps: 2000,
            ramp_steps: 1000,
            u_ref_m_s: None,
        }
    }
}

/// A preview field: SI velocities and pressures, f32, explicitly not
/// claim-grade.
#[derive(Debug, Clone)]
pub struct PreviewField {
    /// Velocity per voxel, m/s (zero for non-fluid).
    pub velocity_m_s: Vec<[f32; 3]>,
    /// Gauge pressure per voxel, Pa (zero for non-fluid).
    pub gauge_pressure_pa: Vec<f32>,
    /// Steps run.
    pub steps: usize,
    /// The unit scaling used.
    pub scaling: Scaling,
    /// The standing disclaimer.
    pub note: &'static str,
}

/// GPU preview errors.
#[derive(Debug)]
pub enum PreviewError {
    /// Model/scaling rejected (same gates as the CPU solver).
    Solve(SolveError),
    /// GPU context/dispatch failure.
    Gpu(GpuError),
}

impl std::fmt::Display for PreviewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PreviewError::Solve(e) => write!(f, "preview model: {e}"),
            PreviewError::Gpu(e) => write!(f, "preview gpu: {e}"),
        }
    }
}

impl std::error::Error for PreviewError {}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    dims: [u32; 4],
    fluid: [f32; 4],
    u_in: [f32; 4],
    a_body: [f32; 4],
    periodic: [u32; 4],
}

/// Blocking preview run (native).
pub fn preview_blocking(
    model: &FlowModel,
    opts: &PreviewOptions,
) -> Result<PreviewField, PreviewError> {
    pollster::block_on(preview_async(model, opts))
}

/// Async preview run (WASM/browser and native).
pub async fn preview_async(
    model: &FlowModel,
    opts: &PreviewOptions,
) -> Result<PreviewField, PreviewError> {
    model
        .validate()
        .map_err(|e| PreviewError::Solve(e.into()))?;
    let n = model.cells.len();
    let dx_m = model.voxel_mm() / 1000.0;
    let inlet_speed = norm(model.inlet_velocity_m_s);
    let u_ref = match opts.u_ref_m_s {
        Some(u) if u > 0.0 => u,
        _ if inlet_speed > 0.0 => inlet_speed,
        _ => return Err(PreviewError::Solve(SolveError::NoReference)),
    };
    let scaling = Scaling::derive(dx_m, model.fluid.kinematic_viscosity_m2_s(), u_ref)
        .map_err(|e| PreviewError::Solve(e.into()))?;
    let omega = (1.0 / scaling.tau) as f32;
    let c_lat = scaling.dx_m / scaling.dt_s;
    let rho_out_lat =
        (1.0 + model.outlet_gauge_pa / (CS2 * model.fluid.density_kg_m3 * c_lat * c_lat)) as f32;
    let u_in_lat = [
        scaling.velocity_to_lattice(model.inlet_velocity_m_s[0]) as f32,
        scaling.velocity_to_lattice(model.inlet_velocity_m_s[1]) as f32,
        scaling.velocity_to_lattice(model.inlet_velocity_m_s[2]) as f32,
    ];
    let a_body = [
        scaling.accel_to_lattice(model.body_force_n_m3[0] / model.fluid.density_kg_m3) as f32,
        scaling.accel_to_lattice(model.body_force_n_m3[1] / model.fluid.density_kg_m3) as f32,
        scaling.accel_to_lattice(model.body_force_n_m3[2] / model.fluid.density_kg_m3) as f32,
    ];

    let ctx = GpuContext::init().await.map_err(PreviewError::Gpu)?;

    let cells_u32: Vec<u32> = model
        .cells
        .iter()
        .map(|c| match c {
            Cell::Solid => 0u32,
            Cell::Fluid => 1,
            Cell::Inlet => 2,
            Cell::Outlet => 3,
        })
        .collect();
    // Rest-state initialization: f = w_q everywhere (rho = 1, u = 0).
    let mut f0 = vec![0.0f32; n * Q];
    for x in 0..n {
        for q in 0..Q {
            f0[x * Q + q] = W[q] as f32;
        }
    }

    let cells_buffer = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("LBM Cells"),
            contents: bytemuck::cast_slice(&cells_u32),
            usage: wgpu::BufferUsages::STORAGE,
        });
    let f_buffers: [wgpu::Buffer; 2] = std::array::from_fn(|i| {
        ctx.device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(&format!("LBM f{i}")),
                contents: bytemuck::cast_slice(&f0),
                usage: wgpu::BufferUsages::STORAGE,
            })
    });
    let vel_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("LBM vel"),
        size: (n * 16) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let params_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("LBM params"),
        size: std::mem::size_of::<Params>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let staging = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("LBM staging"),
        size: (n * 16) as u64,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let shader = ctx
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("LBM Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/lbm.wgsl").into()),
        });
    let storage_entry = |binding: u32, read_only: bool| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };
    let layout = ctx
        .device
        .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("LBM BGL"),
            entries: &[
                storage_entry(0, true),
                storage_entry(1, false),
                storage_entry(2, false),
                storage_entry(3, false),
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
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
    let make_bind_group = |f_in: &wgpu::Buffer, f_out: &wgpu::Buffer| {
        ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("LBM BG"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: cells_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: f_in.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: f_out.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: vel_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: params_buffer.as_entire_binding(),
                },
            ],
        })
    };
    let bind_groups = [
        make_bind_group(&f_buffers[0], &f_buffers[1]),
        make_bind_group(&f_buffers[1], &f_buffers[0]),
    ];

    let pipeline_layout = ctx
        .device
        .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("LBM PL"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
    let make_pipeline = |entry: &str| {
        ctx.device
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
    };
    let collide = make_pipeline("collide");
    let stream = make_pipeline("stream");

    let workgroups = (n as u32).div_ceil(256);
    let (nx, ny, nz) = (
        model.divisions[0] as u32,
        model.divisions[1] as u32,
        model.divisions[2] as u32,
    );
    let periodic = [
        u32::from(model.periodic[0]),
        u32::from(model.periodic[1]),
        u32::from(model.periodic[2]),
        0,
    ];

    for step in 0..opts.steps {
        let s = if opts.ramp_steps == 0 {
            1.0f32
        } else {
            let t = (((step + 1) as f64 / opts.ramp_steps as f64).min(1.0)) as f32;
            t * t * (3.0 - 2.0 * t)
        };
        let params = Params {
            dims: [nx, ny, nz, step as u32],
            fluid: [omega, rho_out_lat, 0.0, 0.0],
            u_in: [u_in_lat[0] * s, u_in_lat[1] * s, u_in_lat[2] * s, 0.0],
            a_body: [a_body[0], a_body[1], a_body[2], 0.0],
            periodic,
        };
        ctx.queue
            .write_buffer(&params_buffer, 0, bytemuck::bytes_of(&params));
        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("LBM step"),
            });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("collide"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&collide);
            pass.set_bind_group(0, &bind_groups[step % 2], &[]);
            pass.dispatch_workgroups(workgroups, 1, 1);
        }
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("stream"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&stream);
            pass.set_bind_group(0, &bind_groups[step % 2], &[]);
            pass.dispatch_workgroups(workgroups, 1, 1);
        }
        ctx.queue.submit(Some(encoder.finish()));
    }

    // Read back the velocity/density field.
    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("LBM readback"),
        });
    encoder.copy_buffer_to_buffer(&vel_buffer, 0, &staging, 0, (n * 16) as u64);
    ctx.queue.submit(Some(encoder.finish()));
    let slice = staging.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    ctx.device.poll(wgpu::Maintain::Wait);
    rx.recv()
        .map_err(|_| PreviewError::Gpu(GpuError::BufferMapping))?
        .map_err(|_| PreviewError::Gpu(GpuError::BufferMapping))?;
    let data: Vec<f32> = bytemuck::cast_slice(&slice.get_mapped_range()).to_vec();
    staging.unmap();

    let vel_scale = (scaling.dx_m / scaling.dt_s) as f32;
    let p_scale = (CS2 * model.fluid.density_kg_m3 * c_lat * c_lat) as f32;
    let mut velocity = vec![[0.0f32; 3]; n];
    let mut pressure = vec![0.0f32; n];
    for x in 0..n {
        if model.cells[x] != Cell::Fluid {
            continue;
        }
        velocity[x] = [
            data[x * 4] * vel_scale,
            data[x * 4 + 1] * vel_scale,
            data[x * 4 + 2] * vel_scale,
        ];
        pressure[x] = (data[x * 4 + 3] - 1.0) * p_scale;
    }

    Ok(PreviewField {
        velocity_m_s: velocity,
        gauge_pressure_pa: pressure,
        steps: opts.steps,
        scaling,
        note: "preview lattice (f32 GPU, fixed steps, no steadiness detection) — not \
               claim-grade; the f64 CPU solve is the oracle",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Fluid;
    use crate::solve::{solve_steady, SolveOptions};

    /// CPU↔GPU parity: the preview lattice must match the claim-grade
    /// solver's steady state on a duct to f32 tolerance. Skips (with a
    /// note) on machines with no GPU adapter.
    #[test]
    fn gpu_preview_matches_cpu_steady_state() {
        let (nx, ny, nz) = (24usize, 7usize, 7usize);
        let mut m = FlowModel::new([0.0; 3], [24.0, 7.0, 7.0], [nx, ny, nz]);
        for k in 0..nz {
            for j in 0..ny {
                for i in 0..nx {
                    let x = m.index(i, j, k);
                    m.cells[x] = if i == 0 {
                        Cell::Inlet
                    } else if i == nx - 1 {
                        Cell::Outlet
                    } else {
                        Cell::Fluid
                    };
                }
            }
        }
        m.fluid = Fluid::AIR_20C;
        m.inlet_velocity_m_s = [0.05, 0.0, 0.0];

        let cpu = solve_steady(&m, &SolveOptions::default()).expect("cpu solve");
        let opts = PreviewOptions {
            steps: cpu.steps * 2,
            ramp_steps: 1000,
            u_ref_m_s: None,
        };
        let gpu = match preview_blocking(&m, &opts) {
            Ok(g) => g,
            Err(PreviewError::Gpu(e)) => {
                eprintln!("skipping GPU parity test (no adapter/context): {e}");
                return;
            }
            Err(e) => panic!("preview failed: {e}"),
        };

        let u_ref = 0.05f32;
        let mut max_rel = 0.0f32;
        for x in 0..m.cells.len() {
            if m.cells[x] != Cell::Fluid {
                continue;
            }
            for a in 0..3 {
                let d = (gpu.velocity_m_s[x][a] - cpu.velocity_m_s[x][a] as f32).abs();
                max_rel = max_rel.max(d / u_ref);
            }
        }
        assert!(
            max_rel < 2e-3,
            "GPU/CPU velocity parity: max relative gap {max_rel:.2e} (f32 budget 2e-3)"
        );
        assert!(
            gpu.note.contains("not \n               claim-grade")
                || gpu.note.contains("not claim-grade")
        );
    }
}
