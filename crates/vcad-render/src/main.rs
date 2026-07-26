//! `vcad-render` CLI — project `.vcad` documents (or the `.loon` source they
//! are built from) to static line art or a raster render.
//!
//! Inputs are dispatched on extension: `.vcad` parses as IR JSON, `.loon`
//! evaluates through `vcad-loon` first, with `[use ...]` module imports
//! resolved against the input file's own directory. Everything downstream is
//! identical, so every flag below works on either.
//!
//! Usage:
//!   vcad-render <path.vcad|path.loon> [--view iso|front|side|top|hero|orbit:AZ,EL] [--scale <px-per-mm>] [--transparent]
//!               [--section x=N|y=N|z=N] [--axes] [--labels] [--dims]
//!   vcad-render <path.vcad> [--azimuth <deg>] [--elevation <deg>] [--focus <part-name>]
//!   vcad-render <path.vcad> -o out.jpg [--view ...] [--size <N|WxH>] [--fill <frac>] [--quality <1-100>]
//!   vcad-render <path.vcad> -o out.png [--auto-aspect] [--trim [--trim-margin <px>]]
//!   vcad-render <path.vcad> -o out.png [--raytrace]   # RGBA raster, transparent background
//!   vcad-render <path.vcad> --sheet [--size <sheet-width-px>] [-o out.svg|out.jpg]
//!   vcad-render <dir-or-paths...> [--out-dir <dir>] [--format svg|jpeg|png]
//!
//! With a single input and no output flag, a self-contained `<svg>` goes to
//! stdout. `-o <path>` picks the format from the extension (`.svg`, `.jpg`,
//! `.jpeg`, `.png`); `-o -` writes SVG to stdout. `--jpeg <path>` is the
//! legacy spelling of `-o <path.jpg>`. PNG output is RGBA with a transparent
//! background. `--raytrace` swaps the tessellated raster path for a
//! pixel-perfect direct-BRep ray trace (exact curved silhouettes, no
//! tessellation); it needs a raster output (`.png`/`.jpg`). `--sheet` emits
//! a multi-view drawing sheet (front/side/top/iso in third-angle
//! arrangement at one shared scale, with a title block) instead of a single
//! view, as SVG or JPEG. Multiple inputs
//! (or a directory, which expands to its `*.vcad` files) render in batch,
//! each to a sibling output file or into `--out-dir`; a per-file failure is
//! reported but does not abort the batch. All rendering logic lives in the
//! `vcad-render` library (see `lib.rs`); this binary only handles argument
//! parsing and file IO.
//!
//! `--azimuth`/`--elevation` select an arbitrary orthographic orbit camera
//! (degrees, Z-up: azimuth CCW from +X, elevation above the XY plane) and
//! override `--view`. `--focus` frames the render on the named part's
//! bounding box instead of the whole document. `--section` renders a cutaway:
//! the half of the model on the camera's side of the plane is removed and the
//! exposed cut faces are cross-hatched.
//!
//! Raster framing: `--size` takes `N` (square) or `WxH`, `--auto-aspect`
//! fits the canvas to the subject's projected aspect ratio, and `--trim`
//! crops the output to the drawn content (exact on PNG, whose background is
//! transparent). Together they keep a tall or wide part from rendering as a
//! thin ribbon of content in a mostly empty square.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::str::FromStr;

use clap::{Parser, ValueEnum};
use vcad_render::{
    render_svg_str_opts, RenderAnnotations, SectionPlane, SvgOptions, View, DEFAULT_SCALE,
};

/// Default overall width of a `--sheet` render when `--size` is unset.
/// Larger than the single-view raster default: a sheet holds four views
/// plus a title block, so it needs the room.
const SHEET_DEFAULT_WIDTH_PX: u32 = 1600;

/// `--size` value: `N` for a square N×N canvas, or `WxH` for a canvas
/// matched to the subject (a tall part in a square frame wastes most of its
/// pixels on background).
#[derive(Debug, Clone, Copy)]
struct SizeArg {
    width: u32,
    height: Option<u32>,
}

impl FromStr for SizeArg {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let px = |t: &str| {
            t.trim()
                .parse::<u32>()
                .map_err(|_| format!("--size expects `N` or `WxH`, got '{s}'"))
        };
        match s.split_once(['x', 'X']) {
            Some((w, h)) => Ok(SizeArg {
                width: px(w)?,
                height: Some(px(h)?),
            }),
            None => Ok(SizeArg {
                width: px(s)?,
                height: None,
            }),
        }
    }
}

/// Output format for a render. Single-file `-o` infers this from the output
/// extension; batch mode takes it from `--format`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Format {
    /// Drafting-style vector SVG.
    Svg,
    /// Z-buffered raster JPEG.
    Jpeg,
    /// Z-buffered raster PNG with a transparent background.
    Png,
}

impl Format {
    fn extension(self) -> &'static str {
        match self {
            Format::Svg => "svg",
            Format::Jpeg => "jpg",
            Format::Png => "png",
        }
    }

    /// True for the raster (raytrace-capable) formats.
    fn is_raster(self) -> bool {
        matches!(self, Format::Jpeg | Format::Png)
    }

    fn from_extension(path: &Path) -> Result<Self, String> {
        match path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("svg") => Ok(Format::Svg),
            Some("jpg") | Some("jpeg") => Ok(Format::Jpeg),
            Some("png") => Ok(Format::Png),
            _ => Err(format!(
                "cannot infer format from '{}' (expected .svg, .jpg, .jpeg, or .png)",
                path.display()
            )),
        }
    }
}

/// Project `.vcad` documents to static line art: isometric/orthographic
/// SVG or raster JPEG/PNG.
#[derive(Parser)]
#[command(name = "vcad-render", version)]
struct Cli {
    /// Input `.vcad` or `.loon` file(s); a directory expands to its
    /// `*.vcad`/`*.loon` files.
    #[arg(required = true)]
    inputs: Vec<PathBuf>,

    /// Camera view: iso|front|side|top|hero|orbit:AZ,EL. Overridden by
    /// `--azimuth`/`--elevation`.
    #[arg(long, default_value = "iso", value_parser = View::from_str)]
    view: View,

    /// Orbit camera azimuth in degrees (CCW from +X, Z-up). Selects an
    /// orbit view and overrides `--view`.
    #[arg(long)]
    azimuth: Option<f64>,

    /// Orbit camera elevation in degrees (above the XY plane, clamped
    /// ±90). Selects an orbit view and overrides `--view`.
    #[arg(long)]
    elevation: Option<f64>,

    /// Frame the render on this part's bounding box (matched against root
    /// node names, assembly instance ids/names, and part-def ids) instead
    /// of the whole document.
    #[arg(long)]
    focus: Option<String>,

    /// Pixels per millimetre (SVG only).
    #[arg(long, default_value_t = DEFAULT_SCALE)]
    scale: f64,

    /// Transparent SVG background.
    #[arg(long)]
    transparent: bool,

    /// Force the top-down PCB board view (Studio Graphite) even when
    /// auto-detection would pick the isometric projection. SVG only.
    #[arg(long)]
    pcb: bool,

    /// Force the isometric mesh projection for documents that contain a
    /// PCB (auto-detection normally selects the board view for those).
    #[arg(long)]
    no_pcb: bool,

    /// Multi-view drawing sheet (front/side/top/iso in third-angle
    /// arrangement at one shared scale, with a title block) instead of a
    /// single view. Uses `--size` as the sheet width; SVG and JPEG only.
    #[arg(long)]
    sheet: bool,

    /// Emit BRep-exact linework where available: circular model edges
    /// (cylinder/cone rims) and sphere view outlines become exact SVG
    /// elliptical arcs instead of tessellated polylines (SVG only).
    #[arg(long)]
    exact_edges: bool,

    /// Section (cutaway) plane: `x=N`, `y=N`, or `z=N` (mm). The half of the
    /// model on the camera's side of the plane is removed and exposed cut
    /// faces are cross-hatched. Composes with `--view` and raster output.
    #[arg(long, value_parser = SectionPlane::from_str)]
    section: Option<SectionPlane>,

    /// Overlay an X/Y/Z origin gizmo (kernel is Z-up).
    #[arg(long)]
    axes: bool,

    /// Label each top-level part with its name.
    #[arg(long)]
    labels: bool,

    /// Overlay overall W×D×H bounding-box dimensions in mm.
    #[arg(long)]
    dims: bool,

    /// Output path; format inferred from extension (.svg/.jpg/.jpeg/.png).
    /// Use `-o -` for SVG on stdout. Single input only.
    #[arg(short, long, conflicts_with = "jpeg")]
    output: Option<PathBuf>,

    /// Legacy alias for `-o <path.jpg>`: write a JPEG to this path.
    #[arg(long)]
    jpeg: Option<PathBuf>,

    /// Directory for batch outputs (default: next to each input).
    #[arg(long, conflicts_with_all = ["output", "jpeg"])]
    out_dir: Option<PathBuf>,

    /// Output format in batch mode (single-file `-o` infers from extension).
    #[arg(long, value_enum, default_value_t = Format::Svg)]
    format: Format,

    /// Raster canvas size in pixels (JPEG/PNG): `N` for an N×N canvas or
    /// `WxH` for a non-square one. Defaults to 1024 for JPEG and 4096 for
    /// PNG when unset; with `--sheet`, the overall sheet width (default
    /// 1600, height ignored).
    #[arg(long)]
    size: Option<SizeArg>,

    /// Fit the canvas to the subject's projected aspect ratio instead of
    /// padding a square: the long screen axis keeps `--size` pixels and the
    /// short axis shrinks to match. Ignored when `--size WxH` is explicit.
    #[arg(long)]
    auto_aspect: bool,

    /// Crop raster output to the drawn content's bounding box. Exact for
    /// PNG (transparent background); on JPEG the background is opaque, so
    /// the crop simply removes empty vellum.
    #[arg(long)]
    trim: bool,

    /// Pixels of margin to keep around the content when `--trim` is set.
    #[arg(long, default_value_t = 0, requires = "trim")]
    trim_margin: u32,

    /// Fraction of the canvas the part's long axis fills (JPEG/PNG only).
    #[arg(long, default_value_t = 0.6)]
    fill: f64,

    /// JPEG quality, 1-100 (ignored for PNG).
    #[arg(long, default_value_t = 92)]
    quality: u8,

    /// Render raster output via direct BRep ray tracing instead of
    /// tessellation: pixel-perfect curved silhouettes. Needs a raster
    /// output (`.png`/`.jpg`).
    #[arg(long)]
    raytrace: bool,

    /// Photorealistic path tracing: physically-based materials, a studio
    /// softbox rig, global illumination, and a real camera lens. Needs a
    /// raster output (`.png`/`.jpg`). Much slower than `--raytrace` — tune
    /// with `--spp`.
    #[arg(long, conflicts_with = "raytrace")]
    photoreal: bool,

    /// Samples per pixel for `--photoreal`. 32 for a quick look, 512+ for a
    /// clean hero render.
    #[arg(long, default_value_t = 128, requires = "photoreal")]
    spp: u32,

    /// Maximum path length (light bounces) for `--photoreal`.
    #[arg(long, default_value_t = 6, requires = "photoreal")]
    max_depth: u32,

    /// Exposure multiplier applied before the ACES tonemap (`--photoreal`).
    #[arg(long, default_value_t = 1.0, requires = "photoreal")]
    exposure: f32,

    /// Vertical field of view in degrees (`--photoreal`). Lower reads as a
    /// longer lens: 30-40 flatters mechanical parts.
    #[arg(long, default_value_t = 34.0, requires = "photoreal")]
    fov: f64,

    /// Keep the orthographic drafting framing but shade physically
    /// (`--photoreal`).
    #[arg(long, requires = "photoreal")]
    ortho: bool,

    /// Aperture radius as a fraction of the scene radius (`--photoreal`).
    /// 0 is a pinhole; 0.02-0.05 gives a tasteful product-shot defocus.
    #[arg(long, default_value_t = 0.0, requires = "photoreal")]
    aperture: f64,

    /// Backdrop for `--photoreal`: studio sweep, shadow-catcher (transparent
    /// but keeps the contact shadow), or none.
    #[arg(long, value_enum, default_value_t = BackdropArg::Studio, requires = "photoreal")]
    backdrop: BackdropArg,

    /// Environment lighting for `--photoreal`: `gradient` (the analytic
    /// studio sky plus the softbox rig, the default), a built-in studio
    /// HDRI (`studio`, `softbox`, `overcast`), or a path to a lat-long
    /// Radiance `.hdr` file. An image environment replaces the softbox rig.
    #[arg(long, default_value = "gradient", requires = "photoreal")]
    env: String,

    /// Spin the image environment about the vertical axis, in degrees
    /// (`--photoreal`). Ignored by `--env gradient`.
    #[arg(long, default_value_t = 0.0, requires = "photoreal")]
    env_rotation: f64,

    /// Random seed for `--photoreal` sampling.
    #[arg(long, default_value_t = 0x5eed_1234, requires = "photoreal")]
    seed: u64,

    /// Skip the edge-aware denoiser (`--photoreal`), leaving the raw Monte
    /// Carlo estimate. Use this for reference or ground-truth renders, where
    /// the noise itself is the thing being measured.
    #[arg(long, requires = "photoreal")]
    no_denoise: bool,
}

/// CLI spelling of the photoreal backdrop options.
#[derive(Copy, Clone, PartialEq, Eq, clap::ValueEnum)]
enum BackdropArg {
    /// Infinite neutral studio sweep.
    Studio,
    /// Transparent background that still receives the contact shadow.
    Shadow,
    /// No floor; the subject floats in the environment gradient.
    None,
}

impl Cli {
    /// The effective view: explicit `--azimuth`/`--elevation` compose into
    /// an orbit camera (unspecified angle defaults to 0°) and override
    /// `--view`.
    /// Sheet width in pixels: `--size`'s width component, or the default.
    /// A `WxH` size contributes only its width — a sheet's height follows
    /// from its layout.
    fn sheet_width_px(&self) -> u32 {
        self.size.map_or(SHEET_DEFAULT_WIDTH_PX, |s| s.width)
    }

    fn effective_view(&self) -> View {
        if self.azimuth.is_some() || self.elevation.is_some() {
            View::Orbit {
                azimuth: self.azimuth.unwrap_or(0.0),
                elevation: self.elevation.unwrap_or(0.0),
            }
        } else {
            self.view
        }
    }
}

/// Expand directory inputs to their `*.vcad`/`*.loon` files (sorted); pass files
/// through untouched.
fn expand_inputs(inputs: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    for input in inputs {
        if input.is_dir() {
            let mut found: Vec<PathBuf> = std::fs::read_dir(input)
                .map_err(|e| format!("read dir {}: {}", input.display(), e))?
                .filter_map(|entry| entry.ok().map(|e| e.path()))
                .filter(|p| {
                    p.is_file()
                        && matches!(
                            p.extension().and_then(|e| e.to_str()),
                            Some("vcad") | Some("loon")
                        )
                })
                .collect();
            if found.is_empty() {
                return Err(format!("no .vcad or .loon files in {}", input.display()));
            }
            found.sort();
            out.extend(found);
        } else {
            out.push(input.clone());
        }
    }
    Ok(out)
}

impl Cli {
    /// The opt-in annotation overlays selected on the command line.
    fn annotations(&self) -> RenderAnnotations {
        RenderAnnotations {
            axes: self.axes,
            labels: self.labels,
            dims: self.dims,
        }
    }
}

/// Raster options for `cli`, defaulting the canvas size per format when the
/// user left `--size` unset: JPEG follows the mecheval 1024px capture rule,
/// PNG (transparent, lossless) targets a much larger 4096px.
#[cfg(feature = "raster")]
fn raster_opts(cli: &Cli, png: bool) -> vcad_render::RasterOptions {
    vcad_render::RasterOptions {
        view: cli.effective_view(),
        size_px: cli
            .size
            .map(|s| s.width)
            .unwrap_or(if png { 4096 } else { 1024 }),
        height_px: cli.size.and_then(|s| s.height),
        auto_aspect: cli.auto_aspect,
        trim_margin_px: cli.trim.then_some(cli.trim_margin),
        fill_frac: cli.fill,
        quality: cli.quality,
        focus: cli.focus.clone(),
        section: cli.section,
        annotations: cli.annotations(),
    }
}

/// Render raw `.vcad` to raster bytes in `format` (JPEG or PNG), via the
/// tessellated path or — when `cli.raytrace` — direct BRep ray tracing.
#[cfg(feature = "raster")]
fn render_raster(raw: &str, cli: &Cli, format: Format) -> Result<Vec<u8>, String> {
    let png = format == Format::Png;
    let opts = raster_opts(cli, png);
    if cli.photoreal {
        // Same constraint as --raytrace: the path tracer needs analytic BRep
        // surfaces, and the overlays are drawn by the projected 2D path.
        if cli.section.is_some() || cli.annotations().any() {
            return Err(
                "--photoreal does not compose with --section/--axes/--labels/--dims; \
                 use the tessellated raster path for those"
                    .to_string(),
            );
        }
        #[cfg(feature = "raytrace")]
        {
            use vcad_render::photoreal::{Backdrop, PhotorealOptions};
            let pr = PhotorealOptions {
                environment: vcad_render::envmap::parse_env_arg(&cli.env),
                env_rotation_deg: cli.env_rotation,
                spp: cli.spp,
                max_depth: cli.max_depth,
                exposure: cli.exposure,
                fov_deg: cli.fov,
                orthographic: cli.ortho,
                aperture_frac: cli.aperture,
                backdrop: match cli.backdrop {
                    BackdropArg::Studio => Backdrop::Studio,
                    BackdropArg::Shadow => Backdrop::ShadowCatcher,
                    BackdropArg::None => Backdrop::None,
                },
                seed: cli.seed,
                denoise: !cli.no_denoise,
            };
            return if png {
                vcad_render::photoreal::render_photoreal_png_str(raw, &opts, &pr)
            } else {
                vcad_render::photoreal::render_photoreal_jpeg_str(raw, &opts, &pr)
            };
        }
        #[cfg(not(feature = "raytrace"))]
        {
            return Err("this build of vcad-render lacks the `raytrace` feature".to_string());
        }
    }
    if cli.raytrace {
        // The ray tracer works on analytic BRep surfaces; sectioning
        // boolean-subtracts (yielding mesh-backed solids it can't trace) and
        // the annotation overlays are drawn by the projected 2D path. Reject
        // the combination rather than silently ignoring those flags.
        if cli.section.is_some() || cli.annotations().any() {
            return Err(
                "--raytrace does not compose with --section/--axes/--labels/--dims; \
                 use the tessellated raster path (drop --raytrace) for those"
                    .to_string(),
            );
        }
        #[cfg(feature = "raytrace")]
        {
            return if png {
                vcad_render::render_raytrace_png_str(raw, &opts)
            } else {
                vcad_render::render_raytrace_jpeg_str(raw, &opts)
            };
        }
        #[cfg(not(feature = "raytrace"))]
        {
            return Err("this build of vcad-render lacks the `raytrace` feature".to_string());
        }
    }
    if png {
        vcad_render::render_png_str(raw, &opts)
    } else {
        vcad_render::render_jpeg_str(raw, &opts)
    }
}

#[cfg(not(feature = "raster"))]
fn render_raster(_raw: &str, _cli: &Cli, _format: Format) -> Result<Vec<u8>, String> {
    Err("this build of vcad-render lacks the `raster` feature".to_string())
}

/// The title-block name for a sheet render: the input file stem.
fn sheet_title(input: &Path) -> String {
    input
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "untitled".to_string())
}

#[cfg(feature = "raster")]
fn render_sheet_jpeg(raw: &str, input: &Path, cli: &Cli) -> Result<Vec<u8>, String> {
    vcad_render::sheet::render_sheet_jpeg_str(
        raw,
        &vcad_render::sheet::SheetRasterOptions {
            width_px: cli.sheet_width_px(),
            quality: cli.quality,
            title: sheet_title(input),
        },
    )
}

#[cfg(not(feature = "raster"))]
fn render_sheet_jpeg(_raw: &str, _input: &Path, _cli: &Cli) -> Result<Vec<u8>, String> {
    Err("this build of vcad-render lacks the `raster` feature".to_string())
}

/// Render a multi-view drawing sheet for one input. SVG and JPEG only —
/// the sheet is a drafting deliverable, not a transparent asset.
fn render_sheet_one(
    input: &Path,
    dest: Option<&Path>,
    format: Format,
    cli: &Cli,
    raw: &str,
) -> Result<(), String> {
    if cli.raytrace {
        return Err("--sheet and --raytrace cannot be combined".to_string());
    }
    if cli.photoreal {
        return Err("--sheet and --photoreal cannot be combined".to_string());
    }
    let bytes = match format {
        Format::Svg => {
            let svg = vcad_render::sheet::render_sheet_svg_str(
                raw,
                &vcad_render::sheet::SheetOptions {
                    width_px: cli.sheet_width_px() as f64,
                    title: sheet_title(input),
                },
            )?;
            match dest {
                None => {
                    println!("{}", svg);
                    return Ok(());
                }
                Some(_) => svg.into_bytes(),
            }
        }
        Format::Jpeg => render_sheet_jpeg(raw, input, cli)?,
        Format::Png => return Err("--sheet supports SVG and JPEG output only, not PNG".to_string()),
    };
    let dest = dest.expect("raster output always has a destination path");
    std::fs::write(dest, bytes).map_err(|e| format!("write {}: {}", dest.display(), e))
}

/// Top-down PCB board render with the standard 2-layer + silk + edge
/// layer set. Dark "Studio Graphite" theme; ratsnest on so unrouted
/// boards show their crime.
fn render_pcb_view(pcb: &vcad_ir::ecad::Pcb, cli: &Cli) -> String {
    use vcad_ir::ecad::PcbLayer;
    let layers = [
        PcbLayer::BCu,
        PcbLayer::FCu,
        PcbLayer::FSilkS,
        PcbLayer::EdgeCuts,
    ];
    vcad_render::pcb::render_pcb_svg_opts(
        pcb,
        &layers,
        cli.scale,
        &vcad_render::pcb::PcbRenderOpts {
            transparent: cli.transparent,
            ..Default::default()
        },
    )
}

/// Read an input as `.vcad` IR JSON. A `.loon` input is evaluated first, so
/// the renderer works on source rather than on a build artifact; `[use ...]`
/// module imports resolve against the input file's own directory.
fn read_document(input: &Path) -> Result<String, String> {
    let raw =
        std::fs::read_to_string(input).map_err(|e| format!("read {}: {}", input.display(), e))?;
    if !is_loon(input) {
        return Ok(raw);
    }
    let doc = vcad_loon::eval_vcad(raw.trim(), input.parent())
        .map_err(|e| format!("{}: {}", input.display(), e))?;
    serde_json::to_string(&doc).map_err(|e| format!("{}: serialize: {}", input.display(), e))
}

/// Does this path name loon source rather than `.vcad` IR?
fn is_loon(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some("loon")
}

/// Render one input to `dest` (`None` = SVG on stdout) in `format`. With
/// `--sheet`, a multi-view drawing sheet replaces the single view.
fn render_one(input: &Path, dest: Option<&Path>, format: Format, cli: &Cli) -> Result<(), String> {
    if cli.raytrace && !format.is_raster() {
        return Err(
            "--raytrace needs a raster output: use -o <out.png> / <out.jpg> or --format png/jpeg"
                .to_string(),
        );
    }
    if cli.photoreal && !format.is_raster() {
        return Err(
            "--photoreal needs a raster output: use -o <out.png> / <out.jpg> or --format png/jpeg"
                .to_string(),
        );
    }
    let raw = read_document(input)?;
    if cli.sheet {
        return render_sheet_one(input, dest, format, cli, &raw);
    }
    let bytes = match format {
        Format::Svg => {
            // ECAD documents get the top-down board view (copper/silk, the
            // view an EDA tool shows) instead of a flat green isometric
            // slab. Auto-detected; --pcb forces it, --no-pcb suppresses it.
            if !cli.no_pcb {
                if let Some(pcb) = vcad_render::extract_pcb(&raw) {
                    let svg = render_pcb_view(&pcb, cli);
                    match dest {
                        None => {
                            println!("{}", svg);
                            return Ok(());
                        }
                        Some(dest) => {
                            return std::fs::write(dest, svg.into_bytes())
                                .map_err(|e| format!("write {}: {}", dest.display(), e));
                        }
                    }
                } else if cli.pcb {
                    return Err("--pcb: document contains no PCB".to_string());
                }
            }
            let svg = render_svg_str_opts(
                &raw,
                cli.scale,
                &SvgOptions {
                    view: cli.effective_view(),
                    transparent: cli.transparent,
                    exact_edges: cli.exact_edges,
                    section: cli.section,
                    focus: cli.focus.clone(),
                    annotations: cli.annotations(),
                    ..Default::default()
                },
            )?;
            match dest {
                None => {
                    println!("{}", svg);
                    return Ok(());
                }
                Some(_) => svg.into_bytes(),
            }
        }
        Format::Jpeg | Format::Png => render_raster(&raw, cli, format)?,
    };
    let dest = dest.expect("raster output always has a destination path");
    std::fs::write(dest, bytes).map_err(|e| format!("write {}: {}", dest.display(), e))
}

/// Batch destination for `input`: `<stem>.<ext>` in `out_dir` or next to
/// the input. Fails on a path with no file name (e.g. one ending in `..`).
fn batch_dest(input: &Path, out_dir: Option<&Path>, format: Format) -> Result<PathBuf, String> {
    let name = input.with_extension(format.extension());
    match out_dir {
        Some(dir) => {
            let file = name
                .file_name()
                .ok_or_else(|| format!("input {} has no file name", input.display()))?;
            Ok(dir.join(file))
        }
        None => Ok(name),
    }
}

fn run(cli: &Cli) -> Result<(), String> {
    let inputs = expand_inputs(&cli.inputs)?;
    let batch = inputs.len() > 1 || cli.out_dir.is_some();

    if batch {
        if cli.output.is_some() || cli.jpeg.is_some() {
            return Err(
                "-o/--jpeg take a single output path; use --out-dir with multiple inputs".into(),
            );
        }
        if let Some(dir) = &cli.out_dir {
            std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {}", dir.display(), e))?;
        }
        let mut failures = 0usize;
        for input in &inputs {
            let result = batch_dest(input, cli.out_dir.as_deref(), cli.format)
                .and_then(|dest| render_one(input, Some(&dest), cli.format, cli).map(|()| dest));
            match result {
                Ok(dest) => eprintln!("{} -> {}", input.display(), dest.display()),
                Err(e) => {
                    failures += 1;
                    eprintln!("{}: {}", input.display(), e);
                }
            }
        }
        if failures > 0 {
            return Err(format!("{failures} of {} renders failed", inputs.len()));
        }
        return Ok(());
    }

    let input = &inputs[0];
    // Legacy --jpeg <path> spelling.
    if let Some(path) = &cli.jpeg {
        return render_one(input, Some(path), Format::Jpeg, cli);
    }
    match &cli.output {
        None => render_one(input, None, Format::Svg, cli),
        Some(path) if path.as_os_str() == "-" => render_one(input, None, Format::Svg, cli),
        Some(path) => render_one(input, Some(path), Format::from_extension(path)?, cli),
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{}", e);
            ExitCode::from(2)
        }
    }
}
