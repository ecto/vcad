//! `--photoreal --gpu`: the photoreal path traced on a wgpu compute pipeline.
//!
//! This is the *same scene* as [`crate::photoreal`], not a second renderer
//! with its own opinions. Geometry comes from `photoreal::build_objects` (one
//! cached triangle BLAS per solid), framing from `photoreal::frame_view`, and
//! lights/environment/floor from `photoreal::dress_scene`; the film goes back
//! through `pathtrace::Film::to_srgb8`, so exposure, ACES and sRGB encoding
//! are byte-for-byte the CPU path's. What changes is only who integrates the
//! rendering equation.
//!
//! # What `--gpu` honours, and what it refuses
//!
//! The GPU tracer is a narrower renderer than the CPU one, and the honest
//! thing is to say so in the error message rather than quietly render
//! something else. Refused outright, with a message naming the flag:
//!
//! * `--exact` — the GPU's BRep path ([`GpuScene::from_brep`]) caps at a
//!   thousand analytic surfaces, which is a bracket, not an assembly. `--gpu`
//!   traces the cached tessellation, exactly as CPU `--photoreal` does by
//!   default.
//! * `--aperture` — the WGSL camera is a pinhole; there is no lens to sample.
//! * `--ortho` — the WGSL camera is projective only.
//! * `--backdrop shadow-catcher` — the shadow catcher is a CPU integrator
//!   feature (a surface that contributes occlusion to alpha and nothing to
//!   colour); there is no shader counterpart.
//! * `--animate` — transforms are *baked* into the uploaded vertices (see
//!   [`GpuScene::from_mesh_bvh_placed`]), so a new pose means a new upload.
//!   A sequence is exactly the case where that is the wrong trade.
//!
//! Honoured, and matching the CPU path: `--spp`, `--max-depth`, `--exposure`,
//! `--fov`, `--seed`, `--size`, `--fill`, `--auto-aspect`, `--view` /
//! `--azimuth` / `--elevation` (including the mirrored isometric basis),
//! `--env` and `--env-rotation` (gradient and HDRI both), per-part materials,
//! and `--backdrop studio` / `--backdrop none`.
//!
//! Two knowing divergences, both documented where they happen:
//!
//! * **The studio floor is a large quad, not an infinite plane.** The CPU
//!   `Ground` is analytic and unbounded; the shader has no such primitive at
//!   an arbitrary height, so the floor is uploaded as two triangles
//!   [`GROUND_EXTENT`] scene-radii across. Within the frame of any sane
//!   product shot the two agree; a camera aimed at the horizon would see
//!   where the quad stops.
//! * **No denoiser.** See [`denoise_is_unavailable`].
//!
//! `--no-adaptive` is accepted and ignored: adaptive sampling is a per-pixel
//! early-out the shader has no equivalent for, so `--spp` on the GPU is an
//! exact count rather than a ceiling.
//!
//! # How close the two get
//!
//! Close, and not identical, and the gap does not close with samples. On
//! rose-pro at 800px the GPU render sits ~29 dB PSNR from a 1024-spp CPU
//! reference at 64 spp and ~30 dB at 1024 spp — it converges, but to a
//! slightly different picture, because the integrator is f32 where the CPU's
//! is f64 and because of the floor above. A CPU render at the same 64 spp
//! reaches ~39 dB. So `--gpu` is the right tool for a fast look or a large
//! sweep, and the CPU path is still the one to render a final hero on.
//!
//! Setting `VCAD_GPU_DEBUG` prints the packed scene's part count, triangle
//! count, node count, BVH depth, light count and environment mode to stderr —
//! the numbers every "why was this refused" question turns out to be about.

use vcad_kernel::vcad_kernel_math::{Point3, Transform, Vec3};
use vcad_kernel::vcad_kernel_tessellate::TriangleMesh;
use vcad_kernel_gpu::{GpuContext, GpuError};
use vcad_kernel_raytrace::gpu::{
    GpuAreaLight, GpuCamera, GpuMaterial, GpuScene, OfflineOptions,
};
use vcad_kernel_raytrace::pathtrace::{Environment, Ground, Object, Scene};
use vcad_kernel_raytrace::{BrepBvh, Bvh};

use super::photoreal::{self, Backdrop, Framing, PhotorealOptions};
use super::raster::{encode_jpeg, encode_png, Frame};
use super::{evaluate_vcad, RasterOptions};

/// Half-width of the studio floor quad, in scene radii.
///
/// The CPU renderer's floor is an unbounded analytic plane. The shader has no
/// plane-at-arbitrary-height primitive (its built-in ground is pinned to
/// z = 0 and faded by distance, which is a viewport look, not this one), so
/// the floor ships as two triangles instead.
///
/// The value is a measured compromise, not a guess. Too small and the quad's
/// edge walks into frame. Too large and f32 loses the plane: at 600 radii a
/// rose-pro render came back with visible wavy banding across the floor —
/// self-shadowing acne from ray/triangle arithmetic on coordinates six
/// hundred times the size of the millimetre geometry beside them. 50 radii
/// puts the edge comfortably outside the frame of any camera actually aimed
/// at the subject, and the banding is gone.
pub const GROUND_EXTENT: f64 = 50.0;

/// Why the CPU denoiser is not run on a GPU film.
///
/// `pathtrace::denoise` is guided by the per-pixel normal, depth and albedo
/// the CPU integrator records from each primary ray. `render_offline` reads
/// back the radiance accumulation buffer and nothing else, so
/// `OfflineResult::to_film` leaves those guides zeroed — and the denoiser
/// treats `depth == 0` as "this pixel escaped to the background, pass it
/// through untouched". Run against a zeroed film it is therefore not
/// *wrong*, it is a silent no-op: the user asks for denoising, waits for it,
/// and gets the raw film back with no indication.
///
/// So `--gpu` disables it and says so on stderr, once. Writing the guide
/// buffers from the shader is the obvious follow-up; it is a real slice of
/// work (three more storage buffers and a first-sample-only write path), not
/// something to fake here.
pub const fn denoise_is_unavailable() -> &'static str {
    "--photoreal --gpu: the denoiser needs the normal/depth/albedo guide \
     buffers, which the GPU tracer does not read back; rendering without it \
     (raise --spp to compensate, or drop --gpu)"
}

/// Render raw `.vcad` document JSON to a photorealistic JPEG on the GPU.
pub fn render_photoreal_gpu_jpeg_str(
    raw_vcad: &str,
    opts: &RasterOptions,
    pr: &PhotorealOptions,
) -> Result<Vec<u8>, String> {
    encode_jpeg(rasterize(raw_vcad, opts, pr, false)?, opts)
}

/// Render raw `.vcad` document JSON to a photorealistic RGBA PNG on the GPU.
pub fn render_photoreal_gpu_png_str(
    raw_vcad: &str,
    opts: &RasterOptions,
    pr: &PhotorealOptions,
) -> Result<Vec<u8>, String> {
    encode_png(rasterize(raw_vcad, opts, pr, true)?, opts)
}

/// Reject the option combinations the GPU tracer cannot render *faithfully*.
///
/// Every one of these would otherwise produce a plausible-looking image that
/// is not the render the user asked for, which is the worst failure mode a
/// renderer has. Called before any GPU work, so the diagnostic arrives
/// immediately rather than after a scene build.
pub fn check_supported(opts: &RasterOptions, pr: &PhotorealOptions) -> Result<(), String> {
    let _ = opts;
    if pr.exact {
        return Err("--gpu does not support --exact: the GPU tracer's analytic \
                    BRep path is capped at ~1k surfaces, far below a real \
                    assembly. Drop --exact to trace the cached tessellation \
                    (what --photoreal does by default), or drop --gpu."
            .to_string());
    }
    if pr.aperture_frac > 0.0 {
        return Err(format!(
            "--gpu does not support --aperture (got {}): the GPU camera is a \
             pinhole, so depth of field would silently render sharp. Drop \
             --aperture, or drop --gpu.",
            pr.aperture_frac
        ));
    }
    if pr.orthographic {
        return Err("--gpu does not support --ortho: the GPU camera is \
                    projective only, and would render a perspective image \
                    under an orthographic flag. Drop --ortho, or drop --gpu."
            .to_string());
    }
    if pr.backdrop == Backdrop::ShadowCatcher {
        return Err("--gpu does not support --backdrop shadow-catcher: the \
                    shadow catcher is a CPU integrator feature (a surface \
                    that contributes to alpha but not to colour) with no \
                    shader counterpart. Use --backdrop studio or none, or \
                    drop --gpu."
            .to_string());
    }
    Ok(())
}

/// Acquire the GPU, or explain why not.
///
/// `--gpu` is an explicit request, so an unavailable adapter is a hard error
/// rather than a quiet fallback: a user who asked for the GPU and silently
/// got a CPU render would draw the wrong conclusion from every timing they
/// took afterwards. The message names the fallback rather than taking it.
fn context() -> Result<&'static GpuContext, String> {
    match pollster::block_on(GpuContext::init()) {
        Ok(ctx) => Ok(ctx),
        Err(GpuError::NoAdapter) => Err("--gpu: no compatible GPU adapter found. \
                                         Drop --gpu to path-trace on the CPU."
            .to_string()),
        Err(e) => Err(format!(
            "--gpu: could not initialise the GPU ({e}). \
                               Drop --gpu to path-trace on the CPU."
        )),
    }
}

/// Two triangles at `z`, centred on `center`, spanning `half` in x and y.
///
/// Wound counter-clockwise seen from +z and given explicit up-normals, so it
/// shades as a floor rather than taking the geometric-normal fallback.
fn ground_mesh(center: Point3, z: f64, half: f64) -> TriangleMesh {
    let (x0, x1) = ((center.x - half) as f32, (center.x + half) as f32);
    let (y0, y1) = ((center.y - half) as f32, (center.y + half) as f32);
    let z = z as f32;
    TriangleMesh {
        vertices: vec![x0, y0, z, x1, y0, z, x1, y1, z, x0, y1, z],
        indices: vec![0, 1, 2, 0, 2, 3],
        normals: vec![0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0],
        face_kinds: Vec::new(),
        face_ids: Vec::new(),
    }
}

/// Pack one CPU [`Object`] as its own single-material GPU scene.
fn object_scene(obj: &Object) -> Result<GpuScene, String> {
    // An identity transform is the overwhelmingly common case (the static
    // photoreal path never places anything); skipping the bake there keeps
    // the packed vertices bit-identical to the un-transformed ones rather
    // than round-tripping every coordinate through an f64 matrix multiply.
    // `Object::transform` is the renderer's `Transform`; the packer takes
    // vcad's. Same matrix under two names, so this re-spells rather than
    // converts.
    let placement = Transform {
        matrix: obj.transform.matrix,
    };
    let transform = (!is_identity(&placement)).then_some(&placement);
    GpuScene::from_mesh_bvh_placed(&obj.bvh, GpuMaterial::from_pbr(obj.material), transform)
        .map_err(|e| format!("--gpu: cannot upload part geometry: {e}"))
}

/// Whether `t` moves nothing.
///
/// Probed rather than read out of the matrix: an affine map is pinned by
/// where it sends the origin and the three basis points, so this is exact
/// without depending on the matrix's storage order.
fn is_identity(t: &Transform) -> bool {
    const EPS: f64 = 1e-12;
    [
        Point3::new(0.0, 0.0, 0.0),
        Point3::new(1.0, 0.0, 0.0),
        Point3::new(0.0, 1.0, 0.0),
        Point3::new(0.0, 0.0, 1.0),
    ]
    .iter()
    .all(|p| {
        let q = t.apply_point(p);
        (q.x - p.x).abs() < EPS && (q.y - p.y).abs() < EPS && (q.z - p.z).abs() < EPS
    })
}

/// Turn the CPU-side [`Scene`] into a single validated [`GpuScene`].
///
/// The order here matters. `GpuScene::merge` re-derives the studio rig from
/// the combined bounds, which is right for the subject and very wrong once the
/// floor quad (hundreds of radii across) joins the scene — so the rig is
/// written once, after the last merge. The CPU path sizes its rig the same way:
/// `dress_scene` works from the subject's bounds, not the floor's.
fn build_scene(scene: &Scene, framing: &Framing) -> Result<GpuScene, String> {
    let parts: Vec<GpuScene> = scene
        .objects
        .iter()
        .map(object_scene)
        .collect::<Result<_, _>>()?;
    let mut merged =
        GpuScene::merge_all(parts).ok_or_else(|| "--gpu: scene has no geometry".to_string())?;

    if let Some(ground) = &scene.ground {
        merged = merged.merge(ground_scene(ground, framing)?);
    }

    // After the last merge, never before — see above. The lights themselves
    // come from `dress_scene`, which is where the CPU renderer gets them,
    // including the empty rig an HDRI environment implies: an image
    // environment already carries its own lighting.
    merged.lights = scene
        .lights
        .iter()
        .map(GpuAreaLight::from_area_light)
        .collect();
    merged.set_environment(match &scene.env {
        Environment::Image(map) => Some(map),
        // The shader's analytic gradient is a transcription of
        // `GradientEnv::default()`; passing None selects it.
        Environment::Gradient(_) => None,
    });
    Ok(merged)
}

fn ground_scene(ground: &Ground, framing: &Framing) -> Result<GpuScene, String> {
    let mesh = ground_mesh(framing.center, ground.z, framing.radius * GROUND_EXTENT);
    GpuScene::from_mesh_bvh_placed(
        &Bvh::build_mesh(&mesh),
        GpuMaterial::from_pbr(ground.material),
        None,
    )
    .map_err(|e| format!("--gpu: cannot upload the studio floor: {e}"))
}

/// The GPU camera for a CPU [`Framing`].
///
/// Hands the screen basis over verbatim. `View::Isometric` and the named CAD
/// views carry a *mirrored* basis (see `View::right`), and the shader's
/// default reconstruction — `right = forward × up` — cannot represent one, so
/// rebuilding it there flips the render left-for-right against every other
/// output style. `CAMERA_BASIS_EXPLICIT` is exactly this fix.
fn camera_for(framing: &Framing, w: u32, h: u32) -> GpuCamera {
    let c = &framing.camera;
    let f32v = |v: Vec3| [v.x as f32, v.y as f32, v.z as f32];
    GpuCamera::from_basis(
        [c.eye.x as f32, c.eye.y as f32, c.eye.z as f32],
        f32v(c.forward),
        f32v(c.right),
        f32v(c.up),
        (c.fov_deg as f32).to_radians(),
        c.focus_dist as f32,
        w,
        h,
    )
}

fn rasterize(
    raw_vcad: &str,
    opts: &RasterOptions,
    pr: &PhotorealOptions,
    png: bool,
) -> Result<Frame, String> {
    photoreal::check_raster_opts(opts)?;
    check_supported(opts, pr)?;

    let ctx = context()?;
    let pipeline =
        vcad_kernel_raytrace::gpu::brep_pipeline(ctx)
            .map_err(|e| format!("--gpu: pipeline creation failed: {e}"))?;

    let solids = evaluate_vcad(raw_vcad)?;
    if solids.is_empty() {
        return Err("no solids produced".to_string());
    }
    // `mesh_segments()` is Some here: --exact is refused above.
    let objects = photoreal::build_objects(&solids, pr.mesh_segments())?;
    let corners: Vec<[f64; 3]> = objects.iter().flat_map(photoreal::object_corners).collect();
    let framing = photoreal::frame_view(&corners, opts, pr)?;
    let scene = photoreal::dress_scene(objects, &framing, pr)?;

    let gpu_scene = build_scene(&scene, &framing)?;
    if std::env::var_os("VCAD_GPU_DEBUG").is_some() {
        eprintln!(
            "--gpu scene: {} parts, {} triangles, {} bvh nodes, depth {}, {} lights, env {}",
            scene.objects.len(),
            gpu_scene.surfaces.len(),
            gpu_scene.bvh_nodes.len(),
            gpu_scene.bvh_depth(),
            gpu_scene.lights.len(),
            if gpu_scene.environment.is_some() {
                "image"
            } else {
                "gradient"
            },
        );
    }
    gpu_scene
        .validate(Some(
            ctx.device.limits().max_storage_buffer_binding_size as u64,
        ))
        .map_err(|e| format!("--gpu: {e}"))?;

    let canvas = framing.canvas;
    let (w, h) = (canvas.w as u32, canvas.h as u32);
    let cpu_opts = photoreal::trace_options(pr, png);
    let out = pipeline
        .render_offline(
            ctx,
            &gpu_scene,
            &camera_for(&framing, w, h),
            &OfflineOptions {
                width: w,
                height: h,
                spp: cpu_opts.spp,
                max_depth: cpu_opts.max_depth,
                rr_start: cpu_opts.rr_start,
                // Only read for the gradient: an image environment carries
                // its own intensity, which `offline_render_state` takes from
                // the packed map.
                env_intensity: match &scene.env {
                    Environment::Gradient(g) => g.intensity,
                    Environment::Image(_) => 1.0,
                },
                firefly_clamp: cpu_opts.firefly_clamp.unwrap_or(0.0),
                // The floor is real geometry here, so the shader's own
                // implicit z=0 ground must stay out of the way — it would
                // otherwise add a second, differently-placed floor.
                ground_enabled: false,
                show_background: cpu_opts.show_background,
                // The CPU seed is 64-bit and only ever mixed into a per-pixel
                // hash; the shader's is 32. Fold rather than truncate so two
                // seeds that differ only in the high word still differ here.
                seed: (pr.seed as u32) ^ ((pr.seed >> 32) as u32),
            },
        )
        .map_err(|e| format!("--gpu: render failed: {e}"))?;

    let film = out.to_film();
    let rgba = film.to_srgb8(pr.exposure, png && pr.backdrop != Backdrop::Studio);

    let n = canvas.len();
    let mut rgb = vec![0u8; n * 3];
    let mut mask = vec![0u8; n];
    for i in 0..n {
        rgb[i * 3] = rgba[i * 4];
        rgb[i * 3 + 1] = rgba[i * 4 + 1];
        rgb[i * 3 + 2] = rgba[i * 4 + 2];
        mask[i] = rgba[i * 4 + 3];
    }
    Ok(Frame { rgb, mask, canvas })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> RasterOptions {
        RasterOptions {
            size_px: 64,
            ..Default::default()
        }
    }

    #[test]
    fn exact_is_refused() {
        let pr = PhotorealOptions {
            exact: true,
            ..Default::default()
        };
        let err = check_supported(&opts(), &pr).expect_err("should refuse");
        assert!(err.contains("--exact"), "unhelpful: {err}");
    }

    #[test]
    fn aperture_is_refused() {
        let pr = PhotorealOptions {
            aperture_frac: 0.03,
            ..Default::default()
        };
        let err = check_supported(&opts(), &pr).expect_err("should refuse");
        assert!(err.contains("--aperture"), "unhelpful: {err}");
    }

    #[test]
    fn ortho_is_refused() {
        let pr = PhotorealOptions {
            orthographic: true,
            ..Default::default()
        };
        let err = check_supported(&opts(), &pr).expect_err("should refuse");
        assert!(err.contains("--ortho"), "unhelpful: {err}");
    }

    #[test]
    fn shadow_catcher_is_refused() {
        let pr = PhotorealOptions {
            backdrop: Backdrop::ShadowCatcher,
            ..Default::default()
        };
        let err = check_supported(&opts(), &pr).expect_err("should refuse");
        assert!(err.contains("shadow-catcher"), "unhelpful: {err}");
    }

    #[test]
    fn the_defaults_are_supported() {
        check_supported(&opts(), &PhotorealOptions::default()).expect("defaults must work on GPU");
    }

    /// The floor quad must actually be under the subject and wider than it,
    /// or the "studio sweep" is a stripe.
    #[test]
    fn ground_quad_spans_the_subject() {
        let mesh = ground_mesh(Point3::new(5.0, -2.0, 10.0), 1.5, 100.0);
        assert_eq!(mesh.indices.len(), 6);
        let xs: Vec<f32> = mesh
            .vertices
            .as_chunks::<3>()
            .0
            .iter()
            .map(|v| v[0])
            .collect();
        let zs: Vec<f32> = mesh
            .vertices
            .as_chunks::<3>()
            .0
            .iter()
            .map(|v| v[2])
            .collect();
        assert!(
            zs.iter().all(|&z| (z - 1.5).abs() < 1e-6),
            "floor is not flat"
        );
        assert!(
            xs.iter().cloned().fold(f32::MIN, f32::max) >= 105.0,
            "floor does not reach past the subject"
        );
        assert!(
            mesh.normals.as_chunks::<3>().0.iter().all(|n| n[2] > 0.9),
            "floor normals must point up"
        );
    }

    #[test]
    fn identity_transform_is_detected() {
        assert!(is_identity(&Transform::identity()));
        assert!(!is_identity(&Transform::translation(0.0, 0.0, 1e-3)));
    }
}
