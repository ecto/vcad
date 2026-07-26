//! Environment selection for the photoreal path: the analytic gradient, a
//! built-in studio HDRI, or a lat-long `.hdr` file from disk.
//!
//! # Why the built-ins are generated rather than shipped
//!
//! The obvious move is to vendor a couple of Poly Haven CC0 HDRIs. These are
//! synthesised instead, for three reasons: the repo carries no binary blobs
//! and no third-party licence to track, the maps are exactly as
//! high-frequency as the sampler needs to be exercised (crisp softbox discs,
//! a hot rim), and a studio environment is a handful of soft rectangles in a
//! dim room — precisely the thing that is cheaper to describe than to store.
//! Real-world HDRIs are fully supported via `--env <path.hdr>`, which is the
//! path any Poly Haven download takes.

use std::path::{Path, PathBuf};

use vcad_kernel::vcad_kernel_math::Vec3;
use vcad_kernel_raytrace::pathtrace::{EnvMap, Environment};

/// Resolution of the generated built-in maps. Enough to keep the softbox
/// edges clean without making CDF construction show up in a profile.
const BUILTIN_W: usize = 256;
const BUILTIN_H: usize = 128;

/// Sin-weighted mean luminance the built-ins are normalised to, so switching
/// between them changes the *look* and not the exposure.
const BUILTIN_MEAN: f32 = 0.30;

/// Which environment lights the scene.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum EnvSource {
    /// The analytic studio gradient plus the softbox rig — the default.
    #[default]
    Gradient,
    /// One of the generated studio HDRIs.
    Builtin(BuiltinEnv),
    /// A lat-long Radiance `.hdr` file.
    File(PathBuf),
}

/// The generated studio environments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinEnv {
    /// Neutral three-light studio: broad key, cool fill, hot rim.
    Studio,
    /// Two crisp softboxes against near-black — a product-shot look with
    /// strong specular shapes.
    Softbox,
    /// Bright, even overcast dome. Low contrast, flattering to complex parts.
    Overcast,
}

impl BuiltinEnv {
    /// CLI spelling.
    pub fn name(self) -> &'static str {
        match self {
            BuiltinEnv::Studio => "studio",
            BuiltinEnv::Softbox => "softbox",
            BuiltinEnv::Overcast => "overcast",
        }
    }

    /// Every built-in, for help text and tests.
    pub fn all() -> [BuiltinEnv; 3] {
        [
            BuiltinEnv::Studio,
            BuiltinEnv::Softbox,
            BuiltinEnv::Overcast,
        ]
    }

    fn parse(s: &str) -> Option<Self> {
        BuiltinEnv::all().into_iter().find(|b| b.name() == s)
    }
}

/// Interpret an `--env` argument: `gradient`, a built-in name, or a path.
pub fn parse_env_arg(s: &str) -> EnvSource {
    if s.eq_ignore_ascii_case("gradient") || s.is_empty() {
        return EnvSource::Gradient;
    }
    match BuiltinEnv::parse(&s.to_ascii_lowercase()) {
        Some(b) => EnvSource::Builtin(b),
        None => EnvSource::File(PathBuf::from(s)),
    }
}

/// Build the environment the path tracer should use.
pub fn resolve(src: &EnvSource, rotation_deg: f64) -> Result<Environment, String> {
    match src {
        EnvSource::Gradient => Ok(Environment::default()),
        EnvSource::Builtin(b) => Ok(Environment::image(
            generate(*b).with_rotation_deg(rotation_deg),
        )),
        EnvSource::File(p) => Ok(Environment::image(
            load_hdr(p)?.with_rotation_deg(rotation_deg),
        )),
    }
}

// ── loading ───────────────────────────────────────────────────────────────

/// Load a lat-long Radiance `.hdr` (RGBE) file as an environment map.
///
/// The image is taken as equirectangular with row 0 at the zenith, which is
/// the convention every HDRI library ships.
pub fn load_hdr(path: &Path) -> Result<EnvMap, String> {
    use image::codecs::hdr::HdrDecoder;
    use image::DynamicImage;
    use std::fs::File;
    use std::io::BufReader;

    let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let decoder = HdrDecoder::new(BufReader::new(file))
        .map_err(|e| format!("{}: not a Radiance .hdr file ({e})", path.display()))?;
    let img = DynamicImage::from_decoder(decoder)
        .map_err(|e| format!("{}: could not decode ({e})", path.display()))?;
    let rgb = img.to_rgb32f();
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);

    // Lat-long maps are 2:1. Warn-by-refusing would be unhelpful (croppped
    // and cube-cross maps exist), but a wildly wrong aspect is worth naming.
    if w < 2 || h < 1 {
        return Err(format!("{}: environment map too small", path.display()));
    }

    let pixels: Vec<[f32; 3]> = rgb.pixels().map(|p| [p[0], p[1], p[2]]).collect();
    EnvMap::new(w, h, pixels).map_err(|e| format!("{}: {e}", path.display()))
}

// ── generated studio maps ─────────────────────────────────────────────────

/// A soft-edged disc of light centred on `dir`, of angular radius `radius`
/// radians.
fn disc(d: Vec3, dir: Vec3, radius: f64, radiance: [f32; 3]) -> [f32; 3] {
    let a = d.dot(dir.normalize()).clamp(-1.0, 1.0).acos();
    if a >= radius {
        return [0.0; 3];
    }
    let t = (1.0 - a / radius) as f32;
    let k = t * t * (3.0 - 2.0 * t);
    [radiance[0] * k, radiance[1] * k, radiance[2] * k]
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

/// Radiance of a built-in environment in direction `d`, before normalisation.
fn builtin_radiance(kind: BuiltinEnv, d: Vec3) -> [f32; 3] {
    let z = d.z as f32;
    match kind {
        BuiltinEnv::Studio => {
            // Dim room: slightly cool above, warm floor bounce below.
            let base = if z >= 0.0 {
                let k = z.powf(0.7);
                [0.14 + 0.05 * k, 0.15 + 0.07 * k, 0.17 + 0.11 * k]
            } else {
                let k = (-z).powf(0.5);
                [0.14 - 0.08 * k, 0.135 - 0.08 * k, 0.13 - 0.08 * k]
            };
            let key = disc(d, Vec3::new(-0.8, -1.0, 1.1), 0.34, [26.0, 25.0, 23.5]);
            let fill = disc(d, Vec3::new(1.3, -0.6, 0.25), 0.50, [2.6, 2.9, 3.5]);
            let rim = disc(d, Vec3::new(0.35, 1.25, 0.8), 0.13, [60.0, 59.0, 57.0]);
            add(add(base, key), add(fill, rim))
        }
        BuiltinEnv::Softbox => {
            // Near-black surround so the specular shapes read hard.
            let base = if z >= 0.0 {
                [0.020, 0.021, 0.024]
            } else {
                [0.012, 0.012, 0.013]
            };
            let a = disc(d, Vec3::new(-0.7, -1.0, 0.5), 0.30, [55.0, 54.0, 52.0]);
            let b = disc(d, Vec3::new(0.9, -0.9, 0.35), 0.20, [22.0, 23.0, 26.0]);
            let c = disc(d, Vec3::new(0.1, 1.1, 0.55), 0.10, [70.0, 69.0, 68.0]);
            add(add(base, a), add(b, c))
        }
        BuiltinEnv::Overcast => {
            let base = if z >= 0.0 {
                let k = z.powf(0.6);
                [0.55 + 0.45 * k, 0.57 + 0.47 * k, 0.60 + 0.50 * k]
            } else {
                let k = (-z).powf(0.5);
                [0.30 - 0.18 * k, 0.29 - 0.17 * k, 0.28 - 0.16 * k]
            };
            // A brighter break in the cloud, so there is something for the
            // CDF to find and for gloss to catch.
            let sun = disc(d, Vec3::new(-0.4, -0.7, 1.0), 0.45, [3.2, 3.2, 3.1]);
            add(base, sun)
        }
    }
}

/// Generate a built-in map, normalised to a common mean radiance.
pub fn generate(kind: BuiltinEnv) -> EnvMap {
    let (w, h) = (BUILTIN_W, BUILTIN_H);
    let mut pixels = vec![[0.0f32; 3]; w * h];
    // Solid-angle-weighted mean, so normalisation is over the sphere and not
    // over image area (which would over-count the poles).
    let mut weighted = 0.0f64;
    let mut weight = 0.0f64;

    for j in 0..h {
        let theta = std::f64::consts::PI * (j as f64 + 0.5) / h as f64;
        let (st, ct) = theta.sin_cos();
        for i in 0..w {
            let phi = std::f64::consts::TAU * (i as f64 + 0.5) / w as f64;
            let d = Vec3::new(st * phi.cos(), st * phi.sin(), ct);
            let c = builtin_radiance(kind, d);
            pixels[j * w + i] = c;
            let lum = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
            weighted += lum as f64 * st;
            weight += st;
        }
    }

    let mean = if weight > 0.0 {
        (weighted / weight) as f32
    } else {
        0.0
    };
    if mean > 0.0 {
        let k = BUILTIN_MEAN / mean;
        for p in &mut pixels {
            p[0] *= k;
            p[1] *= k;
            p[2] *= k;
        }
    }

    EnvMap::new(w, h, pixels).expect("built-in environment dimensions are valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_names_and_paths() {
        assert_eq!(parse_env_arg("gradient"), EnvSource::Gradient);
        assert_eq!(
            parse_env_arg("studio"),
            EnvSource::Builtin(BuiltinEnv::Studio)
        );
        assert_eq!(
            parse_env_arg("Softbox"),
            EnvSource::Builtin(BuiltinEnv::Softbox)
        );
        assert_eq!(
            parse_env_arg("/tmp/kloppenheim.hdr"),
            EnvSource::File(PathBuf::from("/tmp/kloppenheim.hdr"))
        );
    }

    /// Every built-in must be sampleable and normalised to the same exposure,
    /// or switching environments would silently change image brightness.
    #[test]
    fn builtins_share_an_exposure() {
        for kind in BuiltinEnv::all() {
            let map = generate(kind);
            // Sin-weighted mean, recomputed from the finished map.
            let mut weighted = 0.0f64;
            let mut weight = 0.0f64;
            let n = 200;
            for j in 0..n {
                let theta = std::f64::consts::PI * (j as f64 + 0.5) / n as f64;
                let (st, ct) = theta.sin_cos();
                for i in 0..n {
                    let phi = std::f64::consts::TAU * (i as f64 + 0.5) / n as f64;
                    let d = Vec3::new(st * phi.cos(), st * phi.sin(), ct);
                    let c = map.radiance(d);
                    let lum = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
                    weighted += lum as f64 * st;
                    weight += st;
                }
            }
            let mean = weighted / weight;
            assert!(
                (mean - BUILTIN_MEAN as f64).abs() < 0.05,
                "{} has mean radiance {mean}, expected ~{BUILTIN_MEAN}",
                kind.name()
            );
        }
    }

    /// The built-ins exist to exercise the importance sampler, so they must
    /// actually be high-frequency: far brighter somewhere than on average.
    #[test]
    fn builtins_are_high_frequency() {
        for kind in BuiltinEnv::all() {
            let map = generate(kind);
            let mut peak = 0.0f32;
            let n = 240;
            for j in 0..n {
                let theta = std::f64::consts::PI * (j as f64 + 0.5) / n as f64;
                let (st, ct) = theta.sin_cos();
                for i in 0..n {
                    let phi = std::f64::consts::TAU * (i as f64 + 0.5) / n as f64;
                    let d = Vec3::new(st * phi.cos(), st * phi.sin(), ct);
                    peak = peak.max(map.radiance(d)[0]);
                }
            }
            assert!(
                peak > 4.0 * BUILTIN_MEAN,
                "{} peak radiance {peak} is too flat to need a CDF",
                kind.name()
            );
        }
    }

    #[test]
    fn resolve_gradient_is_the_analytic_environment() {
        let env = resolve(&EnvSource::Gradient, 0.0).unwrap();
        assert!(matches!(env, Environment::Gradient(_)));
    }

    #[test]
    fn missing_hdr_file_is_a_clean_error() {
        let err = resolve(&EnvSource::File(PathBuf::from("/nope/missing.hdr")), 0.0)
            .expect_err("missing file must fail");
        assert!(err.contains("missing.hdr"), "unhelpful error: {err}");
    }
}
