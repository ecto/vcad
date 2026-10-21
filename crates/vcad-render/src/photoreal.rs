//! Photorealistic offline rendering (`--style photoreal`).
//!
//! Where the drafting path projects tessellated triangles onto a tonal ramp
//! and the `--raytrace` path swaps in analytic intersection but keeps the
//! same ramp, this path solves the rendering equation properly: a
//! physically-based path tracer lit by a three-softbox studio rig and an
//! analytic sky, viewed through a camera with a real focal length and
//! aperture.
//!
//! **Geometry comes from a tessellation by default** ([`MESH_SEGMENTS`]), so
//! the content-addressed root-mesh cache can serve it and a re-render pays
//! only for path tracing rather than re-evaluating every BRep. `--exact`
//! ([`PhotorealOptions::exact`]) swaps in analytic ray–surface intersection,
//! which is exact at any resolution with no facet banding on curved
//! silhouettes, at the price of a full kernel evaluation every run.

use std::sync::Arc;

use vcad_kernel::vcad_kernel_math::{Point3, Vec3};
use vcad_kernel_raytrace::pathtrace::{
    self, AreaLight, Camera, Environment, Ground, Object, PathTraceOptions, Pbr, Scene,
};
use vcad_kernel_raytrace::BrepBvh;
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
    /// Distance from the camera to the plane of perfect focus, in mm.
    /// `None` focuses on the framed subject's centre (the `--focus` part
    /// when one is given). Only matters with a non-zero aperture.
    pub focus_dist: Option<f64>,
    /// Backdrop treatment.
    pub backdrop: Backdrop,
    /// Random seed, for reproducible noise.
    pub seed: u64,
    /// Run the edge-aware à-trous denoiser over the film before tonemapping.
    ///
    /// On by default: it is worth far more per second of render time than the
    /// equivalent extra samples. Turn it off for reference renders.
    pub denoise: bool,
    /// Stop sampling a pixel early once its own variance says the rest of the
    /// budget cannot move it visibly (`--no-adaptive` turns this off).
    ///
    /// On by default. `spp` becomes a ceiling rather than a fixed count; the
    /// stopping decision is per-pixel and made from that pixel's own running
    /// sums, so a fixed `--seed` still gives a byte-stable image.
    pub adaptive: bool,
    /// Intersect the analytic BRep surfaces instead of a tessellation of them
    /// (`--exact`).
    ///
    /// **Off by default.** A `vcad_kernel::Solid` has no serialized form, so
    /// a BRep can only ever come from re-evaluating the document — which for
    /// a real assembly is most of the wall time of a moderate-`--spp` render,
    /// and is paid again on every invocation. Tracing triangles at
    /// [`MESH_SEGMENTS`] instead lets the content-addressed root-mesh cache
    /// (`vcad_eval::cache`) supply the geometry, leaving only the path
    /// tracing to redo.
    ///
    /// The cost is curved silhouettes. At 128 segments adjacent facets differ
    /// by 2.8°, which is invisible on flat sheet and on large-radius bosses,
    /// but reads as a stair-step along the rim of a small cylinder at hero
    /// resolutions — on rose-pro at `--size 1200` it is plain on the D55
    /// actuator cans. That is what `exact` is for.
    ///
    /// Two further differences are worth knowing before diffing two renders:
    ///
    /// * **Framing shifts slightly.** The camera fits the objects' BVH root
    ///   AABBs, and a tessellation is inscribed in the surface it approximates,
    ///   so the subject's bounds are a hair smaller in mesh mode and the fit
    ///   lands at a marginally different scale. Nothing is clipped; the two
    ///   images simply do not overlay pixel-for-pixel anywhere, not just on
    ///   curves.
    /// * **Shading is not uniformly worse.** Mesh mode goes through
    ///   `render_bake`'s crease-aware normals, which resolve plate edges more
    ///   cleanly than analytic evaluation of the same BRep does; `exact` in
    ///   exchange shows faint banding across some large planar faces. Mesh
    ///   mode loses on curves and wins on creases.
    ///
    /// Note this is a *pixel* switch, not only a speed one, and it is
    /// deliberately not conditioned on whether a cache happens to be
    /// installed: a render must not change its output depending on the state
    /// of an accelerator. Mesh mode therefore tessellates on a cache miss
    /// too, so a cold render and a warm one are identical.
    pub exact: bool,
}

/// Segment count the default (non-[`exact`](PhotorealOptions::exact))
/// photoreal path tessellates curved faces at.
///
/// This is the raster path's hi-res count, but — unlike the raster path — it
/// is used at *every* canvas size rather than scaling with it. Two reasons:
/// the path tracer resolves a silhouette far more sharply than the flat
/// raster shader (a specular highlight tracks facet normals; a tonal ramp
/// mostly doesn't), so the coarser 64-segment count shows at sizes the raster
/// path gets away with; and the extra triangles cost almost nothing here,
/// since BVH traversal is logarithmic in triangle count while the
/// tessellation itself is cached.
///
/// A size-independent count also means one cache entry per root serves every
/// `--size`, instead of one entry per size bucket.
pub const MESH_SEGMENTS: u32 = crate::raster::RASTER_SEGMENTS_HIRES;

impl PhotorealOptions {
    /// Segment count to tessellate BRep-backed solids at before tracing, or
    /// `None` when [`Self::exact`] asks for analytic surfaces.
    pub(crate) fn mesh_segments(&self) -> Option<u32> {
        (!self.exact).then_some(MESH_SEGMENTS)
    }
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
            focus_dist: None,
            backdrop: Backdrop::Studio,
            seed: 0x5eed_1234,
            denoise: true,
            adaptive: true,
            exact: false,
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

/// Build one BVH per solid, world-placed at the identity.
///
/// `mesh_segments` picks the geometry representation, and comes from
/// [`PhotorealOptions::mesh_segments`]:
///
/// * `Some(n)` — the default. Every solid traces as crease-baked triangles,
///   BRep-backed ones tessellated at `n` segments. A solid that arrived
///   mesh-backed (a root-mesh cache hit, a frozen topology-optimization
///   result, an imported STL/GLB) already carries its triangles and `n` is
///   moot for it, which is exactly why cold and warm renders agree.
/// * `None` — `--exact`. BRep-backed solids intersect analytically; mesh-only
///   parts still trace as triangles, since there is nothing else to trace.
///
/// Fails closed when any part has no traceable geometry — a silently missing
/// part reads as a design that doesn't have it.
///
/// BVH construction is the one genuinely parallel-friendly stage of scene
/// setup (each solid is independent, and for a many-root assembly it is the
/// bulk of the time between evaluation and the first traced ray), so the
/// builds fan out over rayon. Results are re-zipped with `solids` afterwards,
/// so object order — and therefore the untraceable-part diagnostics — is
/// unchanged from the serial version.
pub(crate) fn build_objects(
    solids: &[crate::SceneSolid],
    mesh_segments: Option<u32>,
) -> Result<Vec<Object>, String> {
    use rayon::prelude::*;

    let bvhs: Vec<Bvh> = solids
        .par_iter()
        .map(|s| match (mesh_segments, s.solid.as_brep()) {
            (None, Some(brep)) => Bvh::build_brep(brep),
            // `to_mesh` ignores the segment count for a mesh-backed solid,
            // so `unwrap_or(0)` keeps --exact's mesh-only branch
            // byte-identical to what it did before this parameter existed.
            (segments, _) => Bvh::build_mesh(&traced_mesh(&s.solid, segments.unwrap_or(0))),
        })
        .collect();

    let mut objects: Vec<Object> = Vec::new();
    let mut untraceable: Vec<String> = Vec::new();
    for (s, bvh) in solids.iter().zip(bvhs) {
        if bvh.root().is_none() {
            untraceable.push(s.name.clone().unwrap_or_else(|| s.id.clone()));
            continue;
        }
        objects.push(Object::new(
            Arc::new(bvh),
            vcad_kernel_raytrace::pathtrace::from_material_def(s.material.as_ref(), s.tint),
        ));
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
        return Err(format!(
            "photoreal: {} part(s) have no traceable geometry (empty or fully \
             degenerate): {}",
            untraceable.len(),
            untraceable.join(", ")
        ));
    }
    Ok(objects)
}

/// The triangles a solid is traced as in mesh mode: tessellated, then
/// crease-baked so facets meeting at under 30° share one interpolated shading
/// normal. This is what keeps a polyline loft (every strip its own planar
/// BRep face) reading as the smooth round it stands for. Traced analytically
/// (`--exact`, and every build before #825) those strips are flat, and each
/// one catches the light on its own.
pub(crate) fn traced_mesh(
    solid: &vcad_kernel::Solid,
    segments: u32,
) -> vcad_kernel::vcad_kernel_tessellate::TriangleMesh {
    let mut mesh = solid.to_mesh(segments);
    vcad_kernel::vcad_kernel_tessellate::render_bake_default(&mut mesh);
    mesh
}

/// The eight corners of an object's BVH root AABB, in world space after
/// `transform`. These are what framing is computed from.
pub(crate) fn object_corners(obj: &Object) -> Vec<[f64; 3]> {
    object_corners_with(
        obj,
        &vcad_kernel::vcad_kernel_math::Transform {
            matrix: obj.transform.matrix,
        },
    )
}

/// `object_corners` under an explicit transform, so a caller can ask "where
/// would this object's bounds land under pose `t`" without mutating the
/// object (the --animate camera framing sweeps every pose this way).
pub(crate) fn object_corners_with(
    obj: &Object,
    transform: &vcad_kernel::vcad_kernel_math::Transform,
) -> Vec<[f64; 3]> {
    let Some(node) = obj.bvh.root() else {
        return Vec::new();
    };
    let aabb = match node {
        vcad_kernel_raytrace::bvh::BvhNode::Leaf { aabb, .. }
        | vcad_kernel_raytrace::bvh::BvhNode::Internal { aabb, .. } => aabb,
    };
    (0..8)
        .map(|i| {
            let p = Point3::new(
                if i & 1 == 0 { aabb.min.x } else { aabb.max.x },
                if i & 2 == 0 { aabb.min.y } else { aabb.max.y },
                if i & 4 == 0 { aabb.min.z } else { aabb.max.z },
            );
            let w = transform.apply_point(&p);
            [w.x, w.y, w.z]
        })
        .collect()
}

/// Frame the scene, honouring `opts.focus`: the camera fits the focused
/// part(s), while the studio floor stays under the *whole* scene so the rest
/// of the document (a desk under a product, say) is never cut by it.
pub(crate) fn frame_scene(
    solids: &[crate::SceneSolid],
    objects: &[Object],
    opts: &RasterOptions,
    pr: &PhotorealOptions,
) -> Result<Framing, String> {
    let all: Vec<[f64; 3]> = objects.iter().flat_map(object_corners).collect();
    let Some(focus) = opts.focus.as_deref() else {
        return frame_view(&all, opts, pr);
    };
    let mask = crate::focus_mask(solids, focus)?;
    let focused: Vec<[f64; 3]> = objects
        .iter()
        .zip(&mask)
        .filter(|(_, &m)| m)
        .flat_map(|(o, _)| object_corners(o))
        .collect();
    let mut framing = frame_view(&focused, opts, pr)?;
    framing.floor_z = all.iter().map(|c| c[2]).fold(f64::INFINITY, f64::min);
    Ok(framing)
}

/// Camera, canvas, and the floor height a set of world points implies.
pub(crate) struct Framing {
    pub(crate) camera: Camera,
    pub(crate) canvas: crate::raster::Canvas,
    /// World-space centre of the subject, for the studio rig.
    pub(crate) center: Point3,
    /// Bounding-sphere radius, for the rig and the aperture.
    pub(crate) radius: f64,
    /// Lowest world Z — where the studio floor sits.
    pub(crate) floor_z: f64,
}

/// Frame `points` (world-space corners) exactly as the drafting and raytrace
/// paths do, so `--view`, `--fill`, `--size` and `--auto-aspect` behave
/// identically across styles.
pub(crate) fn frame_view(
    points: &[[f64; 3]],
    opts: &RasterOptions,
    pr: &PhotorealOptions,
) -> Result<Framing, String> {
    let cam_dir = normalize(opts.view.cam());
    let right = normalize(opts.view.right());
    let down = normalize(opts.view.down());

    let mut min = [f64::INFINITY; 2];
    let mut max = [f64::NEG_INFINITY; 2];
    let mut world_min = [f64::INFINITY; 3];
    let mut world_max = [f64::NEG_INFINITY; 3];
    for c in points {
        let c = *c;
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
    if let Some(d) = pr.focus_dist {
        if !(d.is_finite() && d > 0.0) {
            return Err(format!(
                "focus distance must be a positive length in mm, got {d}"
            ));
        }
        camera.focus_dist = d;
    }
    camera.ortho_half_height = pr.orthographic.then_some(half_h_world);
    let _ = half_w_world;

    Ok(Framing {
        camera,
        canvas,
        center: center_world,
        radius,
        floor_z: world_min[2],
    })
}

/// Wrap already-built objects in the studio rig, environment, and floor.
pub(crate) fn dress_scene(
    objects: Vec<Object>,
    framing: &Framing,
    pr: &PhotorealOptions,
) -> Result<Scene, String> {
    let env = envmap::resolve(&pr.environment, pr.env_rotation_deg)?;

    // The softbox rig exists to give the low-frequency analytic gradient
    // something to make highlights with. An HDRI already carries its own
    // lights, so keeping the rig would double-light the subject.
    let lights: Vec<AreaLight> = match env {
        Environment::Gradient(_) => pathtrace::studio_rig(framing.center, framing.radius),
        Environment::Image(_) => Vec::new(),
    };

    let ground = match pr.backdrop {
        Backdrop::None => None,
        mode => Some(Ground {
            // Sit the floor exactly on the subject's lowest point so parts
            // read as resting on it rather than floating.
            z: framing.floor_z,
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

    Ok(Scene {
        objects,
        lights,
        env,
        // Daylight is a scene fact a `.vcad` document does not state, and a
        // product render's key light is the softbox rig above.
        sun: None,
        ground,
        // Gaussian splats are a kosm-render scene kind with no `.vcad`
        // spelling: a document has geometry, not a captured radiance field.
        splats: None,
    })
}

pub(crate) fn trace_options(pr: &PhotorealOptions, png: bool) -> PathTraceOptions {
    PathTraceOptions {
        spp: pr.spp.max(1),
        max_depth: pr.max_depth.max(1),
        rr_start: 3,
        firefly_clamp: Some(12.0),
        show_background: !png || pr.backdrop == Backdrop::Studio,
        seed: pr.seed,
        denoise: pr.denoise,
        // Adaptive sampling is a property of the integrator, and the
        // integrator is kosm-render's; `--no-adaptive` turns off its knob.
        adaptive: pr.adaptive,
        ..PathTraceOptions::default()
    }
}

/// Trace one frame of an already-assembled scene.
pub(crate) fn trace_frame(
    scene: &Scene,
    framing: &Framing,
    pr: &PhotorealOptions,
    png: bool,
) -> Frame {
    let canvas = framing.canvas;
    let film = pathtrace::render(
        scene,
        &framing.camera,
        canvas.w as u32,
        canvas.h as u32,
        &trace_options(pr, png),
    );
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
    Frame { rgb, mask, canvas }
}

fn rasterize(
    raw_vcad: &str,
    opts: &RasterOptions,
    pr: &PhotorealOptions,
    png: bool,
) -> Result<Frame, String> {
    check_raster_opts(opts)?;
    let solids = evaluate_vcad(raw_vcad)?;
    if solids.is_empty() {
        return Err("no solids produced".to_string());
    }
    let objects = build_objects(&solids, pr.mesh_segments())?;
    let framing = frame_scene(&solids, &objects, opts, pr)?;
    let scene = dress_scene(objects, &framing, pr)?;
    Ok(trace_frame(&scene, &framing, pr, png))
}

pub(crate) fn check_raster_opts(opts: &RasterOptions) -> Result<(), String> {
    if opts.size_px < 16 {
        return Err("size_px too small".to_string());
    }
    if !(opts.fill_frac > 0.0 && opts.fill_frac <= 1.0) {
        return Err("fill_frac must be in (0, 1]".to_string());
    }
    Ok(())
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

    /// A document that *names* a material without defining one — how every
    /// hand-written `.loon` does it — must still shade as that material
    /// rather than dropping to the path tracer's default clay.
    fn cube_doc_named(material: &str, defs: &str) -> String {
        format!(
            r#"{{
  "version": "0.1",
  "nodes": {{
    "1": {{
      "id": 1,
      "name": "Cube",
      "op": {{ "type": "Cube", "size": {{ "x": 20, "y": 20, "z": 20 }} }}
    }}
  }},
  "materials": {{{defs}}},
  "part_materials": {{}},
  "roots": [{{ "root": 1, "material": "{material}" }}]
}}"#
        )
    }

    #[test]
    fn undeclared_material_name_resolves_to_the_builtin() {
        let solids = evaluate_vcad(&cube_doc_named("copper", "")).expect("eval");
        let def = solids[0].material.as_ref().expect("copper should resolve");
        assert_eq!(
            def.color,
            crate::materials::builtin("copper").unwrap().color
        );

        let pbr = vcad_kernel_raytrace::pathtrace::from_material_def(
            solids[0].material.as_ref(),
            solids[0].tint,
        );
        assert_eq!(pbr.metallic, 1.0, "copper must trace as a metal");
        assert!(
            pbr.base_color[0] > pbr.base_color[2] + 0.3,
            "copper must read warm, got {:?}",
            pbr.base_color
        );
        assert_ne!(
            pbr.base_color,
            Pbr::default().base_color,
            "copper fell through to default clay"
        );
    }

    #[test]
    fn authored_material_overrides_the_builtin() {
        let doc = cube_doc_named(
            "copper",
            r#""copper": { "name": "copper", "color": [0.0, 1.0, 0.0],
                          "metallic": 0.0, "roughness": 0.9 }"#,
        );
        let solids = evaluate_vcad(&doc).expect("eval");
        let pbr = vcad_kernel_raytrace::pathtrace::from_material_def(
            solids[0].material.as_ref(),
            solids[0].tint,
        );
        assert_eq!(pbr.base_color, [0.0, 1.0, 0.0], "authored def must win");
        assert_eq!(pbr.metallic, 0.0);
    }

    #[test]
    fn unknown_material_name_still_falls_back_to_clay() {
        let solids = evaluate_vcad(&cube_doc_named("unobtainium", "")).expect("eval");
        assert!(solids[0].material.is_none());
        let pbr = vcad_kernel_raytrace::pathtrace::from_material_def(None, solids[0].tint);
        assert_eq!(pbr.base_color, Pbr::default().base_color);
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
    /// `--focus` frames the focused part's bounds, while the studio floor
    /// still sits under the whole scene (here, a slab under a small cube).
    #[test]
    fn focus_frames_part_but_floor_spans_scene() {
        let vcad = r#"{ "version": "0.1", "nodes": {
            "1": { "id": 1, "name": "slab", "op": { "type": "Cube", "size": { "x": 200, "y": 200, "z": 5 } } },
            "2": { "id": 2, "name": "small_raw", "op": { "type": "Cube", "size": { "x": 10, "y": 10, "z": 10 } } },
            "3": { "id": 3, "name": "small", "op": { "type": "Translate", "child": 2, "offset": { "x": 95, "y": 95, "z": 5 } } } },
          "materials": {}, "part_materials": {},
          "roots": [{ "root": 1, "material": "default" }, { "root": 3, "material": "default" }] }"#;
        let solids = crate::evaluate_vcad(vcad).unwrap();
        let pr = PhotorealOptions::default();
        let objects = build_objects(&solids, pr.mesh_segments()).unwrap();
        let whole = frame_scene(&solids, &objects, &RasterOptions::default(), &pr).unwrap();
        let opts = RasterOptions {
            focus: Some("small".into()),
            ..RasterOptions::default()
        };
        let focused = frame_scene(&solids, &objects, &opts, &pr).unwrap();

        // Camera fits the cube (half-diagonal ~8.7mm), not the slab (~141mm).
        assert!(focused.radius < 10.0, "focused radius {}", focused.radius);
        assert!(whole.radius > 100.0, "whole radius {}", whole.radius);
        let c = focused.center;
        assert!((c.x - 100.0).abs() < 1.0 && (c.y - 100.0).abs() < 1.0 && (c.z - 10.0).abs() < 1.0);
        // Floor stays at the slab's underside, not the cube's base (z = 5).
        assert!(focused.floor_z.abs() < 1e-6, "floor_z {}", focused.floor_z);
        assert!(whole.floor_z.abs() < 1e-6);
        // An unknown focus is an error, not a silent whole-scene frame.
        let bad = RasterOptions {
            focus: Some("nope".into()),
            ..RasterOptions::default()
        };
        assert!(frame_scene(&solids, &objects, &bad, &pr).is_err());
    }

    /// `--focus-dist` moves the plane of focus; without it the camera
    /// focuses on the framed subject (its distance to the target).
    #[test]
    fn focus_dist_sets_the_thin_lens_focus_plane() {
        let solids = crate::evaluate_vcad(&cube_doc()).unwrap();
        let mut pr = PhotorealOptions {
            aperture_frac: 0.05,
            ..PhotorealOptions::default()
        };
        let objects = build_objects(&solids, pr.mesh_segments()).unwrap();
        let opts = RasterOptions::default();
        let auto = frame_scene(&solids, &objects, &opts, &pr).unwrap();
        let to_target = (auto.camera.eye - auto.center).norm();
        assert!(
            (auto.camera.focus_dist - to_target).abs() < 0.5 * auto.radius,
            "default focus {} should sit on the subject (~{to_target})",
            auto.camera.focus_dist
        );
        assert!(auto.camera.aperture > 0.0);

        pr.focus_dist = Some(123.0);
        let set = frame_scene(&solids, &objects, &opts, &pr).unwrap();
        assert_eq!(set.camera.focus_dist, 123.0);

        pr.focus_dist = Some(-1.0);
        assert!(frame_scene(&solids, &objects, &opts, &pr).is_err());
    }

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

    /// A rounded-rectangle loft drawn as line segments, the way loon sketches
    /// a round (7.5° steps), is a stack of flat BRep strips. Mesh mode must
    /// hand the tracer one continuous shading normal across neighbouring
    /// strips, or the rounds render as jagged facets (the cortex head).
    #[test]
    fn polyline_loft_traces_with_continuous_normals() {
        use crate::{cross, sub};
        use vcad_kernel::vcad_kernel_math::{Point2, Point3, Vec3};
        use vcad_kernel::vcad_kernel_sketch::{SketchProfile, SketchSegment};

        let section = |z: f64, d: f64| {
            let (hx, hy, r) = (40.0 - d, 25.0 - d, 12.0);
            let mut pts: Vec<Point2> = Vec::new();
            for (cx, cy, a0) in [
                (hx - r, hy - r, 0.0),
                (r - hx, hy - r, 90.0),
                (r - hx, r - hy, 180.0),
                (hx - r, r - hy, 270.0),
            ] {
                for k in 0..=12 {
                    let a = (a0 + 7.5 * k as f64).to_radians();
                    pts.push(Point2::new(cx + r * a.cos(), cy + r * a.sin()));
                }
            }
            let segs = (0..pts.len())
                .map(|i| SketchSegment::Line {
                    start: pts[i],
                    end: pts[(i + 1) % pts.len()],
                })
                .collect();
            SketchProfile::new(Point3::new(0.0, 0.0, z), Vec3::x(), Vec3::y(), segs).unwrap()
        };
        let solid =
            vcad_kernel::Solid::loft(&[section(0.0, 0.0), section(30.0, 0.0)], Default::default())
                .expect("loft");
        let mesh = traced_mesh(&solid, MESH_SEGMENTS);

        let v = |i: usize| -> [f64; 3] { std::array::from_fn(|k| mesh.vertices[i * 3 + k] as f64) };
        let n = |i: usize| -> [f64; 3] { std::array::from_fn(|k| mesh.normals[i * 3 + k] as f64) };
        let key = |p: [f64; 3]| p.map(|c| (c * 1e3).round() as i64);
        let mut at: std::collections::HashMap<[i64; 3], Vec<usize>> = Default::default();
        for i in 0..mesh.vertices.len() / 3 {
            at.entry(key(v(i))).or_default().push(i);
        }

        // Shading normals at one point either agree (same smooth surface) or
        // differ by a real crease (wall vs cap, 90°). Anything in between is a
        // strip seam that would show as a facet edge.
        let (cos_same, cos_crease) = (0.5f64.to_radians().cos(), 45f64.to_radians().cos());
        let mut seams = 0;
        for ids in at.values() {
            for (a, &i) in ids.iter().enumerate() {
                for &j in &ids[a + 1..] {
                    let d = dot(n(i), n(j));
                    if d < cos_same && d > cos_crease {
                        seams += 1;
                    }
                }
            }
        }
        assert_eq!(
            seams, 0,
            "shading normals split between neighbouring loft strips"
        );

        // And they really are interpolated: a wall corner on a round must not
        // carry its strip's flat face normal (a 7.5° strip's corner averages
        // to 3.75° off it).
        let smoothed = (0..mesh.vertices.len() / 9).any(|t| {
            let [a, b, c] = [v(3 * t), v(3 * t + 1), v(3 * t + 2)];
            let face = normalize(cross(sub(b, a), sub(c, a)));
            face[2].abs() < 0.1 && (0..3).any(|k| dot(face, n(3 * t + k)) < 2f64.to_radians().cos())
        });
        assert!(smoothed, "loft walls traced with flat facet normals");

        // The flat caps keep their exact plane normal at every corner (#970 —
        // streaks on glass): a cap triangle must never pick up a smoothed
        // normal from a wall strip or a sliver neighbour. Checked on the plain
        // loft and on a visor-style one whose top edge is a 0.4 mm chamfer.
        let assert_flat_caps = |mesh: &vcad_kernel::vcad_kernel_tessellate::TriangleMesh,
                                top: f64| {
            let v =
                |i: usize| -> [f64; 3] { std::array::from_fn(|k| mesh.vertices[i * 3 + k] as f64) };
            let n =
                |i: usize| -> [f64; 3] { std::array::from_fn(|k| mesh.normals[i * 3 + k] as f64) };
            let mut cap_tris = 0;
            for t in 0..mesh.vertices.len() / 9 {
                let zs = [v(3 * t)[2], v(3 * t + 1)[2], v(3 * t + 2)[2]];
                let on = |z0: f64| zs.iter().all(|z| (z - z0).abs() < 1e-4);
                let want = if on(0.0) {
                    [0.0, 0.0, -1.0]
                } else if on(top) {
                    [0.0, 0.0, 1.0]
                } else {
                    continue;
                };
                cap_tris += 1;
                for k in 0..3 {
                    assert!(
                        dot(want, n(3 * t + k)) > 0.05f64.to_radians().cos(),
                        "cap triangle {t} corner {k} normal {:?} is not the cap normal {want:?}",
                        n(3 * t + k)
                    );
                }
            }
            assert!(cap_tris > 0, "loft produced no cap triangles");
            // And nothing beside a cap leans toward it: a wall or chamfer
            // corner on the top rim is a real crease (past the 30 degree crease angle off the
            // cap), not a normal half-smoothed into the cap.
            for i in 0..mesh.vertices.len() / 3 {
                let (p, nn) = (v(i), n(i));
                if (p[2] - top).abs() < 1e-4 {
                    let d = nn[2];
                    assert!(
                        d > 0.05f64.to_radians().cos() || d < 30f64.to_radians().cos(),
                        "rim corner at {p:?} has normal {nn:?} smoothed toward the cap"
                    );
                }
            }
        };
        assert_flat_caps(&mesh, 30.0);
        let visor = vcad_kernel::Solid::loft(
            &[section(0.0, 0.0), section(30.0, 0.0), section(30.4, 0.4)],
            Default::default(),
        )
        .expect("chamfered loft");
        assert_flat_caps(&traced_mesh(&visor, MESH_SEGMENTS), 30.4);
    }
}
