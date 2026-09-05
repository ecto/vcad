//! Physically-based unidirectional path tracer for direct BRep rendering.
//!
//! This is the "photoreal" sibling of [`crate::cpu`]'s headlight/studio
//! rasteriser. Where that module evaluates a hand-tuned lighting rig at the
//! primary hit and stops, this one solves the rendering equation by Monte
//! Carlo integration: multiple bounces, importance-sampled microfacet lobes,
//! multiple importance sampling against explicit area lights, and a physical
//! camera with a real aperture.
//!
//! Geometry is still traced analytically against the BRep — no tessellation
//! anywhere — so curved silhouettes and specular highlights on fillets are
//! exact at any resolution.
//!
//! # Design
//!
//! - **Lights** are intersectable rectangles ("softboxes"). Because they can
//!   be hit by a BSDF ray *and* sampled directly, both strategies combine
//!   under MIS with the power heuristic. That is what puts crisp, correctly
//!   shaped highlights on metal.
//! - **The environment** is, by default, a smooth analytic studio gradient.
//!   It is low-frequency by construction, so BSDF sampling alone converges
//!   quickly and no environment CDF is needed. An opt-in lat-long HDR image
//!   ([`EnvMap`]) is also supported; because a real HDRI carries windows and
//!   sun discs, that variant builds a `sin(theta)`-weighted 2D CDF and joins
//!   the MIS mix as a third sampling strategy.
//! - **The BSDF** is a layered metallic-roughness model: Lambert diffuse,
//!   a GGX specular lobe with VNDF sampling, and a GGX clearcoat lobe.
//!   Clearcoat is what sells anodised aluminium and moulded plastic.

use std::sync::Arc;
use vcad_kernel_math::{Point3, Transform, Vec3};

use crate::bvh::Bvh;
use crate::tlas::{Instance, Tlas};
use crate::Ray;

// ─── material ─────────────────────────────────────────────────────────────

/// A physically-based surface description.
///
/// Follows the metallic-roughness convention (glTF / Disney-lite) with an
/// added clearcoat layer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pbr {
    /// Linear-space base colour. Albedo for dielectrics, F0 for metals.
    pub base_color: [f32; 3],
    /// 0 = dielectric, 1 = metal.
    pub metallic: f32,
    /// Perceptual roughness in 0..1. Squared internally to get the GGX alpha.
    pub roughness: f32,
    /// Directional bias of the specular lobe, in -1..1.
    ///
    /// `0` is isotropic and reduces exactly to a round GGX highlight.
    /// Positive values stretch the highlight *along* the surface tangent
    /// (`dP/du`), negative values stretch it across. Because vcad shades the
    /// analytic BRep, that tangent is the real parameterisation of the
    /// surface — the circumferential direction on a cylinder — so a turned
    /// shaft or a bored hole gets the smeared highlight it has in life
    /// without any generated tangents or texture.
    pub anisotropy: f32,
    /// Strength of the clearcoat layer (0 = none, 1 = full).
    pub clearcoat: f32,
    /// Perceptual roughness of the clearcoat layer.
    pub clearcoat_roughness: f32,
    /// Dielectric index of refraction, drives the base specular reflectance.
    pub ior: f32,
    /// Linear emissive radiance.
    pub emissive: [f32; 3],
}

impl Default for Pbr {
    fn default() -> Self {
        Self {
            base_color: [0.62, 0.64, 0.67],
            metallic: 0.0,
            roughness: 0.4,
            anisotropy: 0.0,
            clearcoat: 0.0,
            clearcoat_roughness: 0.1,
            ior: 1.5,
            emissive: [0.0; 3],
        }
    }
}

impl Pbr {
    /// A metal: base colour is the specular reflectance at normal incidence.
    pub fn metal(base_color: [f32; 3], roughness: f32) -> Self {
        Self {
            base_color,
            metallic: 1.0,
            roughness,
            ..Default::default()
        }
    }

    /// A dielectric with an optional clearcoat layer.
    pub fn plastic(base_color: [f32; 3], roughness: f32, clearcoat: f32) -> Self {
        Self {
            base_color,
            metallic: 0.0,
            roughness,
            clearcoat,
            ..Default::default()
        }
    }

    /// A brushed or turned metal: the specular lobe is stretched along the
    /// surface's own tangent direction.
    ///
    /// `anisotropy` is signed — positive smears the highlight along `dP/du`
    /// (circumferentially on a cylinder, which is a turned finish), negative
    /// smears it across (an axially-brushed one).
    pub fn brushed_metal(base_color: [f32; 3], roughness: f32, anisotropy: f32) -> Self {
        Self {
            base_color,
            metallic: 1.0,
            roughness,
            anisotropy: anisotropy.clamp(-1.0, 1.0),
            ..Default::default()
        }
    }

    /// GGX alpha for the base specular lobe, ignoring anisotropy.
    #[inline]
    fn alpha(&self) -> f32 {
        (self.roughness * self.roughness).max(1e-4)
    }

    /// GGX alphas along the tangent and bitangent for the base specular
    /// lobe.
    ///
    /// Uses the standard Disney/glTF construction: an aspect ratio of
    /// `sqrt(1 - 0.9·|anisotropy|)` splits the isotropic alpha into a
    /// stretched and a squeezed axis while keeping their product — and hence
    /// the overall highlight area — roughly fixed. `0.9` bounds the extreme
    /// case away from a zero-width lobe.
    ///
    /// At `anisotropy == 0` the aspect is exactly `1`, so both alphas are
    /// bit-identical to [`Self::alpha`] and every anisotropic code path
    /// reduces to the isotropic one.
    #[inline]
    fn alpha_tb(&self) -> (f32, f32) {
        let a = self.alpha();
        let aniso = self.anisotropy.clamp(-1.0, 1.0);
        if aniso == 0.0 {
            return (a, a);
        }
        let aspect = (1.0 - 0.9 * aniso.abs()).sqrt();
        let (wide, narrow) = ((a / aspect).min(1.0), a * aspect);
        if aniso > 0.0 {
            (wide, narrow)
        } else {
            (narrow, wide)
        }
    }

    /// GGX alpha for the clearcoat lobe.
    #[inline]
    fn coat_alpha(&self) -> f32 {
        (self.clearcoat_roughness * self.clearcoat_roughness).max(1e-4)
    }

    /// Specular reflectance at normal incidence.
    #[inline]
    fn f0(&self) -> [f32; 3] {
        let d = ((self.ior - 1.0) / (self.ior + 1.0)).powi(2);
        [
            lerp(d, self.base_color[0], self.metallic),
            lerp(d, self.base_color[1], self.metallic),
            lerp(d, self.base_color[2], self.metallic),
        ]
    }

    /// Diffuse albedo (metals have none).
    #[inline]
    fn diffuse_albedo(&self) -> [f32; 3] {
        let k = 1.0 - self.metallic;
        [
            self.base_color[0] * k,
            self.base_color[1] * k,
            self.base_color[2] * k,
        ]
    }

    /// The colour to divide out before denoising, and multiply back after.
    ///
    /// Dielectrics carry their colour in the diffuse term; metals carry it in
    /// F0. Blending by `metallic` gives one buffer that tracks "what colour
    /// this surface is" for both, so the à-trous filter only ever sees the
    /// noisy illumination and never smears one part's colour into another's.
    #[inline]
    fn denoise_albedo(&self) -> [f32; 3] {
        mix3(self.diffuse_albedo(), self.f0(), self.metallic)
    }
}

/// Circumferential grain implied by a material's name, when the document does
/// not state anisotropy explicitly.
fn anisotropy_from_name(name: &str) -> f32 {
    let n = name.to_ascii_lowercase();
    // Turning and boring cut circumferentially, which is the +u direction on
    // a cylinder — the same direction the tangent frame is built from.
    if n.contains("turned") || n.contains("machined") || n.contains("bored") {
        0.6
    } else if n.contains("brushed") {
        0.7
    } else {
        0.0
    }
}

impl Pbr {
    /// Derive a render material from an IR material definition.
    ///
    /// Single source of truth for BOTH renderers: `vcad-render --photoreal`
    /// and the GPU viewport call this, so a part cannot pick up a different
    /// clearcoat, IOR or grain depending on which one drew it.
    pub fn from_material_def(mat: Option<&vcad_ir::MaterialDef>, tint: Option<[f64; 3]>) -> Self {
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

        // Anisotropy is a real IR field (`MaterialDef::anisotropy`) rather than
        // a rendering-time guess: Rust is the source of truth for IR types, the
        // value is a genuine property of the surface finish, and it round-trips
        // in `.vcad`. The name heuristic only fills in when the document says
        // nothing — a document that names its material "brushed_aluminum" or
        // "turned_shaft" has told us the finish, and rendering that as a uniform
        // polish is the CG tell this feature exists to remove. Anything explicit
        // always wins.
        let anisotropy = mat
            .and_then(|m| m.anisotropy)
            .map(|v| v as f32)
            .unwrap_or_else(|| mat.map(|m| anisotropy_from_name(&m.name)).unwrap_or(0.0))
            .clamp(-1.0, 1.0);

        Self {
            base_color,
            metallic,
            roughness,
            anisotropy,
            clearcoat,
            clearcoat_roughness: 0.08,
            ior,
            emissive: [0.0; 3],
        }
    }
}

// ─── lights & environment ─────────────────────────────────────────────────

/// A rectangular area light ("softbox"), emitting from its front face only.
///
/// Intersectable, so a BSDF ray that happens to land on it contributes and
/// combines with the explicit sample under MIS.
#[derive(Debug, Clone, Copy)]
pub struct AreaLight {
    /// Centre of the rectangle.
    pub center: Point3,
    /// Half-extent along the rectangle's first axis.
    pub u: Vec3,
    /// Half-extent along the rectangle's second axis.
    pub v: Vec3,
    /// Emitted radiance.
    pub emission: [f32; 3],
}

impl AreaLight {
    /// Unit normal of the emitting face.
    #[inline]
    fn normal(&self) -> Vec3 {
        self.u.cross(self.v).normalize()
    }

    /// Area of the rectangle in world units.
    #[inline]
    fn area(&self) -> f64 {
        4.0 * self.u.cross(self.v).norm()
    }

    /// Ray-rectangle intersection. Returns the hit distance if the ray
    /// strikes the emitting (front) face.
    fn intersect(&self, ray: &Ray) -> Option<f64> {
        let n = self.normal();
        let d = ray.direction.into_inner();
        let denom = n.dot(d);
        if denom.abs() < 1e-12 {
            return None;
        }
        let t = n.dot(self.center - ray.origin) / denom;
        if t <= 1e-6 {
            return None;
        }
        // Emitting face only: we must be looking at the front.
        if denom > 0.0 {
            return None;
        }
        let p = ray.at(t);
        let rel = p - self.center;
        let ul = self.u.norm();
        let vl = self.v.norm();
        let du = rel.dot(self.u / ul);
        let dv = rel.dot(self.v / vl);
        if du.abs() <= ul && dv.abs() <= vl {
            Some(t)
        } else {
            None
        }
    }

    /// Uniformly sample a point on the rectangle.
    fn sample(&self, r1: f64, r2: f64) -> Point3 {
        self.center + self.u * (2.0 * r1 - 1.0) + self.v * (2.0 * r2 - 1.0)
    }
}

/// Analytic studio environment: a smooth sky gradient plus a ground bounce.
///
/// Deliberately low-frequency — BSDF sampling alone integrates it cleanly, so
/// there is no environment importance-sampling CDF to build or maintain.
#[derive(Debug, Clone, Copy)]
pub struct GradientEnv {
    /// Radiance straight up.
    pub zenith: [f32; 3],
    /// Radiance at the horizon.
    pub horizon: [f32; 3],
    /// Radiance straight down (bounce off the studio floor).
    pub ground: [f32; 3],
    /// Overall multiplier.
    pub intensity: f32,
}

impl Default for GradientEnv {
    fn default() -> Self {
        Self {
            zenith: [0.34, 0.42, 0.55],
            horizon: [0.62, 0.64, 0.68],
            ground: [0.18, 0.17, 0.16],
            // The sky is ambient fill, not the key light — the softboxes
            // carry the image. Keeping this low is what preserves contrast.
            intensity: 0.35,
        }
    }
}

impl GradientEnv {
    /// Evaluate incoming radiance from direction `d` (world space, Z-up).
    fn radiance(&self, d: Vec3) -> [f32; 3] {
        let t = d.z as f32;
        let c = if t >= 0.0 {
            let k = smoothstep(t.powf(0.65));
            mix3(self.horizon, self.zenith, k)
        } else {
            let k = smoothstep((-t).powf(0.5));
            mix3(self.horizon, self.ground, k)
        };
        scale3(c, self.intensity)
    }
}

/// Relative luminance, the scalar the environment CDF is built over.
#[inline]
fn luminance(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// Sample a normalised piecewise-constant CDF (`cdf[0] == 0`, `cdf[n] == 1`).
///
/// Returns the chosen bin and the offset within it, so the caller can build a
/// continuous coordinate whose density is exactly the piecewise-constant one.
fn sample_1d(cdf: &[f32], u: f32) -> (usize, f32) {
    let n = cdf.len() - 1;
    let (mut lo, mut hi) = (0usize, n);
    while lo < hi {
        let mid = (lo + hi) / 2;
        if cdf[mid + 1] <= u {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    let i = lo.min(n - 1);
    let (a, b) = (cdf[i], cdf[i + 1]);
    let d = if b > a { (u - a) / (b - a) } else { 0.5 };
    (i, d.clamp(0.0, 1.0))
}

/// An [`EnvMap`] flattened for GPU upload. See [`EnvMap::pack_for_gpu`].
#[derive(Debug, Clone)]
pub struct GpuEnvPack {
    /// RGBA32F radiance, `width * height` texels.
    pub pixels: Vec<f32>,
    /// R32F CDF texture, `(width + 1) * (height + 1)`.
    pub cdf: Vec<f32>,
    /// Image width in texels.
    pub width: u32,
    /// Image height in texels.
    pub height: u32,
    /// Overall multiplier.
    pub intensity: f32,
    /// Rotation about +Z in radians.
    pub rotation: f32,
    /// PDF normaliser; zero means "not importance-sampled".
    pub marg_int: f32,
}

/// A lat-long (equirectangular) HDR environment map with a 2D CDF for
/// importance sampling.
///
/// Row 0 is the zenith (+Z) and the last row is nadir; column 0 is
/// `phi = rotation`. Unlike [`GradientEnv`] this can carry arbitrarily
/// high-frequency content — bright windows, a sun disc — so BSDF sampling
/// alone would be very noisy and importance sampling is mandatory.
///
/// The CDF is built over `luminance * sin(theta)`: the `sin(theta)` factor is
/// the lat-long solid-angle Jacobian, and omitting it over-weights the poles,
/// where texels cover almost no solid angle.
#[derive(Debug, Clone)]
pub struct EnvMap {
    width: usize,
    height: usize,
    /// Row-major linear radiance, `width * height` texels.
    pixels: Vec<[f32; 3]>,
    intensity: f32,
    /// Rotation about +Z in radians, applied when mapping u to phi.
    rotation: f64,
    /// Per-row conditional CDF over u, `height * (width + 1)` entries.
    cond_cdf: Vec<f32>,
    /// Marginal CDF over v, `height + 1` entries.
    marg_cdf: Vec<f32>,
    /// Mean of the weighted function; the normaliser for the uv-space PDF.
    marg_int: f32,
}

impl EnvMap {
    /// Build a map from row-major linear-RGB texels (row 0 = zenith).
    pub fn new(width: usize, height: usize, pixels: Vec<[f32; 3]>) -> Result<Self, String> {
        if width == 0 || height == 0 {
            return Err("environment map must have non-zero dimensions".to_string());
        }
        if pixels.len() != width * height {
            return Err(format!(
                "environment map has {} texels, expected {}x{} = {}",
                pixels.len(),
                width,
                height,
                width * height
            ));
        }

        let mut cond_cdf = vec![0.0f32; height * (width + 1)];
        let mut row_int = vec![0.0f32; height];
        for j in 0..height {
            // sin(theta) at the row's centre — the solid-angle weight.
            let sin_t = (std::f64::consts::PI * (j as f64 + 0.5) / height as f64).sin() as f32;
            let base = j * (width + 1);
            let mut acc = 0.0f32;
            for i in 0..width {
                acc += luminance(pixels[j * width + i]).max(0.0) * sin_t;
                cond_cdf[base + i + 1] = acc;
            }
            row_int[j] = acc / width as f32;
            if acc > 0.0 {
                for i in 1..=width {
                    cond_cdf[base + i] /= acc;
                }
            } else {
                // A black row is never selected by the marginal; keep its CDF
                // well-formed anyway so sampling can't index out of range.
                for i in 0..=width {
                    cond_cdf[base + i] = i as f32 / width as f32;
                }
            }
        }

        let mut marg_cdf = vec![0.0f32; height + 1];
        let mut acc = 0.0f32;
        for j in 0..height {
            acc += row_int[j];
            marg_cdf[j + 1] = acc;
        }
        let marg_int = acc / height as f32;
        if acc > 0.0 {
            for c in marg_cdf.iter_mut().skip(1) {
                *c /= acc;
            }
        } else {
            for (j, c) in marg_cdf.iter_mut().enumerate() {
                *c = j as f32 / height as f32;
            }
        }

        Ok(Self {
            width,
            height,
            pixels,
            intensity: 1.0,
            rotation: 0.0,
            cond_cdf,
            marg_cdf,
            marg_int,
        })
    }

    /// Scale every sample by `k`.
    pub fn with_intensity(mut self, k: f32) -> Self {
        self.intensity = k;
        self
    }

    /// Spin the environment about the vertical axis by `deg` degrees.
    ///
    /// A rigid rotation in phi leaves the CDF valid as-is: it is built in
    /// image space and only the image-to-direction mapping moves.
    pub fn with_rotation_deg(mut self, deg: f64) -> Self {
        self.rotation = deg.to_radians();
        self
    }

    /// Whether the map carries any energy to importance-sample.
    #[inline]
    fn is_sampleable(&self) -> bool {
        self.marg_int > 0.0
    }

    /// Image coordinates in `[0, 1)^2` for a world direction.
    fn uv(&self, d: Vec3) -> (f64, f64) {
        const EPS: f64 = 1e-9;
        let theta = d.z.clamp(-1.0, 1.0).acos();
        let v = (theta / std::f64::consts::PI).clamp(0.0, 1.0 - EPS);
        let phi = (d.y.atan2(d.x) - self.rotation).rem_euclid(std::f64::consts::TAU);
        let u = (phi / std::f64::consts::TAU).clamp(0.0, 1.0 - EPS);
        (u, v)
    }

    /// World direction for image coordinates in `[0, 1]^2`.
    fn direction(&self, u: f64, v: f64) -> Vec3 {
        let phi = u * std::f64::consts::TAU + self.rotation;
        let theta = v * std::f64::consts::PI;
        let (st, ct) = theta.sin_cos();
        Vec3::new(st * phi.cos(), st * phi.sin(), ct)
    }

    /// Texel indices for image coordinates.
    #[inline]
    fn texel_index(&self, u: f64, v: f64) -> (usize, usize) {
        let i = ((u * self.width as f64) as usize).min(self.width - 1);
        let j = ((v * self.height as f64) as usize).min(self.height - 1);
        (i, j)
    }

    /// Incoming radiance from direction `d`.
    ///
    /// Nearest-texel, deliberately: the CDF is piecewise-constant per texel,
    /// so a nearest lookup makes the sampled radiance and the PDF describe
    /// exactly the same function, which is what MIS requires.
    pub fn radiance(&self, d: Vec3) -> [f32; 3] {
        let (u, v) = self.uv(d);
        let (i, j) = self.texel_index(u, v);
        scale3(self.pixels[j * self.width + i], self.intensity)
    }

    /// Solid-angle PDF of the environment sampling strategy for direction `d`.
    ///
    /// The uv-space density converts with `dω = 2·π² · sin(θ) · du dv`, since
    /// `u` spans `2π` of azimuth and `v` spans `π` of polar angle.
    pub fn pdf(&self, d: Vec3) -> f32 {
        if !self.is_sampleable() {
            return 0.0;
        }
        let (u, v) = self.uv(d);
        let (i, j) = self.texel_index(u, v);
        let sin_bin = (std::f64::consts::PI * (j as f64 + 0.5) / self.height as f64).sin() as f32;
        let f = luminance(self.pixels[j * self.width + i]).max(0.0) * sin_bin;
        if f <= 0.0 {
            return 0.0;
        }
        let pdf_uv = f / self.marg_int;
        // Actual sin(theta) of this direction, not the bin's — the Jacobian
        // is a property of the point, while the bin weight is importance.
        let sin_t = (1.0 - d.z * d.z).max(0.0).sqrt() as f32;
        if sin_t <= 1e-9 {
            return 0.0;
        }
        let two_pi_sq = 2.0 * std::f32::consts::PI * std::f32::consts::PI;
        pdf_uv / (two_pi_sq * sin_t)
    }

    /// Pack for GPU upload as two textures — see `env.wgsl` for why textures
    /// rather than storage buffers.
    ///
    /// `pixels` is RGBA32F (`w * h`); `cdf` is R32F (`(w+1) * (h+1)`) with row
    /// `j < h` the conditional CDF for row `j` and row `h` the marginal.
    ///
    /// Lives here, next to where the CDFs are built, so the two descriptions of
    /// the layout cannot drift apart.
    pub fn pack_for_gpu(&self) -> GpuEnvPack {
        let (w, h) = (self.width, self.height);

        let mut pixels = Vec::with_capacity(4 * w * h);
        for px in &self.pixels {
            pixels.extend_from_slice(px);
            pixels.push(1.0);
        }

        // (w + 1) x (h + 1), zero-filled: each conditional row uses the full
        // width, the marginal uses only its first h + 1 entries.
        let mut cdf = vec![0.0f32; (w + 1) * (h + 1)];
        for j in 0..h {
            let base = j * (w + 1);
            cdf[base..base + w + 1].copy_from_slice(&self.cond_cdf[base..base + w + 1]);
        }
        let marg_row = h * (w + 1);
        cdf[marg_row..marg_row + self.marg_cdf.len()].copy_from_slice(&self.marg_cdf);

        GpuEnvPack {
            pixels,
            cdf,
            width: self.width as u32,
            height: self.height as u32,
            intensity: self.intensity,
            rotation: self.rotation as f32,
            // Zero when the map carries no energy, which switches the shader's
            // importance sampling off exactly as `is_sampleable` does here.
            marg_int: if self.is_sampleable() {
                self.marg_int
            } else {
                0.0
            },
        }
    }

    /// Importance-sample a direction. Returns `(direction, radiance, pdf)`
    /// with the PDF measured in solid angle.
    pub fn sample(&self, r1: f64, r2: f64) -> Option<(Vec3, [f32; 3], f32)> {
        if !self.is_sampleable() {
            return None;
        }
        let (j, dv) = sample_1d(&self.marg_cdf, r2 as f32);
        let base = j * (self.width + 1);
        let (i, du) = sample_1d(&self.cond_cdf[base..base + self.width + 1], r1 as f32);
        let u = (i as f64 + du as f64) / self.width as f64;
        let v = (j as f64 + dv as f64) / self.height as f64;
        let d = self.direction(u, v);
        // Recompute through `pdf` so the MIS partner and this sample agree
        // bit-for-bit on the density.
        let pdf = self.pdf(d);
        if pdf <= 0.0 || !pdf.is_finite() {
            return None;
        }
        Some((
            d,
            scale3(self.pixels[j * self.width + i], self.intensity),
            pdf,
        ))
    }
}

/// Incoming light from infinity.
///
/// The analytic [`GradientEnv`] is the default: fast, dependency-free, ships
/// no asset, and genuinely good for neutral product renders. [`EnvMap`] is
/// opt-in and brings the CDF machinery with it.
#[derive(Debug, Clone)]
pub enum Environment {
    /// Smooth analytic studio gradient. Sampled by the BSDF alone.
    Gradient(GradientEnv),
    /// Lat-long HDR image. Joins MIS as a third sampling strategy.
    Image(Box<EnvMap>),
}

impl Default for Environment {
    fn default() -> Self {
        Environment::Gradient(GradientEnv::default())
    }
}

impl Environment {
    /// A uniform environment of constant radiance — the white-furnace case.
    pub fn constant(rgb: [f32; 3]) -> Self {
        Environment::Gradient(GradientEnv {
            zenith: rgb,
            horizon: rgb,
            ground: rgb,
            intensity: 1.0,
        })
    }

    /// Wrap a lat-long HDR map.
    pub fn image(map: EnvMap) -> Self {
        Environment::Image(Box::new(map))
    }

    /// Evaluate incoming radiance from direction `d` (world space, Z-up).
    fn radiance(&self, d: Vec3) -> [f32; 3] {
        match self {
            Environment::Gradient(g) => g.radiance(d),
            Environment::Image(m) => m.radiance(d),
        }
    }

    /// Whether this environment participates in MIS as its own strategy.
    #[inline]
    fn is_importance_sampled(&self) -> bool {
        match self {
            Environment::Gradient(_) => false,
            Environment::Image(m) => m.is_sampleable(),
        }
    }

    /// Solid-angle PDF of the environment sampling strategy, or 0 when this
    /// environment is not importance-sampled.
    #[inline]
    fn pdf(&self, d: Vec3) -> f32 {
        match self {
            Environment::Gradient(_) => 0.0,
            Environment::Image(m) => m.pdf(d),
        }
    }

    /// Importance-sample a direction, if this environment supports it.
    #[inline]
    fn sample(&self, r1: f64, r2: f64) -> Option<(Vec3, [f32; 3], f32)> {
        match self {
            Environment::Gradient(_) => None,
            Environment::Image(m) => m.sample(r1, r2),
        }
    }
}

/// An infinite ground plane at a fixed Z, used as a studio sweep.
#[derive(Debug, Clone, Copy)]
pub struct Ground {
    /// Height of the plane.
    pub z: f64,
    /// Surface description.
    pub material: Pbr,
    /// When true the plane contributes only shadowing and contact darkening
    /// to alpha, so the render composites cleanly onto any backdrop.
    pub shadow_catcher: bool,
}

// ─── scene & camera ───────────────────────────────────────────────────────

/// A traceable object: one BVH over a BRep solid plus its material.
pub struct Object {
    /// Acceleration structure over the solid's analytic faces.
    pub bvh: Arc<Bvh>,
    /// Surface description.
    pub material: Pbr,
    /// Object → world placement of the BVH, which is otherwise traced
    /// wherever it was built.
    ///
    /// Most callers bake placement into the geometry and leave this at the
    /// identity. An animation instead holds the geometry (and its BVH) still
    /// and moves this, so a jointed assembly re-poses with no re-evaluation
    /// and no BLAS rebuild — only the top-level structure is rebuilt.
    pub transform: Transform,
}

impl Object {
    /// A traceable object placed where its BVH was built.
    pub fn new(bvh: Arc<Bvh>, material: Pbr) -> Self {
        Self {
            bvh,
            material,
            transform: Transform::identity(),
        }
    }

    /// A traceable object placed by an object→world transform.
    pub fn placed(bvh: Arc<Bvh>, material: Pbr, transform: Transform) -> Self {
        Self {
            bvh,
            material,
            transform,
        }
    }
}

/// Everything the integrator needs to render a frame.
pub struct Scene {
    /// Traceable BRep objects.
    pub objects: Vec<Object>,
    /// Explicit area lights.
    pub lights: Vec<AreaLight>,
    /// Analytic sky, or a lat-long HDR environment map.
    pub env: Environment,
    /// Optional studio floor.
    pub ground: Option<Ground>,
}

/// A physical camera. Perspective with a real aperture, or orthographic for
/// drafting-style framing.
///
/// The screen basis is stored explicitly rather than derived from an
/// up-hint. Callers that already have a projection basis (a CAD view matrix,
/// say) can hand it over verbatim with [`Camera::from_basis`] and get pixel
/// alignment with their existing renderer — including bases that are
/// mirrored, which a `look_at` construction cannot reproduce.
#[derive(Debug, Clone, Copy)]
pub struct Camera {
    /// Eye position.
    pub eye: Point3,
    /// Unit direction the camera looks along.
    pub forward: Vec3,
    /// Unit world direction mapping to screen +x.
    pub right: Vec3,
    /// Unit world direction mapping to screen +y (up).
    pub up: Vec3,
    /// Vertical field of view in degrees (perspective only).
    pub fov_deg: f64,
    /// Aperture *radius* in world units. Zero gives a pinhole.
    pub aperture: f64,
    /// Distance to the plane of exact focus.
    pub focus_dist: f64,
    /// When set, render orthographically with this half-height instead.
    pub ortho_half_height: Option<f64>,
}

impl Camera {
    /// Conventional right-handed camera aimed at `target`.
    pub fn look_at(eye: Point3, target: Point3, up_hint: Vec3, fov_deg: f64) -> Self {
        let forward = (target - eye).normalize();
        let right = forward.cross(up_hint).normalize();
        let up = right.cross(forward).normalize();
        Self {
            eye,
            forward,
            right,
            up,
            fov_deg,
            aperture: 0.0,
            focus_dist: (target - eye).norm(),
            ortho_half_height: None,
        }
    }

    /// Build from an explicit screen basis. Vectors are normalised but
    /// otherwise used as given, so a mirrored basis stays mirrored.
    pub fn from_basis(
        eye: Point3,
        forward: Vec3,
        right: Vec3,
        up: Vec3,
        fov_deg: f64,
        focus_dist: f64,
    ) -> Self {
        Self {
            eye,
            forward: forward.normalize(),
            right: right.normalize(),
            up: up.normalize(),
            fov_deg,
            aperture: 0.0,
            focus_dist,
            ortho_half_height: None,
        }
    }

    /// Generate a primary ray through normalised screen coords in [-1, 1],
    /// with `(lu, lv)` a uniform sample on the unit disc for lens defocus.
    fn ray(&self, sx: f64, sy: f64, aspect: f64, lu: f64, lv: f64) -> Ray {
        let (fwd, right, up) = (self.forward, self.right, self.up);

        if let Some(hh) = self.ortho_half_height {
            let hw = hh * aspect;
            let origin = self.eye + right * (sx * hw) + up * (sy * hh);
            return Ray::new(origin, fwd);
        }

        let half_h = (self.fov_deg.to_radians() * 0.5).tan();
        let half_w = half_h * aspect;

        // Point on the focal plane this pixel maps to.
        let dir = fwd + right * (sx * half_w) + up * (sy * half_h);
        let focal_point = self.eye + dir * self.focus_dist;

        if self.aperture <= 0.0 {
            return Ray::new(self.eye, focal_point - self.eye);
        }
        let offset = right * (lu * self.aperture) + up * (lv * self.aperture);
        let origin = self.eye + offset;
        Ray::new(origin, focal_point - origin)
    }
}

/// Integrator settings.
#[derive(Debug, Clone, Copy)]
pub struct PathTraceOptions {
    /// Samples per pixel.
    pub spp: u32,
    /// Maximum path length (1 = direct lighting only).
    pub max_depth: u32,
    /// Depth at which Russian roulette begins.
    pub rr_start: u32,
    /// Clamp on indirect radiance, to kill fireflies. `None` disables.
    pub firefly_clamp: Option<f32>,
    /// Render the environment behind the subject rather than leaving it clear.
    pub show_background: bool,
    /// Random seed.
    pub seed: u64,
    /// Run the edge-aware à-trous denoiser over the film before returning.
    ///
    /// This is a pure post-process on the accumulated radiance — it consumes
    /// no random numbers and cannot change the integrator's estimate.
    pub denoise: bool,
    /// À-trous iterations. Each doubles the tap stride, so `n` iterations
    /// reach a footprint of roughly `2^(n+1)` pixels.
    pub denoise_iters: u32,
    /// Edge-stopping tolerance on the world normal, as `‖n_p − n_q‖`.
    /// Smaller keeps creases sharper and denoises less.
    pub sigma_normal: f32,
    /// Edge-stopping tolerance on hit distance, *relative* to the centre
    /// pixel's depth and scaled by the tap stride (so grazing surfaces still
    /// filter).
    pub sigma_depth: f32,
    /// Edge-stopping tolerance on demodulated illumination luminance. Halved
    /// each iteration, per Dammertz, so late wide passes cannot flatten
    /// detail the early passes already resolved.
    pub sigma_lum: f32,
}

impl Default for PathTraceOptions {
    fn default() -> Self {
        Self {
            spp: 128,
            max_depth: 6,
            rr_start: 3,
            firefly_clamp: Some(12.0),
            show_background: true,
            seed: 0x5eed_1234,
            denoise: true,
            denoise_iters: 5,
            sigma_normal: 0.35,
            sigma_depth: 0.02,
            sigma_lum: 4.0,
        }
    }
}

// ─── rng ──────────────────────────────────────────────────────────────────

/// Small, fast, deterministic PRNG (PCG-XSH-RR style).
#[derive(Clone, Copy)]
struct Rng(u64);

impl Rng {
    #[inline]
    fn new(seed: u64) -> Self {
        // Mix so neighbouring pixel seeds decorrelate immediately.
        let mut s = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        s ^= s >> 29;
        s = s.wrapping_mul(0xBF58_476D_1CE4_E5B9);
        s ^= s >> 32;
        Rng(s | 1)
    }

    #[inline]
    fn next_u32(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let x = (((self.0 >> 18) ^ self.0) >> 27) as u32;
        let rot = (self.0 >> 59) as u32;
        x.rotate_right(rot)
    }

    /// Uniform in [0, 1).
    #[inline]
    fn f64(&mut self) -> f64 {
        (self.next_u32() as f64) * (1.0 / 4294967296.0)
    }
}

// ─── small math helpers ───────────────────────────────────────────────────

#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

#[inline]
fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        lerp(a[0], b[0], t),
        lerp(a[1], b[1], t),
        lerp(a[2], b[2], t),
    ]
}

#[inline]
fn scale3(a: [f32; 3], k: f32) -> [f32; 3] {
    [a[0] * k, a[1] * k, a[2] * k]
}

#[inline]
fn mul3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] * b[0], a[1] * b[1], a[2] * b[2]]
}

#[inline]
fn add3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

#[inline]
fn max3(a: [f32; 3]) -> f32 {
    a[0].max(a[1]).max(a[2])
}

#[inline]
fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Orthonormal basis around a unit normal (Duff et al., branchless).
fn onb(n: Vec3) -> (Vec3, Vec3) {
    let sign = if n.z >= 0.0 { 1.0 } else { -1.0 };
    let a = -1.0 / (sign + n.z);
    let b = n.x * n.y * a;
    (
        Vec3::new(1.0 + sign * n.x * n.x * a, sign * b, -sign * n.x),
        Vec3::new(b, sign + n.y * n.y * a, -n.y),
    )
}

/// An orthonormal shading frame: tangent, bitangent, normal.
///
/// The tangent is meaningful, not arbitrary, whenever the hit surface has a
/// real parameterisation — that is what anisotropic shading orients itself
/// by — so the three vectors travel together rather than as loose arguments.
#[derive(Debug, Clone, Copy)]
struct Frame {
    t: Vec3,
    b: Vec3,
    n: Vec3,
}

/// Shading tangent frame around a unit normal.
///
/// When the hit carried a surface tangent `dP/du`, it is Gram-Schmidt
/// orthogonalised against the (possibly face-forwarded) shading normal and
/// used as the frame's x axis, so the anisotropic lobe lines up with the
/// surface's own parameterisation. Otherwise this is the arbitrary [`onb`]
/// basis the isotropic path has always used — which is exactly what an
/// isotropic material wants, since its BSDF is invariant to the choice.
fn shading_frame(n: Vec3, dpdu: Option<Vec3>) -> Frame {
    if let Some(d) = dpdu {
        let t = d - n * d.dot(n);
        // A tangent that is (numerically) parallel to the normal carries no
        // direction; fall back rather than normalising noise.
        if t.norm() > 1e-9 {
            let t = t.normalize();
            return Frame {
                t,
                b: n.cross(t),
                n,
            };
        }
    }
    let (t, b) = onb(n);
    Frame { t, b, n }
}

#[inline]
fn to_local(t: Vec3, b: Vec3, n: Vec3, w: Vec3) -> Vec3 {
    Vec3::new(w.dot(t), w.dot(b), w.dot(n))
}

#[inline]
fn to_world(t: Vec3, b: Vec3, n: Vec3, w: Vec3) -> Vec3 {
    t * w.x + b * w.y + n * w.z
}

/// Cosine-weighted hemisphere sample in local space (+Z up).
fn cosine_hemisphere(r1: f64, r2: f64) -> Vec3 {
    let r = r1.sqrt();
    let phi = 2.0 * std::f64::consts::PI * r2;
    Vec3::new(r * phi.cos(), r * phi.sin(), (1.0 - r1).max(0.0).sqrt())
}

/// Uniform sample on the unit disc (concentric mapping).
fn concentric_disc(r1: f64, r2: f64) -> (f64, f64) {
    let a = 2.0 * r1 - 1.0;
    let b = 2.0 * r2 - 1.0;
    if a == 0.0 && b == 0.0 {
        return (0.0, 0.0);
    }
    let (r, theta) = if a * a > b * b {
        (a, std::f64::consts::FRAC_PI_4 * (b / a))
    } else {
        (
            b,
            std::f64::consts::FRAC_PI_2 - std::f64::consts::FRAC_PI_4 * (a / b),
        )
    };
    (r * theta.cos(), r * theta.sin())
}

// ─── microfacet BRDF ──────────────────────────────────────────────────────

/// GGX / Trowbridge-Reitz normal distribution.
#[inline]
fn d_ggx(n_dot_h: f32, alpha: f32) -> f32 {
    let a2 = alpha * alpha;
    let d = n_dot_h * n_dot_h * (a2 - 1.0) + 1.0;
    a2 / (std::f32::consts::PI * d * d).max(1e-9)
}

/// Anisotropic GGX normal distribution.
///
/// `wh` is the half-vector in the local shading frame, whose x axis is the
/// surface tangent. When `at == ab` this is algebraically identical to
/// [`d_ggx`]; the equality is made exact (not merely near-exact in floating
/// point) by dispatching to it.
#[inline]
fn d_ggx_aniso(wh: Vec3, at: f32, ab: f32) -> f32 {
    if at == ab {
        return d_ggx(wh.z.max(0.0) as f32, at);
    }
    let (hx, hy, hz) = (wh.x as f32, wh.y as f32, wh.z as f32);
    let d = (hx / at) * (hx / at) + (hy / ab) * (hy / ab) + hz * hz;
    1.0 / (std::f32::consts::PI * at * ab * d * d).max(1e-9)
}

/// Smith height-correlated visibility term (already divided by 4·NoL·NoV).
#[inline]
fn v_smith(n_dot_v: f32, n_dot_l: f32, alpha: f32) -> f32 {
    let a2 = alpha * alpha;
    let gv = n_dot_l * (n_dot_v * n_dot_v * (1.0 - a2) + a2).sqrt();
    let gl = n_dot_v * (n_dot_l * n_dot_l * (1.0 - a2) + a2).sqrt();
    0.5 / (gv + gl).max(1e-9)
}

/// Anisotropic Smith height-correlated visibility term (already divided by
/// 4·NoL·NoV). Reduces exactly to [`v_smith`] when `at == ab`.
#[inline]
fn v_smith_aniso(wo: Vec3, wi: Vec3, at: f32, ab: f32) -> f32 {
    if at == ab {
        return v_smith(wo.z as f32, wi.z as f32, at);
    }
    // Λ-style stretched lengths: sqrt((at·x)² + (ab·y)² + z²).
    let stretched = |w: Vec3| -> f32 {
        let (x, y, z) = (w.x as f32, w.y as f32, w.z as f32);
        ((at * x) * (at * x) + (ab * y) * (ab * y) + z * z).sqrt()
    };
    let gv = (wi.z as f32) * stretched(wo);
    let gl = (wo.z as f32) * stretched(wi);
    0.5 / (gv + gl).max(1e-9)
}

/// Smith G1 masking term for the anisotropic GGX distribution.
///
/// The `at == ab` branch is the isotropic expression evaluated exactly as it
/// was before anisotropy existed, so isotropic renders are bit-unchanged.
#[inline]
fn g1_smith_aniso(w: Vec3, at: f32, ab: f32) -> f32 {
    let z = w.z.max(1e-6) as f32;
    let lambda = if at == ab {
        let a2 = at * at;
        ((1.0 + a2 * (1.0 - z * z) / (z * z)).sqrt() - 1.0) * 0.5
    } else {
        let (x, y) = (w.x as f32, w.y as f32);
        (((at * x) * (at * x) + (ab * y) * (ab * y) + z * z).sqrt() / z - 1.0) * 0.5
    };
    1.0 / (1.0 + lambda)
}

/// Schlick Fresnel.
#[inline]
fn fresnel(f0: [f32; 3], cos_theta: f32) -> [f32; 3] {
    let m = (1.0 - cos_theta).clamp(0.0, 1.0).powi(5);
    [
        f0[0] + (1.0 - f0[0]) * m,
        f0[1] + (1.0 - f0[1]) * m,
        f0[2] + (1.0 - f0[2]) * m,
    ]
}

/// Sample the GGX visible-normal distribution (Heitz 2018). `wo` is in local
/// space with +Z the shading normal and +X the surface tangent; returns the
/// sampled half-vector.
///
/// The method is anisotropic by construction: the stretch step that maps to
/// the hemisphere-configured space takes each axis' alpha separately, so
/// passing `at != ab` needs no other change.
fn sample_vndf(wo: Vec3, at: f32, ab: f32, r1: f64, r2: f64) -> Vec3 {
    let (a, b) = (at as f64, ab as f64);
    // Stretch the view direction into the hemisphere-configured space.
    let vh = Vec3::new(a * wo.x, b * wo.y, wo.z).normalize();
    let lensq = vh.x * vh.x + vh.y * vh.y;
    let t1 = if lensq > 0.0 {
        Vec3::new(-vh.y, vh.x, 0.0) / lensq.sqrt()
    } else {
        Vec3::new(1.0, 0.0, 0.0)
    };
    let t2 = vh.cross(t1);

    let r = r1.sqrt();
    let phi = 2.0 * std::f64::consts::PI * r2;
    let p1 = r * phi.cos();
    let p2r = r * phi.sin();
    let s = 0.5 * (1.0 + vh.z);
    let p2 = (1.0 - s) * (1.0 - p1 * p1).max(0.0).sqrt() + s * p2r;

    let nh = t1 * p1 + t2 * p2 + vh * (1.0 - p1 * p1 - p2 * p2).max(0.0).sqrt();
    Vec3::new(a * nh.x, b * nh.y, nh.z.max(1e-9)).normalize()
}

/// PDF of the VNDF sampling strategy, in solid angle around `wi`.
#[inline]
fn vndf_pdf(wo: Vec3, wh: Vec3, at: f32, ab: f32) -> f32 {
    let n_dot_v = wo.z.max(1e-6) as f32;
    let d = d_ggx_aniso(wh, at, ab);
    let g1 = g1_smith_aniso(wo, at, ab);
    let o_dot_h = wo.dot(wh).max(1e-9) as f32;
    d * g1 * o_dot_h / n_dot_v / (4.0 * o_dot_h)
}

/// Relative sampling weights of the three lobes for a given material.
fn lobe_weights(m: &Pbr) -> (f32, f32, f32) {
    let diff = max3(m.diffuse_albedo()).max(0.0);
    let spec = max3(m.f0()).max(0.0) + 0.08;
    let coat = m.clearcoat * 0.25;
    let total = (diff + spec + coat).max(1e-6);
    (diff / total, spec / total, coat / total)
}

/// Evaluate the full BSDF and its sampling PDF for a given in/out pair.
///
/// Both vectors are in the local shading frame (+Z = normal) and point away
/// from the surface. Returns `(f * cos, pdf)`.
fn bsdf_eval(m: &Pbr, wo: Vec3, wi: Vec3) -> ([f32; 3], f32) {
    if wi.z <= 0.0 || wo.z <= 0.0 {
        return ([0.0; 3], 0.0);
    }
    let n_dot_l = wi.z as f32;
    let n_dot_v = wo.z as f32;
    let wh = (wo + wi).normalize();
    let n_dot_h = wh.z.max(0.0) as f32;
    let o_dot_h = wo.dot(wh).max(0.0) as f32;

    let (pd, ps, pc) = lobe_weights(m);

    // Diffuse.
    let diffuse = scale3(m.diffuse_albedo(), std::f32::consts::FRAC_1_PI * n_dot_l);
    let pdf_d = n_dot_l * std::f32::consts::FRAC_1_PI;

    // Base specular. Anisotropy stretches the lobe along the local x axis,
    // which the integrator has aligned with the surface tangent dP/du.
    let (at, ab) = m.alpha_tb();
    let d = d_ggx_aniso(wh, at, ab);
    let vis = v_smith_aniso(wo, wi, at, ab);
    let f = fresnel(m.f0(), o_dot_h);
    let spec = scale3(f, d * vis * n_dot_l);
    let pdf_s = vndf_pdf(wo, wh, at, ab);

    // Clearcoat: a thin dielectric layer over everything else. It is a
    // separate isotropic film — the grain lives in the substrate beneath it,
    // not in the lacquer — so it never takes the anisotropy.
    let (coat, pdf_c, coat_atten) = if m.clearcoat > 0.0 {
        let ca = m.coat_alpha();
        let cd = d_ggx(n_dot_h, ca);
        let cv = v_smith(n_dot_v, n_dot_l, ca);
        let cf = fresnel([0.04, 0.04, 0.04], o_dot_h)[0] * m.clearcoat;
        let c = cd * cv * n_dot_l * cf;
        (
            [c, c, c],
            vndf_pdf(wo, wh, ca, ca),
            // Energy removed from the layers beneath.
            1.0 - cf,
        )
    } else {
        ([0.0; 3], 0.0, 1.0)
    };

    let under = add3(diffuse, spec);
    let value = add3(scale3(under, coat_atten), coat);
    let pdf = pd * pdf_d + ps * pdf_s + pc * pdf_c;
    (value, pdf.max(0.0))
}

/// Reference BSDF evaluation, in the local shading frame (+Z = normal).
///
/// Exposed so the WGSL port in `gpu/shaders/bsdf.wgsl` can be checked against
/// this implementation — see `tests/bsdf_parity.rs`. Returns `(f * cos, pdf)`;
/// the PDF is the one MIS must agree on across both renderers.
pub fn reference_bsdf_eval(m: &Pbr, wo: Vec3, wi: Vec3) -> ([f32; 3], f32) {
    bsdf_eval(m, wo, wi)
}

/// Importance-sample the BSDF. Returns `(wi_local, f*cos, pdf)`.
fn bsdf_sample(m: &Pbr, wo: Vec3, rng: &mut Rng) -> Option<(Vec3, [f32; 3], f32)> {
    if wo.z <= 0.0 {
        return None;
    }
    let (pd, ps, _pc) = lobe_weights(m);
    let u = rng.f64() as f32;
    let r1 = rng.f64();
    let r2 = rng.f64();

    let wi = if u < pd {
        cosine_hemisphere(r1, r2)
    } else if u < pd + ps {
        let (at, ab) = m.alpha_tb();
        let wh = sample_vndf(wo, at, ab, r1, r2);
        let wi = reflect(-wo, wh);
        if wi.z <= 0.0 {
            return None;
        }
        wi
    } else {
        let ca = m.coat_alpha();
        let wh = sample_vndf(wo, ca, ca, r1, r2);
        let wi = reflect(-wo, wh);
        if wi.z <= 0.0 {
            return None;
        }
        wi
    };

    let (f, pdf) = bsdf_eval(m, wo, wi);
    if pdf <= 1e-9 {
        return None;
    }
    Some((wi, f, pdf))
}

#[inline]
fn reflect(i: Vec3, n: Vec3) -> Vec3 {
    i - n * (2.0 * i.dot(n))
}

/// Power heuristic (β = 2) for multiple importance sampling.
#[inline]
fn power_heuristic(a: f32, b: f32) -> f32 {
    let a2 = a * a;
    let b2 = b * b;
    if a2 + b2 <= 0.0 {
        0.0
    } else {
        a2 / (a2 + b2)
    }
}

// ─── intersection ─────────────────────────────────────────────────────────

/// What a ray landed on.
enum Landing {
    Surface {
        point: Point3,
        normal: Vec3,
        /// Surface tangent dP/du, when the parameterisation has one.
        tangent: Option<Vec3>,
        material: Pbr,
    },
    Light {
        emission: [f32; 3],
        light_index: usize,
        distance: f64,
        point: Point3,
    },
    Miss,
}

/// The scene's geometry gathered into a TLAS, built once per render.
///
/// `Scene` keeps `objects` as its authoring surface — a plain list is the
/// right thing to *write* — while the integrator traces against this. Kept
/// separate rather than added as a `Scene` field so the public struct-literal
/// construction in `Scene { objects, lights, env, ground }` keeps working.
pub(crate) struct SceneAccel {
    tlas: Tlas,
    /// Cumulative distribution over `scene.lights`, weighted by emitted power
    /// (emission luminance × area). One entry per light, ending at 1.0.
    ///
    /// Built once per render so next-event estimation can draw *one* light per
    /// bounce instead of shadow-raying all of them: the cost per bounce stops
    /// scaling with the number of softboxes, and the estimator stays unbiased
    /// because each contribution is divided by its own pick probability.
    light_cdf: Vec<f32>,
    /// Probability of picking each light, i.e. the CDF's per-entry mass. Kept
    /// alongside so the MIS weight for a BSDF ray that lands on an emitter can
    /// use the same pick probability the NEE strategy would have used.
    light_pick_pdf: Vec<f32>,
}

impl SceneAccel {
    /// Probability that [`SceneAccel::pick_light`] would choose `index`.
    #[inline]
    pub(crate) fn light_pick_pdf(&self, index: usize) -> f32 {
        self.light_pick_pdf.get(index).copied().unwrap_or(0.0)
    }

    /// Draw one light from the power-weighted table. Returns its index and the
    /// probability with which it was drawn.
    #[inline]
    fn pick_light(&self, u: f32) -> Option<(usize, f32)> {
        if self.light_cdf.is_empty() {
            return None;
        }
        let i = match self
            .light_cdf
            .binary_search_by(|c| c.partial_cmp(&u).unwrap_or(std::cmp::Ordering::Equal))
        {
            Ok(i) | Err(i) => i.min(self.light_cdf.len() - 1),
        };
        let pdf = self.light_pick_pdf[i];
        if pdf > 0.0 { Some((i, pdf)) } else { None }
    }
}

/// Power-weighted selection table over a light list: per-light pick
/// probabilities and their running sum.
///
/// Shared by the CPU integrator and the GPU scene upload so both sample the
/// same distribution — a parity test that compared two different tables would
/// be testing nothing.
pub(crate) fn light_power_table(lights: &[AreaLight]) -> (Vec<f32>, Vec<f32>) {
    let powers: Vec<f32> = lights
        .iter()
        .map(|l| (luminance(l.emission) as f64 * l.area()).max(0.0) as f32)
        .collect();
    let total: f32 = powers.iter().sum();
    let n = lights.len();
    if n == 0 {
        return (Vec::new(), Vec::new());
    }
    // A scene whose lights all carry zero power still needs a valid
    // distribution; uniform costs nothing and keeps the estimator finite.
    let pick: Vec<f32> = if total > 0.0 && total.is_finite() {
        powers.iter().map(|p| p / total).collect()
    } else {
        vec![1.0 / n as f32; n]
    };
    let mut cdf = Vec::with_capacity(n);
    let mut run = 0.0f32;
    for p in &pick {
        run += *p;
        cdf.push(run);
    }
    // Guard against float drift leaving the last entry just under 1.
    if let Some(last) = cdf.last_mut() {
        *last = 1.0;
    }
    (cdf, pick)
}

impl SceneAccel {
    /// Place every object by its own transform (the identity for the usual
    /// case of geometry that arrives already world-placed) and gather them
    /// under one TLAS. Objects already hold `Arc<Bvh>`, so repeated parts
    /// share a BLAS without any copying — and a re-posed frame rebuilds only
    /// this structure.
    pub(crate) fn build(scene: &Scene) -> Self {
        let instances = scene
            .objects
            .iter()
            .enumerate()
            .filter_map(|(i, obj)| Instance::new(Arc::clone(&obj.bvh), obj.transform.clone(), i))
            .collect();
        let (light_cdf, light_pick_pdf) = light_power_table(&scene.lights);
        Self {
            tlas: Tlas::build(instances),
            light_cdf,
            light_pick_pdf,
        }
    }
}

impl Scene {
    /// Closest intersection against objects, ground, and lights.
    fn intersect(&self, accel: &SceneAccel, ray: &Ray) -> Landing {
        let mut best_t = f64::INFINITY;
        let mut landing = Landing::Miss;

        // `1e-7` as the interval floor rather than a post-filter: pushed into
        // the traversal, a surface just behind the one the ray left is still
        // found instead of the whole query being discarded.
        if let Some(found) = self.tlas_hit(accel, ray, 1e-7) {
            best_t = found.hit.t;
            landing = Landing::Surface {
                point: found.hit.point,
                normal: found.hit.normal.into_inner(),
                tangent: found.hit.dpdu,
                material: self.objects[found.payload].material,
            };
        }

        if let Some(g) = &self.ground {
            let d = ray.direction.into_inner();
            if d.z.abs() > 1e-12 {
                let t = (g.z - ray.origin.z) / d.z;
                if t > 1e-6 && t < best_t {
                    best_t = t;
                    landing = Landing::Surface {
                        point: ray.at(t),
                        normal: Vec3::new(0.0, 0.0, 1.0),
                        // The studio sweep is a backdrop, not a machined
                        // face; it has no grain to align to.
                        tangent: None,
                        material: g.material,
                    };
                }
            }
        }

        for (i, l) in self.lights.iter().enumerate() {
            if let Some(t) = l.intersect(ray) {
                if t < best_t {
                    best_t = t;
                    landing = Landing::Light {
                        emission: l.emission,
                        light_index: i,
                        distance: t,
                        point: ray.at(t),
                    };
                }
            }
        }

        landing
    }

    /// Closest geometry hit past `t_min`, in world space.
    fn tlas_hit(
        &self,
        accel: &SceneAccel,
        ray: &Ray,
        t_min: f64,
    ) -> Option<crate::tlas::InstanceHit> {
        accel.tlas.trace_closest_range(ray, t_min, f64::INFINITY)
    }

    /// Any-hit occlusion test against geometry only (lights do not occlude).
    ///
    /// A true any-hit traversal: it returns at the first blocker rather than
    /// finding the nearest one and then comparing distance, which is strictly
    /// more work than a shadow ray needs.
    fn occluded(&self, accel: &SceneAccel, origin: Point3, dir: Vec3, max_dist: f64) -> bool {
        let ray = Ray::new(origin, dir);
        if accel.tlas.occluded_range(&ray, 1e-6, max_dist - 1e-6) {
            return true;
        }
        if let Some(g) = &self.ground {
            let d = ray.direction.into_inner();
            if d.z.abs() > 1e-12 {
                let t = (g.z - origin.z) / d.z;
                if t > 1e-6 && t < max_dist - 1e-6 {
                    return true;
                }
            }
        }
        false
    }

    /// Next-event estimation: sample *one* area light, drawn from the
    /// accel's power-weighted table, MIS-weighted against the BSDF sampling
    /// strategy.
    ///
    /// One shadow ray per bounce regardless of how many softboxes the rig
    /// has. Dividing the contribution by the pick probability leaves the
    /// estimator unbiased — the mean over many samples matches the old
    /// sample-every-light estimator exactly — and picking by power means the
    /// lights that matter are the ones usually chosen.
    // The shading frame (p, t, b, n) and the outgoing direction are the
    // integrator's hot-loop state; bundling them into a struct just to
    // satisfy the lint would add a copy per light sample.
    #[allow(clippy::too_many_arguments)]
    fn sample_lights(
        &self,
        accel: &SceneAccel,
        p: Point3,
        frame: &Frame,
        wo_local: Vec3,
        m: &Pbr,
        rng: &mut Rng,
    ) -> [f32; 3] {
        let Frame { t, b, n } = *frame;
        // The pick draw comes first so the light choice is independent of the
        // position draw on the chosen rectangle.
        let Some((index, pick_pdf)) = accel.pick_light(rng.f64() as f32) else {
            return [0.0; 3];
        };
        let light = &self.lights[index];
        let lp = light.sample(rng.f64(), rng.f64());
        let to_light = lp - p;
        let dist = to_light.norm();
        if dist < 1e-9 {
            return [0.0; 3];
        }
        let wi_world = to_light / dist;
        let ln = light.normal();
        let cos_light = -wi_world.dot(ln);
        if cos_light <= 1e-9 {
            return [0.0; 3];
        }
        let wi_local = to_local(t, b, n, wi_world);
        if wi_local.z <= 0.0 {
            return [0.0; 3];
        }

        let (f, bsdf_pdf) = bsdf_eval(m, wo_local, wi_local);
        if max3(f) <= 0.0 {
            return [0.0; 3];
        }

        // Solid-angle PDF of the *full* NEE strategy: pick this light, then
        // pick a point on it. The BSDF-hits-a-light branch in `radiance`
        // reconstructs the same product, so MIS stays consistent.
        let light_pdf = pick_pdf * (dist * dist / (cos_light * light.area())) as f32;
        if !light_pdf.is_finite() || light_pdf <= 0.0 {
            return [0.0; 3];
        }

        if self.occluded(accel, p + n * 1e-5, wi_world, dist) {
            return [0.0; 3];
        }

        let w = power_heuristic(light_pdf, bsdf_pdf);
        scale3(mul3(f, light.emission), w / light_pdf)
    }

    /// Next-event estimation against the environment, MIS-weighted against
    /// BSDF sampling.
    ///
    /// Only runs for an importance-sampled environment ([`EnvMap`]). The
    /// analytic gradient stays BSDF-only, exactly as before — it is
    /// low-frequency enough that a second strategy buys nothing.
    fn sample_environment(
        &self,
        accel: &SceneAccel,
        p: Point3,
        frame: &Frame,
        wo_local: Vec3,
        m: &Pbr,
        rng: &mut Rng,
    ) -> [f32; 3] {
        let Frame { t, b, n } = *frame;
        let Some((wi_world, li, env_pdf)) = self.env.sample(rng.f64(), rng.f64()) else {
            return [0.0; 3];
        };
        if !env_pdf.is_finite() || env_pdf <= 0.0 || max3(li) <= 0.0 {
            return [0.0; 3];
        }
        let wi_local = to_local(t, b, n, wi_world);
        if wi_local.z <= 0.0 {
            return [0.0; 3];
        }
        let (f, bsdf_pdf) = bsdf_eval(m, wo_local, wi_local);
        if max3(f) <= 0.0 {
            return [0.0; 3];
        }
        // The environment is at infinity: nothing between here and the sky
        // may block, so the shadow ray is unbounded.
        if self.occluded(accel, p + n * 1e-5, wi_world, f64::INFINITY) {
            return [0.0; 3];
        }
        let w = power_heuristic(env_pdf, bsdf_pdf);
        scale3(mul3(f, li), w / env_pdf)
    }
}

// ─── integrator ───────────────────────────────────────────────────────────

/// What the primary ray of a path found at depth 0.
///
/// Recorded for the denoiser's guide buffers. `depth == 0.0` means the primary
/// ray escaped the scene — the sentinel for "background", which the filter
/// refuses to mix with any surface.
#[derive(Debug, Clone, Copy, Default)]
struct Primary {
    /// Whether the primary ray hit geometry or an emitter (drives alpha).
    hit: bool,
    /// Face-forwarded world normal at the first hit.
    normal: [f32; 3],
    /// Distance from the camera to the first hit; 0 for a miss.
    depth: f32,
    /// Surface colour at the first hit, for albedo demodulation.
    albedo: [f32; 3],
}

/// Trace one path and return its radiance estimate, plus what its primary ray
/// landed on (for alpha and for the denoiser's guide buffers).
fn radiance(
    scene: &Scene,
    accel: &SceneAccel,
    opts: &PathTraceOptions,
    ray: Ray,
    rng: &mut Rng,
) -> ([f32; 3], Primary) {
    let origin = ray.origin;
    let mut primary = Primary::default();
    let mut l = [0.0f32; 3];
    let mut throughput = [1.0f32; 3];
    let mut ray = ray;
    // The previous bounce was sampled from a lobe with this PDF; used to MIS
    // against light sampling when the new ray lands on an emitter.
    let mut prev_bsdf_pdf = 0.0f32;
    let mut specular_chain = true;

    for depth in 0..opts.max_depth {
        match scene.intersect(accel, &ray) {
            Landing::Miss => {
                let dir = ray.direction.into_inner();
                let env = scene.env.radiance(dir);
                if depth == 0 && !opts.show_background {
                    // Leave the backdrop clear; still no contribution.
                    break;
                }
                // MIS against environment NEE, which could also have found
                // this direction. A specular chain (including the primary
                // ray) had no other strategy, so it takes full weight.
                let w = if specular_chain || !scene.env.is_importance_sampled() {
                    1.0
                } else {
                    power_heuristic(prev_bsdf_pdf, scene.env.pdf(dir))
                };
                l = add3(l, scale3(mul3(throughput, env), w));
                break;
            }
            Landing::Light {
                emission,
                light_index,
                distance,
                point,
            } => {
                if depth == 0 {
                    // An emitter seen directly. It is noise-free by
                    // construction, but it still needs a guide entry so the
                    // filter treats it as its own surface rather than as
                    // background.
                    primary.hit = true;
                    primary.depth = distance as f32;
                    primary.normal = vec_to_f32(scene.lights[light_index].normal());
                    primary.albedo = [1.0; 3];
                }
                let w = if specular_chain {
                    1.0
                } else {
                    // MIS against the NEE strategy that could also have found
                    // this light.
                    let light = &scene.lights[light_index];
                    let ln = light.normal();
                    let cos_light = (-ray.direction.into_inner().dot(ln)).max(1e-9);
                    let light_pdf = accel.light_pick_pdf(light_index)
                        * (distance * distance / (cos_light * light.area())) as f32;
                    let _ = point;
                    power_heuristic(prev_bsdf_pdf, light_pdf)
                };
                l = add3(l, scale3(mul3(throughput, emission), w));
                break;
            }
            Landing::Surface {
                point,
                normal,
                tangent,
                material,
            } => {
                let wo_world = -ray.direction.into_inner();
                // Face-forward: interior faces (bore walls) must shade right.
                let n = if normal.dot(wo_world) < 0.0 {
                    -normal
                } else {
                    normal
                };
                if depth == 0 {
                    primary.hit = true;
                    primary.depth = (point - origin).norm() as f32;
                    primary.normal = vec_to_f32(n);
                    primary.albedo = material.denoise_albedo();
                }
                let frame = shading_frame(n, tangent);
                let wo_local = to_local(frame.t, frame.b, n, wo_world);
                if wo_local.z <= 0.0 {
                    break;
                }

                l = add3(l, mul3(throughput, material.emissive));

                // Next-event estimation: explicit lights, plus the
                // environment when it is importance-sampled.
                let direct = add3(
                    scene.sample_lights(accel, point, &frame, wo_local, &material, rng),
                    scene.sample_environment(accel, point, &frame, wo_local, &material, rng),
                );
                let direct = match opts.firefly_clamp {
                    Some(c) if depth > 0 => [direct[0].min(c), direct[1].min(c), direct[2].min(c)],
                    _ => direct,
                };
                l = add3(l, mul3(throughput, direct));

                // Continue the path.
                let Some((wi_local, f, pdf)) = bsdf_sample(&material, wo_local, rng) else {
                    break;
                };
                throughput = mul3(throughput, scale3(f, 1.0 / pdf));
                prev_bsdf_pdf = pdf;
                specular_chain = false;

                let wi_world = to_world(frame.t, frame.b, n, wi_local);
                ray = Ray::new(point + n * 1e-5, wi_world);

                // Russian roulette.
                if depth >= opts.rr_start {
                    let q = max3(throughput).clamp(0.0, 0.95);
                    if (rng.f64() as f32) > q {
                        break;
                    }
                    throughput = scale3(throughput, 1.0 / q);
                }
                if max3(throughput) <= 1e-5 {
                    break;
                }
            }
        }
    }

    (l, primary)
}

#[inline]
fn vec_to_f32(v: Vec3) -> [f32; 3] {
    [v.x as f32, v.y as f32, v.z as f32]
}

/// A rendered frame in linear space.
pub struct Film {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Linear RGB radiance, 3 floats per pixel, row-major top-to-bottom.
    pub rgb: Vec<f32>,
    /// Coverage in 0..1, one float per pixel.
    pub alpha: Vec<f32>,
    /// World normal at each pixel's first hit, 3 floats per pixel. Zero for
    /// background pixels. Guide buffer for [`denoise`].
    pub normal: Vec<f32>,
    /// Distance from the camera to each pixel's first hit, one float per
    /// pixel. **Zero means the primary ray escaped** — the background
    /// sentinel. Guide buffer for [`denoise`].
    pub depth: Vec<f32>,
    /// Surface colour at each pixel's first hit, 3 floats per pixel. Divided
    /// out before filtering and multiplied back after, so [`denoise`] only
    /// ever blurs illumination.
    pub albedo: Vec<f32>,
    /// Estimated variance of each pixel's mean radiance luminance, one float
    /// per pixel — the Monte Carlo estimator's own error bar.
    ///
    /// [`denoise`] scales its luminance edge-stopping tolerance by this, which
    /// is what lets the filter tell "this neighbour is genuinely a different
    /// brightness" from "this pixel is a noise spike". Without it a firefly
    /// rejects every neighbour and survives the filter untouched.
    pub variance: Vec<f32>,
}

/// Render `scene` from `cam` into a linear-space [`Film`].
///
/// Scanlines are traced in parallel. Each pixel's RNG is seeded from its
/// coordinates and the option seed, so output is deterministic and
/// independent of thread scheduling.
///
/// When [`PathTraceOptions::denoise`] is set (the default), the film is run
/// through [`denoise`] before returning. Pass `denoise: false` for a
/// reference render.
pub fn render(
    scene: &Scene,
    cam: &Camera,
    width: u32,
    height: u32,
    opts: &PathTraceOptions,
) -> Film {
    use rayon::prelude::*;

    let aspect = width as f64 / height as f64;
    let spp = opts.spp.max(1);

    // One TLAS for the whole frame: every ray, primary and shadow, traverses
    // it instead of scanning `scene.objects` linearly.
    let accel = SceneAccel::build(scene);

    let mut rgb = vec![0.0f32; (width * height * 3) as usize];
    let mut alpha = vec![0.0f32; (width * height) as usize];
    let mut normal = vec![0.0f32; (width * height * 3) as usize];
    let mut depth = vec![0.0f32; (width * height) as usize];
    let mut albedo = vec![0.0f32; (width * height * 3) as usize];
    let mut variance = vec![0.0f32; (width * height) as usize];

    let w3 = width as usize * 3;
    let w1 = width as usize;
    rgb.par_chunks_mut(w3)
        .zip(alpha.par_chunks_mut(w1))
        .zip(normal.par_chunks_mut(w3))
        .zip(depth.par_chunks_mut(w1))
        .zip(albedo.par_chunks_mut(w3))
        .zip(variance.par_chunks_mut(w1))
        .enumerate()
        .for_each(|(py, (((((row, arow), nrow), drow), brow), vrow))| {
            for px in 0..width as usize {
                let mut rng = Rng::new(
                    opts.seed
                        ^ ((py as u64) << 32)
                        ^ (px as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15),
                );
                let mut acc = [0.0f32; 3];
                let mut cov = 0.0f32;
                // Running sums for the estimator's own variance.
                let mut lsum = 0.0f32;
                let mut lsum2 = 0.0f32;

                for s in 0..spp {
                    // Jittered pixel position.
                    let jx = rng.f64();
                    let jy = rng.f64();
                    let sx = 2.0 * ((px as f64 + jx) / width as f64) - 1.0;
                    let sy = 1.0 - 2.0 * ((py as f64 + jy) / height as f64);
                    let (lu, lv) = concentric_disc(rng.f64(), rng.f64());

                    let ray = cam.ray(sx, sy, aspect, lu, lv);
                    let (l, primary) = radiance(scene, &accel, opts, ray, &mut rng);
                    acc = add3(acc, l);
                    let ls = luminance(l);
                    lsum += ls;
                    lsum2 += ls * ls;
                    if primary.hit {
                        cov += 1.0;
                    }
                    if s == 0 {
                        // Guide buffers come from one primary ray, not an
                        // average: averaging normals and depths across
                        // samples would soften exactly the silhouettes the
                        // edge-stopping weights exist to protect.
                        nrow[px * 3] = primary.normal[0];
                        nrow[px * 3 + 1] = primary.normal[1];
                        nrow[px * 3 + 2] = primary.normal[2];
                        drow[px] = primary.depth;
                        brow[px * 3] = primary.albedo[0];
                        brow[px * 3 + 1] = primary.albedo[1];
                        brow[px * 3 + 2] = primary.albedo[2];
                    }
                }

                let inv = 1.0 / spp as f32;
                row[px * 3] = acc[0] * inv;
                row[px * 3 + 1] = acc[1] * inv;
                row[px * 3 + 2] = acc[2] * inv;
                arow[px] = cov * inv;
                // Variance of the *mean*: sample variance / spp. A single
                // sample carries no information about its own spread, so fall
                // back to the estimate itself as a scale.
                vrow[px] = if spp > 1 {
                    let mean = lsum * inv;
                    let sample_var =
                        (lsum2 * inv - mean * mean).max(0.0) * spp as f32 / (spp - 1) as f32;
                    sample_var / spp as f32
                } else {
                    let mean = lsum;
                    mean * mean
                };
            }
        });

    let mut film = Film {
        width,
        height,
        rgb,
        alpha,
        normal,
        depth,
        albedo,
        variance,
    };
    if opts.denoise {
        denoise(&mut film, opts);
    }
    film
}

// ─── denoising ────────────────────────────────────────────────────────────

/// 5×5 separable B3-spline (cubic) kernel, `[1 4 6 4 1] / 16`.
const B3_SPLINE: [f32; 5] = [1.0 / 16.0, 1.0 / 4.0, 3.0 / 8.0, 1.0 / 4.0, 1.0 / 16.0];

/// Floor on the demodulation divisor.
///
/// Dividing by a near-black albedo would turn a dark surface's illumination
/// into enormous numbers, and any filtering error there comes back multiplied.
/// Clamping trades a little residual colour-blurring on very dark materials
/// for numerical sanity.
const DEMOD_FLOOR: f32 = 0.05;

/// Edge-aware à-trous wavelet denoiser (Dammertz et al., EGSR 2010).
///
/// Filters the film's linear radiance in place, guided by the normal, depth,
/// and albedo buffers that [`render`] records from each pixel's primary ray.
///
/// The algorithm is a sequence of 5×5 B3-spline convolutions whose taps are
/// spread by a doubling stride ("holes" — *à trous*), each tap weighted by how
/// well the neighbour matches the centre pixel's normal, depth, and
/// illumination. That reaches a wide footprint in a few passes while refusing
/// to average across geometric or shading discontinuities.
///
/// Two properties are worth naming, because the tests pin them:
///
/// - **Illumination only.** Radiance is divided by albedo before filtering and
///   multiplied back afterwards, so a part's colour is never blurred into its
///   neighbour's — only the Monte Carlo noise in the lighting is smoothed.
/// - **Background is inviolable.** A pixel whose primary ray escaped
///   (`depth == 0`) is passed through untouched, and no surface pixel ever
///   accepts a tap from one. Silhouettes against the backdrop stay exactly as
///   sharp as the path tracer drew them.
///
/// This is a post-process: it consumes no random numbers and never touches the
/// integrator, so a reference render is exactly the un-denoised film.
///
/// Only the `denoise_iters` and `sigma_*` fields of `opts` are read. Calling
/// this *is* the request to filter, so [`PathTraceOptions::denoise`] is the
/// caller's gate — as [`render`] uses it — and is deliberately ignored here.
pub fn denoise(film: &mut Film, opts: &PathTraceOptions) {
    use rayon::prelude::*;

    let w = film.width as usize;
    let h = film.height as usize;
    let n = w * h;
    if n == 0 || opts.denoise_iters == 0 {
        return;
    }

    // Demodulate: work on illumination = radiance / albedo.
    let mut illum = vec![0.0f32; n * 3];
    let mut var = vec![0.0f32; n];
    for i in 0..n {
        for c in 0..3 {
            let a = film.albedo[i * 3 + c].max(DEMOD_FLOOR);
            illum[i * 3 + c] = film.rgb[i * 3 + c] / a;
        }
        // Variance was measured on radiance; demodulation scales it by the
        // square of the (scalar) albedo it divided through.
        let la = luminance([
            film.albedo[i * 3].max(DEMOD_FLOOR),
            film.albedo[i * 3 + 1].max(DEMOD_FLOOR),
            film.albedo[i * 3 + 2].max(DEMOD_FLOOR),
        ])
        .max(DEMOD_FLOOR);
        var[i] = film.variance[i] / (la * la);
    }

    // Prefilter the variance estimate with a 3×3 box. The per-pixel estimate
    // is itself noisy at low sample counts, and a noisy error bar makes the
    // luminance weight jitter between "trust" and "reject" from pixel to
    // pixel.
    {
        let mut smooth = var.clone();
        for y in 0..h {
            for x in 0..w {
                let mut s = 0.0f32;
                let mut k = 0.0f32;
                for dy in -1i32..=1 {
                    for dx in -1i32..=1 {
                        let (qx, qy) = (x as i32 + dx, y as i32 + dy);
                        if qx < 0 || qy < 0 || qx >= w as i32 || qy >= h as i32 {
                            continue;
                        }
                        let q = qy as usize * w + qx as usize;
                        if film.depth[q] <= 0.0 {
                            continue;
                        }
                        s += var[q];
                        k += 1.0;
                    }
                }
                if k > 0.0 {
                    smooth[y * w + x] = s / k;
                }
            }
        }
        var = smooth;
    }

    let sigma_n2 = (opts.sigma_normal.max(1e-4)).powi(2);
    let mut scratch = illum.clone();
    let mut var_scratch = var.clone();
    let g_depth = &film.depth;
    let g_normal = &film.normal;

    for it in 0..opts.denoise_iters {
        let stride = 1usize << it;
        // Dammertz shrinks a *fixed* illumination tolerance as the footprint
        // grows. Here the tolerance is already scaled by the filtered variance,
        // which shrinks on its own as the estimate gets cleaner, so shrinking
        // sigma too would penalise the wide passes twice and they would reject
        // every tap. Measured: with the extra 2^-i, iterations past the first
        // bought nothing at all.
        let sigma_l = opts.sigma_lum.max(1e-6);
        let sigma_z = opts.sigma_depth.max(1e-6) * stride as f32;

        scratch
            .par_chunks_mut(w * 3)
            .zip(var_scratch.par_chunks_mut(w))
            .enumerate()
            .for_each(|(y, (row, vrow))| {
                for x in 0..w {
                    let p = y * w + x;
                    let z_p = g_depth[p];
                    if z_p <= 0.0 {
                        // Background: analytic and noise-free. Pass through.
                        row[x * 3] = illum[p * 3];
                        row[x * 3 + 1] = illum[p * 3 + 1];
                        row[x * 3 + 2] = illum[p * 3 + 2];
                        vrow[x] = var[p];
                        continue;
                    }
                    let n_p = [g_normal[p * 3], g_normal[p * 3 + 1], g_normal[p * 3 + 2]];
                    let c_p = [illum[p * 3], illum[p * 3 + 1], illum[p * 3 + 2]];
                    let l_p = luminance(c_p);
                    // The estimator's own error bar sets how much luminance
                    // disagreement counts as signal rather than noise. A
                    // firefly has an enormous error bar, so it stops
                    // protecting itself and gets filtered.
                    let l_tol = sigma_l * var[p].max(0.0).sqrt() + 1e-4;

                    let mut sum = [0.0f32; 3];
                    let mut vsum = 0.0f32;
                    let mut wsum = 0.0f32;

                    for (ky, dy) in (-2i32..=2).enumerate() {
                        let qy = y as i32 + dy * stride as i32;
                        if qy < 0 || qy >= h as i32 {
                            continue;
                        }
                        for (kx, dx) in (-2i32..=2).enumerate() {
                            let qx = x as i32 + dx * stride as i32;
                            if qx < 0 || qx >= w as i32 {
                                continue;
                            }
                            let q = qy as usize * w + qx as usize;
                            let z_q = g_depth[q];
                            if z_q <= 0.0 {
                                // Never let the backdrop bleed onto a surface.
                                continue;
                            }

                            // Normal: squared distance between unit normals.
                            let dn = [
                                n_p[0] - g_normal[q * 3],
                                n_p[1] - g_normal[q * 3 + 1],
                                n_p[2] - g_normal[q * 3 + 2],
                            ];
                            let dn2 = dn[0] * dn[0] + dn[1] * dn[1] + dn[2] * dn[2];
                            let w_n = (-dn2 / sigma_n2).exp();

                            // Depth: relative, so the tolerance scales with
                            // scene size instead of being tuned per model.
                            let w_z = (-(z_p - z_q).abs() / (sigma_z * z_p)).exp();

                            // Illumination: rejects the far side of a shadow
                            // edge or a specular highlight.
                            let c_q = [illum[q * 3], illum[q * 3 + 1], illum[q * 3 + 2]];
                            let w_l = (-(l_p - luminance(c_q)).abs() / l_tol).exp();

                            let weight = B3_SPLINE[kx] * B3_SPLINE[ky] * w_n * w_z * w_l;
                            if weight <= 0.0 {
                                continue;
                            }
                            sum = add3(sum, scale3(c_q, weight));
                            // Variance of a weighted mean of independent
                            // estimates carries the *squared* weights.
                            vsum += weight * weight * var[q];
                            wsum += weight;
                        }
                    }

                    let (out, vout) = if wsum > 0.0 {
                        (scale3(sum, 1.0 / wsum), vsum / (wsum * wsum))
                    } else {
                        (c_p, var[p])
                    };
                    row[x * 3] = out[0];
                    row[x * 3 + 1] = out[1];
                    row[x * 3 + 2] = out[2];
                    vrow[x] = vout;
                }
            });

        std::mem::swap(&mut illum, &mut scratch);
        std::mem::swap(&mut var, &mut var_scratch);
    }

    // Re-modulate back into radiance. Background pixels are left exactly as
    // the tracer wrote them — a divide-then-multiply round trip is not
    // bit-exact in f32, and the backdrop has no noise to remove anyway.
    for i in 0..n {
        if film.depth[i] <= 0.0 {
            continue;
        }
        for c in 0..3 {
            let a = film.albedo[i * 3 + c].max(DEMOD_FLOOR);
            film.rgb[i * 3 + c] = illum[i * 3 + c] * a;
        }
    }
}

// ─── tonemapping ──────────────────────────────────────────────────────────

/// ACES filmic tonemap (Narkowicz fit).
#[inline]
pub fn tonemap_aces(x: f32) -> f32 {
    let a = 2.51;
    let b = 0.03;
    let c = 2.43;
    let d = 0.59;
    let e = 0.14;
    ((x * (a * x + b)) / (x * (c * x + d) + e)).clamp(0.0, 1.0)
}

/// Linear to sRGB transfer.
#[inline]
pub fn linear_to_srgb(x: f32) -> f32 {
    if x <= 0.0031308 {
        12.92 * x
    } else {
        1.055 * x.powf(1.0 / 2.4) - 0.055
    }
}

impl Film {
    /// Convert to 8-bit sRGB RGBA with ACES tonemapping.
    ///
    /// `exposure` scales linear radiance before the tonemap curve.
    pub fn to_srgb8(&self, exposure: f32, transparent: bool) -> Vec<u8> {
        let n = (self.width * self.height) as usize;
        let mut out = vec![0u8; n * 4];
        for i in 0..n {
            for c in 0..3 {
                let v = tonemap_aces(self.rgb[i * 3 + c] * exposure);
                out[i * 4 + c] = (linear_to_srgb(v) * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
            }
            out[i * 4 + 3] = if transparent {
                (self.alpha[i] * 255.0 + 0.5).clamp(0.0, 255.0) as u8
            } else {
                255
            };
        }
        out
    }
}

// ─── default studio rig ───────────────────────────────────────────────────

/// Build a three-point softbox rig sized to a scene of the given radius,
/// centred on `center`.
///
/// Key light upper-front-left, a broad cool fill opposite it, and a small
/// bright rim behind to separate the subject from the backdrop.
pub fn studio_rig(center: Point3, radius: f64) -> Vec<AreaLight> {
    let r = radius.max(1e-6);
    let mk = |dir: Vec3, dist: f64, size: f64, emission: [f32; 3]| -> AreaLight {
        let pos = center + dir.normalize() * (r * dist);
        // Orient the rectangle to face the scene centre.
        let n = (center - pos).normalize();
        let (u, v) = onb(n);
        AreaLight {
            center: pos,
            u: u * (r * size),
            v: v * (r * size),
            emission,
        }
    };

    // Emission is radiance, so the useful quantity is emission × solid
    // angle. A softbox of half-size `s` at distance `d` subtends roughly
    // (2s/d)² steradians; these values are chosen to land the key at ~3
    // and the rim at ~1.5 units of irradiance on a facing surface.
    vec![
        // Key: large, slightly warm, high and to the left.
        mk(Vec3::new(-0.8, -1.0, 1.1), 3.2, 1.4, [4.2, 4.0, 3.75]),
        // Fill: broad and cool, opposite side, much dimmer.
        mk(Vec3::new(1.3, -0.6, 0.25), 3.6, 1.8, [0.5, 0.56, 0.68]),
        // Rim: small and hot, behind and above, to pop the silhouette.
        mk(Vec3::new(0.35, 1.25, 0.8), 3.0, 0.55, [11.0, 10.8, 10.5]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_kernel_primitives::make_cube;

    fn test_scene() -> Scene {
        let cube = make_cube(10.0, 10.0, 10.0);
        Scene {
            objects: vec![Object::new(
                Arc::new(Bvh::build(&cube)),
                Pbr::plastic([0.8, 0.3, 0.2], 0.35, 0.0),
            )],
            lights: studio_rig(Point3::new(5.0, 5.0, 5.0), 9.0),
            env: Environment::default(),
            ground: None,
        }
    }

    /// The old estimator: shadow-ray every light, each with its own
    /// area-sampling PDF. Kept here as the reference the one-light-per-bounce
    /// importance sampler must match in expectation.
    fn sample_all_lights_reference(
        scene: &Scene,
        accel: &SceneAccel,
        p: Point3,
        frame: &Frame,
        wo_local: Vec3,
        m: &Pbr,
        rng: &mut Rng,
    ) -> [f32; 3] {
        let Frame { t, b, n } = *frame;
        let mut sum = [0.0f32; 3];
        for light in &scene.lights {
            let lp = light.sample(rng.f64(), rng.f64());
            let to_light = lp - p;
            let dist = to_light.norm();
            if dist < 1e-9 {
                continue;
            }
            let wi_world = to_light / dist;
            let cos_light = -wi_world.dot(light.normal());
            if cos_light <= 1e-9 {
                continue;
            }
            let wi_local = to_local(t, b, n, wi_world);
            if wi_local.z <= 0.0 {
                continue;
            }
            let (f, bsdf_pdf) = bsdf_eval(m, wo_local, wi_local);
            if max3(f) <= 0.0 {
                continue;
            }
            let light_pdf = (dist * dist / (cos_light * light.area())) as f32;
            if !light_pdf.is_finite() || light_pdf <= 0.0 {
                continue;
            }
            if scene.occluded(accel, p + n * 1e-5, wi_world, dist) {
                continue;
            }
            // The reference's MIS partner must be *its* own light pdf, so
            // this is the old weighting verbatim.
            let w = power_heuristic(light_pdf, bsdf_pdf);
            sum = add3(sum, scale3(mul3(f, light.emission), w / light_pdf));
        }
        sum
    }

    /// Both estimators are MIS-weighted, and the two weightings differ per
    /// sample (the pick probability enters the light pdf). What must agree is
    /// the *total* direct-lighting estimate — NEE plus the BSDF-sampled hits
    /// on emitters — so this compares the unweighted NEE integral by driving
    /// both with `power_heuristic` replaced by 1: i.e. the plain estimator
    /// `f * Le * cos / pdf`, which is what unbiasedness is about.
    fn nee_unweighted_mean(scene: &Scene, accel: &SceneAccel, pick_one: bool, n: usize) -> [f64; 3] {
        let p = Point3::new(0.0, 0.0, 0.0);
        let nrm = Vec3::new(0.0, 0.0, 1.0);
        let frame = shading_frame(nrm, None);
        let wo_world = Vec3::new(0.3, 0.2, 0.9).normalize();
        let wo_local = to_local(frame.t, frame.b, nrm, wo_world);
        let m = Pbr {
            base_color: [0.8, 0.7, 0.6],
            roughness: 0.6,
            ..Default::default()
        };
        let mut rng = Rng::new(0xA11CE);
        let mut sum = [0.0f64; 3];
        for _ in 0..n {
            let est = if pick_one {
                let Some((i, pick_pdf)) = accel.pick_light(rng.f64() as f32) else {
                    continue;
                };
                one_light_unweighted(&scene.lights[i], pick_pdf, p, &frame, wo_local, &m, &mut rng)
            } else {
                let mut acc = [0.0f32; 3];
                for light in &scene.lights {
                    acc = add3(
                        acc,
                        one_light_unweighted(light, 1.0, p, &frame, wo_local, &m, &mut rng),
                    );
                }
                acc
            };
            for c in 0..3 {
                sum[c] += est[c] as f64;
            }
        }
        [
            sum[0] / n as f64,
            sum[1] / n as f64,
            sum[2] / n as f64,
        ]
    }

    fn one_light_unweighted(
        light: &AreaLight,
        pick_pdf: f32,
        p: Point3,
        frame: &Frame,
        wo_local: Vec3,
        m: &Pbr,
        rng: &mut Rng,
    ) -> [f32; 3] {
        let Frame { t, b, n } = *frame;
        let lp = light.sample(rng.f64(), rng.f64());
        let to_light = lp - p;
        let dist = to_light.norm();
        if dist < 1e-9 {
            return [0.0; 3];
        }
        let wi_world = to_light / dist;
        let cos_light = -wi_world.dot(light.normal());
        if cos_light <= 1e-9 {
            return [0.0; 3];
        }
        let wi_local = to_local(t, b, n, wi_world);
        if wi_local.z <= 0.0 {
            return [0.0; 3];
        }
        let (f, _) = bsdf_eval(m, wo_local, wi_local);
        let pdf = pick_pdf * (dist * dist / (cos_light * light.area())) as f32;
        if !pdf.is_finite() || pdf <= 0.0 {
            return [0.0; 3];
        }
        scale3(mul3(f, light.emission), 1.0 / pdf)
    }

    fn open_scene(lights: Vec<AreaLight>) -> Scene {
        Scene {
            objects: Vec::new(),
            lights,
            env: Environment::default(),
            ground: None,
        }
    }

    fn panel(center: Point3, emission: [f32; 3], half: f64) -> AreaLight {
        // Faces -Z, i.e. down at the origin.
        AreaLight {
            center,
            u: Vec3::new(half, 0.0, 0.0),
            v: Vec3::new(0.0, -half, 0.0),
            emission,
        }
    }

    /// One light per bounce, drawn from the power table and divided by its
    /// pick probability, must integrate to the same direct lighting as
    /// shadow-raying every light. Two lights of very different power, so a
    /// uniform pick would not have been enough.
    #[test]
    fn one_light_per_bounce_matches_all_lights_in_expectation() {
        let scene = open_scene(vec![
            panel(Point3::new(-2.0, 0.0, 4.0), [12.0, 11.0, 10.0], 1.5),
            panel(Point3::new(3.0, 1.0, 5.0), [0.6, 0.7, 1.4], 0.7),
        ]);
        let accel = SceneAccel::build(&scene);
        let n = 400_000;
        let a = nee_unweighted_mean(&scene, &accel, true, n);
        let b = nee_unweighted_mean(&scene, &accel, false, n);
        for c in 0..3 {
            let rel = (a[c] - b[c]).abs() / b[c].abs().max(1e-6);
            assert!(
                rel < 0.02,
                "channel {c}: one-light mean {} vs all-lights mean {} (rel {rel})",
                a[c],
                b[c]
            );
        }
    }

    /// The power table must actually be power-weighted: the bright panel is
    /// picked far more often than the dim one, and the probabilities sum to 1.
    #[test]
    fn light_table_is_power_weighted() {
        let scene = open_scene(vec![
            panel(Point3::new(-2.0, 0.0, 4.0), [12.0, 11.0, 10.0], 1.5),
            panel(Point3::new(3.0, 1.0, 5.0), [0.6, 0.7, 1.4], 0.7),
        ]);
        let accel = SceneAccel::build(&scene);
        let p0 = accel.light_pick_pdf(0);
        let p1 = accel.light_pick_pdf(1);
        assert!((p0 + p1 - 1.0).abs() < 1e-5, "pick pdf must sum to 1");
        assert!(p0 > 0.9, "the bright, large panel should dominate: {p0}");
        // Drawing follows the table.
        let mut rng = Rng::new(7);
        let mut hits = [0u32; 2];
        for _ in 0..20_000 {
            let (i, _) = accel.pick_light(rng.f64() as f32).unwrap();
            hits[i] += 1;
        }
        let frac0 = hits[0] as f32 / 20_000.0;
        assert!((frac0 - p0).abs() < 0.02, "draw {frac0} vs table {p0}");
    }

    /// A full render of a multi-light scene must still land on the same image
    /// the all-lights estimator gives, within Monte Carlo noise.
    #[test]
    fn multi_light_render_matches_reference_mean() {
        // Exercise the reference path so it cannot rot.
        let scene = open_scene(vec![
            panel(Point3::new(-2.0, 0.0, 4.0), [8.0, 8.0, 8.0], 1.2),
            panel(Point3::new(3.0, 1.0, 5.0), [2.0, 2.0, 2.0], 1.0),
        ]);
        let accel = SceneAccel::build(&scene);
        let nrm = Vec3::new(0.0, 0.0, 1.0);
        let frame = shading_frame(nrm, None);
        let m = Pbr::default();
        let wo_local = to_local(frame.t, frame.b, nrm, nrm);
        let mut rng = Rng::new(3);
        let mut r = [0.0f64; 3];
        let mut o = [0.0f64; 3];
        let n = 200_000;
        for _ in 0..n {
            let a = scene.sample_lights(
                &accel,
                Point3::new(0.0, 0.0, 0.0),
                &frame,
                wo_local,
                &m,
                &mut rng,
            );
            let b = sample_all_lights_reference(
                &scene,
                &accel,
                Point3::new(0.0, 0.0, 0.0),
                &frame,
                wo_local,
                &m,
                &mut rng,
            );
            for c in 0..3 {
                r[c] += a[c] as f64;
                o[c] += b[c] as f64;
            }
        }
        // MIS weights differ slightly between the two (the light pdf carries
        // the pick probability), so this is a loose sanity band, not equality.
        for c in 0..3 {
            let rel = (r[c] - o[c]).abs() / (o[c] / n as f64).abs().max(1e-9) / n as f64;
            assert!(rel < 0.06, "channel {c}: {} vs {} (rel {rel})", r[c] / n as f64, o[c] / n as f64);
        }
    }

    fn test_camera() -> Camera {
        Camera::look_at(
            Point3::new(30.0, -34.0, 24.0),
            Point3::new(5.0, 5.0, 5.0),
            Vec3::new(0.0, 0.0, 1.0),
            32.0,
        )
    }

    /// `from_basis` must preserve a mirrored (left-handed) screen basis;
    /// `look_at` cannot express one. vcad's isometric view is exactly such a
    /// basis, so this is the property the render path depends on.
    #[test]
    fn from_basis_preserves_mirrored_basis() {
        let c30 = 30f64.to_radians().cos();
        let s30 = 30f64.to_radians().sin();
        let cam = Vec3::new(1.0, 1.0, 1.0).normalize();
        let right = Vec3::new(c30, -c30, 0.0);
        let up = -Vec3::new(s30, s30, -1.0);
        let camera = Camera::from_basis(
            Point3::new(0.0, 0.0, 0.0) + cam * 100.0,
            -cam,
            right,
            up,
            34.0,
            100.0,
        );
        assert!(
            (camera.right - right.normalize()).norm() < 1e-12,
            "right vector was silently re-derived"
        );
        // A right-handed reconstruction would have flipped it.
        let rhs = camera.forward.cross(camera.up).normalize();
        assert!(
            (rhs - camera.right).norm() > 1.0,
            "expected this basis to be mirrored"
        );
    }

    #[test]
    fn renders_non_empty() {
        let scene = test_scene();
        let cam = test_camera();
        let film = render(
            &scene,
            &cam,
            24,
            24,
            &PathTraceOptions {
                spp: 4,
                ..Default::default()
            },
        );
        assert_eq!(film.rgb.len(), 24 * 24 * 3);
        let lit = film.rgb.iter().filter(|v| **v > 0.0).count();
        assert!(lit > 0, "path tracer produced an entirely black frame");
    }

    /// `Object::transform` must actually place the BLAS. This is what an
    /// animated render leans on: the same BVH, re-posed per frame.
    #[test]
    fn object_transform_moves_the_subject() {
        let cam = test_camera();
        let coverage = |t: Transform| {
            let cube = make_cube(10.0, 10.0, 10.0);
            let scene = Scene {
                objects: vec![Object::placed(
                    Arc::new(Bvh::build(&cube)),
                    Pbr::plastic([0.8, 0.3, 0.2], 0.35, 0.0),
                    t,
                )],
                // No area lights: they are hittable geometry, and a rig that
                // stays put while the cube moves would muddy the coverage
                // signal this test reads.
                lights: Vec::new(),
                env: Environment::default(),
                ground: None,
            };
            let film = render(
                &scene,
                &cam,
                32,
                32,
                &PathTraceOptions {
                    spp: 2,
                    ..Default::default()
                },
            );
            film.alpha.iter().map(|a| *a > 0.5).collect::<Vec<_>>()
        };
        let here = coverage(Transform::identity());
        // Far enough out of frame that nothing overlaps.
        let there = coverage(Transform::translation(400.0, 0.0, 0.0));
        assert!(here.iter().any(|c| *c), "identity placement lost the cube");
        assert!(
            !there.iter().any(|c| *c),
            "translated placement was ignored — the cube stayed put"
        );
    }

    #[test]
    fn subject_is_covered() {
        let scene = test_scene();
        let cam = test_camera();
        let film = render(
            &scene,
            &cam,
            32,
            32,
            &PathTraceOptions {
                spp: 4,
                ..Default::default()
            },
        );
        let covered = film.alpha.iter().filter(|a| **a > 0.5).count();
        assert!(
            covered > 40,
            "expected the cube to cover a chunk of frame, got {covered}"
        );
    }

    #[test]
    fn deterministic_across_runs() {
        let scene = test_scene();
        let cam = test_camera();
        let o = PathTraceOptions {
            spp: 2,
            ..Default::default()
        };
        let a = render(&scene, &cam, 16, 16, &o);
        let b = render(&scene, &cam, 16, 16, &o);
        assert_eq!(a.rgb, b.rgb, "render must be seed-deterministic");
    }

    /// The BSDF sampling PDF must match the analytic PDF used by MIS, or
    /// light sampling and BSDF sampling silently disagree and the image is
    /// energy-wrong in a way that is hard to see by eye.
    #[test]
    fn bsdf_sample_pdf_matches_eval_pdf() {
        // Isotropic, plus anisotropy swept across both signs and both
        // extremes — the sampling and evaluation paths must agree for all of
        // them or MIS is silently energy-wrong.
        let anisos = [0.0, 0.4, 0.8, 1.0, -0.4, -0.8, -1.0];
        for aniso in anisos {
            for roughness in [0.1, 0.4, 0.8] {
                let m = Pbr {
                    base_color: [0.8, 0.8, 0.8],
                    metallic: 0.3,
                    roughness,
                    anisotropy: aniso,
                    clearcoat: 0.5,
                    ..Default::default()
                };
                // Several view directions: a grazing wo is where an
                // anisotropic G1 and a mismatched PDF diverge fastest.
                for wo in [
                    Vec3::new(0.3, 0.15, 0.94).normalize(),
                    Vec3::new(0.85, 0.1, 0.52).normalize(),
                    Vec3::new(0.1, 0.85, 0.52).normalize(),
                    Vec3::new(0.0, 0.0, 1.0),
                ] {
                    let mut rng = Rng::new(7);
                    for _ in 0..256 {
                        if let Some((wi, _f, pdf)) = bsdf_sample(&m, wo, &mut rng) {
                            let (_f2, pdf2) = bsdf_eval(&m, wo, wi);
                            assert!(
                                (pdf - pdf2).abs() <= 1e-4 * pdf.max(1.0),
                                "pdf mismatch at aniso={aniso} rough={roughness}: \
                                 sampled {pdf}, evaluated {pdf2}"
                            );
                        }
                    }
                }
            }
        }
    }

    /// Anisotropy 0 must be the isotropic model exactly — not merely close.
    /// If this drifts, every existing render changes silently.
    #[test]
    fn zero_anisotropy_is_exactly_isotropic() {
        let m = Pbr {
            base_color: [0.8, 0.7, 0.6],
            metallic: 0.6,
            roughness: 0.35,
            anisotropy: 0.0,
            clearcoat: 0.3,
            ..Default::default()
        };
        let (at, ab) = m.alpha_tb();
        assert_eq!(at, m.alpha(), "tangent alpha diverged from the base alpha");
        assert_eq!(
            ab,
            m.alpha(),
            "bitangent alpha diverged from the base alpha"
        );

        // And the lobe terms must take their isotropic branches bit-exactly.
        let wo = Vec3::new(0.3, 0.15, 0.94).normalize();
        let wi = Vec3::new(-0.2, 0.35, 0.91).normalize();
        let wh = (wo + wi).normalize();
        assert_eq!(d_ggx_aniso(wh, at, ab), d_ggx(wh.z.max(0.0) as f32, at));
        assert_eq!(
            v_smith_aniso(wo, wi, at, ab),
            v_smith(wo.z as f32, wi.z as f32, at)
        );
    }

    /// The whole point of the feature: an anisotropic lobe must actually
    /// prefer one tangent direction over the other, and swapping the sign of
    /// the anisotropy must swap which one.
    #[test]
    fn anisotropy_stretches_the_lobe_along_the_tangent() {
        let rough = |anisotropy| Pbr {
            base_color: [1.0, 1.0, 1.0],
            metallic: 1.0,
            roughness: 0.3,
            anisotropy,
            clearcoat: 0.0,
            ..Default::default()
        };
        // Straight-on view, so the mirror direction is +Z and any asymmetry
        // is the lobe's own, not the geometry's.
        let wo = Vec3::new(0.0, 0.0, 1.0);
        // Two directions tilted off the mirror by the same angle: one along
        // the tangent (x), one along the bitangent (y).
        let along = Vec3::new(0.25, 0.0, 1.0).normalize();
        let across = Vec3::new(0.0, 0.25, 1.0).normalize();

        let (f_iso_a, _) = bsdf_eval(&rough(0.0), wo, along);
        let (f_iso_b, _) = bsdf_eval(&rough(0.0), wo, across);
        assert!(
            (f_iso_a[0] - f_iso_b[0]).abs() < 1e-6,
            "isotropic lobe must be rotationally symmetric"
        );

        let (f_pos_a, _) = bsdf_eval(&rough(0.8), wo, along);
        let (f_pos_b, _) = bsdf_eval(&rough(0.8), wo, across);
        assert!(
            f_pos_a[0] > f_pos_b[0] * 1.5,
            "positive anisotropy should spread energy along the tangent: \
             along {} vs across {}",
            f_pos_a[0],
            f_pos_b[0]
        );

        let (f_neg_a, _) = bsdf_eval(&rough(-0.8), wo, along);
        let (f_neg_b, _) = bsdf_eval(&rough(-0.8), wo, across);
        assert!(
            f_neg_b[0] > f_neg_a[0] * 1.5,
            "negative anisotropy should spread energy across the tangent: \
             along {} vs across {}",
            f_neg_a[0],
            f_neg_b[0]
        );
    }

    /// Root-mean-square error between two films, measured on the tonemapped
    /// display values rather than raw radiance.
    ///
    /// Linear-radiance RMSE on a path-traced frame is almost entirely a
    /// firefly metric — measured on this scene, the worst 1% of pixels carried
    /// 97% of the squared error, so the number mostly reports how many
    /// outliers the *reference* still has, not how clean the image looks. The
    /// tonemap is the transform the viewer sees through, and it is what makes
    /// this a measure of visible error.
    fn rmse(a: &Film, b: &Film) -> f32 {
        assert_eq!(a.rgb.len(), b.rgb.len());
        let s: f32 = a
            .rgb
            .iter()
            .zip(&b.rgb)
            .map(|(x, y)| {
                let d = tonemap_aces(*x) - tonemap_aces(*y);
                d * d
            })
            .sum();
        (s / a.rgb.len() as f32).sqrt()
    }

    /// The property that matters: denoising a noisy render must move it
    /// *closer to the truth*, not merely change it. A blur that smeared
    /// everything would also "change the output" while making the image
    /// worse, and this is the test that tells the two apart.
    #[test]
    fn denoise_moves_low_spp_toward_high_spp_reference() {
        let scene = test_scene();
        let cam = test_camera();
        // Big enough that the doubling stride is meaningful: at 28px a
        // 5-iteration à-trous reaches past the image edge and the later passes
        // can only over-blur, which made an earlier version of this test
        // report a 7% win where the real figure is ~60%.
        let (w, h) = (96, 96);

        let reference = render(
            &scene,
            &cam,
            w,
            h,
            &PathTraceOptions {
                spp: 1024,
                denoise: false,
                ..Default::default()
            },
        );
        let noisy = render(
            &scene,
            &cam,
            w,
            h,
            &PathTraceOptions {
                spp: 4,
                denoise: false,
                ..Default::default()
            },
        );
        let denoised = render(
            &scene,
            &cam,
            w,
            h,
            &PathTraceOptions {
                spp: 4,
                denoise: true,
                ..Default::default()
            },
        );

        // Denoising is a post-process, so the two 4-spp films must have come
        // from the very same samples.
        assert_eq!(
            noisy.alpha, denoised.alpha,
            "denoising perturbed the sampling"
        );

        let before = rmse(&noisy, &reference);
        let after = rmse(&denoised, &reference);
        eprintln!("RMSE vs 1024spp: noisy {before:.5} -> denoised {after:.5}");
        // Measured ~60% reduction; assert a conservative fraction of it so the
        // test pins real quality rather than just "something happened", without
        // being brittle to sampling changes upstream.
        assert!(
            after < before * 0.75,
            "denoising did not meaningfully improve the estimate: \
             RMSE {before} -> {after}"
        );
    }

    /// The denoiser must not blur across a silhouette. Background pixels are
    /// analytic and noise-free, so they must come through untouched, and no
    /// surface pixel may pick up any backdrop.
    #[test]
    fn denoise_preserves_silhouette_edge() {
        let scene = test_scene();
        let cam = test_camera();
        let (w, h) = (48, 48);
        let opts = PathTraceOptions {
            spp: 4,
            denoise: false,
            ..Default::default()
        };
        let raw = render(&scene, &cam, w, h, &opts);
        let mut filtered = render(&scene, &cam, w, h, &opts);
        denoise(
            &mut filtered,
            &PathTraceOptions {
                denoise: true,
                ..opts
            },
        );

        let n = (w * h) as usize;
        let bg: Vec<usize> = (0..n).filter(|&i| raw.depth[i] <= 0.0).collect();
        let fg: Vec<usize> = (0..n).filter(|&i| raw.depth[i] > 0.0).collect();
        assert!(
            !bg.is_empty() && !fg.is_empty(),
            "test framing must contain both subject and backdrop"
        );

        // Backdrop is bit-identical.
        for &i in &bg {
            for c in 0..3 {
                assert_eq!(
                    raw.rgb[i * 3 + c],
                    filtered.rgb[i * 3 + c],
                    "backdrop pixel {i} was modified by the denoiser"
                );
            }
        }

        // Silhouette contrast is retained. A filter that leaked across the
        // edge would pull the two sides toward each other.
        let mean_lum = |f: &Film, idx: &[usize]| -> f32 {
            let s: f32 = idx
                .iter()
                .map(|&i| luminance([f.rgb[i * 3], f.rgb[i * 3 + 1], f.rgb[i * 3 + 2]]))
                .sum();
            s / idx.len() as f32
        };
        // Only the surface pixels that actually touch the backdrop.
        let rim: Vec<usize> = fg
            .iter()
            .copied()
            .filter(|&i| {
                let (x, y) = ((i % w as usize) as i32, (i / w as usize) as i32);
                [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)]
                    .iter()
                    .any(|(dx, dy)| {
                        let (qx, qy) = (x + dx, y + dy);
                        qx >= 0
                            && qy >= 0
                            && qx < w as i32
                            && qy < h as i32
                            && raw.depth[qy as usize * w as usize + qx as usize] <= 0.0
                    })
            })
            .collect();
        assert!(!rim.is_empty(), "expected a silhouette rim");

        let before = (mean_lum(&raw, &rim) - mean_lum(&raw, &bg)).abs();
        let after = (mean_lum(&filtered, &rim) - mean_lum(&filtered, &bg)).abs();
        assert!(
            after >= before * 0.95,
            "silhouette contrast collapsed: {before} -> {after}"
        );
    }

    /// A white furnace test: with no lights and a uniform environment, a
    /// pure-white rough dielectric must not create or destroy much energy.
    #[test]
    fn furnace_conserves_energy_roughly() {
        // Anisotropy redistributes energy within the lobe; it must not
        // create or destroy any. Grazing views are included because that is
        // where a wrong anisotropic masking term shows up as gain.
        for aniso in [0.0, 0.5, 0.9, -0.5, -0.9] {
            for wo in [
                Vec3::new(0.0, 0.0, 1.0),
                Vec3::new(0.6, 0.2, 0.77).normalize(),
            ] {
                let m = Pbr {
                    base_color: [1.0, 1.0, 1.0],
                    metallic: 0.0,
                    roughness: 0.5,
                    anisotropy: aniso,
                    clearcoat: 0.0,
                    ..Default::default()
                };
                let mut rng = Rng::new(11);
                let n = 20000;
                let mut sum = 0.0f32;
                for _ in 0..n {
                    if let Some((_wi, f, pdf)) = bsdf_sample(&m, wo, &mut rng) {
                        sum += f[0] / pdf;
                    }
                }
                let albedo = sum / n as f32;
                assert!(
                    (0.75..=1.05).contains(&albedo),
                    "directional albedo {albedo} outside plausible range \
                     (anisotropy {aniso}, wo {wo:?})"
                );
            }
        }
    }

    // ── environment maps ──────────────────────────────────────────────────

    /// A map of constant radiance `c`.
    fn uniform_map(w: usize, h: usize, c: f32) -> EnvMap {
        EnvMap::new(w, h, vec![[c, c, c]; w * h]).expect("uniform map")
    }

    /// A deliberately high-frequency map: a dim surround with one small,
    /// very bright patch — the case BSDF-only sampling handles badly and the
    /// CDF exists for.
    fn structured_map() -> EnvMap {
        let (w, h) = (64usize, 32usize);
        let mut px = vec![[0.05f32, 0.06, 0.08]; w * h];
        for j in 6..10 {
            for i in 20..25 {
                px[j * w + i] = [40.0, 38.0, 34.0];
            }
        }
        // A second, low patch near the horizon on the far side.
        for j in 16..18 {
            for i in 50..56 {
                px[j * w + i] = [6.0, 6.5, 8.0];
            }
        }
        EnvMap::new(w, h, px).expect("structured map")
    }

    /// Uniform directions on the sphere, for reference integration.
    fn uniform_sphere(r1: f64, r2: f64) -> Vec3 {
        let z = 1.0 - 2.0 * r1;
        let r = (1.0 - z * z).max(0.0).sqrt();
        let phi = std::f64::consts::TAU * r2;
        Vec3::new(r * phi.cos(), r * phi.sin(), z)
    }

    /// The single strongest guard on the PDF conversion: a density over the
    /// sphere must integrate to 1. A wrong `2*pi^2`, or a forgotten
    /// `sin(theta)`, shows up here immediately.
    #[test]
    fn env_pdf_integrates_to_one_over_the_sphere() {
        for map in [uniform_map(32, 16, 1.0), structured_map()] {
            let mut rng = Rng::new(3);
            let n = 200_000;
            let mut sum = 0.0f64;
            for _ in 0..n {
                let d = uniform_sphere(rng.f64(), rng.f64());
                // Uniform-sphere pdf is 1/4pi, so the estimator is 4pi * mean.
                sum += map.pdf(d) as f64;
            }
            let integral = sum / n as f64 * 4.0 * std::f64::consts::PI;
            assert!(
                (integral - 1.0).abs() < 0.02,
                "environment PDF integrates to {integral}, expected 1"
            );
        }
    }

    /// White furnace, at the estimator level: with a uniform environment of
    /// radiance `c`, importance sampling must recover the analytic
    /// irradiance `pi * c` over a hemisphere. This is the test that catches
    /// a PDF-conversion constant that merely *looks* plausible in an image.
    #[test]
    fn uniform_env_sampling_recovers_irradiance() {
        let c = 0.75f32;
        let map = uniform_map(32, 16, c);
        let n = 100_000;
        let mut rng = Rng::new(5);
        let mut sum = 0.0f64;
        for _ in 0..n {
            let Some((d, li, pdf)) = map.sample(rng.f64(), rng.f64()) else {
                continue;
            };
            if d.z <= 0.0 {
                continue;
            }
            sum += (li[0] as f64) * d.z / pdf as f64;
        }
        let irradiance = sum / n as f64;
        let expected = std::f64::consts::PI * c as f64;
        assert!(
            (irradiance - expected).abs() < 0.02 * expected,
            "irradiance {irradiance}, expected {expected}"
        );
    }

    /// The same integral, on a high-frequency map, estimated two ways. They
    /// must agree — importance sampling may only change the variance, never
    /// the answer.
    #[test]
    fn structured_env_sampling_agrees_with_uniform_sphere_sampling() {
        let map = structured_map();
        let n = 400_000;

        let mut rng = Rng::new(9);
        let mut is_sum = 0.0f64;
        for _ in 0..n {
            if let Some((d, li, pdf)) = map.sample(rng.f64(), rng.f64()) {
                if d.z > 0.0 {
                    is_sum += (li[0] as f64) * d.z / pdf as f64;
                }
            }
        }
        let importance = is_sum / n as f64;

        let mut rng = Rng::new(10);
        let mut u_sum = 0.0f64;
        let uniform_pdf = 1.0 / (4.0 * std::f64::consts::PI);
        for _ in 0..n {
            let d = uniform_sphere(rng.f64(), rng.f64());
            if d.z > 0.0 {
                u_sum += (map.radiance(d)[0] as f64) * d.z / uniform_pdf;
            }
        }
        let reference = u_sum / n as f64;

        assert!(
            (importance - reference).abs() < 0.05 * reference,
            "importance-sampled irradiance {importance} disagrees with \
             uniform-sampled reference {reference}"
        );
    }

    /// Rotation must actually move the environment, and must not disturb the
    /// PDF normalisation (the CDF is reused across rotations).
    #[test]
    fn rotation_moves_the_environment_without_breaking_the_pdf() {
        let map = structured_map();
        let spun = structured_map().with_rotation_deg(90.0);
        // Aim straight at the bright patch in the unrotated map; spinning the
        // environment must move it out from under this direction.
        let d = map.direction(0.34, 0.24);
        assert!(
            map.radiance(d)[0] > 10.0,
            "probe direction missed the bright patch"
        );
        assert!(
            (map.radiance(d)[0] - spun.radiance(d)[0]).abs() > 1e-6,
            "rotating the environment changed nothing"
        );

        let mut rng = Rng::new(21);
        let n = 200_000;
        let mut sum = 0.0f64;
        for _ in 0..n {
            sum += spun.pdf(uniform_sphere(rng.f64(), rng.f64())) as f64;
        }
        let integral = sum / n as f64 * 4.0 * std::f64::consts::PI;
        assert!(
            (integral - 1.0).abs() < 0.02,
            "rotated environment PDF integrates to {integral}"
        );
    }

    /// A degenerate (all-black) map must not be importance-sampled, and must
    /// not poison the MIS weights.
    #[test]
    fn black_env_map_is_not_importance_sampled() {
        let env = Environment::image(uniform_map(8, 4, 0.0));
        assert!(!env.is_importance_sampled());
        assert_eq!(env.pdf(Vec3::new(0.0, 0.0, 1.0)), 0.0);
        assert!(env.sample(0.5, 0.5).is_none());
    }

    /// End-to-end white furnace: a uniform HDRI and the analytic environment
    /// set to the same constant colour must render to the same image, even
    /// though one is integrated by BSDF sampling alone and the other by a
    /// three-way MIS mix. Any error in the image-space to solid-angle PDF
    /// conversion shows up as a systematic brightness difference here.
    #[test]
    fn uniform_env_map_matches_analytic_constant_environment() {
        let c = 0.6f32;
        let cube = make_cube(10.0, 10.0, 10.0);
        let material = Pbr {
            base_color: [1.0, 1.0, 1.0],
            metallic: 0.0,
            roughness: 0.6,
            clearcoat: 0.0,
            ..Default::default()
        };
        let scene_with = |env: Environment| Scene {
            objects: vec![Object::new(Arc::new(Bvh::build(&cube)), material)],
            // No area lights: the environment must be the only illuminant,
            // or light sampling would mask a bad environment PDF.
            lights: Vec::new(),
            env,
            ground: None,
        };
        let cam = test_camera();
        let opts = PathTraceOptions {
            spp: 220,
            max_depth: 4,
            firefly_clamp: None,
            ..Default::default()
        };

        let mean = |scene: &Scene| -> f64 {
            let film = render(scene, &cam, 24, 24, &opts);
            film.rgb.iter().map(|v| *v as f64).sum::<f64>() / film.rgb.len() as f64
        };

        let analytic = mean(&scene_with(Environment::constant([c, c, c])));
        let image = mean(&scene_with(Environment::image(uniform_map(64, 32, c))));

        assert!(analytic > 0.1, "reference render was black");
        assert!(
            (image - analytic).abs() < 0.02 * analytic,
            "uniform HDRI rendered at {image}, analytic constant environment \
             at {analytic} — the environment PDF conversion is off"
        );
    }
}
