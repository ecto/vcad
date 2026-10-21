//! `vcad render --photoreal`: headless path tracing.
//!
//! This is the same renderer the app's photoreal viewport uses — the WGSL
//! path tracer in `vcad-kernel-gpu` (one shared `bsdf.wgsl`), the studio
//! softbox rig, the HDRI/gradient environment, the document's PBR materials
//! and the ACES tonemap — driven through `vcad_render::photoreal_gpu` on a
//! headless wgpu device (no surface, no window). `--cpu` selects the CPU
//! reference integrator in `vcad_render::photoreal`, which shares the BSDF
//! (the parity test pins them together) and adds the edge-aware denoiser.

use anyhow::{anyhow, bail, Result};
use std::path::Path;
use vcad_render::photoreal::{Backdrop, PhotorealOptions};
use vcad_render::{RasterOptions, View};

/// Everything `vcad render --photoreal` was asked for.
#[derive(Debug, Clone)]
pub struct PhotorealRequest {
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// Camera direction.
    pub view: View,
    /// Samples per pixel.
    pub samples: u32,
    /// Maximum bounces.
    pub max_depth: u32,
    /// Exposure multiplier before the tonemap.
    pub exposure: f64,
    /// `gradient` or a path to an equirectangular `.hdr`.
    pub env: String,
    /// Sampling seed.
    pub seed: u64,
    /// Trace on the CPU reference integrator instead of the GPU.
    pub cpu: bool,
    /// Thin-lens aperture radius as a fraction of the scene radius (0 = pinhole).
    pub aperture: f64,
    /// Plane-of-focus distance in mm; `None` focuses on the framed subject.
    pub focus_dist: Option<f64>,
    /// Frame (and so focus) on this part instead of the whole document.
    pub focus_part: Option<String>,
    /// Light rig (`studio` or `warm-studio`).
    pub lighting: vcad_render::photoreal::Lighting,
    /// Seamless cyclorama backdrop instead of the infinite floor.
    pub cyclorama: bool,
}

/// Parse `WxH` (e.g. `1600x1000`).
pub fn parse_size(s: &str) -> Result<(u32, u32)> {
    let (w, h) = s
        .to_ascii_lowercase()
        .split_once('x')
        .map(|(w, h)| (w.trim().to_string(), h.trim().to_string()))
        .ok_or_else(|| anyhow!("bad --size '{s}' (expected WxH, e.g. 1600x1000)"))?;
    let w: u32 = w
        .parse()
        .map_err(|_| anyhow!("bad --size width in '{s}'"))?;
    let h: u32 = h
        .parse()
        .map_err(|_| anyhow!("bad --size height in '{s}'"))?;
    if w == 0 || h == 0 || w > 16384 || h > 16384 {
        bail!("--size {w}x{h} out of range (1..=16384 per side)");
    }
    Ok((w, h))
}

/// Resolve the camera: `--preset` wins, then `--camera az,el`, then iso.
pub fn resolve_view(preset: Option<&str>, camera: Option<&str>) -> Result<View> {
    if let Some(p) = preset {
        return p.parse::<View>().map_err(|e| anyhow!("--preset: {e}"));
    }
    if let Some(c) = camera {
        return format!("orbit:{c}").parse::<View>().map_err(|_| {
            anyhow!("bad --camera '{c}' (expected <azimuth>,<elevation> in degrees)")
        });
    }
    Ok(View::Isometric)
}

fn options(req: &PhotorealRequest) -> (RasterOptions, PhotorealOptions) {
    let ro = RasterOptions {
        view: req.view,
        size_px: req.width,
        height_px: Some(req.height),
        focus: req.focus_part.clone(),
        ..RasterOptions::default()
    };
    let pr = PhotorealOptions {
        environment: vcad_render::envmap::parse_env_arg(&req.env),
        env_rotation_deg: 0.0,
        spp: req.samples,
        max_depth: req.max_depth,
        exposure: req.exposure as f32,
        fov_deg: 34.0,
        orthographic: false,
        aperture_frac: req.aperture,
        focus_dist: req.focus_dist,
        backdrop: if req.cyclorama {
            Backdrop::Cyclorama
        } else {
            Backdrop::Studio
        },
        lighting: req.lighting,
        seed: req.seed,
        // The GPU tracer reads back radiance only, so the guided denoiser has
        // nothing to steer by; the CPU path keeps it.
        denoise: req.cpu,
        adaptive: true,
        exact: false,
    };
    (ro, pr)
}

/// Render raw `.vcad` JSON to an encoded image (PNG unless `jpeg`).
pub fn render_bytes(raw_vcad: &str, req: &PhotorealRequest, jpeg: bool) -> Result<Vec<u8>> {
    let (ro, pr) = options(req);
    // The GPU camera is a pinhole; depth of field is a CPU-integrator
    // feature, so an aperture routes there rather than being refused.
    if !req.cpu && req.aperture > 0.0 {
        let cpu = PhotorealRequest {
            cpu: true,
            ..req.clone()
        };
        return render_bytes(raw_vcad, &cpu, jpeg);
    }
    let out = if req.cpu {
        if jpeg {
            vcad_render::photoreal::render_photoreal_jpeg_str(raw_vcad, &ro, &pr)
        } else {
            vcad_render::photoreal::render_photoreal_png_str(raw_vcad, &ro, &pr)
        }
    } else {
        vcad_render::photoreal_gpu::check_supported(&ro, &pr).map_err(|e| anyhow!(e))?;
        let gpu = if jpeg {
            vcad_render::photoreal_gpu::render_photoreal_gpu_jpeg_str(raw_vcad, &ro, &pr)
        } else {
            vcad_render::photoreal_gpu::render_photoreal_gpu_png_str(raw_vcad, &ro, &pr)
        };
        // No adapter, or a driver that cannot build the tracer: the CPU
        // integrator renders the same scene, so say so and use it rather
        // than failing the command.
        match gpu {
            Err(e) if gpu_unavailable(&e) => {
                eprintln!("photoreal: GPU unavailable ({e}); rendering on the CPU instead");
                let cpu = PhotorealRequest {
                    cpu: true,
                    ..req.clone()
                };
                FELL_BACK_TO_CPU.store(true, std::sync::atomic::Ordering::Relaxed);
                return render_bytes(raw_vcad, &cpu, jpeg);
            }
            other => other,
        }
    };
    out.map_err(|e| anyhow!("photoreal render failed: {e}"))
}

/// Set when a GPU render fell back to the CPU, so the summary line reports
/// the integrator that actually ran.
static FELL_BACK_TO_CPU: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether a GPU render error means "no usable GPU here" rather than a
/// problem with the scene.
fn gpu_unavailable(e: &str) -> bool {
    e.contains("no compatible GPU adapter")
        || e.contains("could not initialise the GPU")
        || e.contains("device was lost")
}

/// Render a loaded document to `out`.
pub fn render_document(doc: &vcad_ir::Document, out: &Path, req: &PhotorealRequest) -> Result<()> {
    let raw = doc.to_json()?;
    let jpeg = matches!(
        out.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
        Some("jpg" | "jpeg")
    );
    let t0 = std::time::Instant::now();
    let bytes = render_bytes(&raw, req, jpeg)?;
    std::fs::write(out, bytes)?;
    eprintln!(
        "photoreal ({}) {}x{} @ {} spp -> {} in {:.1}s",
        if req.cpu || FELL_BACK_TO_CPU.load(std::sync::atomic::Ordering::Relaxed) {
            "cpu"
        } else {
            "gpu"
        },
        req.width,
        req.height,
        req.samples,
        out.display(),
        t0.elapsed().as_secs_f64()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_and_view_parse() {
        assert_eq!(parse_size("640x480").unwrap(), (640, 480));
        assert!(parse_size("640").is_err());
        assert_eq!(
            resolve_view(Some("front"), Some("10,20")).unwrap(),
            View::Front
        );
        assert_eq!(
            resolve_view(None, Some("10,20")).unwrap(),
            View::Orbit {
                azimuth: 10.0,
                elevation: 20.0
            }
        );
        assert_eq!(resolve_view(None, None).unwrap(), View::Isometric);
    }

    const TINY: &str = r#"{"version":"0.1","nodes":{"1":{"id":1,"name":null,"op":{"type":"Cube","size":{"x":10,"y":10,"z":10}}}},"materials":{"red":{"name":"red","color":[0.8,0.1,0.1],"metallic":0.0,"roughness":0.5}},"part_materials":{},"roots":[{"root":1,"material":"red"}]}"#;

    fn tiny_req(cpu: bool) -> PhotorealRequest {
        PhotorealRequest {
            width: 48,
            height: 32,
            view: View::Isometric,
            samples: 4,
            max_depth: 3,
            exposure: 1.0,
            env: "gradient".into(),
            seed: 7,
            cpu,
            aperture: 0.0,
            focus_dist: None,
            focus_part: None,
            lighting: Default::default(),
            cyclorama: false,
        }
    }

    /// A red cube must render red-dominant (not flat grey), and the same seed
    /// must give the same bytes. Runs on the CPU integrator so it holds on
    /// GPU-less CI.
    #[test]
    fn tiny_scene_is_coloured_and_deterministic() {
        let a = render_bytes(TINY, &tiny_req(true), false).expect("render");
        let b = render_bytes(TINY, &tiny_req(true), false).expect("render");
        assert_eq!(a, b, "same seed must reproduce the same image");
        let img = image::load_from_memory(&a).unwrap().to_rgba8();
        let reddish = img
            .pixels()
            .filter(|p| p[0] as i32 > p[1] as i32 + 30)
            .count();
        assert!(reddish > 20, "expected red cube pixels, got {reddish}");
    }

    /// Same check through the headless GPU tracer. Skips (passes with a note)
    /// when the machine has no wgpu adapter, e.g. GPU-less CI runners.
    #[test]
    fn tiny_scene_gpu_when_available() {
        let a = match render_bytes(TINY, &tiny_req(false), false) {
            Ok(a) => a,
            Err(e) if e.to_string().to_ascii_lowercase().contains("adapter") => {
                eprintln!("skipping: {e}");
                return;
            }
            Err(e) => panic!("{e}"),
        };
        let img = image::load_from_memory(&a).unwrap().to_rgba8();
        let reddish = img
            .pixels()
            .filter(|p| p[0] as i32 > p[1] as i32 + 30)
            .count();
        assert!(reddish > 20, "expected red cube pixels, got {reddish}");
    }
}
