//! wgpu compute pipeline for ray tracing.

#[cfg(feature = "gpu")]
use vcad_kernel_gpu::{GpuContext, GpuError};

#[cfg(feature = "gpu")]
use bytemuck::Zeroable;

#[cfg(feature = "gpu")]
use super::buffers::{GpuCamera, GpuRenderState, GpuScene};

#[cfg(not(feature = "gpu"))]
use super::buffers::GpuCamera;

/// Ray tracing compute pipeline.
///
/// Note: This requires the `gpu` feature to be enabled.
#[cfg(feature = "gpu")]
pub struct RayTracePipeline {
    pipeline: wgpu::ComputePipeline,
    /// Second pass that refines edge pixels with additional stratified samples.
    refine_pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,
}

#[cfg(feature = "gpu")]
impl RayTracePipeline {
    /// Create a new ray trace pipeline.
    pub fn new(ctx: &GpuContext) -> Result<Self, GpuError> {
        let shader_module = ctx
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Ray Trace Shader"),
                source: wgpu::ShaderSource::Wgsl(super::shaders::raytrace_shader().into()),
            });

        let bind_group_layout =
            ctx.device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("Ray Trace Bind Group Layout"),
                    entries: &[
                        // Camera uniform
                        wgpu::BindGroupLayoutEntry {
                            binding: 0,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Uniform,
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        // Surfaces storage
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
                        // Faces storage
                        wgpu::BindGroupLayoutEntry {
                            binding: 2,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Storage { read_only: true },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        // BVH nodes storage
                        wgpu::BindGroupLayoutEntry {
                            binding: 3,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Storage { read_only: true },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        // Trim vertices storage
                        wgpu::BindGroupLayoutEntry {
                            binding: 4,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Storage { read_only: true },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        // Inner loop descriptors (vertex counts for each inner loop)
                        wgpu::BindGroupLayoutEntry {
                            binding: 5,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Storage { read_only: true },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        // Output texture
                        wgpu::BindGroupLayoutEntry {
                            binding: 6,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::StorageTexture {
                                access: wgpu::StorageTextureAccess::WriteOnly,
                                format: wgpu::TextureFormat::Rgba8Unorm,
                                view_dimension: wgpu::TextureViewDimension::D2,
                            },
                            count: None,
                        },
                        // Render state uniform (for progressive rendering)
                        wgpu::BindGroupLayoutEntry {
                            binding: 7,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Uniform,
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        // Accumulation buffer (for progressive rendering)
                        // Using storage buffer instead of ReadWrite texture for WebGPU compatibility
                        wgpu::BindGroupLayoutEntry {
                            binding: 8,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Storage { read_only: false },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        // Materials storage
                        wgpu::BindGroupLayoutEntry {
                            binding: 9,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Storage { read_only: true },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        // Depth/normal buffer (for edge detection)
                        // Using storage buffer instead of ReadWrite texture for WebGPU compatibility
                        wgpu::BindGroupLayoutEntry {
                            binding: 10,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Storage { read_only: false },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        // Area lights (softboxes) for MIS-weighted direct lighting
                        wgpu::BindGroupLayoutEntry {
                            binding: 11,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Storage { read_only: true },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        // HDR environment. Textures, not storage buffers: the
                        // ten bindings above already exhaust the browser's
                        // maxStorageBuffersPerShaderStage.
                        wgpu::BindGroupLayoutEntry {
                            binding: 13,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Texture {
                                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                                view_dimension: wgpu::TextureViewDimension::D2,
                                multisampled: false,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 14,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Texture {
                                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                                view_dimension: wgpu::TextureViewDimension::D2,
                                multisampled: false,
                            },
                            count: None,
                        },
                        // Feature ID buffer (per-pixel face_idx for analytic crease detection)
                        wgpu::BindGroupLayoutEntry {
                            binding: 12,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Storage { read_only: false },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                    ],
                });

        let pipeline_layout = ctx
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Ray Trace Pipeline Layout"),
                bind_group_layouts: &[&bind_group_layout],
                push_constant_ranges: &[],
            });

        let pipeline = ctx
            .device
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("Ray Trace Pipeline"),
                layout: Some(&pipeline_layout),
                module: &shader_module,
                entry_point: Some("main"),
                compilation_options: Default::default(),
                cache: None,
            });

        let refine_pipeline =
            ctx.device
                .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some("Ray Trace Refine Pipeline"),
                    layout: Some(&pipeline_layout),
                    module: &shader_module,
                    entry_point: Some("refine"),
                    compilation_options: Default::default(),
                    cache: None,
                });

        Ok(Self {
            pipeline,
            refine_pipeline,
            bind_group_layout,
        })
    }

    /// Render a scene to an output texture with progressive accumulation.
    ///
    /// This function is async to support WASM's single-threaded environment where
    /// blocking GPU buffer readback causes deadlocks. The async wrapper allows
    /// wasm-bindgen-futures to yield control back to the browser event loop.
    ///
    /// # Arguments
    /// * `ctx` - GPU context
    /// * `scene` - Scene data to render
    /// * `camera` - Camera parameters
    /// * `width` - Output width in pixels
    /// * `height` - Output height in pixels
    /// * `frame_index` - Frame number for progressive accumulation (1 = first frame/reset)
    /// * `accum_buffer` - Optional accumulation buffer from previous frames
    ///
    /// # Returns
    /// A tuple of (pixels, accumulation_buffer) for progressive rendering.
    #[allow(clippy::too_many_arguments)]
    pub async fn render_progressive(
        &self,
        ctx: &GpuContext,
        scene: &GpuScene,
        camera: &GpuCamera,
        width: u32,
        height: u32,
        frame_index: u32,
        accum_buffer: Option<wgpu::Buffer>,
    ) -> Result<(Vec<u8>, wgpu::Buffer), GpuError> {
        self.render_progressive_with_debug(
            ctx,
            scene,
            camera,
            width,
            height,
            frame_index,
            accum_buffer,
            0,
        )
        .await
    }

    /// Render a scene with debug visualization mode.
    ///
    /// # Arguments
    /// * Same as render_progressive, plus:
    /// * `debug_mode` - Debug visualization: 0=normal, 1=normals as RGB, 2=face_id, 3=n_dot_l, 4=orientation
    #[allow(clippy::too_many_arguments)]
    pub async fn render_progressive_with_debug(
        &self,
        ctx: &GpuContext,
        scene: &GpuScene,
        camera: &GpuCamera,
        width: u32,
        height: u32,
        frame_index: u32,
        accum_buffer: Option<wgpu::Buffer>,
        debug_mode: u32,
    ) -> Result<(Vec<u8>, wgpu::Buffer), GpuError> {
        // Delegate to full settings with default edge, AO, and refinement parameters
        self.render_with_full_settings(
            ctx,
            scene,
            camera,
            width,
            height,
            frame_index,
            accum_buffer,
            debug_mode,
            true,
            0.1,
            30.0,
            0,
            0,
        )
        .await
    }

    /// Render a scene with full control over all settings.
    ///
    /// # Arguments
    /// * Same as render_progressive_with_debug, plus:
    /// * `enable_edges` - Whether to show edge detection overlay
    /// * `edge_depth_threshold` - Depth discontinuity threshold for edges
    /// * `edge_normal_threshold` - Normal angle threshold (degrees) for edges
    /// * `theme` - Visual theme (0=dark, 1=light)
    /// * `refine_sample_count` - Additional rays per edge pixel (0=disabled, 4/9/16 typical)
    #[allow(clippy::too_many_arguments)]
    pub async fn render_with_full_settings(
        &self,
        ctx: &GpuContext,
        scene: &GpuScene,
        camera: &GpuCamera,
        width: u32,
        height: u32,
        frame_index: u32,
        accum_buffer: Option<wgpu::Buffer>,
        debug_mode: u32,
        enable_edges: bool,
        edge_depth_threshold: f32,
        edge_normal_threshold: f32,
        theme: u32,
        refine_sample_count: u32,
    ) -> Result<(Vec<u8>, wgpu::Buffer), GpuError> {
        let render_state = GpuRenderState::with_refinement(
            frame_index,
            debug_mode,
            enable_edges,
            edge_depth_threshold,
            edge_normal_threshold,
            theme,
            refine_sample_count,
        );
        let (pixels, accum) = self
            .render_with_render_state(
                ctx,
                scene,
                camera,
                width,
                height,
                accum_buffer,
                render_state,
            )
            .await?;
        Ok((pixels, accum))
    }

    /// Render with a fully-constructed `GpuRenderState` (supports per-type edge style).
    #[allow(clippy::too_many_arguments)]
    pub async fn render_with_render_state(
        &self,
        ctx: &GpuContext,
        scene: &GpuScene,
        camera: &GpuCamera,
        width: u32,
        height: u32,
        accum_buffer: Option<wgpu::Buffer>,
        render_state: GpuRenderState,
    ) -> Result<(Vec<u8>, wgpu::Buffer), GpuError> {
        use wgpu::util::DeviceExt;

        // Derive the scene-dependent state here rather than trusting callers to
        // keep it in sync with the buffers we are about to bind.
        let mut render_state = render_state;
        render_state.light_count = scene.lights.len() as u32;
        match &scene.environment {
            Some(e) => {
                render_state.env_mode = 1;
                render_state.env_width = e.width;
                render_state.env_height = e.height;
                render_state.env_intensity = e.intensity;
                render_state.env_rotation = e.rotation;
                render_state.env_marg_int = e.marg_int;
            }
            None => render_state.env_mode = 0,
        }

        // Create camera buffer
        let camera_buffer = ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Camera Buffer"),
                contents: bytemuck::bytes_of(camera),
                usage: wgpu::BufferUsages::UNIFORM,
            });

        // Create render state buffer
        let render_state_buffer =
            ctx.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Render State Buffer"),
                    contents: bytemuck::bytes_of(&render_state),
                    usage: wgpu::BufferUsages::UNIFORM,
                });

        // Create scene buffers (with at least 1 element to avoid zero-size buffers)
        let surfaces = if scene.surfaces.is_empty() {
            vec![super::buffers::GpuSurface::zeroed()]
        } else {
            scene.surfaces.clone()
        };
        let surfaces_buffer = ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Surfaces Buffer"),
                contents: bytemuck::cast_slice(&surfaces),
                usage: wgpu::BufferUsages::STORAGE,
            });

        let faces = if scene.faces.is_empty() {
            vec![super::buffers::GpuFace::zeroed()]
        } else {
            scene.faces.clone()
        };
        let faces_buffer = ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Faces Buffer"),
                contents: bytemuck::cast_slice(&faces),
                usage: wgpu::BufferUsages::STORAGE,
            });

        let bvh_nodes = if scene.bvh_nodes.is_empty() {
            vec![super::buffers::GpuBvhNode::zeroed()]
        } else {
            scene.bvh_nodes.clone()
        };
        let bvh_buffer = ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("BVH Buffer"),
                contents: bytemuck::cast_slice(&bvh_nodes),
                usage: wgpu::BufferUsages::STORAGE,
            });

        let trim_verts = if scene.trim_verts.is_empty() {
            vec![super::buffers::GpuVec2 { x: 0.0, y: 0.0 }]
        } else {
            scene.trim_verts.clone()
        };
        let trim_buffer = ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Trim Buffer"),
                contents: bytemuck::cast_slice(&trim_verts),
                usage: wgpu::BufferUsages::STORAGE,
            });

        let inner_loop_descs = if scene.inner_loop_descs.is_empty() {
            vec![0u32]
        } else {
            scene.inner_loop_descs.clone()
        };
        let inner_loop_descs_buffer =
            ctx.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Inner Loop Descs Buffer"),
                    contents: bytemuck::cast_slice(&inner_loop_descs),
                    usage: wgpu::BufferUsages::STORAGE,
                });

        let materials = if scene.materials.is_empty() {
            vec![super::buffers::GpuMaterial::default()]
        } else {
            scene.materials.clone()
        };
        let materials_buffer = ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Materials Buffer"),
                contents: bytemuck::cast_slice(&materials),
                usage: wgpu::BufferUsages::STORAGE,
            });

        // Create output texture
        let output_texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Output Texture"),
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
        let output_view = output_texture.create_view(&Default::default());

        // Create or reuse accumulation buffer (4 floats per pixel: r, g, b, count)
        let accum_buf_size = (width * height * 16) as u64; // 4 * sizeof(f32)
        let accum = accum_buffer.unwrap_or_else(|| {
            ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Accumulation Buffer"),
                size: accum_buf_size,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            })
        });

        // Depth/normal buffer for edge detection (vec4 per pixel: normal.xyz, depth).
        let depth_normal_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Depth Normal Buffer"),
            size: accum_buf_size,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });

        // Area lights. WGSL cannot bind a zero-length storage array, so an
        // unlit scene still gets one dummy entry; `light_count` is what the
        // shader actually loops over.
        let lights: Vec<super::buffers::GpuAreaLight> = if scene.lights.is_empty() {
            vec![super::buffers::GpuAreaLight::default()]
        } else {
            scene.lights.clone()
        };
        let light_buf = ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Area Light Buffer"),
                contents: bytemuck::cast_slice(&lights),
                usage: wgpu::BufferUsages::STORAGE,
            });

        // HDR environment. A gradient-lit scene still gets 1x1 dummies — a
        // texture binding cannot be null — and `env_mode` is what the shader
        // actually branches on.
        let mk_tex = |label: &str, w: u32, h: u32, fmt: wgpu::TextureFormat, data: &[f32]| {
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
                wgpu::ImageCopyTexture {
                    texture: &tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                bytemuck::cast_slice(data),
                wgpu::ImageDataLayout {
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
        };

        let (env_pixels_view, env_cdf_view) = match &scene.environment {
            Some(e) if e.width > 0 && e.height > 0 => (
                mk_tex(
                    "Environment Pixels",
                    e.width,
                    e.height,
                    wgpu::TextureFormat::Rgba32Float,
                    &e.pixels,
                ),
                mk_tex(
                    "Environment CDF",
                    e.width + 1,
                    e.height + 1,
                    wgpu::TextureFormat::R32Float,
                    &e.cdf,
                ),
            ),
            _ => (
                mk_tex(
                    "Environment Pixels (unused)",
                    1,
                    1,
                    wgpu::TextureFormat::Rgba32Float,
                    &[0.0, 0.0, 0.0, 1.0],
                ),
                mk_tex(
                    "Environment CDF (unused)",
                    1,
                    1,
                    wgpu::TextureFormat::R32Float,
                    &[0.0],
                ),
            ),
        };

        // Feature ID buffer: one u32 per pixel storing face_idx (0xFFFFFFFF = background).
        // Written at frame 1 and reused by the crease detector on subsequent frames.
        let feature_id_buf_size = (width * height * 4) as u64;
        let feature_id_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Feature ID Buffer"),
            size: feature_id_buf_size,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });

        // Create readback buffer
        let output_size = (width * height * 4) as u64;
        let padded_bytes_per_row = (width * 4).div_ceil(256) * 256;
        let readback_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Readback Buffer"),
            size: (padded_bytes_per_row * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        // Create bind group
        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Ray Trace Bind Group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: camera_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: surfaces_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: faces_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: bvh_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: trim_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: inner_loop_descs_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::TextureView(&output_view),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: render_state_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: accum.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 9,
                    resource: materials_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 10,
                    resource: depth_normal_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 11,
                    resource: light_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 12,
                    resource: feature_id_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 13,
                    resource: wgpu::BindingResource::TextureView(&env_pixels_view),
                },
                wgpu::BindGroupEntry {
                    binding: 14,
                    resource: wgpu::BindingResource::TextureView(&env_cdf_view),
                },
            ],
        });

        // Dispatch compute shader
        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Ray Trace Encoder"),
            });

        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Ray Trace Pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(width.div_ceil(8), height.div_ceil(8), 1);
        }

        // The SSAO pass used to run here. Real multi-bounce GI computes contact
        // occlusion correctly, so a screen-space proxy on top of it would only
        // double-darken concave regions.

        // Adaptive refinement pass: fires extra stratified rays at edge pixels.
        // The main pass must fully complete before refine reads depth_normal_buffer,
        // which is guaranteed by wgpu's sequential command encoding.
        if render_state.refine_sample_count > 0 {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Ray Trace Refine Pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.refine_pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(width.div_ceil(8), height.div_ceil(8), 1);
        }

        // Copy texture to readback buffer
        encoder.copy_texture_to_buffer(
            wgpu::ImageCopyTexture {
                texture: &output_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::ImageCopyBuffer {
                buffer: &readback_buffer,
                layout: wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );

        #[cfg(target_arch = "wasm32")]
        web_sys::console::log_1(
            &format!(
                "[RT] Submitting GPU work: {}x{} = {} pixels",
                width,
                height,
                width * height
            )
            .into(),
        );

        ctx.queue.submit(Some(encoder.finish()));

        // Map and read buffer
        let buffer_slice = readback_buffer.slice(..);

        #[cfg(target_arch = "wasm32")]
        web_sys::console::log_1(&"[RT] Calling map_async...".into());

        // On WASM, use a Promise that resolves when the callback fires
        // This properly yields to the browser event loop
        #[cfg(target_arch = "wasm32")]
        let map_result = {
            use wasm_bindgen::prelude::*;
            use wasm_bindgen_futures::JsFuture;

            // Create a Promise that resolves when map_async callback fires
            let (promise, resolve, reject) = {
                let resolve_ref =
                    std::rc::Rc::new(std::cell::RefCell::new(None::<js_sys::Function>));
                let reject_ref =
                    std::rc::Rc::new(std::cell::RefCell::new(None::<js_sys::Function>));
                let resolve_clone = resolve_ref.clone();
                let reject_clone = reject_ref.clone();

                let promise = js_sys::Promise::new(&mut |resolve, reject| {
                    *resolve_clone.borrow_mut() = Some(resolve);
                    *reject_clone.borrow_mut() = Some(reject);
                });

                let resolve = resolve_ref.borrow().clone().unwrap();
                let reject = reject_ref.borrow().clone().unwrap();
                (promise, resolve, reject)
            };

            buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
                web_sys::console::log_1(
                    &format!("[RT] map_async callback: {:?}", result.is_ok()).into(),
                );
                match result {
                    Ok(()) => {
                        let _ = resolve.call0(&JsValue::undefined());
                    }
                    Err(_) => {
                        let _ = reject.call1(
                            &JsValue::undefined(),
                            &JsValue::from_str("Buffer mapping failed"),
                        );
                    }
                }
            });

            // Single poll to submit the mapping request
            ctx.device.poll(wgpu::Maintain::Poll);

            web_sys::console::log_1(&"[RT] Awaiting buffer mapping...".into());

            // Await the promise - this yields to browser event loop properly
            match JsFuture::from(promise).await {
                Ok(_) => {
                    web_sys::console::log_1(&"[RT] Buffer mapping complete".into());
                    Ok(())
                }
                Err(_) => Err(GpuError::BufferMapping),
            }
        };

        #[cfg(not(target_arch = "wasm32"))]
        let map_result = {
            use std::sync::atomic::{AtomicBool, Ordering};
            use std::sync::Arc;

            let success = Arc::new(AtomicBool::new(false));
            let success_clone = success.clone();

            buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
                if result.is_ok() {
                    success_clone.store(true, Ordering::SeqCst);
                }
            });

            ctx.device.poll(wgpu::Maintain::Wait);

            if success.load(Ordering::SeqCst) {
                Ok(())
            } else {
                Err(GpuError::BufferMapping)
            }
        };

        if map_result.is_err() {
            return Err(GpuError::BufferMapping);
        }

        #[cfg(target_arch = "wasm32")]
        web_sys::console::log_1(&"[RT] Reading mapped data...".into());

        let data = buffer_slice.get_mapped_range();

        // Remove padding from rows
        let mut result = Vec::with_capacity(output_size as usize);
        for row in 0..height {
            let row_start = (row * padded_bytes_per_row) as usize;
            let row_end = row_start + (width * 4) as usize;
            result.extend_from_slice(&data[row_start..row_end]);
        }

        drop(data);
        readback_buffer.unmap();

        Ok((result, accum))
    }

    /// Render a scene to an output texture (single-frame, non-progressive).
    ///
    /// This is a convenience wrapper around render_progressive for backward compatibility.
    pub async fn render(
        &self,
        ctx: &GpuContext,
        scene: &GpuScene,
        camera: &GpuCamera,
        width: u32,
        height: u32,
    ) -> Result<Vec<u8>, GpuError> {
        let (pixels, _accum) = self
            .render_progressive(ctx, scene, camera, width, height, 1, None)
            .await?;
        Ok(pixels)
    }
}

/// Settings for an offline (non-interactive) GPU render.
///
/// Deliberately narrow compared with [`GpuRenderState`]: the viewport-only
/// knobs (edge overlay, theme, debug modes, adaptive refinement) are all
/// forced off, because an offline render wants the integrator's estimate and
/// nothing painted on top of it.
#[cfg(all(feature = "gpu", not(target_arch = "wasm32")))]
#[derive(Debug, Clone, Copy)]
pub struct OfflineOptions {
    /// Output width in pixels.
    pub width: u32,
    /// Output height in pixels.
    pub height: u32,
    /// Samples per pixel. Each is one dispatch of the main kernel.
    pub spp: u32,
    /// Maximum path length. Held constant across all samples — the
    /// viewport's `depth_for_frame` escalation exists to make the *first*
    /// frame land fast, which an offline render does not care about, and it
    /// would bias the running mean toward the shallow early samples.
    pub max_depth: u32,
    /// Depth at which Russian roulette begins.
    pub rr_start: u32,
    /// Clamp on indirect radiance to kill fireflies (0 disables).
    pub firefly_clamp: f32,
    /// Overall multiplier on the studio environment.
    pub env_intensity: f32,
    /// Whether the implicit ground plane participates in the path trace.
    pub ground_enabled: bool,
    /// The exact counterpart of `PathTraceOptions::show_background`.
    ///
    /// `true` puts the *lighting environment* behind the subject; `false`
    /// leaves it black, which paired with the film's coverage alpha gives a
    /// transparent RGBA render. Either way the viewport's themed `sky_color`
    /// backdrop — a UI choice unrelated to the sky the integrator samples —
    /// is out of the picture.
    ///
    /// This has to be a shader-side switch rather than a composite after the
    /// fact: a pixel on the silhouette has already averaged its background
    /// and surface samples together.
    ///
    /// Defaults to `true` — an offline render is not a viewport.
    pub show_background: bool,
    /// RNG decorrelation seed. The same seed and the same scene give the
    /// same image, every run, on the same adapter.
    pub seed: u32,
}

#[cfg(all(feature = "gpu", not(target_arch = "wasm32")))]
impl Default for OfflineOptions {
    fn default() -> Self {
        Self {
            width: 512,
            height: 512,
            spp: 64,
            max_depth: super::buffers::DEFAULT_MAX_DEPTH,
            rr_start: super::buffers::DEFAULT_RR_START,
            firefly_clamp: super::buffers::DEFAULT_FIREFLY_CLAMP,
            env_intensity: super::buffers::DEFAULT_ENV_INTENSITY,
            ground_enabled: true,
            show_background: true,
            seed: 0,
        }
    }
}

/// The HDR result of an offline render.
///
/// This is the accumulation buffer verbatim: **linear radiance**, not
/// tonemapped and not gamma-encoded. Exposure, ACES and sRGB encoding belong
/// to the caller — feed [`Self::to_film`] into
/// [`crate::pathtrace::Film::to_srgb8`] to get exactly the CPU renderer's
/// output transform rather than a second, subtly different one.
#[cfg(all(feature = "gpu", not(target_arch = "wasm32")))]
#[derive(Debug, Clone)]
pub struct OfflineResult {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Samples per pixel that were actually accumulated.
    pub spp: u32,
    /// Linear RGB radiance plus coverage, 4 floats per pixel, row-major
    /// top-to-bottom. The alpha channel is the shader's sample-count marker
    /// (1.0 from the main pass), not opacity.
    pub rgba: Vec<f32>,
}

#[cfg(all(feature = "gpu", not(target_arch = "wasm32")))]
impl OfflineResult {
    /// Linear RGB at one pixel.
    pub fn pixel(&self, x: u32, y: u32) -> [f32; 3] {
        let i = ((y * self.width + x) * 4) as usize;
        [self.rgba[i], self.rgba[i + 1], self.rgba[i + 2]]
    }

    /// Mean relative luminance over the whole image.
    pub fn mean_luminance(&self) -> f32 {
        if self.rgba.is_empty() {
            return 0.0;
        }
        let sum: f64 = self
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| (0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]) as f64)
            .sum();
        (sum / (self.rgba.len() / 4) as f64) as f32
    }

    /// Repackage as a [`crate::pathtrace::Film`] so the CPU renderer's
    /// exposure/ACES/sRGB path can be reused unchanged.
    ///
    /// The normal, depth, albedo and variance guide buffers are left zeroed —
    /// the GPU does not read them back — so the result must not be fed to
    /// [`crate::pathtrace::denoise`], which needs them.
    pub fn to_film(&self) -> crate::pathtrace::Film {
        let n = (self.width * self.height) as usize;
        let mut rgb = Vec::with_capacity(n * 3);
        let mut alpha = Vec::with_capacity(n);
        for p in self.rgba.as_chunks::<4>().0 {
            rgb.extend_from_slice(&p[..3]);
            alpha.push(p[3]);
        }
        crate::pathtrace::Film {
            width: self.width,
            height: self.height,
            rgb,
            alpha,
            normal: vec![0.0; n * 3],
            depth: vec![0.0; n],
            albedo: vec![0.0; n * 3],
            variance: vec![0.0; n],
        }
    }
}

#[cfg(all(feature = "gpu", not(target_arch = "wasm32")))]
impl RayTracePipeline {
    /// Render `spp` samples per pixel offline and read back the HDR
    /// accumulation buffer once.
    ///
    /// This exists because [`Self::render_with_render_state`] is shaped for a
    /// 1-spp progressive viewport: every call recreates every scene buffer,
    /// rebuilds the bind group, and reads back the tonemapped `Rgba8Unorm`
    /// texture. At 1 frame per user gesture that is free; at 512 spp it is
    /// 512 scene uploads and 512 GPU→CPU round trips, and the round trips
    /// alone dominate the render.
    ///
    /// Here the scene is uploaded once, the bind group is built once, and the
    /// per-sample loop rewrites only the 128-byte `RenderState` uniform
    /// before dispatching the main kernel. The viewport's refine and denoise
    /// passes are skipped: both are interactivity aids that trade bias for
    /// speed at low sample counts, which is the wrong trade when the whole
    /// point is to converge.
    ///
    /// The readback is the f32 accumulation buffer (binding 8), not the
    /// tonemapped texture, so the caller gets linear HDR radiance and can
    /// apply exposure and ACES itself.
    ///
    /// Native only — it blocks on `device.poll(Maintain::Wait)`, which
    /// deadlocks the browser's single-threaded event loop.
    pub fn render_offline(
        &self,
        ctx: &GpuContext,
        scene: &GpuScene,
        camera: &GpuCamera,
        opts: &OfflineOptions,
    ) -> Result<OfflineResult, GpuError> {
        use wgpu::util::DeviceExt;

        let (width, height) = (opts.width.max(1), opts.height.max(1));
        let spp = opts.spp.max(1);

        // ── one-time uploads ──────────────────────────────────────────────
        let camera_buffer = ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Offline Camera Buffer"),
                contents: bytemuck::bytes_of(camera),
                usage: wgpu::BufferUsages::UNIFORM,
            });

        // WGSL cannot bind a zero-length storage array, so every empty scene
        // buffer still gets one dummy element; the counts in the scene
        // structs are what the shader actually loops over.
        let storage = |label: &str, bytes: &[u8]| {
            ctx.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(label),
                    contents: bytes,
                    usage: wgpu::BufferUsages::STORAGE,
                })
        };

        let surfaces = if scene.surfaces.is_empty() {
            vec![super::buffers::GpuSurface::zeroed()]
        } else {
            scene.surfaces.clone()
        };
        let surfaces_buffer = storage("Offline Surfaces", bytemuck::cast_slice(&surfaces));

        let faces = if scene.faces.is_empty() {
            vec![super::buffers::GpuFace::zeroed()]
        } else {
            scene.faces.clone()
        };
        let faces_buffer = storage("Offline Faces", bytemuck::cast_slice(&faces));

        let bvh_nodes = if scene.bvh_nodes.is_empty() {
            vec![super::buffers::GpuBvhNode::zeroed()]
        } else {
            scene.bvh_nodes.clone()
        };
        let bvh_buffer = storage("Offline BVH", bytemuck::cast_slice(&bvh_nodes));

        let trim_verts = if scene.trim_verts.is_empty() {
            vec![super::buffers::GpuVec2 { x: 0.0, y: 0.0 }]
        } else {
            scene.trim_verts.clone()
        };
        let trim_buffer = storage("Offline Trim", bytemuck::cast_slice(&trim_verts));

        let inner_loop_descs = if scene.inner_loop_descs.is_empty() {
            vec![0u32]
        } else {
            scene.inner_loop_descs.clone()
        };
        let inner_loop_descs_buffer = storage(
            "Offline Inner Loops",
            bytemuck::cast_slice(&inner_loop_descs),
        );

        let materials = if scene.materials.is_empty() {
            vec![super::buffers::GpuMaterial::default()]
        } else {
            scene.materials.clone()
        };
        let materials_buffer = storage("Offline Materials", bytemuck::cast_slice(&materials));

        let lights: Vec<super::buffers::GpuAreaLight> = if scene.lights.is_empty() {
            vec![super::buffers::GpuAreaLight::default()]
        } else {
            scene.lights.clone()
        };
        let light_buf = storage("Offline Area Lights", bytemuck::cast_slice(&lights));

        // The output texture is never read back — the HDR accumulation
        // buffer is the deliverable — but binding 6 is not optional, and
        // every dispatch writes the tonemapped pixel there.
        let output_texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Offline Output Texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING,
            view_formats: &[],
        });
        let output_view = output_texture.create_view(&Default::default());

        let px_buf_size = (width as u64) * (height as u64) * 16;
        let accum = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Offline Accumulation Buffer"),
            size: px_buf_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let depth_normal_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Offline Depth Normal Buffer"),
            size: px_buf_size,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let feature_id_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Offline Feature ID Buffer"),
            size: (width as u64) * (height as u64) * 4,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });

        let (env_pixels_view, env_cdf_view) = self.offline_env_views(ctx, scene);

        // The one buffer that changes between samples. COPY_DST so the loop
        // can `write_buffer` a new frame index instead of allocating.
        let mut render_state = self.offline_render_state(scene, opts, 1);
        let render_state_buffer =
            ctx.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Offline Render State Buffer"),
                    contents: bytemuck::bytes_of(&render_state),
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                });

        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Offline Ray Trace Bind Group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: camera_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: surfaces_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: faces_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: bvh_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: trim_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: inner_loop_descs_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::TextureView(&output_view),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: render_state_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: accum.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 9,
                    resource: materials_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 10,
                    resource: depth_normal_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 11,
                    resource: light_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 12,
                    resource: feature_id_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 13,
                    resource: wgpu::BindingResource::TextureView(&env_pixels_view),
                },
                wgpu::BindGroupEntry {
                    binding: 14,
                    resource: wgpu::BindingResource::TextureView(&env_cdf_view),
                },
            ],
        });

        // ── the sample loop ───────────────────────────────────────────────
        // One submit per sample. `write_buffer` is staged and applied at the
        // next submit, so the frame index cannot be updated for several
        // dispatches inside a single encoder — they would all read the same
        // uniform and collapse the running mean.
        let (groups_x, groups_y) = (width.div_ceil(8), height.div_ceil(8));
        for frame_index in 1..=spp {
            render_state.frame_index = frame_index;
            let (jx, jy) = super::buffers::halton_jitter(frame_index);
            render_state.jitter_x = jx;
            render_state.jitter_y = jy;
            ctx.queue
                .write_buffer(&render_state_buffer, 0, bytemuck::bytes_of(&render_state));

            let mut encoder = ctx
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("Offline Ray Trace Encoder"),
                });
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("Offline Ray Trace Pass"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &bind_group, &[]);
                pass.dispatch_workgroups(groups_x, groups_y, 1);
            }
            ctx.queue.submit(Some(encoder.finish()));
        }

        // ── one readback of the HDR accumulation buffer ───────────────────
        let readback = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Offline HDR Readback Buffer"),
            size: px_buf_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Offline Readback Encoder"),
            });
        encoder.copy_buffer_to_buffer(&accum, 0, &readback, 0, px_buf_size);
        ctx.queue.submit(Some(encoder.finish()));

        let slice = readback.slice(..);
        let mapped = {
            use std::sync::atomic::{AtomicBool, Ordering};
            use std::sync::Arc;

            let ok = Arc::new(AtomicBool::new(false));
            let ok_clone = ok.clone();
            slice.map_async(wgpu::MapMode::Read, move |r| {
                if r.is_ok() {
                    ok_clone.store(true, Ordering::SeqCst);
                }
            });
            ctx.device.poll(wgpu::Maintain::Wait);
            ok.load(Ordering::SeqCst)
        };
        if !mapped {
            return Err(GpuError::BufferMapping);
        }

        let data = slice.get_mapped_range();
        let rgba: Vec<f32> = bytemuck::cast_slice::<u8, f32>(&data).to_vec();
        drop(data);
        readback.unmap();

        Ok(OfflineResult {
            width,
            height,
            spp,
            rgba,
        })
    }

    /// The render state shared by every sample in an offline render.
    ///
    /// Everything the viewport uses to stay responsive is off: no edge
    /// overlay, no stylisation, no debug mode, no adaptive refinement, and a
    /// fixed `max_depth` instead of the per-frame escalation.
    fn offline_render_state(
        &self,
        scene: &GpuScene,
        opts: &OfflineOptions,
        frame_index: u32,
    ) -> GpuRenderState {
        let mut s = GpuRenderState::new(frame_index);
        s.enable_edges = 0;
        s.stylize = 0;
        s.debug_mode = 0;
        s.refine_sample_count = 0;
        s.max_depth = opts.max_depth.max(1);
        s.rr_start = opts.rr_start;
        s.firefly_clamp = opts.firefly_clamp;
        s.env_intensity = opts.env_intensity;
        s.ground_enabled = u32::from(opts.ground_enabled);
        s.seed = opts.seed;
        s.background_mode = if opts.show_background {
            super::buffers::BACKGROUND_ENVIRONMENT
        } else {
            super::buffers::BACKGROUND_BLACK
        };
        s.light_count = scene.lights.len() as u32;
        match &scene.environment {
            Some(e) => {
                s.env_mode = 1;
                s.env_width = e.width;
                s.env_height = e.height;
                s.env_intensity = e.intensity;
                s.env_rotation = e.rotation;
                s.env_marg_int = e.marg_int;
            }
            None => s.env_mode = 0,
        }
        s
    }

    /// Environment texture views for the offline bind group.
    ///
    /// A gradient-lit scene still needs 1x1 dummies: a texture binding cannot
    /// be null, and `env_mode` is what the shader branches on.
    fn offline_env_views(
        &self,
        ctx: &GpuContext,
        scene: &GpuScene,
    ) -> (wgpu::TextureView, wgpu::TextureView) {
        let mk = |label: &str, w: u32, h: u32, fmt: wgpu::TextureFormat, data: &[f32]| {
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
            let bpp = if fmt == wgpu::TextureFormat::Rgba32Float {
                16
            } else {
                4
            };
            ctx.queue.write_texture(
                wgpu::ImageCopyTexture {
                    texture: &tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                bytemuck::cast_slice(data),
                wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(w * bpp),
                    rows_per_image: Some(h),
                },
                wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
            );
            tex.create_view(&wgpu::TextureViewDescriptor::default())
        };

        match &scene.environment {
            Some(e) if e.width > 0 && e.height > 0 => (
                mk(
                    "Offline Environment Pixels",
                    e.width,
                    e.height,
                    wgpu::TextureFormat::Rgba32Float,
                    &e.pixels,
                ),
                mk(
                    "Offline Environment CDF",
                    e.width + 1,
                    e.height + 1,
                    wgpu::TextureFormat::R32Float,
                    &e.cdf,
                ),
            ),
            _ => (
                mk(
                    "Offline Environment Pixels (unused)",
                    1,
                    1,
                    wgpu::TextureFormat::Rgba32Float,
                    &[0.0, 0.0, 0.0, 1.0],
                ),
                mk(
                    "Offline Environment CDF (unused)",
                    1,
                    1,
                    wgpu::TextureFormat::R32Float,
                    &[0.0],
                ),
            ),
        }
    }
}

/// Stub for when GPU feature is not enabled.
#[cfg(not(feature = "gpu"))]
pub struct RayTracePipeline;

#[cfg(not(feature = "gpu"))]
impl RayTracePipeline {
    /// Returns an error when GPU feature is not enabled.
    pub fn new() -> Result<Self, String> {
        Err("GPU feature not enabled. Compile with --features gpu".to_string())
    }
}
