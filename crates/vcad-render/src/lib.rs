//! `vcad-render` — project a `.vcad` to static line art.
//!
//! Drafting-style rendering: low-poly painter's-algorithm shading with
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
//! Two output paths share the same projection and edge classification:
//!
//!   - SVG: [`render_svg_str`] / [`render_svg_solids`] (isometric, the
//!     historical default) and the [`View`]-parameterised
//!     [`render_svg_str_view`] / [`render_svg_solids_view`].
//!   - Raster JPEG (behind the default `raster` feature):
//!     [`render_jpeg_str`] / [`render_jpeg_solids`], z-buffered with
//!     hidden-line-removed edge overlay. Used to generate the mecheval
//!     task reference images (front/side/top views).
//!
//! The `vcad-render` binary is a thin CLI wrapper over both.
//!
//! Originally lived as `mecheval-render` inside the mecheval grader crate.
//! Promoted to a standalone vcad tool so other consumers (docs, marketing,
//! task previewers, the MCP `render_view` tool) can use it without
//! depending on the grader.

#![warn(missing_docs)]

pub mod pcb;

use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};

use vcad_eval::{evaluate_document, EvalOptions};
use vcad_ir::file_io::parse_vcad_file;
use vcad_kernel::Solid;

// ─── tunable knobs ────────────────────────────────────────────────────────

/// Default pixels-per-millimetre when no `--scale` is given.
pub const DEFAULT_SCALE: f64 = 2.0;
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

// ─── views ────────────────────────────────────────────────────────────────

const COS30: f64 = 0.866_025_403_784_438_6;
const SIN30: f64 = 0.5;

/// Camera orientation for a render.
///
/// Axis-aligned views follow the mecheval reference-image convention
/// (kernel space is Z-up):
///
/// - [`View::Front`] looks down **+Y** (camera on the −Y side, +Z up).
/// - [`View::Side`]  looks down **+X** (camera on the −X side, +Z up).
/// - [`View::Top`]   looks down **−Z** (camera above, +Y up in image).
///
/// [`View::Isometric`] is the historical 3/4 view from the (+X, +Y, +Z)
/// octant, also used for "hero" shots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum View {
    /// 3/4 view from the (+1, +1, +1) direction (the historical default).
    #[default]
    Isometric,
    /// Orthographic, looking down +Y. Screen: +X right, +Z up.
    Front,
    /// Orthographic, looking down +X. Screen: −Y right, +Z up.
    Side,
    /// Orthographic, looking down −Z. Screen: +X right, +Y up.
    Top,
}

impl View {
    /// Unit vector pointing from the scene toward the camera. Used for
    /// back-face culling and depth ordering.
    fn cam(self) -> [f64; 3] {
        match self {
            View::Isometric => normalize([1.0, 1.0, 1.0]),
            View::Front => [0.0, -1.0, 0.0],
            View::Side => [-1.0, 0.0, 0.0],
            View::Top => [0.0, 0.0, 1.0],
        }
    }

    /// World direction mapping to screen +x.
    fn right(self) -> [f64; 3] {
        match self {
            // Non-unit on purpose: preserves the exact legacy isometric
            // projection (uniform √1.5 scale vs the axis views).
            View::Isometric => [COS30, -COS30, 0.0],
            View::Front => [1.0, 0.0, 0.0],
            View::Side => [0.0, -1.0, 0.0],
            View::Top => [1.0, 0.0, 0.0],
        }
    }

    /// World direction mapping to screen +y (SVG/raster y grows down).
    fn down(self) -> [f64; 3] {
        match self {
            View::Isometric => [SIN30, SIN30, -1.0],
            View::Front => [0.0, 0.0, -1.0],
            View::Side => [0.0, 0.0, -1.0],
            View::Top => [0.0, -1.0, 0.0],
        }
    }
}

impl std::str::FromStr for View {
    type Err = String;

    /// Accepts `iso`/`isometric` (and `hero`, which renders as the same
    /// 3/4 view), `front`, `side`, `top`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "iso" | "isometric" | "hero" => Ok(View::Isometric),
            "front" => Ok(View::Front),
            "side" => Ok(View::Side),
            "top" => Ok(View::Top),
            other => Err(format!(
                "unknown view '{other}' (expected iso|front|side|top|hero)"
            )),
        }
    }
}

// ─── .vcad → solids ───────────────────────────────────────────────────────

fn evaluate_vcad(raw_vcad: &str) -> Result<Vec<Solid>, String> {
    let parsed = parse_vcad_file(raw_vcad).map_err(|e| format!("parse: {}", e))?;
    // NOTE: catch_unwind only works on native targets. On
    // wasm32-unknown-unknown a panic compiles to an `unreachable` trap —
    // it never unwinds, this guard never fires, and the WASM instance is
    // left in an undefined state. The JS caller is responsible for
    // catching the trap (WebAssembly.RuntimeError) and poisoning the
    // shared instance; see packages/mcp/src/tools/render.ts.
    let scene = catch_unwind(AssertUnwindSafe(|| {
        evaluate_document(&parsed.document, &EvalOptions::default())
    }))
    .map_err(|_| "eval panicked".to_string())?
    .map_err(|e| format!("eval: {}", e))?;

    let solids: Vec<Solid> = scene.parts.iter().filter_map(|p| p.solid.clone()).collect();
    Ok(solids)
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

// ─── per-solid render artifacts (shared by SVG + raster paths) ────────────

struct SolidArtifacts {
    verts: Vec<[f64; 3]>,
    tris: Vec<[usize; 3]>,
    normals: Vec<[f64; 3]>,
    visible: Vec<bool>,
    /// Kept edges (boundary, crease, or silhouette, with ≥1 visible
    /// adjacent triangle) as canonical-vertex index pairs.
    edges: Vec<(usize, usize)>,
}

/// How `build_artifacts` decides which mesh edges to keep as linework.
struct EdgeRules {
    /// Two adjacent triangles count as coplanar (edge hidden) when their
    /// normals' dot product exceeds this.
    coplanar_dot_tol: f64,
    /// Also keep edges where one adjacent triangle faces the camera and
    /// the other faces away — explicit silhouette outlines. The raster
    /// path needs this because its loose `coplanar_dot_tol` would
    /// otherwise swallow the outline of finely tessellated cylinders.
    mark_silhouette: bool,
}

impl EdgeRules {
    /// The historical SVG behaviour.
    fn svg() -> Self {
        EdgeRules {
            coplanar_dot_tol: COPLANAR_DOT_TOL,
            mark_silhouette: false,
        }
    }
}

fn build_artifacts(
    solids: &[Solid],
    cam: [f64; 3],
    segments: u32,
    rules: &EdgeRules,
) -> Vec<SolidArtifacts> {
    let mut out = Vec::new();
    for solid in solids {
        let mesh = catch_unwind(AssertUnwindSafe(|| solid.to_mesh(segments)));
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

        // Keep boundary + crease edges. Skip internal ones.
        let mut edges: Vec<(usize, usize)> = Vec::new();
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
                    let (t0, t1) = (adj_tris[0], adj_tris[1]);
                    let silhouette = rules.mark_silhouette
                        && (dot(normals[t0], cam) >= 0.0) != (dot(normals[t1], cam) >= 0.0);
                    // Crease: only if normals are not coplanar.
                    let cosang = dot(normals[t0], normals[t1]).abs();
                    silhouette || cosang < rules.coplanar_dot_tol
                }
                _ => true, // weird (non-manifold); render conservatively
            };
            if keep {
                edges.push(*edge);
            }
        }

        out.push(SolidArtifacts {
            verts: cm.verts,
            tris: cm.tris,
            normals,
            visible,
            edges,
        });
    }
    out
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

fn mix_rgb(a: [u8; 3], b: [u8; 3], t: f64) -> [u8; 3] {
    let mix = |x: u8, y: u8| ((x as f64) * (1.0 - t) + (y as f64) * t).round() as u8;
    [mix(a[0], b[0]), mix(a[1], b[1]), mix(a[2], b[2])]
}

fn mix_color(a: [u8; 3], b: [u8; 3], t: f64) -> String {
    let [r, g, bl] = mix_rgb(a, b, t);
    format!("#{:02x}{:02x}{:02x}", r, g, bl)
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

// ─── public API: SVG ──────────────────────────────────────────────────────

/// Render raw `.vcad` document JSON to a self-contained isometric SVG.
///
/// `scale` is pixels per millimetre ([`DEFAULT_SCALE`] when in doubt).
/// Errors are human-readable strings ("parse: …", "eval: …",
/// "no solids produced") suitable for surfacing directly to a caller.
pub fn render_svg_str(raw_vcad: &str, scale: f64) -> Result<String, String> {
    render_svg_str_view(raw_vcad, scale, View::Isometric)
}

/// Render raw `.vcad` document JSON to a self-contained SVG from `view`.
pub fn render_svg_str_view(raw_vcad: &str, scale: f64, view: View) -> Result<String, String> {
    let solids = evaluate_vcad(raw_vcad)?;
    render_svg_solids_view(&solids, scale, view)
}

/// Render pre-evaluated solids to a self-contained isometric SVG.
///
/// `scale` is pixels per millimetre. Returns an error when no solids are
/// given or nothing survives tessellation + back-face culling.
pub fn render_svg_solids(solids: &[Solid], scale: f64) -> Result<String, String> {
    render_svg_solids_view(solids, scale, View::Isometric)
}

/// Render pre-evaluated solids to a self-contained SVG from `view`.
pub fn render_svg_solids_view(solids: &[Solid], scale: f64, view: View) -> Result<String, String> {
    if solids.is_empty() {
        return Err("no solids produced".to_string());
    }

    let cam = view.cam();
    let right = view.right();
    let down = view.down();
    let light = normalize(LIGHT);
    let project = |p: [f64; 3]| -> (f64, f64) { (dot(p, right) * scale, dot(p, down) * scale) };

    let mut polys: Vec<ProjPoly> = Vec::new();
    let mut edges: Vec<ProjEdge> = Vec::new();

    for art in build_artifacts(solids, cam, TESSELLATION_SEGMENTS, &EdgeRules::svg()) {
        // Project canonical vertices once.
        let proj: Vec<(f64, f64)> = art.verts.iter().map(|v| project(*v)).collect();

        // Emit visible polygons.
        for (ti, t) in art.tris.iter().enumerate() {
            if !art.visible[ti] {
                continue;
            }
            let centroid = [
                (art.verts[t[0]][0] + art.verts[t[1]][0] + art.verts[t[2]][0]) / 3.0,
                (art.verts[t[0]][1] + art.verts[t[1]][1] + art.verts[t[2]][1]) / 3.0,
                (art.verts[t[0]][2] + art.verts[t[1]][2] + art.verts[t[2]][2]) / 3.0,
            ];
            polys.push(ProjPoly {
                s: [proj[t[0]], proj[t[1]], proj[t[2]]],
                fill: mix_color(
                    FILL_DARK,
                    FILL_LIGHT,
                    lambertian(art.normals[ti], light).clamp(0.0, 1.0),
                ),
                depth: dot(centroid, cam),
            });
        }

        for &(a, b) in &art.edges {
            let mid = [
                (art.verts[a][0] + art.verts[b][0]) / 2.0,
                (art.verts[a][1] + art.verts[b][1]) / 2.0,
                (art.verts[a][2] + art.verts[b][2]) / 2.0,
            ];
            edges.push(ProjEdge {
                a: proj[a],
                b: proj[b],
                depth: dot(mid, cam),
            });
        }
    }

    if polys.is_empty() {
        return Err("no visible polygons after culling".to_string());
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
    // Emit explicit width/height alongside viewBox so the SVG has intrinsic
    // dimensions. Without them, browsers compute auto/auto as 0×0 inside flex
    // containers (Chrome/Safari), which silently hides the render.
    out.push_str(&format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w:.2}" height="{h:.2}" viewBox="0 0 {w:.2} {h:.2}" role="img" aria-label="vcad render">"#
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

    Ok(out)
}

// ─── public API: raster JPEG ──────────────────────────────────────────────

#[cfg(feature = "raster")]
pub use raster::{render_jpeg_solids, render_jpeg_str, RasterOptions};

#[cfg(feature = "raster")]
mod raster {
    use super::*;

    /// Curved primitives get a finer tessellation than the SVG path —
    /// faceted silhouettes are much more visible at 1024px.
    const RASTER_SEGMENTS: u32 = 64;

    /// Looser coplanar tolerance than the SVG path: at 64 segments,
    /// adjacent cylinder facets differ by 5.6°, which the SVG's ~4.5°
    /// threshold would draw as stripes down every curved face. Hiding
    /// everything under ~10° keeps real creases (chamfers, fillet rims)
    /// while letting tessellation facets blend together.
    const RASTER_COPLANAR_DOT_TOL: f64 = 0.985; // cos(~10°)

    /// Matte, neutral background per the mecheval capture rules.
    const BACKGROUND: [u8; 3] = [244, 243, 241];

    /// Options for [`render_jpeg_str`] / [`render_jpeg_solids`].
    #[derive(Debug, Clone)]
    pub struct RasterOptions {
        /// Camera orientation.
        pub view: View,
        /// Output canvas is `size_px` × `size_px`.
        pub size_px: u32,
        /// Fraction of the canvas the part's long axis fills (mecheval
        /// capture rules say ~60%).
        pub fill_frac: f64,
        /// JPEG quality, 1–100 (capture rules say ≥ 90).
        pub quality: u8,
    }

    impl Default for RasterOptions {
        fn default() -> Self {
            RasterOptions {
                view: View::Isometric,
                size_px: 1024,
                fill_frac: 0.6,
                quality: 92,
            }
        }
    }

    /// Render raw `.vcad` document JSON to JPEG bytes.
    ///
    /// Orthographic projection from `opts.view`, z-buffered flat shading
    /// with the same edge classification as the SVG path drawn on top
    /// (hidden lines removed). Errors are human-readable strings.
    pub fn render_jpeg_str(raw_vcad: &str, opts: &RasterOptions) -> Result<Vec<u8>, String> {
        let solids = evaluate_vcad(raw_vcad)?;
        render_jpeg_solids(&solids, opts)
    }

    /// Render pre-evaluated solids to JPEG bytes. See [`render_jpeg_str`].
    pub fn render_jpeg_solids(solids: &[Solid], opts: &RasterOptions) -> Result<Vec<u8>, String> {
        if solids.is_empty() {
            return Err("no solids produced".to_string());
        }
        if opts.size_px < 16 {
            return Err("size_px too small".to_string());
        }
        if !(opts.fill_frac > 0.0 && opts.fill_frac <= 1.0) {
            return Err("fill_frac must be in (0, 1]".to_string());
        }

        let cam = opts.view.cam();
        let right = normalize(opts.view.right());
        let down = normalize(opts.view.down());
        let light = normalize(LIGHT);

        let arts = build_artifacts(
            solids,
            cam,
            RASTER_SEGMENTS,
            &EdgeRules {
                coplanar_dot_tol: RASTER_COPLANAR_DOT_TOL,
                mark_silhouette: true,
            },
        );

        // Screen-plane (mm) and 3D bounding boxes over all vertices.
        let mut min = [f64::INFINITY; 2];
        let mut max = [f64::NEG_INFINITY; 2];
        let mut lo3 = [f64::INFINITY; 3];
        let mut hi3 = [f64::NEG_INFINITY; 3];
        for art in &arts {
            for v in &art.verts {
                let s = [dot(*v, right), dot(*v, down)];
                for i in 0..2 {
                    min[i] = min[i].min(s[i]);
                    max[i] = max[i].max(s[i]);
                }
                for i in 0..3 {
                    lo3[i] = lo3[i].min(v[i]);
                    hi3[i] = hi3[i].max(v[i]);
                }
            }
        }
        let extent = (max[0] - min[0]).max(max[1] - min[1]);
        if !extent.is_finite() || extent < 1e-9 {
            return Err("degenerate projection (no extent)".to_string());
        }
        let diag =
            ((hi3[0] - lo3[0]).powi(2) + (hi3[1] - lo3[1]).powi(2) + (hi3[2] - lo3[2]).powi(2))
                .sqrt();

        let size = opts.size_px as usize;
        let px_per_mm = opts.fill_frac * opts.size_px as f64 / extent;
        let cx = (min[0] + max[0]) / 2.0;
        let cy = (min[1] + max[1]) / 2.0;
        let half = opts.size_px as f64 / 2.0;
        // World point → (pixel x, pixel y, depth toward camera in mm).
        let to_px = |v: [f64; 3]| -> (f64, f64, f64) {
            (
                (dot(v, right) - cx) * px_per_mm + half,
                (dot(v, down) - cy) * px_per_mm + half,
                dot(v, cam),
            )
        };

        let mut rgb: Vec<u8> = BACKGROUND
            .iter()
            .copied()
            .cycle()
            .take(size * size * 3)
            .collect();
        let mut zbuf: Vec<f64> = vec![f64::NEG_INFINITY; size * size];

        // Depth range for depth cueing (below).
        let mut dmin = f64::INFINITY;
        let mut dmax = f64::NEG_INFINITY;
        for art in &arts {
            for v in &art.verts {
                let d = dot(*v, cam);
                dmin = dmin.min(d);
                dmax = dmax.max(d);
            }
        }
        let dspan = (dmax - dmin).max(1e-9);

        // Pass 1: z-buffered flat-shaded triangles. Pure Lambertian shading
        // gives every face with the same normal the same colour, which
        // makes axis-aligned recesses invisible in axis-aligned views — a
        // mild depth cue (farther → darker) separates them.
        for art in &arts {
            let proj: Vec<(f64, f64, f64)> = art.verts.iter().map(|v| to_px(*v)).collect();
            for (ti, t) in art.tris.iter().enumerate() {
                if !art.visible[ti] {
                    continue;
                }
                let centroid_d = (proj[t[0]].2 + proj[t[1]].2 + proj[t[2]].2) / 3.0;
                let cue = 0.78 + 0.22 * ((centroid_d - dmin) / dspan).clamp(0.0, 1.0);
                let shade = mix_rgb(
                    FILL_DARK,
                    FILL_LIGHT,
                    (lambertian(art.normals[ti], light) * cue).clamp(0.0, 1.0),
                );
                fill_triangle(
                    &mut rgb,
                    &mut zbuf,
                    size,
                    [proj[t[0]], proj[t[1]], proj[t[2]]],
                    shade,
                );
            }
        }

        // Pass 2: hidden-line-removed edge overlay.
        //
        // An edge sample is visible when its depth is within a bias of the
        // z-buffer at its pixel. The bias is adaptive: near-grazing faces
        // (where silhouette edges live) have steep depth gradients, so the
        // local neighbour depth delta is added to a small base tolerance.
        let bias_base = 0.5 + 0.005 * diag;
        for art in &arts {
            let proj: Vec<(f64, f64, f64)> = art.verts.iter().map(|v| to_px(*v)).collect();
            for &(a, b) in &art.edges {
                draw_edge(&mut rgb, &zbuf, size, proj[a], proj[b], bias_base);
            }
        }

        // Encode.
        let mut out = Vec::new();
        let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(
            &mut out,
            opts.quality.clamp(1, 100),
        );
        enc.encode(
            &rgb,
            opts.size_px,
            opts.size_px,
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|e| format!("jpeg encode: {}", e))?;
        Ok(out)
    }

    fn edge_fn(a: (f64, f64, f64), b: (f64, f64, f64), px: f64, py: f64) -> f64 {
        (b.0 - a.0) * (py - a.1) - (b.1 - a.1) * (px - a.0)
    }

    fn fill_triangle(
        rgb: &mut [u8],
        zbuf: &mut [f64],
        size: usize,
        p: [(f64, f64, f64); 3],
        shade: [u8; 3],
    ) {
        let area = edge_fn(p[0], p[1], p[2].0, p[2].1);
        if area.abs() < 1e-12 {
            return;
        }
        let xs = [p[0].0, p[1].0, p[2].0];
        let ys = [p[0].1, p[1].1, p[2].1];
        let x0 = xs
            .iter()
            .cloned()
            .fold(f64::INFINITY, f64::min)
            .floor()
            .max(0.0) as usize;
        let y0 = ys
            .iter()
            .cloned()
            .fold(f64::INFINITY, f64::min)
            .floor()
            .max(0.0) as usize;
        let x1 = xs
            .iter()
            .cloned()
            .fold(f64::NEG_INFINITY, f64::max)
            .ceil()
            .min((size - 1) as f64) as usize;
        let y1 = ys
            .iter()
            .cloned()
            .fold(f64::NEG_INFINITY, f64::max)
            .ceil()
            .min((size - 1) as f64) as usize;
        if x0 > x1 || y0 > y1 {
            return;
        }
        let sign = area.signum();
        for py in y0..=y1 {
            for px in x0..=x1 {
                let cxp = px as f64 + 0.5;
                let cyp = py as f64 + 0.5;
                let w0 = edge_fn(p[1], p[2], cxp, cyp) * sign;
                let w1 = edge_fn(p[2], p[0], cxp, cyp) * sign;
                let w2 = edge_fn(p[0], p[1], cxp, cyp) * sign;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let inv = 1.0 / area.abs();
                let depth = (w0 * p[0].2 + w1 * p[1].2 + w2 * p[2].2) * inv;
                let idx = py * size + px;
                if depth > zbuf[idx] {
                    zbuf[idx] = depth;
                    rgb[idx * 3] = shade[0];
                    rgb[idx * 3 + 1] = shade[1];
                    rgb[idx * 3 + 2] = shade[2];
                }
            }
        }
    }

    fn draw_edge(
        rgb: &mut [u8],
        zbuf: &[f64],
        size: usize,
        a: (f64, f64, f64),
        b: (f64, f64, f64),
        bias_base: f64,
    ) {
        let stroke = FILL_DARK;
        let len = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
        let steps = (len * 2.0).ceil().max(1.0) as usize;
        for s in 0..=steps {
            let t = s as f64 / steps as f64;
            let x = a.0 + (b.0 - a.0) * t;
            let y = a.1 + (b.1 - a.1) * t;
            let d = a.2 + (b.2 - a.2) * t;
            let ix = x.floor() as i64;
            let iy = y.floor() as i64;
            if ix < 0 || iy < 0 || ix >= size as i64 || iy >= size as i64 {
                continue;
            }
            let (ux, uy) = (ix as usize, iy as usize);
            let idx = uy * size + ux;
            let z = zbuf[idx];
            let visible = if z == f64::NEG_INFINITY {
                // Over background: nothing occludes it.
                true
            } else {
                // Adaptive bias: steepest neighbour depth delta captures
                // grazing faces whose interpolated depth at the pixel
                // centre can be far from the edge's own depth.
                let mut delta: f64 = 0.0;
                let mut probe = |nx: i64, ny: i64| {
                    if nx >= 0 && ny >= 0 && nx < size as i64 && ny < size as i64 {
                        let nz = zbuf[ny as usize * size + nx as usize];
                        if nz != f64::NEG_INFINITY {
                            delta = delta.max((z - nz).abs());
                        } else {
                            // Background neighbour → silhouette pixel.
                            delta = f64::INFINITY;
                        }
                    }
                };
                probe(ix - 1, iy);
                probe(ix + 1, iy);
                probe(ix, iy - 1);
                probe(ix, iy + 1);
                d >= z - (bias_base + delta)
            };
            if !visible {
                continue;
            }
            // ~2px stroke: paint the pixel and its right/down neighbours.
            for (dx, dy) in [(0i64, 0i64), (1, 0), (0, 1), (1, 1)] {
                let (qx, qy) = (ix + dx, iy + dy);
                if qx < 0 || qy < 0 || qx >= size as i64 || qy >= size as i64 {
                    continue;
                }
                let qi = (qy as usize * size + qx as usize) * 3;
                rgb[qi] = stroke[0];
                rgb[qi + 1] = stroke[1];
                rgb[qi + 2] = stroke[2];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube_vcad(sx: f64, sy: f64, sz: f64) -> String {
        format!(
            r#"{{
  "version": "0.1",
  "nodes": {{
    "1": {{
      "id": 1,
      "name": "Cube",
      "op": {{ "type": "Cube", "size": {{ "x": {sx}, "y": {sy}, "z": {sz} }} }}
    }}
  }},
  "materials": {{
    "aluminum": {{
      "name": "aluminum",
      "color": [0.91, 0.92, 0.93],
      "metallic": 1.0,
      "roughness": 0.4,
      "density": 2700.0,
      "friction": 0.6
    }}
  }},
  "part_materials": {{}},
  "roots": [{{ "root": 1, "material": "aluminum" }}]
}}"#
        )
    }

    #[test]
    fn renders_cube_to_svg() {
        let svg = render_svg_str(&cube_vcad(20.0, 30.0, 10.0), DEFAULT_SCALE)
            .expect("cube should render");
        assert!(svg.starts_with("<svg "));
        assert!(svg.ends_with("</svg>"));
        assert!(svg.contains("<polygon"));
        assert!(svg.contains("<line"));
    }

    #[test]
    fn scale_changes_canvas_size() {
        let small = render_svg_str(&cube_vcad(20.0, 20.0, 20.0), 1.0).unwrap();
        let large = render_svg_str(&cube_vcad(20.0, 20.0, 20.0), 4.0).unwrap();
        let width = |svg: &str| -> f64 {
            let start = svg.find("width=\"").unwrap() + 7;
            let end = svg[start..].find('"').unwrap() + start;
            svg[start..end].parse().unwrap()
        };
        assert!(width(&large) > width(&small) * 2.0);
    }

    #[test]
    fn rejects_garbage_input() {
        assert!(render_svg_str("not json", DEFAULT_SCALE)
            .unwrap_err()
            .starts_with("parse:"));
    }

    #[test]
    fn rejects_empty_solid_list() {
        assert_eq!(
            render_svg_solids(&[], DEFAULT_SCALE).unwrap_err(),
            "no solids produced"
        );
    }

    #[test]
    fn parses_view_names() {
        use std::str::FromStr;
        assert_eq!(View::from_str("iso").unwrap(), View::Isometric);
        assert_eq!(View::from_str("hero").unwrap(), View::Isometric);
        assert_eq!(View::from_str("FRONT").unwrap(), View::Front);
        assert_eq!(View::from_str("side").unwrap(), View::Side);
        assert_eq!(View::from_str("top").unwrap(), View::Top);
        assert!(View::from_str("back").is_err());
    }

    #[test]
    fn ortho_views_render_svg() {
        for view in [View::Front, View::Side, View::Top] {
            let svg = render_svg_str_view(&cube_vcad(20.0, 30.0, 10.0), DEFAULT_SCALE, view)
                .expect("ortho view should render");
            assert!(svg.starts_with("<svg "));
        }
    }

    #[test]
    fn front_view_of_box_has_expected_aspect() {
        // 20 (X) × 30 (Y) × 10 (Z) box: front view (+Y) sees X × Z, i.e.
        // 20mm wide × 10mm tall.
        let svg = render_svg_str_view(&cube_vcad(20.0, 30.0, 10.0), 1.0, View::Front).unwrap();
        let attr = |name: &str| -> f64 {
            let pat = format!("{name}=\"");
            let start = svg.find(&pat).unwrap() + pat.len();
            let end = svg[start..].find('"').unwrap() + start;
            svg[start..end].parse().unwrap()
        };
        // Plus 2×8px padding on each axis.
        assert!(
            (attr("width") - 36.0).abs() < 0.5,
            "width: {}",
            attr("width")
        );
        assert!(
            (attr("height") - 26.0).abs() < 0.5,
            "height: {}",
            attr("height")
        );
    }

    #[cfg(feature = "raster")]
    mod raster_tests {
        use super::*;

        #[test]
        fn renders_cube_to_jpeg() {
            let opts = RasterOptions {
                size_px: 256,
                ..Default::default()
            };
            let jpg = render_jpeg_str(&cube_vcad(20.0, 30.0, 10.0), &opts).unwrap();
            assert!(jpg.len() > 1000);
            assert_eq!(&jpg[..2], &[0xFF, 0xD8], "missing JPEG SOI marker");
        }

        #[test]
        fn views_produce_distinct_images() {
            let doc = cube_vcad(20.0, 30.0, 10.0);
            let render = |view: View| {
                render_jpeg_str(
                    &doc,
                    &RasterOptions {
                        view,
                        size_px: 256,
                        ..Default::default()
                    },
                )
                .unwrap()
            };
            let front = render(View::Front);
            let top = render(View::Top);
            assert_ne!(front, top);
        }

        #[test]
        fn rejects_bad_options() {
            let doc = cube_vcad(10.0, 10.0, 10.0);
            assert!(render_jpeg_str(
                &doc,
                &RasterOptions {
                    size_px: 4,
                    ..Default::default()
                }
            )
            .is_err());
            assert!(render_jpeg_str(
                &doc,
                &RasterOptions {
                    fill_frac: 0.0,
                    ..Default::default()
                }
            )
            .is_err());
        }
    }
}
