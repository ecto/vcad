//! GPU wavefront path-search over a multi-layer occupancy raster.
//!
//! Spike implementation of maze-routing distance fields on the GPU:
//! iterative Bellman-Ford-style relaxation over a 3D grid
//! (`nx x ny x layers`) with 8-way in-plane moves and vias between
//! adjacent layers. Ping-pong buffering keeps each sweep deterministic;
//! a `changed` flag lets the host stop once the field is settled.
//!
//! [`distance_field`] runs on the GPU (falling back to an error when no
//! adapter exists); [`distance_field_cpu`] is the exact-integer Dijkstra
//! reference used by tests and as a CPU fallback. [`extract_path`] walks
//! either field downhill from a goal back to a source.

use crate::context::{GpuContext, GpuError};
use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

/// Distance value marking an unreachable (or not-yet-reached) node.
pub const UNREACHABLE: u32 = u32::MAX;

/// A multi-layer routing grid with per-node occupancy.
///
/// Node `(x, y, l)` lives at index `(l * ny + y) * nx + x`.
pub struct WavefrontGrid {
    /// Grid width (cells along X).
    pub nx: usize,
    /// Grid height (cells along Y).
    pub ny: usize,
    /// Number of routing layers.
    pub layers: usize,
    /// Per-node blocked flag, length `nx * ny * layers`.
    pub blocked: Vec<bool>,
}

impl WavefrontGrid {
    /// Total node count (`nx * ny * layers`).
    pub fn len(&self) -> usize {
        self.nx * self.ny * self.layers
    }

    /// Whether the grid has zero nodes.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Node index for `(x, y, layer)`.
    pub fn index(&self, x: usize, y: usize, layer: usize) -> usize {
        (layer * self.ny + y) * self.nx + x
    }
}

/// Integer edge costs for wavefront expansion (e.g. cost x 1000).
pub struct WavefrontCosts {
    /// Cost of an orthogonal in-plane step.
    pub step: u32,
    /// Cost of a diagonal in-plane step.
    pub diag: u32,
    /// Cost of a via move to an adjacent layer.
    pub via: u32,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct WavefrontParams {
    nx: u32,
    ny: u32,
    nl: u32,
    step_cost: u32,
    diag_cost: u32,
    via_cost: u32,
    _pad0: u32,
    _pad1: u32,
}

/// How many relaxation sweeps to run between `changed`-flag readbacks.
const SWEEPS_PER_READBACK: usize = 32;

/// Multi-source distance field from `sources` (node indices), computed on
/// the GPU. Returns one `u32` distance per node ([`UNREACHABLE`] for nodes
/// no source can reach). Blocked nodes stay unreachable unless they are
/// sources themselves.
#[cfg(not(target_arch = "wasm32"))]
pub fn distance_field(
    grid: &WavefrontGrid,
    sources: &[usize],
    costs: &WavefrontCosts,
) -> Result<Vec<u32>, GpuError> {
    pollster::block_on(distance_field_async(grid, sources, costs))
}

/// Async variant of [`distance_field`] (usable on WASM, where blocking on
/// the GPU is not possible).
pub async fn distance_field_async(
    grid: &WavefrontGrid,
    sources: &[usize],
    costs: &WavefrontCosts,
) -> Result<Vec<u32>, GpuError> {
    let total = grid.len();
    if total == 0 {
        return Ok(Vec::new());
    }

    let ctx = GpuContext::init().await?;

    let occupancy: Vec<u32> = grid.blocked.iter().map(|&b| u32::from(b)).collect();
    let mut init_dist = vec![UNREACHABLE; total];
    for &s in sources {
        init_dist[s] = 0;
    }

    let byte_len = (total * std::mem::size_of::<u32>()) as u64;

    let occupancy_buffer = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Wavefront Occupancy Buffer"),
            contents: bytemuck::cast_slice(&occupancy),
            usage: wgpu::BufferUsages::STORAGE,
        });

    // Ping-pong distance buffers.
    let dist_buffers: [wgpu::Buffer; 2] = std::array::from_fn(|i| {
        ctx.device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(&format!("Wavefront Dist Buffer {i}")),
                contents: bytemuck::cast_slice(&init_dist),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            })
    });

    let changed_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Wavefront Changed Buffer"),
        size: std::mem::size_of::<u32>() as u64,
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_SRC
            | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let params = WavefrontParams {
        nx: grid.nx as u32,
        ny: grid.ny as u32,
        nl: grid.layers as u32,
        step_cost: costs.step,
        diag_cost: costs.diag,
        via_cost: costs.via,
        _pad0: 0,
        _pad1: 0,
    };

    let params_buffer = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Wavefront Params Buffer"),
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM,
        });

    let shader = ctx
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Wavefront Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/wavefront.wgsl").into()),
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

    let bind_group_layout = ctx
        .device
        .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Wavefront Bind Group Layout"),
            entries: &[
                storage_entry(0, true),
                storage_entry(1, true),
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

    // Two bind groups: (in=0, out=1) and (in=1, out=0).
    let make_bind_group = |input: &wgpu::Buffer, output: &wgpu::Buffer| {
        ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Wavefront Bind Group"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: occupancy_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: input.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: output.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: changed_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: params_buffer.as_entire_binding(),
                },
            ],
        })
    };
    let bind_groups = [
        make_bind_group(&dist_buffers[0], &dist_buffers[1]),
        make_bind_group(&dist_buffers[1], &dist_buffers[0]),
    ];

    let pipeline_layout = ctx
        .device
        .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Wavefront Pipeline Layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

    let pipeline = ctx
        .device
        .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Wavefront Relax Pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("relax"),
            compilation_options: Default::default(),
            cache: None,
        });

    let changed_staging = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Wavefront Changed Staging"),
        size: std::mem::size_of::<u32>() as u64,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let workgroups = (total as u32).div_ceil(256);
    // Each sweep propagates the frontier by at least one node, so `total`
    // sweeps is a hard upper bound on convergence.
    let max_sweeps = total;

    let mut sweeps_done = 0usize;
    let mut current = 0usize; // which buffer holds the latest field
    while sweeps_done < max_sweeps {
        let batch = SWEEPS_PER_READBACK.min(max_sweeps - sweeps_done);

        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Wavefront Sweep Encoder"),
            });
        // Clear the changed flag, run a batch of ping-pong sweeps, then
        // read the flag back once.
        encoder.clear_buffer(&changed_buffer, 0, None);
        for i in 0..batch {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Wavefront Relax Pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bind_groups[(current + i) % 2], &[]);
            pass.dispatch_workgroups(workgroups, 1, 1);
        }
        encoder.copy_buffer_to_buffer(
            &changed_buffer,
            0,
            &changed_staging,
            0,
            std::mem::size_of::<u32>() as u64,
        );
        ctx.queue.submit(std::iter::once(encoder.finish()));

        current = (current + batch) % 2;
        sweeps_done += batch;

        let changed = read_u32(ctx, &changed_staging)?;
        if changed == 0 {
            break;
        }
    }

    // Read back the settled distance field.
    let dist_staging = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Wavefront Dist Staging"),
        size: byte_len,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Wavefront Readback Encoder"),
        });
    encoder.copy_buffer_to_buffer(&dist_buffers[current], 0, &dist_staging, 0, byte_len);
    ctx.queue.submit(std::iter::once(encoder.finish()));

    let buffer_slice = dist_staging.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
        tx.send(result).unwrap();
    });
    ctx.device.poll(wgpu::Maintain::Wait);
    rx.recv()
        .map_err(|_| GpuError::BufferMapping)?
        .map_err(|_| GpuError::BufferMapping)?;

    let data = buffer_slice.get_mapped_range();
    let dist: Vec<u32> = bytemuck::cast_slice(&data).to_vec();
    drop(data);
    dist_staging.unmap();

    Ok(dist)
}

fn read_u32(ctx: &GpuContext, staging: &wgpu::Buffer) -> Result<u32, GpuError> {
    let slice = staging.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        tx.send(result).unwrap();
    });
    ctx.device.poll(wgpu::Maintain::Wait);
    rx.recv()
        .map_err(|_| GpuError::BufferMapping)?
        .map_err(|_| GpuError::BufferMapping)?;
    let data = slice.get_mapped_range();
    let value = bytemuck::cast_slice::<u8, u32>(&data)[0];
    drop(data);
    staging.unmap();
    Ok(value)
}

/// Neighbor moves of a node: `(neighbor index, edge cost)`.
fn neighbors(
    grid: &WavefrontGrid,
    idx: usize,
    costs: &WavefrontCosts,
) -> impl Iterator<Item = (usize, u32)> {
    let nx = grid.nx;
    let ny = grid.ny;
    let layers = grid.layers;
    let x = (idx % nx) as isize;
    let y = ((idx / nx) % ny) as isize;
    let l = idx / (nx * ny);

    const IN_PLANE: [(isize, isize); 8] = [
        (-1, 0),
        (1, 0),
        (0, -1),
        (0, 1),
        (-1, -1),
        (1, -1),
        (-1, 1),
        (1, 1),
    ];

    let plane_base = l * nx * ny;
    let step = costs.step;
    let diag = costs.diag;
    let via = costs.via;
    let in_plane = IN_PLANE.into_iter().filter_map(move |(dx, dy)| {
        let (px, py) = (x + dx, y + dy);
        if px < 0 || py < 0 || px as usize >= nx || py as usize >= ny {
            return None;
        }
        let cost = if dx != 0 && dy != 0 { diag } else { step };
        Some((plane_base + py as usize * nx + px as usize, cost))
    });

    let cell = idx % (nx * ny);
    let down = (l > 0).then(|| ((l - 1) * nx * ny + cell, via));
    let up = (l + 1 < layers).then(|| ((l + 1) * nx * ny + cell, via));

    in_plane.chain(down).chain(up)
}

/// CPU reference implementation of [`distance_field`]: Dijkstra over the
/// same integer costs. Used by tests and as a fallback when no GPU adapter
/// is available.
pub fn distance_field_cpu(
    grid: &WavefrontGrid,
    sources: &[usize],
    costs: &WavefrontCosts,
) -> Vec<u32> {
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;

    let mut dist = vec![UNREACHABLE; grid.len()];
    let mut heap = BinaryHeap::new();
    for &s in sources {
        dist[s] = 0;
        heap.push(Reverse((0u32, s)));
    }

    while let Some(Reverse((d, idx))) = heap.pop() {
        if d > dist[idx] {
            continue;
        }
        for (nb, cost) in neighbors(grid, idx, costs) {
            if grid.blocked[nb] {
                continue;
            }
            let nd = d.saturating_add(cost);
            if nd < dist[nb] {
                dist[nb] = nd;
                heap.push(Reverse((nd, nb)));
            }
        }
    }

    dist
}

/// Walk a distance field downhill from `goal` back to a source.
///
/// Returns node indices from `goal` to a source (inclusive), or `None`
/// when `goal` is unreachable. Relies on the costs being exact integers:
/// a predecessor is any neighbor with `dist[nb] + edge_cost == dist[cur]`.
pub fn extract_path(
    grid: &WavefrontGrid,
    dist: &[u32],
    goal: usize,
    costs: &WavefrontCosts,
) -> Option<Vec<usize>> {
    if dist[goal] == UNREACHABLE {
        return None;
    }

    let mut path = vec![goal];
    let mut cur = goal;
    while dist[cur] != 0 {
        let prev = neighbors(grid, cur, costs)
            .find(|&(nb, cost)| dist[nb] != UNREACHABLE && dist[nb] + cost == dist[cur])?;
        cur = prev.0;
        path.push(cur);
    }
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    const COSTS: WavefrontCosts = WavefrontCosts {
        step: 1000,
        diag: 1414,
        via: 5000,
    };

    fn grid(nx: usize, ny: usize, layers: usize) -> WavefrontGrid {
        WavefrontGrid {
            nx,
            ny,
            layers,
            blocked: vec![false; nx * ny * layers],
        }
    }

    #[test]
    fn cpu_empty_grid_straight_line() {
        let g = grid(10, 1, 1);
        let dist = distance_field_cpu(&g, &[0], &COSTS);
        for x in 0..10 {
            assert_eq!(dist[x], x as u32 * 1000);
        }
    }

    #[test]
    fn cpu_diagonal_costs() {
        let g = grid(5, 5, 1);
        let dist = distance_field_cpu(&g, &[g.index(0, 0, 0)], &COSTS);
        // Pure diagonal to (3,3).
        assert_eq!(dist[g.index(3, 3, 0)], 3 * 1414);
        // Octile: (4,2) = 2 diagonals + 2 steps.
        assert_eq!(dist[g.index(4, 2, 0)], 2 * 1414 + 2 * 1000);
    }

    /// Wall across layer 0 forces a via detour; dist reflects via cost.
    fn walled_grid() -> WavefrontGrid {
        let mut g = grid(9, 5, 3);
        for y in 0..5 {
            let idx = g.index(4, y, 0);
            g.blocked[idx] = true;
        }
        g
    }

    #[test]
    fn cpu_wall_forces_via_detour() {
        let g = walled_grid();
        let src = g.index(0, 2, 0);
        let goal = g.index(8, 2, 0);
        let dist = distance_field_cpu(&g, &[src], &COSTS);
        // Straight line on layer 0 would be 8 * 1000; the wall forces two
        // vias (down/up) plus the 8 in-plane steps on layer 1.
        assert_eq!(dist[goal], 8 * 1000 + 2 * 5000);
        // The wall itself is unreachable.
        assert_eq!(dist[g.index(4, 2, 0)], UNREACHABLE);
    }

    #[test]
    fn cpu_extract_path_monotone_and_valid() {
        let g = walled_grid();
        let src = g.index(0, 2, 0);
        let goal = g.index(8, 2, 0);
        let dist = distance_field_cpu(&g, &[src], &COSTS);
        let path = extract_path(&g, &dist, goal, &COSTS).expect("path exists");

        assert_eq!(*path.first().unwrap(), goal);
        assert_eq!(*path.last().unwrap(), src);
        for pair in path.windows(2) {
            // Monotone decreasing along goal -> source.
            assert!(dist[pair[0]] > dist[pair[1]]);
            // Each hop is a legal move with matching edge cost.
            let edge = neighbors(&g, pair[0], &COSTS).find(|&(nb, _)| nb == pair[1]);
            let (_, cost) = edge.expect("consecutive path nodes are neighbors");
            assert_eq!(dist[pair[1]] + cost, dist[pair[0]]);
            assert!(!g.blocked[pair[1]]);
        }
        // The detour uses another layer.
        assert!(path.iter().any(|&idx| idx / (g.nx * g.ny) != 0));
    }

    #[test]
    fn cpu_unreachable_goal_returns_none() {
        let mut g = grid(5, 1, 1);
        g.blocked[2] = true;
        let dist = distance_field_cpu(&g, &[0], &COSTS);
        assert_eq!(dist[4], UNREACHABLE);
        assert!(extract_path(&g, &dist, 4, &COSTS).is_none());
    }

    #[test]
    fn gpu_matches_cpu_on_walled_grid() {
        let g = walled_grid();
        let src = g.index(0, 2, 0);
        let gpu = match distance_field(&g, &[src], &COSTS) {
            Ok(d) => d,
            Err(GpuError::NoAdapter) => return, // headless CI without a GPU
            Err(e) => panic!("GPU error: {e}"),
        };
        let cpu = distance_field_cpu(&g, &[src], &COSTS);
        assert_eq!(gpu, cpu);
    }

    /// Rough GPU-vs-CPU timing on a router-scale grid. Run manually:
    /// `cargo test -p vcad-kernel-gpu --release -- --ignored bench_wavefront --nocapture`
    #[test]
    #[ignore = "benchmark; run manually with --ignored"]
    fn bench_wavefront_400x400x10() {
        let mut g = grid(400, 400, 10);
        let mut state = 0x9e3779b9u32;
        let mut rand = move || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state
        };
        for b in g.blocked.iter_mut() {
            *b = rand() % 4 == 0;
        }
        let src = g.index(0, 0, 0);
        g.blocked[src] = false;

        let t0 = std::time::Instant::now();
        let gpu = distance_field(&g, &[src], &COSTS).expect("gpu");
        let gpu_time = t0.elapsed();
        let t1 = std::time::Instant::now();
        let cpu = distance_field_cpu(&g, &[src], &COSTS);
        let cpu_time = t1.elapsed();
        assert_eq!(gpu, cpu);
        println!("400x400x10: gpu={gpu_time:?} cpu={cpu_time:?}");
    }

    #[test]
    fn gpu_matches_cpu_multi_source_random_obstacles() {
        // Deterministic pseudo-random obstacles (xorshift), 3 layers.
        let mut g = grid(32, 24, 3);
        let mut state = 0x2545f491u32;
        let mut rand = move || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state
        };
        for b in g.blocked.iter_mut() {
            *b = rand() % 5 == 0;
        }
        let sources: Vec<usize> = [(1, 1, 0), (30, 22, 2)]
            .iter()
            .map(|&(x, y, l)| (l * g.ny + y) * g.nx + x)
            .collect();
        for &s in &sources {
            g.blocked[s] = false;
        }

        let gpu = match distance_field(&g, &sources, &COSTS) {
            Ok(d) => d,
            Err(GpuError::NoAdapter) => return,
            Err(e) => panic!("GPU error: {e}"),
        };
        let cpu = distance_field_cpu(&g, &sources, &COSTS);
        assert_eq!(gpu, cpu);
    }
}
