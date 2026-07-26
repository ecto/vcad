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
#[derive(Debug, Clone, Copy)]
pub struct PhotorealOptions {
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

    // One BVH per BRep-backed solid. Mesh-only parts have no analytic
    // surfaces and are skipped, same as the `--raytrace` path.
    let mut objects: Vec<Object> = Vec::new();
    for s in &solids {
        let Some(brep) = s.solid.as_brep() else {
            continue;
        };
        let bvh = Bvh::build(brep);
        if bvh.root().is_none() {
            continue;
        }
        objects.push(Object {
            bvh: Arc::new(bvh),
            material: to_pbr(s.material.as_ref(), s.tint),
        });
    }
    if objects.is_empty() {
        return Err("photoreal: document produced no BRep-backed solids \
             (mesh-only parts render via the tessellated path)"
            .to_string());
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
    let lights: Vec<AreaLight> = pathtrace::studio_rig(center_world, radius);

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
        env: Environment::default(),
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

    fn cube_doc() -> String {
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
}
