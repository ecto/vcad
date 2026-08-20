//! `--photoreal --gpu` against `--photoreal`, through the full `vcad-render`
//! entry points.
//!
//! The unit tests inside `photoreal_gpu.rs` cover the refusals; this is the
//! one that costs a GPU and answers the only question that matters: does the
//! GPU path render *the same picture* the CPU path does? Same document, same
//! seed, same sample count, same framing — compared as PSNR over the encoded
//! sRGB pixels, which is what a viewer actually sees.
//!
//! Run with:
//!
//! ```text
//! cargo test -p vcad-render --features photoreal-gpu --test photoreal_gpu
//! ```
//!
//! Skipped, not failed, when there is no adapter: CI machines without a GPU
//! must not turn red over a feature they cannot exercise.

#![cfg(feature = "photoreal-gpu")]

use vcad_render::photoreal::{Backdrop, PhotorealOptions};
use vcad_render::photoreal_gpu::render_photoreal_gpu_png_str;
use vcad_render::RasterOptions;

/// A stepped block: flat faces at several angles, a through hole, and enough
/// self-shadowing that a lighting disagreement between the two integrators
/// would show up rather than averaging out.
fn doc() -> String {
    r#"{
  "version": "0.1",
  "nodes": {
    "1": { "id": 1, "name": "base",
           "op": { "type": "Cube", "size": { "x": 30, "y": 20, "z": 6 } } },
    "2": { "id": 2, "name": "post",
           "op": { "type": "Cylinder", "radius": 5, "height": 18, "segments": 64 } }
  },
  "materials": {
    "aluminum": { "name": "aluminum", "color": [0.91, 0.92, 0.93],
                  "metallic": 1.0, "roughness": 0.4 },
    "abs": { "name": "abs", "color": [0.15, 0.16, 0.18],
             "metallic": 0.0, "roughness": 0.55 }
  },
  "part_materials": {},
  "roots": [{ "root": 1, "material": "aluminum" },
            { "root": 2, "material": "abs" }]
}"#
    .to_string()
}

fn raster_opts(size: u32) -> RasterOptions {
    RasterOptions {
        size_px: size,
        ..Default::default()
    }
}

/// Both renderers get an *identical* brief: the same sample count spent on
/// every pixel (no adaptive early-out, which the GPU has no equivalent for),
/// and no denoiser (which the GPU cannot run at all). Comparing a denoised
/// CPU image against a raw GPU one would measure the denoiser, not the
/// integrators.
fn photoreal_opts(spp: u32) -> PhotorealOptions {
    PhotorealOptions {
        spp,
        seed: 7,
        denoise: false,
        adaptive: false,
        backdrop: Backdrop::Studio,
        ..Default::default()
    }
}

struct Image {
    w: u32,
    h: u32,
    rgb: Vec<u8>,
}

fn decode(png: &[u8]) -> Image {
    let img = image::load_from_memory(png).expect("valid PNG").to_rgb8();
    Image {
        w: img.width(),
        h: img.height(),
        rgb: img.into_raw(),
    }
}

/// Peak signal-to-noise ratio over the 8-bit sRGB samples, in dB. Infinite
/// for identical images, which no two integrators ever are.
fn psnr(a: &Image, b: &Image) -> f64 {
    assert_eq!((a.w, a.h), (b.w, b.h), "size mismatch");
    let mse: f64 = a
        .rgb
        .iter()
        .zip(&b.rgb)
        .map(|(&x, &y)| {
            let d = x as f64 - y as f64;
            d * d
        })
        .sum::<f64>()
        / a.rgb.len() as f64;
    if mse <= 0.0 {
        return f64::INFINITY;
    }
    10.0 * (255.0f64 * 255.0 / mse).log10()
}

/// Render on the GPU, or `None` when this machine has no adapter.
///
/// The skip is keyed on the specific "no adapter" message the `--gpu` path
/// produces. Every *other* failure is a real failure and is allowed to
/// propagate: a test that skipped on any error at all would go green on a
/// broken shader.
fn gpu_png(opts: &RasterOptions, pr: &PhotorealOptions) -> Option<Vec<u8>> {
    match render_photoreal_gpu_png_str(&doc(), opts, pr) {
        Ok(png) => Some(png),
        Err(e) if e.contains("no compatible GPU adapter") => {
            eprintln!("skipped: {e}");
            None
        }
        Err(e) => panic!("GPU render failed: {e}"),
    }
}

/// The gate. 24 dB is deliberately below the ~28-30 dB the two paths actually
/// hit on real scenes: at a test-sized sample count both images are still
/// visibly noisy, and the two integrators' noise is independent, so a chunk
/// of the measured error here is Monte Carlo disagreement rather than a
/// difference of opinion about the scene. Anything that gets the geometry,
/// the framing, the lighting or the tonemap wrong lands far below this — a
/// mirrored camera basis scores in the low teens, a dropped part or a missing
/// floor worse still.
const MIN_PSNR: f64 = 24.0;

#[test]
fn gpu_render_matches_the_cpu_render() {
    let opts = raster_opts(128);
    let pr = photoreal_opts(96);

    let Some(gpu) = gpu_png(&opts, &pr) else {
        return;
    };
    let cpu = vcad_render::photoreal::render_photoreal_png_str(&doc(), &opts, &pr)
        .expect("CPU render");

    let (gpu, cpu) = (decode(&gpu), decode(&cpu));
    let db = psnr(&gpu, &cpu);
    eprintln!("CPU vs GPU: {db:.2} dB");
    assert!(
        db >= MIN_PSNR,
        "GPU render is {db:.2} dB from the CPU render (floor {MIN_PSNR} dB) -- \
         the two paths disagree about the scene, not merely about noise"
    );
}

/// The subject must land in the same place, not merely look similar on
/// average. A mirrored camera basis — which is exactly what the GPU's default
/// right-handed reconstruction does to `View::Isometric` — leaves the overall
/// brightness untouched and moves every pixel, so a whole-image metric is a
/// weak detector for it and a column profile is a strong one.
#[test]
fn the_isometric_view_is_not_mirrored() {
    let opts = RasterOptions {
        size_px: 128,
        view: vcad_render::View::Isometric,
        ..Default::default()
    };
    let pr = photoreal_opts(48);

    let Some(gpu) = gpu_png(&opts, &pr) else {
        return;
    };
    let cpu = vcad_render::photoreal::render_photoreal_png_str(&doc(), &opts, &pr)
        .expect("CPU render");
    let (gpu, cpu) = (decode(&gpu), decode(&cpu));

    // Per-column mean luminance: a silhouette signature that survives noise
    // but not a left-right flip.
    let profile = |img: &Image| -> Vec<f64> {
        (0..img.w)
            .map(|x| {
                (0..img.h)
                    .map(|y| {
                        let i = ((y * img.w + x) * 3) as usize;
                        0.2126 * img.rgb[i] as f64
                            + 0.7152 * img.rgb[i + 1] as f64
                            + 0.0722 * img.rgb[i + 2] as f64
                    })
                    .sum::<f64>()
                    / img.h as f64
            })
            .collect()
    };
    let (pg, pc) = (profile(&gpu), profile(&cpu));
    let err = |a: &[f64], b: &[f64]| -> f64 {
        a.iter().zip(b).map(|(x, y)| (x - y).abs()).sum::<f64>() / a.len() as f64
    };

    let mut flipped = pg.clone();
    flipped.reverse();
    let (straight, mirrored) = (err(&pg, &pc), err(&flipped, &pc));
    assert!(
        straight < mirrored,
        "the GPU isometric render matches the CPU one better MIRRORED \
         ({mirrored:.3}) than straight ({straight:.3}) -- the camera basis is \
         being rebuilt right-handedly instead of handed over verbatim"
    );
}

/// `--backdrop none` must leave the background transparent, exactly as the
/// CPU path does. This is the shader's `background_mode` reaching the
/// coverage alpha; if it did not, the escaped rays would paint the viewport's
/// themed sky and the PNG would be opaque.
#[test]
fn backdrop_none_leaves_the_background_transparent() {
    let opts = raster_opts(96);
    let pr = PhotorealOptions {
        backdrop: Backdrop::None,
        ..photoreal_opts(16)
    };
    let Some(png) = gpu_png(&opts, &pr) else {
        return;
    };
    let img = image::load_from_memory(&png).expect("valid PNG").to_rgba8();

    let clear = img.pixels().filter(|p| p.0[3] < 8).count();
    let opaque = img.pixels().filter(|p| p.0[3] > 200).count();
    assert!(
        clear > 0 && opaque > 0,
        "expected a mix of covered and transparent pixels, got \
         {clear} clear / {opaque} opaque of {}",
        img.pixels().len()
    );
}
