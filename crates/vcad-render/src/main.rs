//! `vcad-render` CLI — project a `.vcad` to static line art or a raster render.
//!
//! Usage:
//!   vcad-render <path.vcad> [--view iso|front|side|top|hero] [--scale <px-per-mm>] [--transparent]
//!   vcad-render <path.vcad> --jpeg <out.jpg> [--view ...] [--size <px>] [--fill <frac>] [--quality <1-100>]
//!   vcad-render <path.vcad> --png <out.png>  [--view ...] [--size <px>] [--fill <frac>]
//!   vcad-render <path.vcad> --raytrace --png <out.png>   (or --jpeg <out.jpg>)
//!
//! Without `--jpeg`/`--png`: a single self-contained `<svg>` on stdout.
//! With `--jpeg`/`--png`: a z-buffered raster render written to the given
//! path — or, with `--raytrace`, a pixel-perfect direct-BRep ray trace
//! (no tessellation, exact curved silhouettes).
//! All rendering logic lives in the `vcad-render` library (see `lib.rs`);
//! this binary only handles argument parsing and file IO.

use std::path::PathBuf;
use std::process::ExitCode;
use std::str::FromStr;

use vcad_render::{render_svg_str_view_opts, View, DEFAULT_SCALE};

/// Raster output container, chosen by `--jpeg` vs `--png`.
enum RasterOut {
    Jpeg(PathBuf),
    Png(PathBuf),
}

struct Args {
    path: PathBuf,
    scale: f64,
    view: View,
    raster: Option<RasterOut>,
    raytrace: bool,
    size: u32,
    fill: f64,
    quality: u8,
    transparent: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = std::env::args().skip(1);
    let path = args.next().ok_or(
        "usage: vcad-render <path.vcad> [--view iso|front|side|top|hero] [--scale N] [--transparent] \
         [--raytrace] [--jpeg out.jpg | --png out.png] [--size N] [--fill F] [--quality Q]",
    )?;
    let mut out = Args {
        path: PathBuf::from(path),
        scale: DEFAULT_SCALE,
        view: View::Isometric,
        raster: None,
        raytrace: false,
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
                if out.raster.is_some() {
                    return Err("pick one of --jpeg or --png".to_string());
                }
                out.raster = Some(RasterOut::Jpeg(PathBuf::from(value("--jpeg")?)));
            }
            "--png" => {
                if out.raster.is_some() {
                    return Err("pick one of --jpeg or --png".to_string());
                }
                out.raster = Some(RasterOut::Png(PathBuf::from(value("--png")?)));
            }
            "--raytrace" => {
                out.raytrace = true;
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
    if out.raytrace && out.raster.is_none() {
        return Err(
            "--raytrace needs a raster output: add --png <out.png> or --jpeg <out.jpg>".to_string(),
        );
    }
    Ok(out)
}

#[cfg(feature = "raster")]
fn run_raster(raw: &str, args: &Args, out: &RasterOut) -> Result<(), String> {
    let opts = vcad_render::RasterOptions {
        view: args.view,
        size_px: args.size,
        fill_frac: args.fill,
        quality: args.quality,
    };
    let (bytes, out_path) = if args.raytrace {
        #[cfg(feature = "raytrace")]
        {
            match out {
                RasterOut::Jpeg(p) => (vcad_render::render_raytrace_jpeg_str(raw, &opts)?, p),
                RasterOut::Png(p) => (vcad_render::render_raytrace_png_str(raw, &opts)?, p),
            }
        }
        #[cfg(not(feature = "raytrace"))]
        {
            return Err("this build of vcad-render lacks the `raytrace` feature".to_string());
        }
    } else {
        match out {
            RasterOut::Jpeg(p) => (vcad_render::render_jpeg_str(raw, &opts)?, p),
            RasterOut::Png(p) => (vcad_render::render_png_str(raw, &opts)?, p),
        }
    };
    std::fs::write(out_path, bytes).map_err(|e| format!("write {}: {}", out_path.display(), e))
}

#[cfg(not(feature = "raster"))]
fn run_raster(_raw: &str, _args: &Args, _out: &RasterOut) -> Result<(), String> {
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

    let result = match &args.raster {
        Some(out) => run_raster(&raw, &args, out),
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
