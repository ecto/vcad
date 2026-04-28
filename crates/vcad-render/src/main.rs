//! `vcad-render` — project a `.vcad` to a static isometric SVG.
//!
//! Drafting-style line art: low-poly painter's-algorithm shading with
//! proper edge classification so tessellated flat faces don't look like
//! a fan of slivers. Each triangulation edge is emitted as one of:
//!
//!   - boundary  (1 triangle)                   → drawn as silhouette
//!   - crease    (2 tris, non-parallel normals) → drawn
//!   - internal  (2 tris, parallel normals)     → hidden
//!
//! Filled polygons get no stroke; the edge lines get strokes. The result
//! reads as drafting linework, not 3D rendering.
//!
//! Usage:
//!   vcad-render <path.vcad> [--scale <px-per-mm>]
//!
//! Output: a single self-contained `<svg>` on stdout.
//!
//! Originally lived as `mecheval-render` inside the mecheval grader crate.
//! Promoted to a standalone vcad tool so other consumers (docs, marketing,
//! task previewers) can use it without depending on the grader.

use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::process::ExitCode;

use vcad_eval::{evaluate_document, EvalOptions};
use vcad_ir::file_io::parse_vcad_file;
use vcad_kernel::Solid;

// ─── tunable knobs ────────────────────────────────────────────────────────

const DEFAULT_SCALE: f64 = 2.0;
const PADDING_PX: f64 = 8.0;
const STROKE_PX: f64 = 0.5;
const TESSELLATION_SEGMENTS: u32 = 28;

/// Two triangles are considered coplanar when their unit normals' dot
/// product exceeds this threshold. Tighter values reveal more creases;
/// looser values hide more internal edges.
const COPLANAR_DOT_TOL: f64 = 0.997; // cos(~4.5°)

/// Cull triangles whose normal points away from the camera by more than
/// this margin. Anything just-barely-back-facing stays so silhouette
/// edges don't disappear.
const BACKFACE_DOT_MIN: f64 = -0.04;

/// Vertex deduplication tolerance — two positions within this many mm
/// collapse to a single canonical vertex. Critical for proper edge
/// classification on tessellated curved surfaces, which often emit the
/// same vertex twice via separate facets.
const VERT_DEDUP_MM: f64 = 1e-3;

/// Light direction in kernel space (Z-up).
const LIGHT: [f64; 3] = [-0.6, -0.7, 0.8];

const FILL_DARK: [u8; 3] = [14, 57, 96];
const FILL_LIGHT: [u8; 3] = [200, 220, 235];
const STROKE: &str = "#0e3960";

// ─── argument parsing ─────────────────────────────────────────────────────

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

// ─── .vcad → solids ───────────────────────────────────────────────────────

fn evaluate_vcad(raw_vcad: &str) -> Result<Vec<Solid>, String> {
    let parsed = parse_vcad_file(raw_vcad).map_err(|e| format!("parse: {}", e))?;
    let scene = catch_unwind(AssertUnwindSafe(|| {
        evaluate_document(&parsed.document, &EvalOptions::default())
    }))
    .map_err(|_| "eval panicked".to_string())?
    .map_err(|e| format!("eval: {}", e))?;

    let solids: Vec<Solid> = scene
        .parts
        .iter()
        .filter_map(|p| p.solid.clone())
        .collect();
    Ok(solids)
}

// ─── projection ───────────────────────────────────────────────────────────

fn project(p: [f64; 3], scale: f64) -> (f64, f64) {
    const COS30: f64 = 0.866_025_403_784_438_6;
    const SIN30: f64 = 0.5;
    let sx = (p[0] - p[1]) * COS30 * scale;
    let sy = ((p[0] + p[1]) * SIN30 - p[2]) * scale;
    (sx, sy)
}

fn depth(p: [f64; 3]) -> f64 {
    p[0] + p[1] + p[2]
}

// ─── canonicalized per-solid mesh ─────────────────────────────────────────

struct CanonMesh {
    /// Canonical vertices in 3D (kernel space).
    verts: Vec<[f64; 3]>,
    /// Triangles as triples of canonical-vertex indices.
    tris: Vec<[usize; 3]>,
}

fn canonicalize(mesh: &vcad_kernel::vcad_kernel_tessellate::TriangleMesh) -> CanonMesh {
    let scale = 1.0 / VERT_DEDUP_MM;
    let mut verts: Vec<[f64; 3]> = Vec::new();
    let mut canon: HashMap<(i64, i64, i64), usize> = HashMap::new();
    let mut tris: Vec<[usize; 3]> = Vec::new();
    for chunk in mesh.indices.chunks_exact(3) {
        let mut tri = [0usize; 3];
        let mut ok = true;
        for i in 0..3 {
            let off = chunk[i] as usize * 3;
            if off + 2 >= mesh.vertices.len() {
                ok = false;
                break;
            }
            let p = [
                mesh.vertices[off] as f64,
                mesh.vertices[off + 1] as f64,
                mesh.vertices[off + 2] as f64,
            ];
            let key = (
                (p[0] * scale).round() as i64,
                (p[1] * scale).round() as i64,
                (p[2] * scale).round() as i64,
            );
            let id = *canon.entry(key).or_insert_with(|| {
                verts.push(p);
                verts.len() - 1
            });
            tri[i] = id;
        }
        if !ok {
            continue;
        }
        // Drop degenerate triangles (any two canon vertices coincide).
        if tri[0] == tri[1] || tri[1] == tri[2] || tri[0] == tri[2] {
            continue;
        }
        tris.push(tri);
    }
    CanonMesh { verts, tris }
}

// ─── geometry helpers ─────────────────────────────────────────────────────

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
    if m < 1e-12 {
        [0.0, 0.0, 0.0]
    } else {
        [v[0] / m, v[1] / m, v[2] / m]
    }
}

fn lambertian(n: [f64; 3], light: [f64; 3]) -> f64 {
    let d = dot(n, light);
    (d * 0.5 + 0.5).powf(0.85)
}

fn mix_color(a: [u8; 3], b: [u8; 3], t: f64) -> String {
    let mix = |x: u8, y: u8| ((x as f64) * (1.0 - t) + (y as f64) * t).round() as u8;
    format!(
        "#{:02x}{:02x}{:02x}",
        mix(a[0], b[0]),
        mix(a[1], b[1]),
        mix(a[2], b[2]),
    )
}

// ─── output buffers ───────────────────────────────────────────────────────

#[derive(Clone)]
struct ProjPoly {
    s: [(f64, f64); 3],
    fill: String,
    depth: f64,
}

#[derive(Clone)]
struct ProjEdge {
    a: (f64, f64),
    b: (f64, f64),
    depth: f64,
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

    let solids = match evaluate_vcad(&raw) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{}", e);
            return ExitCode::from(2);
        }
    };
    if solids.is_empty() {
        eprintln!("no solids produced");
        return ExitCode::from(2);
    }

    let cam = normalize([1.0, 1.0, 1.0]);
    let light = normalize(LIGHT);

    let mut polys: Vec<ProjPoly> = Vec::new();
    let mut edges: Vec<ProjEdge> = Vec::new();

    for solid in &solids {
        let mesh = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            solid.to_mesh(TESSELLATION_SEGMENTS)
        }));
        let Ok(mesh) = mesh else { continue };
        if mesh.indices.is_empty() {
            continue;
        }
        let cm = canonicalize(&mesh);
        if cm.tris.is_empty() {
            continue;
        }

        // Per-triangle normal + back-face visibility.
        let normals: Vec<[f64; 3]> = cm
            .tris
            .iter()
            .map(|t| face_normal([cm.verts[t[0]], cm.verts[t[1]], cm.verts[t[2]]]))
            .collect();
        let visible: Vec<bool> = normals
            .iter()
            .map(|n| dot(*n, cam) >= BACKFACE_DOT_MIN)
            .collect();

        // Build undirected-edge → adjacent-triangle-indices map.
        let mut edge_to_tris: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
        for (ti, t) in cm.tris.iter().enumerate() {
            for i in 0..3 {
                let a = t[i];
                let b = t[(i + 1) % 3];
                let edge = if a < b { (a, b) } else { (b, a) };
                edge_to_tris.entry(edge).or_default().push(ti);
            }
        }

        // Project canonical vertices once.
        let proj: Vec<(f64, f64)> = cm.verts.iter().map(|v| project(*v, args.scale)).collect();

        // Emit visible polygons.
        for (ti, t) in cm.tris.iter().enumerate() {
            if !visible[ti] {
                continue;
            }
            let centroid = [
                (cm.verts[t[0]][0] + cm.verts[t[1]][0] + cm.verts[t[2]][0]) / 3.0,
                (cm.verts[t[0]][1] + cm.verts[t[1]][1] + cm.verts[t[2]][1]) / 3.0,
                (cm.verts[t[0]][2] + cm.verts[t[1]][2] + cm.verts[t[2]][2]) / 3.0,
            ];
            polys.push(ProjPoly {
                s: [proj[t[0]], proj[t[1]], proj[t[2]]],
                fill: mix_color(
                    FILL_DARK,
                    FILL_LIGHT,
                    lambertian(normals[ti], light).clamp(0.0, 1.0),
                ),
                depth: depth(centroid),
            });
        }

        // Emit boundary + crease edges. Skip internal ones.
        for (edge, adj_tris) in &edge_to_tris {
            // Visibility: at least one adjacent triangle must be visible,
            // otherwise we'd be drawing a hidden edge over the silhouette.
            let any_visible = adj_tris.iter().any(|&ti| visible[ti]);
            if !any_visible {
                continue;
            }
            let keep = match adj_tris.len() {
                1 => true, // boundary / silhouette
                2 => {
                    // Crease: only if normals are not coplanar.
                    let cosang = dot(normals[adj_tris[0]], normals[adj_tris[1]]).abs();
                    cosang < COPLANAR_DOT_TOL
                }
                _ => true, // weird (non-manifold); render conservatively
            };
            if !keep {
                continue;
            }
            let mid = [
                (cm.verts[edge.0][0] + cm.verts[edge.1][0]) / 2.0,
                (cm.verts[edge.0][1] + cm.verts[edge.1][1]) / 2.0,
                (cm.verts[edge.0][2] + cm.verts[edge.1][2]) / 2.0,
            ];
            edges.push(ProjEdge {
                a: proj[edge.0],
                b: proj[edge.1],
                depth: depth(mid),
            });
        }
    }

    if polys.is_empty() {
        eprintln!("no visible polygons after culling");
        return ExitCode::from(2);
    }

    // Painter's algorithm — back-to-front.
    polys.sort_by(|a, b| {
        a.depth
            .partial_cmp(&b.depth)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    edges.sort_by(|a, b| {
        a.depth
            .partial_cmp(&b.depth)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // BBox over polygons + edges.
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for p in &polys {
        for &(x, y) in &p.s {
            if x < min_x {
                min_x = x;
            }
            if y < min_y {
                min_y = y;
            }
            if x > max_x {
                max_x = x;
            }
            if y > max_y {
                max_y = y;
            }
        }
    }
    for e in &edges {
        for &(x, y) in &[e.a, e.b] {
            if x < min_x {
                min_x = x;
            }
            if y < min_y {
                min_y = y;
            }
            if x > max_x {
                max_x = x;
            }
            if y > max_y {
                max_y = y;
            }
        }
    }
    let w = (max_x - min_x) + 2.0 * PADDING_PX;
    let h = (max_y - min_y) + 2.0 * PADDING_PX;

    // Emit SVG.
    let mut out = String::new();
    out.push_str(&format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w:.2} {h:.2}" role="img" aria-label="vcad render">"#
    ));

    // Filled polygons — no stroke.
    out.push_str(r#"<g shape-rendering="geometricPrecision">"#);
    for p in &polys {
        let pts = format!(
            "{:.2},{:.2} {:.2},{:.2} {:.2},{:.2}",
            p.s[0].0 - min_x + PADDING_PX,
            p.s[0].1 - min_y + PADDING_PX,
            p.s[1].0 - min_x + PADDING_PX,
            p.s[1].1 - min_y + PADDING_PX,
            p.s[2].0 - min_x + PADDING_PX,
            p.s[2].1 - min_y + PADDING_PX,
        );
        out.push_str(&format!(r#"<polygon points="{}" fill="{}"/>"#, pts, p.fill,));
    }
    out.push_str("</g>");

    // Boundary + crease edges on top.
    out.push_str(&format!(
        r#"<g stroke="{STROKE}" stroke-width="{STROKE_PX}" stroke-linecap="round" fill="none">"#
    ));
    for e in &edges {
        out.push_str(&format!(
            r#"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}"/>"#,
            e.a.0 - min_x + PADDING_PX,
            e.a.1 - min_y + PADDING_PX,
            e.b.0 - min_x + PADDING_PX,
            e.b.1 - min_y + PADDING_PX,
        ));
    }
    out.push_str("</g></svg>");

    println!("{}", out);
    ExitCode::SUCCESS
}
