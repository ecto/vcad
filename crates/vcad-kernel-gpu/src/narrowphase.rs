//! Batched copper narrowphase prefilter (GPU-router charter M2).
//!
//! Evaluates edge-to-edge distances for a list of primitive pairs in one
//! dispatch and reports which pairs CLEAR their requirement by more than a
//! safety margin. The contract is asymmetric by design: a GPU "clears" verdict
//! is trusted (margin absorbs f32 drift), everything else re-runs on the
//! exact CPU oracle — so the prefilter can only ever *reduce* exact work,
//! never change an outcome.

use crate::{GpuContext, GpuError};
use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

/// One primitive of a pair: capsule (swept segment) or disc.
#[derive(Debug, Clone, Copy)]
pub enum NarrowGeom {
    /// Segment from `a` to `b` swept by radius `r`.
    Capsule {
        /// Start point.
        a: [f32; 2],
        /// End point.
        b: [f32; 2],
        /// Sweep radius (half-width).
        r: f32,
    },
    /// Disc at `c` with radius `r`.
    Disc {
        /// Centre.
        c: [f32; 2],
        /// Radius.
        r: f32,
    },
}

/// A candidate pair with its required edge clearance.
#[derive(Debug, Clone, Copy)]
pub struct NarrowPair {
    /// First primitive.
    pub a: NarrowGeom,
    /// Second primitive.
    pub b: NarrowGeom,
    /// Required edge-to-edge clearance (mm).
    pub required: f32,
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct PairIn {
    kind_a: u32,
    kind_b: u32,
    required: f32,
    margin: f32,
    a_ax: f32,
    a_ay: f32,
    a_bx: f32,
    a_by: f32,
    a_r: f32,
    b_ax: f32,
    b_ay: f32,
    b_bx: f32,
    b_by: f32,
    b_r: f32,
    _pad0: f32,
    _pad1: f32,
}

fn encode(g: NarrowGeom) -> (u32, [f32; 5]) {
    match g {
        NarrowGeom::Capsule { a, b, r } => (0, [a[0], a[1], b[0], b[1], r]),
        NarrowGeom::Disc { c, r } => (1, [c[0], c[1], 0.0, 0.0, r]),
    }
}

/// f32 drift absorbed before trusting a "clears" verdict (mm).
pub const DEFAULT_MARGIN: f32 = 1e-3;

/// Evaluate `pairs`; returns one bool per pair — `true` = provably clears
/// (trustworthy), `false` = needs the exact oracle. Synchronous, native only.
#[cfg(not(target_arch = "wasm32"))]
pub fn clears_batch(pairs: &[NarrowPair], margin: f32) -> Result<Vec<bool>, GpuError> {
    pollster::block_on(clears_batch_async(pairs, margin))
}

/// Async variant of [`clears_batch`].
pub async fn clears_batch_async(pairs: &[NarrowPair], margin: f32) -> Result<Vec<bool>, GpuError> {
    if pairs.is_empty() {
        return Ok(Vec::new());
    }
    let ctx = GpuContext::init().await?;
    let encoded: Vec<PairIn> = pairs
        .iter()
        .map(|p| {
            let (ka, ga) = encode(p.a);
            let (kb, gb) = encode(p.b);
            PairIn {
                kind_a: ka,
                kind_b: kb,
                required: p.required,
                margin,
                a_ax: ga[0],
                a_ay: ga[1],
                a_bx: ga[2],
                a_by: ga[3],
                a_r: ga[4],
                b_ax: gb[0],
                b_ay: gb[1],
                b_bx: gb[2],
                b_by: gb[3],
                b_r: gb[4],
                _pad0: 0.0,
                _pad1: 0.0,
            }
        })
        .collect();
    let pair_buf = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("narrowphase-pairs"),
            contents: bytemuck::cast_slice(&encoded),
            usage: wgpu::BufferUsages::STORAGE,
        });
    let out_buf = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("narrowphase-out"),
        size: (pairs.len() * 4) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let shader = ctx
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("narrowphase"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/narrowphase.wgsl").into()),
        });
    let pipeline = ctx
        .device
        .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("narrowphase"),
            layout: None,
            module: &shader,
            entry_point: Some("narrowphase"),
            compilation_options: Default::default(),
            cache: None,
        });
    let bind = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("narrowphase-bind"),
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: pair_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: out_buf.as_entire_binding(),
            },
        ],
    });
    let mut enc = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    {
        let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("narrowphase"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.dispatch_workgroups((pairs.len() as u32).div_ceil(256), 1, 1);
    }
    // Readback.
    let staging = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("narrowphase-staging"),
        size: (pairs.len() * 4) as u64,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    enc.copy_buffer_to_buffer(&out_buf, 0, &staging, 0, (pairs.len() * 4) as u64);
    ctx.queue.submit([enc.finish()]);
    let slice = staging.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    let _ = ctx.device.poll(wgpu::PollType::wait_indefinitely());
    rx.recv()
        .map_err(|_| GpuError::BufferMapping)?
        .map_err(|_| GpuError::BufferMapping)?;
    let words: Vec<u32> = bytemuck::cast_slice(
        &slice
            .get_mapped_range()
            .expect("the buffer was just mapped"),
    )
    .to_vec();
    Ok(words.into_iter().map(|w| w == 1).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// GPU verdicts agree with exact CPU distances on both sides of the
    /// requirement, and the margin makes near-line cases conservative.
    #[test]
    fn prefilter_is_conservative_and_correct() {
        let pairs = vec![
            // Far apart: clears.
            NarrowPair {
                a: NarrowGeom::Capsule {
                    a: [0.0, 0.0],
                    b: [10.0, 0.0],
                    r: 0.1,
                },
                b: NarrowGeom::Capsule {
                    a: [0.0, 5.0],
                    b: [10.0, 5.0],
                    r: 0.1,
                },
                required: 0.2,
            },
            // Violating: crossing segments.
            NarrowPair {
                a: NarrowGeom::Capsule {
                    a: [0.0, 0.0],
                    b: [10.0, 10.0],
                    r: 0.1,
                },
                b: NarrowGeom::Capsule {
                    a: [0.0, 10.0],
                    b: [10.0, 0.0],
                    r: 0.1,
                },
                required: 0.2,
            },
            // Exactly at requirement: NOT trusted (margin) — needs oracle.
            NarrowPair {
                a: NarrowGeom::Disc {
                    c: [0.0, 0.0],
                    r: 0.1,
                },
                b: NarrowGeom::Disc {
                    c: [0.5, 0.0],
                    r: 0.1,
                },
                required: 0.3,
            },
            // Disc vs capsule, comfortably clear.
            NarrowPair {
                a: NarrowGeom::Disc {
                    c: [5.0, 3.0],
                    r: 0.2,
                },
                b: NarrowGeom::Capsule {
                    a: [0.0, 0.0],
                    b: [10.0, 0.0],
                    r: 0.1,
                },
                required: 0.2,
            },
        ];
        let out = match clears_batch(&pairs, DEFAULT_MARGIN) {
            Ok(o) => o,
            Err(e) => {
                eprintln!("no GPU ({e}) — skipping");
                return;
            }
        };
        assert_eq!(out, vec![true, false, false, true]);
    }
}
