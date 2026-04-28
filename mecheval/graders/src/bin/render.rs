//! `mecheval-render` — project a `.vcad` to a static isometric SVG.
//!
//! Used to render the OPERATOR mascot, task reference parts, and run
//! artifacts (model-built .vcads) for the leaderboard. Output is a single
//! self-contained <svg> on stdout.
//!
//! Style: low-poly painter's-algorithm shading. Each tessellated triangle
//! becomes one <polygon>. Fill is the project ink at a Lambertian
//! intensity computed from the triangle normal vs a fixed light direction
//! (looks like a faceted technical drawing). Strokes are thin and
//! consistent so the result reads as drafting linework, not 3D rendering.
//!
//! Usage:
//!   mecheval-render <path.vcad> [--accent <#hex>] [--scale <px-per-mm>]

use mecheval_grader::eval::evaluate_vcad;
use std::path::PathBuf;
use std::process::ExitCode;

// ─── tunable knobs ────────────────────────────────────────────────────────

/// SVG units per kernel mm at the default --scale.
const DEFAULT_SCALE: f64 = 2.0;
const PADDING_PX: f64 = 8.0;
const STROKE_PX: f64 = 0.4;
const TESSELLATION_SEGMENTS: u32 = 28;

/// Light direction in kernel space (normalized at use time). Pointing
/// roughly from upper-front-left so faces facing camera read brighter.
const LIGHT: [f64; 3] = [-0.6, -0.7, 0.8];

/// Default fill is the project blueprint cyan; lit by Lambert.
const FILL_DARK: [u8; 3] = [14, 57, 96]; // #0e3960
const FILL_LIGHT: [u8; 3] = [200, 220, 235]; // pale cyan
const STROKE: &str = "#0e3960";

// ─── argument parsing ─────────────────────────────────────────────────────

struct Args {
    path: PathBuf,
    scale: f64,
}

fn parse_args() -> Result<Args, String> {
    let mut args = std::env::args().skip(1);
    let path = args.next().ok_or("usage: mecheval-render <path.vcad> [--scale N]")?;
    let mut scale = DEFAULT_SCALE;
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--scale" => {
                let v = args.next().ok_or("--scale needs a value")?;
                scale = v.parse().map_err(|e: std::num::ParseFloatError| e.to_string())?;
            }
            other => return Err(format!("unknown flag: {}", other)),
        }
    }
    Ok(Args { path: PathBuf::from(path), scale })
}

// ─── projection ───────────────────────────────────────────────────────────

/// Standard 30° isometric. Z is up in kernel space; SVG y is down.
fn project(p: [f64; 3], scale: f64) -> (f64, f64) {
    const COS30: f64 = 0.866_025_403_784_438_6;
    const SIN30: f64 = 0.5;
    let sx = (p[0] - p[1]) * COS30 * scale;
    let sy = ((p[0] + p[1]) * SIN30 - p[2]) * scale;
    (sx, sy)
}

/// Higher = closer to camera. Used for painter's-algorithm sort.
fn depth(p: [f64; 3]) -> f64 {
    p[0] + p[1] + p[2]
}

// ─── main ─────────────────────────────────────────────────────────────────

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

    let snap = evaluate_vcad(&raw);
    if let Some(err) = &snap.fatal {
        eprintln!("eval error: {}", err);
        return ExitCode::from(2);
    }
    if snap.solids.is_empty() {
        eprintln!("no solids produced");
        return ExitCode::from(2);
    }

    // 1. Collect every triangle from every solid.
    struct Tri {
        v: [[f64; 3]; 3],
        normal: [f64; 3],
        depth: f64,
    }
    let mut tris: Vec<Tri> = Vec::new();
    for solid in &snap.solids {
        let mesh = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            solid.to_mesh(TESSELLATION_SEGMENTS)
        }));
        let Ok(mesh) = mesh else { continue };
        if mesh.indices.is_empty() {
            continue;
        }
        for chunk in mesh.indices.chunks_exact(3) {
            let mut v = [[0.0; 3]; 3];
            for (i, &idx) in chunk.iter().enumerate() {
                let off = (idx as usize) * 3;
                if off + 2 >= mesh.vertices.len() {
                    continue;
                }
                v[i] = [
                    mesh.vertices[off] as f64,
                    mesh.vertices[off + 1] as f64,
                    mesh.vertices[off + 2] as f64,
                ];
            }
            let normal = face_normal(v);
            let centroid = [
                (v[0][0] + v[1][0] + v[2][0]) / 3.0,
                (v[0][1] + v[1][1] + v[2][1]) / 3.0,
                (v[0][2] + v[1][2] + v[2][2]) / 3.0,
            ];
            tris.push(Tri {
                v,
                normal,
                depth: depth(centroid),
            });
        }
    }

    if tris.is_empty() {
        eprintln!("zero triangles — nothing to render");
        return ExitCode::from(2);
    }

    // 2. Cull back-faces (cheap visibility cull — assumes outward normals).
    let cam = normalize([1.0, 1.0, 1.0]);
    tris.retain(|t| dot(t.normal, cam) >= -0.05);

    // 3. Project every vertex to 2D and remember projected triangles.
    struct PTri {
        s: [(f64, f64); 3],
        intensity: f64,
        depth: f64,
    }
    let light = normalize(LIGHT);
    let mut ptris: Vec<PTri> = tris
        .iter()
        .map(|t| PTri {
            s: [
                project(t.v[0], args.scale),
                project(t.v[1], args.scale),
                project(t.v[2], args.scale),
            ],
            intensity: lambertian(t.normal, light),
            depth: t.depth,
        })
        .collect();

    // 4. Sort back-to-front (lower depth first).
    ptris.sort_by(|a, b| a.depth.partial_cmp(&b.depth).unwrap_or(std::cmp::Ordering::Equal));

    // 5. Compute viewBox.
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for t in &ptris {
        for &(x, y) in &t.s {
            if x < min_x { min_x = x; }
            if y < min_y { min_y = y; }
            if x > max_x { max_x = x; }
            if y > max_y { max_y = y; }
        }
    }
    let w = (max_x - min_x) + 2.0 * PADDING_PX;
    let h = (max_y - min_y) + 2.0 * PADDING_PX;

    // 6. Emit SVG.
    let mut out = String::new();
    out.push_str(&format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w:.2} {h:.2}" role="img" aria-label="vcad render">"#
    ));
    out.push_str(r#"<g stroke-linejoin="round">"#);
    for t in &ptris {
        let pts = format!(
            "{:.2},{:.2} {:.2},{:.2} {:.2},{:.2}",
            t.s[0].0 - min_x + PADDING_PX,
            t.s[0].1 - min_y + PADDING_PX,
            t.s[1].0 - min_x + PADDING_PX,
            t.s[1].1 - min_y + PADDING_PX,
            t.s[2].0 - min_x + PADDING_PX,
            t.s[2].1 - min_y + PADDING_PX,
        );
        let fill = mix_color(FILL_DARK, FILL_LIGHT, t.intensity.clamp(0.0, 1.0));
        out.push_str(&format!(
            r#"<polygon points="{pts}" fill="{fill}" stroke="{STROKE}" stroke-width="{STROKE_PX}"/>"#
        ));
    }
    out.push_str("</g></svg>");
    println!("{}", out);
    ExitCode::SUCCESS
}

// ─── tiny vec / shading helpers ───────────────────────────────────────────

fn face_normal(v: [[f64; 3]; 3]) -> [f64; 3] {
    let e1 = sub(v[1], v[0]);
    let e2 = sub(v[2], v[0]);
    normalize(cross(e1, e2))
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn normalize(v: [f64; 3]) -> [f64; 3] {
    let m = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if m < 1e-12 { [0.0, 0.0, 0.0] } else { [v[0] / m, v[1] / m, v[2] / m] }
}

fn lambertian(n: [f64; 3], light: [f64; 3]) -> f64 {
    // Wrap from -1..1 to 0..1 so the back face still renders (we already
    // cull aggressively-backfacing triangles; what remains is mostly
    // visible, and a soft wrap reads better than hard zero).
    let d = dot(n, light);
    (d * 0.5 + 0.5).powf(0.85)
}

fn mix_color(a: [u8; 3], b: [u8; 3], t: f64) -> String {
    let mix = |x: u8, y: u8| ((x as f64) * (1.0 - t) + (y as f64) * t).round() as u8;
    format!("#{:02x}{:02x}{:02x}", mix(a[0], b[0]), mix(a[1], b[1]), mix(a[2], b[2]))
}
