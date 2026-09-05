//! GPU-resident router board state (GPU-router charter M0).
//!
//! The router's CPU occupancy rasters are rebuilt per search because they
//! bake in the searching net's `(half_width, clearance)`. The resident GPU
//! state therefore lives **per rule class**: one multi-layer occupancy
//! buffer for each distinct `(half_width_milli, clearance_milli)` the board's
//! net classes produce (two or three classes cover a real board; exotic nets
//! fall back to the CPU path). Each class buffer is uploaded once and then
//! maintained by *delta rectangles* driven by the session's dirty-grid
//! epochs — the board never crosses the bus again after warm-up.
//!
//! This module owns only GPU residency and delta plumbing. Raster *content*
//! (exact-oracle cell states) is produced by the router crate and handed in
//! as byte slices, so the legality semantics stay in exactly one place.

use crate::{GpuContext, GpuError};
use std::collections::HashMap;

/// Identity of a rule class a resident raster is built for. Millimeter
/// dimensions quantized to micrometres so the key is `Eq + Hash`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RuleClassKey {
    /// Trace half-width in micrometres.
    pub half_width_um: u32,
    /// Required clearance in micrometres.
    pub clearance_um: u32,
}

impl RuleClassKey {
    /// Quantize a `(half_width, clearance)` pair in millimetres.
    pub fn from_mm(half_width: f64, clearance: f64) -> Self {
        Self {
            half_width_um: (half_width * 1000.0).round() as u32,
            clearance_um: (clearance * 1000.0).round() as u32,
        }
    }
}

/// Grid geometry shared by every class raster of one resident state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResidentDims {
    /// Cells along X.
    pub nx: usize,
    /// Cells along Y.
    pub ny: usize,
    /// Copper layer count.
    pub layers: usize,
    /// Cell pitch in millimetres.
    pub pitch: f64,
    /// World origin of cell (0, 0).
    pub origin: [f64; 2],
}

impl ResidentDims {
    /// Total node count.
    pub fn len(&self) -> usize {
        self.nx * self.ny * self.layers
    }

    /// True when the grid has zero nodes.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A dirty-region update: cell-space rectangle plus fresh states for every
/// layer, tightly packed as `layers * w * h` bytes (layer-major, then rows).
pub struct DeltaRect<'a> {
    /// Inclusive min cell (x, y).
    pub min: (usize, usize),
    /// Inclusive max cell (x, y).
    pub max: (usize, usize),
    /// Packed replacement states, length `layers * w * h`.
    pub states: &'a [u8],
}

struct ClassRaster {
    buffer: wgpu::Buffer,
    /// Session epoch the buffer contents reflect.
    epoch: u64,
}

/// GPU-resident, delta-maintained occupancy state for one board.
pub struct GpuRouterState {
    dims: ResidentDims,
    classes: HashMap<RuleClassKey, ClassRaster>,
    /// Bytes shipped across the bus since creation (telemetry for the M0
    /// exit criterion: delta traffic, not board re-uploads).
    pub bytes_uploaded: u64,
}

impl GpuRouterState {
    /// Create an empty resident state for a board grid.
    pub fn new(dims: ResidentDims) -> Self {
        Self {
            dims,
            classes: HashMap::new(),
            bytes_uploaded: 0,
        }
    }

    /// Grid geometry.
    pub fn dims(&self) -> ResidentDims {
        self.dims
    }

    /// Epoch of a resident class, if present.
    pub fn class_epoch(&self, key: RuleClassKey) -> Option<u64> {
        self.classes.get(&key).map(|c| c.epoch)
    }

    /// Upload (or replace) a full class raster. `states` is layer-major,
    /// `dims.len()` bytes of the router's CELL_* values.
    pub fn upload_class(
        &mut self,
        gpu: &GpuContext,
        key: RuleClassKey,
        states: &[u8],
        epoch: u64,
    ) -> Result<(), GpuError> {
        if states.len() != self.dims.len() {
            return Err(GpuError::InvalidInput(format!(
                "class raster length {} != grid nodes {}",
                states.len(),
                self.dims.len()
            )));
        }
        use wgpu::util::DeviceExt;
        // u8 states packed into a u32 storage buffer (wgpu has no u8
        // storage); the wavefront kernels index with `(word >> shift) & 0xff`.
        let packed = pack_u8_words(states);
        let buffer = gpu
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("router-class-raster"),
                contents: bytemuck::cast_slice(&packed),
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_DST
                    | wgpu::BufferUsages::COPY_SRC,
            });
        self.bytes_uploaded += (packed.len() * 4) as u64;
        self.classes.insert(key, ClassRaster { buffer, epoch });
        Ok(())
    }

    /// Apply delta rectangles to a resident class, advancing its epoch.
    ///
    /// Word-aligned writes: each dirty row is expanded to the covering u32
    /// span and written via `queue.write_buffer`, so a typical commit costs a
    /// few hundred bytes, not a board.
    pub fn apply_deltas(
        &mut self,
        gpu: &GpuContext,
        key: RuleClassKey,
        deltas: &[DeltaRect<'_>],
        epoch: u64,
    ) -> Result<(), GpuError> {
        let dims = self.dims;
        let class = self.classes.get_mut(&key).ok_or_else(|| {
            GpuError::InvalidInput("apply_deltas on a class never uploaded".into())
        })?;
        for d in deltas {
            let (x0, y0) = d.min;
            let (x1, y1) = d.max;
            if x1 >= dims.nx || y1 >= dims.ny || x0 > x1 || y0 > y1 {
                return Err(GpuError::InvalidInput(format!(
                    "delta rect ({x0},{y0})..({x1},{y1}) outside {}x{}",
                    dims.nx, dims.ny
                )));
            }
            let w = x1 - x0 + 1;
            let h = y1 - y0 + 1;
            if d.states.len() != dims.layers * w * h {
                return Err(GpuError::InvalidInput(format!(
                    "delta payload {} != layers*w*h {}",
                    d.states.len(),
                    dims.layers * w * h
                )));
            }
            for li in 0..dims.layers {
                for row in 0..h {
                    let src = &d.states[(li * h + row) * w..(li * h + row) * w + w];
                    // Word-aligned destination span covering [x0, x1] on this row.
                    let node0 = (li * dims.ny + (y0 + row)) * dims.nx + x0;
                    let node1 = node0 + w - 1;
                    let w0 = node0 / 4;
                    let w1 = node1 / 4;
                    let mut words = vec![0u32; w1 - w0 + 1];
                    // Fill edges from src (interior bytes fully covered; the
                    // partial edge words merge with src too — bytes outside
                    // [x0,x1] on this row belong to OTHER cells whose value
                    // we must preserve, so read them from the delta's
                    // neighbours is impossible; instead require callers to
                    // send word-aligned rects. Enforced here:
                    if !node0.is_multiple_of(4) || !(node1 + 1).is_multiple_of(4) {
                        return Err(GpuError::InvalidInput(
                            "delta rects must be 4-cell aligned in x (word-aligned rows)".into(),
                        ));
                    }
                    for (i, &b) in src.iter().enumerate() {
                        let word = i / 4;
                        let shift = (i % 4) * 8;
                        words[word] |= (b as u32) << shift;
                    }
                    gpu.queue.write_buffer(
                        &class.buffer,
                        (w0 * 4) as u64,
                        bytemuck::cast_slice(&words),
                    );
                    self.bytes_uploaded += (words.len() * 4) as u64;
                }
            }
        }
        class.epoch = epoch;
        Ok(())
    }

    /// The resident buffer for a class (for wavefront kernel binding).
    pub fn class_buffer(&self, key: RuleClassKey) -> Option<&wgpu::Buffer> {
        self.classes.get(&key).map(|c| &c.buffer)
    }
}

/// Pack u8 cell states little-endian into u32 words (length rounded up).
pub fn pack_u8_words(states: &[u8]) -> Vec<u32> {
    let mut out = vec![0u32; states.len().div_ceil(4)];
    for (i, &b) in states.iter().enumerate() {
        out[i / 4] |= (b as u32) << ((i % 4) * 8);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Blocking staging-buffer readback for tests.
    fn read_back_u32(gpu: &GpuContext, src: &wgpu::Buffer, words: usize) -> Vec<u32> {
        let size = (words * 4) as u64;
        let staging = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("router-state-readback"),
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut enc = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        enc.copy_buffer_to_buffer(src, 0, &staging, 0, size);
        gpu.queue.submit([enc.finish()]);
        let slice = staging.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
        let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
        rx.recv().unwrap().unwrap();
        let view = slice.get_mapped_range().unwrap();
        let out: Vec<u32> = bytemuck::cast_slice(&view).to_vec();
        out
    }

    #[test]
    fn pack_words_little_endian() {
        let words = pack_u8_words(&[1, 2, 3, 4, 5]);
        assert_eq!(words, vec![0x04030201, 0x0000_0005]);
    }

    #[test]
    fn rule_class_key_quantizes() {
        assert_eq!(
            RuleClassKey::from_mm(0.04, 0.08),
            RuleClassKey {
                half_width_um: 40,
                clearance_um: 80
            }
        );
    }

    /// Residency round-trip on real hardware; skipped when no adapter.
    #[test]
    fn resident_upload_and_delta_roundtrip() {
        let Ok(gpu) = pollster::block_on(GpuContext::init()) else {
            eprintln!("no GPU adapter — skipping");
            return;
        };
        let dims = ResidentDims {
            nx: 8,
            ny: 4,
            layers: 2,
            pitch: 0.1,
            origin: [0.0, 0.0],
        };
        let mut state = GpuRouterState::new(dims);
        let key = RuleClassKey::from_mm(0.04, 0.08);
        let full: Vec<u8> = (0..dims.len() as u32).map(|i| (i % 3) as u8).collect();
        state.upload_class(&gpu, key, &full, 1).unwrap();
        assert_eq!(state.class_epoch(key), Some(1));

        // Word-aligned delta: row y=1, x 0..=7, both layers -> all CELL 2.
        let w = 8usize;
        let payload = vec![2u8; dims.layers * w];
        state
            .apply_deltas(
                &gpu,
                key,
                &[DeltaRect {
                    min: (0, 1),
                    max: (7, 1),
                    states: &payload,
                }],
                2,
            )
            .unwrap();
        assert_eq!(state.class_epoch(key), Some(2));

        // Read back and verify the delta landed and the rest is intact.
        let buf = state.class_buffer(key).unwrap();
        let read = read_back_u32(gpu, buf, dims.len().div_ceil(4));
        let mut got = vec![0u8; dims.len()];
        for (i, g) in got.iter_mut().enumerate() {
            *g = ((read[i / 4] >> ((i % 4) * 8)) & 0xff) as u8;
        }
        for li in 0..dims.layers {
            for y in 0..dims.ny {
                for x in 0..dims.nx {
                    let idx = (li * dims.ny + y) * dims.nx + x;
                    let want = if y == 1 { 2 } else { full[idx] };
                    assert_eq!(got[idx], want, "node ({x},{y},{li})");
                }
            }
        }
    }
}
