//! GPU-accelerated mesh decimation using quadric error metrics.

use crate::{GpuContext, GpuError};
use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

/// Result of mesh decimation.
#[derive(Debug, Clone)]
pub struct DecimationResult {
    /// Decimated vertex positions (flat array: x, y, z, x, y, z, ...).
    pub positions: Vec<f32>,
    /// Triangle indices for the decimated mesh.
    pub indices: Vec<u32>,
    /// Vertex normals (recomputed after decimation).
    pub normals: Vec<f32>,
}

/// Parameters for the decimation shader.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct DecimationParams {
    vertex_count: u32,
    triangle_count: u32,
    edge_count: u32,
    target_triangles: u32,
}

/// Decimate a mesh to reduce triangle count.
///
/// Uses quadric error metrics to minimize visual deviation while reducing
/// polygon count. This is useful for generating LOD (level of detail) meshes.
///
/// # Arguments
/// * `positions` - Flat array of vertex positions
/// * `indices` - Triangle indices
/// * `target_ratio` - Target ratio of triangles to keep (0.5 = 50%)
///
/// # Returns
/// Decimated mesh with positions, indices, and recomputed normals.
pub async fn decimate_mesh(
    positions: &[f32],
    indices: &[u32],
    target_ratio: f32,
) -> Result<DecimationResult, GpuError> {
    let ctx = GpuContext::init().await?;

    let vertex_count = (positions.len() / 3) as u32;
    let triangle_count = (indices.len() / 3) as u32;
    let target_triangles = ((triangle_count as f32) * target_ratio.clamp(0.1, 1.0)) as u32;

    // For a proper GPU decimation we'd need multiple passes.
    // This is a simplified version that does CPU-side decimation with GPU acceleration
    // for the quadric computation phase.

    // Build edge list from triangles
    let edges = build_edge_list(indices, vertex_count);
    let edge_count = edges.len() as u32;

    // Prepare combined index buffer (triangles + edges)
    let mut combined_indices = indices.to_vec();
    for (v0, v1) in &edges {
        combined_indices.push(*v0);
        combined_indices.push(*v1);
    }

    // Create GPU buffers
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
            contents: bytemuck::cast_slice(&combined_indices),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        });

    let quadric_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Quadric Buffer"),
        size: (vertex_count as usize * 10 * std::mem::size_of::<f32>()) as u64,
        usage: wgpu::BufferUsages::STORAGE,
        mapped_at_creation: false,
    });

    let edge_cost_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Edge Cost Buffer"),
        size: (edge_count as usize * std::mem::size_of::<f32>()) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });

    let edge_target_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Edge Target Buffer"),
        size: (edge_count as usize * 3 * std::mem::size_of::<f32>()) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });

    let collapse_flag_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Collapse Flag Buffer"),
        size: (edge_count as usize * std::mem::size_of::<u32>()) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });

    let params = DecimationParams {
        vertex_count,
        triangle_count,
        edge_count,
        target_triangles,
    };

    let params_buffer = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Params Buffer"),
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM,
        });

    // Create shader and pipelines
    let shader = ctx
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Decimation Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/decimate.wgsl").into()),
        });

    let bind_group_layout = ctx
        .device
        .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Decimation Bind Group Layout"),
            entries: &[
                buffer_layout_entry(0, true),  // positions
                buffer_layout_entry(1, false), // indices
                buffer_layout_entry(2, false), // quadrics
                buffer_layout_entry(3, false), // edge_costs
                buffer_layout_entry(4, false), // edge_targets
                buffer_layout_entry(5, false), // collapse_flags
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
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
        label: Some("Decimation Bind Group"),
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
                resource: quadric_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: edge_cost_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: edge_target_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: collapse_flag_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: params_buffer.as_entire_binding(),
            },
        ],
    });

    let pipeline_layout = ctx
        .device
        .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Decimation Pipeline Layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

    let init_pipeline =
        create_compute_pipeline(&ctx.device, &pipeline_layout, &shader, "init_quadrics");
    let accumulate_pipeline = create_compute_pipeline(
        &ctx.device,
        &pipeline_layout,
        &shader,
        "accumulate_quadrics",
    );
    let cost_pipeline =
        create_compute_pipeline(&ctx.device, &pipeline_layout, &shader, "compute_edge_costs");

    // Run GPU compute passes
    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Decimation Encoder"),
        });

    // Phase 1: Init quadrics
    dispatch_compute(
        &mut encoder,
        &init_pipeline,
        &bind_group,
        vertex_count,
        "Init Quadrics",
    );

    // Phase 2: Accumulate quadrics from faces
    dispatch_compute(
        &mut encoder,
        &accumulate_pipeline,
        &bind_group,
        triangle_count,
        "Accumulate Quadrics",
    );

    // Phase 3: Compute edge costs
    dispatch_compute(
        &mut encoder,
        &cost_pipeline,
        &bind_group,
        edge_count,
        "Compute Edge Costs",
    );

    // Read back edge costs
    let cost_staging = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Cost Staging"),
        size: (edge_count as usize * std::mem::size_of::<f32>()) as u64,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    encoder.copy_buffer_to_buffer(
        &edge_cost_buffer,
        0,
        &cost_staging,
        0,
        (edge_count as usize * std::mem::size_of::<f32>()) as u64,
    );

    ctx.queue.submit(std::iter::once(encoder.finish()));

    // Read edge costs
    let edge_costs = read_buffer::<f32>(&ctx.device, &cost_staging, edge_count as usize).await?;

    // CPU-side decimation using GPU-computed costs
    let (decimated_positions, decimated_indices) = cpu_decimate(
        positions,
        indices,
        &edges,
        &edge_costs,
        target_triangles as usize,
    );

    // Recompute normals for decimated mesh
    let normals = crate::compute_creased_normals(
        &decimated_positions,
        &decimated_indices,
        std::f32::consts::PI / 6.0,
    )
    .await?;

    Ok(DecimationResult {
        positions: decimated_positions,
        indices: decimated_indices,
        normals,
    })
}

/// Decimate a mesh synchronously (native only).
#[cfg(not(target_arch = "wasm32"))]
#[allow(dead_code)]
pub fn decimate_mesh_blocking(
    positions: &[f32],
    indices: &[u32],
    target_ratio: f32,
) -> Result<DecimationResult, GpuError> {
    pollster::block_on(decimate_mesh(positions, indices, target_ratio))
}

fn buffer_layout_entry(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn create_compute_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    entry_point: &str,
) -> wgpu::ComputePipeline {
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(entry_point),
        layout: Some(layout),
        module: shader,
        entry_point: Some(entry_point),
        compilation_options: Default::default(),
        cache: None,
    })
}

fn dispatch_compute(
    encoder: &mut wgpu::CommandEncoder,
    pipeline: &wgpu::ComputePipeline,
    bind_group: &wgpu::BindGroup,
    count: u32,
    label: &str,
) {
    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
        label: Some(label),
        timestamp_writes: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, bind_group, &[]);
    pass.dispatch_workgroups(count.div_ceil(256), 1, 1);
}

async fn read_buffer<T: Pod>(
    device: &wgpu::Device,
    buffer: &wgpu::Buffer,
    _count: usize,
) -> Result<Vec<T>, GpuError> {
    let slice = buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        tx.send(result).unwrap();
    });

    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    rx.recv().unwrap().map_err(|_| GpuError::BufferMapping)?;

    let data = slice
        .get_mapped_range()
        .expect("the buffer was just mapped");
    let result: Vec<T> = bytemuck::cast_slice(&data).to_vec();
    drop(data);
    buffer.unmap();

    Ok(result)
}

fn build_edge_list(indices: &[u32], _vertex_count: u32) -> Vec<(u32, u32)> {
    use std::collections::HashSet;

    let mut edge_set = HashSet::new();

    for tri in indices.chunks(3) {
        if tri.len() < 3 {
            continue;
        }
        let (i0, i1, i2) = (tri[0], tri[1], tri[2]);

        // Add edges in canonical order (smaller index first)
        edge_set.insert(if i0 < i1 { (i0, i1) } else { (i1, i0) });
        edge_set.insert(if i1 < i2 { (i1, i2) } else { (i2, i1) });
        edge_set.insert(if i2 < i0 { (i2, i0) } else { (i0, i2) });
    }

    edge_set.into_iter().collect()
}

fn cpu_decimate(
    positions: &[f32],
    indices: &[u32],
    edges: &[(u32, u32)],
    edge_costs: &[f32],
    target_triangles: usize,
) -> (Vec<f32>, Vec<u32>) {
    use std::collections::{BinaryHeap, HashMap};

    // Build priority queue of edges by cost
    #[derive(PartialEq)]
    struct EdgeEntry {
        cost: f32,
        edge_idx: usize,
    }

    impl Eq for EdgeEntry {}

    impl Ord for EdgeEntry {
        fn cmp(&self, other: &Self) -> std::cmp::Ordering {
            // Reverse order for min-heap
            other
                .cost
                .partial_cmp(&self.cost)
                .unwrap_or(std::cmp::Ordering::Equal)
        }
    }

    impl PartialOrd for EdgeEntry {
        fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
            Some(self.cmp(other))
        }
    }

    let mut heap: BinaryHeap<EdgeEntry> = edges
        .iter()
        .enumerate()
        .map(|(i, _)| EdgeEntry {
            cost: edge_costs[i],
            edge_idx: i,
        })
        .collect();

    // Vertex remapping (for collapsed vertices)
    let vertex_count = positions.len() / 3;
    let mut vertex_map: Vec<u32> = (0..vertex_count as u32).collect();
    let mut positions = positions.to_vec();
    let mut active_triangles: Vec<bool> = vec![true; indices.len() / 3];
    let mut current_triangle_count = active_triangles.len();

    // Collapse edges until we reach target
    while current_triangle_count > target_triangles {
        let Some(entry) = heap.pop() else {
            break;
        };

        if entry.cost >= 1e20 {
            break;
        }

        let (v0, v1) = edges[entry.edge_idx];

        // Find canonical vertices
        let mut v0_canon = v0;
        while vertex_map[v0_canon as usize] != v0_canon {
            v0_canon = vertex_map[v0_canon as usize];
        }
        let mut v1_canon = v1;
        while vertex_map[v1_canon as usize] != v1_canon {
            v1_canon = vertex_map[v1_canon as usize];
        }

        if v0_canon == v1_canon {
            continue; // Already collapsed
        }

        // Collapse v1 into v0
        vertex_map[v1_canon as usize] = v0_canon;

        // Update v0 position to midpoint
        let base0 = (v0_canon as usize) * 3;
        let base1 = (v1_canon as usize) * 3;
        positions[base0] = (positions[base0] + positions[base1]) / 2.0;
        positions[base0 + 1] = (positions[base0 + 1] + positions[base1 + 1]) / 2.0;
        positions[base0 + 2] = (positions[base0 + 2] + positions[base1 + 2]) / 2.0;

        // Mark degenerate triangles
        for (tri_idx, active) in active_triangles.iter_mut().enumerate() {
            if !*active {
                continue;
            }

            let base = tri_idx * 3;
            let tri_verts = [
                get_canonical(&vertex_map, indices[base]),
                get_canonical(&vertex_map, indices[base + 1]),
                get_canonical(&vertex_map, indices[base + 2]),
            ];

            // Check for degenerate triangle
            if tri_verts[0] == tri_verts[1]
                || tri_verts[1] == tri_verts[2]
                || tri_verts[2] == tri_verts[0]
            {
                *active = false;
                current_triangle_count -= 1;
            }
        }
    }

    // Build output mesh
    let mut new_vertex_map: HashMap<u32, u32> = HashMap::new();
    let mut new_positions = Vec::new();
    let mut new_indices = Vec::new();

    for (tri_idx, active) in active_triangles.iter().enumerate() {
        if !*active {
            continue;
        }

        let base = tri_idx * 3;
        for i in 0..3 {
            let old_idx = get_canonical(&vertex_map, indices[base + i]);
            let new_idx = *new_vertex_map.entry(old_idx).or_insert_with(|| {
                let idx = (new_positions.len() / 3) as u32;
                let old_base = (old_idx as usize) * 3;
                new_positions.push(positions[old_base]);
                new_positions.push(positions[old_base + 1]);
                new_positions.push(positions[old_base + 2]);
                idx
            });
            new_indices.push(new_idx);
        }
    }

    (new_positions, new_indices)
}

fn get_canonical(vertex_map: &[u32], mut idx: u32) -> u32 {
    while vertex_map[idx as usize] != idx {
        idx = vertex_map[idx as usize];
    }
    idx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_edge_list() {
        let indices = vec![0, 1, 2, 0, 2, 3];
        let edges = build_edge_list(&indices, 4);
        assert_eq!(edges.len(), 5); // 5 unique edges for 2 triangles sharing an edge
    }

    #[test]
    #[ignore = "requires GPU"]
    fn test_decimate_mesh() {
        // Simple quad (2 triangles)
        let positions = vec![
            0.0, 0.0, 0.0, // v0
            1.0, 0.0, 0.0, // v1
            1.0, 1.0, 0.0, // v2
            0.0, 1.0, 0.0, // v3
        ];
        let indices = vec![0, 1, 2, 0, 2, 3];

        let result = decimate_mesh_blocking(&positions, &indices, 0.5);
        assert!(result.is_ok());
    }
}
