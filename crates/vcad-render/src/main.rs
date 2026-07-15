//! `vcad-render` CLI — project `.vcad` documents to static line art.
//!
//! Usage:
//!   vcad-render <path.vcad> [--view iso|front|side|top|hero] [--scale <px-per-mm>] [--transparent]
//!               [--axes] [--labels] [--dims]
//!   vcad-render <path.vcad> -o out.jpg [--view ...] [--size <px>] [--fill <frac>] [--quality <1-100>]
//!   vcad-render <dir-or-paths...> [--out-dir <dir>] [--format svg|jpeg]
//!
//! With a single input and no output flag, a self-contained `<svg>` goes to
//! stdout. `-o <path>` picks the format from the extension (`.svg`, `.jpg`,
//! `.jpeg`); `-o -` writes SVG to stdout. `--jpeg <path>` is the legacy
//! spelling of `-o <path.jpg>`. Multiple inputs (or a directory, which
//! expands to its `*.vcad` files) render in batch, each to a sibling output
//! file or into `--out-dir`; a per-file failure is reported but does not
//! abort the batch. All rendering logic lives in the `vcad-render` library
//! (see `lib.rs`); this binary only handles argument parsing and file IO.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::str::FromStr;

use clap::{Parser, ValueEnum};
use vcad_render::{render_svg_str_view_opts, RenderAnnotations, View, DEFAULT_SCALE};

/// Raster/vector format for batch outputs (single-file `-o` infers the
/// format from the extension instead).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Format {
    /// Drafting-style vector SVG.
    Svg,
    /// Z-buffered raster JPEG.
    Jpeg,
}

impl Format {
    fn extension(self) -> &'static str {
        match self {
            Format::Svg => "svg",
            Format::Jpeg => "jpg",
        }
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
            _ => Err(format!(
                "cannot infer format from '{}' (expected .svg, .jpg, or .jpeg)",
                path.display()
            )),
        }
    }
}

/// Project `.vcad` documents to static line art: isometric/orthographic
/// SVG or raster JPEG.
#[derive(Parser)]
#[command(name = "vcad-render", version)]
struct Cli {
    /// Input `.vcad` file(s); a directory expands to its `*.vcad` files.
    #[arg(required = true)]
    inputs: Vec<PathBuf>,

    /// Camera view: iso|front|side|top|hero.
    #[arg(long, default_value = "iso", value_parser = View::from_str)]
    view: View,

    /// Pixels per millimetre (SVG only).
    #[arg(long, default_value_t = DEFAULT_SCALE)]
    scale: f64,

    /// Transparent SVG background.
    #[arg(long)]
    transparent: bool,

    /// Overlay an X/Y/Z origin gizmo (kernel is Z-up).
    #[arg(long)]
    axes: bool,

    /// Label each top-level part with its name.
    #[arg(long)]
    labels: bool,

    /// Overlay overall W×D×H bounding-box dimensions in mm.
    #[arg(long)]
    dims: bool,

    /// Output path; format inferred from extension (.svg/.jpg/.jpeg).
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

    /// Raster canvas size in pixels (JPEG only).
    #[arg(long, default_value_t = 1024)]
    size: u32,

    /// Fraction of the canvas the part's long axis fills (JPEG only).
    #[arg(long, default_value_t = 0.6)]
    fill: f64,

    /// JPEG quality, 1-100.
    #[arg(long, default_value_t = 92)]
    quality: u8,
}

/// Expand directory inputs to their `*.vcad` files (sorted); pass files
/// through untouched.
fn expand_inputs(inputs: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    for input in inputs {
        if input.is_dir() {
            let mut found: Vec<PathBuf> = std::fs::read_dir(input)
                .map_err(|e| format!("read dir {}: {}", input.display(), e))?
                .filter_map(|entry| entry.ok().map(|e| e.path()))
                .filter(|p| p.is_file() && p.extension().and_then(|e| e.to_str()) == Some("vcad"))
                .collect();
            if found.is_empty() {
                return Err(format!("no .vcad files in {}", input.display()));
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

#[cfg(feature = "raster")]
fn render_jpeg(raw: &str, cli: &Cli) -> Result<Vec<u8>, String> {
    let opts = vcad_render::RasterOptions {
        view: cli.view,
        size_px: cli.size,
        fill_frac: cli.fill,
        quality: cli.quality,
        annotations: cli.annotations(),
    };
    vcad_render::render_jpeg_str(raw, &opts)
}

#[cfg(not(feature = "raster"))]
fn render_jpeg(_raw: &str, _cli: &Cli) -> Result<Vec<u8>, String> {
    Err("this build of vcad-render lacks the `raster` feature".to_string())
}

/// Render one input to `dest` (`None` = SVG on stdout) in `format`.
fn render_one(input: &Path, dest: Option<&Path>, format: Format, cli: &Cli) -> Result<(), String> {
    let raw =
        std::fs::read_to_string(input).map_err(|e| format!("read {}: {}", input.display(), e))?;
    let bytes = match format {
        Format::Svg => {
            let svg = render_svg_str_view_opts(
                &raw,
                cli.scale,
                cli.view,
                cli.transparent,
                &cli.annotations(),
            )?;
            match dest {
                None => {
                    println!("{}", svg);
                    return Ok(());
                }
                Some(_) => svg.into_bytes(),
            }
        }
        Format::Jpeg => render_jpeg(&raw, cli)?,
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
