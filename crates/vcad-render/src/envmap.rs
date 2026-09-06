//! Environment selection for the photoreal path: the analytic gradient, a
//! built-in studio HDRI, or a lat-long `.hdr` file from disk.
//!
//! What is left here is the CLI shape of the choice. The environments
//! themselves — the synthesised studio maps and the Radiance RGBE reader —
//! moved to [`vcad_kernel_raytrace::env`] (which is `kosm_render::env`),
//! because neither knew what a `.vcad` document was. Parsing an `--env`
//! argument does: `gradient`, a built-in's name, or a path is a fact about
//! this binary's command line and nothing else's.

use std::path::{Path, PathBuf};

use vcad_kernel_raytrace::pathtrace::{EnvMap, Environment};

/// The generated studio environments, and the code that generates them.
pub use vcad_kernel_raytrace::env::{generate, BuiltinEnv};

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

/// Load a lat-long Radiance `.hdr` (RGBE) file as an environment map.
///
/// The image is taken as equirectangular with row 0 at the zenith, which is
/// the convention every HDRI library ships.
pub fn load_hdr(path: &Path) -> Result<EnvMap, String> {
    vcad_kernel_raytrace::env::load_hdr(path)
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

    #[test]
    fn resolve_gradient_is_the_analytic_environment() {
        let env = resolve(&EnvSource::Gradient, 0.0).unwrap();
        assert!(matches!(env, Environment::Gradient(_)));
    }

    /// The built-ins still resolve through this crate, whoever generates
    /// them: a rotation is applied and the map is sampleable.
    #[test]
    fn resolve_builtin_is_an_image_environment() {
        for kind in BuiltinEnv::all() {
            let env = resolve(&EnvSource::Builtin(kind), 45.0).unwrap();
            assert!(matches!(env, Environment::Image(_)), "{}", kind.name());
        }
    }

    #[test]
    fn missing_hdr_file_is_a_clean_error() {
        let err = resolve(&EnvSource::File(PathBuf::from("/nope/missing.hdr")), 0.0)
            .expect_err("missing file must fail");
        assert!(err.contains("missing.hdr"), "unhelpful error: {err}");
    }
}
