//! A scene that stays on the GPU between passes.
//!
//! [`RayTracePipeline::render_with_render_state`] builds every buffer, every
//! texture and the bind group from scratch on each call. That is the right
//! shape for a one-shot render, and the wrong one for an animation or an
//! interactive viewport, where the geometry is the same from frame to frame
//! and only the camera and the instance placements move.
//!
//! [`ResidentScene`] uploads once and then rewrites in place: the camera and
//! render state every pass, the surface, face, BVH and light buffers whenever
//! the caller re-places the instances. Buffers are only recreated — and the
//! bind group with them — when a scene arrives that no longer fits in what is
//! already allocated, so a re-posed frame of the same assembly is pure
//! `write_buffer` traffic.
//!
//! Three ways out. [`RayTracePipeline::render_resident`] reads the frame back
//! to the CPU as `render_with_render_state` does, and
//! [`RayTracePipeline::render_resident_into`] writes into a storage texture the
//! caller owns and never touches the readback path at all — which is what a
//! surface presenting the frame itself wants, since a round trip through
//! system memory just to upload it again is the most expensive thing in the
//! loop. Both of those hand back display-referred 8-bit pixels.
//!
//! [`RayTracePipeline::render_resident_linear`] is the third, for a host that
//! keeps a per-pixel history of its own: one raw linear sample per pass plus
//! the denoiser's guide buffers, packed into the same [`Film`] the CPU
//! renderer produces. A tonemapped byte is not something you can average or
//! reproject, which is what makes the other two exits unusable for that.

use vcad_kernel_gpu::{GpuContext, GpuError};

use bytemuck::Zeroable;
use wgpu::util::DeviceExt;

use super::buffers::{
    pack_light_power_table, GpuAreaLight, GpuBvhNode, GpuCamera, GpuFace, GpuMaterial,
    GpuRenderState, GpuScene, GpuSurface, GpuVec2,
};
use super::pipeline::{read_back_f32, read_back_rgba, RayTracePipeline};
use crate::pathtrace::Film;

/// Usage flags for a storage buffer this module rewrites in place.
const STORAGE_RW: wgpu::BufferUsages =
    wgpu::BufferUsages::STORAGE.union(wgpu::BufferUsages::COPY_DST);

/// A storage buffer plus the byte capacity actually allocated for it, so a
/// smaller update can be written in place and a larger one knows to grow.
struct Slab {
    buffer: wgpu::Buffer,
    capacity: u64,
}

impl Slab {
    fn new(ctx: &GpuContext, label: &str, contents: &[u8]) -> Self {
        let buffer = ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents,
                usage: STORAGE_RW,
            });
        Self {
            capacity: contents.len() as u64,
            buffer,
        }
    }

    /// Write `contents`, reallocating only if it no longer fits. Returns true
    /// when the buffer was replaced, which means the bind group is stale.
    fn write(&mut self, ctx: &GpuContext, label: &str, contents: &[u8]) -> bool {
        if contents.len() as u64 <= self.capacity {
            ctx.queue.write_buffer(&self.buffer, 0, contents);
            // The tail past `contents` keeps whatever the previous scene left
            // there. Nothing reads it: every array is indexed through counts
            // that came from the same upload.
            return false;
        }
        *self = Self::new(ctx, label, contents);
        true
    }
}

/// Everything a pass needs that depends only on the frame size.
struct FrameTargets {
    width: u32,
    height: u32,
    padded_bytes_per_row: u32,
    output: wgpu::Texture,
    output_view: wgpu::TextureView,
    accum: wgpu::Buffer,
    depth_normal: wgpu::Buffer,
    feature_id: wgpu::Buffer,
    readback: wgpu::Buffer,
    /// Staging buffer for the linear exit: the accumulation buffer followed by
    /// the depth/normal buffer's two guide planes. Allocated on first use, so
    /// a caller that never asks for linear output never pays for it.
    linear_readback: Option<wgpu::Buffer>,
}

impl FrameTargets {
    fn new(ctx: &GpuContext, width: u32, height: u32) -> Self {
        let output = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Resident Output Texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let output_view = output.create_view(&Default::default());
        let per_pixel_vec4 = (width as u64) * (height as u64) * 16;
        let mk = |label: &str, size: u64| {
            ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                // COPY_SRC so the linear exit can read the accumulated
                // radiance and the guide planes back without a second pass.
                size,
                usage: STORAGE_RW.union(wgpu::BufferUsages::COPY_SRC),
                mapped_at_creation: false,
            })
        };
        let padded_bytes_per_row = (width * 4).div_ceil(256) * 256;
        Self {
            width,
            height,
            padded_bytes_per_row,
            output,
            output_view,
            accum: mk("Resident Accumulation Buffer", per_pixel_vec4),
            // Three planes: the shader's own (normal, t), then the two guide
            // planes a raw-sample pass fills. See the binding's comment in
            // `raytrace.wgsl`.
            depth_normal: mk("Resident Depth Normal Buffer", per_pixel_vec4 * 3),
            feature_id: mk(
                "Resident Feature ID Buffer",
                (width as u64) * (height as u64) * 4,
            ),
            readback: ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Resident Readback Buffer"),
                size: (padded_bytes_per_row as u64) * (height as u64),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }),
            linear_readback: None,
        }
    }

    /// The linear staging buffer, allocated on first use.
    fn linear_staging(&mut self, ctx: &GpuContext) -> &wgpu::Buffer {
        let bytes = (self.width as u64) * (self.height as u64) * 16;
        self.linear_readback.get_or_insert_with(|| {
            ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Resident Linear Readback Buffer"),
                // accumulation + the two guide planes.
                size: bytes * 3,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            })
        })
    }
}

/// The environment's two textures, plus enough of a signature to know when a
/// new scene needs new ones.
struct EnvTextures {
    pixels: wgpu::TextureView,
    cdf: wgpu::TextureView,
    /// `None` for the analytic gradient, else the image's dimensions.
    signature: Option<(u32, u32)>,
}

fn upload_tex(
    ctx: &GpuContext,
    label: &str,
    w: u32,
    h: u32,
    fmt: wgpu::TextureFormat,
    data: &[f32],
) -> wgpu::TextureView {
    let tex = ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: fmt,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let bytes_per_px = if fmt == wgpu::TextureFormat::Rgba32Float {
        16
    } else {
        4
    };
    ctx.queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        bytemuck::cast_slice(data),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(w * bytes_per_px),
            rows_per_image: Some(h),
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    tex.create_view(&wgpu::TextureViewDescriptor::default())
}

impl EnvTextures {
    fn new(ctx: &GpuContext, scene: &GpuScene) -> Self {
        match &scene.environment {
            Some(e) if e.width > 0 && e.height > 0 => Self {
                pixels: upload_tex(
                    ctx,
                    "Resident Environment Pixels",
                    e.width,
                    e.height,
                    wgpu::TextureFormat::Rgba32Float,
                    &e.pixels,
                ),
                cdf: upload_tex(
                    ctx,
                    "Resident Environment CDF",
                    e.width + 1,
                    e.height + 1,
                    wgpu::TextureFormat::R32Float,
                    &e.cdf,
                ),
                signature: Some((e.width, e.height)),
            },
            // A texture binding cannot be null, so a gradient-lit scene still
            // gets 1x1 dummies; `env_mode` is what the shader branches on.
            _ => Self {
                pixels: upload_tex(
                    ctx,
                    "Resident Environment Pixels (unused)",
                    1,
                    1,
                    wgpu::TextureFormat::Rgba32Float,
                    &[0.0, 0.0, 0.0, 1.0],
                ),
                cdf: upload_tex(
                    ctx,
                    "Resident Environment CDF (unused)",
                    1,
                    1,
                    wgpu::TextureFormat::R32Float,
                    &[0.0],
                ),
                signature: None,
            },
        }
    }

    fn signature_of(scene: &GpuScene) -> Option<(u32, u32)> {
        match &scene.environment {
            Some(e) if e.width > 0 && e.height > 0 => Some((e.width, e.height)),
            _ => None,
        }
    }
}

/// Pad a packed array to at least one element: WGSL cannot bind a zero-length
/// storage array, and every one of these is indexed through a count that came
/// from the same upload, so the filler is never read.
fn at_least_one<T: Clone + Zeroable>(v: &[T]) -> Vec<T> {
    if v.is_empty() {
        vec![T::zeroed()]
    } else {
        v.to_vec()
    }
}

/// A packed scene held on the GPU across passes.
///
/// Build one with [`RayTracePipeline::resident_scene`], hand it new placements
/// with [`ResidentScene::update_scene`], and render it with
/// [`RayTracePipeline::render_resident`] or
/// [`RayTracePipeline::render_resident_into`].
pub struct ResidentScene {
    surfaces: Slab,
    faces: Slab,
    bvh: Slab,
    trim: Slab,
    inner_loops: Slab,
    materials: Slab,
    lights: Slab,
    env: EnvTextures,
    targets: FrameTargets,
    camera_buffer: wgpu::Buffer,
    render_state_buffer: wgpu::Buffer,
    /// Cached bind group for the scene's own output texture. Dropped whenever
    /// a buffer or target is replaced.
    bind_group: Option<wgpu::BindGroup>,
    /// Scene-derived render-state fields, refreshed on every `update_scene` so
    /// a caller's state cannot disagree with the buffers actually bound.
    light_count: u32,
    env_state: (u32, u32, u32, f32, f32, f32),
}

impl ResidentScene {
    fn build(ctx: &GpuContext, scene: &GpuScene, width: u32, height: u32) -> Self {
        let mut lights = at_least_one(&scene.lights);
        pack_light_power_table(&mut lights);
        let mut me = Self {
            surfaces: Slab::new(
                ctx,
                "Resident Surfaces",
                bytemuck::cast_slice(&at_least_one::<GpuSurface>(&scene.surfaces)),
            ),
            faces: Slab::new(
                ctx,
                "Resident Faces",
                bytemuck::cast_slice(&at_least_one::<GpuFace>(&scene.faces)),
            ),
            bvh: Slab::new(
                ctx,
                "Resident BVH",
                bytemuck::cast_slice(&at_least_one::<GpuBvhNode>(&scene.bvh_nodes)),
            ),
            trim: Slab::new(
                ctx,
                "Resident Trim",
                bytemuck::cast_slice(&at_least_one::<GpuVec2>(&scene.trim_verts)),
            ),
            inner_loops: Slab::new(
                ctx,
                "Resident Inner Loop Descs",
                bytemuck::cast_slice(&at_least_one::<u32>(&scene.inner_loop_descs)),
            ),
            materials: Slab::new(
                ctx,
                "Resident Materials",
                bytemuck::cast_slice(&at_least_one::<GpuMaterial>(&scene.materials)),
            ),
            lights: Slab::new(ctx, "Resident Area Lights", bytemuck::cast_slice(&lights)),
            env: EnvTextures::new(ctx, scene),
            targets: FrameTargets::new(ctx, width, height),
            camera_buffer: ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Resident Camera Buffer"),
                size: std::mem::size_of::<GpuCamera>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            render_state_buffer: ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Resident Render State Buffer"),
                size: std::mem::size_of::<GpuRenderState>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            bind_group: None,
            light_count: 0,
            env_state: (0, 0, 0, 0.0, 0.0, 0.0),
        };
        me.refresh_scene_state(scene);
        me
    }

    fn refresh_scene_state(&mut self, scene: &GpuScene) {
        self.light_count = scene.lights.len() as u32;
        self.env_state = match &scene.environment {
            Some(e) => (1, e.width, e.height, e.intensity, e.rotation, e.marg_int),
            None => (0, 0, 0, 0.0, 0.0, 0.0),
        };
    }

    /// The frame size this scene's targets are allocated for.
    pub fn size(&self) -> (u32, u32) {
        (self.targets.width, self.targets.height)
    }

    /// Re-upload the scene's geometry, materials and lights.
    ///
    /// This is the call for a new frame of an animation: hand it
    /// `packed.placed(&transform)` and only the bytes that moved are written.
    /// Buffers grow — and the bind group is rebuilt — only when a scene
    /// arrives that no longer fits.
    ///
    /// Accumulation is *not* reset, because only the caller knows whether the
    /// new placement invalidates the frames already averaged in. It almost
    /// always does; call [`ResidentScene::reset_accumulation`].
    pub fn update_scene(&mut self, ctx: &GpuContext, scene: &GpuScene) {
        let mut stale = false;
        stale |= self.surfaces.write(
            ctx,
            "Resident Surfaces",
            bytemuck::cast_slice(&at_least_one::<GpuSurface>(&scene.surfaces)),
        );
        stale |= self.faces.write(
            ctx,
            "Resident Faces",
            bytemuck::cast_slice(&at_least_one::<GpuFace>(&scene.faces)),
        );
        stale |= self.bvh.write(
            ctx,
            "Resident BVH",
            bytemuck::cast_slice(&at_least_one::<GpuBvhNode>(&scene.bvh_nodes)),
        );
        stale |= self.trim.write(
            ctx,
            "Resident Trim",
            bytemuck::cast_slice(&at_least_one::<GpuVec2>(&scene.trim_verts)),
        );
        stale |= self.inner_loops.write(
            ctx,
            "Resident Inner Loop Descs",
            bytemuck::cast_slice(&at_least_one::<u32>(&scene.inner_loop_descs)),
        );
        stale |= self.materials.write(
            ctx,
            "Resident Materials",
            bytemuck::cast_slice(&at_least_one::<GpuMaterial>(&scene.materials)),
        );
        stale |= self.set_lights_inner(ctx, &scene.lights);

        // The environment is an image, not a placement: only a different one
        // costs an upload.
        if EnvTextures::signature_of(scene) != self.env.signature {
            self.env = EnvTextures::new(ctx, scene);
            stale = true;
        }
        self.refresh_scene_state(scene);
        if stale {
            self.bind_group = None;
        }
    }

    fn set_lights_inner(&mut self, ctx: &GpuContext, lights: &[GpuAreaLight]) -> bool {
        let mut packed = at_least_one(lights);
        pack_light_power_table(&mut packed);
        self.lights
            .write(ctx, "Resident Area Lights", bytemuck::cast_slice(&packed))
    }

    /// Replace just the light rig, re-deriving the power table.
    ///
    /// The rest of the scene stays exactly where it is, which is what a
    /// lighting tweak in a viewport wants.
    pub fn set_lights(&mut self, ctx: &GpuContext, lights: &[GpuAreaLight]) {
        if self.set_lights_inner(ctx, lights) {
            self.bind_group = None;
        }
        self.light_count = lights.len() as u32;
    }

    /// Reallocate the frame-sized targets for a new resolution.
    ///
    /// A no-op at the current size, so it is safe to call every frame.
    /// Accumulation starts over: the buffer is new.
    pub fn resize(&mut self, ctx: &GpuContext, width: u32, height: u32) {
        if (width, height) == (self.targets.width, self.targets.height) {
            return;
        }
        self.targets = FrameTargets::new(ctx, width, height);
        self.bind_group = None;
    }

    /// Zero the accumulation buffer, so the next pass at `frame_index` 1
    /// starts a fresh average. Call this after any change the accumulated
    /// frames no longer describe — a camera move, a re-pose, a new rig.
    pub fn reset_accumulation(&mut self, ctx: &GpuContext) {
        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Resident Accumulation Reset"),
            });
        encoder.clear_buffer(&self.targets.accum, 0, None);
        encoder.clear_buffer(&self.targets.depth_normal, 0, None);
        ctx.queue.submit(Some(encoder.finish()));
    }

    /// Fill in the render-state fields that describe the buffers actually
    /// bound, so a caller's state cannot drift out of step with them.
    fn derive_state(&self, mut state: GpuRenderState) -> GpuRenderState {
        state.light_count = self.light_count;
        let (mode, w, h, intensity, rotation, marg) = self.env_state;
        state.env_mode = mode;
        if mode == 1 {
            state.env_width = w;
            state.env_height = h;
            state.env_intensity = intensity;
            state.env_rotation = rotation;
            state.env_marg_int = marg;
        }
        state
    }

    fn bind_entries<'a>(&'a self, output: &'a wgpu::TextureView) -> [wgpu::BindGroupEntry<'a>; 15] {
        [
            wgpu::BindGroupEntry {
                binding: 0,
                resource: self.camera_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: self.surfaces.buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: self.faces.buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: self.bvh.buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: self.trim.buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: self.inner_loops.buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::TextureView(output),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: self.render_state_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 8,
                resource: self.targets.accum.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 9,
                resource: self.materials.buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 10,
                resource: self.targets.depth_normal.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 11,
                resource: self.lights.buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 12,
                resource: self.targets.feature_id.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 13,
                resource: wgpu::BindingResource::TextureView(&self.env.pixels),
            },
            wgpu::BindGroupEntry {
                binding: 14,
                resource: wgpu::BindingResource::TextureView(&self.env.cdf),
            },
        ]
    }
}

impl RayTracePipeline {
    /// Upload `scene` and keep it on the GPU for repeated passes at
    /// `width` x `height`.
    pub fn resident_scene(
        &self,
        ctx: &GpuContext,
        scene: &GpuScene,
        width: u32,
        height: u32,
    ) -> ResidentScene {
        let _ = self;
        ResidentScene::build(ctx, scene, width, height)
    }

    /// Encode one pass over a resident scene into `encoder`, writing into
    /// `output`. Shared by both entry points.
    fn encode_resident(
        &self,
        ctx: &GpuContext,
        res: &mut ResidentScene,
        camera: &GpuCamera,
        state: GpuRenderState,
        output: Option<&wgpu::TextureView>,
        encoder: &mut wgpu::CommandEncoder,
    ) {
        let state = res.derive_state(state);
        ctx.queue
            .write_buffer(&res.camera_buffer, 0, bytemuck::bytes_of(camera));
        ctx.queue
            .write_buffer(&res.render_state_buffer, 0, bytemuck::bytes_of(&state));

        // A caller-supplied target gets a fresh bind group — it is a different
        // view every time and there is nothing to cache. The scene's own
        // texture keeps one, which is the whole point of being resident.
        let transient;
        let bind_group = match output {
            Some(view) => {
                transient = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("Resident Ray Trace Bind Group (external target)"),
                    layout: self.layout(),
                    entries: &res.bind_entries(view),
                });
                &transient
            }
            None => {
                if res.bind_group.is_none() {
                    let view = res.targets.output_view.clone();
                    res.bind_group =
                        Some(ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                            label: Some("Resident Ray Trace Bind Group"),
                            layout: self.layout(),
                            entries: &res.bind_entries(&view),
                        }));
                }
                res.bind_group.as_ref().expect("just built")
            }
        };

        let (dw, dh) = match state.scissor() {
            Some([x, y, w, h]) => (
                w.min(res.targets.width.saturating_sub(x)),
                h.min(res.targets.height.saturating_sub(y)),
            ),
            None => (res.targets.width, res.targets.height),
        };

        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Resident Ray Trace Pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(self.compute_pipeline());
            pass.set_bind_group(0, bind_group, &[]);
            pass.dispatch_workgroups(dw.div_ceil(8), dh.div_ceil(8), 1);
        }
        if state.refine_sample_count > 0 {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Resident Ray Trace Refine Pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(self.refine_compute_pipeline());
            pass.set_bind_group(0, bind_group, &[]);
            pass.dispatch_workgroups(dw.div_ceil(8), dh.div_ceil(8), 1);
        }
    }

    /// Render a resident scene and read the frame back as RGBA8.
    ///
    /// Byte-for-byte what [`RayTracePipeline::render_with_render_state`] would
    /// have produced for the same scene, camera and state — the only
    /// difference is that nothing was reallocated to get it.
    pub async fn render_resident(
        &self,
        ctx: &GpuContext,
        res: &mut ResidentScene,
        camera: &GpuCamera,
        state: GpuRenderState,
    ) -> Result<Vec<u8>, GpuError> {
        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Resident Ray Trace Encoder"),
            });
        self.encode_resident(ctx, res, camera, state, None, &mut encoder);
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &res.targets.output,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &res.targets.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(res.targets.padded_bytes_per_row),
                    rows_per_image: Some(res.targets.height),
                },
            },
            wgpu::Extent3d {
                width: res.targets.width,
                height: res.targets.height,
                depth_or_array_layers: 1,
            },
        );
        ctx.queue.submit(Some(encoder.finish()));
        read_back_rgba(
            ctx,
            &res.targets.readback,
            res.targets.width,
            res.targets.height,
            res.targets.padded_bytes_per_row,
        )
        .await
    }

    /// Render one pass and read it back in **linear** space, with the
    /// denoiser's guide buffers, as a [`Film`].
    ///
    /// This is the exit for a host that keeps its own per-pixel history. The
    /// other two write display-referred 8-bit pixels, which cannot be averaged
    /// or reprojected without undoing a tonemap that is not invertible.
    ///
    /// `state` is forced into raw-sample mode
    /// ([`GpuRenderState::set_raw_sample`]) and its refinement pass is turned
    /// off, so what comes back is **one unweighted sample** of the image, not
    /// a step of the shader's running average. `state.frame_index` still
    /// drives the jitter and the RNG: pass an increasing index and successive
    /// calls are independent samples of the same picture, which is exactly
    /// what a caller-side mean wants.
    ///
    /// The film matches [`crate::pathtrace::render`]'s conventions field for
    /// field, so [`crate::pathtrace::denoise`] and any reprojection written
    /// against the CPU tier work on it unchanged:
    ///
    /// * `rgb` — linear radiance, no tonemap, no gamma.
    /// * `alpha` — the path tracer's coverage, 1 where the primary ray hit.
    /// * `depth` — distance from the eye along the primary ray, in world
    ///   units, and **0 for background**. The shader's own depth buffer uses
    ///   `MAX_T` for background and is left alone; this is converted on the
    ///   way out.
    /// * `normal` — world normal at the first hit, face-forwarded against the
    ///   view ray (what the shading actually used), zero for background.
    /// * `albedo` — the first hit's `denoise_albedo`: diffuse albedo mixed
    ///   toward F0 by metallic, zero for background.
    /// * `variance` — 1 spp carries no spread of its own, so this is the
    ///   luminance squared, the same fallback `trace_pixel` uses at `spp == 1`.
    ///
    /// # Scissor
    ///
    /// A scissored pass traces only its rectangle, and the film is read back
    /// from buffers that persist between calls, so **pixels outside the
    /// rectangle carry whatever the previous pass left there** — stale, not
    /// zero, and on the very first pass after allocation, zero. That is
    /// deliberate: it is what lets a caller re-render a rectangle into a
    /// history it is otherwise keeping. Call
    /// [`ResidentScene::reset_accumulation`] if you want the untouched region
    /// zeroed instead.
    pub async fn render_resident_linear(
        &self,
        ctx: &GpuContext,
        res: &mut ResidentScene,
        camera: &GpuCamera,
        state: GpuRenderState,
    ) -> Result<Film, GpuError> {
        let mut state = state;
        state.set_raw_sample(true);
        // The refinement pass blends into the accumulation buffer with weights
        // of its own and would make the readback something other than one
        // sample.
        state.refine_sample_count = 0;

        let (w, h) = (res.targets.width, res.targets.height);
        let n = (w as u64) * (h as u64);
        let plane = n * 16;

        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Resident Linear Ray Trace Encoder"),
            });
        self.encode_resident(ctx, res, camera, state, None, &mut encoder);
        res.targets.linear_staging(ctx);
        {
            let staging = res
                .targets
                .linear_readback
                .as_ref()
                .expect("staging buffer was just allocated");
            encoder.copy_buffer_to_buffer(&res.targets.accum, 0, staging, 0, plane);
            // The two guide planes, which start one plane into the depth/normal
            // buffer — the first plane is the shader's own edge-detection copy.
            encoder.copy_buffer_to_buffer(
                &res.targets.depth_normal,
                plane,
                staging,
                plane,
                plane * 2,
            );
        }
        ctx.queue.submit(Some(encoder.finish()));

        let staging = res
            .targets
            .linear_readback
            .as_ref()
            .expect("staging buffer was just allocated");
        let raw = read_back_f32(ctx, staging).await?;

        let n = n as usize;
        let mut film = Film {
            width: w,
            height: h,
            rgb: vec![0.0; n * 3],
            alpha: vec![0.0; n],
            normal: vec![0.0; n * 3],
            depth: vec![0.0; n],
            albedo: vec![0.0; n * 3],
            variance: vec![0.0; n],
        };
        for i in 0..n {
            let c = &raw[i * 4..i * 4 + 4];
            let g = &raw[(n + i) * 4..(n + i) * 4 + 4];
            let a = &raw[(2 * n + i) * 4..(2 * n + i) * 4 + 3];
            film.rgb[i * 3..i * 3 + 3].copy_from_slice(&c[..3]);
            film.alpha[i] = c[3];
            film.normal[i * 3..i * 3 + 3].copy_from_slice(&g[..3]);
            film.depth[i] = g[3];
            film.albedo[i * 3..i * 3 + 3].copy_from_slice(a);
            // Matching `trace_pixel`'s single-sample fallback: one sample says
            // nothing about its own spread, so its magnitude stands in for it.
            let l = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
            film.variance[i] = l * l;
        }
        Ok(film)
    }

    /// Render a resident scene straight into a storage texture the caller
    /// owns, with no readback at all.
    ///
    /// `target` must be a view of an `Rgba8Unorm` texture with
    /// `STORAGE_BINDING` usage, at least the scene's size. The work is
    /// submitted and this returns immediately; the caller sequences it with
    /// whatever presents the texture.
    pub fn render_resident_into(
        &self,
        ctx: &GpuContext,
        res: &mut ResidentScene,
        camera: &GpuCamera,
        state: GpuRenderState,
        target: &wgpu::TextureView,
    ) {
        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Resident Ray Trace Encoder (external target)"),
            });
        self.encode_resident(ctx, res, camera, state, Some(target), &mut encoder);
        ctx.queue.submit(Some(encoder.finish()));
    }
}
