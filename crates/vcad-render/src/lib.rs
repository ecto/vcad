//! `vcad-render` — project a `.vcad` to static line art.
//!
//! Drafting-style rendering: painter's-algorithm flat shading under a bold,
//! hidden-line-removed outline. The mesh is canonicalized (welded by
//! position), each shell oriented outward by signed volume (winding alone is
//! not reliably outward — cone laterals wind inward), then each triangle
//! edge is classified as one of:
//!
//!   - outline (boundary, or a silhouette between non-coplanar faces) → bold
//!   - smooth  (silhouette of a curved surface: a cylinder/sphere profile) → bold,
//!     but interior fragments (the stub off a bore's rim) are dropped
//!   - crease  (two same-facing, non-coplanar faces)                  → fine
//!   - internal (coplanar)                                            → hidden
//!
//! Fills are stroked with their own colour at a hairline width so adjacent
//! triangles overlap and the anti-alias seams that read as a fan across a
//! flat tessellated face disappear. Kept edges are then clipped against an
//! off-screen depth buffer so back faces, the far rims of bores, and
//! occluded creases don't bleed through. The result reads as a clean
//! drafting drawing, not a wireframe over fills.
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
use vcad_kernel::vcad_kernel_math::Transform;
use vcad_kernel::Solid;

// ─── tunable knobs ────────────────────────────────────────────────────────

/// Default pixels-per-millimetre when no `--scale` is given.
pub const DEFAULT_SCALE: f64 = 2.0;
const PADDING_PX: f64 = 8.0;

/// Interior crease stroke width (user units). Light, so tessellation and
/// fillet rims read as fine linework rather than heavy outlines.
const STROKE_CREASE_PX: f64 = 0.5;
/// Silhouette / boundary stroke width (user units). Heavier than creases —
/// the classic drafting convention of a bold outline over light interior
/// lines, which makes the part pop and reads as deliberate, not noisy.
const STROKE_OUTLINE_PX: f64 = 1.2;
/// Hidden (occluded) edge stroke width — drawn dashed, the engineering
/// convention that lets a single isometric reveal bores and pockets behind
/// the front faces.
const STROKE_HIDDEN_PX: f64 = 0.45;

/// Curved primitives in the SVG path get a fine tessellation: at 64
/// segments a cylinder facet spans ~5.6°, well under the crease threshold,
/// so bores and fillets read as smooth surfaces instead of faceted barrels.
const TESSELLATION_SEGMENTS: u32 = 64;

/// Two triangles are considered coplanar when their unit normals' dot
/// product exceeds this threshold. Tighter values reveal more creases;
/// looser values hide more internal edges. The SVG path uses
/// [`COPLANAR_DOT_TOL_SVG`]; this tighter value is the historical knob kept
/// for reference.
#[allow(dead_code)]
const COPLANAR_DOT_TOL: f64 = 0.997; // cos(~4.5°)

/// Coplanar tolerance for the SVG path. At 64 segments adjacent curved
/// facets differ by ~5.6°, so a ~10° threshold blends tessellation while
/// still revealing real creases (chamfers, fillet rims, sharp edges).
const COPLANAR_DOT_TOL_SVG: f64 = 0.985; // cos(~10°)

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

/// Visible linework ink — a touch warmer and darker than the fill navy so
/// lines read as ink laid on the wash, not the same hue as the fill.
const INK: &str = "#0b2742";
/// Warm off-white "vellum" the drafting plate sits on (matches the raster
/// path's matte background). The signature ground behind every render.
const PAPER: &str = "#f4f3f1";

/// The vcad-Blue tonal ramp: deep-shadow → core navy → mid → ice highlight.
/// Sampled by the shading term so curved surfaces read as a graded wash
/// instead of a two-colour lerp. The house colour system — one ramp, many
/// renders.
const RAMP: [[u8; 3]; 4] = [
    [13, 44, 74],    // deep shadow
    [30, 86, 130],   // core navy
    [123, 160, 192], // mid
    [205, 224, 238], // ice highlight
];

/// Long-side resolution of the off-screen depth buffer used for vector
/// hidden-line removal in the SVG path. Decoupled from `scale` and the
/// final raster size: a fixed budget keeps occlusion decisions crisp for
/// tiny parts and bounded for huge ones. The final SVG is resolution-
/// independent regardless; this only governs which edge spans survive.
const OCC_BUFFER_LONG_SIDE: f64 = 1100.0;

/// How much screen-space ambient occlusion darkens a fully-occluded corner
/// (0 = off). Kept gentle — AO is a depth cue, not a stain. Concave
/// creases, bore/pocket rims, and part-contact lines pick up a soft
/// shadow; convex edges and open faces stay bright.
const AO_STRENGTH: f64 = 0.7;
/// AO below this fraction is treated as zero — it's the weak, noisy
/// occlusion that coarse flat-face tessellation produces, which would
/// otherwise band. Real cavities and contacts read well above it.
const AO_FLOOR: f64 = 0.24;

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

/// A solid plus its material colour (linear RGB in `[0,1]`), if the
/// document assigned one — used to tint the shading ramp.
type TintedSolid = (Solid, Option<[f64; 3]>);

fn evaluate_vcad(raw_vcad: &str) -> Result<Vec<TintedSolid>, String> {
    let parsed = parse_vcad_file(raw_vcad).map_err(|e| format!("parse: {}", e))?;
    // NOTE: catch_unwind only works on native targets. On
    // wasm32-unknown-unknown a panic compiles to an `unreachable` trap —
    // it never unwinds, this guard never fires, and the WASM instance is
    // left in an undefined state. The JS caller is responsible for
    // catching the trap (WebAssembly.RuntimeError) and poisoning the
    // shared instance; see packages/mcp/src/tools/render.ts.
    // A static renderer never displays clash meshes, and clash detection
    // is O(n²) pairwise booleans across scene roots — fatal for many-root
    // documents (an imported chip die has ~90k roots).
    let scene = catch_unwind(AssertUnwindSafe(|| {
        evaluate_document(
            &parsed.document,
            &EvalOptions {
                skip_clash_detection: true,
                ..Default::default()
            },
        )
    }))
    .map_err(|_| "eval panicked".to_string())?
    .map_err(|e| format!("eval: {}", e))?;

    let materials = &parsed.document.materials;
    let mut solids: Vec<TintedSolid> = scene
        .parts
        .iter()
        .filter_map(|p| {
            p.solid.clone().map(|s| {
                let color = materials.get(&p.material).map(|m| m.color);
                (s, color)
            })
        })
        .collect();

    // Assembly instances: `evaluate_document` only carries meshes for
    // instances, not BRep solids, so re-evaluate each referenced part
    // definition once and place a transformed copy per instance. Without
    // this, an assembly-only document (no scene roots) rendered as
    // "no solids produced" despite being perfectly valid.
    solids.extend(evaluate_assembly_instances(&parsed.document)?);
    Ok(solids)
}

/// Evaluate the document's assembly instances (if any) into world-placed
/// tinted solids. Part-definition solids are evaluated once and shared;
/// per-instance world poses come from forward kinematics, falling back to
/// the instance's static transform.
fn evaluate_assembly_instances(doc: &vcad_ir::Document) -> Result<Vec<TintedSolid>, String> {
    let (Some(part_defs), Some(instances)) = (&doc.part_defs, &doc.instances) else {
        return Ok(Vec::new());
    };
    if part_defs.is_empty() || instances.is_empty() {
        return Ok(Vec::new());
    }

    let world = catch_unwind(AssertUnwindSafe(|| {
        vcad_eval::solve_forward_kinematics(doc)
    }))
    .map_err(|_| "fk panicked".to_string())?;

    let mut cache: HashMap<vcad_ir::NodeId, Option<Solid>> = HashMap::new();
    let mut def_solids: HashMap<&str, Option<Solid>> = HashMap::new();
    let mut out = Vec::new();

    for inst in instances {
        let Some(def) = part_defs.get(&inst.part_def_id) else {
            continue;
        };
        let solid = def_solids
            .entry(inst.part_def_id.as_str())
            .or_insert_with(|| {
                catch_unwind(AssertUnwindSafe(|| {
                    vcad_eval::evaluate_node(def.root, &doc.nodes, &mut cache)
                        .ok()
                        .flatten()
                }))
                .unwrap_or(None)
            })
            .clone();
        let Some(solid) = solid else { continue };

        let placed = match world.get(&inst.id).cloned().or(inst.transform) {
            Some(t) => solid.apply_transform(&transform3d_to_kernel(&t)),
            None => solid,
        };

        let material = inst
            .material
            .clone()
            .or_else(|| def.default_material.clone())
            .unwrap_or_else(|| "default".to_string());
        let color = doc.materials.get(&material).map(|m| m.color);
        out.push((placed, color));
    }
    Ok(out)
}

/// IR `Transform3D` → kernel `Transform`, matching the evaluator's
/// convention: scale, then Rx·Ry·Rz (applied x-first), then translation.
fn transform3d_to_kernel(t: &vcad_ir::Transform3D) -> Transform {
    Transform::scale(t.scale.x, t.scale.y, t.scale.z)
        .then(&Transform::rotation_x(t.rotation.x.to_radians()))
        .then(&Transform::rotation_y(t.rotation.y.to_radians()))
        .then(&Transform::rotation_z(t.rotation.z.to_radians()))
        .then(&Transform::translation(
            t.translation.x,
            t.translation.y,
            t.translation.z,
        ))
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
    for chunk in mesh.indices.as_chunks::<3>().0 {
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

/// Orient per-triangle normals consistently outward, per connected shell.
///
/// Tessellation winding is not reliably outward across the kernel's
/// primitives and booleans — cone laterals wind inward (so the raw winding
/// normal points into the solid), while some boolean caps wind outward but
/// carry inward *vertex* normals. Neither the winding nor the stored
/// normals can be trusted alone. Geometry can: a closed shell whose
/// triangle winding encloses negative signed volume is inside-out, so we
/// flip that shell's normals. Shells are the connected components of the
/// vertex-adjacency graph, so disjoint solids in one mesh are oriented
/// independently.
fn orient_normals(cm: &CanonMesh, normals: &mut [[f64; 3]]) {
    let n = cm.verts.len();
    if n == 0 {
        return;
    }
    // Union-find over canonical vertices → connected shells.
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(parent: &mut [usize], x: usize) -> usize {
        let mut root = x;
        while parent[root] != root {
            root = parent[root];
        }
        let mut cur = x;
        while parent[cur] != cur {
            let next = parent[cur];
            parent[cur] = root;
            cur = next;
        }
        root
    }
    for t in &cm.tris {
        let a = find(&mut parent, t[0]);
        let b = find(&mut parent, t[1]);
        let c = find(&mut parent, t[2]);
        parent[b] = a;
        parent[c] = a;
    }
    // Signed volume per shell (6× actual; only the sign matters).
    let mut vol6: HashMap<usize, f64> = HashMap::new();
    for t in &cm.tris {
        let root = find(&mut parent, t[0]);
        let (v0, v1, v2) = (cm.verts[t[0]], cm.verts[t[1]], cm.verts[t[2]]);
        *vol6.entry(root).or_default() += dot(v0, cross(v1, v2));
    }
    for (ti, t) in cm.tris.iter().enumerate() {
        let root = find(&mut parent, t[0]);
        if vol6.get(&root).copied().unwrap_or(0.0) < 0.0 {
            normals[ti] = [-normals[ti][0], -normals[ti][1], -normals[ti][2]];
        }
    }
}

// ─── per-solid render artifacts (shared by SVG + raster paths) ────────────

/// How a kept edge reads, and how hidden-line removal treats it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EdgeKind {
    /// A hard model outline: a boundary (open) edge, or a silhouette
    /// between two clearly non-coplanar faces (a box edge, a bore rim at
    /// its tangent). Stroked heavy; always kept where visible.
    Outline,
    /// The silhouette of a smoothly curved surface — a cylinder/sphere
    /// profile, or the generatrix of a bore. Stroked heavy, but kept only
    /// where it runs over open background (a true exterior outline) or as a
    /// long interior run; short interior fragments are tangent-point noise
    /// (the stub hanging off a hole's rim) and are dropped.
    Smooth,
    /// Interior crease between two same-facing, non-coplanar faces (a
    /// chamfer rim, a sharp fold). Stroked light.
    Crease,
}

struct SolidArtifacts {
    verts: Vec<[f64; 3]>,
    tris: Vec<[usize; 3]>,
    normals: Vec<[f64; 3]>,
    /// Per-triangle, per-corner smoothed normals (angle-grouped: averaged
    /// only across facets within the smoothing tolerance, so curved
    /// surfaces interpolate smoothly while hard edges stay crisp). Drives
    /// Gouraud shading in the SVG path.
    corner_normals: Vec<[[f64; 3]; 3]>,
    /// The shading ramp for this solid — the base navy ramp, tinted toward
    /// the part's material colour when one is known.
    ramp: [[u8; 3]; 4],
    visible: Vec<bool>,
    /// Kept edges (boundary, crease, or silhouette, with ≥1 visible
    /// adjacent triangle) as canonical-vertex index pairs plus their kind.
    edges: Vec<(usize, usize, EdgeKind)>,
}

/// Per-triangle, per-corner smoothed normals. A corner's normal is the
/// average of the face normals of all triangles incident to that vertex
/// whose normal lies within `tol` of *this* triangle's normal — i.e. the
/// same smooth surface. This rounds curved tessellation (cylinders,
/// fillets, spheres) while leaving hard edges (a cube corner, a bore rim)
/// crisp, because across a hard edge the neighbour's normal falls outside
/// `tol` and is excluded.
fn smoothed_corner_normals(
    verts: &[[f64; 3]],
    tris: &[[usize; 3]],
    normals: &[[f64; 3]],
    tol: f64,
) -> Vec<[[f64; 3]; 3]> {
    // Vertex → incident triangles.
    let mut vtris: Vec<Vec<usize>> = vec![Vec::new(); verts.len()];
    for (ti, t) in tris.iter().enumerate() {
        for &v in t {
            vtris[v].push(ti);
        }
    }
    tris.iter()
        .enumerate()
        .map(|(ti, t)| {
            let fn_ti = normals[ti];
            let mut corners = [[0.0; 3]; 3];
            for (c, &v) in t.iter().enumerate() {
                let mut acc = [0.0; 3];
                for &tj in &vtris[v] {
                    if dot(fn_ti, normals[tj]) >= tol {
                        acc = [
                            acc[0] + normals[tj][0],
                            acc[1] + normals[tj][1],
                            acc[2] + normals[tj][2],
                        ];
                    }
                }
                let n = normalize(acc);
                // Degenerate fallback: keep the flat face normal.
                corners[c] = if n == [0.0; 3] { fn_ti } else { n };
            }
            corners
        })
        .collect()
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
    /// Keep hard edges (boundary/crease/silhouette) even when *both*
    /// adjacent faces point away from the camera. The SVG path sets this so
    /// fully-occluded model edges — the far side of a box, the near arc of a
    /// through-bore's hidden rim — survive to be drawn as dashed hidden
    /// lines; their occlusion is resolved by the depth buffer afterward.
    /// (Coplanar tessellation facets are still dropped regardless.)
    keep_occluded: bool,
}

impl EdgeRules {
    /// The SVG path: silhouette-aware (vector hidden-line removal needs
    /// explicit outlines) with the curved-facet-blending coplanar tol, and
    /// keeps occluded hard edges so they can be dashed.
    fn svg() -> Self {
        EdgeRules {
            coplanar_dot_tol: COPLANAR_DOT_TOL_SVG,
            mark_silhouette: true,
            keep_occluded: true,
        }
    }
}

fn build_artifacts(
    solids: &[Solid],
    tints: &[Option<[f64; 3]>],
    cam: [f64; 3],
    segments: u32,
    rules: &EdgeRules,
) -> Vec<SolidArtifacts> {
    let mut out = Vec::new();
    for (si, solid) in solids.iter().enumerate() {
        let ramp = tints
            .get(si)
            .copied()
            .flatten()
            .map(tint_ramp)
            .unwrap_or(RAMP);
        let mesh = catch_unwind(AssertUnwindSafe(|| solid.to_mesh(segments)));
        let Ok(mesh) = mesh else { continue };
        if mesh.indices.is_empty() {
            continue;
        }
        let cm = canonicalize(&mesh);
        if cm.tris.is_empty() {
            continue;
        }

        // Per-triangle winding normal, then oriented outward per shell so
        // back-face culling and silhouette tests use a consistent "out".
        let mut normals: Vec<[f64; 3]> = cm
            .tris
            .iter()
            .map(|t| face_normal([cm.verts[t[0]], cm.verts[t[1]], cm.verts[t[2]]]))
            .collect();
        orient_normals(&cm, &mut normals);
        let corner_normals =
            smoothed_corner_normals(&cm.verts, &cm.tris, &normals, rules.coplanar_dot_tol);
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
        let mut edges: Vec<(usize, usize, EdgeKind)> = Vec::new();
        for (edge, adj_tris) in &edge_to_tris {
            // Unless the caller wants occluded edges (for dashed hidden
            // lines), drop edges with no front-facing adjacent triangle —
            // without a depth buffer they'd be drawn over the silhouette.
            // With one (the SVG path), keep them and let HLR dash them.
            if !rules.keep_occluded && !adj_tris.iter().any(|&ti| visible[ti]) {
                continue;
            }
            let kind = match adj_tris.len() {
                1 => Some(EdgeKind::Outline), // boundary
                2 => {
                    let (t0, t1) = (adj_tris[0], adj_tris[1]);
                    let silhouette = rules.mark_silhouette
                        && (dot(normals[t0], cam) >= 0.0) != (dot(normals[t1], cam) >= 0.0);
                    let cosang = dot(normals[t0], normals[t1]).abs();
                    let coplanar = cosang >= rules.coplanar_dot_tol;
                    if silhouette {
                        // A silhouette between near-coplanar facets is the
                        // smooth outline of a curved surface; between
                        // distinct faces it's a hard model edge.
                        Some(if coplanar {
                            EdgeKind::Smooth
                        } else {
                            EdgeKind::Outline
                        })
                    } else if !coplanar {
                        Some(EdgeKind::Crease)
                    } else {
                        None // coplanar internal edge — hidden
                    }
                }
                _ => Some(EdgeKind::Outline), // weird (non-manifold); render conservatively
            };
            if let Some(kind) = kind {
                edges.push((edge.0, edge.1, kind));
            }
        }

        out.push(SolidArtifacts {
            verts: cm.verts,
            tris: cm.tris,
            normals,
            corner_normals,
            ramp,
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

/// Studio shading term in `[0, 1]` for the SVG path: an upper-left key
/// light over a soft ambient floor (no facet crushes to pure shadow) plus
/// a grazing-angle "rim" lift that puts a bright lip on curved silhouettes
/// — the cue that reads as a lit, three-dimensional surface rather than a
/// flat fill.
fn shade(n: [f64; 3], cam: [f64; 3], light: [f64; 3]) -> f64 {
    let key = (dot(n, light) * 0.5 + 0.5).powf(0.85);
    // Rim: strongest where the facet grazes the camera and is at least
    // partially lit, so the lift lands on the lit side of a silhouette.
    let graze = (1.0 - dot(n, cam).abs()).clamp(0.0, 1.0);
    let rim = graze.powi(3) * (dot(n, light) * 0.5 + 0.5);
    (key * 0.86 + 0.1 + rim * 0.26).clamp(0.0, 1.0)
}

/// Sample a 4-stop tonal `ramp` at `t ∈ [0, 1]` with linear interpolation.
fn ramp_sample(ramp: &[[u8; 3]; 4], t: f64) -> [u8; 3] {
    let t = t.clamp(0.0, 1.0) * 3.0;
    let i = (t.floor() as usize).min(2);
    mix_rgb(ramp[i], ramp[i + 1], t - i as f64)
}

fn luma(c: [f64; 3]) -> f64 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// Tint the base [`RAMP`] toward a material colour while staying in the navy
/// tonal family. Every stop is blended toward the material colour
/// *rescaled to that stop's own luminance*, so the hue shift is coherent
/// from shadow to highlight (no two-tone navy-body / warm-cap split) and
/// the lighting tone is preserved. The blend strength scales with the
/// material's saturation, so achromatic materials (aluminium, steel, the
/// default) leave the navy essentially untouched while chromatic ones
/// (copper, brass, gold, wood) warm or cool the whole ramp — one
/// disciplined tint per part, never an arbitrary rainbow.
fn tint_ramp(color: [f64; 3]) -> [[u8; 3]; 4] {
    let mx = color[0].max(color[1]).max(color[2]).max(1e-3);
    let mn = color[0].min(color[1]).min(color[2]);
    let sat = (mx - mn) / mx; // HSV saturation
                              // The blend strength is the material's saturation directly, capped
                              // just under 1 so a thin navy shading remains for lighting tone.
                              // Achromatic materials (steel, aluminium, default) → k≈0 → navy
                              // ramp untouched. Chromatic materials (brass, copper, gold, pure
                              // red) → k near the cap → the material's hue actually drives the
                              // rendered colour. The previous `(sat * 0.3).clamp(0, 0.2)` capped
                              // every preset at ≤20% material contribution, so even fully
                              // saturated red rendered as navy with the faintest pink hint.
    let k = sat.clamp(0.0, 0.85);
    let ml = luma(color).max(1e-3);
    let mut out = RAMP;
    for stop in out.iter_mut() {
        let sl = luma([stop[0] as f64, stop[1] as f64, stop[2] as f64]) / 255.0;
        let scale = sl / ml; // material rescaled to this stop's luminance
        for c in 0..3 {
            let base = stop[c] as f64;
            let mat_at = (color[c] * scale * 255.0).clamp(0.0, 255.0);
            stop[c] = (base * (1.0 - k) + mat_at * k).round() as u8;
        }
    }
    out
}

fn hex(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

fn mix_rgb(a: [u8; 3], b: [u8; 3], t: f64) -> [u8; 3] {
    let mix = |x: u8, y: u8| ((x as f64) * (1.0 - t) + (y as f64) * t).round() as u8;
    [mix(a[0], b[0]), mix(a[1], b[1]), mix(a[2], b[2])]
}

// ─── output buffers ───────────────────────────────────────────────────────

/// A 2D line segment in final SVG coordinates.
type Seg = ((f64, f64), (f64, f64));

/// A projected, shaded triangle ready to emit. Screen coords are in final
/// SVG user units (padding already applied); `depth` is the centroid depth
/// for painter sorting. `fill` is a solid colour or a `url(#gN)` gradient
/// ref; `stroke` is a solid colour that closes anti-alias seams without
/// fighting the gradient.
#[derive(Clone)]
struct ProjPoly {
    s: [(f64, f64); 3],
    fill: String,
    stroke: String,
    depth: f64,
}

/// An SVG `linearGradient` reproducing a triangle's Gouraud shade: the
/// vector `(x1,y1)→(x2,y2)` and the colours at its two ends.
struct GouraudGrad {
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    lo: [u8; 3],
    hi: [u8; 3],
}

/// Build a per-triangle linear gradient that reproduces Gouraud (barycentric)
/// shade interpolation. Shade is affine over the triangle, so its screen-space
/// gradient is a single direction; we place an SVG `linearGradient` along it.
/// Returns `None` when the shade range is negligible or the triangle is
/// degenerate (the caller then flat-fills).
fn gouraud_gradient(ps: [(f64, f64); 3], sh: [f64; 3], ramp: &[[u8; 3]; 4]) -> Option<GouraudGrad> {
    let smin = sh[0].min(sh[1]).min(sh[2]);
    let smax = sh[0].max(sh[1]).max(sh[2]);
    if smax - smin < 0.012 {
        return None;
    }
    let (e1x, e1y) = (ps[1].0 - ps[0].0, ps[1].1 - ps[0].1);
    let (e2x, e2y) = (ps[2].0 - ps[0].0, ps[2].1 - ps[0].1);
    let det = e1x * e2y - e1y * e2x;
    if det.abs() < 1e-9 {
        return None;
    }
    let (d1, d2) = (sh[1] - sh[0], sh[2] - sh[0]);
    // ∇shade in screen space.
    let bx = (e2y * d1 - e1y * d2) / det;
    let by = (-e2x * d1 + e1x * d2) / det;
    let blen = (bx * bx + by * by).sqrt();
    if blen < 1e-9 {
        return None;
    }
    let (ux, uy) = (bx / blen, by / blen);
    // Project the corners onto the gradient direction; shade is monotonic
    // along it, so min/max projection ↔ min/max shade.
    let proj: [f64; 3] = [
        ps[0].0 * ux + ps[0].1 * uy,
        ps[1].0 * ux + ps[1].1 * uy,
        ps[2].0 * ux + ps[2].1 * uy,
    ];
    let mut lo = 0;
    let mut hi = 0;
    for i in 1..3 {
        if proj[i] < proj[lo] {
            lo = i;
        }
        if proj[i] > proj[hi] {
            hi = i;
        }
    }
    let span = proj[hi] - proj[lo];
    let (ax, ay) = ps[lo];
    Some(GouraudGrad {
        x1: ax,
        y1: ay,
        x2: ax + ux * span,
        y2: ay + uy * span,
        lo: ramp_sample(ramp, sh[lo]),
        hi: ramp_sample(ramp, sh[hi]),
    })
}

/// A projected edge in final SVG coords, with per-endpoint depth for the
/// hidden-line-removal walk and a kind for stroke weighting.
#[derive(Clone)]
struct ProjEdge {
    a: (f64, f64),
    da: f64,
    b: (f64, f64),
    db: f64,
    kind: EdgeKind,
}

// ─── off-screen depth buffer (vector hidden-line removal) ──────────────────

/// Fixed-budget depth buffer the SVG path rasterizes its triangles into so
/// edges can be occlusion-tested before they're emitted as vector lines.
struct DepthBuffer {
    z: Vec<f64>,
    bw: usize,
    bh: usize,
    /// Buffer cells per SVG user unit.
    scale: f64,
}

impl DepthBuffer {
    fn new(w: f64, h: f64) -> Self {
        let s = (OCC_BUFFER_LONG_SIDE / w.max(h).max(1.0)).max(1.0);
        let bw = ((w * s).ceil() as usize).clamp(1, 4096);
        let bh = ((h * s).ceil() as usize).clamp(1, 4096);
        DepthBuffer {
            z: vec![f64::NEG_INFINITY; bw * bh],
            bw,
            bh,
            scale: s,
        }
    }

    #[inline]
    fn at(&self, x: i64, y: i64) -> f64 {
        if x < 0 || y < 0 || x >= self.bw as i64 || y >= self.bh as i64 {
            f64::NEG_INFINITY
        } else {
            self.z[y as usize * self.bw + x as usize]
        }
    }

    /// Rasterize a triangle (final SVG coords + per-vertex depth) into the
    /// buffer, keeping the nearest (largest) depth per cell.
    fn raster(&mut self, s: [(f64, f64); 3], vd: [f64; 3]) {
        let p = [
            (s[0].0 * self.scale, s[0].1 * self.scale, vd[0]),
            (s[1].0 * self.scale, s[1].1 * self.scale, vd[1]),
            (s[2].0 * self.scale, s[2].1 * self.scale, vd[2]),
        ];
        let area = edge_area(p[0], p[1], p[2].0, p[2].1);
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
        let x1 = (xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max).ceil())
            .min((self.bw - 1) as f64) as usize;
        let y1 = (ys.iter().cloned().fold(f64::NEG_INFINITY, f64::max).ceil())
            .min((self.bh - 1) as f64) as usize;
        if x0 > x1 || y0 > y1 {
            return;
        }
        let sign = area.signum();
        let inv = 1.0 / area.abs();
        for py in y0..=y1 {
            for px in x0..=x1 {
                let cx = px as f64 + 0.5;
                let cy = py as f64 + 0.5;
                let w0 = edge_area(p[1], p[2], cx, cy) * sign;
                let w1 = edge_area(p[2], p[0], cx, cy) * sign;
                let w2 = edge_area(p[0], p[1], cx, cy) * sign;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let depth = (w0 * p[0].2 + w1 * p[1].2 + w2 * p[2].2) * inv;
                let idx = py * self.bw + px;
                if depth > self.z[idx] {
                    self.z[idx] = depth;
                }
            }
        }
    }

    /// Screen-space ambient occlusion at a final-SVG point whose surface is
    /// at camera depth `depth`. Samples rings of neighbours and counts those
    /// significantly *nearer* than this point (geometry poking up in front,
    /// i.e. blocking ambient light), with a distance falloff so only local
    /// cavities contribute. Returns occlusion in `[0, 1]` (0 = open).
    ///
    /// `bias` ignores tiny depth differences (a surface's own screen slope);
    /// `range` is the falloff distance — occluders nearer by more than this
    /// don't count, so a foreground part doesn't shadow the whole background.
    fn ao(&self, sx: f64, sy: f64, depth: f64, bias: f64, range: f64) -> f64 {
        let bx = sx * self.scale;
        let by = sy * self.scale;
        // Two rings scaled to the buffer so the radius is resolution-stable.
        let r0 = (self.bw.max(self.bh) as f64 * 0.006).clamp(2.0, 9.0);
        let radii = [r0, r0 * 2.2, r0 * 4.0];
        const ANG: usize = 8;
        let mut occ = 0.0;
        let mut total = 0.0;
        for &r in &radii {
            for a in 0..ANG {
                let th = a as f64 / ANG as f64 * std::f64::consts::TAU;
                let px = (bx + r * th.cos()).floor() as i64;
                let py = (by + r * th.sin()).floor() as i64;
                total += 1.0;
                let z = self.at(px, py);
                if z == f64::NEG_INFINITY {
                    continue; // open sky — not an occluder
                }
                let diff = z - depth; // > 0 ⇒ neighbour is nearer (occludes)
                if diff > bias {
                    occ += (1.0 - diff / range).clamp(0.0, 1.0);
                }
            }
        }
        if total > 0.0 {
            occ / total
        } else {
            0.0
        }
    }

    /// Walk an edge (final SVG coords + endpoint depths) and split it into
    /// its visible and hidden (occluded) sub-segments. Each [`VisSpan`]
    /// carries its length in buffer cells and whether any of it ran over
    /// open background, so the caller can apply per-kind keep rules and emit
    /// the hidden parts as dashed convention lines. `bias` is an absolute
    /// depth tolerance; the local depth gradient is added adaptively so
    /// grazing/silhouette samples aren't self-occluded.
    fn clip_edge(&self, a: (f64, f64), da: f64, b: (f64, f64), db: f64, bias: f64) -> EdgeClip {
        let (ax, ay) = (a.0 * self.scale, a.1 * self.scale);
        let (bx, by) = (b.0 * self.scale, b.1 * self.scale);
        let len = ((bx - ax).powi(2) + (by - ay).powi(2)).sqrt();
        let steps = len.ceil().max(1.0) as usize;
        let scale = self.scale;
        let mut clip = EdgeClip {
            visible: Vec::new(),
            hidden: Vec::new(),
        };
        // Current run state: visibility, start point, last point, bg flag.
        let mut state: Option<bool> = None;
        let mut run_start = (ax, ay);
        let mut run_last = (ax, ay);
        let mut run_over_bg = false;
        let flush =
            |vis: bool, start: (f64, f64), end: (f64, f64), over_bg: bool, c: &mut EdgeClip| {
                let len_cells = ((end.0 - start.0).powi(2) + (end.1 - start.1).powi(2)).sqrt();
                let span = VisSpan {
                    a: (start.0 / scale, start.1 / scale),
                    b: (end.0 / scale, end.1 / scale),
                    len_cells,
                    over_bg,
                };
                if vis {
                    c.visible.push(span);
                } else {
                    c.hidden.push(span);
                }
            };
        for i in 0..=steps {
            let t = i as f64 / steps as f64;
            let bxp = ax + (bx - ax) * t;
            let byp = ay + (by - ay) * t;
            let d = da + (db - da) * t;
            let ix = bxp.floor() as i64;
            let iy = byp.floor() as i64;
            let z = self.at(ix, iy);
            let mut over_bg = false;
            let visible = if z == f64::NEG_INFINITY {
                over_bg = true; // over background — nothing occludes it
                true
            } else {
                let mut delta = 0.0f64;
                for (nx, ny) in [(ix - 1, iy), (ix + 1, iy), (ix, iy - 1), (ix, iy + 1)] {
                    let nz = self.at(nx, ny);
                    if nz == f64::NEG_INFINITY {
                        delta = f64::INFINITY; // silhouette pixel — always show
                        over_bg = true;
                    } else {
                        delta = delta.max((z - nz).abs());
                    }
                }
                d >= z - (bias + delta)
            };
            let pt = (bxp, byp);
            match state {
                Some(prev) if prev == visible => {
                    run_last = pt;
                    run_over_bg |= over_bg;
                }
                Some(prev) => {
                    // Visibility flipped — close the previous run at this
                    // boundary point so visible and hidden spans abut.
                    flush(prev, run_start, pt, run_over_bg, &mut clip);
                    run_start = pt;
                    run_last = pt;
                    run_over_bg = over_bg;
                    state = Some(visible);
                }
                None => {
                    run_start = pt;
                    run_last = pt;
                    run_over_bg = over_bg;
                    state = Some(visible);
                }
            }
        }
        if let Some(vis) = state {
            flush(vis, run_start, run_last, run_over_bg, &mut clip);
        }
        clip
    }
}

/// The visible and hidden sub-segments of one edge after the depth-buffer walk.
struct EdgeClip {
    visible: Vec<VisSpan>,
    hidden: Vec<VisSpan>,
}

/// One sub-segment of an edge after hidden-line removal.
struct VisSpan {
    a: (f64, f64),
    b: (f64, f64),
    /// Length in depth-buffer cells (resolution-independent measure).
    len_cells: f64,
    /// True if any sample ran over / beside open background.
    over_bg: bool,
}

/// Twice the signed area of triangle (a, b, (px, py)) — the half-plane
/// edge function shared by the depth rasterizer.
#[inline]
fn edge_area(a: (f64, f64, f64), b: (f64, f64, f64), px: f64, py: f64) -> f64 {
    (b.0 - a.0) * (py - a.1) - (b.1 - a.1) * (px - a.0)
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
///
/// Unlike [`render_svg_solids_view`], this path knows each part's material
/// and tints the shading ramp accordingly.
pub fn render_svg_str_view(raw_vcad: &str, scale: f64, view: View) -> Result<String, String> {
    render_svg_str_view_opts(raw_vcad, scale, view, false)
}

/// Render raw `.vcad` document JSON to a self-contained SVG from `view`,
/// optionally omitting the opaque paper background for a transparent render.
///
/// When `transparent` is true the vellum ground rect is skipped, so the
/// SVG composites cleanly over any background; the soft contact shadow is
/// still emitted (it is already semi-transparent).
pub fn render_svg_str_view_opts(
    raw_vcad: &str,
    scale: f64,
    view: View,
    transparent: bool,
) -> Result<String, String> {
    let tinted = evaluate_vcad(raw_vcad)?;
    let solids: Vec<Solid> = tinted.iter().map(|(s, _)| s.clone()).collect();
    let tints: Vec<Option<[f64; 3]>> = tinted.iter().map(|(_, t)| *t).collect();
    render_svg_impl(&solids, &tints, scale, view, transparent)
}

/// Render pre-evaluated solids to a self-contained isometric SVG.
///
/// `scale` is pixels per millimetre. Returns an error when no solids are
/// given or nothing survives tessellation + back-face culling.
pub fn render_svg_solids(solids: &[Solid], scale: f64) -> Result<String, String> {
    render_svg_solids_view(solids, scale, View::Isometric)
}

/// Render pre-evaluated solids to a self-contained SVG from `view`.
///
/// Pipeline: tessellate → back-face cull → project → shade (painter's
/// algorithm, back-to-front) → rasterize a depth buffer → emit only the
/// edge sub-segments that survive a z-test against it. The depth buffer
/// gives true hidden-line removal in vector form, so back faces, the far
/// rims of bores, and occluded creases no longer bleed through the fills.
///
/// This entry point has no material information, so every part uses the
/// base navy ramp; use [`render_svg_str_view`] to honour document materials.
pub fn render_svg_solids_view(solids: &[Solid], scale: f64, view: View) -> Result<String, String> {
    let tints = vec![None; solids.len()];
    render_svg_impl(solids, &tints, scale, view, false)
}

/// Shared SVG renderer; `tints[i]` optionally tints solid `i`'s ramp.
/// When `transparent`, the opaque paper background rect is omitted.
fn render_svg_impl(
    solids: &[Solid],
    tints: &[Option<[f64; 3]>],
    scale: f64,
    view: View,
    transparent: bool,
) -> Result<String, String> {
    if solids.is_empty() {
        return Err("no solids produced".to_string());
    }

    let cam = view.cam();
    let right = view.right();
    let down = view.down();
    let light = normalize(LIGHT);
    let project = |p: [f64; 3]| -> (f64, f64) { (dot(p, right) * scale, dot(p, down) * scale) };

    let arts = build_artifacts(solids, tints, cam, TESSELLATION_SEGMENTS, &EdgeRules::svg());

    // First pass: screen-space bbox + 3D bbox (for the depth-cue bias),
    // over every projected vertex of every kept artifact.
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    let mut lo3 = [f64::INFINITY; 3];
    let mut hi3 = [f64::NEG_INFINITY; 3];
    let projected: Vec<Vec<(f64, f64)>> = arts
        .iter()
        .map(|art| {
            art.verts
                .iter()
                .map(|v| {
                    let (x, y) = project(*v);
                    min_x = min_x.min(x);
                    min_y = min_y.min(y);
                    max_x = max_x.max(x);
                    max_y = max_y.max(y);
                    for i in 0..3 {
                        lo3[i] = lo3[i].min(v[i]);
                        hi3[i] = hi3[i].max(v[i]);
                    }
                    (x, y)
                })
                .collect()
        })
        .collect();

    if !min_x.is_finite() || !max_x.is_finite() {
        return Err("no visible polygons after culling".to_string());
    }
    let w = (max_x - min_x) + 2.0 * PADDING_PX;
    let h = (max_y - min_y) + 2.0 * PADDING_PX;
    // Final SVG coordinate from a projected screen point.
    let fin =
        |(x, y): (f64, f64)| -> (f64, f64) { (x - min_x + PADDING_PX, y - min_y + PADDING_PX) };

    // Depth buffer, built up front from every visible triangle. It serves
    // two passes: ambient-occlusion sampling (below) and hidden-line removal
    // (further down). Building it once, before shading, lets AO darken the
    // corner shades that then drive the Gouraud gradients.
    let diag =
        ((hi3[0] - lo3[0]).powi(2) + (hi3[1] - lo3[1]).powi(2) + (hi3[2] - lo3[2]).powi(2)).sqrt();
    let bias = 0.5 + 0.01 * diag;
    let mut zbuf = DepthBuffer::new(w, h);
    for (ai, art) in arts.iter().enumerate() {
        let proj = &projected[ai];
        for (ti, t) in art.tris.iter().enumerate() {
            if !art.visible[ti] {
                continue;
            }
            zbuf.raster(
                [fin(proj[t[0]]), fin(proj[t[1]]), fin(proj[t[2]])],
                [
                    dot(art.verts[t[0]], cam),
                    dot(art.verts[t[1]], cam),
                    dot(art.verts[t[2]], cam),
                ],
            );
        }
    }
    // AO ignores a surface's own screen slope (larger than the HLR bias) and
    // falls off over a fraction of the part so only local cavities darken.
    let ao_bias = (0.02 * diag).max(bias);
    let ao_range = (0.22 * diag).max(ao_bias * 3.0);

    let mut polys: Vec<ProjPoly> = Vec::new();
    let mut edges: Vec<ProjEdge> = Vec::new();
    // Gradient defs for Gouraud-shaded (curved) facets, accumulated as we go.
    let mut gradients = String::new();
    let mut grad_id = 0usize;

    for (ai, art) in arts.iter().enumerate() {
        let proj = &projected[ai];
        for (ti, t) in art.tris.iter().enumerate() {
            if !art.visible[ti] {
                continue;
            }
            let vd = [
                dot(art.verts[t[0]], cam),
                dot(art.verts[t[1]], cam),
                dot(art.verts[t[2]], cam),
            ];
            let ps = [fin(proj[t[0]]), fin(proj[t[1]]), fin(proj[t[2]])];
            // Gouraud: shade each corner from its smoothed normal, darkened
            // by screen-space ambient occlusion. Curved facets get a linear
            // gradient; flat ones a single fill. Shared-vertex shades match
            // across edges (same normal-group, same AO sample point), so
            // curved surfaces stay continuous while hard edges stay crisp.
            let cn = &art.corner_normals[ti];
            // Floor weak AO to zero: a flat face triangulated as a coarse
            // fan picks up tiny (~0.05) occlusion variation between corners
            // that, amplified into the shade, would cross the gradient
            // threshold and draw a fan of seams. Only meaningful occlusion
            // (cavities, concave creases, part contact) survives the floor.
            let ao = |p: (f64, f64), d: f64| {
                ((zbuf.ao(p.0, p.1, d, ao_bias, ao_range) - AO_FLOOR) / (1.0 - AO_FLOOR)).max(0.0)
            };
            let sh = [
                shade(cn[0], cam, light) * (1.0 - AO_STRENGTH * ao(ps[0], vd[0])),
                shade(cn[1], cam, light) * (1.0 - AO_STRENGTH * ao(ps[1], vd[1])),
                shade(cn[2], cam, light) * (1.0 - AO_STRENGTH * ao(ps[2], vd[2])),
            ];
            let avg = hex(ramp_sample(&art.ramp, (sh[0] + sh[1] + sh[2]) / 3.0));
            let (fill, stroke) = match gouraud_gradient(ps, sh, &art.ramp) {
                Some(g) => {
                    let id = grad_id;
                    grad_id += 1;
                    gradients.push_str(&format!(
                        r#"<linearGradient id="g{id}" gradientUnits="userSpaceOnUse" x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}"><stop offset="0" stop-color="{}"/><stop offset="1" stop-color="{}"/></linearGradient>"#,
                        g.x1, g.y1, g.x2, g.y2, hex(g.lo), hex(g.hi),
                    ));
                    (format!("url(#g{id})"), avg)
                }
                None => (avg.clone(), avg),
            };
            polys.push(ProjPoly {
                s: ps,
                fill,
                stroke,
                depth: (vd[0] + vd[1] + vd[2]) / 3.0,
            });
        }
        for &(a, b, kind) in &art.edges {
            edges.push(ProjEdge {
                a: fin(proj[a]),
                da: dot(art.verts[a], cam),
                b: fin(proj[b]),
                db: dot(art.verts[b], cam),
                kind,
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

    // Hidden-line removal: clip every kept edge to its visible spans against
    // the depth buffer built above. (`bias` already defined alongside it.)
    // Minimum length (buffer cells) for an interior fragment of a smooth
    // curved-surface silhouette to survive. Exterior outlines (over
    // background) and long interior runs are kept; short interior stubs —
    // the generatrix hanging off a bore's rim tangent — are dropped.
    const SMOOTH_INTERIOR_MIN_CELLS: f64 = 18.0;
    // A hidden (occluded) span must be at least this long to be drawn
    // dashed — shorter ones are z-fighting noise, not real occluded edges.
    const HIDDEN_MIN_CELLS: f64 = 6.0;
    let mut crease_lines: Vec<Seg> = Vec::new();
    let mut outline_lines: Vec<Seg> = Vec::new();
    let mut hidden_lines: Vec<Seg> = Vec::new();
    for e in &edges {
        let clip = zbuf.clip_edge(e.a, e.da, e.b, e.db, bias);
        let dst = match e.kind {
            EdgeKind::Crease => &mut crease_lines,
            EdgeKind::Outline | EdgeKind::Smooth => &mut outline_lines,
        };
        for span in &clip.visible {
            let keep = match e.kind {
                EdgeKind::Smooth => span.over_bg || span.len_cells >= SMOOTH_INTERIOR_MIN_CELLS,
                // Hard edges and creases: keep everything visible.
                EdgeKind::Outline | EdgeKind::Crease => true,
            };
            if keep {
                dst.push((span.a, span.b));
            }
        }
        // Hidden-line convention: occluded hard edges and creases become
        // dashed lines (so a bore or pocket reads from a single view).
        // Smooth curved-surface silhouettes are never dashed — their
        // occluded fragments are tangent-point noise, not real edges.
        if matches!(e.kind, EdgeKind::Outline | EdgeKind::Crease) {
            for span in &clip.hidden {
                if span.len_cells >= HIDDEN_MIN_CELLS {
                    hidden_lines.push((span.a, span.b));
                }
            }
        }
    }

    // Emit SVG.
    let mut out = String::new();
    // Emit explicit width/height alongside viewBox so the SVG has intrinsic
    // dimensions. Without them, browsers compute auto/auto as 0×0 inside flex
    // containers (Chrome/Safari), which silently hides the render.
    out.push_str(&format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w:.2}" height="{h:.2}" viewBox="0 0 {w:.2} {h:.2}" role="img" aria-label="vcad render">"#
    ));

    // Gouraud gradient defs.
    let blur = ((w + h) * 0.008).clamp(1.0, 12.0);
    out.push_str(&format!(r#"<defs>{gradients}</defs>"#));

    // Vellum ground — the warm paper the plate sits on. Skipped for a
    // transparent render so the SVG composites over any background.
    if !transparent {
        out.push_str(&format!(
            r#"<rect x="0" y="0" width="{w:.2}" height="{h:.2}" fill="{PAPER}"/>"#
        ));
    }

    // Filled polygons. Each is stroked with its own fill colour at a
    // hairline width so adjacent triangles overlap by a sub-pixel sliver —
    // this closes the anti-alias seams that otherwise read as a faint fan
    // of lines across coplanar tessellated faces.
    out.push_str(r#"<g shape-rendering="geometricPrecision" stroke-linejoin="round">"#);
    for p in &polys {
        let pts = format!(
            "{:.2},{:.2} {:.2},{:.2} {:.2},{:.2}",
            p.s[0].0, p.s[0].1, p.s[1].0, p.s[1].1, p.s[2].0, p.s[2].1,
        );
        out.push_str(&format!(
            r#"<polygon points="{pts}" fill="{}" stroke="{}" stroke-width="0.75"/>"#,
            p.fill, p.stroke,
        ));
    }
    out.push_str("</g>");

    // Linework, drawn over the fills. Hidden (occluded) edges first as fine
    // dashed convention lines; then visible creases (light); then the bold
    // outline (heavy) on top. Edges meet exactly at shared vertices (round
    // caps) — no overshoot, so corners read precise rather than sketchy.
    let dash = (blur * 0.6).clamp(2.0, 6.0);
    let emit_lines = |out: &mut String,
                      lines: &[Seg],
                      width: f64,
                      opacity: f64,
                      dasharray: Option<f64>| {
        if lines.is_empty() {
            return;
        }
        let dash_attr = match dasharray {
            Some(d) => format!(r#" stroke-dasharray="{d:.2} {:.2}""#, d * 0.6),
            None => String::new(),
        };
        out.push_str(&format!(
                r#"<g stroke="{INK}" stroke-width="{width}" stroke-linecap="round" fill="none" opacity="{opacity}"{dash_attr}>"#
            ));
        for &(a, b) in lines {
            out.push_str(&format!(
                r#"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}"/>"#,
                a.0, a.1, b.0, b.1,
            ));
        }
        out.push_str("</g>");
    };
    emit_lines(&mut out, &hidden_lines, STROKE_HIDDEN_PX, 0.5, Some(dash));
    emit_lines(&mut out, &crease_lines, STROKE_CREASE_PX, 1.0, None);
    emit_lines(&mut out, &outline_lines, STROKE_OUTLINE_PX, 1.0, None);

    out.push_str("</svg>");
    Ok(out)
}

// ─── public API: raster JPEG ──────────────────────────────────────────────

#[cfg(feature = "raster")]
pub use raster::{
    render_jpeg_solids, render_jpeg_str, render_png_solids, render_png_str, RasterOptions,
};

#[cfg(feature = "raster")]
mod raster {
    use super::*;

    /// Curved primitives get a finer tessellation than the SVG path —
    /// faceted silhouettes are much more visible at 1024px.
    const RASTER_SEGMENTS: u32 = 64;

    /// Above this canvas size, curved primitives tessellate at
    /// [`RASTER_SEGMENTS_HIRES`] instead — 64 facets read as a polygon at
    /// 4096px. Keeping 1024px renders on the original segment count
    /// preserves mecheval reference images byte-for-byte.
    const HIRES_THRESHOLD_PX: u32 = 2048;
    /// Segment count for canvases at or above [`HIRES_THRESHOLD_PX`].
    /// Adjacent facets differ by 2.8°, still well under the ~10° coplanar
    /// tolerance, so no facet stripes appear.
    const RASTER_SEGMENTS_HIRES: u32 = 128;

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
        let tinted = evaluate_vcad(raw_vcad)?;
        let solids: Vec<Solid> = tinted.iter().map(|(s, _)| s.clone()).collect();
        let tints: Vec<Option<[f64; 3]>> = tinted.iter().map(|(_, t)| *t).collect();
        encode_jpeg(rasterize(&solids, &tints, opts)?, opts)
    }

    /// Render pre-evaluated solids to JPEG bytes, monochrome (no material
    /// tints — mecheval reference images depend on this). See
    /// [`render_jpeg_str`], which honours document material colours.
    pub fn render_jpeg_solids(solids: &[Solid], opts: &RasterOptions) -> Result<Vec<u8>, String> {
        let no_tints: Vec<Option<[f64; 3]>> = vec![None; solids.len()];
        encode_jpeg(rasterize(solids, &no_tints, opts)?, opts)
    }

    /// Render raw `.vcad` document JSON to RGBA PNG bytes with a fully
    /// transparent background (alpha 0 wherever no geometry or edge stroke
    /// was drawn). Same projection and shading as [`render_jpeg_str`];
    /// `opts.quality` is ignored (PNG is lossless).
    pub fn render_png_str(raw_vcad: &str, opts: &RasterOptions) -> Result<Vec<u8>, String> {
        let tinted = evaluate_vcad(raw_vcad)?;
        let solids: Vec<Solid> = tinted.iter().map(|(s, _)| s.clone()).collect();
        let tints: Vec<Option<[f64; 3]>> = tinted.iter().map(|(_, t)| *t).collect();
        encode_png(rasterize(&solids, &tints, opts)?, opts)
    }

    /// Render pre-evaluated solids to RGBA PNG bytes with a transparent
    /// background, monochrome (no material tints). See [`render_png_str`].
    pub fn render_png_solids(solids: &[Solid], opts: &RasterOptions) -> Result<Vec<u8>, String> {
        let no_tints: Vec<Option<[f64; 3]>> = vec![None; solids.len()];
        encode_png(rasterize(solids, &no_tints, opts)?, opts)
    }

    /// JPEG-encode a rasterized frame (coverage mask ignored — JPEG keeps
    /// the opaque vellum background).
    fn encode_jpeg(frame: (Vec<u8>, Vec<u8>), opts: &RasterOptions) -> Result<Vec<u8>, String> {
        let (rgb, _mask) = frame;
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

    /// PNG-encode a rasterized frame as RGBA: covered pixels are opaque,
    /// background pixels get alpha 0.
    fn encode_png(frame: (Vec<u8>, Vec<u8>), opts: &RasterOptions) -> Result<Vec<u8>, String> {
        let (rgb, mask) = frame;
        let mut rgba = Vec::with_capacity(mask.len() * 4);
        for (i, &a) in mask.iter().enumerate() {
            rgba.extend_from_slice(&rgb[i * 3..i * 3 + 3]);
            rgba.push(a);
        }
        let mut out = Vec::new();
        let enc = image::codecs::png::PngEncoder::new(&mut out);
        image::ImageEncoder::write_image(
            enc,
            &rgba,
            opts.size_px,
            opts.size_px,
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|e| format!("png encode: {}", e))?;
        Ok(out)
    }

    /// Shared raster implementation; `tints[i]`, when present, tints solid
    /// `i`'s shading ramp exactly as the SVG path does. Untinted solids keep
    /// the original two-stop monochrome shading, byte-for-byte. Returns the
    /// RGB frame plus a per-pixel coverage mask (255 where geometry or an
    /// edge stroke was drawn, 0 over untouched background).
    fn rasterize(
        solids: &[Solid],
        tints: &[Option<[f64; 3]>],
        opts: &RasterOptions,
    ) -> Result<(Vec<u8>, Vec<u8>), String> {
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

        let segments = if opts.size_px >= HIRES_THRESHOLD_PX {
            RASTER_SEGMENTS_HIRES
        } else {
            RASTER_SEGMENTS
        };
        let arts = build_artifacts(
            solids,
            tints,
            cam,
            segments,
            &EdgeRules {
                coplanar_dot_tol: RASTER_COPLANAR_DOT_TOL,
                mark_silhouette: true,
                keep_occluded: false,
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
        let mut mask: Vec<u8> = vec![0; size * size];

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
        for (ai, art) in arts.iter().enumerate() {
            let proj: Vec<(f64, f64, f64)> = art.verts.iter().map(|v| to_px(*v)).collect();
            let tinted = tints.get(ai).copied().flatten().is_some();
            for (ti, t) in art.tris.iter().enumerate() {
                if !art.visible[ti] {
                    continue;
                }
                let centroid_d = (proj[t[0]].2 + proj[t[1]].2 + proj[t[2]].2) / 3.0;
                let cue = 0.78 + 0.22 * ((centroid_d - dmin) / dspan).clamp(0.0, 1.0);
                let lit = (lambertian(art.normals[ti], light) * cue).clamp(0.0, 1.0);
                // Tinted parts sample their material ramp (same as the SVG
                // path); untinted parts keep the original monochrome shade
                // byte-for-byte (mecheval reference images).
                let shade = if tinted {
                    ramp_sample(&art.ramp, lit)
                } else {
                    mix_rgb(FILL_DARK, FILL_LIGHT, lit)
                };
                fill_triangle(
                    &mut rgb,
                    &mut zbuf,
                    &mut mask,
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
        // Stroke width scales with the canvas so lines keep the same
        // apparent weight at high resolution (2px at ≤1024, 8px at 4096).
        let stroke_px = (size / 512).max(2);
        for art in &arts {
            let proj: Vec<(f64, f64, f64)> = art.verts.iter().map(|v| to_px(*v)).collect();
            for &(a, b, _kind) in &art.edges {
                draw_edge(
                    &mut rgb, &zbuf, &mut mask, size, proj[a], proj[b], bias_base, stroke_px,
                );
            }
        }

        Ok((rgb, mask))
    }

    fn edge_fn(a: (f64, f64, f64), b: (f64, f64, f64), px: f64, py: f64) -> f64 {
        (b.0 - a.0) * (py - a.1) - (b.1 - a.1) * (px - a.0)
    }

    fn fill_triangle(
        rgb: &mut [u8],
        zbuf: &mut [f64],
        mask: &mut [u8],
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
                    mask[idx] = 255;
                    rgb[idx * 3] = shade[0];
                    rgb[idx * 3 + 1] = shade[1];
                    rgb[idx * 3 + 2] = shade[2];
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_edge(
        rgb: &mut [u8],
        zbuf: &[f64],
        mask: &mut [u8],
        size: usize,
        a: (f64, f64, f64),
        b: (f64, f64, f64),
        bias_base: f64,
        stroke_px: usize,
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
            // Paint a stroke_px × stroke_px block anchored at the sample
            // (right/down, matching the original 2px behaviour).
            for dy in 0..stroke_px as i64 {
                for dx in 0..stroke_px as i64 {
                    let (qx, qy) = (ix + dx, iy + dy);
                    if qx < 0 || qy < 0 || qx >= size as i64 || qy >= size as i64 {
                        continue;
                    }
                    let pi = qy as usize * size + qx as usize;
                    mask[pi] = 255;
                    let qi = pi * 3;
                    rgb[qi] = stroke[0];
                    rgb[qi + 1] = stroke[1];
                    rgb[qi + 2] = stroke[2];
                }
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

    fn cone_vcad(radius_bottom: f64, radius_top: f64, height: f64) -> String {
        format!(
            r#"{{
  "version": "0.1",
  "nodes": {{
    "1": {{
      "id": 1,
      "name": "Cone",
      "op": {{ "type": "Cone", "radius_bottom": {radius_bottom}, "radius_top": {radius_top}, "height": {height}, "segments": 0 }}
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

    /// Regression: an assembly-only document (partDefs + instances, no
    /// scene roots) must render its placed instances rather than failing
    /// with "no solids produced".
    #[test]
    fn assembly_instances_render() {
        let vcad = r#"{
  "version": "0.1",
  "nodes": {
    "1": { "id": 1, "name": "base", "op": { "type": "Cube", "size": { "x": 40.0, "y": 40.0, "z": 5.0 } } },
    "2": { "id": 2, "name": "post", "op": { "type": "Cylinder", "radius": 5.0, "height": 30.0, "segments": 0 } }
  },
  "materials": {},
  "part_materials": {},
  "roots": [],
  "partDefs": {
    "base": { "id": "base", "name": "base", "root": 1 },
    "post": { "id": "post", "name": "post", "root": 2 }
  },
  "instances": [
    { "id": "base1", "partDefId": "base", "transform": { "translation": { "x": 0.0, "y": 0.0, "z": 0.0 }, "rotation": { "x": 0.0, "y": 0.0, "z": 0.0 }, "scale": { "x": 1.0, "y": 1.0, "z": 1.0 } } },
    { "id": "post1", "partDefId": "post", "transform": { "translation": { "x": 20.0, "y": 20.0, "z": 5.0 }, "rotation": { "x": 0.0, "y": 0.0, "z": 0.0 }, "scale": { "x": 1.0, "y": 1.0, "z": 1.0 } } }
  ],
  "groundInstanceId": "base1"
}"#;
        let svg = render_svg_str(vcad, 2.0).expect("assembly should render");
        assert!(
            svg.matches("<polygon").count() > 6,
            "expected filled polygons for both instances"
        );
    }

    /// Regression: the cone's lateral facets wind inward, so winding-only
    /// orientation culled its front faces (you saw the concave inside) and
    /// the top view culled everything ("no visible polygons"). Signed-volume
    /// shell orientation must make every axis view produce visible fills.
    #[test]
    fn cone_renders_outward_in_all_views() {
        for view in [View::Isometric, View::Front, View::Side, View::Top] {
            let svg = render_svg_str_view(&cone_vcad(9.0, 0.0, 22.0), 4.0, view)
                .unwrap_or_else(|e| panic!("cone {view:?} should render, got: {e}"));
            assert!(
                svg.matches("<polygon").count() > 4,
                "cone {view:?} produced too few filled polygons — front faces likely culled"
            );
        }
    }

    /// The SVG path weights silhouettes/outlines heavier than interior
    /// creases. A cube viewed isometrically shows both: its boundary
    /// silhouette (outline) and the interior edges where visible faces meet
    /// (crease).
    #[test]
    fn svg_emits_weighted_outline_and_crease_strokes() {
        let svg = render_svg_str(&cube_vcad(20.0, 20.0, 20.0), 4.0).expect("cube should render");
        assert!(
            svg.contains(&format!(r#"stroke-width="{STROKE_OUTLINE_PX}""#)),
            "expected a bold outline stroke group"
        );
        assert!(
            svg.contains(&format!(r#"stroke-width="{STROKE_CREASE_PX}""#)),
            "expected a fine crease stroke group"
        );
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

        #[test]
        fn png_has_transparent_corners_and_opaque_center() {
            let png = render_png_str(
                &cube_vcad(20.0, 30.0, 10.0),
                &RasterOptions {
                    size_px: 128,
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
            let img = image::load_from_memory(&png).unwrap().to_rgba8();
            assert_eq!((img.width(), img.height()), (128, 128));
            // Part fills ~60% of the canvas from the center; corners stay
            // background and must be fully transparent.
            for (x, y) in [(0, 0), (127, 0), (0, 127), (127, 127)] {
                assert_eq!(
                    img.get_pixel(x, y)[3],
                    0,
                    "corner ({x},{y}) not transparent"
                );
            }
            assert_eq!(img.get_pixel(64, 64)[3], 255, "center should be opaque");
        }
    }

    /// `tint_ramp` was previously capped at k ≤ 0.2 — even fully-saturated
    /// material colours rendered as the default navy with the faintest hint
    /// of hue. This pins the contract that a saturated material actually
    /// shifts the ramp towards itself: pure red must read as warm, pure
    /// green as cool, and pure red and pure green must differ visibly.
    #[test]
    fn tint_ramp_respects_saturated_materials() {
        let navy_ramp = RAMP;
        let red = tint_ramp([1.0, 0.0, 0.0]);
        let green = tint_ramp([0.0, 1.0, 0.0]);

        // Pure red shifts the highlight to look red-dominant.
        let red_hi = red[3]; // brightest stop
        assert!(
            red_hi[0] > red_hi[2],
            "pure red ramp highlight should have R>B, got {:?}",
            red_hi
        );

        // Pure green shifts towards green-dominant.
        let green_hi = green[3];
        assert!(
            green_hi[1] > green_hi[2],
            "pure green ramp highlight should have G>B, got {:?}",
            green_hi
        );

        // Two distinct saturated colours produce visibly distinct ramps —
        // the old code returned ramps within ~5/255 of each other.
        let mut max_delta = 0i32;
        for (r, g) in red.iter().zip(green.iter()) {
            for c in 0..3 {
                let d = (r[c] as i32 - g[c] as i32).abs();
                if d > max_delta {
                    max_delta = d;
                }
            }
        }
        assert!(
            max_delta > 60,
            "saturated red vs green ramps should differ by >60/255, got {}",
            max_delta
        );

        // Achromatic materials (zero saturation) leave navy essentially
        // untouched — the brand-look contract documented above the
        // function still holds.
        let achromatic = tint_ramp([0.5, 0.5, 0.5]);
        for (a, n) in achromatic.iter().zip(navy_ramp.iter()) {
            for c in 0..3 {
                assert_eq!(
                    a[c], n[c],
                    "achromatic material must not shift the navy ramp"
                );
            }
        }
    }
}
