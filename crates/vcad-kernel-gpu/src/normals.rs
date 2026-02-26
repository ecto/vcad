//! GPU-accelerated creased normal computation.

use crate::context::{GpuContext, GpuError};
use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

/// Parameters for the normal computation shader.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct NormalParams {
    crease_angle_cos: f32,
    vertex_count: u32,
    triangle_count: u32,
    _padding: u32,
}

/// Compute creased normals for a mesh on the GPU.
///
/// # Arguments
/// * `positions` - Flat array of vertex positions (x, y, z, x, y, z, ...)
/// * `indices` - Triangle indices (3 per triangle)
/// * `crease_angle` - Angle in radians; faces meeting at a sharper angle get hard edges
///
/// # Returns
/// Flat array of normals, same layout as positions.
pub async fn compute_creased_normals(
    positions: &[f32],
    indices: &[u32],
    crease_angle: f32,
) -> Result<Vec<f32>, GpuError> {
    if positions.is_empty() || indices.is_empty() {
        return Ok(vec![0.0f32; positions.len()]);
    }

    let ctx = GpuContext::init().await?;

    let vertex_count = (positions.len() / 3) as u32;
    let triangle_count = (indices.len() / 3) as u32;

    // Create buffers
    let position_buffer = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Position Buffer"),
            contents: bytemuck::cast_slice(positions),
            usage: wgpu::BufferUsages::STORAGE,
        });

    let index_buffer = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Index Buffer"),
            contents: bytemuck::cast_slice(indices),
            usage: wgpu::BufferUsages::STORAGE,
        });

    let normal_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Normal Buffer"),
        size: std::mem::size_of_val(positions) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });

    let face_normal_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Face Normal Buffer"),
        size: (triangle_count as usize * 3 * std::mem::size_of::<f32>()) as u64,
        usage: wgpu::BufferUsages::STORAGE,
        mapped_at_creation: false,
    });

    let params = NormalParams {
        crease_angle_cos: crease_angle.cos(),
        vertex_count,
        triangle_count,
        _padding: 0,
    };

    let params_buffer = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Params Buffer"),
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM,
        });

    // Create shader module
    let shader = ctx
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Normal Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/normals.wgsl").into()),
        });

    // Create bind group layout
    let bind_group_layout = ctx
        .device
        .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Normal Bind Group Layout"),
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
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
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

    let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Normal Bind Group"),
        layout: &bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: position_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: index_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: normal_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: face_normal_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: params_buffer.as_entire_binding(),
            },
        ],
    });

    let pipeline_layout = ctx
        .device
        .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Normal Pipeline Layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

    // Create pipelines for both phases
    let face_normal_pipeline =
        ctx.device
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("Face Normal Pipeline"),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: Some("compute_face_normals"),
                compilation_options: Default::default(),
                cache: None,
            });

    let accumulate_pipeline =
        ctx.device
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("Accumulate Normal Pipeline"),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: Some("accumulate_normals"),
                compilation_options: Default::default(),
                cache: None,
            });

    // Dispatch compute passes
    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Normal Compute Encoder"),
        });

    // Phase 1: Compute face normals
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("Face Normal Pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&face_normal_pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.dispatch_workgroups(triangle_count.div_ceil(256), 1, 1);
    }

    // Phase 2: Accumulate to vertices
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("Accumulate Normal Pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&accumulate_pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.dispatch_workgroups(vertex_count.div_ceil(256), 1, 1);
    }

    // Copy results to staging buffer
    let staging_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Staging Buffer"),
        size: std::mem::size_of_val(positions) as u64,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    encoder.copy_buffer_to_buffer(
        &normal_buffer,
        0,
        &staging_buffer,
        0,
        std::mem::size_of_val(positions) as u64,
    );

    ctx.queue.submit(std::iter::once(encoder.finish()));

    // Read back results
    let buffer_slice = staging_buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
        tx.send(result).unwrap();
    });

    ctx.device.poll(wgpu::Maintain::Wait);
    rx.recv().unwrap().map_err(|_| GpuError::BufferMapping)?;

    let data = buffer_slice.get_mapped_range();
    let normals: Vec<f32> = bytemuck::cast_slice(&data).to_vec();
    drop(data);
    staging_buffer.unmap();

    Ok(normals)
}

/// Compute creased normals synchronously (native only).
#[cfg(not(target_arch = "wasm32"))]
#[allow(dead_code)]
pub fn compute_creased_normals_blocking(
    positions: &[f32],
    indices: &[u32],
    crease_angle: f32,
) -> Result<Vec<f32>, GpuError> {
    pollster::block_on(compute_creased_normals(positions, indices, crease_angle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires GPU"]
    fn test_compute_normals() {
        // Simple triangle
        let positions = vec![
            0.0, 0.0, 0.0, // v0
            1.0, 0.0, 0.0, // v1
            0.5, 1.0, 0.0, // v2
        ];
        let indices = vec![0, 1, 2];

        let normals =
            compute_creased_normals_blocking(&positions, &indices, std::f32::consts::PI / 6.0);
        assert!(normals.is_ok());
        let normals = normals.unwrap();
        assert_eq!(normals.len(), positions.len());

        // Normal should point in +Z direction
        for i in 0..3 {
            let nz = normals[i * 3 + 2];
            assert!(nz > 0.9, "Expected +Z normal, got {}", nz);
        }
    }
}
