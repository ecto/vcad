//! Triangle meshes in the wgpu path tracer.
//!
//! `--photoreal` traces cached triangle meshes by default, so until the GPU
//! could intersect a triangle the fast path was unavailable for exactly the
//! geometry the renderer actually feeds it. These tests cover the seam:
//! a mesh-only scene must render, a mixed BRep+mesh scene must render *both*
//! halves, and both must land on top of what the CPU path tracer produces
//! from the same BVH.
//!
//! Parity is measured against the CPU tracer driving the **same**
//! `Bvh::build_mesh` tree, not against the analytic solid the mesh was
//! tessellated from. That isolates what is under test — the WGSL triangle
//! intersector and normal interpolation — from tessellation error, which is a
//! property of the mesh both renderers share.
//!
//! These tests need a real adapter and are `#[ignore]`-tagged like
//! `gpu_smoke.rs` and `gpu_offline.rs`. Run locally with:
//!
//! ```text
//! cargo test -p vcad-kernel-raytrace --features gpu --test gpu_mesh -- --ignored --nocapture
//! ```

#![cfg(all(feature = "gpu", not(target_arch = "wasm32")))]

use std::sync::Arc;

use vcad_kernel_gpu::{GpuContext, GpuError};
use vcad_kernel_math::{Point3, Vec3};
use vcad_kernel_primitives::{make_cube, make_sphere};
use vcad_kernel_raytrace::gpu::{GpuCamera, GpuScene, OfflineOptions, OfflineResult};
use vcad_kernel_raytrace::pathtrace::{
    self, Camera, Environment, Object, PathTraceOptions, Pbr, Scene,
};
use vcad_kernel_raytrace::Bvh;
use vcad_kernel_tessellate::TriangleMesh;

use vcad_kernel_raytrace::gpu::RayTracePipeline;

/// Skip with a clear message when no adapter is available.
fn ctx_or_skip(test_name: &str) -> Option<&'static GpuContext> {
    match pollster::block_on(GpuContext::init()) {
        Ok(ctx) => Some(ctx),
        Err(GpuError::NoAdapter) => {
            eprintln!("[{test_name}] skipped: no compatible GPU adapter");
            None
        }
        Err(e) => panic!("GPU init failed unexpectedly: {e}"),
    }
}

/// Subject radius and the camera distance that makes it overfill the frame.
/// Same framing as `gpu_offline.rs`, and for the same reason: every pixel
/// lands on the subject, so a whole-image comparison never straddles the
/// backdrop — where the GPU's themed `sky_color` and the CPU's `env_radiance`
/// legitimately differ.
const R: f64 = 5.0;
const EYE_Z: f64 = 14.0;
const FOV_DEG: f64 = 24.0;

/// The GPU's default material, spelled as a CPU `Pbr`. `GpuMaterial::default`
/// is 0.7 grey at roughness 0.5, which is *not* `Pbr::default`.
fn gpu_default_material() -> Pbr {
    Pbr {
        base_color: [0.7, 0.7, 0.7],
        metallic: 0.0,
        roughness: 0.5,
        anisotropy: 0.0,
        clearcoat: 0.0,
        clearcoat_roughness: 0.1,
        ior: 1.5,
        emissive: [0.0; 3],
    }
}

/// A tessellated sphere: the mesh-heavy subject the parity tests trace.
fn mesh_sphere(segments: u32) -> TriangleMesh {
    vcad_kernel_tessellate::tessellate(&make_sphere(R, segments), segments)
}

/// The CPU counterpart of the scene the GPU builders produce: the given
/// objects, the studio rig sized to the union of their BVH bounds, the shared
/// gradient environment, no ground.
///
/// The rig derivation mirrors `studio_lights_for_bvh` in `gpu/buffers.rs`,
/// including how `GpuScene::merge` re-derives it from the merged root — so a
/// mixed scene is lit identically on both sides.
fn cpu_scene(bvhs: Vec<Arc<Bvh>>) -> Scene {
    let mut min = [f64::MAX; 3];
    let mut max = [f64::MIN; 3];
    for bvh in &bvhs {
        let aabb = match bvh.root().expect("BVH has a root") {
            vcad_kernel_raytrace::bvh::BvhNode::Leaf { aabb, .. }
            | vcad_kernel_raytrace::bvh::BvhNode::Internal { aabb, .. } => *aabb,
        };
        let extents = [
            (aabb.min.x, aabb.max.x),
            (aabb.min.y, aabb.max.y),
            (aabb.min.z, aabb.max.z),
        ];
        for (a, (lo, hi)) in extents.into_iter().enumerate() {
            min[a] = min[a].min(lo);
            max[a] = max[a].max(hi);
        }
    }

    let center = Point3::new(
        (min[0] + max[0]) * 0.5,
        (min[1] + max[1]) * 0.5,
        (min[2] + max[2]) * 0.5,
    );
    let radius = 0.5
        * ((max[0] - min[0]).powi(2) + (max[1] - min[1]).powi(2) + (max[2] - min[2]).powi(2))
            .sqrt();

    Scene {
        objects: bvhs
            .into_iter()
            .map(|b| Object::new(b, gpu_default_material()))
            .collect(),
        lights: pathtrace::studio_rig(center, radius),
        env: Environment::default(),
        ground: None,
    }
}

fn gpu_camera(w: u32, h: u32) -> GpuCamera {
    GpuCamera::new(
        [0.0, 0.0, EYE_Z as f32],
        [0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        (FOV_DEG as f32).to_radians(),
        w,
        h,
    )
}

fn cpu_camera() -> Camera {
    Camera::look_at(
        Point3::new(0.0, 0.0, EYE_Z),
        Point3::new(0.0, 0.0, 0.0),
        Vec3::new(0.0, 1.0, 0.0),
        FOV_DEG,
    )
}

fn offline_opts(w: u32, h: u32, spp: u32) -> OfflineOptions {
    OfflineOptions {
        width: w,
        height: h,
        spp,
        ground_enabled: false,
        seed: 7,
        ..Default::default()
    }
}

fn cpu_opts(spp: u32) -> PathTraceOptions {
    PathTraceOptions {
        spp,
        adaptive: false,
        denoise: false,
        show_background: true,
        seed: 7,
        ..Default::default()
    }
}

/// Peak signal-to-noise ratio between a GPU HDR readback and a CPU render,
/// in dB, over the whole frame.
///
/// Both images are clamped into [0, 1] first — the peak the ratio is taken
/// against. Radiance above 1 is a specular highlight whose *absolute* error
/// scales with its magnitude, so leaving it unclamped would let a single hot
/// pixel dominate a metric meant to describe the whole image. Clamping is
/// also what a display does, which makes the number mean "how different do
/// these look" rather than "how different are these float buffers".
fn psnr(gpu: &OfflineResult, cpu: &pathtrace::Film) -> f64 {
    psnr_masked(gpu, cpu, false)
}

/// As [`psnr`], but optionally restricted to pixels where the CPU render hit
/// geometry.
///
/// Masking is required whenever the subject does not fill the frame. The two
/// renderers deliberately draw *different backdrops* — the GPU's `sky_color`
/// is a themed UI choice, the CPU's `env_radiance` is the shared lighting
/// environment — so background pixels compare two things that were never
/// meant to match, and on a mostly-empty frame they swamp the metric. Where
/// the subject fills the frame (`mesh_render_matches_cpu_reference`) the mask
/// is a no-op and the unmasked form is used to keep the comparison honest.
fn psnr_masked(gpu: &OfflineResult, cpu: &pathtrace::Film, subject_only: bool) -> f64 {
    let mut mse = 0.0f64;
    let mut counted = 0usize;
    let n = gpu.rgba.len() / 4;
    assert_eq!(n, cpu.rgb.len() / 3, "image sizes differ");
    for i in 0..n {
        if subject_only && cpu.depth[i] <= 0.0 {
            continue;
        }
        counted += 1;
        for c in 0..3 {
            let a = (gpu.rgba[i * 4 + c] as f64).clamp(0.0, 1.0);
            let b = (cpu.rgb[i * 3 + c] as f64).clamp(0.0, 1.0);
            mse += (a - b) * (a - b);
        }
    }
    assert!(counted > 0, "nothing to compare -- the mask kept no pixels");
    mse /= (counted * 3) as f64;
    if mse <= 0.0 {
        return f64::INFINITY;
    }
    10.0 * (1.0 / mse).log10()
}

/// The core parity test: a mesh-only GPU render must land on top of the CPU
/// render of the same mesh BVH, at the same spp and seed.
///
/// The threshold is 25 dB, which is loose on purpose. The two integrators
/// share a BSDF, an environment and a light rig, but not their sampling:
/// different RNG streams, different MIS bookkeeping, f32 against f64. At 64
/// spp each image still carries visible Monte Carlo noise, and two
/// *independently* noisy estimates of the same signal differ by roughly the
/// sum of their variances — which on this subject sits in the low 30s dB even
/// when both are correct. 25 dB leaves headroom for that while remaining far
/// below what a real defect produces: a dropped shading normal (flat-faceted
/// mesh) lands near 20 dB, and a broken intersector renders background and
/// scores in the single digits.
#[test]
#[ignore = "requires GPU"]
fn mesh_render_matches_cpu_reference() {
    let Some(ctx) = ctx_or_skip("mesh_render_matches_cpu_reference") else {
        return;
    };
    let pipeline = RayTracePipeline::new(ctx).expect("pipeline creation");

    let mesh = mesh_sphere(48);
    let bvh = Arc::new(Bvh::build_mesh(&mesh));
    let scene = GpuScene::from_mesh(&mesh).expect("mesh scene builds");

    // Every triangle became one surface and one face.
    assert_eq!(
        scene.surfaces.len(),
        scene.faces.len(),
        "mesh scenes carry one surface per face"
    );
    assert!(
        scene.surfaces.iter().all(|s| s.is_gpu_traceable()),
        "a packed triangle must be traceable, or every mesh hit is a miss"
    );

    let (w, h) = (64u32, 64u32);
    let spp = 64;
    let gpu = pipeline
        .render_offline(ctx, &scene, &gpu_camera(w, h), &offline_opts(w, h, spp))
        .expect("offline render");

    assert!(
        gpu.rgba.iter().all(|v| v.is_finite()),
        "mesh render produced NaN or infinity -- a zero shading normal \
         normalized into garbage is the usual cause"
    );

    let mean = gpu.mean_luminance();
    assert!(
        mean > 1e-4,
        "mesh render is black (mean luminance {mean}) -- the triangles were \
         uploaded but the shader never hit one"
    );

    let cpu = pathtrace::render(&cpu_scene(vec![bvh]), &cpu_camera(), w, h, &cpu_opts(spp));

    // Assert the framing premise the whole-image comparison rests on.
    let covered = cpu.depth.iter().filter(|&&d| d > 0.0).count();
    let total = (w * h) as usize;
    assert!(
        covered * 100 >= total * 98,
        "the subject does not fill the frame ({covered}/{total} pixels hit)"
    );

    let cpu_mean = cpu
        .rgb
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| (0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]) as f64)
        .sum::<f64>()
        / total as f64;
    let ratio = mean as f64 / cpu_mean;
    let db = psnr(&gpu, &cpu);
    eprintln!("[mesh parity] GPU mean {mean:.4}, CPU mean {cpu_mean:.4}, ratio {ratio:.3}, PSNR {db:.1} dB");

    assert!(
        (0.5..=2.0).contains(&ratio),
        "GPU mean luminance {mean} vs CPU {cpu_mean} (ratio {ratio:.3}) -- \
         the two paths disagree about lighting, not about noise"
    );
    assert!(
        db >= 25.0,
        "GPU/CPU mesh renders differ by more than Monte Carlo noise \
         (PSNR {db:.1} dB, threshold 25)"
    );
}

/// Shading normals must actually be interpolated. A tessellated sphere with
/// vertex normals reads smooth; the same mesh stripped of them reads
/// faceted. If the shader ignored `params[18]` and the packed normals, the
/// two would render identically.
#[test]
#[ignore = "requires GPU"]
fn interpolated_normals_change_the_shading() {
    let Some(ctx) = ctx_or_skip("interpolated_normals_change_the_shading") else {
        return;
    };
    let pipeline = RayTracePipeline::new(ctx).expect("pipeline creation");

    // Coarse on purpose: at high tessellation smooth and faceted converge,
    // and the difference this test looks for would vanish into the noise.
    let smooth = mesh_sphere(12);
    let mut faceted = smooth.clone();
    faceted.normals.clear();

    let (w, h) = (64u32, 64u32);
    let cam = gpu_camera(w, h);
    let opts = offline_opts(w, h, 32);

    let a = pipeline
        .render_offline(
            ctx,
            &GpuScene::from_mesh(&smooth).expect("smooth scene"),
            &cam,
            &opts,
        )
        .expect("smooth render");
    let b = pipeline
        .render_offline(
            ctx,
            &GpuScene::from_mesh(&faceted).expect("faceted scene"),
            &cam,
            &opts,
        )
        .expect("faceted render");

    // Same geometry, same seed, same sample count: any difference is the
    // shading normal and nothing else.
    let diff: f64 = a
        .rgba
        .iter()
        .zip(&b.rgba)
        .map(|(x, y)| (x - y).abs() as f64)
        .sum::<f64>()
        / a.rgba.len() as f64;
    eprintln!("[normals] mean |smooth - faceted| = {diff:.5}");
    assert!(
        diff > 1e-3,
        "dropping the vertex normals changed nothing (mean diff {diff:.6}) -- \
         the shader is not reading the packed shading normals"
    );

    // ...and the faceted one must still be a valid image, not NaN soup from
    // normalizing the zeroed normal slots.
    assert!(b.rgba.iter().all(|v| v.is_finite()));
    assert!(b.mean_luminance() > 1e-4);
}

/// A merged BRep + mesh scene must render both halves.
///
/// `GpuScene::merge` rebases every cross-buffer index; a triangle's
/// `surface_idx` has to survive that just as an analytic face's does. The two
/// subjects are placed side by side so each owns a known half of the frame,
/// which lets the test check coverage per region rather than trusting a
/// whole-image statistic to notice one of them missing.
#[test]
#[ignore = "requires GPU"]
fn merged_brep_and_mesh_scene_shows_both() {
    let Some(ctx) = ctx_or_skip("merged_brep_and_mesh_scene_shows_both") else {
        return;
    };
    let pipeline = RayTracePipeline::new(ctx).expect("pipeline creation");

    // Analytic sphere at the origin, mesh cube well clear of it to the +x
    // side. The cube is displaced by editing its mesh vertices rather than
    // by transforming the solid: this test is about the GPU consuming
    // triangles, and a mesh translation is exact, so it introduces nothing
    // the parity check would then have to tolerate.
    let sphere = make_sphere(2.0, 32);
    let mut cube_mesh = vcad_kernel_tessellate::tessellate(&make_cube(3.0, 3.0, 3.0), 16);
    // `make_cube` spans 0..3 on each axis; centre it on y and z, then push
    // it out to x in 5..8.
    for v in cube_mesh.vertices.as_chunks_mut::<3>().0 {
        v[0] += 5.0;
        v[1] -= 1.5;
        v[2] -= 1.5;
    }

    let scene = GpuScene::from_brep(&sphere)
        .expect("analytic half builds")
        .merge(GpuScene::from_mesh(&cube_mesh).expect("mesh half builds"));

    // The merge must have kept every surface traceable and every face
    // pointing at a surface that exists.
    assert!(scene.surfaces.iter().all(|s| s.is_gpu_traceable()));
    assert!(
        scene
            .faces
            .iter()
            .all(|f| (f.surface_idx as usize) < scene.surfaces.len()),
        "merge left a face pointing past the end of the surface array"
    );

    let (w, h) = (96u32, 96u32);
    let cam = GpuCamera::new(
        [0.0, 0.0, 20.0],
        [0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        45f32.to_radians(),
        w,
        h,
    );
    let out = pipeline
        .render_offline(ctx, &scene, &cam, &offline_opts(w, h, 32))
        .expect("merged render");

    assert!(out.rgba.iter().all(|v| v.is_finite()));

    // Depth is the honest coverage signal: the background has its own
    // radiance, so a bright pixel does not prove a subject is there. The
    // offline result carries no depth, so use the CPU render of the same
    // scene for the geometry check and the GPU render for the parity one.
    let cpu = pathtrace::render(
        &cpu_scene(vec![
            Arc::new(Bvh::build(&sphere)),
            Arc::new(Bvh::build_mesh(&cube_mesh)),
        ]),
        &Camera::look_at(
            Point3::new(0.0, 0.0, 20.0),
            Point3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            45.0,
        ),
        w,
        h,
        &cpu_opts(32),
    );

    // Column ranges each subject projects into. At z=20 with a 45deg vertical
    // FOV the frame half-width at z=0 is 20*tan(22.5deg) = 8.28, so world x
    // maps to column 48 + 48*x/8.28: the sphere (x in -2..2) lands in 36..60
    // and the cube (x in 5..8) in 77..94. The bands below are those, trimmed
    // in a little so a pixel of antialiasing at the silhouette cannot leak
    // one subject's coverage into the other's count.
    let hits_in = |x0: u32, x1: u32| -> usize {
        (0..h)
            .flat_map(|y| (x0..x1).map(move |x| (x, y)))
            .filter(|(x, y)| cpu.depth[(y * w + x) as usize] > 0.0)
            .count()
    };
    let (left, right) = (hits_in(38, 58), hits_in(79, 92));
    assert!(left > 100, "the analytic sphere is missing ({left} px)");
    assert!(right > 100, "the mesh cube is missing ({right} px)");

    // Both halves must also agree between the renderers. This is the check
    // that would catch a merge rebasing triangles onto the wrong surfaces:
    // the image would still be non-empty on both sides, but wrong.
    let db = psnr_masked(&out, &cpu, true);
    eprintln!("[merged] left {left} px, right {right} px, subject PSNR {db:.1} dB");
    assert!(
        db >= 20.0,
        "merged BRep+mesh render disagrees with the CPU reference \
         (PSNR {db:.1} dB) -- index rebasing across the merge is suspect"
    );
}

/// Measurement, not an assertion: mesh-heavy render, GPU against CPU.
/// Run with `--ignored --nocapture` to see the numbers.
#[test]
#[ignore = "benchmark; requires GPU"]
fn bench_mesh_gpu_vs_cpu() {
    let Some(ctx) = ctx_or_skip("bench_mesh_gpu_vs_cpu") else {
        return;
    };
    let pipeline = RayTracePipeline::new(ctx).expect("pipeline creation");

    let mesh = mesh_sphere(300);
    let tris = mesh.indices.len() / 3;
    let (w, h) = (512u32, 512u32);
    let spp = 128u32;

    let t_build = std::time::Instant::now();
    let scene = GpuScene::from_mesh(&mesh).expect("mesh scene builds");
    let build = t_build.elapsed();

    let surface_mb =
        scene.surfaces.len() * std::mem::size_of::<vcad_kernel_raytrace::gpu::GpuSurface>();
    eprintln!(
        "[bench] {tris} triangles -> {} surfaces / {} faces / {} BVH nodes, \
         {:.1} MB of surface buffer, scene build {:?}",
        scene.surfaces.len(),
        scene.faces.len(),
        scene.bvh_nodes.len(),
        surface_mb as f64 / (1024.0 * 1024.0),
        build,
    );

    // Warm up shader/pipeline caches so the timed run is not paying for them.
    let cam = gpu_camera(w, h);
    let _ = pipeline
        .render_offline(ctx, &scene, &cam, &offline_opts(w, h, 2))
        .expect("warmup");

    let t0 = std::time::Instant::now();
    let out = pipeline
        .render_offline(ctx, &scene, &cam, &offline_opts(w, h, spp))
        .expect("gpu render");
    let gpu = t0.elapsed();
    assert!(out.mean_luminance() > 0.0);

    let bvh = Arc::new(Bvh::build_mesh(&mesh));
    let cpu_sc = cpu_scene(vec![bvh]);
    let t1 = std::time::Instant::now();
    let _ = pathtrace::render(&cpu_sc, &cpu_camera(), w, h, &cpu_opts(spp));
    let cpu = t1.elapsed();

    eprintln!(
        "[bench] {w}x{h} @ {spp} spp, {tris} tris: GPU {:?}, CPU {:?} ({:.1}x)",
        gpu,
        cpu,
        cpu.as_secs_f64() / gpu.as_secs_f64(),
    );
}
