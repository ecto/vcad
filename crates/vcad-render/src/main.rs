//! `vcad-render` CLI — project a `.vcad` to static line art.
//!
//! Usage:
//!   vcad-render <path.vcad> [--view iso|front|side|top|hero] [--scale <px-per-mm>] [--transparent]
//!   vcad-render <path.vcad> --jpeg <out.jpg> [--view ...] [--size <px>] [--fill <frac>] [--quality <1-100>]
//!
//! Without `--jpeg`: a single self-contained `<svg>` on stdout.
//! With `--jpeg`: a z-buffered raster render written to the given path.
//! All rendering logic lives in the `vcad-render` library (see `lib.rs`);
//! this binary only handles argument parsing and file IO.

use std::path::PathBuf;
use std::process::ExitCode;
use std::str::FromStr;

use vcad_render::{render_svg_str_view_opts, View, DEFAULT_SCALE};

struct Args {
    path: PathBuf,
    scale: f64,
    view: View,
    jpeg: Option<PathBuf>,
    size: u32,
    fill: f64,
    quality: u8,
    transparent: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = std::env::args().skip(1);
    let path = args.next().ok_or(
        "usage: vcad-render <path.vcad> [--view iso|front|side|top|hero] [--scale N] [--transparent] \
         [--jpeg out.jpg [--size N] [--fill F] [--quality Q]]",
    )?;
    let mut out = Args {
        path: PathBuf::from(path),
        scale: DEFAULT_SCALE,
        view: View::Isometric,
        jpeg: None,
        size: 1024,
        fill: 0.6,
        quality: 92,
        transparent: false,
    };
    while let Some(flag) = args.next() {
        let mut value = |name: &str| args.next().ok_or(format!("{name} needs a value"));
        match flag.as_str() {
            "--scale" => {
                out.scale = value("--scale")?
                    .parse()
                    .map_err(|e: std::num::ParseFloatError| e.to_string())?;
            }
            "--view" => {
                out.view = View::from_str(&value("--view")?)?;
            }
            "--jpeg" => {
                out.jpeg = Some(PathBuf::from(value("--jpeg")?));
            }
            "--size" => {
                out.size = value("--size")?
                    .parse()
                    .map_err(|e: std::num::ParseIntError| e.to_string())?;
            }
            "--fill" => {
                out.fill = value("--fill")?
                    .parse()
                    .map_err(|e: std::num::ParseFloatError| e.to_string())?;
            }
            "--quality" => {
                out.quality = value("--quality")?
                    .parse()
                    .map_err(|e: std::num::ParseIntError| e.to_string())?;
            }
            "--transparent" => {
                out.transparent = true;
            }
            other => return Err(format!("unknown flag: {}", other)),
        }
    }
    Ok(out)
}

#[cfg(feature = "raster")]
fn run_jpeg(raw: &str, args: &Args, out_path: &std::path::Path) -> Result<(), String> {
    let opts = vcad_render::RasterOptions {
        view: args.view,
        size_px: args.size,
        fill_frac: args.fill,
        quality: args.quality,
    };
    let bytes = vcad_render::render_jpeg_str(raw, &opts)?;
    std::fs::write(out_path, bytes).map_err(|e| format!("write {}: {}", out_path.display(), e))
}

#[cfg(not(feature = "raster"))]
fn run_jpeg(_raw: &str, _args: &Args, _out_path: &std::path::Path) -> Result<(), String> {
    Err("this build of vcad-render lacks the `raster` feature".to_string())
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{}", e);
            return ExitCode::from(2);
        }
    };

    let raw = match std::fs::read_to_string(&args.path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("read error: {}", e);
            return ExitCode::from(2);
        }
    };

    let result = match &args.jpeg {
        Some(out_path) => run_jpeg(&raw, &args, out_path),
        None => render_svg_str_view_opts(&raw, args.scale, args.view, args.transparent)
            .map(|svg| println!("{}", svg)),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{}", e);
            ExitCode::from(2)
        }
    }
}
