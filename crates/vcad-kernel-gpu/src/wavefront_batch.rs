//! Batched multi-search wavefront fields (GPU-router charter M1).
//!
//! Relax N independent searches in one dispatch stream against a shared,
//! resident class raster ([`crate::router_state`]). Each search carries an
//! own-net overlay bitset (its copper is passable and seeds at distance 0
//! alongside its explicit sources). Returns one distance field per search;
//! path extraction and exact validation stay on the CPU per the charter's
//! invariant (GPU proposes, oracle disposes).

use crate::context::{GpuContext, GpuError};
use crate::router_state::pack_u8_words;
use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

/// Distance value for unreached nodes.
pub const UNREACHABLE: u32 = u32::MAX;

/// Integer step costs (scaled, e.g. x1000).
#[derive(Debug, Clone, Copy)]
pub struct BatchCosts {
    /// Orthogonal in-plane step.
    pub step: u32,
    /// Diagonal in-plane step.
    pub diag: u32,
    /// Via to an adjacent layer.
    pub via: u32,
}

/// One search of a batch.
#[derive(Debug, Clone, Default)]
pub struct BatchSearch {
    /// Seed nodes at distance 0 (start pad cells and multi-source tree
    /// copper).
    pub sources: Vec<u32>,
    /// Own-net copper nodes: passable regardless of the shared raster.
    /// (Sources are implicitly overlaid too.)
    pub overlay: Vec<u32>,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct BatchParams {
    nx: u32,
    ny: u32,
    nl: u32,
    n_searches: u32,
    step_cost: u32,
    diag_cost: u32,
    via_cost: u32,
    _pad: u32,
}

/// Sweeps between changed-flag readbacks.
const SWEEPS_PER_READBACK: usize = 32;

/// Compute distance fields for `searches` against one shared occupancy
/// raster (`states`: layer-major CELL_* bytes; 0 = blocked). Synchronous
/// wrapper; native only.
#[cfg(not(target_arch = "wasm32"))]
pub fn distance_fields_batch(
    dims: (usize, usize, usize),
    states: &[u8],
    searches: &[BatchSearch],
    costs: &BatchCosts,
) -> Result<Vec<Vec<u32>>, GpuError> {
    pollster::block_on(distance_fields_batch_async(dims, states, searches, costs))
}

/// Async batched distance fields; see [`distance_fields_batch`].
pub async fn distance_fields_batch_async(
    dims: (usize, usize, usize),
    states: &[u8],
    searches: &[BatchSearch],
    costs: &BatchCosts,
) -> Result<Vec<Vec<u32>>, GpuError> {
    let (nx, ny, nl) = dims;
    let total = nx * ny * nl;
    let n = searches.len();
    if total == 0 || n == 0 {
        return Ok(vec![Vec::new(); n]);
    }
    if states.len() != total {
        return Err(GpuError::InvalidInput(format!(
            "states length {} != nodes {total}",
            states.len()
        )));
    }
    let ctx = GpuContext::init().await?;

    // Shared raster, u8 packed in u32 words.
    let occ_words = pack_u8_words(states);
    let occ_buf = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("batch-occupancy"),
            contents: bytemuck::cast_slice(&occ_words),
            usage: wgpu::BufferUsages::STORAGE,
        });

    // Overlay bitsets + initial distances.
    let bit_words = (n * total).div_ceil(32);
    let mut overlay = vec![0u32; bit_words];
    let mut init_dist = vec![UNREACHABLE; n * total];
    for (si, s) in searches.iter().enumerate() {
        for &node in s.overlay.iter().chain(s.sources.iter()) {
            let node = node as usize;
            if node >= total {
                return Err(GpuError::InvalidInput(format!(
                    "search {si}: node {node} out of range {total}"
                )));
            }
            let bit = si * total + node;
            overlay[bit / 32] |= 1 << (bit % 32);
        }
        for &node in &s.sources {
            init_dist[si * total + node as usize] = 0;
        }
    }
    let overlay_buf = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("batch-overlay"),
            contents: bytemuck::cast_slice(&overlay),
            usage: wgpu::BufferUsages::STORAGE,
        });

    let dist_bytes = (n * total * 4) as u64;
    let mk_dist = |label: &str| {
        ctx.device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: bytemuck::cast_slice(&init_dist),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            })
    };
    let dist_a = mk_dist("batch-dist-a");
    let dist_b = mk_dist("batch-dist-b");
    let _ = dist_bytes;

    let changed_buf = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("batch-changed"),
        size: (n * 4) as u64,
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_DST
            | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });

    let params = BatchParams {
        nx: nx as u32,
        ny: ny as u32,
        nl: nl as u32,
        n_searches: n as u32,
        step_cost: costs.step,
        diag_cost: costs.diag,
        via_cost: costs.via,
        _pad: 0,
    };
    let params_buf = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("batch-params"),
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM,
        });

    let shader = ctx
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("wavefront-batch"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/wavefront_batch.wgsl").into()),
        });
    let pipeline = ctx
        .device
        .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("relax-batch"),
            layout: None,
            module: &shader,
            entry_point: Some("relax_batch"),
            compilation_options: Default::default(),
            cache: None,
        });
    let layout = pipeline.get_bind_group_layout(0);
    let bind = |a: &wgpu::Buffer, b: &wgpu::Buffer| {
        ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("batch-bind"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: occ_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: overlay_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: a.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: b.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: changed_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: params_buf.as_entire_binding(),
                },
            ],
        })
    };
    let bind_ab = bind(&dist_a, &dist_b);
    let bind_ba = bind(&dist_b, &dist_a);

    let groups = ((n * total) as u32).div_ceil(256);
    // Worst-case sweep bound: longest shortest path < total nodes.
    let max_sweeps = 4 * (nx + ny + nl);
    let mut sweeps_done = 0usize;
    let mut current_is_a = true;
    'outer: while sweeps_done < max_sweeps {
        // Zero the changed flags, run K sweeps, read the flags once.
        ctx.queue.write_buffer(&changed_buf, 0, &vec![0u8; n * 4]);
        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("batch-sweeps"),
            });
        for _ in 0..SWEEPS_PER_READBACK {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("relax"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, if current_is_a { &bind_ab } else { &bind_ba }, &[]);
            pass.dispatch_workgroups(groups, 1, 1);
            drop(pass);
            current_is_a = !current_is_a;
            sweeps_done += 1;
        }
        ctx.queue.submit([encoder.finish()]);

        // Any search still changing?
        let flags = read_back_u32(ctx, &changed_buf, n)?;
        if flags.iter().all(|&f| f == 0) {
            break 'outer;
        }
    }

    // Read the settled field (the buffer last WRITTEN: after an even number
    // of sweeps current_is_a is back to true meaning A is next input == last
    // output was A).
    let final_buf = if current_is_a { &dist_a } else { &dist_b };
    let flat = read_back_u32(ctx, final_buf, n * total)?;
    Ok(flat.chunks(total).map(|c| c.to_vec()).collect())
}

fn read_back_u32(ctx: &GpuContext, src: &wgpu::Buffer, words: usize) -> Result<Vec<u32>, GpuError> {
    let size = (words * 4) as u64;
    let staging = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("batch-readback"),
        size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut enc = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    enc.copy_buffer_to_buffer(src, 0, &staging, 0, size);
    ctx.queue.submit([enc.finish()]);
    let slice = staging.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    ctx.device.poll(wgpu::Maintain::Wait);
    rx.recv()
        .map_err(|_| GpuError::BufferMapping)?
        .map_err(|_| GpuError::BufferMapping)?;
    let out: Vec<u32> = bytemuck::cast_slice(&slice.get_mapped_range()).to_vec();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CPU Dijkstra reference over the same semantics (states + overlay).
    fn cpu_field(
        dims: (usize, usize, usize),
        states: &[u8],
        search: &BatchSearch,
        costs: &BatchCosts,
    ) -> Vec<u32> {
        let (nx, ny, nl) = dims;
        let total = nx * ny * nl;
        let mut passable: Vec<bool> = states.iter().map(|&s| s != 0).collect();
        for &node in search.overlay.iter().chain(search.sources.iter()) {
            passable[node as usize] = true;
        }
        let mut dist = vec![UNREACHABLE; total];
        let mut heap = std::collections::BinaryHeap::new();
        for &s in &search.sources {
            dist[s as usize] = 0;
            heap.push(std::cmp::Reverse((0u32, s as usize)));
        }
        while let Some(std::cmp::Reverse((d, node))) = heap.pop() {
            if d > dist[node] {
                continue;
            }
            let l = node / (ny * nx);
            let rem = node % (ny * nx);
            let (x, y) = ((rem % nx) as i64, (rem / nx) as i64);
            let mut push = |nx_i: i64, ny_i: i64, nl_i: usize, cost: u32| {
                if nx_i < 0 || ny_i < 0 || nx_i >= nx as i64 || ny_i >= ny as i64 {
                    return;
                }
                let idx = (nl_i * ny + ny_i as usize) * nx + nx_i as usize;
                if !passable[idx] {
                    return;
                }
                let nd = d + cost;
                if nd < dist[idx] {
                    dist[idx] = nd;
                    heap.push(std::cmp::Reverse((nd, idx)));
                }
            };
            for (dx, dy, c) in [
                (-1, 0, costs.step),
                (1, 0, costs.step),
                (0, -1, costs.step),
                (0, 1, costs.step),
                (-1, -1, costs.diag),
                (1, -1, costs.diag),
                (-1, 1, costs.diag),
                (1, 1, costs.diag),
            ] {
                push(x + dx, y + dy, l, c);
            }
            if l > 0 {
                push(x, y, l - 1, costs.via);
            }
            if l + 1 < nl {
                push(x, y, l + 1, costs.via);
            }
        }
        dist
    }

    /// GPU batch fields match per-search CPU Dijkstra exactly.
    #[test]
    fn batch_matches_cpu_reference() {
        let dims = (16usize, 12usize, 3usize);
        let total = dims.0 * dims.1 * dims.2;
        // Occupancy: a vertical wall on layer 0 with a gap, clear elsewhere.
        let mut states = vec![2u8; total];
        for y in 0..dims.1 {
            if y != 6 {
                states[y * dims.0 + 8] = 0; // layer 0, x=8
            }
        }
        // Search 1 crosses the wall; search 2 starts on the far side and has
        // an overlay strip along the wall (its "own copper").
        let searches = vec![
            BatchSearch {
                sources: vec![(2 * dims.0 + 2) as u32],
                overlay: vec![],
            },
            BatchSearch {
                sources: vec![(10 * dims.0 + 14) as u32],
                overlay: (0..dims.1).map(|y| (y * dims.0 + 8) as u32).collect(),
            },
        ];
        let costs = BatchCosts {
            step: 1000,
            diag: 1414,
            via: 4000,
        };
        let gpu = match distance_fields_batch(dims, &states, &searches, &costs) {
            Ok(g) => g,
            Err(e) => {
                eprintln!("no GPU ({e}) — skipping");
                return;
            }
        };
        for (si, s) in searches.iter().enumerate() {
            let cpu = cpu_field(dims, &states, s, &costs);
            assert_eq!(gpu[si], cpu, "search {si} field mismatch");
        }
    }
}
