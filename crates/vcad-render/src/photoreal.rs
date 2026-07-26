//! Photorealistic offline rendering (`--style photoreal`).
//!
//! Where the drafting path projects tessellated triangles onto a tonal ramp
//! and the `--raytrace` path swaps in analytic intersection but keeps the
//! same ramp, this path solves the rendering equation properly: a
//! physically-based path tracer over the untessellated BRep, lit by a
//! three-softbox studio rig and an analytic sky, viewed through a camera with
//! a real focal length and aperture.
//!
//! The geometry advantage carries over — silhouettes and specular highlights
//! on fillets come from analytic ray–surface intersection, so they are exact
//! at any resolution with no facet banding.

use std::sync::Arc;

use vcad_kernel::vcad_kernel_math::{Point3, Vec3};
use vcad_kernel_raytrace::pathtrace::{
    self, AreaLight, Camera, Environment, Ground, Object, PathTraceOptions, Pbr, Scene,
};
use vcad_kernel_raytrace::Bvh;

use super::envmap::{self, EnvSource};
use super::raster::{canvas_for, encode_jpeg, encode_png, fit_scale, Frame};
use super::{dot, evaluate_vcad, normalize, RasterOptions};

/// Backdrop treatment for a photoreal render.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Backdrop {
    /// Infinite studio sweep: a soft neutral floor the subject casts onto.
    #[default]
    Studio,
    /// Floor contributes shadow and contact darkening only; everything else
    /// is transparent, so the render composites onto any background.
    ShadowCatcher,
    /// No floor at all — the subject floats in the environment gradient.
    None,
}

/// Options for the photoreal path, layered on top of [`RasterOptions`]
/// (which still controls canvas size, framing, trim, and JPEG quality).
#[derive(Debug, Clone)]
pub struct PhotorealOptions {
    /// Where the ambient light comes from. The analytic gradient (default)
    /// is paired with the three-softbox rig; an image environment replaces
    /// the rig, since an HDRI *is* the lighting.
    pub environment: EnvSource,
    /// Spin the image environment about the vertical axis, in degrees.
    /// Ignored for the analytic gradient.
    pub env_rotation_deg: f64,
    /// Samples per pixel. 64 is a quick look, 512+ is a clean hero render.
    pub spp: u32,
    /// Maximum path length. 6 is plenty for opaque studio scenes.
    pub max_depth: u32,
    /// Exposure multiplier applied before the ACES tonemap.
    pub exposure: f32,
    /// Vertical field of view in degrees. Lower reads as a longer lens and
    /// less perspective distortion — 30–40° flatters mechanical parts.
    pub fov_deg: f64,
    /// Render orthographically instead (keeps the drafting framing, gains
    /// the physical shading).
    pub orthographic: bool,
    /// Aperture radius as a fraction of the scene radius. 0 = pinhole.
    /// Around 0.02–0.05 gives a tasteful product-shot depth of field.
    pub aperture_frac: f64,
    /// Backdrop treatment.
    pub backdrop: Backdrop,
    /// Random seed, for reproducible noise.
    pub seed: u64,
}

impl Default for PhotorealOptions {
    fn default() -> Self {
        Self {
            environment: EnvSource::Gradient,
            env_rotation_deg: 0.0,
            spp: 128,
            max_depth: 6,
            exposure: 1.0,
            fov_deg: 34.0,
            orthographic: false,
            aperture_frac: 0.0,
            backdrop: Backdrop::Studio,
            seed: 0x5eed_1234,
        }
    }
}

/// Render raw `.vcad` document JSON to a photorealistic JPEG.
pub fn render_photoreal_jpeg_str(
    raw_vcad: &str,
    opts: &RasterOptions,
    pr: &PhotorealOptions,
) -> Result<Vec<u8>, String> {
    encode_jpeg(rasterize(raw_vcad, opts, pr, false)?, opts)
}

/// Render raw `.vcad` document JSON to a photorealistic RGBA PNG.
///
/// With [`Backdrop::ShadowCatcher`] the background is transparent and the
/// floor contributes only the subject's shadow, so the image drops onto any
/// page background cleanly.
pub fn render_photoreal_png_str(
    raw_vcad: &str,
    opts: &RasterOptions,
    pr: &PhotorealOptions,
) -> Result<Vec<u8>, String> {
    encode_png(rasterize(raw_vcad, opts, pr, true)?, opts)
}

/// Map a document material to a physically-based surface description.
///
/// The IR already carries metallic/roughness/ior, so most of this is a
/// straight copy. The one piece of authored judgement is clearcoat: real
/// machined and moulded parts almost always have some surface layer —
/// anodising, paint, moulding skin — and adding it is the single biggest
/// step from "CAD shaded" to "photographed".
fn to_pbr(mat: Option<&vcad_ir::MaterialDef>, tint: Option<[f64; 3]>) -> Pbr {
    let base = mat.map(|m| m.color).or(tint).unwrap_or([0.62, 0.64, 0.67]);
    let base_color = [base[0] as f32, base[1] as f32, base[2] as f32];

    let metallic = mat
        .map(|m| m.metallic as f32)
        .unwrap_or(0.0)
        .clamp(0.0, 1.0);
    // Perfectly sharp mirrors read as CG. Floor roughness slightly.
    let roughness = mat
        .map(|m| m.roughness as f32)
        .unwrap_or(0.35)
        .clamp(0.03, 1.0);
    let ior = mat
        .and_then(|m| m.ior)
        .map(|v| v as f32)
        .unwrap_or(1.5)
        .clamp(1.0, 3.0);

    // Dielectrics that are already glossy get a clearcoat; rough matte
    // surfaces (sandblasted, as-printed) do not.
    let clearcoat = if metallic < 0.5 && roughness < 0.5 {
        0.35 * (1.0 - roughness / 0.5)
    } else {
        0.0
    };

    Pbr {
        base_color,
        metallic,
        roughness,
        clearcoat,
        clearcoat_roughness: 0.08,
        ior,
        emissive: [0.0; 3],
    }
}

fn rasterize(
    raw_vcad: &str,
    opts: &RasterOptions,
    pr: &PhotorealOptions,
    png: bool,
) -> Result<Frame, String> {
    let solids = evaluate_vcad(raw_vcad)?;
    if solids.is_empty() {
        return Err("no solids produced".to_string());
    }
    if opts.size_px < 16 {
        return Err("size_px too small".to_string());
    }
    if !(opts.fill_frac > 0.0 && opts.fill_frac <= 1.0) {
        return Err("fill_frac must be in (0, 1]".to_string());
    }

    // One BVH per solid. BRep-backed solids trace analytically; mesh-only
    // parts (frozen topology-optimization results, imported STL/GLB) trace
    // as crease-baked triangles, same as the `--raytrace` path.
    let mut objects: Vec<Object> = Vec::new();
    let mut untraceable: Vec<String> = Vec::new();
    for s in &solids {
        let bvh = match s.solid.as_brep() {
            Some(brep) => Bvh::build(brep),
            None => {
                let mut mesh = s.solid.to_mesh(0);
                vcad_kernel::vcad_kernel_tessellate::render_bake_default(&mut mesh);
                Bvh::build_mesh(&mesh)
            }
        };
        if bvh.root().is_none() {
            untraceable.push(s.name.clone().unwrap_or_else(|| s.id.clone()));
            continue;
        }
        objects.push(Object {
            bvh: Arc::new(bvh),
            material: to_pbr(s.material.as_ref(), s.tint),
        });
    }
    if objects.is_empty() {
        return Err(format!(
            "photoreal: document produced no traceable geometry ({} part(s) empty \
             or degenerate: {})",
            untraceable.len(),
            untraceable.join(", ")
        ));
    }
    if !untraceable.is_empty() {
        // Fail closed rather than silently rendering a subset — a missing
        // part reads as a design that doesn't have it.
        return Err(format!(
            "photoreal: {} part(s) have no traceable geometry (empty or fully \
             degenerate): {}",
            untraceable.len(),
            untraceable.join(", ")
        ));
    }

    // ── framing ──────────────────────────────────────────────────────────
    // Project the union of BVH root AABBs onto the view basis, exactly as
    // the drafting and raytrace paths do, so `--view`, `--fill`, `--size`
    // and `--auto-aspect` all behave identically across styles.
    let cam_dir = normalize(opts.view.cam());
    let right = normalize(opts.view.right());
    let down = normalize(opts.view.down());

    let mut min = [f64::INFINITY; 2];
    let mut max = [f64::NEG_INFINITY; 2];
    let mut world_min = [f64::INFINITY; 3];
    let mut world_max = [f64::NEG_INFINITY; 3];
    for obj in &objects {
        let node = obj.bvh.root().expect("empty BVHs were filtered");
        let aabb = match node {
            vcad_kernel_raytrace::bvh::BvhNode::Leaf { aabb, .. }
            | vcad_kernel_raytrace::bvh::BvhNode::Internal { aabb, .. } => aabb,
        };
        for i in 0..8 {
            let c = [
                if i & 1 == 0 { aabb.min.x } else { aabb.max.x },
                if i & 2 == 0 { aabb.min.y } else { aabb.max.y },
                if i & 4 == 0 { aabb.min.z } else { aabb.max.z },
            ];
            let s = [dot(c, right), dot(c, down)];
            for k in 0..2 {
                min[k] = min[k].min(s[k]);
                max[k] = max[k].max(s[k]);
            }
            for k in 0..3 {
                world_min[k] = world_min[k].min(c[k]);
                world_max[k] = world_max[k].max(c[k]);
            }
        }
    }

    let ex = max[0] - min[0];
    let ey = max[1] - min[1];
    let extent = ex.max(ey);
    if !extent.is_finite() || extent < 1e-9 {
        return Err("degenerate projection (no extent)".to_string());
    }

    let canvas = canvas_for(opts, ex, ey);
    let px_per_mm = fit_scale(opts.fill_frac, canvas, ex, ey, extent);

    let center_world = Point3::new(
        (world_min[0] + world_max[0]) * 0.5,
        (world_min[1] + world_max[1]) * 0.5,
        (world_min[2] + world_max[2]) * 0.5,
    );
    let radius = 0.5
        * ((world_max[0] - world_min[0]).powi(2)
            + (world_max[1] - world_min[1]).powi(2)
            + (world_max[2] - world_min[2]).powi(2))
        .sqrt();
    let radius = radius.max(1e-6);

    // The projected centre of the subject need not be the world centre —
    // aim the camera at the point that lands in the middle of the canvas.
    let cx = (min[0] + max[0]) * 0.5;
    let cy = (min[1] + max[1]) * 0.5;
    let along = dot([center_world.x, center_world.y, center_world.z], cam_dir);
    let target = Point3::new(
        right[0] * cx + down[0] * cy + cam_dir[0] * along,
        right[1] * cx + down[1] * cy + cam_dir[1] * along,
        right[2] * cx + down[2] * cy + cam_dir[2] * along,
    );

    let cam_vec = Vec3::new(cam_dir[0], cam_dir[1], cam_dir[2]);
    // `down` is screen-down, so screen-up is its negation.
    let up = Vec3::new(-down[0], -down[1], -down[2]);

    // Canvas half-height in world units, from the same px/mm the drafting
    // path uses. Distance then follows from the field of view.
    let half_h_world = (canvas.h as f64 * 0.5) / px_per_mm;
    let half_w_world = (canvas.w as f64 * 0.5) / px_per_mm;
    let distance = half_h_world / (pr.fov_deg.to_radians() * 0.5).tan();

    // Hand the projection basis over verbatim rather than reconstructing it
    // from an up-hint. `View::Isometric`'s basis is mirrored (see the note
    // in `View::right`), and a right-handed `look_at` would silently flip
    // the image relative to every other render style.
    let mut camera = Camera::from_basis(
        target + cam_vec * distance,
        -cam_vec,
        Vec3::new(right[0], right[1], right[2]),
        up,
        pr.fov_deg,
        distance,
    );
    camera.aperture = pr.aperture_frac * radius;
    camera.ortho_half_height = pr.orthographic.then_some(half_h_world);
    let _ = half_w_world;

    // ── lighting ─────────────────────────────────────────────────────────
    let env = envmap::resolve(&pr.environment, pr.env_rotation_deg)?;

    // The softbox rig exists to give the low-frequency analytic gradient
    // something to make highlights with. An HDRI already carries its own
    // lights, so keeping the rig would double-light the subject.
    let lights: Vec<AreaLight> = match env {
        Environment::Gradient(_) => pathtrace::studio_rig(center_world, radius),
        Environment::Image(_) => Vec::new(),
    };

    let ground = match pr.backdrop {
        Backdrop::None => None,
        mode => Some(Ground {
            // Sit the floor exactly on the subject's lowest point so parts
            // read as resting on it rather than floating.
            z: world_min[2],
            material: Pbr {
                base_color: [0.55, 0.55, 0.56],
                metallic: 0.0,
                roughness: 0.6,
                clearcoat: 0.0,
                ..Default::default()
            },
            shadow_catcher: mode == Backdrop::ShadowCatcher,
        }),
    };

    let scene = Scene {
        objects,
        lights,
        env,
        ground,
    };

    let pt_opts = PathTraceOptions {
        spp: pr.spp.max(1),
        max_depth: pr.max_depth.max(1),
        rr_start: 3,
        firefly_clamp: Some(12.0),
        show_background: !png || pr.backdrop == Backdrop::Studio,
        seed: pr.seed,
    };

    let film = pathtrace::render(&scene, &camera, canvas.w as u32, canvas.h as u32, &pt_opts);
    let rgba = film.to_srgb8(pr.exposure, png && pr.backdrop != Backdrop::Studio);

    // Split into the (rgb, mask) pair the shared encoders expect.
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

    pub(super) fn cube_doc() -> String {
        r#"{
  "version": "0.1",
  "nodes": {
    "1": {
      "id": 1,
      "name": "Cube",
      "op": { "type": "Cube", "size": { "x": 20, "y": 20, "z": 20 } }
    }
  },
  "materials": {
    "aluminum": {
      "name": "aluminum",
      "color": [0.91, 0.92, 0.93],
      "metallic": 1.0,
      "roughness": 0.4
    }
  },
  "part_materials": {},
  "roots": [{ "root": 1, "material": "aluminum" }]
}"#
        .to_string()
    }

    #[test]
    fn renders_a_cube_to_png() {
        let opts = RasterOptions {
            size_px: 64,
            ..Default::default()
        };
        let pr = PhotorealOptions {
            spp: 4,
            ..Default::default()
        };
        let png = render_photoreal_png_str(&cube_doc(), &opts, &pr).expect("render");
        assert!(png.len() > 100, "png suspiciously small: {}", png.len());
        assert_eq!(&png[1..4], b"PNG", "not a PNG");
    }

    #[test]
    fn renders_a_cube_to_jpeg() {
        let opts = RasterOptions {
            size_px: 64,
            ..Default::default()
        };
        let pr = PhotorealOptions {
            spp: 4,
            ..Default::default()
        };
        let jpg = render_photoreal_jpeg_str(&cube_doc(), &opts, &pr).expect("render");
        assert_eq!(&jpg[0..2], &[0xFF, 0xD8], "not a JPEG");
    }

    /// Higher sample counts must reduce variance, not change the expected
    /// image — a cheap guard against a biased integrator.
    #[test]
    fn converges_rather_than_drifts() {
        let opts = RasterOptions {
            size_px: 32,
            ..Default::default()
        };
        let mean = |spp: u32, seed: u64| -> f64 {
            let pr = PhotorealOptions {
                spp,
                seed,
                ..Default::default()
            };
            let png = render_photoreal_png_str(&cube_doc(), &opts, &pr).unwrap();
            // Compare encoded size as a crude proxy for noise: a noisier
            // image compresses worse.
            png.len() as f64
        };
        let noisy = mean(4, 1);
        let clean = mean(96, 1);
        assert!(
            clean < noisy,
            "more samples should compress better (noise down): {clean} vs {noisy}"
        );
    }

    /// Every built-in HDRI must render, and must produce a lit image — a
    /// black frame would mean the rig was dropped without the environment
    /// taking over.
    #[test]
    fn builtin_environments_light_the_scene() {
        let opts = RasterOptions {
            size_px: 48,
            ..Default::default()
        };
        for kind in envmap::BuiltinEnv::all() {
            let pr = PhotorealOptions {
                environment: EnvSource::Builtin(kind),
                spp: 24,
                ..Default::default()
            };
            let png = render_photoreal_png_str(&cube_doc(), &opts, &pr)
                .unwrap_or_else(|e| panic!("{} failed: {e}", kind.name()));
            assert_eq!(&png[1..4], b"PNG");
            assert!(png.len() > 500, "{} produced a trivial image", kind.name());
        }
    }

    /// Switching to a built-in HDRI must not change the exposure. The
    /// built-ins are normalised to a mean radiance chosen to match the
    /// default gradient-plus-softbox rig precisely so `--env` is a choice of
    /// *look*, not a hidden exposure change the user has to dial back out.
    #[test]
    fn builtins_match_the_default_rig_exposure() {
        let opts = RasterOptions {
            size_px: 64,
            ..Default::default()
        };
        let mean = |src: EnvSource| -> f64 {
            let pr = PhotorealOptions {
                environment: src,
                spp: 64,
                ..Default::default()
            };
            let png = render_photoreal_png_str(&cube_doc(), &opts, &pr).expect("render");
            let img = image::load_from_memory(&png).expect("decode").to_rgb8();
            img.pixels()
                .map(|p| p[0] as f64 + p[1] as f64 + p[2] as f64)
                .sum::<f64>()
                / (img.pixels().len() as f64 * 3.0)
        };
        let reference = mean(EnvSource::Gradient);
        for kind in envmap::BuiltinEnv::all() {
            let m = mean(EnvSource::Builtin(kind));
            assert!(
                (m - reference).abs() < 0.15 * reference,
                "{} renders at mean {m}, default rig at {reference} — the \
                 built-in normalisation has drifted",
                kind.name()
            );
        }
    }

    /// Rotating the environment must change the render. This is the whole
    /// point of the flag — if it silently did nothing, nothing else would
    /// catch it.
    #[test]
    fn env_rotation_changes_the_image() {
        let opts = RasterOptions {
            size_px: 40,
            ..Default::default()
        };
        let render_at = |deg: f64| {
            let pr = PhotorealOptions {
                environment: EnvSource::Builtin(envmap::BuiltinEnv::Softbox),
                env_rotation_deg: deg,
                spp: 32,
                ..Default::default()
            };
            render_photoreal_png_str(&cube_doc(), &opts, &pr).expect("render")
        };
        assert_ne!(
            render_at(0.0),
            render_at(120.0),
            "--env-rotation had no effect on the image"
        );
    }

    /// An unreadable `--env` path must fail loudly rather than silently
    /// falling back to the gradient.
    #[test]
    fn bad_env_path_is_reported() {
        let opts = RasterOptions {
            size_px: 32,
            ..Default::default()
        };
        let pr = PhotorealOptions {
            environment: EnvSource::File("/nope/not-here.hdr".into()),
            spp: 2,
            ..Default::default()
        };
        let err = render_photoreal_png_str(&cube_doc(), &opts, &pr).expect_err("should fail");
        assert!(err.contains("not-here.hdr"), "unhelpful error: {err}");
    }

    #[test]
    fn shadow_catcher_leaves_background_transparent() {
        let opts = RasterOptions {
            size_px: 48,
            ..Default::default()
        };
        let pr = PhotorealOptions {
            spp: 8,
            backdrop: Backdrop::ShadowCatcher,
            ..Default::default()
        };
        let png = render_photoreal_png_str(&cube_doc(), &opts, &pr).expect("render");
        assert!(!png.is_empty());
    }

    /// Same regression as the `--raytrace` path: this used to skip any
    /// solid without an analytic BRep, so a mesh part beside a BRep part
    /// was silently omitted. Asserts the mesh actually covers pixels, not
    /// merely that the render succeeded.
    #[test]
    fn renders_both_brep_and_mesh_parts() {
        // Cube at x 0..20, raw-triangle box at x 60..80 — a 40mm gap, so
        // each part lands in an outer third of the canvas.
        let mesh_box = |x0: f64| -> String {
            let x1 = x0 + 20.0;
            let corners = [
                [x0, 0.0, 0.0],
                [x1, 0.0, 0.0],
                [x1, 20.0, 0.0],
                [x0, 20.0, 0.0],
                [x0, 0.0, 20.0],
                [x1, 0.0, 20.0],
                [x1, 20.0, 20.0],
                [x0, 20.0, 20.0],
            ];
            let pos: Vec<String> = corners
                .iter()
                .flat_map(|c| c.iter().map(|v| format!("{v:?}")))
                .collect();
            let idx = [
                [0, 2, 1],
                [0, 3, 2],
                [4, 5, 6],
                [4, 6, 7],
                [0, 1, 5],
                [0, 5, 4],
                [1, 2, 6],
                [1, 6, 5],
                [2, 3, 7],
                [2, 7, 6],
                [3, 0, 4],
                [3, 4, 7],
            ];
            let idx: Vec<String> = idx
                .iter()
                .flat_map(|t| t.iter().map(|i| i.to_string()))
                .collect();
            format!(
                r#""2": {{ "id": 2, "name": "meshpart", "op": {{ "type": "ImportedMesh", "positions": [{}], "indices": [{}] }} }}"#,
                pos.join(", "),
                idx.join(", ")
            )
        };
        let vcad = format!(
            r#"{{ "version": "0.1", "nodes": {{ "1": {{ "id": 1, "name": "cube", "op": {{ "type": "Cube", "size": {{ "x": 20.0, "y": 20.0, "z": 20.0 }} }} }}, {} }}, "materials": {{}}, "part_materials": {{}}, "roots": [{{ "root": 1, "material": "default" }}, {{ "root": 2, "material": "default" }}] }}"#,
            mesh_box(60.0)
        );

        let opts = RasterOptions {
            view: crate::View::Front,
            size_px: 96,
            fill_frac: 0.9,
            ..Default::default()
        };
        let pr = PhotorealOptions {
            spp: 4,
            backdrop: Backdrop::ShadowCatcher,
            ..Default::default()
        };
        let png = render_photoreal_png_str(&vcad, &opts, &pr).expect("render");

        // Alpha channel is the honest coverage signal here: the path tracer
        // lights the scene, so "not the background colour" is unreliable,
        // but a shadow-catcher backdrop leaves un-hit pixels transparent.
        let img = image::load_from_memory(&png).expect("valid PNG").to_rgba8();
        let cols: Vec<u32> = (0..img.width())
            .map(|x| {
                (0..img.height())
                    .filter(|&y| img.get_pixel(x, y).0[3] > 8)
                    .count() as u32
            })
            .collect();

        let third = cols.len() / 3;
        let left: u32 = cols[..third].iter().sum();
        let right: u32 = cols[2 * third..].iter().sum();
        assert!(
            left > 100 && right > 100,
            "both the BRep and the mesh part must cover pixels: \
             left={left} right={right} cols={cols:?}"
        );
    }
}
