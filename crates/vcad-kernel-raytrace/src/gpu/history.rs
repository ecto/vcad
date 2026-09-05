//! Per-pixel history and denoising, on the device.
//!
//! [`RayTracePipeline::render_resident_linear`] hands a host one raw linear
//! sample per pass and leaves it to keep the history itself. That works, and
//! for a viewport it is the wrong place for the work: folding the sample in is
//! cheap, but running [`crate::pathtrace::denoise`] over the whole frame every
//! pass is not — measured from Kosm's viewer at 512x288, 420 ms of CPU filter
//! against 25 ms of tracing. The filter, not the path tracer, was the frame.
//!
//! [`HistoryBuffers`] moves all of it onto the GPU. The running mean, the
//! per-pixel sample count and the moments that feed the variance estimate live
//! in buffers beside the resident scene; a small compute pass folds each raw
//! sample in, an à-trous pass filters the mean guided by the resident depth,
//! normal and albedo planes, and a resolve pass tonemaps into a storage
//! texture the caller owns. Nothing is read back, so a viewport can blit the
//! texture straight to its surface.
//!
//! # What the caller still owns
//!
//! **The keep mask.** The device side takes one byte per pixel, 1 to keep
//! accumulating and 0 to restart that pixel: everything the device cannot
//! know, from a re-pose to a region the caller wants re-converged. An empty
//! slice is all-1.
//!
//! **Reprojection** used to be on that list, and a host without one had only
//! the blunt instrument: upload an all-zero mask on a camera move and watch
//! every pixel of an orbit restart from a single sample, most of them the
//! same surface seen from a hair to the left.
//! [`RayTracePipeline::accumulate_and_denoise_resident_reprojected`] does it
//! on the device instead — hand it the previous pass's camera and it carries
//! each pixel's mean and count across the move, restarting only what the move
//! actually disoccluded. See [`HistoryBuffers`]' `prev_guides`.
//!
//! # Parity with the CPU filter
//!
//! [`HistoryBuffers::denoise_params`] defaults to
//! [`crate::pathtrace::PathTraceOptions`]'s filter constants, and the shader
//! is a port of [`crate::pathtrace::denoise`] weight for weight: same
//! B3-spline taps, same normal/depth/luminance edge stops, same albedo
//! demodulation with the same floor, same variance prefilter. A one-sample
//! history therefore denoises to what the CPU would have produced from the
//! same [`crate::pathtrace::Film`], which is what `tests/gpu_history.rs`
//! checks.
//!
//! The one deliberate difference is the fade: the filter's strength falls
//! linearly with the pixel's history length and reaches zero at
//! [`GpuDenoiseParams::count_cutoff`] samples, at which point the à-trous pass
//! skips the pixel entirely. A converged pixel needs no filter and paying for
//! one only softens it.

use bytemuck::{Pod, Zeroable};
use vcad_kernel_gpu::{GpuContext, GpuError};

use super::buffers::{GpuCamera, GpuRenderState};
use super::pipeline::{read_back_f32, RayTracePipeline};
use super::resident::ResidentScene;

/// The most à-trous iterations one call may dispatch.
///
/// Each iteration needs its own slot in the parameter buffer, which is sized
/// once. Five is [`crate::pathtrace::PathTraceOptions`]'s default and doubles
/// the tap stride each time, so the widest footprint is already 32 pixels.
pub const MAX_DENOISE_ITERS: u32 = 8;

/// Uniform slots: one per à-trous iteration, plus one shared by the
/// accumulate, demodulate and resolve passes.
const PARAM_SLOTS: u32 = MAX_DENOISE_ITERS + 1;

/// Uniform buffer offsets must be a multiple of this on every backend we
/// target, so each parameter slot is padded out to it.
const PARAM_STRIDE: u64 = 256;

/// Filter settings for [`RayTracePipeline::accumulate_and_denoise_resident`].
///
/// The `sigma_*` fields and `iters` mean exactly what the identically-named
/// fields of [`crate::pathtrace::PathTraceOptions`] mean, and default to the
/// same values, so the two tiers filter the same way.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GpuDenoiseParams {
    /// Number of à-trous iterations, clamped to [`MAX_DENOISE_ITERS`]. Zero
    /// turns the filter off and tonemaps the bare running mean.
    pub iters: u32,
    /// Tolerance on the normal guide.
    pub sigma_normal: f32,
    /// Tolerance on the depth guide, relative to the centre pixel's depth.
    pub sigma_depth: f32,
    /// Tolerance on illumination, in units of the pixel's own error bar.
    pub sigma_lum: f32,
    /// History length at which the filter has fully faded out. A pixel with at
    /// least this many samples is passed through untouched — the temporal mean
    /// is already cleaner than anything a spatial filter would leave.
    pub count_cutoff: u32,
    /// Linear exposure applied before the tonemap curve.
    pub exposure: f32,
}

impl Default for GpuDenoiseParams {
    fn default() -> Self {
        let d = crate::pathtrace::PathTraceOptions::default();
        Self {
            iters: d.denoise_iters,
            sigma_normal: d.sigma_normal,
            sigma_depth: d.sigma_depth,
            sigma_lum: d.sigma_lum,
            count_cutoff: 32,
            exposure: 1.0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct HistoryParams {
    width: u32,
    height: u32,
    count_cutoff: u32,
    iters: u32,
    sigma_lum: f32,
    sigma_depth: f32,
    sigma_normal: f32,
    exposure: f32,
    stride: u32,
    src_is_b: u32,
    scissor_xy: u32,
    scissor_wh: u32,
    // `reproject` only; zero elsewhere. Both views as the shader's ray
    // generator builds them, then (tan(fov/2), aspect) for each.
    cur_eye: [f32; 4],
    cur_right: [f32; 4],
    cur_up: [f32; 4],
    cur_forward: [f32; 4],
    prev_eye: [f32; 4],
    prev_right: [f32; 4],
    prev_up: [f32; 4],
    prev_forward: [f32; 4],
    view_params: [f32; 4],
    reprojected: u32,
    _pad_reproj: [u32; 3],
}

/// The camera basis the shader's ray generator derives from a [`GpuCamera`],
/// reproduced here so the reprojection pass can be told about a view it is
/// not currently rendering.
fn view_basis(cam: &GpuCamera) -> ([f32; 4], [f32; 4], [f32; 4], [f32; 4], f32, f32) {
    let norm = |v: [f32; 3]| {
        let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        [v[0] / l, v[1] / l, v[2] / l]
    };
    let cross = |a: [f32; 3], b: [f32; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let eye = [cam.position[0], cam.position[1], cam.position[2]];
    let fwd = norm([
        cam.target[0] - eye[0],
        cam.target[1] - eye[1],
        cam.target[2] - eye[2],
    ]);
    let right = norm(cross(fwd, [cam.up[0], cam.up[1], cam.up[2]]));
    let up = cross(right, fwd);
    let v4 = |v: [f32; 3]| [v[0], v[1], v[2], 0.0];
    (
        [eye[0], eye[1], eye[2], 1.0],
        v4(right),
        v4(up),
        v4(fwd),
        (cam.fov * 0.5).tan(),
        cam.width as f32 / cam.height as f32,
    )
}

/// The running mean and sample count read back off the device.
///
/// For tests and for a host that wants to checkpoint a converged frame; the
/// render path never needs it.
#[derive(Debug, Clone)]
pub struct History {
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Running mean of the linear radiance, 3 floats per pixel.
    pub rgb: Vec<f32>,
    /// Running mean of the path tracer's coverage, one float per pixel.
    pub alpha: Vec<f32>,
    /// How many samples each pixel's mean is over.
    pub count: Vec<u32>,
    /// Variance of each pixel's mean luminance, in
    /// [`crate::pathtrace::Film::variance`]'s convention.
    pub variance: Vec<f32>,
}

/// The device-side history for one resident scene, at one frame size.
///
/// Built on first use by [`RayTracePipeline::accumulate_and_denoise_resident`]
/// and thrown away whenever the scene is resized.
pub struct HistoryBuffers {
    width: u32,
    height: u32,
    /// (linear radiance, coverage), the running mean.
    mean: wgpu::Buffer,
    /// (count, luminance sum, luminance-squared sum, variance of the mean).
    stats: wgpu::Buffer,
    /// The caller's keep mask, widened to one `u32` per pixel.
    keep: wgpu::Buffer,
    /// The *previous* pass's guide plane 1 — (normal, distance from that
    /// pass's eye) — one vec4 per pixel. Copied out of the resident scene's
    /// depth/normal buffer at the end of every pass, so the next pass's
    /// reprojection has something to test against. Zeroed until the first
    /// pass has run, which reads as "restart everything".
    prev_guides: wgpu::Buffer,
    /// (illumination, variance) ping-pong for the wavelet iterations.
    scratch_a: wgpu::Buffer,
    scratch_b: wgpu::Buffer,
    /// One uniform slot per pass; see [`PARAM_SLOTS`].
    params: wgpu::Buffer,
    /// Staging for [`RayTracePipeline::read_history`], allocated on first use.
    readback: Option<wgpu::Buffer>,
    /// Scratch for widening the caller's `u8` mask, kept so a per-frame
    /// upload allocates nothing.
    keep_staging: Vec<u32>,
}

impl HistoryBuffers {
    pub(super) fn new(ctx: &GpuContext, width: u32, height: u32) -> Self {
        let n = (width as u64) * (height as u64);
        let mk = |label: &str, size: u64| {
            ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: size.max(16),
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_DST
                    | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            })
        };
        Self {
            width,
            height,
            mean: mk("History Mean", n * 16),
            stats: mk("History Stats", n * 16),
            keep: mk("History Keep Mask", n * 4),
            prev_guides: mk("History Previous Guides", n * 16),
            scratch_a: mk("History Scratch A", n * 16),
            scratch_b: mk("History Scratch B", n * 16),
            params: ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("History Params"),
                size: PARAM_STRIDE * PARAM_SLOTS as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            readback: None,
            keep_staging: Vec::new(),
        }
    }

    /// The frame size this history is allocated for.
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Forget every pixel's history.
    ///
    /// Equivalent to passing an all-zero keep mask to the next pass, and
    /// cheaper: it is a buffer clear rather than an upload.
    pub fn clear(&self, ctx: &GpuContext) {
        let mut enc = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("History Clear"),
            });
        enc.clear_buffer(&self.mean, 0, None);
        enc.clear_buffer(&self.stats, 0, None);
        // A reprojection into a history that is gone would carry zeros, which
        // is harmless, but forgetting the previous view too keeps "cleared"
        // meaning exactly one thing.
        enc.clear_buffer(&self.prev_guides, 0, None);
        ctx.queue.submit(Some(enc.finish()));
    }
}

/// The three compute pipelines the history passes run, plus their shared
/// layout.
///
/// Built once and reused; hand it to
/// [`RayTracePipeline::accumulate_and_denoise_resident`] every pass.
pub struct HistoryPipeline {
    reproject: wgpu::ComputePipeline,
    accumulate: wgpu::ComputePipeline,
    demodulate: wgpu::ComputePipeline,
    atrous: wgpu::ComputePipeline,
    resolve: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
}

impl HistoryPipeline {
    /// Compile the history and denoise passes.
    pub fn new(ctx: &GpuContext) -> Result<Self, GpuError> {
        let module = ctx
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("History Shader"),
                source: wgpu::ShaderSource::Wgsl(super::shaders::HISTORY_SHADER.into()),
            });

        let storage = |binding: u32, read_only: bool| wgpu::BindGroupLayoutEntry {
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
                label: Some("History Bind Group Layout"),
                entries: &[
                    // Params, one slot per pass, selected by dynamic offset.
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: true,
                            min_binding_size: wgpu::BufferSize::new(
                                std::mem::size_of::<HistoryParams>() as u64,
                            ),
                        },
                        count: None,
                    },
                    storage(1, true),  // raw sample
                    storage(2, true),  // guide planes
                    storage(3, false), // mean
                    storage(4, false), // stats
                    storage(5, true),  // keep mask
                    storage(6, false), // scratch src
                    storage(7, false), // scratch dst
                    wgpu::BindGroupLayoutEntry {
                        binding: 8,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::StorageTexture {
                            access: wgpu::StorageTextureAccess::WriteOnly,
                            format: wgpu::TextureFormat::Rgba8Unorm,
                            view_dimension: wgpu::TextureViewDimension::D2,
                        },
                        count: None,
                    },
                    storage(9, true), // the previous pass's guide plane
                ],
            });

        let pipeline_layout = ctx
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("History Pipeline Layout"),
                bind_group_layouts: &[Some(&layout)],
                immediate_size: 0,
            });

        let mk = |entry: &str| {
            ctx.device
                .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some(entry),
                    layout: Some(&pipeline_layout),
                    module: &module,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    cache: None,
                })
        };

        Ok(Self {
            reproject: mk("reproject"),
            accumulate: mk("accumulate"),
            demodulate: mk("demodulate"),
            atrous: mk("atrous"),
            resolve: mk("resolve"),
            layout,
        })
    }
}

impl RayTracePipeline {
    /// Trace one pass, fold it into the device-side history, denoise the
    /// running mean, and tonemap the result into `target` — with no readback
    /// at all.
    ///
    /// This is the whole of a progressive viewport's per-frame work. Call it
    /// once per pass with an increasing `state.frame_index`; the shader's
    /// jitter and RNG are driven by that index, so successive passes are
    /// independent samples of the same picture and the running mean converges.
    ///
    /// `keep` is one byte per pixel in row-major order, the same length as the
    /// frame: **1 keeps that pixel's history, 0 restarts it** at this pass's
    /// sample. It is how the caller expresses everything the device cannot
    /// know — a camera move (upload zeros), a reprojection that found a valid
    /// history for some pixels and not others, a region the caller wants to
    /// re-converge. Pass an all-1 mask for a still frame. An empty slice is
    /// read as all-1, which is the common case and costs no upload.
    ///
    /// A **scissor** set on `state` (see `GpuRenderState::set_scissor`) is
    /// honoured all the way through: the trace pass dispatches only over the
    /// rectangle, and the accumulate pass folds a sample in only for the
    /// pixels inside it. Every pixel outside keeps the mean, the sample count
    /// and the variance it already had — nothing stale is counted as fresh.
    /// The resolve pass still covers the frame, so the target texture stays
    /// whole; it simply re-resolves the untouched pixels from their unchanged
    /// history. That is what lets a viewer trace only the part of the frame
    /// that moved and keep the rest.
    ///
    /// `target` must be a view of an `Rgba8Unorm` texture with
    /// `STORAGE_BINDING` usage, at least the resident scene's size.
    ///
    /// `state` is forced into raw-sample mode and its refinement pass turned
    /// off, exactly as [`RayTracePipeline::render_resident_linear`] forces
    /// them: what the history folds in has to be one unweighted sample, not a
    /// step of the shader's own running average.
    ///
    /// The work is submitted and this returns immediately; the caller
    /// sequences it against whatever presents the texture.
    #[allow(clippy::too_many_arguments)]
    pub fn accumulate_and_denoise_resident(
        &self,
        ctx: &GpuContext,
        history_pipeline: &HistoryPipeline,
        res: &mut ResidentScene,
        camera: &GpuCamera,
        state: GpuRenderState,
        keep: &[u8],
        denoise: &GpuDenoiseParams,
        target: &wgpu::TextureView,
    ) -> Result<(), GpuError> {
        self.accumulate_and_denoise_resident_reprojected(
            ctx,
            history_pipeline,
            res,
            camera,
            state,
            keep,
            denoise,
            target,
            None,
        )
    }

    /// [`RayTracePipeline::accumulate_and_denoise_resident`], plus the
    /// device-side reprojection.
    ///
    /// Pass the camera the *previous* call to this method rendered from as
    /// `prev_view` and the history is carried across the move: each pixel is
    /// unprojected through this pass's depth, projected into the previous
    /// view, and takes the nearest previous pixel's mean and sample count
    /// where that pixel's depth agrees within 2% and its normal within a dot
    /// product of 0.9. Everything else — disocclusions, pixels that were off
    /// the previous film, background — restarts at this pass's sample.
    ///
    /// `None` skips the pass entirely and is exactly
    /// [`RayTracePipeline::accumulate_and_denoise_resident`]. Pass `None` on
    /// a still frame too: reprojecting a view onto itself is a no-op that
    /// still costs two dispatches, and passing the *same* camera is
    /// harmless but pointless.
    ///
    /// The keep mask still applies, and applies *after* the reprojection: a
    /// caller that reprojects on the device wants an empty (all-keep) mask
    /// and lets this pass decide what survives. Zeroing a pixel's mask entry
    /// still restarts it, whatever the reprojection found.
    ///
    /// Depth is [`crate::pathtrace::Film::depth`]'s convention throughout —
    /// distance from the eye along the primary ray, zero for background — and
    /// the reprojection reads the previous pass's copy of that plane, which
    /// this method keeps for itself. The first pass after a resize therefore
    /// restarts every pixel, since there is no previous plane to test yet.
    #[allow(clippy::too_many_arguments)]
    pub fn accumulate_and_denoise_resident_reprojected(
        &self,
        ctx: &GpuContext,
        history_pipeline: &HistoryPipeline,
        res: &mut ResidentScene,
        camera: &GpuCamera,
        state: GpuRenderState,
        keep: &[u8],
        denoise: &GpuDenoiseParams,
        target: &wgpu::TextureView,
        prev_view: Option<&GpuCamera>,
    ) -> Result<(), GpuError> {
        let (w, h) = res.size();
        let n = (w as usize) * (h as usize);
        if !keep.is_empty() && keep.len() != n {
            return Err(GpuError::InvalidInput(format!(
                "keep mask has {} entries, expected {w}x{h} = {n}",
                keep.len(),
            )));
        }

        res.ensure_history(ctx, w, h);
        let iters = denoise.iters.min(MAX_DENOISE_ITERS);

        // Widen the caller's mask. WGSL has no 8-bit storage type, and one
        // word per pixel is a 590 KB upload at 512x288 — an order of magnitude
        // less than the frame it saves reading back.
        // The two views the reprojection pass works between. With no
        // previous view the pass is not dispatched and these are inert.
        let cur = view_basis(camera);
        let prev = view_basis(prev_view.unwrap_or(camera));

        {
            let hist = res.history_mut().expect("history was just ensured");
            hist.keep_staging.clear();
            hist.keep_staging.reserve(n);
            if keep.is_empty() {
                hist.keep_staging.resize(n, 1);
            } else {
                hist.keep_staging.extend(keep.iter().map(|&b| u32::from(b)));
            }
            ctx.queue.write_buffer(
                &hist.keep,
                0,
                bytemuck::cast_slice(hist.keep_staging.as_slice()),
            );

            // Slot 0 is shared by accumulate, demodulate and resolve; slots
            // 1..=iters carry each wavelet iteration's tap stride.
            let base = HistoryParams {
                width: w,
                height: h,
                count_cutoff: denoise.count_cutoff.max(1),
                iters,
                sigma_lum: denoise.sigma_lum,
                sigma_depth: denoise.sigma_depth,
                sigma_normal: denoise.sigma_normal,
                exposure: denoise.exposure,
                stride: 1,
                // The final iteration lands in scratch_b when the count is
                // odd, since iteration 0 reads A and writes B.
                src_is_b: u32::from(iters % 2 == 1),
                // Straight from the trace pass's own state, so the two can
                // never disagree about which pixels this pass refreshed.
                scissor_xy: state.scissor_xy,
                scissor_wh: state.scissor_wh,
                cur_eye: cur.0,
                cur_right: cur.1,
                cur_up: cur.2,
                cur_forward: cur.3,
                prev_eye: prev.0,
                prev_right: prev.1,
                prev_up: prev.2,
                prev_forward: prev.3,
                view_params: [cur.4, cur.5, prev.4, prev.5],
                reprojected: u32::from(prev_view.is_some()),
                _pad_reproj: [0; 3],
            };
            ctx.queue
                .write_buffer(&hist.params, 0, bytemuck::bytes_of(&base));
            for it in 0..iters {
                let p = HistoryParams {
                    stride: 1u32 << it,
                    ..base
                };
                ctx.queue.write_buffer(
                    &hist.params,
                    PARAM_STRIDE * (1 + it) as u64,
                    bytemuck::bytes_of(&p),
                );
            }
        }

        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("History Encoder"),
            });

        // One raw sample into the resident scene's own accumulation buffer,
        // with the guide planes filled.
        let mut state = state;
        state.set_raw_sample(true);
        state.refine_sample_count = 0;
        self.encode_raw_sample_into(ctx, res, camera, state, &mut encoder);

        let (raw, guides) = res.raw_and_guide_buffers();
        let hist = res.history().expect("history was just ensured");

        // Two bind groups differing only in which scratch buffer is the
        // source, so the wavelet iterations can ping-pong under a fixed
        // layout.
        let bind = |src: &wgpu::Buffer, dst: &wgpu::Buffer, label: &str| {
            ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &history_pipeline.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &hist.params,
                            offset: 0,
                            size: wgpu::BufferSize::new(
                                std::mem::size_of::<HistoryParams>() as u64
                            ),
                        }),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: raw.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: guides.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: hist.mean.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: hist.stats.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: hist.keep.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: src.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 7,
                        resource: dst.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 8,
                        resource: wgpu::BindingResource::TextureView(target),
                    },
                    wgpu::BindGroupEntry {
                        binding: 9,
                        resource: hist.prev_guides.as_entire_binding(),
                    },
                ],
            })
        };
        let ab = bind(&hist.scratch_a, &hist.scratch_b, "History Bind Group A->B");
        let ba = bind(&hist.scratch_b, &hist.scratch_a, "History Bind Group B->A");

        let groups = (w.div_ceil(8), h.div_ceil(8));
        let mut run =
            |pipeline: &wgpu::ComputePipeline, group: &wgpu::BindGroup, slot: u32, label: &str| {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some(label),
                    timestamp_writes: None,
                });
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, group, &[(PARAM_STRIDE * slot as u64) as u32]);
                pass.dispatch_workgroups(groups.0, groups.1, 1);
            };

        // Before anything is folded in: carry what the previous view already
        // knew about each pixel onto this view's pixel grid.
        // `accumulate` picks the gather up out of the scratch pair.
        if prev_view.is_some() {
            run(&history_pipeline.reproject, &ab, 0, "History Reproject");
        }
        run(&history_pipeline.accumulate, &ab, 0, "History Accumulate");
        if iters > 0 {
            run(&history_pipeline.demodulate, &ab, 0, "History Demodulate");
            for it in 0..iters {
                // Iteration 0 reads A and writes B, so even iterations use the
                // A->B group and odd ones B->A.
                let group = if it % 2 == 0 { &ab } else { &ba };
                run(&history_pipeline.atrous, group, 1 + it, "History A-Trous");
            }
        }
        run(&history_pipeline.resolve, &ab, 0, "History Resolve");

        // Keep this pass's depth/normal plane for the next one to reproject
        // against. Guide plane 1 starts one plane into the depth/normal
        // buffer; plane 0 is the shader's own edge-detection copy.
        let plane = (w as u64) * (h as u64) * 16;
        encoder.copy_buffer_to_buffer(guides, plane, &hist.prev_guides, 0, plane);

        ctx.queue.submit(Some(encoder.finish()));
        Ok(())
    }

    /// Read the device-side history back.
    ///
    /// For tests, and for a host that wants to save the converged frame. The
    /// render path never needs it — that is the point of
    /// [`RayTracePipeline::accumulate_and_denoise_resident`].
    ///
    /// Returns `None` if no pass has built a history for this scene yet.
    pub async fn read_history(
        &self,
        ctx: &GpuContext,
        res: &mut ResidentScene,
    ) -> Result<Option<History>, GpuError> {
        let Some((w, h)) = res.history().map(|hi| (hi.width, hi.height)) else {
            return Ok(None);
        };
        let n = (w as u64) * (h as u64);
        let plane = n * 16;

        {
            let hist = res.history_mut().expect("just checked");
            if hist.readback.is_none() {
                hist.readback = Some(ctx.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("History Readback"),
                    size: plane * 2,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                }));
            }
        }

        let hist = res.history().expect("just checked");
        let staging = hist.readback.as_ref().expect("just allocated");
        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("History Readback Encoder"),
            });
        encoder.copy_buffer_to_buffer(&hist.mean, 0, staging, 0, plane);
        encoder.copy_buffer_to_buffer(&hist.stats, 0, staging, plane, plane);
        ctx.queue.submit(Some(encoder.finish()));

        let raw = read_back_f32(ctx, staging).await?;
        let n = n as usize;
        let mut out = History {
            width: w,
            height: h,
            rgb: vec![0.0; n * 3],
            alpha: vec![0.0; n],
            count: vec![0; n],
            variance: vec![0.0; n],
        };
        for i in 0..n {
            let m = &raw[i * 4..i * 4 + 4];
            let s = &raw[(n + i) * 4..(n + i) * 4 + 4];
            out.rgb[i * 3..i * 3 + 3].copy_from_slice(&m[..3]);
            out.alpha[i] = m[3];
            out.count[i] = s[0] as u32;
            out.variance[i] = s[3];
        }
        Ok(Some(out))
    }
}
