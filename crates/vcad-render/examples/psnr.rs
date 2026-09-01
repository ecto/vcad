//! Image-quality harness for photoreal render changes.
//!
//! Compares a candidate PNG against a reference PNG and reports PSNR (dB) and
//! grayscale SSIM. Used to gate sampling changes in the path tracer: a
//! candidate render at the normal sample count must stay above a PSNR floor
//! relative to a high-spp reference of the same scene.
//!
//! ```text
//! cargo run --release -p vcad-render --example psnr -- ref.png test.png [--min-psnr 35]
//! ```
//!
//! Exits non-zero when `--min-psnr` is given and the candidate falls below it,
//! so it can drive a shell gate directly.
//!
//! Both metrics run on the images' 8-bit sRGB samples, which is what a viewer
//! actually sees; PSNR uses all three channels, SSIM the Rec. 709 luma.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut paths: Vec<&str> = Vec::new();
    let mut min_psnr: Option<f64> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--min-psnr" => {
                i += 1;
                match args.get(i).and_then(|s| s.parse::<f64>().ok()) {
                    Some(v) => min_psnr = Some(v),
                    None => {
                        eprintln!("--min-psnr expects a number");
                        return ExitCode::from(2);
                    }
                }
            }
            "-h" | "--help" => {
                println!("usage: psnr <reference.png> <candidate.png> [--min-psnr <db>]");
                return ExitCode::SUCCESS;
            }
            other => paths.push(other),
        }
        i += 1;
    }
    if paths.len() != 2 {
        eprintln!("usage: psnr <reference.png> <candidate.png> [--min-psnr <db>]");
        return ExitCode::from(2);
    }

    let reference = match load_rgb(paths[0]) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{}: {e}", paths[0]);
            return ExitCode::from(2);
        }
    };
    let candidate = match load_rgb(paths[1]) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{}: {e}", paths[1]);
            return ExitCode::from(2);
        }
    };
    if reference.width != candidate.width || reference.height != candidate.height {
        eprintln!(
            "size mismatch: {}x{} vs {}x{}",
            reference.width, reference.height, candidate.width, candidate.height
        );
        return ExitCode::from(2);
    }

    let psnr = psnr(&reference, &candidate);
    let ssim = ssim(&reference, &candidate);
    let psnr_str = if psnr.is_infinite() {
        "inf (identical)".to_string()
    } else {
        format!("{psnr:.2} dB")
    };
    println!("psnr {psnr_str}");
    println!("ssim {ssim:.5}");

    match min_psnr {
        Some(floor) if psnr < floor => {
            eprintln!("FAIL: psnr {psnr:.2} dB below floor {floor:.2} dB");
            ExitCode::FAILURE
        }
        _ => ExitCode::SUCCESS,
    }
}

/// An 8-bit RGB image, three bytes per pixel.
struct Image {
    width: u32,
    height: u32,
    rgb: Vec<u8>,
}

fn load_rgb(path: &str) -> Result<Image, String> {
    let img = image::open(path).map_err(|e| e.to_string())?.to_rgb8();
    Ok(Image {
        width: img.width(),
        height: img.height(),
        rgb: img.into_raw(),
    })
}

/// Peak signal-to-noise ratio over all three channels, 255 peak.
///
/// Infinite for byte-identical images.
fn psnr(a: &Image, b: &Image) -> f64 {
    let mut sse = 0.0f64;
    for (x, y) in a.rgb.iter().zip(b.rgb.iter()) {
        let d = *x as f64 - *y as f64;
        sse += d * d;
    }
    let mse = sse / a.rgb.len() as f64;
    if mse == 0.0 {
        return f64::INFINITY;
    }
    10.0 * (255.0 * 255.0 / mse).log10()
}

/// Rec. 709 luma, 0..255.
fn luma(rgb: &[u8], i: usize) -> f64 {
    0.2126 * rgb[i * 3] as f64 + 0.7152 * rgb[i * 3 + 1] as f64 + 0.0722 * rgb[i * 3 + 2] as f64
}

/// Mean grayscale SSIM over 8x8 windows (stride 4).
///
/// The standard 11x11 Gaussian window is overkill for a gate; a uniform
/// window over a dense stride tracks the same structural differences and is
/// far simpler to read.
fn ssim(a: &Image, b: &Image) -> f64 {
    const WIN: usize = 8;
    const STRIDE: usize = 4;
    // Stabilising constants from Wang et al. 2004, for L = 255.
    let c1 = (0.01 * 255.0f64).powi(2);
    let c2 = (0.03 * 255.0f64).powi(2);

    let w = a.width as usize;
    let h = a.height as usize;
    if w < WIN || h < WIN {
        return f64::NAN;
    }

    let mut total = 0.0f64;
    let mut windows = 0usize;
    let mut y0 = 0;
    while y0 + WIN <= h {
        let mut x0 = 0;
        while x0 + WIN <= w {
            let (mut sa, mut sb, mut saa, mut sbb, mut sab) = (0.0, 0.0, 0.0, 0.0, 0.0);
            for dy in 0..WIN {
                for dx in 0..WIN {
                    let i = (y0 + dy) * w + x0 + dx;
                    let va = luma(&a.rgb, i);
                    let vb = luma(&b.rgb, i);
                    sa += va;
                    sb += vb;
                    saa += va * va;
                    sbb += vb * vb;
                    sab += va * vb;
                }
            }
            let n = (WIN * WIN) as f64;
            let ma = sa / n;
            let mb = sb / n;
            // Unbiased (n-1) variance/covariance, as in the reference paper.
            let va = (saa - sa * ma) / (n - 1.0);
            let vb = (sbb - sb * mb) / (n - 1.0);
            let cov = (sab - sa * mb) / (n - 1.0);
            let num = (2.0 * ma * mb + c1) * (2.0 * cov + c2);
            let den = (ma * ma + mb * mb + c1) * (va + vb + c2);
            total += num / den;
            windows += 1;
            x0 += STRIDE;
        }
        y0 += STRIDE;
    }
    total / windows as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: u32, height: u32, v: u8) -> Image {
        Image {
            width,
            height,
            rgb: vec![v; (width * height * 3) as usize],
        }
    }

    #[test]
    fn identical_images_score_perfectly() {
        let a = solid(16, 16, 128);
        let b = solid(16, 16, 128);
        assert!(psnr(&a, &b).is_infinite());
        assert!((ssim(&a, &b) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn known_offset_matches_closed_form() {
        // Every sample off by 1 => MSE 1 => PSNR = 20*log10(255).
        let a = solid(16, 16, 100);
        let b = solid(16, 16, 101);
        let expected = 20.0 * 255.0f64.log10();
        assert!((psnr(&a, &b) - expected).abs() < 1e-9);
    }

    #[test]
    fn structural_difference_drops_ssim() {
        let a = solid(32, 32, 10);
        let mut b = solid(32, 32, 10);
        for i in 0..(32 * 32) {
            if (i / 32) % 2 == 0 {
                b.rgb[i * 3] = 240;
                b.rgb[i * 3 + 1] = 240;
                b.rgb[i * 3 + 2] = 240;
            }
        }
        assert!(ssim(&a, &b) < 0.1);
    }
}
