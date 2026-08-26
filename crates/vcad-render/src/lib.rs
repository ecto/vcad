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

#[cfg(feature = "raytrace")]
pub mod animate;
#[cfg(feature = "raytrace")]
pub mod envmap;
mod exact;
pub mod materials;
pub mod pcb;
#[cfg(feature = "raytrace")]
pub mod photoreal;
#[cfg(feature = "photoreal-gpu")]
pub mod photoreal_gpu;
pub mod web;

/// First PCB in a raw `.vcad` document, if any: `PcbBoard` nodes are
/// checked in node-id order, then the legacy top-level `pcb` field.
/// Used by the CLI to auto-select the top-down board view for ECAD
/// documents instead of the isometric mesh projection.
pub fn extract_pcb(raw: &str) -> Option<vcad_ir::ecad::Pcb> {
    let doc: vcad_ir::Document = serde_json::from_str(raw).ok()?;
    let mut ids: Vec<_> = doc.nodes.keys().copied().collect();
    ids.sort();
    for id in ids {
        if let Some(node) = doc.nodes.get(&id) {
            if let vcad_ir::CsgOp::PcbBoard { board } = &node.op {
                return Some((**board).clone());
            }
        }
    }
    doc.pcb
}
pub mod sheet;

use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};

use vcad_eval::{evaluate_document_with_sheet_metal, EvalOptions};
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
/// Accent outline stroke width for highlighted (just-changed) parts —
/// heavier than the standard outline so the change reads at a glance.
const STROKE_ACCENT_PX: f64 = 1.8;

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

#[cfg_attr(not(feature = "raster"), allow(dead_code))]
const FILL_DARK: [u8; 3] = [14, 57, 96];
#[cfg_attr(not(feature = "raster"), allow(dead_code))]
const FILL_LIGHT: [u8; 3] = [200, 220, 235];

/// Visible linework ink — a touch warmer and darker than the fill navy so
/// lines read as ink laid on the wash, not the same hue as the fill.
const INK: &str = "#0b2742";
/// Warm off-white "vellum" the drafting plate sits on (matches the raster
/// path's matte background). The signature ground behind every render.
const PAPER: &str = "#f4f3f1";
/// [`PAPER`] as RGB — ghosted (non-highlighted) parts fade toward this.
const PAPER_RGB: [u8; 3] = [244, 243, 241];

/// Brand orange — "interaction / attention" per docs/brand-spec.md. Used
/// only as the accent outline on highlighted (just-changed) parts, never
/// as a material colour.
const ACCENT: &str = "#f25c1f";

/// How far a ghosted part's shading ramp is pushed toward [`PAPER`] when a
/// highlight set is active — high enough that unchanged parts read as
/// context, low enough that their silhouettes stay legible.
const GHOST_MIX: f64 = 0.7;

/// Line opacity for ghosted parts' visible edges.
const GHOST_LINE_OPACITY: f64 = 0.35;

/// Background of a section-cut face, under the 45° hatch lines. A pale
/// ice tint from the ramp family, so cut faces read as freshly exposed
/// material rather than another shaded surface.
const HATCH_BG: &str = "#dce8f2";
/// Hatch line spacing in SVG user units.
const HATCH_SPACING_PX: f64 = 6.0;
/// Hatch line stroke width in SVG user units.
const HATCH_STROKE_PX: f64 = 0.8;

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
///
/// [`View::Orbit`] is an arbitrary orthographic orbit camera: `azimuth`
/// degrees counter-clockwise from +X in the XY plane, `elevation` degrees
/// above the XY plane (Z-up kernel frame). The camera sits on the unit
/// direction `(cos el·cos az, cos el·sin az, sin el)` looking at the scene
/// with +Z as the up reference, so e.g. `azimuth: 45, elevation: 35` is a
/// 3/4 view close to the classic isometric.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
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
    /// Arbitrary orthographic orbit camera (angles in degrees, Z-up).
    Orbit {
        /// Degrees counter-clockwise from +X in the XY plane.
        azimuth: f64,
        /// Degrees above the XY plane, clamped to `[-90, 90]`.
        elevation: f64,
    },
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
            View::Orbit { azimuth, elevation } => {
                let az = azimuth.to_radians();
                let el = elevation.clamp(-90.0, 90.0).to_radians();
                [el.cos() * az.cos(), el.cos() * az.sin(), el.sin()]
            }
        }
    }

    /// World direction mapping to screen +x.
    fn right(self) -> [f64; 3] {
        match self {
            // Screen-right for a camera at (1, 1, 1) looking at the origin
            // with +Z up: (−1, 1, 0)/√2. Non-unit on purpose — the √1.5
            // scale (vs the axis views) is the legacy isometric projection
            // scale. Was [COS30, −COS30, 0] until 2026-07, which mirrored
            // every isometric render; see `view_basis_handedness_is_pinned`.
            View::Isometric => [-COS30, COS30, 0.0],
            View::Front => [1.0, 0.0, 0.0],
            View::Side => [0.0, -1.0, 0.0],
            View::Top => [1.0, 0.0, 0.0],
            // Horizontal screen-right for a Z-up orbit camera. Depends on
            // azimuth only, so it stays well-defined at elevation ±90.
            View::Orbit { azimuth, .. } => {
                let az = azimuth.to_radians();
                [-az.sin(), az.cos(), 0.0]
            }
        }
    }

    /// World direction mapping to screen +y (SVG/raster y grows down).
    fn down(self) -> [f64; 3] {
        match self {
            View::Isometric => [SIN30, SIN30, -1.0],
            View::Front => [0.0, 0.0, -1.0],
            View::Side => [0.0, 0.0, -1.0],
            View::Top => [0.0, -1.0, 0.0],
            // down = (−cam) × right, completing a non-mirrored screen basis
            // (right × down points into the screen) like the axis views.
            View::Orbit { .. } => {
                let cam = self.cam();
                cross([-cam[0], -cam[1], -cam[2]], self.right())
            }
        }
    }
}

impl std::str::FromStr for View {
    type Err = String;

    /// Accepts `iso`/`isometric` (and `hero`, which renders as the same
    /// 3/4 view), `front`, `side`, `top`, or `orbit:<azimuth>,<elevation>`
    /// with angles in degrees (e.g. `orbit:35,25`).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let lower = s.to_ascii_lowercase();
        if let Some(angles) = lower.strip_prefix("orbit:") {
            let parse_orbit = || -> Option<View> {
                let (az, el) = angles.split_once(',')?;
                let azimuth: f64 = az.trim().parse().ok()?;
                let elevation: f64 = el.trim().parse().ok()?;
                if !(azimuth.is_finite() && elevation.is_finite()) {
                    return None;
                }
                Some(View::Orbit { azimuth, elevation })
            };
            return parse_orbit().ok_or_else(|| {
                format!("bad orbit view '{s}' (expected orbit:<azimuth>,<elevation> in degrees)")
            });
        }
        match lower.as_str() {
            "iso" | "isometric" | "hero" => Ok(View::Isometric),
            "front" => Ok(View::Front),
            "side" => Ok(View::Side),
            "top" => Ok(View::Top),
            other => Err(format!(
                "unknown view '{other}' (expected iso|front|side|top|hero|orbit:<az>,<el>)"
            )),
        }
    }
}

// ─── section (cutaway) planes ─────────────────────────────────────────────

/// A principal axis, naming the normal of a [`SectionPlane`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// Plane normal to X (`--section x=N`).
    X,
    /// Plane normal to Y (`--section y=N`).
    Y,
    /// Plane normal to Z (`--section z=N`).
    Z,
}

impl Axis {
    fn index(self) -> usize {
        match self {
            Axis::X => 0,
            Axis::Y => 1,
            Axis::Z => 2,
        }
    }
}

/// A section (cutaway) plane normal to a principal axis. Material on the
/// camera's side of the plane is removed before rendering — so the viewer
/// always looks into the cut — and the exposed cut faces are drawn with a
/// 45° drafting hatch instead of the usual shading.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SectionPlane {
    /// The axis the plane is normal to.
    pub axis: Axis,
    /// The axis coordinate (mm) the plane sits at.
    pub coord: f64,
}

impl std::str::FromStr for SectionPlane {
    type Err = String;

    /// Parses `x=N`, `y=N`, or `z=N` (case-insensitive), e.g. `z=10`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (axis, val) = s
            .split_once('=')
            .ok_or_else(|| format!("bad section '{s}' (expected x=N|y=N|z=N)"))?;
        let axis = match axis.trim().to_ascii_lowercase().as_str() {
            "x" => Axis::X,
            "y" => Axis::Y,
            "z" => Axis::Z,
            other => return Err(format!("bad section axis '{other}' (expected x|y|z)")),
        };
        let coord: f64 = val
            .trim()
            .parse()
            .map_err(|e: std::num::ParseFloatError| format!("bad section coordinate: {e}"))?;
        Ok(SectionPlane { axis, coord })
    }
}

impl SectionPlane {
    /// Tolerance (mm) within which a vertex counts as lying on the plane.
    /// Tessellated vertices round-trip through `f32`, so the tolerance
    /// scales with the plane coordinate's own magnitude.
    fn on_plane_tol(self) -> f64 {
        (self.coord.abs() * 1e-5).max(1e-2)
    }
}

/// Which side of the section plane gets cut away: always the side the
/// camera sits on, so the viewer looks *into* the section. For a view
/// whose camera is edge-on to the plane's axis (a Front view with an
/// `x=` section) the positive side is removed by convention.
fn section_removes_positive_side(plane: SectionPlane, view: View) -> bool {
    view.cam()[plane.axis.index()] >= 0.0
}

/// Cut one solid by `plane`, removing material on one side via a boolean
/// difference against a generous half-space box. `remove_positive` picks
/// the removed side (`axis > coord` when true, `axis < coord` when false).
///
/// `Ok(None)` means the solid was cut away entirely (it lies wholly on the
/// removed side); `Err` surfaces a boolean failure or kernel panic so the
/// caller can fall back to the uncut solid.
fn section_solid(
    solid: &Solid,
    plane: SectionPlane,
    remove_positive: bool,
) -> Result<Option<Solid>, String> {
    let (lo, hi) = solid.bounding_box();
    let ax = plane.axis.index();
    if !lo[ax].is_finite() || !hi[ax].is_finite() {
        return Err("degenerate bounding box".to_string());
    }
    let (kept_extent, removed_extent) = if remove_positive {
        (plane.coord - lo[ax], hi[ax] - plane.coord)
    } else {
        (hi[ax] - plane.coord, plane.coord - lo[ax])
    };
    if removed_extent <= 0.0 {
        return Ok(Some(solid.clone())); // plane clear of the solid — nothing removed
    }
    if kept_extent <= 0.0 {
        return Ok(None); // wholly on the removed side
    }
    // Cutter: a box overhanging the solid's bbox on every open side, with
    // one face on the section axis sitting exactly at the plane — so the
    // cut faces the boolean leaves behind are planar at `coord`.
    let margin = 0.1 * ((hi[0] - lo[0]).max(hi[1] - lo[1]).max(hi[2] - lo[2])).max(1.0) + 1.0;
    let mut size = [
        hi[0] - lo[0] + 2.0 * margin,
        hi[1] - lo[1] + 2.0 * margin,
        hi[2] - lo[2] + 2.0 * margin,
    ];
    let mut corner = [lo[0] - margin, lo[1] - margin, lo[2] - margin];
    size[ax] = removed_extent + margin;
    if remove_positive {
        corner[ax] = plane.coord;
    } else {
        corner[ax] = plane.coord - size[ax];
    }
    let cutter = Solid::cube(size[0], size[1], size[2])
        .apply_transform(&Transform::translation(corner[0], corner[1], corner[2]));
    let cut = catch_unwind(AssertUnwindSafe(|| solid.try_difference(&cutter)))
        .map_err(|_| "boolean panicked".to_string())?
        .map_err(|e| format!("boolean failed: {e:?}"))?;
    Ok(Some(cut))
}

/// Apply a section plane to every scene solid, removing the half of each
/// on the camera's side of the plane (see [`section_removes_positive_side`]).
/// A solid whose boolean fails is kept uncut (never fail the whole render),
/// with a note on stderr; solids wholly on the removed side are dropped.
fn apply_section(scene: Vec<SceneSolid>, plane: SectionPlane, view: View) -> Vec<SceneSolid> {
    let remove_positive = section_removes_positive_side(plane, view);
    let mut out = Vec::with_capacity(scene.len());
    for (i, mut s) in scene.into_iter().enumerate() {
        match section_solid(&s.solid, plane, remove_positive) {
            Ok(Some(cut)) => {
                s.solid = cut;
                out.push(s);
            }
            Ok(None) => {}
            Err(e) => {
                eprintln!("vcad-render: section: solid {i} left uncut ({e})");
                out.push(s);
            }
        }
    }
    out
}

// ─── annotations ──────────────────────────────────────────────────────────

/// Opt-in engineering-context overlays. Everything defaults to off, and the
/// default render is byte-identical to an annotation-free build — the
/// overlays exist primarily so the MCP `render_view` "agent eyes" path can
/// carry orientation, identity, and scale in the image itself.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RenderAnnotations {
    /// Draw an X/Y/Z origin gizmo (kernel is Z-up) projected in the current
    /// view, in the lower-left corner of the canvas.
    pub axes: bool,
    /// Label each top-level part with its name, anchored at its projected
    /// bounding-box centre with a leader line.
    pub labels: bool,
    /// Draw overall W×D×H bounding-box dimensions with mm values, using
    /// drafting-style extension lines.
    pub dims: bool,
}

impl RenderAnnotations {
    /// True when any overlay is enabled.
    pub fn any(&self) -> bool {
        self.axes || self.labels || self.dims
    }
}

/// Extra canvas margin (SVG user units / raster px at SVG scale) reserved
/// when any annotation is on, so dimension lines, axis arrows, and labels
/// land outside the model's silhouette instead of over it.
const ANNOT_MARGIN_PX: f64 = 46.0;
/// Dimension-line offset from the model bbox, and the further text offset.
const DIM_OFFSET_PX: f64 = 16.0;
/// Axis gizmo arrow length.
const AXIS_LEN_PX: f64 = 24.0;
/// Axis gizmo colours (X, Y, Z) — the conventional RGB triad, muted to sit
/// on the vellum ground.
const AXIS_COLORS: [&str; 3] = ["#c0392b", "#1e8e3e", "#2b6cb0"];
const AXIS_NAMES: [&str; 3] = ["X", "Y", "Z"];

/// One overall bounding-box dimension: a world-space baseline (two bbox
/// corners along one axis) plus its drafting label.
struct DimSpec {
    a: [f64; 3],
    b: [f64; 3],
    label: String,
}

/// Format a millimetre value for a dimension label: integers stay bare
/// ("20"), fractional extents keep one decimal ("12.5").
fn format_mm(v: f64) -> String {
    if (v - v.round()).abs() < 0.05 {
        format!("{}", v.round() as i64)
    } else {
        format!("{v:.1}")
    }
}

/// Overall W (X) / D (Y) / H (Z) dimensions along the lower-right silhouette
/// edges of the world bbox — the classic drafting placement for an
/// isometric, and still sensible corners in the axis views.
fn bbox_dim_specs(lo3: [f64; 3], hi3: [f64; 3]) -> Vec<DimSpec> {
    let (lo, hi) = (lo3, hi3);
    vec![
        DimSpec {
            a: [lo[0], lo[1], lo[2]],
            b: [hi[0], lo[1], lo[2]],
            label: format!("W {} mm", format_mm(hi[0] - lo[0])),
        },
        DimSpec {
            a: [hi[0], lo[1], lo[2]],
            b: [hi[0], hi[1], lo[2]],
            label: format!("D {} mm", format_mm(hi[1] - lo[1])),
        },
        DimSpec {
            a: [hi[0], hi[1], lo[2]],
            b: [hi[0], hi[1], hi[2]],
            label: format!("H {} mm", format_mm(hi[2] - lo[2])),
        },
    ]
}

/// Escape a part name for embedding in SVG text content / attributes.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

// ─── .vcad → solids ───────────────────────────────────────────────────────

/// A solid plus its material colour (linear RGB in `[0,1]`), its
/// human-readable name, and the labels a focus query can match against.
/// The colour tints the shading ramp; `name` feeds the optional part-label
/// annotation; `labels` (node name, instance id/name, part-def id) drive
/// `--focus`.
#[derive(Clone)]
struct SceneSolid {
    solid: Solid,
    tint: Option<[f64; 3]>,
    /// Full material definition (metallic/roughness/transmission/ior), kept
    /// alongside `tint` because the photoreal path needs more than a colour.
    /// `None` when the document names no material for this solid.
    ///
    /// Only the `raytrace`-gated `photoreal` module reads this; the SVG and
    /// raster paths shade from `tint` alone.
    #[cfg_attr(not(feature = "raytrace"), allow(dead_code))]
    material: Option<vcad_ir::MaterialDef>,
    name: Option<String>,
    /// Focus-match labels (node name, instance id/name, part-def id) for
    /// `--focus` / `CameraOptions::focus`.
    labels: Vec<String>,
    /// Root node id (as a string) for scene roots; instance id for
    /// assembly instances. Matches the `part_id` the MCP mutation diff
    /// reports, so a highlight set can select by it.
    id: String,
}

thread_local! {
    /// Root-mesh cache consulted by [`evaluate_vcad`] on this thread — see
    /// [`with_root_cache`]. A thread-local rather than a parameter because
    /// every `render_*_str` entry point funnels through `evaluate_vcad`, and
    /// threading an option through all of them would change a dozen public
    /// signatures for one CLI feature.
    static ROOT_CACHE: std::cell::RefCell<Option<(std::rc::Rc<dyn vcad_eval::cache::RootMeshCache>, u32)>> =
        const { std::cell::RefCell::new(None) };
}

/// The segment count the tessellated renderers draw curved faces at for a
/// given output: the SVG path's constant, or the raster path's size-scaled
/// count. `None` size means SVG. This is what [`with_root_cache`] must be
/// given so a cached mesh has the same facet count as a fresh tessellation
/// — the two must be pixel-identical, and they are only at equal segments.
pub fn tessellation_segments(raster_size_px: Option<u32>) -> u32 {
    match raster_size_px {
        None => TESSELLATION_SEGMENTS,
        #[cfg(feature = "raster")]
        Some(px) => raster::segments_for(px),
        #[cfg(not(feature = "raster"))]
        Some(_) => TESSELLATION_SEGMENTS,
    }
}

/// Run `f` with `cache` supplying (and receiving) evaluated root meshes for
/// every document evaluated on this thread in the meantime, tessellated at
/// `segments` (see [`tessellation_segments`]).
///
/// A cache hit yields a mesh-backed `Solid` (no BRep), which the tessellated
/// raster and SVG paths render identically to a freshly evaluated root.
///
/// The photoreal path is cacheable too, and by default is: it traces
/// triangles at `photoreal::MESH_SEGMENTS`, so a cached mesh is all the
/// geometry it wants, and it tessellates on a miss as well so cold and warm
/// renders agree. Wrap it in a scope built with those segments —
/// [`tessellation_segments`] answers for the raster and SVG paths, not for
/// this one.
///
/// `--raytrace`, `--section` and photoreal's `--exact`
/// (`PhotorealOptions::exact`) do need analytic BRep surfaces,
/// which a cached mesh cannot supply. Callers must not wrap those in a cache
/// scope: they would pay to populate a cache they can never read, and a hit
/// would silently downgrade them to triangles.
pub fn with_root_cache<T>(
    cache: std::rc::Rc<dyn vcad_eval::cache::RootMeshCache>,
    segments: u32,
    f: impl FnOnce() -> T,
) -> T {
    let prev = ROOT_CACHE.with(|c| c.replace(Some((cache, segments))));
    struct Restore(Option<(std::rc::Rc<dyn vcad_eval::cache::RootMeshCache>, u32)>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let prev = self.0.take();
            ROOT_CACHE.with(|c| *c.borrow_mut() = prev);
        }
    }
    let _restore = Restore(prev);
    f()
}

// Count of documents evaluated by `evaluate_vcad_document` — the one funnel
// every render path goes through.
//
// Exists so the animation path can *prove* it evaluates geometry once for a
// whole sequence rather than once per frame; the integration test asserts
// the counter advances by exactly one across an N-frame render, which is the
// only thing that would catch a regression putting evaluation back inside
// the frame loop.
//
// Per *thread*, not per process: a render is driven from one thread (rayon
// parallelism lives inside the tracer, below this level), and a global would
// be unreadable in a test binary running cases concurrently.
thread_local! {
    static DOCUMENT_EVALS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// How many documents this thread has evaluated. See [`DOCUMENT_EVALS`].
pub fn document_eval_count() -> u64 {
    DOCUMENT_EVALS.with(|c| c.get())
}

/// One assembly instance's geometry, still in its part-definition frame.
///
/// The world placement is deliberately *not* baked in: an animation re-poses
/// the same solids every frame, so the geometry (and the BVH built over it)
/// has to outlive any one pose.
struct ArticulatedPart {
    /// Instance id — the key forward kinematics reports world poses under.
    instance_id: String,
    /// Local, unplaced geometry. Shared between instances of one part def.
    solid: Solid,
    tint: Option<[f64; 3]>,
    #[cfg_attr(not(feature = "raytrace"), allow(dead_code))]
    material: Option<vcad_ir::MaterialDef>,
    name: Option<String>,
    labels: Vec<String>,
}

/// A document evaluated once: its static scene roots, world-placed, plus its
/// assembly instances in local space alongside the document itself (which
/// carries the joints those instances hang off).
struct EvaluatedDocument {
    doc: vcad_ir::Document,
    statics: Vec<SceneSolid>,
    parts: Vec<ArticulatedPart>,
}

/// Forward kinematics for an assembly document — the same solver the app's
/// assembly mode and the MCP timeline use, so a render agrees with the
/// viewer by construction rather than by a re-implementation.
fn assembly_world_transforms(
    doc: &vcad_ir::Document,
) -> Result<HashMap<String, vcad_ir::Transform3D>, String> {
    catch_unwind(AssertUnwindSafe(|| {
        vcad_eval::solve_forward_kinematics(doc)
    }))
    .map_err(|_| "fk panicked".to_string())
}

/// An articulated part as a scene solid in its *local* frame, keyed by
/// instance id. The animation path builds its BVHs from these once and then
/// only ever changes the placement.
#[cfg(feature = "raytrace")]
fn part_as_local_scene_solid(part: ArticulatedPart) -> SceneSolid {
    SceneSolid {
        solid: part.solid,
        tint: part.tint,
        material: part.material,
        name: part.name,
        labels: part.labels,
        id: part.instance_id,
    }
}

/// Place an articulated part by the world pose FK gave its instance, falling
/// back to the instance's own static transform.
fn place_part(
    part: ArticulatedPart,
    world: &HashMap<String, vcad_ir::Transform3D>,
    fallback: Option<vcad_ir::Transform3D>,
) -> SceneSolid {
    let placed = match world.get(&part.instance_id).cloned().or(fallback) {
        Some(t) => part.solid.apply_transform(&transform3d_to_kernel(&t)),
        None => part.solid,
    };
    SceneSolid {
        solid: placed,
        tint: part.tint,
        material: part.material,
        name: part.name,
        labels: part.labels,
        id: part.instance_id,
    }
}

fn evaluate_vcad(raw_vcad: &str) -> Result<Vec<SceneSolid>, String> {
    let ev = evaluate_vcad_document(raw_vcad)?;
    let mut solids = ev.statics;
    if !ev.parts.is_empty() {
        let world = assembly_world_transforms(&ev.doc)?;
        let fallbacks: HashMap<&str, Option<vcad_ir::Transform3D>> = ev
            .doc
            .instances
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|i| (i.id.as_str(), i.transform))
            .collect();
        for part in ev.parts {
            let fallback = fallbacks
                .get(part.instance_id.as_str())
                .copied()
                .unwrap_or(None);
            solids.push(place_part(part, &world, fallback));
        }
    }
    Ok(solids)
}

fn evaluate_vcad_document(raw_vcad: &str) -> Result<EvaluatedDocument, String> {
    DOCUMENT_EVALS.with(|c| c.set(c.get() + 1));
    let parsed = parse_vcad_file(raw_vcad).map_err(|e| format!("parse: {}", e))?;
    let (root_cache, mesh_segments) = ROOT_CACHE
        .with(|c| c.borrow().clone())
        .map_or((None, 0), |(c, n)| (Some(c), n));
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
        evaluate_document_with_sheet_metal(
            &parsed.document,
            &EvalOptions {
                skip_clash_detection: true,
                root_cache,
                mesh_segments,
                ..Default::default()
            },
        )
    }))
    .map_err(|_| "eval panicked".to_string())?
    .map_err(|e| format!("eval: {}", e))?;

    let materials = &parsed.document.materials;
    // `scene.parts` is 1:1 with the document's *visible* roots, in order —
    // failed roots still push an (empty) part — so zipping recovers each
    // part's root node and thence its authored name.
    let visible_roots: Vec<&vcad_ir::SceneEntry> = parsed
        .document
        .roots
        .iter()
        .filter(|r| r.visible != Some(false))
        .collect();
    // Part-definition prototype roots: a URDF import pushes each link's
    // geometry both as a scene root AND as a part-def referenced by a
    // world-placed instance. Drawing the root too would pile every link
    // untransformed at the origin on top of the FK-placed assembly.
    let proto_roots: std::collections::HashSet<vcad_ir::NodeId> =
        match (&parsed.document.part_defs, &parsed.document.instances) {
            (Some(defs), Some(insts)) if !insts.is_empty() => {
                defs.values().map(|d| d.root).collect()
            }
            _ => Default::default(),
        };
    let solids: Vec<SceneSolid> = scene
        .parts
        .iter()
        .enumerate()
        .filter_map(|(i, p)| {
            if visible_roots
                .get(i)
                .is_some_and(|r| proto_roots.contains(&r.root))
            {
                return None;
            }
            // A root whose chain bottoms out in an `ImportedMesh` (frozen
            // topology-optimization results, drag-dropped STL/GLB) carries
            // no `Solid` — the evaluator has no path from a triangle soup
            // back to a BRep. Wrap the raw mesh so it still reaches the
            // renderer instead of vanishing from the scene entirely.
            let solid = p.solid.clone().or_else(|| {
                (!p.mesh.indices.is_empty()).then(|| {
                    Solid::from_mesh(vcad_kernel::vcad_kernel_tessellate::TriangleMesh {
                        vertices: p.mesh.positions.clone(),
                        indices: p.mesh.indices.clone(),
                        normals: p.mesh.normals.clone().unwrap_or_default(),
                        face_kinds: p.mesh.face_kinds.clone().unwrap_or_default(),
                    })
                })
            });
            // A named-but-undeclared material (`[root rail "aluminum"]` with
            // no `[material ...]` block, which is how every hand-written
            // `.loon` does it) falls back to the built-in library rather than
            // to the renderer's default clay.
            let material = materials::resolve(materials, &p.material);
            solid.map(|s| {
                let name = visible_roots
                    .get(i)
                    .and_then(|r| parsed.document.nodes.get(&r.root))
                    .and_then(|n| n.name.clone());
                SceneSolid {
                    solid: s,
                    tint: material.as_ref().map(|m| m.color),
                    material: material.clone(),
                    labels: name.clone().into_iter().collect(),
                    name,
                    id: visible_roots
                        .get(i)
                        .map(|r| r.root.to_string())
                        .unwrap_or_default(),
                }
            })
        })
        .collect();

    // Assembly instances: `evaluate_document` only carries meshes for
    // instances, not BRep solids, so re-evaluate each referenced part
    // definition once and hand back a local copy per instance. Without
    // this, an assembly-only document (no scene roots) rendered as
    // "no solids produced" despite being perfectly valid.
    let parts = evaluate_assembly_instances(&parsed.document, &scene)?;
    Ok(EvaluatedDocument {
        doc: parsed.document,
        statics: solids,
        parts,
    })
}

/// Evaluate the document's assembly instances (if any) into tinted solids in
/// part-definition space. Part-definition solids are evaluated once and
/// shared; placement is the caller's job (see [`place_part`]) so an
/// animation can re-pose the same geometry without re-evaluating it.
fn evaluate_assembly_instances(
    doc: &vcad_ir::Document,
    scene: &vcad_eval::EvaluatedScene,
) -> Result<Vec<ArticulatedPart>, String> {
    let (Some(part_defs), Some(instances)) = (&doc.part_defs, &doc.instances) else {
        return Ok(Vec::new());
    };
    if part_defs.is_empty() || instances.is_empty() {
        return Ok(Vec::new());
    }

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
                // No BRep (mesh-imported part, e.g. a URDF link's STL):
                // wrap the evaluated triangle mesh, same as the roots path.
                .or_else(|| {
                    scene
                        .part_defs
                        .as_ref()?
                        .iter()
                        .find(|pd| pd.id == inst.part_def_id)
                        .filter(|pd| !pd.mesh.indices.is_empty())
                        .map(|pd| {
                            Solid::from_mesh(vcad_kernel::vcad_kernel_tessellate::TriangleMesh {
                                vertices: pd.mesh.positions.clone(),
                                indices: pd.mesh.indices.clone(),
                                normals: pd.mesh.normals.clone().unwrap_or_default(),
                                face_kinds: pd.mesh.face_kinds.clone().unwrap_or_default(),
                            })
                        })
                })
            })
            .clone();
        let Some(solid) = solid else { continue };

        let material = inst
            .material
            .clone()
            .or_else(|| def.default_material.clone())
            .unwrap_or_else(|| "default".to_string());
        let material_def = materials::resolve(&doc.materials, &material);
        let color = material_def.as_ref().map(|m| m.color);
        let name = inst
            .name
            .clone()
            .or_else(|| def.name.clone())
            .or_else(|| Some(inst.id.clone()));
        let mut labels = vec![inst.id.clone(), inst.part_def_id.clone()];
        if let Some(n) = &inst.name {
            labels.push(n.clone());
        }
        out.push(ArticulatedPart {
            instance_id: inst.id.clone(),
            solid,
            tint: color,
            material: material_def,
            name,
            labels,
        });
    }
    Ok(out)
}

/// Resolve a `focus` query to a per-solid mask (case-insensitive match
/// against each solid's labels: root node name, instance id/name, part-def
/// id). Errors with the available labels when nothing matches.
fn focus_mask(scene: &[SceneSolid], focus: &str) -> Result<Vec<bool>, String> {
    let want = focus.trim().to_ascii_lowercase();
    let mask: Vec<bool> = scene
        .iter()
        .map(|s| s.labels.iter().any(|l| l.to_ascii_lowercase() == want))
        .collect();
    if mask.iter().any(|&m| m) {
        return Ok(mask);
    }
    let mut available: Vec<&str> = scene
        .iter()
        .flat_map(|s| s.labels.iter().map(String::as_str))
        .collect();
    available.sort_unstable();
    available.dedup();
    Err(format!(
        "focus part '{focus}' not found (available: {})",
        if available.is_empty() {
            "no named parts".to_string()
        } else {
            available.join(", ")
        }
    ))
}

/// IR `Transform3D` → kernel `Transform`, matching the evaluator's
/// convention: scale, then Rx·Ry·Rz (applied x-first), then translation.
fn transform3d_to_kernel(t: &vcad_ir::Transform3D) -> Transform {
    // `Transform::then` composes self·other with column vectors, so `other`
    // acts on the point FIRST. The intended world placement is
    // T · Rz · Ry · Rx · S (scale first, translation last — matching the
    // Rz·Ry·Rx euler convention `vcad_eval::kinematics` emits), so the chain
    // reads outermost-first. Chaining the other way rotated the translation
    // itself, which swung a jointed child about the world origin instead of
    // about its parent anchor.
    Transform::translation(t.translation.x, t.translation.y, t.translation.z)
        .then(&Transform::rotation_z(t.rotation.z.to_radians()))
        .then(&Transform::rotation_y(t.rotation.y.to_radians()))
        .then(&Transform::rotation_x(t.rotation.x.to_radians()))
        .then(&Transform::scale(t.scale.x, t.scale.y, t.scale.z))
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
    /// Index of the source solid in the caller's `solids` slice. Solids
    /// that fail to tessellate are skipped, so artifact index alone doesn't
    /// line up with the input — needed to re-derive exact BRep edges, to
    /// recover per-solid metadata (names for labels), and for focus lookup.
    src: usize,
    verts: Vec<[f64; 3]>,
    tris: Vec<[usize; 3]>,
    #[cfg_attr(not(feature = "raster"), allow(dead_code))]
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
    /// Per-triangle: true when the triangle is an exposed section-cut face
    /// (planar on the section plane) — rendered cross-hatched.
    cut: Vec<bool>,
    /// Kept edges (boundary, crease, or silhouette, with ≥1 visible
    /// adjacent triangle) as canonical-vertex index pairs plus their kind.
    edges: Vec<(usize, usize, EdgeKind)>,
    /// How this solid participates in a highlight pass (Normal when no
    /// highlight set is active).
    emphasis: Emphasis,
}

/// A solid's role when a highlight set is active: `Accent` parts keep
/// their material colour and gain the brand-orange outline, `Ghost` parts
/// fade toward the paper, `Normal` means no highlight pass at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Emphasis {
    Normal,
    Accent,
    Ghost,
}

/// Fade `ramp` toward [`PAPER_RGB`] by [`GHOST_MIX`] — the ghosted look
/// for parts outside the highlight set.
fn ghost_ramp(ramp: [[u8; 3]; 4]) -> [[u8; 3]; 4] {
    let mut out = ramp;
    for stop in out.iter_mut() {
        *stop = mix_rgb(*stop, PAPER_RGB, GHOST_MIX);
    }
    out
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

#[allow(clippy::too_many_arguments)]
fn build_artifacts(
    solids: &[Solid],
    tints: &[Option<[f64; 3]>],
    accents: &[bool],
    cam: [f64; 3],
    segments: u32,
    rules: &EdgeRules,
    section: Option<SectionPlane>,
    style: RenderStyle,
) -> Vec<SolidArtifacts> {
    let highlighting = accents.iter().any(|&a| a);
    let mut out = Vec::new();
    for (si, solid) in solids.iter().enumerate() {
        let emphasis = if !highlighting {
            Emphasis::Normal
        } else if accents.get(si).copied().unwrap_or(false) {
            Emphasis::Accent
        } else {
            Emphasis::Ghost
        };
        let mut ramp = tints
            .get(si)
            .copied()
            .flatten()
            .map(match style {
                RenderStyle::Drafting => tint_ramp,
                RenderStyle::Shaded => shaded_ramp,
            })
            .unwrap_or(RAMP);
        if emphasis == Emphasis::Ghost {
            ramp = ghost_ramp(ramp);
        }
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

        // Exposed cut faces: every vertex on the section plane (within
        // tolerance) and the face normal parallel to the plane's axis.
        let cut: Vec<bool> = match section {
            Some(plane) => {
                let ax = plane.axis.index();
                let tol = plane.on_plane_tol();
                cm.tris
                    .iter()
                    .enumerate()
                    .map(|(ti, t)| {
                        normals[ti][ax].abs() > 0.99
                            && t.iter()
                                .all(|&v| (cm.verts[v][ax] - plane.coord).abs() <= tol)
                    })
                    .collect()
            }
            None => vec![false; cm.tris.len()],
        };

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
            src: si,
            verts: cm.verts,
            tris: cm.tris,
            normals,
            corner_normals,
            ramp,
            visible,
            cut,
            edges,
            emphasis,
        });
    }
    out
}

// ─── geometry helpers ─────────────────────────────────────────────────────

/// Build an SVG path `d` string tracing the exact projected ellipse arc
/// from parameter `t0` to `t1` (radians on the source circle).
///
/// `a2`/`b2` are the ellipse's screen-space conjugate radius vectors — the
/// projections of the circle's `radius·u` and `radius·v` — so a point at
/// parameter θ is `p_screen(θ) = center' + a2·cosθ + b2·sinθ`. The SVG arc
/// command needs canonical semi-axes: they come from the eigen-decomposition
/// of `M·Mᵀ` with `M = [a2 b2]`, which is exact. The arc is split into
/// ≤90° chunks so the large-arc flag is never needed, and the sweep flag is
/// read off the actual turn direction of each chunk's sampled midpoint.
fn arc_path_d(
    a2: (f64, f64),
    b2: (f64, f64),
    p_screen: &dyn Fn(f64) -> (f64, f64),
    t0: f64,
    t1: f64,
) -> String {
    // A zero-extent interval (the caller's `d <= 0.0` guard should already
    // exclude these) would emit an arc with start == end, which renderers
    // treat inconsistently (no-op vs full ellipse). Emit a bare moveto.
    if (t1 - t0).abs() < 1e-9 {
        let (x, y) = p_screen(t0);
        return format!("M {x:.3} {y:.3}");
    }
    // M·Mᵀ of the 2×2 linear map [a2 b2].
    let m00 = a2.0 * a2.0 + b2.0 * b2.0;
    let m01 = a2.0 * a2.1 + b2.0 * b2.1;
    let m11 = a2.1 * a2.1 + b2.1 * b2.1;
    // Diagonalizing rotation; the radius along direction φ is √λ_φ.
    let phi = 0.5 * (2.0 * m01).atan2(m00 - m11);
    let (cp, sp) = (phi.cos(), phi.sin());
    let l1 = (m00 * cp * cp + 2.0 * m01 * sp * cp + m11 * sp * sp).max(0.0);
    let l2 = (m00 + m11 - l1).max(0.0);
    let rx = l1.sqrt();
    let ry = l2.sqrt();
    let rot_deg = phi.to_degrees();

    let chunks = (((t1 - t0) / (std::f64::consts::FRAC_PI_2 * 0.99)).ceil() as usize).max(1);
    let start = p_screen(t0);
    let mut d = format!("M {:.3} {:.3}", start.0, start.1);
    let mut prev = start;
    let mut prev_t = t0;
    for k in 1..=chunks {
        let t = t0 + (t1 - t0) * k as f64 / chunks as f64;
        let end = p_screen(t);
        let mid = p_screen((prev_t + t) / 2.0);
        // Turn direction across the chunk: positive cross in SVG's y-down
        // frame is the "positive angle" (sweep = 1) direction.
        let c = (mid.0 - prev.0) * (end.1 - mid.1) - (mid.1 - prev.1) * (end.0 - mid.0);
        let sweep = if c > 0.0 { 1 } else { 0 };
        d.push_str(&format!(
            " A {rx:.3} {ry:.3} {rot_deg:.3} 0 {sweep} {:.3} {:.3}",
            end.0, end.1
        ));
        prev = end;
        prev_t = t;
    }
    d
}

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

#[cfg_attr(not(feature = "raster"), allow(dead_code))]
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
    tint_ramp_k(color, k)
}

/// Full-material-colour ramp for [`RenderStyle::Shaded`]: the material colour
/// drives every stop outright (k = 1), rescaled to each stop's luminance so
/// the lighting ladder is preserved. Achromatic materials (steel, aluminium)
/// therefore render as true greys instead of staying in the navy family.
fn shaded_ramp(color: [f64; 3]) -> [[u8; 3]; 4] {
    tint_ramp_k(color, 1.0)
}

/// Shared ramp construction: blend the navy [`RAMP`] toward the material
/// colour (rescaled per-stop to the stop's luminance) by strength `k`.
fn tint_ramp_k(color: [f64; 3], k: f64) -> [[u8; 3]; 4] {
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
    emphasis: Emphasis,
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

    /// Per-sample visibility test at a final-SVG point with camera depth
    /// `d`: visible when its depth is within a (gradient-adaptive) bias of
    /// the buffer, or it runs over open background. Returns
    /// `(visible, over_bg)`. Shared by the segment walk ([`Self::clip_edge`])
    /// and the exact-arc walk.
    fn sample_visible(&self, p: (f64, f64), d: f64, bias: f64) -> (bool, bool) {
        let bxp = p.0 * self.scale;
        let byp = p.1 * self.scale;
        let ix = bxp.floor() as i64;
        let iy = byp.floor() as i64;
        let z = self.at(ix, iy);
        if z == f64::NEG_INFINITY {
            return (true, true); // over background — nothing occludes it
        }
        let mut over_bg = false;
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
        (d >= z - (bias + delta), over_bg)
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
                    edge_frac: if len > 0.0 { len_cells / len } else { 1.0 },
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
            let (visible, over_bg) =
                self.sample_visible((bxp / self.scale, byp / self.scale), d, bias);
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

/// Link segments that share endpoints into open polylines.
///
/// Hidden-line removal emits one segment per mesh edge, so a tessellated
/// curve arrives as hundreds of separate two-point pieces. Written out as
/// individual `<line>` elements that is *visually* fine for a solid stroke
/// but wrong for a dashed one: SVG restarts `stroke-dasharray` at every
/// element, so a segment shorter than one dash period renders entirely
/// inside its first "on" dash. Every dashed hidden line along a curve
/// therefore drew solid — indistinguishable from real visible linework.
/// Chaining lets the dash phase run along the whole curve.
fn chain_segments(segs: &[Seg]) -> Vec<Vec<(f64, f64)>> {
    /// Endpoint match tolerance, in SVG user units. Shared vertices are
    /// computed from the same projected point, so they agree to well
    /// within this; it only absorbs the rounding in the walk's endpoints.
    const EPS: f64 = 1e-3;
    let key = |p: (f64, f64)| ((p.0 / EPS).round() as i64, (p.1 / EPS).round() as i64);
    // Chain from a canonical ordering. Edges reach here in the iteration
    // order of a `HashMap`, which varies run to run; that was invisible
    // while every segment became its own `<line>`, but which segment starts
    // a chain decides how the curve is partitioned, so without this the
    // same document renders to different (equivalent) SVG bytes each time.
    // Normalising each segment's endpoints makes the sort a total order.
    let mut segs: Vec<Seg> = segs
        .iter()
        .map(|&(a, b)| if key(a) <= key(b) { (a, b) } else { (b, a) })
        .collect();
    segs.sort_by(|p, q| {
        (key(p.0), key(p.1))
            .partial_cmp(&(key(q.0), key(q.1)))
            .expect("integer keys are totally ordered")
    });
    let segs = &segs[..];
    let mut ends: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, &(a, b)) in segs.iter().enumerate() {
        ends.entry(key(a)).or_default().push(i);
        ends.entry(key(b)).or_default().push(i);
    }
    let mut used = vec![false; segs.len()];
    let mut out: Vec<Vec<(f64, f64)>> = Vec::new();
    for start in 0..segs.len() {
        if used[start] {
            continue;
        }
        used[start] = true;
        let (a, b) = segs[start];
        let mut chain = vec![a, b];
        // Grow from the far end, reverse, then grow from the near end, so
        // a chain started mid-curve still collects both directions.
        for pass in 0..2 {
            while let Some(&tip) = chain.last() {
                let Some(cands) = ends.get(&key(tip)) else {
                    break;
                };
                let Some(&next) = cands.iter().find(|&&i| !used[i]) else {
                    break;
                };
                used[next] = true;
                let (na, nb) = segs[next];
                chain.push(if key(na) == key(tip) { nb } else { na });
            }
            if pass == 0 {
                chain.reverse();
            }
        }
        out.push(chain);
    }
    out
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
    /// Fraction of the parent edge's projected length this span covers.
    /// `1.0` means the whole edge fell on one side of the depth test;
    /// a small value means the edge was chopped, which is what
    /// z-fighting on a coplanar surface looks like.
    edge_frac: f64,
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
    render_svg_str_view_opts(raw_vcad, scale, view, false, &RenderAnnotations::default())
}

/// Render raw `.vcad` document JSON to a self-contained SVG from `view`,
/// optionally omitting the opaque paper background for a transparent render
/// and/or overlaying engineering annotations.
///
/// When `transparent` is true the vellum ground rect is skipped, so the
/// SVG composites cleanly over any background; the soft contact shadow is
/// still emitted (it is already semi-transparent). With
/// [`RenderAnnotations::default()`] the output is byte-identical to an
/// annotation-free render.
pub fn render_svg_str_view_opts(
    raw_vcad: &str,
    scale: f64,
    view: View,
    transparent: bool,
    annotations: &RenderAnnotations,
) -> Result<String, String> {
    render_svg_str_opts(
        raw_vcad,
        scale,
        &SvgOptions {
            view,
            transparent,
            annotations: *annotations,
            ..Default::default()
        },
    )
}

/// Render raw `.vcad` document JSON to an SVG from `view`, optionally
/// sectioned (cutaway) by `section` and/or overlaid with `annotations`.
/// Material on the camera's side of the plane is boolean-subtracted before
/// tessellation; exposed cut faces are drawn with a 45° drafting hatch. A
/// solid whose boolean fails is rendered uncut (noted on stderr) rather than
/// failing the render.
pub fn render_svg_str_section(
    raw_vcad: &str,
    scale: f64,
    view: View,
    transparent: bool,
    section: Option<SectionPlane>,
    annotations: &RenderAnnotations,
) -> Result<String, String> {
    render_svg_str_opts(
        raw_vcad,
        scale,
        &SvgOptions {
            view,
            transparent,
            section,
            annotations: *annotations,
            ..Default::default()
        },
    )
}

/// Options for the SVG render path ([`render_svg_str_opts`]).
#[derive(Debug, Clone, Default)]
pub struct SvgOptions {
    /// Camera orientation.
    pub view: View,
    /// Omit the opaque paper background rect.
    pub transparent: bool,
    /// Emit BRep-exact linework where available: circular model edges
    /// (cylinder/cone rims) and sphere view outlines become mathematically
    /// exact SVG elliptical arcs instead of tessellated polylines, so they
    /// stay smooth at any zoom. Curves the extractor doesn't recognise
    /// (tori, NURBS, boolean intersection seams) fall back to polylines;
    /// fills and hidden-line removal still use the tessellation.
    pub exact_edges: bool,
    /// Section (cutaway) plane. Material on the camera's side is
    /// boolean-subtracted before tessellation and exposed cut faces get a
    /// 45° drafting hatch. `None` renders the whole model.
    pub section: Option<SectionPlane>,
    /// Opt-in engineering annotation overlays (axes gizmo, part labels,
    /// bounding-box dimensions). [`RenderAnnotations::default()`] draws none.
    pub annotations: RenderAnnotations,
    /// Frame the render on this part instead of the whole document. Matched
    /// case-insensitively against root-part node names, assembly instance
    /// ids/names, and part-definition ids. Geometry outside the focused
    /// part's projected bounds is cropped by the viewport. `None` frames the
    /// whole document.
    pub focus: Option<String>,
    /// Changed-part highlight set. Selects parts by root node id (the
    /// `part_id` a mutation diff reports), node name, or assembly instance
    /// id/name. When non-empty, matched parts keep their full material
    /// colour and gain a brand-orange accent outline while every other part
    /// is ghosted toward the paper — the "what did my edit just touch"
    /// view. Empty renders normally; a non-empty set matching no part is an
    /// error (never a silently unhighlighted render).
    pub highlight: Vec<String>,
    /// Shading style: [`RenderStyle::Drafting`] (default) keeps material
    /// colours in the disciplined navy tonal family;
    /// [`RenderStyle::Shaded`] renders each part in its full material
    /// colour (luminance-laddered so lighting is preserved).
    pub style: RenderStyle,
}

/// Shading style for the SVG render path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RenderStyle {
    /// Drafting look: material colours tinted toward the navy tonal family
    /// in proportion to their saturation (the historical default).
    #[default]
    Drafting,
    /// Full material colour: parts render in their assigned material colour
    /// with the same Lambertian shading ladder.
    Shaded,
}

impl std::str::FromStr for RenderStyle {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "" | "drafting" => Ok(RenderStyle::Drafting),
            "shaded" => Ok(RenderStyle::Shaded),
            other => Err(format!(
                "unknown render style '{other}' — supported: 'drafting', 'shaded'"
            )),
        }
    }
}

/// Render raw `.vcad` document JSON to a self-contained SVG with full
/// [`SvgOptions`] control. The other `render_svg_str*` entry points are
/// thin wrappers over this.
pub fn render_svg_str_opts(
    raw_vcad: &str,
    scale: f64,
    opts: &SvgOptions,
) -> Result<String, String> {
    let mut scene = evaluate_vcad(raw_vcad)?;
    if let Some(plane) = opts.section {
        scene = apply_section(scene, plane, opts.view);
    }
    let accents: Vec<bool> = scene
        .iter()
        .map(|s| {
            opts.highlight
                .iter()
                .any(|h| *h == s.id || s.name.as_deref() == Some(h.as_str()))
        })
        .collect();
    if !opts.highlight.is_empty() && !accents.iter().any(|&a| a) {
        let known: Vec<String> = scene
            .iter()
            .map(|s| match &s.name {
                Some(n) => format!("{} ({n})", s.id),
                None => s.id.clone(),
            })
            .collect();
        return Err(format!(
            "highlight matched no parts: wanted [{}], document has [{}]",
            opts.highlight.join(", "),
            known.join(", "),
        ));
    }
    // Focus mask (per-solid) frames the viewport on the matched part(s).
    let focus_sel = opts
        .focus
        .as_deref()
        .map(|f| focus_mask(&scene, f))
        .transpose()?;
    let solids: Vec<Solid> = scene.iter().map(|s| s.solid.clone()).collect();
    let tints: Vec<Option<[f64; 3]>> = scene.iter().map(|s| s.tint).collect();
    let names: Vec<Option<String>> = scene.iter().map(|s| s.name.clone()).collect();
    render_svg_impl(
        &solids,
        &tints,
        &names,
        &accents,
        focus_sel.as_deref(),
        scale,
        opts,
    )
}

/// Camera options for [`render_svg_str_camera`]: an arbitrary [`View`]
/// (including [`View::Orbit`]), transparency, an optional part focus, and
/// engineering annotation overlays. A thin convenience over [`SvgOptions`]
/// for the WASM/CLI "agent eyes" path.
#[derive(Debug, Clone, Default)]
pub struct CameraOptions {
    /// Camera orientation.
    pub view: View,
    /// Omit the opaque paper background.
    pub transparent: bool,
    /// Frame the render on this part instead of the whole document.
    /// Matched case-insensitively against root-part node names, assembly
    /// instance ids/names, and part-definition ids. Geometry outside the
    /// focused part's projected bounds is cropped by the viewport.
    pub focus: Option<String>,
    /// Opt-in annotation overlays (axes gizmo, part labels, dimensions).
    pub annotations: RenderAnnotations,
}

/// Render raw `.vcad` document JSON to a self-contained SVG with full
/// camera control — arbitrary orbit views, part-focused framing, and
/// annotation overlays. Delegates to [`render_svg_str_opts`].
pub fn render_svg_str_camera(
    raw_vcad: &str,
    scale: f64,
    opts: &CameraOptions,
) -> Result<String, String> {
    render_svg_str_opts(
        raw_vcad,
        scale,
        &SvgOptions {
            view: opts.view,
            transparent: opts.transparent,
            focus: opts.focus.clone(),
            annotations: opts.annotations,
            ..Default::default()
        },
    )
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
    let names = vec![None; solids.len()];
    let accents = vec![false; solids.len()];
    render_svg_impl(
        solids,
        &tints,
        &names,
        &accents,
        None,
        scale,
        &SvgOptions {
            view,
            ..Default::default()
        },
    )
}

/// Shared SVG renderer; `tints[i]` optionally tints solid `i`'s ramp and
/// `names[i]` labels it when the `labels` annotation is on. When any
/// `accents[i]` is set, solid `i` keeps its full ramp and gains an
/// [`ACCENT`] outline while the rest are ghosted toward [`PAPER`].
/// `opts.section` (already applied to the solids by the caller) only drives
/// cut-face detection here, so exposed section faces get the hatch fill.
/// When `focus` is given (a per-solid mask), the viewport is framed on the
/// focused solids' projected bounds; everything else is still drawn but
/// cropped to that frame.
#[allow(clippy::too_many_arguments)]
fn render_svg_impl(
    solids: &[Solid],
    tints: &[Option<[f64; 3]>],
    names: &[Option<String>],
    accents: &[bool],
    focus: Option<&[bool]>,
    scale: f64,
    opts: &SvgOptions,
) -> Result<String, String> {
    if solids.is_empty() {
        return Err("no solids produced".to_string());
    }
    let view = opts.view;
    let transparent = opts.transparent;
    let section = opts.section;
    let annos = &opts.annotations;

    let cam = view.cam();
    let right = view.right();
    let down = view.down();
    let light = normalize(LIGHT);
    let project = |p: [f64; 3]| -> (f64, f64) { (dot(p, right) * scale, dot(p, down) * scale) };

    let arts = build_artifacts(
        solids,
        tints,
        accents,
        cam,
        TESSELLATION_SEGMENTS,
        &EdgeRules::svg(),
        section,
        opts.style,
    );

    // First pass: screen-space bbox + 3D bbox (for the depth-cue bias),
    // over every projected vertex of every kept artifact.
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    let mut lo3 = [f64::INFINITY; 3];
    let mut hi3 = [f64::NEG_INFINITY; 3];
    let mut projected: Vec<Vec<(f64, f64)>> = Vec::with_capacity(arts.len());
    for art in &arts {
        // With a focus mask, only the focused solids' vertices drive the
        // screen frame; everything still projects (and later crops).
        let framed = focus.is_none_or(|m| m.get(art.src).copied().unwrap_or(false));
        let mut pv = Vec::with_capacity(art.verts.len());
        for v in &art.verts {
            let (x, y) = project(*v);
            if framed {
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
            for i in 0..3 {
                lo3[i] = lo3[i].min(v[i]);
                hi3[i] = hi3[i].max(v[i]);
            }
            pv.push((x, y));
        }
        projected.push(pv);
    }

    if !min_x.is_finite() || !max_x.is_finite() {
        return Err(if focus.is_some() {
            "focused part produced no geometry".to_string()
        } else {
            "no visible polygons after culling".to_string()
        });
    }
    // Annotations live outside the model silhouette; reserve margin for
    // them. With no annotations `pad == PADDING_PX` and the output is
    // byte-identical to the historical render.
    let pad = PADDING_PX + if annos.any() { ANNOT_MARGIN_PX } else { 0.0 };
    let w = (max_x - min_x) + 2.0 * pad;
    let h = (max_y - min_y) + 2.0 * pad;
    // Final SVG coordinate from a projected screen point.
    let fin = |(x, y): (f64, f64)| -> (f64, f64) { (x - min_x + pad, y - min_y + pad) };

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

    // BRep-exact linework (opt-in): analytic circles recovered from each
    // solid's BRep, plus the mask of mesh edges they replace. Extraction
    // failures are impossible by construction (no BRep → no candidates →
    // empty mask), so unrecognised geometry silently keeps its polylines.
    let exact_curves: Vec<Option<exact::ExactCurves>> = arts
        .iter()
        .map(|art| {
            opts.exact_edges.then(|| {
                exact::extract(
                    &solids[art.src],
                    &art.verts,
                    &art.edges,
                    cam,
                    TESSELLATION_SEGMENTS,
                )
            })
        })
        .collect();

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
            if art.cut[ti] {
                // Exposed section-cut face: drafting convention says
                // cross-hatch, not shade. Pattern defined once in <defs>.
                polys.push(ProjPoly {
                    s: ps,
                    fill: "url(#section-hatch)".to_string(),
                    stroke: HATCH_BG.to_string(),
                    depth: (vd[0] + vd[1] + vd[2]) / 3.0,
                });
                continue;
            }
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
        for (ei, &(a, b, kind)) in art.edges.iter().enumerate() {
            // Skip polylines an exact arc replaces.
            if let Some(Some(ex)) = exact_curves.get(ai).map(|e| e.as_ref()) {
                if ex.suppressed[ei] {
                    continue;
                }
            }
            edges.push(ProjEdge {
                a: fin(proj[a]),
                da: dot(art.verts[a], cam),
                b: fin(proj[b]),
                db: dot(art.verts[b], cam),
                kind,
                emphasis: art.emphasis,
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
    // A hidden (occluded) *fragment* of an edge must be at least this long
    // to be drawn dashed — shorter ones are z-fighting noise, not real
    // occluded edges.
    const HIDDEN_MIN_CELLS: f64 = 6.0;
    // …but an edge that is hidden along essentially its whole length is not
    // a fragment, however short it projects. Measuring only absolute length
    // silently deleted the middle of long hidden curves: a tessellated rim
    // arrives as hundreds of separate short edges, each fully occluded and
    // each yielding exactly one span, and those spans foreshorten as the
    // curve turns away from the viewer. On a Ø120 disc the bottom rim's
    // hidden half ran 130°–320°, but per-segment length fell under 6 cells
    // outside 190°–260°, so the ends — the part that reads as "this is the
    // bottom of the disc", where the dashes meet the visible silhouette at
    // the tangent points — were dropped and the surviving 61° floated in
    // the middle of the part looking like a stray curve belonging to
    // nothing.
    const HIDDEN_WHOLE_EDGE_FRAC: f64 = 0.9;
    let mut crease_lines: Vec<Seg> = Vec::new();
    let mut outline_lines: Vec<Seg> = Vec::new();
    let mut hidden_lines: Vec<Seg> = Vec::new();
    // Highlight-pass buckets: ghosted parts' visible linework fades with
    // their fills; accented parts' outlines are re-stroked in brand orange
    // on top of everything else.
    let mut ghost_crease_lines: Vec<Seg> = Vec::new();
    let mut ghost_outline_lines: Vec<Seg> = Vec::new();
    let mut accent_outline_lines: Vec<Seg> = Vec::new();
    for e in &edges {
        let clip = zbuf.clip_edge(e.a, e.da, e.b, e.db, bias);
        let dst = match (e.kind, e.emphasis) {
            (EdgeKind::Crease, Emphasis::Ghost) => &mut ghost_crease_lines,
            (EdgeKind::Crease, _) => &mut crease_lines,
            (EdgeKind::Outline | EdgeKind::Smooth, Emphasis::Ghost) => &mut ghost_outline_lines,
            (EdgeKind::Outline | EdgeKind::Smooth, Emphasis::Accent) => &mut accent_outline_lines,
            (EdgeKind::Outline | EdgeKind::Smooth, Emphasis::Normal) => &mut outline_lines,
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
        // Ghosted parts drop their dashed hidden lines entirely — they are
        // context, not the subject.
        if matches!(e.kind, EdgeKind::Outline | EdgeKind::Crease) && e.emphasis != Emphasis::Ghost {
            for span in &clip.hidden {
                if span.len_cells >= HIDDEN_MIN_CELLS || span.edge_frac >= HIDDEN_WHOLE_EDGE_FRAC {
                    hidden_lines.push((span.a, span.b));
                }
            }
        }
    }

    // BRep-exact arcs: walk each analytic arc against the same depth
    // buffer and emit the surviving parameter runs as exact SVG
    // elliptical-arc paths, styled identically to the lines they replace.
    let mut outline_arcs: Vec<String> = Vec::new();
    let mut crease_arcs: Vec<String> = Vec::new();
    let mut hidden_arcs: Vec<String> = Vec::new();
    for ex in exact_curves.iter().flatten() {
        for span in &ex.arcs {
            let c = &ex.circles[span.circle];
            // Screen-space conjugate radius vectors of the projected
            // ellipse (an orthographic projection maps the circle's
            // parametric form to `P(center) + A·cosθ + B·sinθ`).
            let a2 = (
                dot(c.u, right) * scale * c.radius,
                dot(c.u, down) * scale * c.radius,
            );
            let b2 = (
                dot(c.v, right) * scale * c.radius,
                dot(c.v, down) * scale * c.radius,
            );
            let p_screen = |th: f64| fin(project(c.point(th)));
            let conj = ((a2.0 * a2.0 + a2.1 * a2.1) + (b2.0 * b2.0 + b2.1 * b2.1)).sqrt();
            // ~1 depth-buffer cell per visibility sample along the arc.
            let steps =
                (((span.end - span.start) * conj * zbuf.scale).ceil() as usize).clamp(16, 4096);
            // Walk visibility runs over the parameter interval.
            let mut run_start = span.start;
            let mut run_cells = 0.0f64;
            let mut run_bg = false;
            let mut state: Option<bool> = None;
            let mut prev_pt = p_screen(span.start);
            let mut flush = |vis: bool, t0: f64, t1: f64, cells: f64, over_bg: bool| {
                if t1 <= t0 {
                    return;
                }
                if vis {
                    let keep = match span.kind {
                        EdgeKind::Smooth => over_bg || cells >= SMOOTH_INTERIOR_MIN_CELLS,
                        EdgeKind::Outline | EdgeKind::Crease => true,
                    };
                    if keep {
                        let d = arc_path_d(a2, b2, &p_screen, t0, t1);
                        match span.kind {
                            EdgeKind::Crease => crease_arcs.push(d),
                            _ => outline_arcs.push(d),
                        }
                    }
                } else if matches!(span.kind, EdgeKind::Outline | EdgeKind::Crease)
                    && cells >= HIDDEN_MIN_CELLS
                {
                    hidden_arcs.push(arc_path_d(a2, b2, &p_screen, t0, t1));
                }
            };
            for i in 0..=steps {
                let t = span.start + (span.end - span.start) * i as f64 / steps as f64;
                let p3 = c.point(t);
                let spt = p_screen(t);
                let (vis, over_bg) = zbuf.sample_visible(spt, dot(p3, cam), bias);
                let step_cells =
                    ((spt.0 - prev_pt.0).powi(2) + (spt.1 - prev_pt.1).powi(2)).sqrt() * zbuf.scale;
                prev_pt = spt;
                match state {
                    Some(prev) if prev == vis => {
                        run_cells += step_cells;
                        run_bg |= over_bg;
                    }
                    Some(prev) => {
                        flush(prev, run_start, t, run_cells, run_bg);
                        run_start = t;
                        run_cells = 0.0;
                        run_bg = over_bg;
                        state = Some(vis);
                    }
                    None => {
                        run_bg = over_bg;
                        state = Some(vis);
                    }
                }
            }
            if let Some(vis) = state {
                flush(vis, run_start, span.end, run_cells, run_bg);
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
    // 45° cross-hatch pattern for exposed section-cut faces (drafting
    // convention). Only emitted when a section plane is active.
    let hatch_def = if section.is_some() {
        format!(
            r#"<pattern id="section-hatch" patternUnits="userSpaceOnUse" width="{sp:.2}" height="{sp:.2}" patternTransform="rotate(45)"><rect width="{sp:.2}" height="{sp:.2}" fill="{HATCH_BG}"/><line x1="0" y1="0" x2="0" y2="{sp:.2}" stroke="{INK}" stroke-width="{HATCH_STROKE_PX}"/></pattern>"#,
            sp = HATCH_SPACING_PX,
        )
    } else {
        String::new()
    };
    out.push_str(&format!(r#"<defs>{hatch_def}{gradients}</defs>"#));

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
                      arcs: &[String],
                      stroke: &str,
                      width: f64,
                      opacity: f64,
                      dasharray: Option<f64>| {
        if lines.is_empty() && arcs.is_empty() {
            return;
        }
        let dash_attr = match dasharray {
            Some(d) => format!(r#" stroke-dasharray="{d:.2} {:.2}""#, d * 0.6),
            None => String::new(),
        };
        out.push_str(&format!(
                r#"<g stroke="{stroke}" stroke-width="{width}" stroke-linecap="round" stroke-linejoin="round" fill="none" opacity="{opacity}"{dash_attr}>"#
            ));
        for chain in chain_segments(lines) {
            if let [a, b] = chain[..] {
                out.push_str(&format!(
                    r#"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}"/>"#,
                    a.0, a.1, b.0, b.1,
                ));
                continue;
            }
            out.push_str(r#"<polyline points=""#);
            for (i, p) in chain.iter().enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                out.push_str(&format!("{:.2},{:.2}", p.0, p.1));
            }
            out.push_str(r#""/>"#);
        }
        for d in arcs {
            out.push_str(&format!(r#"<path d="{d}"/>"#));
        }
        out.push_str("</g>");
    };
    let no_arcs: [String; 0] = [];
    emit_lines(
        &mut out,
        &hidden_lines,
        &hidden_arcs,
        INK,
        STROKE_HIDDEN_PX,
        0.5,
        Some(dash),
    );
    // Ghosted parts' faded linework, under the normal creases/outlines.
    emit_lines(
        &mut out,
        &ghost_crease_lines,
        &no_arcs,
        INK,
        STROKE_CREASE_PX,
        GHOST_LINE_OPACITY,
        None,
    );
    emit_lines(
        &mut out,
        &ghost_outline_lines,
        &no_arcs,
        INK,
        STROKE_OUTLINE_PX,
        GHOST_LINE_OPACITY,
        None,
    );
    emit_lines(
        &mut out,
        &crease_lines,
        &crease_arcs,
        INK,
        STROKE_CREASE_PX,
        1.0,
        None,
    );
    emit_lines(
        &mut out,
        &outline_lines,
        &outline_arcs,
        INK,
        STROKE_OUTLINE_PX,
        1.0,
        None,
    );
    // Accent outlines last — the brand-orange "this is what changed" stroke
    // sits on top of every other line.
    emit_lines(
        &mut out,
        &accent_outline_lines,
        &no_arcs,
        ACCENT,
        STROKE_ACCENT_PX,
        1.0,
        None,
    );

    // Opt-in engineering overlays, drawn over the linework.
    if annos.any() {
        let world_to_fin = |p: [f64; 3]| fin(project(p));
        if annos.dims {
            emit_svg_dims(&mut out, lo3, hi3, w, h, &world_to_fin);
        }
        if annos.labels {
            for (ai, art) in arts.iter().enumerate() {
                let Some(name) = names.get(art.src).and_then(|n| n.as_deref()) else {
                    continue;
                };
                let mut lo = (f64::INFINITY, f64::INFINITY);
                let mut hi = (f64::NEG_INFINITY, f64::NEG_INFINITY);
                for &(x, y) in &projected[ai] {
                    lo = (lo.0.min(x), lo.1.min(y));
                    hi = (hi.0.max(x), hi.1.max(y));
                }
                if !lo.0.is_finite() {
                    continue;
                }
                let anchor = fin(((lo.0 + hi.0) / 2.0, (lo.1 + hi.1) / 2.0));
                emit_svg_label(&mut out, name, anchor, w);
            }
        }
        if annos.axes {
            emit_svg_axes(&mut out, view, h);
        }
    }

    out.push_str("</svg>");
    Ok(out)
}

/// Emit the part-label annotation: an anchor dot at the part's projected
/// bbox centre, a leader line, and the name with a paper-coloured halo so
/// it stays legible over fills.
fn emit_svg_label(out: &mut String, name: &str, anchor: (f64, f64), w: f64) {
    // Lead up-right by default; flip left when close to the right edge.
    let dir = if anchor.0 + 70.0 > w { -1.0 } else { 1.0 };
    let elbow = (anchor.0 + 18.0 * dir, anchor.1 - 18.0);
    let (ta, tx) = if dir > 0.0 {
        ("start", elbow.0 + 3.0)
    } else {
        ("end", elbow.0 - 3.0)
    };
    out.push_str(&format!(
        r#"<g class="annot-label"><circle cx="{:.2}" cy="{:.2}" r="1.6" fill="{INK}"/><line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke="{INK}" stroke-width="0.6"/><text x="{tx:.2}" y="{:.2}" font-family="JetBrains Mono, Menlo, monospace" font-size="10" fill="{INK}" text-anchor="{ta}" paint-order="stroke" stroke="{PAPER}" stroke-width="3">{}</text></g>"#,
        anchor.0,
        anchor.1,
        anchor.0,
        anchor.1,
        elbow.0,
        elbow.1,
        elbow.1 + 3.0,
        xml_escape(name),
    ));
}

/// Emit drafting-style overall bounding-box dimensions: extension lines
/// from the bbox corners, a dimension line offset outside the silhouette,
/// and the mm value at its midpoint.
fn emit_svg_dims(
    out: &mut String,
    lo3: [f64; 3],
    hi3: [f64; 3],
    w: f64,
    h: f64,
    world_to_fin: &dyn Fn([f64; 3]) -> (f64, f64),
) {
    out.push_str(r#"<g class="annot-dims">"#);
    for dim in bbox_dim_specs(lo3, hi3) {
        let pa = world_to_fin(dim.a);
        let pb = world_to_fin(dim.b);
        let (dx, dy) = (pb.0 - pa.0, pb.1 - pa.1);
        let len = (dx * dx + dy * dy).sqrt();
        if len < 6.0 {
            continue; // axis is parallel to the camera in this view
        }
        // Perpendicular, pointing away from the canvas centre.
        let mut n = (-dy / len, dx / len);
        let mid = ((pa.0 + pb.0) / 2.0, (pa.1 + pb.1) / 2.0);
        if n.0 * (mid.0 - w / 2.0) + n.1 * (mid.1 - h / 2.0) < 0.0 {
            n = (-n.0, -n.1);
        }
        let off = DIM_OFFSET_PX;
        let (a1, a2) = (
            (pa.0 + n.0 * off, pa.1 + n.1 * off),
            (pb.0 + n.0 * off, pb.1 + n.1 * off),
        );
        // Extension lines (slightly past the dimension line).
        for (p, e) in [(pa, a1), (pb, a2)] {
            out.push_str(&format!(
                r#"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke="{INK}" stroke-width="0.45" opacity="0.75"/>"#,
                p.0,
                p.1,
                e.0 + n.0 * 4.0,
                e.1 + n.1 * 4.0,
            ));
        }
        // Dimension line with drafting tick marks at both ends.
        out.push_str(&format!(
            r#"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke="{INK}" stroke-width="0.6"/>"#,
            a1.0, a1.1, a2.0, a2.1,
        ));
        let (ux, uy) = (dx / len, dy / len);
        for p in [a1, a2] {
            let t = ((ux - n.0) * 3.0, (uy - n.1) * 3.0);
            out.push_str(&format!(
                r#"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke="{INK}" stroke-width="0.8"/>"#,
                p.0 - t.0,
                p.1 - t.1,
                p.0 + t.0,
                p.1 + t.1,
            ));
        }
        let text = (mid.0 + n.0 * (off + 10.0), mid.1 + n.1 * (off + 10.0));
        out.push_str(&format!(
            r#"<text x="{:.2}" y="{:.2}" font-family="JetBrains Mono, Menlo, monospace" font-size="9" fill="{INK}" text-anchor="middle" dominant-baseline="middle" paint-order="stroke" stroke="{PAPER}" stroke-width="3">{}</text>"#,
            text.0,
            text.1,
            xml_escape(&dim.label),
        ));
    }
    out.push_str("</g>");
}

/// Emit the origin/axes gizmo in the lower-left corner: world X/Y/Z arrows
/// projected through the current view (kernel is Z-up). An axis parallel to
/// the camera projects to nothing and is skipped.
fn emit_svg_axes(out: &mut String, view: View, h: f64) {
    let right = view.right();
    let down = view.down();
    // Inset far enough that an arrow pointing toward the corner (plus its
    // label) stays on canvas.
    let origin = (AXIS_LEN_PX + 14.0, h - AXIS_LEN_PX - 14.0);
    out.push_str(r#"<g class="annot-axes">"#);
    for i in 0..3 {
        let mut e = [0.0; 3];
        e[i] = 1.0;
        let d = (dot(e, right), dot(e, down));
        let m = (d.0 * d.0 + d.1 * d.1).sqrt();
        if m < 1e-6 {
            continue;
        }
        let (ux, uy) = (d.0 / m, d.1 / m);
        let tip = (origin.0 + ux * AXIS_LEN_PX, origin.1 + uy * AXIS_LEN_PX);
        let color = AXIS_COLORS[i];
        out.push_str(&format!(
            r#"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke="{color}" stroke-width="1.4" stroke-linecap="round"/>"#,
            origin.0, origin.1, tip.0, tip.1,
        ));
        // Arrowhead: two barbs swept back from the tip.
        for s in [0.45f64, -0.45] {
            let (bx, by) = (-ux * s.cos() - uy * s.sin(), ux * s.sin() - uy * s.cos());
            out.push_str(&format!(
                r#"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke="{color}" stroke-width="1.4" stroke-linecap="round"/>"#,
                tip.0,
                tip.1,
                tip.0 + bx * 5.0,
                tip.1 + by * 5.0,
            ));
        }
        out.push_str(&format!(
            r#"<text x="{:.2}" y="{:.2}" font-family="JetBrains Mono, Menlo, monospace" font-size="9" fill="{color}" text-anchor="middle" dominant-baseline="middle" paint-order="stroke" stroke="{PAPER}" stroke-width="2.5">{}</text>"#,
            tip.0 + ux * 8.0,
            tip.1 + uy * 8.0,
            AXIS_NAMES[i],
        ));
    }
    out.push_str("</g>");
}

// ─── public API: raster JPEG ──────────────────────────────────────────────

#[cfg(feature = "raster")]
pub use raster::{
    render_jpeg_solids, render_jpeg_str, render_png_solids, render_png_str, FixedFraming,
    FramingBuilder, RasterOptions,
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
    /// Also the photoreal path's fixed count — see
    /// `photoreal::MESH_SEGMENTS`.
    pub(crate) const RASTER_SEGMENTS_HIRES: u32 = 128;

    /// The segment count a raster canvas of `size_px` tessellates at.
    pub(super) fn segments_for(size_px: u32) -> u32 {
        if size_px >= HIRES_THRESHOLD_PX {
            RASTER_SEGMENTS_HIRES
        } else {
            RASTER_SEGMENTS
        }
    }

    /// Looser coplanar tolerance than the SVG path: at 64 segments,
    /// adjacent cylinder facets differ by 5.6°, which the SVG's ~4.5°
    /// threshold would draw as stripes down every curved face. Hiding
    /// everything under ~10° keeps real creases (chamfers, fillet rims)
    /// while letting tessellation facets blend together.
    const RASTER_COPLANAR_DOT_TOL: f64 = 0.985; // cos(~10°)

    /// Matte, neutral background per the mecheval capture rules.
    pub(crate) const BACKGROUND: [u8; 3] = [244, 243, 241];

    /// Output canvas dimensions in pixels. Row-major, `w` pixels per row.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) struct Canvas {
        pub w: usize,
        pub h: usize,
    }

    impl Canvas {
        /// Index of pixel `(x, y)` in a row-major buffer.
        pub(crate) fn idx(&self, x: usize, y: usize) -> usize {
            y * self.w + x
        }
        /// Pixel count.
        pub(crate) fn len(&self) -> usize {
            self.w * self.h
        }
        /// True when `(x, y)` is on-canvas.
        fn contains(&self, x: i64, y: i64) -> bool {
            x >= 0 && y >= 0 && (x as usize) < self.w && (y as usize) < self.h
        }
        /// Shorter side, for size-relative linework weights.
        fn min_side(&self) -> usize {
            self.w.min(self.h)
        }
    }

    /// A rasterized frame: RGB pixels plus a per-pixel coverage mask (255
    /// where geometry, an edge stroke, or an annotation was drawn, 0 over
    /// untouched background), and the canvas they were drawn on.
    pub(crate) struct Frame {
        pub rgb: Vec<u8>,
        pub mask: Vec<u8>,
        pub canvas: Canvas,
    }

    impl Frame {
        /// Crop to the coverage mask's bounding box plus `margin` pixels.
        /// `None` (or an empty mask) returns the frame untouched.
        pub(crate) fn trimmed(self, margin: Option<u32>) -> Frame {
            let Some(margin) = margin else {
                return self;
            };
            let (mut x0, mut y0) = (self.canvas.w, self.canvas.h);
            let (mut x1, mut y1) = (0usize, 0usize);
            for y in 0..self.canvas.h {
                for x in 0..self.canvas.w {
                    if self.mask[self.canvas.idx(x, y)] != 0 {
                        x0 = x0.min(x);
                        y0 = y0.min(y);
                        x1 = x1.max(x);
                        y1 = y1.max(y);
                    }
                }
            }
            if x0 > x1 || y0 > y1 {
                return self; // nothing drawn — leave the canvas alone
            }
            let m = margin as usize;
            let x0 = x0.saturating_sub(m);
            let y0 = y0.saturating_sub(m);
            let x1 = (x1 + m).min(self.canvas.w - 1);
            let y1 = (y1 + m).min(self.canvas.h - 1);
            let out = Canvas {
                w: x1 - x0 + 1,
                h: y1 - y0 + 1,
            };
            let mut rgb = Vec::with_capacity(out.len() * 3);
            let mut mask = Vec::with_capacity(out.len());
            for y in y0..=y1 {
                let row = self.canvas.idx(x0, y);
                rgb.extend_from_slice(&self.rgb[row * 3..(row + out.w) * 3]);
                mask.extend_from_slice(&self.mask[row..row + out.w]);
            }
            Frame {
                rgb,
                mask,
                canvas: out,
            }
        }
    }

    /// A camera framing computed once and reused for every frame of a
    /// sequence.
    ///
    /// A single still frames itself: the projected bounds of what it draws
    /// pick the canvas, the scale, and the depth-cue range. An *animation*
    /// cannot do that per frame — the subject's projected bounds change as
    /// the machine moves, so each frame would get its own canvas size and
    /// its own scale, and the assembly would swim about (and, with
    /// `auto_aspect`, the PNGs would not even agree on their dimensions,
    /// which no muxer will accept). Computing this once over the union of
    /// every posed frame and handing the same value to all of them pins the
    /// camera: identical canvas, identical scale, identical shading ladder.
    #[derive(Debug, Clone, Copy)]
    pub struct FixedFraming {
        /// Screen-plane (mm) lower bound, in the view's right/down basis.
        pub screen_min: [f64; 2],
        /// Screen-plane (mm) upper bound.
        pub screen_max: [f64; 2],
        /// 3D bounding-box diagonal (mm) — drives the hidden-line depth bias.
        pub diag: f64,
        /// Depth (toward the camera, mm) range for the depth cue.
        pub depth: (f64, f64),
    }

    /// Accumulates world points into a [`FixedFraming`]. Callers feed it
    /// every vertex of every pose; the projection math here is the same the
    /// raster path uses, so a fixed framing frames exactly as a still would
    /// have framed that union.
    #[derive(Debug, Clone)]
    pub struct FramingBuilder {
        right: [f64; 3],
        down: [f64; 3],
        cam: [f64; 3],
        min: [f64; 2],
        max: [f64; 2],
        lo3: [f64; 3],
        hi3: [f64; 3],
        dmin: f64,
        dmax: f64,
    }

    impl FramingBuilder {
        /// Start an empty framing for `view`.
        pub fn new(view: View) -> Self {
            FramingBuilder {
                right: normalize(view.right()),
                down: normalize(view.down()),
                cam: view.cam(),
                min: [f64::INFINITY; 2],
                max: [f64::NEG_INFINITY; 2],
                lo3: [f64::INFINITY; 3],
                hi3: [f64::NEG_INFINITY; 3],
                dmin: f64::INFINITY,
                dmax: f64::NEG_INFINITY,
            }
        }

        /// Fold one world-space point into the framing.
        pub fn add(&mut self, v: [f64; 3]) {
            let s = [dot(v, self.right), dot(v, self.down)];
            for (i, si) in s.iter().enumerate() {
                self.min[i] = self.min[i].min(*si);
                self.max[i] = self.max[i].max(*si);
            }
            for (i, vi) in v.iter().enumerate() {
                self.lo3[i] = self.lo3[i].min(*vi);
                self.hi3[i] = self.hi3[i].max(*vi);
            }
            let d = dot(v, self.cam);
            self.dmin = self.dmin.min(d);
            self.dmax = self.dmax.max(d);
        }

        /// Finish, failing closed on an empty or degenerate accumulation
        /// rather than handing the renderer an infinite canvas.
        pub fn finish(self) -> Result<FixedFraming, String> {
            let extent = (self.max[0] - self.min[0]).max(self.max[1] - self.min[1]);
            if !extent.is_finite() || extent < 1e-9 {
                return Err("degenerate projection (no extent) across the sequence".to_string());
            }
            let d = (0..3)
                .map(|i| (self.hi3[i] - self.lo3[i]).powi(2))
                .sum::<f64>()
                .sqrt();
            Ok(FixedFraming {
                screen_min: self.min,
                screen_max: self.max,
                diag: d,
                depth: (self.dmin, self.dmax),
            })
        }
    }

    /// Pick the output canvas for a projection whose screen extents are
    /// `ex` × `ey` (mm). `height_px` wins outright; `auto_aspect` fits the
    /// short axis to the projection (long axis keeps `size_px`); otherwise
    /// the canvas is square, as it always was.
    pub(crate) fn canvas_for(opts: &RasterOptions, ex: f64, ey: f64) -> Canvas {
        const MIN_PX: u32 = 16;
        let w = opts.size_px;
        match opts.height_px {
            Some(h) => Canvas {
                w: w as usize,
                h: h.max(MIN_PX) as usize,
            },
            None if opts.auto_aspect
                && ex.is_finite()
                && ey.is_finite()
                && ex > 0.0
                && ey > 0.0 =>
            {
                let short = |long: f64, other: f64| {
                    ((w as f64 * other / long).round() as u32).clamp(MIN_PX, w)
                };
                if ex >= ey {
                    Canvas {
                        w: w as usize,
                        h: short(ex, ey) as usize,
                    }
                } else {
                    Canvas {
                        w: short(ey, ex) as usize,
                        h: w as usize,
                    }
                }
            }
            None => Canvas {
                w: w as usize,
                h: w as usize,
            },
        }
    }

    /// Pixels per mm that fit a projection with screen extents `ex` × `ey`
    /// into `canvas` at `fill` of the binding axis. On a square canvas this
    /// is evaluated exactly as it always was (`fill * size / extent`) so
    /// existing reference renders stay byte-identical.
    pub(crate) fn fit_scale(fill: f64, canvas: Canvas, ex: f64, ey: f64, extent: f64) -> f64 {
        if canvas.w == canvas.h {
            return fill * canvas.w as f64 / extent;
        }
        let axis = |px: usize, e: f64| {
            if e > 0.0 {
                fill * px as f64 / e
            } else {
                f64::INFINITY
            }
        };
        let s = axis(canvas.w, ex).min(axis(canvas.h, ey));
        if s.is_finite() {
            s
        } else {
            fill * canvas.min_side() as f64 / extent
        }
    }

    /// Options for [`render_jpeg_str`] / [`render_jpeg_solids`].
    #[derive(Debug, Clone)]
    pub struct RasterOptions {
        /// Camera orientation.
        pub view: View,
        /// Canvas width in pixels; also the height unless `height_px` or
        /// `auto_aspect` says otherwise.
        pub size_px: u32,
        /// Canvas height in pixels. `None` = square (`size_px` × `size_px`),
        /// unless `auto_aspect` picks a height from the projection.
        pub height_px: Option<u32>,
        /// Fit the canvas to the projected aspect ratio: the subject's long
        /// screen axis keeps `size_px` pixels and the short axis shrinks to
        /// match, instead of padding a square with background. Ignored when
        /// `height_px` is set.
        pub auto_aspect: bool,
        /// Crop the output to the drawn content's bounding box, keeping this
        /// many pixels of margin. `None` = no crop (the full canvas).
        pub trim_margin_px: Option<u32>,
        /// Fraction of the canvas the part's long axis fills (mecheval
        /// capture rules say ~60%).
        pub fill_frac: f64,
        /// JPEG quality, 1–100 (capture rules say ≥ 90).
        pub quality: u8,
        /// Frame the render on this part instead of the whole document
        /// (same matching rules as [`CameraOptions::focus`]). Only honoured
        /// by [`render_jpeg_str`] / [`render_png_str`]; the solids path has
        /// no part names.
        pub focus: Option<String>,
        /// Optional section (cutaway) plane: material on the camera's
        /// side of the plane is removed and exposed cut faces are hatched.
        pub section: Option<SectionPlane>,
        /// Opt-in engineering overlays (axes gizmo, part labels, bbox
        /// dimensions). All-off by default; the default render is
        /// byte-identical to an annotation-free build.
        pub annotations: RenderAnnotations,
        /// Supersampling factor for the tessellated raster path: geometry
        /// and linework rasterize on an `aa`× canvas and box-filter down,
        /// so silhouettes and edge strokes anti-alias. `None` picks a
        /// factor from `size_px` (see [`auto_aa`]); `Some(1)` is the old
        /// point-sampled output. Clamped to 1..=4. Ignored by the
        /// ray-traced path, which supersamples on its own.
        pub aa: Option<u32>,
    }

    /// Supersampling factor for a given output width when the caller
    /// didn't pick one. Small renders alias worst and cost least, so they
    /// get the most samples; past 2048px the point-sampled output already
    /// reads clean and the `ss²` memory is not worth it (`--aa` still
    /// overrides).
    pub(crate) fn auto_aa(size_px: u32) -> u32 {
        match size_px {
            0..=1024 => 3,
            1025..=2048 => 2,
            _ => 1,
        }
    }

    impl Default for RasterOptions {
        fn default() -> Self {
            RasterOptions {
                view: View::Isometric,
                size_px: 1024,
                height_px: None,
                auto_aspect: false,
                trim_margin_px: None,
                fill_frac: 0.6,
                quality: 92,
                focus: None,
                section: None,
                annotations: RenderAnnotations::default(),
                aa: None,
            }
        }
    }

    /// Render raw `.vcad` document JSON to JPEG bytes.
    ///
    /// Orthographic projection from `opts.view`, z-buffered flat shading
    /// with the same edge classification as the SVG path drawn on top
    /// (hidden lines removed). Errors are human-readable strings.
    pub fn render_jpeg_str(raw_vcad: &str, opts: &RasterOptions) -> Result<Vec<u8>, String> {
        let scene = evaluate_vcad(raw_vcad)?;
        let solids: Vec<Solid> = scene.iter().map(|s| s.solid.clone()).collect();
        let tints: Vec<Option<[f64; 3]>> = scene.iter().map(|s| s.tint).collect();
        let names: Vec<Option<String>> = scene.iter().map(|s| s.name.clone()).collect();
        // Focus framing and section cutting don't currently compose in the
        // raster path: `rasterize` applies the section (which can drop
        // solids) after this mask is built. Callers combine them via the SVG
        // path (`render_svg_str_opts`), which sections before masking.
        let mask = opts
            .focus
            .as_deref()
            .map(|f| focus_mask(&scene, f))
            .transpose()?;
        encode_jpeg(
            rasterize(&solids, &tints, &names, opts, mask.as_deref(), false, None)?,
            opts,
        )
    }

    /// Render pre-evaluated solids to JPEG bytes, monochrome (no material
    /// tints — mecheval reference images depend on this). See
    /// [`render_jpeg_str`], which honours document material colours.
    pub fn render_jpeg_solids(solids: &[Solid], opts: &RasterOptions) -> Result<Vec<u8>, String> {
        let no_tints: Vec<Option<[f64; 3]>> = vec![None; solids.len()];
        let no_names: Vec<Option<String>> = vec![None; solids.len()];
        encode_jpeg(
            rasterize(solids, &no_tints, &no_names, opts, None, false, None)?,
            opts,
        )
    }

    /// Render raw `.vcad` document JSON to RGBA PNG bytes with a fully
    /// transparent background (alpha 0 wherever no geometry or edge stroke
    /// was drawn). Same projection and shading as [`render_jpeg_str`];
    /// `opts.quality` is ignored (PNG is lossless).
    pub fn render_png_str(raw_vcad: &str, opts: &RasterOptions) -> Result<Vec<u8>, String> {
        let scene = evaluate_vcad(raw_vcad)?;
        let solids: Vec<Solid> = scene.iter().map(|s| s.solid.clone()).collect();
        let tints: Vec<Option<[f64; 3]>> = scene.iter().map(|s| s.tint).collect();
        let names: Vec<Option<String>> = scene.iter().map(|s| s.name.clone()).collect();
        let mask = opts
            .focus
            .as_deref()
            .map(|f| focus_mask(&scene, f))
            .transpose()?;
        encode_png(
            rasterize(&solids, &tints, &names, opts, mask.as_deref(), true, None)?,
            opts,
        )
    }

    /// Render pre-evaluated, already-placed solids to RGBA PNG bytes using a
    /// framing computed elsewhere — the animation path's per-frame entry
    /// point.
    ///
    /// Everything else matches [`render_png_str`]; only the camera framing
    /// (canvas, scale, depth-cue range) comes from `framing` instead of from
    /// this frame's own bounds, which is what keeps every frame of a
    /// sequence the same size with the subject nailed in place.
    pub(crate) fn render_png_solids_framed(
        solids: &[Solid],
        tints: &[Option<[f64; 3]>],
        names: &[Option<String>],
        opts: &RasterOptions,
        framing: &FixedFraming,
    ) -> Result<Vec<u8>, String> {
        encode_png(
            rasterize(solids, tints, names, opts, None, true, Some(framing))?,
            opts,
        )
    }

    /// Render pre-evaluated solids to RGBA PNG bytes with a transparent
    /// background, monochrome (no material tints). See [`render_png_str`].
    pub fn render_png_solids(solids: &[Solid], opts: &RasterOptions) -> Result<Vec<u8>, String> {
        let no_tints: Vec<Option<[f64; 3]>> = vec![None; solids.len()];
        let no_names: Vec<Option<String>> = vec![None; solids.len()];
        encode_png(
            rasterize(solids, &no_tints, &no_names, opts, None, true, None)?,
            opts,
        )
    }

    /// JPEG-encode a rasterized frame (coverage mask ignored — JPEG keeps
    /// the opaque vellum background).
    pub(crate) fn encode_jpeg(frame: Frame, opts: &RasterOptions) -> Result<Vec<u8>, String> {
        let frame = frame.trimmed(opts.trim_margin_px);
        let mut out = Vec::new();
        let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(
            &mut out,
            opts.quality.clamp(1, 100),
        );
        enc.encode(
            &frame.rgb,
            frame.canvas.w as u32,
            frame.canvas.h as u32,
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|e| format!("jpeg encode: {}", e))?;
        Ok(out)
    }

    /// PNG-encode a rasterized frame as RGBA: covered pixels are opaque,
    /// background pixels get alpha 0.
    pub(crate) fn encode_png(frame: Frame, opts: &RasterOptions) -> Result<Vec<u8>, String> {
        let frame = frame.trimmed(opts.trim_margin_px);
        let mut rgba = Vec::with_capacity(frame.mask.len() * 4);
        for (i, &a) in frame.mask.iter().enumerate() {
            rgba.extend_from_slice(&frame.rgb[i * 3..i * 3 + 3]);
            rgba.push(a);
        }
        let mut out = Vec::new();
        let enc = image::codecs::png::PngEncoder::new(&mut out);
        image::ImageEncoder::write_image(
            enc,
            &rgba,
            frame.canvas.w as u32,
            frame.canvas.h as u32,
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|e| format!("png encode: {}", e))?;
        Ok(out)
    }

    /// Shared raster implementation; `tints[i]`, when present, tints solid
    /// `i`'s shading ramp exactly as the SVG path does. Untinted solids keep
    /// the original two-stop monochrome shading, byte-for-byte. `names[i]`
    /// labels solid `i` when the `labels` annotation is on. Returns the RGB
    /// frame plus a per-pixel coverage mask (255 where geometry, an edge
    /// stroke, or an annotation was drawn, 0 over untouched background).
    ///
    /// `framing`, when given, replaces this frame's own projected bounds
    /// (and the 3D diagonal and depth range derived from them) with a
    /// pre-computed camera — see [`FixedFraming`]. `None` frames the frame
    /// on itself, which is what every single-image caller does and what
    /// makes their output byte-identical to before this parameter existed.
    #[allow(clippy::too_many_arguments)]
    fn rasterize(
        solids: &[Solid],
        tints: &[Option<[f64; 3]>],
        names: &[Option<String>],
        opts: &RasterOptions,
        focus: Option<&[bool]>,
        png: bool,
        framing: Option<&FixedFraming>,
    ) -> Result<Frame, String> {
        if solids.is_empty() {
            return Err("no solids produced".to_string());
        }
        if opts.size_px < 16 {
            return Err("size_px too small".to_string());
        }
        if !(opts.fill_frac > 0.0 && opts.fill_frac <= 1.0) {
            return Err("fill_frac must be in (0, 1]".to_string());
        }

        // Apply the section cut (if any) before tessellation. A solid whose
        // boolean fails is rendered uncut; one wholly on the removed side is
        // dropped. Rebuild SceneSolids so tints and names stay aligned with
        // the (possibly shorter) cut solid list.
        // Parallel solid/tint/name columns, kept aligned across a section cut.
        type Columns = (Vec<Solid>, Vec<Option<[f64; 3]>>, Vec<Option<String>>);
        let (solids, tints, names): Columns = match opts.section {
            Some(plane) => {
                let scene: Vec<SceneSolid> = solids
                    .iter()
                    .enumerate()
                    .map(|(i, s)| SceneSolid {
                        solid: s.clone(),
                        tint: tints.get(i).copied().flatten(),
                        // Section path only needs colour; the photoreal
                        // renderer does its own evaluation.
                        material: None,
                        name: names.get(i).cloned().flatten(),
                        // Raster path does no highlighting or focus lookup
                        // here; id and labels are unused.
                        labels: Vec::new(),
                        id: String::new(),
                    })
                    .collect();
                let cut = apply_section(scene, plane, opts.view);
                (
                    cut.iter().map(|s| s.solid.clone()).collect(),
                    cut.iter().map(|s| s.tint).collect(),
                    cut.iter().map(|s| s.name.clone()).collect(),
                )
            }
            None => (solids.to_vec(), tints.to_vec(), names.to_vec()),
        };
        if solids.is_empty() {
            return Err("no solids survive the section plane".to_string());
        }
        let (solids, tints, names) = (&solids[..], &tints[..], &names[..]);

        let cam = opts.view.cam();
        let right = normalize(opts.view.right());
        let down = normalize(opts.view.down());
        let light = normalize(LIGHT);

        let segments = segments_for(opts.size_px);
        let no_accents = vec![false; solids.len()];
        let arts = build_artifacts(
            solids,
            tints,
            &no_accents,
            cam,
            segments,
            &EdgeRules {
                coplanar_dot_tol: RASTER_COPLANAR_DOT_TOL,
                mark_silhouette: true,
                keep_occluded: false,
            },
            opts.section,
            RenderStyle::Drafting,
        );

        // Screen-plane (mm) and 3D bounding boxes over all vertices.
        let mut min = [f64::INFINITY; 2];
        let mut max = [f64::NEG_INFINITY; 2];
        let mut lo3 = [f64::INFINITY; 3];
        let mut hi3 = [f64::NEG_INFINITY; 3];
        for art in &arts {
            // With a focus mask, only the focused solids' vertices drive
            // the screen frame; everything still rasterizes (and crops).
            let framed = focus.is_none_or(|m| m.get(art.src).copied().unwrap_or(false));
            for v in &art.verts {
                let s = [dot(*v, right), dot(*v, down)];
                if framed {
                    for i in 0..2 {
                        min[i] = min[i].min(s[i]);
                        max[i] = max[i].max(s[i]);
                    }
                }
                for i in 0..3 {
                    lo3[i] = lo3[i].min(v[i]);
                    hi3[i] = hi3[i].max(v[i]);
                }
            }
        }
        // A pinned camera (animation) overrides the self-framing above.
        if let Some(f) = framing {
            min = f.screen_min;
            max = f.screen_max;
        }
        let extent = (max[0] - min[0]).max(max[1] - min[1]);
        if !extent.is_finite() || extent < 1e-9 {
            return Err(if focus.is_some() {
                "focused part produced no geometry".to_string()
            } else {
                "degenerate projection (no extent)".to_string()
            });
        }
        let diag = match framing {
            Some(f) => f.diag,
            None => {
                ((hi3[0] - lo3[0]).powi(2) + (hi3[1] - lo3[1]).powi(2) + (hi3[2] - lo3[2]).powi(2))
                    .sqrt()
            }
        };

        let canvas = canvas_for(opts, max[0] - min[0], max[1] - min[1]);
        // The subject fills `fill_frac` of whichever canvas axis binds
        // first; on a square canvas this is the historical
        // `fill_frac * size_px / extent`, byte-for-byte.
        let px_per_mm = fit_scale(
            opts.fill_frac,
            canvas,
            max[0] - min[0],
            max[1] - min[1],
            extent,
        );
        let cx = (min[0] + max[0]) / 2.0;
        let cy = (min[1] + max[1]) / 2.0;
        let (halfx, halfy) = (canvas.w as f64 / 2.0, canvas.h as f64 / 2.0);
        // World point → (output pixel x, output pixel y, depth toward
        // camera in mm). Annotations draw in this space; geometry and
        // linework scale it up by `ss` (below).
        let to_px = |v: [f64; 3]| -> (f64, f64, f64) {
            (
                (dot(v, right) - cx) * px_per_mm + halfx,
                (dot(v, down) - cy) * px_per_mm + halfy,
                dot(v, cam),
            )
        };

        // Supersampling. The tessellated path point-samples at pixel
        // centres, so at 1× every silhouette and every edge stroke is hard
        // aliased. Rasterizing `ss`× oversized and box-filtering down
        // anti-aliases both, and — unlike blending partial coverage in
        // place — leaves no seam where two triangles of one flat face
        // abut, since both sub-samples land on geometry.
        let ss = opts.aa.unwrap_or_else(|| auto_aa(opts.size_px)).clamp(1, 4) as usize;
        let sc = Canvas {
            w: canvas.w * ss,
            h: canvas.h * ss,
        };
        let to_ss = |p: (f64, f64, f64)| (p.0 * ss as f64, p.1 * ss as f64, p.2);

        let mut rgb: Vec<u8> = BACKGROUND
            .iter()
            .copied()
            .cycle()
            .take(sc.len() * 3)
            .collect();
        let mut zbuf: Vec<f64> = vec![f64::NEG_INFINITY; sc.len()];
        let mut mask: Vec<u8> = vec![0; sc.len()];

        // Depth range for depth cueing (below). Pinned across an animation
        // so a part doesn't change shade merely because another part moved.
        let mut dmin = f64::INFINITY;
        let mut dmax = f64::NEG_INFINITY;
        for art in &arts {
            for v in &art.verts {
                let d = dot(*v, cam);
                dmin = dmin.min(d);
                dmax = dmax.max(d);
            }
        }
        if let Some(f) = framing {
            (dmin, dmax) = f.depth;
        }
        let dspan = (dmax - dmin).max(1e-9);

        // Pass 1: z-buffered flat-shaded triangles. Pure Lambertian shading
        // gives every face with the same normal the same colour, which
        // makes axis-aligned recesses invisible in axis-aligned views — a
        // mild depth cue (farther → darker) separates them.
        for (ai, art) in arts.iter().enumerate() {
            let proj: Vec<(f64, f64, f64)> = art.verts.iter().map(|v| to_ss(to_px(*v))).collect();
            let tinted = tints.get(ai).copied().flatten().is_some();
            for (ti, t) in art.tris.iter().enumerate() {
                if !art.visible[ti] {
                    continue;
                }
                // Tinted parts sample their material ramp (same as the SVG
                // path); untinted parts keep the original two-stop
                // monochrome shade.
                let shade = TriShade {
                    lit: lambertian(art.normals[ti], light),
                    ramp: tinted.then_some(art.ramp),
                    dmin,
                    dspan,
                };
                fill_triangle(
                    &mut rgb,
                    &mut zbuf,
                    &mut mask,
                    sc,
                    [proj[t[0]], proj[t[1]], proj[t[2]]],
                    shade,
                    art.cut[ti],
                    ss,
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
        // Measured on the *output* canvas and scaled by `ss` so the
        // downsampled weight is independent of the supersampling factor.
        let stroke_px = (canvas.min_side() / 512).max(2) * ss;
        for art in &arts {
            let proj: Vec<(f64, f64, f64)> = art.verts.iter().map(|v| to_ss(to_px(*v))).collect();
            for &(a, b, _kind) in &art.edges {
                draw_edge(
                    &mut rgb, &zbuf, &mut mask, sc, proj[a], proj[b], bias_base, stroke_px,
                );
            }
        }

        // Box-filter the supersampled buffer down to the output canvas.
        // Colour is the mean of the *covered* sub-pixels only, so a
        // partly-covered edge pixel isn't tinted by the vellum ground it
        // overlaps; the coverage goes to the mask instead. As in the
        // ray-traced path, a JPEG frame composites that coverage over
        // BACKGROUND here and a PNG frame keeps straight alpha.
        let (mut rgb, mut mask) = if ss == 1 {
            (rgb, mask)
        } else {
            let n = (ss * ss) as f64;
            let mut out_rgb: Vec<u8> = Vec::with_capacity(canvas.len() * 3);
            let mut out_mask: Vec<u8> = Vec::with_capacity(canvas.len());
            for y in 0..canvas.h {
                for x in 0..canvas.w {
                    let mut acc = [0.0f64; 3];
                    let mut hits = 0u32;
                    for sy in 0..ss {
                        for sx in 0..ss {
                            let i = sc.idx(x * ss + sx, y * ss + sy);
                            if mask[i] == 0 {
                                continue;
                            }
                            hits += 1;
                            for (k, a) in acc.iter_mut().enumerate() {
                                *a += rgb[i * 3 + k] as f64;
                            }
                        }
                    }
                    if hits == 0 {
                        out_rgb.extend_from_slice(&BACKGROUND);
                        out_mask.push(0);
                        continue;
                    }
                    out_mask.push(((hits as f64 / n) * 255.0).round() as u8);
                    for (k, a) in acc.iter().enumerate() {
                        let v = if png {
                            a / hits as f64
                        } else {
                            (a + (n - hits as f64) * BACKGROUND[k] as f64) / n
                        };
                        out_rgb.push(v.round().clamp(0.0, 255.0) as u8);
                    }
                }
            }
            (out_rgb, out_mask)
        };

        // Pass 3 (opt-in): engineering-context overlays, drawn over the
        // finished render in output pixel space. Overlay pixels also mark
        // the coverage mask so gizmo/dimension linework over the
        // background stays opaque in the transparent PNG output.
        if opts.annotations.any() {
            draw_annotations(
                &mut rgb, &mut mask, canvas, &arts, names, opts, &to_px, lo3, hi3,
            );
        }

        Ok(Frame { rgb, mask, canvas })
    }

    fn edge_fn(a: (f64, f64, f64), b: (f64, f64, f64), px: f64, py: f64) -> f64 {
        (b.0 - a.0) * (py - a.1) - (b.1 - a.1) * (px - a.0)
    }

    /// Hatch period (px) for section-cut faces in the raster path; lines
    /// run at 45° because `x + y` is constant along each anti-diagonal.
    const HATCH_PERIOD_PX: usize = 9;
    /// Hatch line thickness (px) within each period.
    const HATCH_LINE_PX: usize = 2;
    /// Cut-face background — a pale ice tint (matches the SVG `HATCH_BG`).
    const HATCH_BG_RGB: [u8; 3] = [220, 232, 242];

    /// A triangle's shading inputs, resolved to a colour **per pixel**.
    ///
    /// The Lambertian term is constant over a flat-shaded triangle, but the
    /// depth cue is not: evaluating it once at the centroid quantizes it to
    /// the triangulation, so a single flat face reads as a fan of hard
    /// tonal wedges (worst on the large top faces that dominate an
    /// isometric view). Interpolating the same cue from the z-buffer depth
    /// the rasterizer already computes makes it continuous across triangle
    /// boundaries — matching what the ray-traced path does per hit point.
    #[derive(Clone, Copy)]
    struct TriShade {
        /// Lambertian term for the triangle's normal, before the cue.
        lit: f64,
        /// Material ramp for a tinted solid; `None` = two-stop monochrome.
        ramp: Option<[[u8; 3]; 4]>,
        /// Depth-cue normalization over the whole scene.
        dmin: f64,
        dspan: f64,
    }

    impl TriShade {
        /// Colour at a pixel whose interpolated depth is `depth`.
        fn at(&self, depth: f64) -> [u8; 3] {
            let cue = 0.78 + 0.22 * ((depth - self.dmin) / self.dspan).clamp(0.0, 1.0);
            let lit = (self.lit * cue).clamp(0.0, 1.0);
            match &self.ramp {
                Some(ramp) => ramp_sample(ramp, lit),
                None => mix_rgb(FILL_DARK, FILL_LIGHT, lit),
            }
        }
    }

    /// `hatch_scale` stretches the section-cut hatch pattern so it keeps
    /// its output-space period when rasterizing supersampled.
    #[allow(clippy::too_many_arguments)]
    fn fill_triangle(
        rgb: &mut [u8],
        zbuf: &mut [f64],
        mask: &mut [u8],
        canvas: Canvas,
        p: [(f64, f64, f64); 3],
        shade: TriShade,
        hatch: bool,
        hatch_scale: usize,
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
            .min((canvas.w - 1) as f64) as usize;
        let y1 = ys
            .iter()
            .cloned()
            .fold(f64::NEG_INFINITY, f64::max)
            .ceil()
            .min((canvas.h - 1) as f64) as usize;
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
                let idx = canvas.idx(px, py);
                if depth > zbuf[idx] {
                    zbuf[idx] = depth;
                    mask[idx] = 255;
                    let c = if hatch {
                        // 45° drafting hatch: ink line over pale ice.
                        if (px + py) % (HATCH_PERIOD_PX * hatch_scale) < HATCH_LINE_PX * hatch_scale
                        {
                            FILL_DARK
                        } else {
                            HATCH_BG_RGB
                        }
                    } else {
                        shade.at(depth)
                    };
                    rgb[idx * 3] = c[0];
                    rgb[idx * 3 + 1] = c[1];
                    rgb[idx * 3 + 2] = c[2];
                }
            }
        }
    }

    // ── raster annotation overlay ─────────────────────────────────────────

    /// Visible linework ink as RGB (the raster twin of [`INK`]).
    const INK_RGB: [u8; 3] = [11, 39, 66];
    /// Axis gizmo colours (X, Y, Z), matching [`AXIS_COLORS`]. These appear
    /// nowhere else in a render, which makes them a clean handle on the
    /// overlay in tests.
    pub(crate) const AXIS_RGB: [[u8; 3]; 3] = [[192, 57, 43], [30, 142, 62], [43, 108, 176]];
    /// Text glyph pixel scale (5×7 font → 10×14 px per glyph) on the
    /// reference canvas; multiplied by [`AnnoScale`] in practice.
    const FONT_SCALE: usize = 2;

    /// Canvas the annotation overlay's pixel constants were tuned on.
    const ANNO_REFERENCE_PX: f64 = 1024.0;

    /// Converts *annotation units* — the pixel sizes that read correctly on
    /// a 1024px canvas — into output pixels.
    ///
    /// Every constant in the overlay (glyph size, leader offsets, gizmo arm
    /// length, line weights) used to be a bare pixel count, so the overlay
    /// only ever looked right at one canvas size: at 2048px+ the gizmo
    /// shrivelled into the corner and the dimension leaders, offset a fixed
    /// 18px, ran straight through the part they were measuring; at 256px
    /// they swamped it. Scaling them keeps a thumbnail and a hero render
    /// carrying the same-looking annotation.
    #[derive(Clone, Copy)]
    struct AnnoScale(f64);

    impl AnnoScale {
        fn for_canvas(canvas: Canvas) -> Self {
            AnnoScale(canvas.min_side() as f64 / ANNO_REFERENCE_PX)
        }
        /// Annotation units → output pixels.
        fn u(self, units: f64) -> f64 {
            units * self.0
        }
        /// Annotation units → a whole-pixel span, never rounded away to
        /// nothing (a 1px hairline must survive a 256px canvas).
        fn i(self, units: f64) -> i64 {
            self.u(units).round().max(1.0) as i64
        }
        /// Magnification of the 5×7 glyph grid.
        fn font(self) -> usize {
            self.i(FONT_SCALE as f64) as usize
        }
        /// Height of a line of text, in output pixels.
        fn text_h(self) -> f64 {
            (7 * self.font()) as f64
        }
    }

    /// Draw the opt-in overlays (axes gizmo, part labels, bbox dimensions)
    /// over a finished raster render, in pixel space.
    #[allow(clippy::too_many_arguments)]
    fn draw_annotations(
        rgb: &mut [u8],
        mask: &mut [u8],
        canvas: Canvas,
        arts: &[SolidArtifacts],
        names: &[Option<String>],
        opts: &RasterOptions,
        to_px: &dyn Fn([f64; 3]) -> (f64, f64, f64),
        lo3: [f64; 3],
        hi3: [f64; 3],
    ) {
        let annos = &opts.annotations;
        let s = AnnoScale::for_canvas(canvas);
        let hair = s.i(1.0);
        if annos.dims {
            // Every projected vertex, so a dimension line can be pushed
            // clear of the silhouette rather than a fixed distance off the
            // bounding box (see `off` below).
            let screen: Vec<(f64, f64)> = arts
                .iter()
                .flat_map(|a| a.verts.iter())
                .map(|v| {
                    let (x, y, _) = to_px(*v);
                    (x, y)
                })
                .filter(|p| p.0.is_finite() && p.1.is_finite())
                .collect();
            for dim in bbox_dim_specs(lo3, hi3) {
                let (ax, ay, _) = to_px(dim.a);
                let (bx, by, _) = to_px(dim.b);
                let (dx, dy) = (bx - ax, by - ay);
                let len = (dx * dx + dy * dy).sqrt();
                if len < s.u(6.0) {
                    continue; // axis parallel to the camera in this view
                }
                let mut n = (-dy / len, dx / len);
                let mid = ((ax + bx) / 2.0, (ay + by) / 2.0);
                let (halfx, halfy) = (canvas.w as f64 / 2.0, canvas.h as f64 / 2.0);
                if n.0 * (mid.0 - halfx) + n.1 * (mid.1 - halfy) < 0.0 {
                    n = (-n.0, -n.1);
                }
                // Stand the dimension line off the *silhouette*, not the
                // bounding box. A fixed offset works for a box, whose bbox
                // corners are its own corners, but on anything round the
                // corner projects inside the outline — so the dimension
                // line and its label landed on top of the part. Reach past
                // the furthest projected vertex along `n` instead, and keep
                // the same clear margin beyond it.
                let clear = screen
                    .iter()
                    .map(|p| (p.0 - ax) * n.0 + (p.1 - ay) * n.1)
                    .fold(0.0, f64::max);
                let off = clear + s.u(18.0);
                let tick = s.u(4.0);
                let a1 = (ax + n.0 * off, ay + n.1 * off);
                let a2 = (bx + n.0 * off, by + n.1 * off);
                for (p, e) in [((ax, ay), a1), ((bx, by), a2)] {
                    draw_line_col(
                        rgb,
                        mask,
                        canvas,
                        p,
                        (e.0 + n.0 * tick, e.1 + n.1 * tick),
                        INK_RGB,
                        hair,
                    );
                }
                draw_line_col(rgb, mask, canvas, a1, a2, INK_RGB, hair);
                let (ux, uy) = (dx / len, dy / len);
                for p in [a1, a2] {
                    let t = ((ux - n.0) * tick, (uy - n.1) * tick);
                    draw_line_col(
                        rgb,
                        mask,
                        canvas,
                        (p.0 - t.0, p.1 - t.1),
                        (p.0 + t.0, p.1 + t.1),
                        INK_RGB,
                        hair,
                    );
                }
                let out = off + s.u(14.0);
                let text = (mid.0 + n.0 * out, mid.1 + n.1 * out);
                draw_text_centered(rgb, mask, canvas, text, &dim.label, INK_RGB, s);
            }
        }
        if annos.labels {
            for art in arts {
                let Some(name) = names.get(art.src).and_then(|n| n.as_deref()) else {
                    continue;
                };
                let pts: Vec<(f64, f64)> = art
                    .verts
                    .iter()
                    .map(|v| {
                        let (x, y, _) = to_px(*v);
                        (x, y)
                    })
                    .filter(|p| p.0.is_finite() && p.1.is_finite())
                    .collect();
                if pts.is_empty() {
                    continue;
                }
                let cx = pts.iter().map(|p| p.0).sum::<f64>() / pts.len() as f64;
                // Lead away from whichever side of the canvas the part sits on.
                let dir = if cx > canvas.w as f64 * 0.5 {
                    -1.0
                } else {
                    1.0
                };
                // Anchor on the silhouette, not the centre of the bounding
                // box: a centred leader plants its dot and its text on top
                // of the very geometry it names — for a single-part document
                // that is dead centre of the render. The extreme vertex along
                // the leader's own diagonal is always on the projection's
                // convex hull, so the dot lands on the part and the leader
                // runs straight out into empty background.
                let score = |p: &(f64, f64)| p.0 * dir - p.1;
                let anchor = *pts
                    .iter()
                    .max_by(|a, b| score(a).total_cmp(&score(b)))
                    .expect("pts is non-empty");
                let reach = s.u(22.0);
                let elbow = (anchor.0 + reach * dir, anchor.1 - reach);
                draw_line_col(rgb, mask, canvas, anchor, elbow, INK_RGB, hair);
                // Anchor dot.
                let dot = s.u(1.0);
                draw_line_col(
                    rgb,
                    mask,
                    canvas,
                    (anchor.0 - dot, anchor.1),
                    (anchor.0 + dot, anchor.1),
                    INK_RGB,
                    s.i(2.0),
                );
                let tw = text_width_px(name, s) as f64;
                let gap = s.u(4.0);
                let tx = if dir > 0.0 {
                    elbow.0 + gap
                } else {
                    elbow.0 - gap - tw
                };
                // Keep the plate on-canvas even when the part crowds an edge.
                let tx = tx.clamp(0.0, (canvas.w as f64 - tw).max(0.0));
                let ty = (elbow.1 - s.text_h() / 2.0).clamp(0.0, canvas.h as f64 - s.text_h());
                draw_text(rgb, mask, canvas, (tx, ty), name, INK_RGB, s);
            }
        }
        if annos.axes {
            let inset = s.u(48.0);
            let origin = (inset, canvas.h as f64 - inset);
            let right = normalize(opts.view.right());
            let down = normalize(opts.view.down());
            let arm = s.u(26.0);
            let barb = s.u(6.0);
            let weight = s.i(2.0);
            for i in 0..3 {
                let mut e = [0.0; 3];
                e[i] = 1.0;
                let d = (dot(e, right), dot(e, down));
                let m = (d.0 * d.0 + d.1 * d.1).sqrt();
                if m < 1e-6 {
                    continue;
                }
                let (ux, uy) = (d.0 / m, d.1 / m);
                let tip = (origin.0 + ux * arm, origin.1 + uy * arm);
                let color = AXIS_RGB[i];
                draw_line_col(rgb, mask, canvas, origin, tip, color, weight);
                for a in [0.45f64, -0.45] {
                    let (bx, by) = (-ux * a.cos() - uy * a.sin(), ux * a.sin() - uy * a.cos());
                    draw_line_col(
                        rgb,
                        mask,
                        canvas,
                        tip,
                        (tip.0 + bx * barb, tip.1 + by * barb),
                        color,
                        weight,
                    );
                }
                let label = s.u(10.0);
                draw_text_centered(
                    rgb,
                    mask,
                    canvas,
                    (tip.0 + ux * label, tip.1 + uy * label),
                    AXIS_NAMES[i],
                    color,
                    s,
                );
            }
        }
    }

    /// Unconditional coloured line (no z-test) for annotation overlays.
    /// `thick` paints an n×n block per sample.
    fn draw_line_col(
        rgb: &mut [u8],
        mask: &mut [u8],
        canvas: Canvas,
        a: (f64, f64),
        b: (f64, f64),
        color: [u8; 3],
        thick: i64,
    ) {
        let len = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
        let steps = (len * 2.0).ceil().max(1.0) as usize;
        for s in 0..=steps {
            let t = s as f64 / steps as f64;
            let x = (a.0 + (b.0 - a.0) * t).floor() as i64;
            let y = (a.1 + (b.1 - a.1) * t).floor() as i64;
            for dy in 0..thick {
                for dx in 0..thick {
                    let (qx, qy) = (x + dx, y + dy);
                    if !canvas.contains(qx, qy) {
                        continue;
                    }
                    let cell = canvas.idx(qx as usize, qy as usize);
                    let qi = cell * 3;
                    rgb[qi] = color[0];
                    rgb[qi + 1] = color[1];
                    rgb[qi + 2] = color[2];
                    mask[cell] = 255;
                }
            }
        }
    }

    /// Classic 5×7 pixel font, column-major, LSB = top row. Covers digits,
    /// uppercase letters (input is uppercase-folded), and the punctuation a
    /// dimension label or part name plausibly needs; anything else renders
    /// as a hollow box.
    fn glyph5x7(c: char) -> [u8; 5] {
        match c.to_ascii_uppercase() {
            ' ' => [0x00, 0x00, 0x00, 0x00, 0x00],
            '-' => [0x08, 0x08, 0x08, 0x08, 0x08],
            '_' => [0x40, 0x40, 0x40, 0x40, 0x40],
            '.' => [0x00, 0x60, 0x60, 0x00, 0x00],
            '/' => [0x20, 0x10, 0x08, 0x04, 0x02],
            '0' => [0x3E, 0x51, 0x49, 0x45, 0x3E],
            '1' => [0x00, 0x42, 0x7F, 0x40, 0x00],
            '2' => [0x42, 0x61, 0x51, 0x49, 0x46],
            '3' => [0x21, 0x41, 0x45, 0x4B, 0x31],
            '4' => [0x18, 0x14, 0x12, 0x7F, 0x10],
            '5' => [0x27, 0x45, 0x45, 0x45, 0x39],
            '6' => [0x3C, 0x4A, 0x49, 0x49, 0x30],
            '7' => [0x01, 0x71, 0x09, 0x05, 0x03],
            '8' => [0x36, 0x49, 0x49, 0x49, 0x36],
            '9' => [0x06, 0x49, 0x49, 0x29, 0x1E],
            'A' => [0x7E, 0x11, 0x11, 0x11, 0x7E],
            'B' => [0x7F, 0x49, 0x49, 0x49, 0x36],
            'C' => [0x3E, 0x41, 0x41, 0x41, 0x22],
            'D' => [0x7F, 0x41, 0x41, 0x22, 0x1C],
            'E' => [0x7F, 0x49, 0x49, 0x49, 0x41],
            'F' => [0x7F, 0x09, 0x09, 0x09, 0x01],
            'G' => [0x3E, 0x41, 0x49, 0x49, 0x7A],
            'H' => [0x7F, 0x08, 0x08, 0x08, 0x7F],
            'I' => [0x00, 0x41, 0x7F, 0x41, 0x00],
            'J' => [0x20, 0x40, 0x41, 0x3F, 0x01],
            'K' => [0x7F, 0x08, 0x14, 0x22, 0x41],
            'L' => [0x7F, 0x40, 0x40, 0x40, 0x40],
            'M' => [0x7F, 0x02, 0x0C, 0x02, 0x7F],
            'N' => [0x7F, 0x04, 0x08, 0x10, 0x7F],
            'O' => [0x3E, 0x41, 0x41, 0x41, 0x3E],
            'P' => [0x7F, 0x09, 0x09, 0x09, 0x06],
            'Q' => [0x3E, 0x41, 0x51, 0x21, 0x5E],
            'R' => [0x7F, 0x09, 0x19, 0x29, 0x46],
            'S' => [0x46, 0x49, 0x49, 0x49, 0x31],
            'T' => [0x01, 0x01, 0x7F, 0x01, 0x01],
            'U' => [0x3F, 0x40, 0x40, 0x40, 0x3F],
            'V' => [0x1F, 0x20, 0x40, 0x20, 0x1F],
            'W' => [0x3F, 0x40, 0x38, 0x40, 0x3F],
            'X' => [0x63, 0x14, 0x08, 0x14, 0x63],
            'Y' => [0x07, 0x08, 0x70, 0x08, 0x07],
            'Z' => [0x61, 0x51, 0x49, 0x45, 0x43],
            _ => [0x7F, 0x41, 0x41, 0x41, 0x7F],
        }
    }

    /// Rendered width of `text` in output pixels at scale `s`.
    fn text_width_px(text: &str, s: AnnoScale) -> usize {
        text.chars().count() * 6 * s.font()
    }

    /// Draw `text` with its top-left at `pos`, over a paper-coloured pad so
    /// it stays legible on part fills.
    fn draw_text(
        rgb: &mut [u8],
        mask: &mut [u8],
        canvas: Canvas,
        pos: (f64, f64),
        text: &str,
        color: [u8; 3],
        s: AnnoScale,
    ) {
        let font = s.font();
        let x0 = pos.0.round() as i64;
        let y0 = pos.1.round() as i64;
        let w = text_width_px(text, s) as i64;
        let h = (7 * font) as i64;
        let pad = s.i(2.0);
        // Halo pad — opaque so the label stays legible over the transparent
        // PNG background as well as over part fills.
        for py in (y0 - pad)..(y0 + h + pad) {
            for px in (x0 - pad)..(x0 + w + pad) {
                if !canvas.contains(px, py) {
                    continue;
                }
                let cell = canvas.idx(px as usize, py as usize);
                let qi = cell * 3;
                rgb[qi] = BACKGROUND[0];
                rgb[qi + 1] = BACKGROUND[1];
                rgb[qi + 2] = BACKGROUND[2];
                mask[cell] = 255;
            }
        }
        for (ci, c) in text.chars().enumerate() {
            let cols = glyph5x7(c);
            let gx = x0 + (ci * 6 * font) as i64;
            for (col, bits) in cols.iter().enumerate() {
                for row in 0..7 {
                    if bits & (1 << row) == 0 {
                        continue;
                    }
                    for sy in 0..font {
                        for sx in 0..font {
                            let px = gx + (col * font + sx) as i64;
                            let py = y0 + (row * font + sy) as i64;
                            if !canvas.contains(px, py) {
                                continue;
                            }
                            let cell = canvas.idx(px as usize, py as usize);
                            let qi = cell * 3;
                            rgb[qi] = color[0];
                            rgb[qi + 1] = color[1];
                            rgb[qi + 2] = color[2];
                            mask[cell] = 255;
                        }
                    }
                }
            }
        }
    }

    /// Draw `text` centred on `pos`.
    #[allow(clippy::too_many_arguments)]
    fn draw_text_centered(
        rgb: &mut [u8],
        mask: &mut [u8],
        canvas: Canvas,
        pos: (f64, f64),
        text: &str,
        color: [u8; 3],
        s: AnnoScale,
    ) {
        let w = text_width_px(text, s) as f64;
        draw_text(
            rgb,
            mask,
            canvas,
            (pos.0 - w / 2.0, pos.1 - s.text_h() / 2.0),
            text,
            color,
            s,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_edge(
        rgb: &mut [u8],
        zbuf: &[f64],
        mask: &mut [u8],
        canvas: Canvas,
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
            if !canvas.contains(ix, iy) {
                continue;
            }
            let (ux, uy) = (ix as usize, iy as usize);
            let idx = canvas.idx(ux, uy);
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
                    if canvas.contains(nx, ny) {
                        let nz = zbuf[canvas.idx(nx as usize, ny as usize)];
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
            // Paint a stroke_px × stroke_px block *centred* on the sample.
            // Anchoring it right/down (as it once was) offsets linework
            // from its geometry by half the stroke — invisible at 2px,
            // a clear 4px misregistration at the 4096px stroke weight.
            let half = stroke_px as i64 / 2;
            for dy in 0..stroke_px as i64 {
                for dx in 0..stroke_px as i64 {
                    let (qx, qy) = (ix + dx - half, iy + dy - half);
                    if !canvas.contains(qx, qy) {
                        continue;
                    }
                    let pi = canvas.idx(qx as usize, qy as usize);
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

// ─── raytrace path (feature-gated) ────────────────────────────────────────

#[cfg(feature = "raytrace")]
pub(crate) use raytrace::render_raytrace_png_solids_framed;
#[cfg(feature = "raytrace")]
pub use raytrace::{render_raytrace_jpeg_str, render_raytrace_png_str};

/// Direct BRep ray tracing (`--raytrace`): pixel-perfect raster output with
/// no tessellation anywhere in the pipeline. Analytic ray–surface
/// intersection (via `vcad-kernel-raytrace`'s SAH BVH and trimmed-face
/// tests) means curved silhouettes are exact at any resolution — no facet
/// banding, no segment count to tune.
///
/// The camera is the same orthographic [`View`] basis and framing math as
/// the tessellated raster path, and shading samples the same vcad-Blue
/// tonal ramp ([`RAMP`], tinted by document material colours), so the two
/// paths are drop-in alternatives that read as the same house style.
#[cfg(feature = "raytrace")]
mod raytrace {
    use super::raster::{canvas_for, encode_jpeg, encode_png, fit_scale, Frame, BACKGROUND};
    use super::*;
    use vcad_kernel::vcad_kernel_math::{Point3, Vec3};
    use vcad_kernel_raytrace::{Bvh, Instance, Ray, Tlas};

    /// Stratified supersampling grid per pixel (N × N rays). Analytic
    /// intersection makes interior shading perfectly smooth already; the
    /// samples exist to anti-alias silhouettes.
    const SS_GRID: u32 = 2;

    /// Render raw `.vcad` document JSON to a ray-traced JPEG.
    ///
    /// Same options, framing, and tonal ramp as
    /// [`render_jpeg_str`](super::render_jpeg_str), but every pixel is an
    /// analytic ray–BRep intersection instead of a tessellated triangle.
    pub fn render_raytrace_jpeg_str(
        raw_vcad: &str,
        opts: &RasterOptions,
    ) -> Result<Vec<u8>, String> {
        encode_jpeg(rasterize_rt(raw_vcad, opts, false)?, opts)
    }

    /// Render raw `.vcad` document JSON to a ray-traced RGBA PNG with a fully
    /// transparent background (alpha 0 where no surface was hit) — the raster
    /// analogue of the tessellated [`render_png_str`](super::render_png_str).
    /// See [`render_raytrace_jpeg_str`].
    pub fn render_raytrace_png_str(
        raw_vcad: &str,
        opts: &RasterOptions,
    ) -> Result<Vec<u8>, String> {
        encode_png(rasterize_rt(raw_vcad, opts, true)?, opts)
    }

    /// Trace the scene to an RGB frame plus a per-pixel coverage mask (255
    /// where a surface was hit, fractional at supersampled silhouette edges,
    /// 0 over background). The `png` flavour keeps hit pixels at their pure
    /// geometry colour (straight-alpha transparency); the JPEG flavour
    /// composites coverage over the opaque vellum background. Both share the
    /// tessellated path's `encode_jpeg` / `encode_png`.
    fn rasterize_rt(raw_vcad: &str, opts: &RasterOptions, png: bool) -> Result<Frame, String> {
        let tinted = evaluate_vcad(raw_vcad)?;
        rasterize_rt_scene(&tinted, opts, png, None)
    }

    /// Ray-trace already-placed scene solids to RGBA PNG bytes with a
    /// camera pinned by `framing` — the `--raytrace --animate` per-frame
    /// entry point. See [`FixedFraming`](super::FixedFraming).
    pub(crate) fn render_raytrace_png_solids_framed(
        scene: &[SceneSolid],
        opts: &RasterOptions,
        framing: &FixedFraming,
    ) -> Result<Vec<u8>, String> {
        encode_png(rasterize_rt_scene(scene, opts, true, Some(framing))?, opts)
    }

    /// The body of [`rasterize_rt`], over an already-evaluated scene so an
    /// animation can re-pose the same document without re-evaluating it,
    /// and with an optional pinned camera.
    fn rasterize_rt_scene(
        tinted: &[SceneSolid],
        opts: &RasterOptions,
        png: bool,
        framing: Option<&FixedFraming>,
    ) -> Result<Frame, String> {
        if tinted.is_empty() {
            return Err("no solids produced".to_string());
        }
        if opts.size_px < 16 {
            return Err("size_px too small".to_string());
        }
        if !(opts.fill_frac > 0.0 && opts.fill_frac <= 1.0) {
            return Err("fill_frac must be in (0, 1]".to_string());
        }

        // One BLAS per solid, gathered under a TLAS so a ray only descends
        // into the parts whose bounds it actually crosses (the old linear scan
        // cost O(parts) per ray). Assembly instances arrive from
        // `evaluate_vcad` already world-placed, so every instance here sits at
        // the identity. BRep-backed solids trace analytically; mesh-only ones
        // (frozen topology-optimization results, imported STL/GLB parts) trace
        // as triangles, crease-baked so they read smooth next to the analytic
        // surfaces rather than faceted.
        let mut ramps: Vec<Option<[[u8; 3]; 4]>> = Vec::new();
        let mut instances = Vec::new();
        let mut untraceable: Vec<String> = Vec::new();
        for s in tinted {
            let bvh = match s.solid.as_brep() {
                Some(brep) => Bvh::build(brep),
                None => {
                    let mut mesh = s.solid.to_mesh(0);
                    vcad_kernel::vcad_kernel_tessellate::render_bake_default(&mut mesh);
                    Bvh::build_mesh(&mesh)
                }
            };
            // `Instance::identity` rejects an empty BLAS, which is exactly the
            // `bvh.root().is_none()` filter this replaced: an empty solid, or
            // a mesh whose triangles were all degenerate. Name it rather than
            // dropping it on the floor.
            let Some(inst) = Instance::identity(std::sync::Arc::new(bvh), ramps.len()) else {
                untraceable.push(s.name.clone().unwrap_or_else(|| s.id.clone()));
                continue;
            };
            instances.push(inst);
            ramps.push(s.tint.map(tint_ramp));
        }
        if instances.is_empty() {
            return Err(format!(
                "raytrace: document produced no traceable geometry ({} part(s) empty \
                 or degenerate: {})",
                untraceable.len(),
                untraceable.join(", ")
            ));
        }
        if !untraceable.is_empty() {
            // Fail closed rather than silently rendering a subset: a missing
            // part in a render reads as a design that doesn't have it, which
            // is worse than no render at all.
            return Err(format!(
                "raytrace: {} part(s) have no traceable geometry (empty or \
                 fully degenerate): {}",
                untraceable.len(),
                untraceable.join(", ")
            ));
        }
        let scene = Tlas::build(instances);

        // Framing: project the union of the BVH root AABBs onto the view
        // basis — same fill/centre math as the tessellated raster path.
        let cam = opts.view.cam();
        let right = normalize(opts.view.right());
        let down = normalize(opts.view.down());
        let light = normalize(LIGHT);

        let mut min = [f64::INFINITY; 2];
        let mut max = [f64::NEG_INFINITY; 2];
        let mut dmin = f64::INFINITY;
        let mut dmax = f64::NEG_INFINITY;
        for inst in scene.instances() {
            let aabb = inst.world_aabb();
            for i in 0..8 {
                let c = [
                    if i & 1 == 0 { aabb.min.x } else { aabb.max.x },
                    if i & 2 == 0 { aabb.min.y } else { aabb.max.y },
                    if i & 4 == 0 { aabb.min.z } else { aabb.max.z },
                ];
                let s = [dot(c, right), dot(c, down)];
                for k in 0..2 {
                    min[k] = min[k].min(s[k]);
                    max[k] = max[k].max(s[k]);
                }
                let d = dot(c, cam);
                dmin = dmin.min(d);
                dmax = dmax.max(d);
            }
        }
        // A pinned camera (animation) overrides the self-framing above, so
        // every frame of a sequence shares one canvas, scale, and depth cue.
        if let Some(f) = framing {
            min = f.screen_min;
            max = f.screen_max;
            (dmin, dmax) = f.depth;
        }
        let extent = (max[0] - min[0]).max(max[1] - min[1]);
        if !extent.is_finite() || extent < 1e-9 {
            return Err("degenerate projection (no extent)".to_string());
        }
        let dspan = (dmax - dmin).max(1e-9);

        let canvas = canvas_for(opts, max[0] - min[0], max[1] - min[1]);
        let px_per_mm = fit_scale(
            opts.fill_frac,
            canvas,
            max[0] - min[0],
            max[1] - min[1],
            extent,
        );
        let cx = (min[0] + max[0]) / 2.0;
        let cy = (min[1] + max[1]) / 2.0;
        let (halfx, halfy) = (canvas.w as f64 / 2.0, canvas.h as f64 / 2.0);

        // Orthographic rays: start on a plane just past the scene along
        // the camera direction, fire straight down it.
        let d0 = dmax + dspan + 1.0;
        let dir = Vec3::new(-cam[0], -cam[1], -cam[2]);

        let mut rgb: Vec<u8> = BACKGROUND
            .iter()
            .copied()
            .cycle()
            .take(canvas.len() * 3)
            .collect();
        let mut mask: Vec<u8> = vec![0u8; canvas.len()];

        let n = (SS_GRID * SS_GRID) as f64;
        for py in 0..canvas.h {
            for px in 0..canvas.w {
                // Sum only the sub-samples that hit a surface, and count
                // them: `hits/n` is the pixel's coverage.
                let mut acc = [0.0f64; 3];
                let mut hits = 0u32;
                for sy in 0..SS_GRID {
                    for sx in 0..SS_GRID {
                        let fx = px as f64 + (sx as f64 + 0.5) / SS_GRID as f64;
                        let fy = py as f64 + (sy as f64 + 0.5) / SS_GRID as f64;
                        let sx_mm = (fx - halfx) / px_per_mm + cx;
                        let sy_mm = (fy - halfy) / px_per_mm + cy;
                        let origin = Point3::new(
                            right[0] * sx_mm + down[0] * sy_mm + cam[0] * d0,
                            right[1] * sx_mm + down[1] * sy_mm + cam[1] * d0,
                            right[2] * sx_mm + down[2] * sy_mm + cam[2] * d0,
                        );
                        let ray = Ray::new(origin, dir);

                        let shaded = scene.trace_closest(&ray).map(|found| {
                            shade_hit(&found.hit, &ramps[found.payload], cam, light, dmin, dspan)
                        });
                        if let Some(c) = shaded {
                            for k in 0..3 {
                                acc[k] += c[k] as f64;
                            }
                            hits += 1;
                        }
                    }
                }
                let pi = canvas.idx(px, py);
                let idx = pi * 3;
                if hits == 0 {
                    continue; // leave background rgb, mask 0 (transparent)
                }
                mask[pi] = ((hits as f64 / n) * 255.0).round() as u8;
                for k in 0..3 {
                    rgb[idx + k] = if png {
                        // Straight-alpha: pure geometry colour, coverage in α.
                        (acc[k] / hits as f64).round() as u8
                    } else {
                        // Composite coverage over the opaque vellum ground.
                        ((acc[k] + (n - hits as f64) * BACKGROUND[k] as f64) / n).round() as u8
                    };
                }
            }
        }

        Ok(Frame { rgb, mask, canvas })
    }

    /// Shade an analytic hit with the same terms as the tessellated raster
    /// path: face-forward normal, Lambertian key light, mild depth cue,
    /// tonal-ramp (tinted) or two-stop monochrome (untinted) colour.
    fn shade_hit(
        hit: &vcad_kernel_raytrace::RayHit,
        ramp: &Option<[[u8; 3]; 4]>,
        cam: [f64; 3],
        light: [f64; 3],
        dmin: f64,
        dspan: f64,
    ) -> [u8; 3] {
        let mut n = [hit.normal.x, hit.normal.y, hit.normal.z];
        if dot(n, cam) < 0.0 {
            n = [-n[0], -n[1], -n[2]];
        }
        let p = [hit.point.x, hit.point.y, hit.point.z];
        let cue = 0.78 + 0.22 * ((dot(p, cam) - dmin) / dspan).clamp(0.0, 1.0);
        let lit = (lambertian(n, light) * cue).clamp(0.0, 1.0);
        match ramp {
            Some(r) => ramp_sample(r, lit),
            None => mix_rgb(FILL_DARK, FILL_LIGHT, lit),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A world transform must rotate the geometry about the transform's own
    /// origin and *then* translate — never rotate the translation itself.
    ///
    /// Regression: the chain was built scale→Rx→Ry→Rz→T, but
    /// `Transform::then` composes `self·other` (other acts first), so the
    /// translation was applied first and the rotation then swung it about
    /// the world origin. A revolute child anchored at z=94 detached from its
    /// parent and orbited the origin at radius 94 — invisible at state 0,
    /// where the rotation is identity.
    #[test]
    fn world_transform_rotates_about_its_own_origin() {
        let t = vcad_ir::Transform3D {
            translation: vcad_ir::Vec3::new(0.0, 0.0, 94.0),
            rotation: vcad_ir::Vec3::new(0.0, -30.0, 0.0),
            scale: vcad_ir::Vec3::new(1.0, 1.0, 1.0),
        };
        let k = transform3d_to_kernel(&t);

        // The transform's origin is the pivot: it maps to the translation.
        let pivot = k.apply_point(&vcad_kernel::vcad_kernel_math::Point3::new(0.0, 0.0, 0.0));
        assert!(
            (pivot.x).abs() < 1e-9 && (pivot.z - 94.0).abs() < 1e-9,
            "pivot moved: {pivot:?}"
        );

        // A point 58mm below the pivot swings to R·(0,0,-58) + (0,0,94).
        let tip = k.apply_point(&vcad_kernel::vcad_kernel_math::Point3::new(0.0, 0.0, -58.0));
        let (s, c) = (-30.0_f64).to_radians().sin_cos();
        assert!((tip.x - (-58.0 * s)).abs() < 1e-9, "tip.x = {}", tip.x);
        assert!(
            (tip.z - (-58.0 * c + 94.0)).abs() < 1e-9,
            "tip.z = {}",
            tip.z
        );
    }

    /// Pins the handedness of every [`View`]'s screen basis.
    ///
    /// For a camera looking at the scene, `right × down` must point *into*
    /// the screen, i.e. along `−cam` (`cam` points scene → camera). A basis
    /// where it points along `+cam` renders a mirror image — invisible on
    /// flat-shaded line art, obvious once shadows or reflections exist.
    ///
    /// `View::Isometric` was mirrored until 2026-07 (`right = [COS30,
    /// −COS30, 0]`), which mirrored every default render — docs assets,
    /// mecheval leaderboard, MCP `render_view`. Every view is non-mirrored
    /// now; a `+1` here means someone flipped a basis by accident.
    #[test]
    fn view_basis_handedness_is_pinned() {
        // right × down points INTO the screen for every view.
        let cases: &[(View, f64)] = &[
            (View::Isometric, -1.0),
            (View::Front, -1.0),
            (View::Side, -1.0),
            (View::Top, -1.0),
            (
                View::Orbit {
                    azimuth: 45.0,
                    elevation: 35.0,
                },
                -1.0,
            ),
            (
                View::Orbit {
                    azimuth: -117.0,
                    elevation: -12.0,
                },
                -1.0,
            ),
            (
                View::Orbit {
                    azimuth: 0.0,
                    elevation: 90.0,
                },
                -1.0,
            ),
        ];

        for &(view, want_sign) in cases {
            let n = normalize(cross(view.right(), view.down()));
            let cam = normalize(view.cam());
            // n must be parallel (or antiparallel) to cam: |dot| ≈ 1.
            let dot = n[0] * cam[0] + n[1] * cam[1] + n[2] * cam[2];
            assert!(
                (dot.abs() - 1.0).abs() < 1e-9,
                "{view:?}: right × down is not parallel to cam (dot = {dot})",
            );
            assert!(
                (dot - want_sign).abs() < 1e-9,
                "{view:?}: handedness flipped — right × down · cam = {dot}, expected {want_sign}",
            );
        }
    }

    #[test]
    fn render_style_parses_known_names_and_rejects_unknown() {
        assert_eq!("drafting".parse::<RenderStyle>(), Ok(RenderStyle::Drafting));
        assert_eq!("".parse::<RenderStyle>(), Ok(RenderStyle::Drafting));
        assert_eq!("Shaded".parse::<RenderStyle>(), Ok(RenderStyle::Shaded));
        let err = "raytrace".parse::<RenderStyle>().unwrap_err();
        assert!(err.contains("unknown render style"), "{err}");
        assert!(err.contains("shaded"), "{err}");
    }

    #[test]
    fn shaded_ramp_uses_full_material_color() {
        // Achromatic steel grey: drafting keeps navy (blue-dominant stops),
        // shaded must produce true greys (r ≈ g ≈ b at every stop).
        let grey = [0.5, 0.5, 0.5];
        let drafting = tint_ramp(grey);
        let shaded = shaded_ramp(grey);
        assert!(
            drafting.iter().any(|s| s[2] > s[0] + 10),
            "drafting ramp should stay navy for achromatic materials: {drafting:?}"
        );
        for stop in shaded {
            let (r, g, b) = (stop[0] as i32, stop[1] as i32, stop[2] as i32);
            assert!(
                (r - g).abs() <= 2 && (g - b).abs() <= 2,
                "shaded ramp for grey must be grey: {stop:?}"
            );
        }
    }

    #[test]
    fn shaded_style_renders_material_color_svg() {
        let doc = cube_vcad(10.0, 10.0, 10.0);
        let drafting = render_svg_str_opts(&doc, 2.0, &SvgOptions::default()).unwrap();
        let shaded = render_svg_str_opts(
            &doc,
            2.0,
            &SvgOptions {
                style: RenderStyle::Shaded,
                ..Default::default()
            },
        )
        .unwrap();
        assert_ne!(drafting, shaded, "shaded style must change the output");
    }

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

    /// Two disjoint copper cubes: root node 1 ("base") at the origin and
    /// root node 3 ("lid") translated clear of it — the minimal highlight
    /// fixture.
    fn two_cube_vcad() -> String {
        r#"{
  "version": "0.1",
  "nodes": {
    "1": { "id": 1, "name": "base", "op": { "type": "Cube", "size": { "x": 20, "y": 20, "z": 10 } } },
    "2": { "id": 2, "name": null, "op": { "type": "Cube", "size": { "x": 20, "y": 20, "z": 10 } } },
    "3": { "id": 3, "name": "lid", "op": { "type": "Translate", "child": 2, "offset": { "x": 40, "y": 0, "z": 0 } } }
  },
  "materials": {
    "copper": {
      "name": "copper",
      "color": [0.72, 0.45, 0.2],
      "metallic": 1.0,
      "roughness": 0.4,
      "density": 8960.0,
      "friction": 0.6
    }
  },
  "part_materials": {},
  "roots": [
    { "root": 1, "material": "copper" },
    { "root": 3, "material": "copper" }
  ]
}"#
        .to_string()
    }

    /// [`SvgOptions`] selecting the given highlight ids on the isometric view.
    fn highlight_opts(ids: &[&str]) -> SvgOptions {
        SvgOptions {
            view: View::Isometric,
            highlight: ids.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn highlight_strokes_accent_and_ghosts_the_rest() {
        let vcad = two_cube_vcad();
        let plain = render_svg_str_opts(&vcad, DEFAULT_SCALE, &highlight_opts(&[])).unwrap();
        assert!(
            !plain.contains(ACCENT),
            "no accent stroke without a highlight set"
        );

        // Highlight by root node id — the `part_id` a mutation diff reports.
        let hl = render_svg_str_opts(&vcad, DEFAULT_SCALE, &highlight_opts(&["3"]))
            .expect("highlighted render");
        assert!(
            hl.contains(&format!(
                r#"stroke="{ACCENT}" stroke-width="{STROKE_ACCENT_PX}""#
            )),
            "highlighted part must carry the brand-orange accent outline"
        );
        // The non-highlighted part is ghosted: its fills fade toward paper
        // (colours the plain render never emits) and its visible linework
        // drops to the ghost opacity.
        assert!(
            hl.contains(&format!(r#"opacity="{GHOST_LINE_OPACITY}""#)),
            "ghosted part's linework must fade"
        );
        let ghosted_fill = hex(ghost_ramp(tint_ramp([0.72, 0.45, 0.2]))[1]);
        assert!(
            hl.contains(&ghosted_fill) && !plain.contains(&ghosted_fill),
            "ghosted fills must fade toward paper (expected {ghosted_fill})"
        );
    }

    #[test]
    fn highlight_matches_by_node_name() {
        let hl = render_svg_str_opts(&two_cube_vcad(), DEFAULT_SCALE, &highlight_opts(&["lid"]))
            .expect("name-matched highlight render");
        assert!(hl.contains(ACCENT));
    }

    #[test]
    fn highlight_with_no_match_is_an_error() {
        let err = render_svg_str_opts(
            &two_cube_vcad(),
            DEFAULT_SCALE,
            &highlight_opts(&["no-such-part"]),
        )
        .unwrap_err();
        assert!(
            err.contains("highlight matched no parts") && err.contains("base"),
            "error must list the document's parts, got: {err}"
        );
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

    /// Two named parts: a 20×30×10 bracket plus a small pin inside its
    /// footprint (so the overall bbox is exactly the bracket's).
    fn two_part_vcad() -> &'static str {
        r#"{
  "version": "0.1",
  "nodes": {
    "1": { "id": 1, "name": "Bracket", "op": { "type": "Cube", "size": { "x": 20.0, "y": 30.0, "z": 10.0 } } },
    "2": { "id": 2, "name": "Pin", "op": { "type": "Cube", "size": { "x": 5.0, "y": 5.0, "z": 5.0 } } }
  },
  "materials": {},
  "part_materials": {},
  "roots": [ { "root": 1, "material": "default" }, { "root": 2, "material": "default" } ]
}"#
    }

    /// Default (all-off) annotations must not change the output at all.
    /// Edge emission order comes from a HashMap and is not run-stable, so
    /// compare the canonicalized element set rather than raw bytes.
    #[test]
    fn default_annotations_are_a_no_op() {
        let canon = |svg: &str| {
            let mut elems: Vec<&str> = svg.split('<').collect();
            elems.sort_unstable();
            elems.join("<")
        };
        let plain = render_svg_str_view(two_part_vcad(), 2.0, View::Isometric).unwrap();
        let opts = render_svg_str_view_opts(
            two_part_vcad(),
            2.0,
            View::Isometric,
            false,
            &RenderAnnotations::default(),
        )
        .unwrap();
        assert_eq!(canon(&plain), canon(&opts));
        assert!(!opts.contains("annot-"), "no annotation groups by default");
        assert!(!opts.contains("<text"), "no text elements by default");
    }

    /// Labels + dims on a two-part doc: the SVG must carry both part names
    /// and the correct overall W×D×H mm values.
    #[test]
    fn labels_and_dims_annotate_two_part_doc() {
        let svg = render_svg_str_view_opts(
            two_part_vcad(),
            2.0,
            View::Isometric,
            false,
            &RenderAnnotations {
                labels: true,
                dims: true,
                axes: false,
            },
        )
        .unwrap();
        for name in ["Bracket", "Pin"] {
            assert!(
                svg.contains(&format!(">{name}</text>")),
                "missing label {name}"
            );
        }
        for dim in ["W 20 mm", "D 30 mm", "H 10 mm"] {
            assert!(svg.contains(&format!(">{dim}</text>")), "missing dim {dim}");
        }
        // Annotation margin: the canvas grows beyond the plain render.
        let plain = render_svg_str_view(two_part_vcad(), 2.0, View::Isometric).unwrap();
        assert!(svg.len() > plain.len());
    }

    /// The axes gizmo emits all three axis arrows in an isometric view, and
    /// drops the view-parallel axis in an orthographic one (front looks
    /// down +Y, so Y projects to nothing).
    #[test]
    fn axes_gizmo_projects_per_view() {
        let annos = RenderAnnotations {
            axes: true,
            ..Default::default()
        };
        let iso =
            render_svg_str_view_opts(two_part_vcad(), 2.0, View::Isometric, false, &annos).unwrap();
        for a in [">X</text>", ">Y</text>", ">Z</text>"] {
            assert!(iso.contains(a), "iso gizmo missing {a}");
        }
        let front =
            render_svg_str_view_opts(two_part_vcad(), 2.0, View::Front, false, &annos).unwrap();
        assert!(front.contains(">X</text>") && front.contains(">Z</text>"));
        assert!(
            !front.contains(">Y</text>"),
            "front view must drop the view-parallel Y axis"
        );
    }

    /// Fractional extents keep one decimal in the dim label.
    #[test]
    fn dim_labels_format_fractional_mm() {
        assert_eq!(format_mm(20.0), "20");
        assert_eq!(format_mm(12.5), "12.5");
        let svg = render_svg_str_view_opts(
            &cube_vcad(12.5, 8.0, 3.0),
            2.0,
            View::Isometric,
            false,
            &RenderAnnotations {
                dims: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(svg.contains(">W 12.5 mm</text>"));
        assert!(svg.contains(">D 8 mm</text>"));
        assert!(svg.contains(">H 3 mm</text>"));
    }

    #[test]
    fn renders_cube_to_svg() {
        let svg = render_svg_str(&cube_vcad(20.0, 30.0, 10.0), DEFAULT_SCALE)
            .expect("cube should render");
        assert!(svg.starts_with("<svg "));
        assert!(svg.ends_with("</svg>"));
        assert!(svg.contains("<polygon"));
        assert!(svg.contains("<line "));
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

    /// Two named cubes far apart; used by the focus tests.
    fn two_named_cube_vcad() -> String {
        r#"{
  "version": "0.1",
  "nodes": {
    "1": { "id": 1, "name": "left_cube", "op": { "type": "Cube", "size": { "x": 10.0, "y": 10.0, "z": 10.0 } } },
    "2": { "id": 2, "name": "right_cube", "op": { "type": "Translate", "child": 3, "offset": { "x": 200.0, "y": 0.0, "z": 0.0 } } },
    "3": { "id": 3, "name": "right_cube_base", "op": { "type": "Cube", "size": { "x": 10.0, "y": 10.0, "z": 10.0 } } }
  },
  "materials": {},
  "part_materials": {},
  "roots": [
    { "root": 1, "material": "default" },
    { "root": 2, "material": "default" }
  ]
}"#
        .to_string()
    }

    #[test]
    fn parses_orbit_views() {
        use std::str::FromStr;
        assert_eq!(
            View::from_str("orbit:35,25").unwrap(),
            View::Orbit {
                azimuth: 35.0,
                elevation: 25.0
            }
        );
        assert_eq!(
            View::from_str("ORBIT: -120 , 45.5 ").unwrap(),
            View::Orbit {
                azimuth: -120.0,
                elevation: 45.5
            }
        );
        assert!(View::from_str("orbit:35").is_err());
        assert!(View::from_str("orbit:a,b").is_err());
    }

    /// The orbit basis must be orthonormal and non-mirrored (right × down
    /// points into the screen, i.e. −cam) at arbitrary angles, including
    /// the elevation ±90 poles.
    #[test]
    fn orbit_basis_is_orthonormal() {
        for (az, el) in [(35.0, 25.0), (-120.0, 45.5), (0.0, 90.0), (10.0, -90.0)] {
            let v = View::Orbit {
                azimuth: az,
                elevation: el,
            };
            let (cam, right, down) = (v.cam(), v.right(), v.down());
            for (a, b) in [(cam, right), (cam, down), (right, down)] {
                assert!(
                    dot(a, b).abs() < 1e-9,
                    "orbit({az},{el}) basis not orthogonal"
                );
            }
            let into = cross(right, down);
            for i in 0..3 {
                assert!(
                    (into[i] + cam[i]).abs() < 1e-9,
                    "orbit({az},{el}) basis is mirrored"
                );
            }
        }
    }

    #[test]
    fn orbit_view_renders_and_differs() {
        let doc = cube_vcad(20.0, 30.0, 10.0);
        let orbit = render_svg_str_view(
            &doc,
            2.0,
            View::Orbit {
                azimuth: 60.0,
                elevation: 20.0,
            },
        )
        .expect("orbit view should render");
        assert!(orbit.starts_with("<svg "));
        let front = render_svg_str_view(&doc, 2.0, View::Front).unwrap();
        assert_ne!(orbit, front);
    }

    #[test]
    fn focus_frames_named_part() {
        let attr = |svg: &str, name: &str| -> f64 {
            let pat = format!("{name}=\"");
            let start = svg.find(&pat).unwrap() + pat.len();
            let end = svg[start..].find('"').unwrap() + start;
            svg[start..end].parse().unwrap()
        };
        let full = render_svg_str(&two_named_cube_vcad(), 2.0).unwrap();
        let focused = render_svg_str_camera(
            &two_named_cube_vcad(),
            2.0,
            &CameraOptions {
                focus: Some("LEFT_CUBE".to_string()), // case-insensitive
                ..Default::default()
            },
        )
        .expect("focused render should succeed");
        // The two cubes are 200mm apart; framing on one must shrink the
        // canvas to roughly a single 10mm cube's projection.
        assert!(
            attr(&focused, "width") < attr(&full, "width") / 4.0,
            "focused width {} should be far below full width {}",
            attr(&focused, "width"),
            attr(&full, "width"),
        );
    }

    #[test]
    fn focus_unknown_part_errors_with_labels() {
        let err = render_svg_str_camera(
            &two_named_cube_vcad(),
            2.0,
            &CameraOptions {
                focus: Some("nope".to_string()),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(err.contains("not found"), "err: {err}");
        assert!(err.contains("left_cube"), "err should list labels: {err}");
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

        /// Width and height from a PNG's IHDR chunk.
        fn png_dims(png: &[u8]) -> (u32, u32) {
            let n = |at: usize| u32::from_be_bytes(png[at..at + 4].try_into().unwrap());
            (n(16), n(20))
        }

        /// Count of pixels exactly matching `want`.
        fn count_rgb(png: &[u8], want: [u8; 3]) -> usize {
            image::load_from_memory(png)
                .expect("valid PNG")
                .to_rgba8()
                .pixels()
                .filter(|p| p.0[0] == want[0] && p.0[1] == want[1] && p.0[2] == want[2])
                .count()
        }

        /// The overlay's pixel constants used to be bare pixel counts, so the
        /// gizmo, glyphs and leaders were a fixed size no matter how big the
        /// canvas — legible at 1024px, a speck at 4096px. Scaled, the gizmo
        /// covers a constant *fraction* of the canvas, so doubling the
        /// canvas roughly quadruples its pixel count. The X axis' red is
        /// unique to the gizmo, which makes it a clean thing to count.
        #[test]
        fn annotation_overlay_scales_with_the_canvas() {
            let doc = cube_vcad(30.0, 30.0, 30.0);
            let ink = |size| {
                let png = render_png_str(
                    &doc,
                    &RasterOptions {
                        size_px: size,
                        annotations: RenderAnnotations {
                            axes: true,
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                )
                .unwrap();
                count_rgb(&png, super::super::raster::AXIS_RGB[0])
            };
            let (small, big) = (ink(400), ink(800));
            assert!(small > 20, "gizmo should be drawn at 400px, got {small} px");
            let growth = big as f64 / small as f64;
            assert!(
                growth > 2.5,
                "doubling the canvas should roughly quadruple the gizmo's area; \
                 it grew {growth:.2}x ({small} → {big} px)"
            );
        }

        /// A part label used to anchor its leader at the centre of the
        /// part's projected bounding box, which planted the dot — and, for a
        /// single-part document, the text plate — right on the middle of the
        /// geometry being named. The leader now starts on the silhouette, so
        /// the centre of the render must come through untouched.
        #[test]
        fn part_label_keeps_off_the_middle_of_the_part() {
            // A *single* named part, so the part's projected centre is the
            // canvas centre — exactly the case the old centre-anchored
            // leader drew straight through. A two-part document would pass
            // either way and prove nothing.
            let doc = cube_vcad(30.0, 30.0, 30.0);
            let render = |labels| {
                let png = render_png_str(
                    &doc,
                    &RasterOptions {
                        size_px: 400,
                        annotations: RenderAnnotations {
                            labels,
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                )
                .unwrap();
                image::load_from_memory(&png).expect("valid PNG").to_rgba8()
            };
            let plain = render(false);
            let labelled = render(true);
            let (cx, cy) = (plain.width() / 2, plain.height() / 2);
            for y in (cy - 6)..=(cy + 6) {
                for x in (cx - 6)..=(cx + 6) {
                    assert_eq!(
                        plain.get_pixel(x, y),
                        labelled.get_pixel(x, y),
                        "label overlay painted over the centre of the part at ({x}, {y})"
                    );
                }
            }
            // …and it did draw something, somewhere.
            let changed = plain
                .pixels()
                .zip(labelled.pixels())
                .filter(|(a, b)| a != b)
                .count();
            assert!(changed > 50, "labels drew almost nothing ({changed} px)");
        }

        /// Every distinct alpha value in an RGBA PNG.
        fn alphas(png: &[u8]) -> std::collections::BTreeSet<u8> {
            image::load_from_memory(png)
                .expect("valid PNG")
                .to_rgba8()
                .pixels()
                .map(|p| p.0[3])
                .collect()
        }

        /// Supersampling is what anti-aliases the tessellated path: at
        /// `aa: 1` a pixel is point-sampled, so coverage is all-or-nothing
        /// and a curved silhouette staircases. Above 1 the box filter has
        /// to produce partial coverage along that silhouette.
        #[test]
        fn supersampling_gives_silhouettes_partial_coverage() {
            let doc = cylinder_vcad(20.0, 8.0);
            let opts = |aa| RasterOptions {
                size_px: 192,
                aa: Some(aa),
                ..Default::default()
            };
            let flat = alphas(&render_png_str(&doc, &opts(1)).unwrap());
            let aa = alphas(&render_png_str(&doc, &opts(3)).unwrap());

            assert_eq!(
                flat,
                [0, 255].into_iter().collect(),
                "aa:1 must stay point-sampled (binary coverage), got {} levels",
                flat.len()
            );
            let partial = aa.iter().filter(|&&a| a != 0 && a != 255).count();
            assert!(
                partial >= 2,
                "aa:3 should anti-alias the disc silhouette; got {partial} partial-coverage levels"
            );
        }

        /// The depth cue must be continuous across a face. Evaluating it
        /// once per triangle quantizes it to the triangulation, so a flat
        /// top face reads as a fan of hard tonal wedges; interpolating it
        /// from the rasterizer's own depth makes the same face a smooth
        /// gradient. Scanned at `aa: 1` so no downsampling smooths over a
        /// step that is really there.
        #[test]
        fn flat_face_shading_has_no_triangulation_steps() {
            // A thin disc seen isometrically: the top face is a wide flat
            // fan whose triangles differ a lot in depth (an axis-aligned
            // view would give the face no depth spread at all, and no cue
            // to be discontinuous), and it covers the whole mid scanline.
            let png = render_png_str(
                &cylinder_vcad(20.0, 0.5),
                &RasterOptions {
                    view: View::Isometric,
                    size_px: 256,
                    aa: Some(1),
                    ..Default::default()
                },
            )
            .unwrap();
            let img = image::load_from_memory(&png).expect("valid PNG").to_rgba8();

            // Scan *down* the image, not across: `right`, `down` and `cam`
            // are orthonormal, so on a Z-const face depth is constant along
            // a screen row and varies only down the screen. A horizontal
            // scan would see no cue at all and pass either way.
            //
            // Keep only the top face — opaque, and lighter than the midpoint
            // of the shading ramp, which excludes both the ink linework and
            // the much darker rim (in an isometric view of a disc the top is
            // the one upward-facing surface).
            let lum = |c: [u8; 3]| luma([c[0] as f64, c[1] as f64, c[2] as f64]);
            let mid = (lum(FILL_DARK) + lum(FILL_LIGHT)) / 2.0;
            let col: Vec<f64> = (0..img.height())
                .map(|y| *img.get_pixel(img.width() / 2, y))
                .filter(|p| p.0[3] == 255)
                .map(|p| lum([p.0[0], p.0[1], p.0[2]]))
                .filter(|&l| l > mid)
                .collect();
            assert!(
                col.len() > 60,
                "the disc's top face should span a good part of the column, \
                 got {} interior pixels",
                col.len()
            );
            let step = col
                .windows(2)
                .map(|w| (w[1] - w[0]).abs())
                .fold(0.0, f64::max);
            assert!(
                step < 1.5,
                "flat face should shade as a continuous gradient; \
                 largest adjacent luma jump was {step:.2}"
            );
        }

        #[test]
        fn size_px_alone_still_renders_a_square() {
            let png = render_png_str(
                &cube_vcad(20.0, 20.0, 200.0),
                &RasterOptions {
                    size_px: 256,
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(png_dims(&png), (256, 256));
        }

        #[test]
        fn height_px_gives_a_non_square_canvas() {
            let png = render_png_str(
                &cube_vcad(20.0, 20.0, 200.0),
                &RasterOptions {
                    size_px: 128,
                    height_px: Some(512),
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(png_dims(&png), (128, 512));
        }

        #[test]
        fn auto_aspect_fits_the_canvas_to_a_tall_part() {
            // A 20 × 20 × 400 column seen from the front projects far
            // taller than it is wide, so the canvas should too — the long
            // axis keeps size_px and the short axis shrinks.
            let png = render_png_str(
                &cube_vcad(20.0, 20.0, 400.0),
                &RasterOptions {
                    view: View::Front,
                    size_px: 512,
                    auto_aspect: true,
                    ..Default::default()
                },
            )
            .unwrap();
            let (w, h) = png_dims(&png);
            assert_eq!(h, 512, "long axis keeps size_px");
            assert!(w > 16 && w < 128, "short axis should shrink, got {w}x{h}");
        }

        #[test]
        fn trim_crops_to_the_drawn_content_plus_margin() {
            let doc = cube_vcad(20.0, 20.0, 200.0);
            let opts = RasterOptions {
                view: View::Front,
                size_px: 256,
                ..Default::default()
            };
            let full = png_dims(&render_png_str(&doc, &opts).unwrap());
            let margin = 4;
            let trimmed = png_dims(
                &render_png_str(
                    &doc,
                    &RasterOptions {
                        trim_margin_px: Some(margin),
                        ..opts.clone()
                    },
                )
                .unwrap(),
            );
            assert_eq!(full, (256, 256));
            assert!(
                trimmed.0 < full.0 && trimmed.1 < full.1,
                "trim should shrink both axes: {trimmed:?} vs {full:?}"
            );
            // Default fill is 0.6, so ~40% of each axis is background; the
            // crop keeps the content plus 2×margin.
            let expect = |px: u32| ((px as f64 * 0.6).round() as u32, px);
            let (lo, hi) = expect(256);
            assert!(
                trimmed.1 >= lo && trimmed.1 <= hi,
                "trimmed height {} outside [{lo}, {hi}]",
                trimmed.1
            );
        }

        #[test]
        fn trim_with_nothing_drawn_is_a_no_op() {
            // Every render draws *something*, so the empty-mask branch is
            // exercised through a margin larger than the canvas: the crop
            // saturates at the canvas bounds rather than overflowing.
            let png = render_png_str(
                &cube_vcad(20.0, 20.0, 20.0),
                &RasterOptions {
                    size_px: 128,
                    trim_margin_px: Some(9999),
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(png_dims(&png), (128, 128));
        }

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

        /// Annotated raster output differs from plain, and default
        /// (all-off) annotations leave it byte-identical.
        #[test]
        fn raster_annotations_change_output_only_when_enabled() {
            let doc = two_part_vcad();
            let render = |annotations: RenderAnnotations| {
                render_jpeg_str(
                    doc,
                    &RasterOptions {
                        size_px: 256,
                        annotations,
                        ..Default::default()
                    },
                )
                .unwrap()
            };
            let plain = render(RenderAnnotations::default());
            let legacy = render_jpeg_str(
                doc,
                &RasterOptions {
                    size_px: 256,
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(plain, legacy, "default annotations must be a no-op");
            let annotated = render(RenderAnnotations {
                axes: true,
                labels: true,
                dims: true,
            });
            assert_ne!(plain, annotated, "overlays must actually draw");
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
            // The part is centered and fills ~60% of the canvas, so the
            // central region is solidly covered. Assert on a small window
            // rather than a single pixel so a future projection/fill tweak
            // that nudges the centroid can't silently break the invariant.
            let opaque = (56..72)
                .flat_map(|y| (56..72).map(move |x| (x, y)))
                .any(|(x, y)| img.get_pixel(x, y)[3] == 255);
            assert!(opaque, "central region should contain an opaque pixel");
        }

        /// The axes gizmo sits in the lower-left corner over the background;
        /// its linework must mark the coverage mask so it stays opaque in the
        /// transparent PNG (regression: annotations that only wrote RGB would
        /// vanish under alpha 0).
        #[test]
        fn png_annotations_are_opaque_over_background() {
            let opts = |annotations| RasterOptions {
                size_px: 256,
                annotations,
                ..Default::default()
            };
            let load = |o: &RasterOptions| {
                image::load_from_memory(&render_png_str(&cube_vcad(20.0, 20.0, 20.0), o).unwrap())
                    .unwrap()
                    .to_rgba8()
            };
            let plain = load(&opts(RenderAnnotations::default()));
            let annotated = load(&opts(RenderAnnotations {
                axes: true,
                ..Default::default()
            }));
            // The gizmo lives near (48, size-48); scan that lower-left region.
            let opaque_in_region = |img: &image::RgbaImage| {
                (200..245)
                    .flat_map(|y| (20..80).map(move |x| (x, y)))
                    .filter(|&(x, y)| img.get_pixel(x, y)[3] == 255)
                    .count()
            };
            assert_eq!(
                opaque_in_region(&plain),
                0,
                "no geometry in the lower-left corner without the gizmo"
            );
            assert!(
                opaque_in_region(&annotated) > 0,
                "axes gizmo must be opaque over the transparent background"
            );
        }
    }

    fn cylinder_vcad(radius: f64, height: f64) -> String {
        format!(
            r#"{{
  "version": "0.1",
  "nodes": {{
    "1": {{
      "id": 1,
      "name": "Cyl",
      "op": {{ "type": "Cylinder", "radius": {radius}, "height": {height}, "segments": 0 }}
    }}
  }},
  "materials": {{}},
  "part_materials": {{}},
  "roots": [{{ "root": 1, "material": "default" }}]
}}"#
        )
    }

    fn sphere_vcad(radius: f64) -> String {
        format!(
            r#"{{
  "version": "0.1",
  "nodes": {{
    "1": {{
      "id": 1,
      "name": "Sph",
      "op": {{ "type": "Sphere", "radius": {radius}, "segments": 0 }}
    }}
  }},
  "materials": {{}},
  "part_materials": {{}},
  "roots": [{{ "root": 1, "material": "default" }}]
}}"#
        )
    }

    fn render_exact(vcad: &str) -> String {
        render_svg_str_opts(
            vcad,
            8.0,
            &SvgOptions {
                exact_edges: true,
                ..Default::default()
            },
        )
        .expect("exact-edges render should succeed")
    }

    /// With `exact_edges`, a cylinder's rims are emitted as exact SVG
    /// elliptical-arc paths and the corresponding rim polylines vanish —
    /// only the two straight silhouette rulings remain as `<line>`s.
    #[test]
    fn exact_edges_replace_cylinder_rims_with_arcs() {
        let svg = render_exact(&cylinder_vcad(10.0, 24.0));
        assert!(
            svg.contains(r#"<path d="M"#) && svg.contains(" A "),
            "expected exact elliptical-arc paths in the linework"
        );
        let lines = svg.matches("<line ").count();
        assert!(
            lines <= 8,
            "rim polylines should be replaced by arcs, found {lines} <line>s"
        );
    }

    /// With `exact_edges`, a sphere's view outline is a single exact
    /// ellipse path (no polyline silhouette at all).
    #[test]
    fn exact_edges_replace_sphere_outline_with_arcs() {
        let svg = render_exact(&sphere_vcad(12.0));
        assert!(svg.contains(" A "), "expected an exact outline arc path");
        assert_eq!(
            svg.matches("<line ").count(),
            0,
            "sphere silhouette polylines should be fully replaced"
        );
    }

    /// A cube has no circular edges — exact mode must not invent arcs, and
    /// its output must keep the plain polyline linework.
    #[test]
    fn exact_edges_leave_cube_linework_alone() {
        let svg = render_exact(&cube_vcad(20.0, 20.0, 20.0));
        assert_eq!(
            svg.matches("<path").count(),
            0,
            "no arcs expected for a cube"
        );
        assert!(svg.contains("<line "));
    }

    /// Points of every `<line>`/`<polyline>` inside the `<g>` whose stroke
    /// width is `width`, one `Vec` per element.
    fn stroke_group_chains(svg: &str, width: f64) -> Vec<Vec<(f64, f64)>> {
        let open = format!(r#"stroke-width="{width}""#);
        let Some(start) = svg.find(&open) else {
            return Vec::new();
        };
        let body_start = start + svg[start..].find('>').expect("group tag closes");
        let body = &svg[body_start..];
        let body = &body[..body.find("</g>").unwrap_or(body.len())];
        let num = |s: &str| s.parse::<f64>().expect("numeric SVG coordinate");
        let mut out = Vec::new();
        for chunk in body.split("<polyline points=\"").skip(1) {
            let pts = &chunk[..chunk.find('"').expect("points attribute closes")];
            out.push(
                pts.split_whitespace()
                    .map(|p| {
                        let (x, y) = p.split_once(',').expect("x,y pair");
                        (num(x), num(y))
                    })
                    .collect(),
            );
        }
        for chunk in body.split("<line ").skip(1) {
            let tag = &chunk[..chunk.find("/>").expect("line tag closes")];
            let v: Vec<f64> = ["x1=\"", "y1=\"", "x2=\"", "y2=\""]
                .iter()
                .map(|k| {
                    let at = tag.find(k).expect("line coordinate attribute") + k.len();
                    num(&tag[at..][..tag[at..].find('"').expect("attribute closes")])
                })
                .collect();
            out.push(vec![(v[0], v[1]), (v[2], v[3])]);
        }
        out
    }

    /// Hidden-line spans arrive one per mesh edge, so a tessellated curve is
    /// hundreds of two-point pieces. Emitted as separate `<line>` elements
    /// a dashed stroke restarts its pattern at each one, and any piece
    /// shorter than a dash period draws 100% "on" — so every dashed hidden
    /// line along a curve came out solid, reading as stray visible linework.
    /// They must be chained into polylines for the dash phase to run.
    #[test]
    fn hidden_lines_chain_into_polylines() {
        let svg = render_svg_str(&cylinder_vcad(30.0, 20.0), 4.0).unwrap();
        let longest = stroke_group_chains(&svg, STROKE_HIDDEN_PX)
            .into_iter()
            .map(|c| c.len())
            .max()
            .unwrap_or(0);
        assert!(
            longest >= 10,
            "the hidden bottom rim should be one chained polyline; \
             longest hidden run was {longest} points"
        );
    }

    /// A fully-occluded edge is not a z-fighting fragment however short it
    /// projects. Judging hidden spans purely on absolute projected length
    /// ate the ends of long hidden curves — a rim's segments foreshorten as
    /// it turns away from the viewer — leaving a fragment floating in the
    /// middle of the part, attached to nothing. The hidden bottom rim of a
    /// cylinder must run tangent point to tangent point, i.e. span the
    /// drawing.
    ///
    /// A *plain* cylinder does not reproduce this — its rim is coarse
    /// enough that every segment clears the absolute threshold. It takes a
    /// boolean result, whose rims get a sag-adaptive canonical grid of a
    /// few hundred points, for the per-segment projection to fall under it.
    #[test]
    fn fully_occluded_edges_survive_a_short_projection() {
        // Ø120 × 15 disc with a Ø40 bore through it.
        let vcad = r#"{
  "version": "0.1",
  "nodes": {
    "1": { "id": 1, "name": "Disc", "op": { "type": "Cylinder", "radius": 60.0, "height": 15.0, "segments": 0 } },
    "2": { "id": 2, "name": "Bore", "op": { "type": "Cylinder", "radius": 20.0, "height": 17.0, "segments": 0 } },
    "3": { "id": 3, "name": "BoreT", "op": { "type": "Translate", "child": 2, "offset": { "x": 0.0, "y": 0.0, "z": -1.0 } } },
    "4": { "id": 4, "name": "Drilled", "op": { "type": "Difference", "left": 1, "right": 3 } }
  },
  "materials": {},
  "part_materials": {},
  "roots": [{ "root": 4, "material": "default" }]
}"#;
        let svg = render_svg_str(vcad, 4.0).unwrap();
        let span_x = |chains: Vec<Vec<(f64, f64)>>| {
            let xs: Vec<f64> = chains.into_iter().flatten().map(|p| p.0).collect();
            let lo = xs.iter().cloned().fold(f64::INFINITY, f64::min);
            let hi = xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            (hi - lo).max(0.0)
        };
        let hidden = span_x(stroke_group_chains(&svg, STROKE_HIDDEN_PX));
        let outline = span_x(stroke_group_chains(&svg, STROKE_OUTLINE_PX));
        assert!(outline > 0.0, "cylinder should have an outline");
        let ratio = hidden / outline;
        assert!(
            ratio >= 0.9,
            "the hidden rim should span the part, not float in the middle of it; \
             it covered {ratio:.2} of the outline's width"
        );
    }

    /// Edges reach the emitter in `HashMap` iteration order, which varies
    /// between runs. That was harmless while each was its own `<line>`, but
    /// chaining turns it into a different partition of the same curve, so
    /// the same document would render to different bytes each time —
    /// churning any committed or cached SVG. Chaining sorts canonically
    /// first; this holds it to that.
    #[test]
    fn the_same_document_renders_to_identical_bytes() {
        let doc = cylinder_vcad(30.0, 20.0);
        let first = render_svg_str(&doc, 4.0).unwrap();
        for i in 0..4 {
            assert_eq!(
                first,
                render_svg_str(&doc, 4.0).unwrap(),
                "render {i} differed from the first"
            );
        }
    }

    /// Default (non-exact) output stays polyline-only — the flag is opt-in.
    #[test]
    fn exact_edges_off_by_default() {
        let svg = render_svg_str(&cylinder_vcad(10.0, 24.0), 8.0).unwrap();
        assert_eq!(svg.matches("<path").count(), 0);
    }

    /// A boolean-drilled hole keeps exact rims: the boolean pipeline
    /// preserves the cylindrical surface, so the bore's mouth must come out
    /// as arcs, not a 64-gon.
    #[test]
    fn exact_edges_survive_booleans() {
        let vcad = r#"{
  "version": "0.1",
  "nodes": {
    "1": { "id": 1, "name": "Plate", "op": { "type": "Cube", "size": { "x": 50.0, "y": 40.0, "z": 8.0 } } },
    "2": { "id": 2, "name": "Hole", "op": { "type": "Cylinder", "radius": 6.0, "height": 20.0, "segments": 0 } },
    "3": { "id": 3, "name": "HoleT", "op": { "type": "Translate", "child": 2, "offset": { "x": 25.0, "y": 20.0, "z": -5.0 } } },
    "4": { "id": 4, "name": "Drilled", "op": { "type": "Difference", "left": 1, "right": 3 } }
  },
  "materials": {},
  "part_materials": {},
  "roots": [{ "root": 4, "material": "default" }]
}"#;
        let svg = render_exact(vcad);
        assert!(
            svg.contains(" A "),
            "drilled hole rim should render as exact arcs"
        );
    }

    /// A hollow box (cube minus an inset cube) whose cavity never reaches
    /// the outside — a solid shell with walls all around.
    fn hollow_box_vcad() -> &'static str {
        r#"{
  "version": "0.1",
  "nodes": {
    "1": { "id": 1, "name": "outer", "op": { "type": "Cube", "size": { "x": 30.0, "y": 30.0, "z": 30.0 } } },
    "2": { "id": 2, "name": "inner", "op": { "type": "Cube", "size": { "x": 20.0, "y": 20.0, "z": 20.0 } } },
    "3": { "id": 3, "name": "inner_placed", "op": { "type": "Translate", "child": 2, "offset": { "x": 5.0, "y": 5.0, "z": 5.0 } } },
    "4": { "id": 4, "name": "shell", "op": { "type": "Difference", "left": 1, "right": 3 } }
  },
  "materials": {},
  "part_materials": {},
  "roots": [{ "root": 4, "material": "default" }]
}"#
    }

    #[test]
    fn parses_section_planes() {
        use std::str::FromStr;
        let s = SectionPlane::from_str("z=10").unwrap();
        assert_eq!(s.axis, Axis::Z);
        assert!((s.coord - 10.0).abs() < 1e-12);
        assert_eq!(SectionPlane::from_str("X=-2.5").unwrap().axis, Axis::X);
        assert!(SectionPlane::from_str("w=1").is_err());
        assert!(SectionPlane::from_str("z").is_err());
        assert!(SectionPlane::from_str("z=abc").is_err());
    }

    /// Sectioning a hollow box at mid-height must expose the cavity: the
    /// SVG gains cross-hatched cut faces (the hatch pattern def plus
    /// polygons filled with it) that an unsectioned render lacks.
    #[test]
    fn section_of_hollow_box_emits_hatched_cut_faces() {
        let plane = SectionPlane {
            axis: Axis::Z,
            coord: 15.0,
        };
        let svg = render_svg_str_section(
            hollow_box_vcad(),
            4.0,
            View::Isometric,
            false,
            Some(plane),
            &RenderAnnotations::default(),
        )
        .expect("sectioned hollow box should render");
        assert!(
            svg.contains(r#"<pattern id="section-hatch""#),
            "expected the section hatch pattern def"
        );
        assert!(
            svg.contains(r#"fill="url(#section-hatch)""#),
            "expected polygons filled with the section hatch"
        );

        let plain = render_svg_str_view(hollow_box_vcad(), 4.0, View::Isometric).unwrap();
        assert!(
            !plain.contains("section-hatch"),
            "unsectioned render must not carry hatch markup"
        );
    }

    /// A section plane entirely above the part removes nothing and adds no
    /// hatched faces; one entirely below removes everything.
    #[test]
    fn section_plane_outside_part_bounds() {
        let above = SectionPlane {
            axis: Axis::Z,
            coord: 100.0,
        };
        let svg = render_svg_str_section(
            hollow_box_vcad(),
            2.0,
            View::Isometric,
            false,
            Some(above),
            &RenderAnnotations::default(),
        )
        .expect("plane above the part should render it uncut");
        assert!(!svg.contains(r#"fill="url(#section-hatch)""#));

        let below = SectionPlane {
            axis: Axis::Z,
            coord: -100.0,
        };
        assert!(
            render_svg_str_section(
                hollow_box_vcad(),
                2.0,
                View::Isometric,
                false,
                Some(below),
                &RenderAnnotations::default()
            )
            .is_err(),
            "plane below the part removes all material"
        );
    }

    #[cfg(feature = "raster")]
    #[test]
    fn section_composes_with_jpeg_and_view() {
        let opts = RasterOptions {
            view: View::Front,
            size_px: 256,
            section: Some(SectionPlane {
                axis: Axis::Y,
                coord: 15.0,
            }),
            ..Default::default()
        };
        let jpg = render_jpeg_str(hollow_box_vcad(), &opts).unwrap();
        assert_eq!(&jpg[..2], &[0xFF, 0xD8]);
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

    #[cfg(feature = "raytrace")]
    mod raytrace {
        use super::*;

        fn sphere_vcad(radius: f64) -> String {
            format!(
                r#"{{
  "version": "0.1",
  "nodes": {{
    "1": {{
      "id": 1,
      "name": "Sphere",
      "op": {{ "type": "Sphere", "radius": {radius}, "segments": 0 }}
    }}
  }},
  "materials": {{}},
  "part_materials": {{}},
  "roots": [{{ "root": 1, "material": "default" }}]
}}"#
            )
        }

        /// Luminance of the center row's interior (non-background) pixels.
        fn center_row_luma(png: &[u8], size: u32) -> Vec<f64> {
            let img = image::load_from_memory(png).expect("valid PNG").to_rgb8();
            assert_eq!(img.width(), size);
            assert_eq!(img.height(), size);
            let y = size / 2;
            let bg = super::super::raster::BACKGROUND;
            (0..size)
                .map(|x| *img.get_pixel(x, y))
                .filter(|p| p.0 != bg)
                .map(|p| luma([p.0[0] as f64, p.0[1] as f64, p.0[2] as f64]))
                .collect()
        }

        /// Largest luma jump between adjacent pixels, ignoring a margin at
        /// each end (grazing-angle silhouette pixels legitimately change
        /// fast in both paths).
        fn max_adjacent_delta(row: &[f64], margin: usize) -> f64 {
            let inner = &row[margin.min(row.len())..row.len().saturating_sub(margin)];
            inner
                .windows(2)
                .map(|w| (w[1] - w[0]).abs())
                .fold(0.0, f64::max)
        }

        /// The point of the raytrace path: analytic intersection means a
        /// sphere shades as a continuous gradient, while the tessellated
        /// path flat-shades facets that band. The ray-traced center row
        /// must be strictly smoother than the tessellated one, and smooth
        /// in absolute terms.
        #[test]
        fn raytraced_sphere_is_smooth_vs_tessellated() {
            let doc = sphere_vcad(10.0);
            let opts = RasterOptions {
                view: View::Front,
                size_px: 128,
                ..Default::default()
            };
            let rt = render_raytrace_png_str(&doc, &opts).unwrap();
            let tess = render_png_str(&doc, &opts).unwrap();

            let rt_row = center_row_luma(&rt, 128);
            let tess_row = center_row_luma(&tess, 128);
            assert!(
                rt_row.len() > 40,
                "sphere should span a good chunk of the 128px canvas, got {} interior pixels",
                rt_row.len()
            );

            let rt_max = max_adjacent_delta(&rt_row, 4);
            let tess_max = max_adjacent_delta(&tess_row, 4);
            assert!(
                rt_max < 10.0,
                "ray-traced sphere row should shade smoothly, max adjacent luma jump {rt_max:.1}"
            );
            assert!(
                rt_max < tess_max,
                "ray-traced row (max jump {rt_max:.1}) should be smoother than the \
                 tessellated row (max jump {tess_max:.1})"
            );
        }

        #[test]
        fn raytrace_jpeg_has_soi_marker() {
            let opts = RasterOptions {
                size_px: 64,
                ..Default::default()
            };
            let jpg = render_raytrace_jpeg_str(&sphere_vcad(5.0), &opts).unwrap();
            assert_eq!(&jpg[..2], &[0xFF, 0xD8], "missing JPEG SOI marker");
        }

        /// Assembly-only documents (partDefs + instances, no scene roots)
        /// must ray trace their world-placed instances, same as the
        /// tessellated path.
        #[test]
        fn raytrace_renders_assembly_instances() {
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
            let opts = RasterOptions {
                size_px: 96,
                ..Default::default()
            };
            let png = render_raytrace_png_str(vcad, &opts).unwrap();
            let img = image::load_from_memory(&png).unwrap().to_rgb8();
            assert_eq!(img.width(), 96);
            let bg = super::super::raster::BACKGROUND;
            let non_bg = img.pixels().filter(|p| p.0 != bg).count();
            assert!(
                non_bg > 500,
                "expected both placed instances to cover pixels, got {non_bg} non-background"
            );
        }

        /// A 20mm axis-aligned box as a raw triangle mesh, translated along
        /// X. Twelve triangles, no normals — the shape a frozen
        /// `topology_optimize` result or a dropped-in STL arrives as.
        fn mesh_box_node(id: u32, x0: f64) -> String {
            let (x1, s) = (x0 + 20.0, 20.0);
            let corners = [
                [x0, 0.0, 0.0],
                [x1, 0.0, 0.0],
                [x1, s, 0.0],
                [x0, s, 0.0],
                [x0, 0.0, s],
                [x1, 0.0, s],
                [x1, s, s],
                [x0, s, s],
            ];
            let positions: Vec<String> = corners
                .iter()
                .flat_map(|c| c.iter().map(|v| format!("{v:?}")))
                .collect();
            // Two triangles per face, outward-wound (winding is irrelevant to
            // the double-sided intersector, but keeps the fixture sane).
            let indices = [
                [0, 2, 1],
                [0, 3, 2],
                [4, 5, 6],
                [4, 6, 7],
                [0, 1, 5],
                [0, 5, 4],
                [1, 2, 6],
                [1, 6, 5],
                [2, 3, 7],
                [2, 7, 6],
                [3, 0, 4],
                [3, 4, 7],
            ];
            let indices: Vec<String> = indices
                .iter()
                .flat_map(|t| t.iter().map(|i| i.to_string()))
                .collect();
            format!(
                r#""{id}": {{ "id": {id}, "name": "meshpart", "op": {{ "type": "ImportedMesh", "positions": [{}], "indices": [{}] }} }}"#,
                positions.join(", "),
                indices.join(", ")
            )
        }

        /// Per-column non-background pixel counts.
        fn column_coverage(png: &[u8]) -> Vec<u32> {
            let img = image::load_from_memory(png).expect("valid PNG").to_rgb8();
            let bg = super::super::raster::BACKGROUND;
            (0..img.width())
                .map(|x| {
                    (0..img.height())
                        .filter(|&y| img.get_pixel(x, y).0 != bg)
                        .count() as u32
                })
                .collect()
        }

        /// The regression this guards: `rasterize_rt` used to skip any solid
        /// without an analytic BRep, so a document mixing a BRep part with a
        /// mesh part rendered the BRep and silently omitted the mesh — no
        /// error, no warning. Asserting "it didn't error" would have passed
        /// against the bug; this asserts the mesh actually covers pixels.
        #[test]
        fn raytrace_renders_both_brep_and_mesh_parts() {
            // Cube at x 0..20, mesh box at x 60..80: with a 40mm gap the two
            // parts land in opposite thirds of the canvas whichever way the
            // view basis runs.
            let cube = r#""1": { "id": 1, "name": "cube", "op": { "type": "Cube", "size": { "x": 20.0, "y": 20.0, "z": 20.0 } } }"#;
            let doc = |nodes: &str, roots: &str| {
                format!(
                    r#"{{ "version": "0.1", "nodes": {{ {nodes} }}, "materials": {{}}, "part_materials": {{}}, "roots": [{roots}] }}"#
                )
            };
            let both = doc(
                &format!("{cube}, {}", mesh_box_node(2, 60.0)),
                r#"{ "root": 1, "material": "default" }, { "root": 2, "material": "default" }"#,
            );

            let opts = RasterOptions {
                view: View::Front,
                size_px: 128,
                fill_frac: 0.9,
                ..Default::default()
            };
            let cols = column_coverage(&render_raytrace_png_str(&both, &opts).unwrap());

            // Both outer thirds must carry geometry, and the gap between the
            // parts must not.
            let third = cols.len() / 3;
            let left: u32 = cols[..third].iter().sum();
            let middle: u32 = cols[third..2 * third].iter().sum();
            let right: u32 = cols[2 * third..].iter().sum();
            assert!(
                left > 200 && right > 200,
                "both parts must cover pixels: left={left} middle={middle} right={right}"
            );
            assert_eq!(
                middle, 0,
                "the 40mm gap between the parts should be background"
            );

            // Framing-independent restatement of the same thing: the
            // covered columns form exactly two disjoint runs, one per part.
            // Under the old `as_brep()`-or-skip code only the cube survived,
            // which frames to a single run filling the canvas.
            let runs = cols
                .iter()
                .zip(cols.iter().skip(1))
                .filter(|(a, b)| (**a == 0) != (**b == 0))
                .count();
            assert_eq!(
                runs, 4,
                "expected two separate covered runs (4 background/content \
                 transitions), got {runs}: {cols:?}"
            );
        }

        /// The drop path is honest: a part with no traceable geometry is
        /// named in the error rather than quietly omitted from the image.
        #[test]
        fn raytrace_reports_untraceable_parts_instead_of_dropping_them() {
            // A degenerate "mesh" — three collinear points, so every
            // triangle is dropped at BVH build time.
            let degenerate = r#""2": { "id": 2, "name": "flat", "op": { "type": "ImportedMesh", "positions": [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 2.0, 0.0, 0.0], "indices": [0, 1, 2] } }"#;
            let vcad = format!(
                r#"{{ "version": "0.1", "nodes": {{ "1": {{ "id": 1, "name": "cube", "op": {{ "type": "Cube", "size": {{ "x": 20.0, "y": 20.0, "z": 20.0 }} }} }}, {degenerate} }}, "materials": {{}}, "part_materials": {{}}, "roots": [{{ "root": 1, "material": "default" }}, {{ "root": 2, "material": "default" }}] }}"#
            );
            let opts = RasterOptions {
                size_px: 64,
                ..Default::default()
            };
            let err = render_raytrace_png_str(&vcad, &opts).unwrap_err();
            assert!(
                err.contains("no traceable geometry") && err.contains("flat"),
                "error should name the untraceable part, got: {err}"
            );
        }
    }
}
