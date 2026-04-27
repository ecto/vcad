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
                source: wgpu::ShaderSource::Wgsl(super::shaders::RAYTRACE_SHADER.into()),
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

        Ok(Self {
            pipeline,
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
        // Delegate to full settings with default edge parameters
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
    ) -> Result<(Vec<u8>, wgpu::Buffer), GpuError> {
        use wgpu::util::DeviceExt;

        // Create camera buffer
        let camera_buffer = ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Camera Buffer"),
                contents: bytemuck::bytes_of(camera),
                usage: wgpu::BufferUsages::UNIFORM,
            });

        // Create render state buffer
        let render_state = GpuRenderState::with_full_settings(
            frame_index,
            debug_mode,
            enable_edges,
            edge_depth_threshold,
            edge_normal_threshold,
            theme,
        );
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

        // Create depth/normal buffer for edge detection (4 floats per pixel: depth, nx, ny, nz)
        let depth_normal_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Depth Normal Buffer"),
            size: accum_buf_size,
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
