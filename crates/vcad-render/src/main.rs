//! `vcad-render` CLI — project a `.vcad` to a static isometric SVG.
//!
//! Usage:
//!   vcad-render <path.vcad> [--scale <px-per-mm>]
//!
//! Output: a single self-contained `<svg>` on stdout. All rendering logic
//! lives in the `vcad-render` library (see `lib.rs`); this binary only
//! handles argument parsing and file IO.

use std::path::PathBuf;
use std::process::ExitCode;

use vcad_render::{render_svg_str, DEFAULT_SCALE};

struct Args {
    path: PathBuf,
    scale: f64,
}

fn parse_args() -> Result<Args, String> {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .ok_or("usage: vcad-render <path.vcad> [--scale N]")?;
    let mut scale = DEFAULT_SCALE;
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--scale" => {
                let v = args.next().ok_or("--scale needs a value")?;
                scale = v
                    .parse()
                    .map_err(|e: std::num::ParseFloatError| e.to_string())?;
            }
            other => return Err(format!("unknown flag: {}", other)),
        }
    }
    Ok(Args {
        path: PathBuf::from(path),
        scale,
    })
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

    match render_svg_str(&raw, args.scale) {
        Ok(svg) => {
            println!("{}", svg);
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{}", e);
            ExitCode::from(2)
        }
    }
}
