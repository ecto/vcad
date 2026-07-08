//! Layered, colored PCB preview meshes for 3D visualization.
//!
//! The canonical [`CsgOp::PcbBoard`](vcad_ir::CsgOp::PcbBoard) evaluation in
//! [`crate::evaluate`] merges the FR4 slab, copper, and crude component boxes
//! into a single solid with one material — correct for STEP export and ray
//! tracing, but it renders as a featureless gray slab in a lit GLB viewer.
//!
//! This module produces the *same* board as a small set of separately colored
//! meshes — green soldermask, mask-clad copper (green) with exposed gold pads,
//! real 3D component bodies, and white silkscreen — for the inline GLB preview.
//! It reuses the exact copper-mesh
//! helpers the merged path uses ([`trace_to_mesh`](crate::evaluate::trace_to_mesh)
//! et al.) so the two views never diverge, and pulls component bodies from
//! [`vcad_ecad_pcb::component_mesh`] so the preview shows real packages
//! (chips with metallic end caps, dark ICs, pin headers) instead of 1 mm boxes.
//!
//! All output is in the board-local frame, centered on `z = 0` (top surface at
//! `+thickness/2`), matching the merged `PcbBoard` solid so preview meshes drop
//! straight into a scene alongside other parts.

use std::collections::HashMap;

use serde::Serialize;
use vcad_ir::ecad::{Footprint, FootprintGraphic, Pcb, PcbLayer};
use vcad_ir::stroke_font::{text_strokes, text_width};
use vcad_ir::Vec2;
use vcad_kernel::Solid;
use vcad_kernel_math::Vec3;

use crate::convert::ir_sketch_to_profile;
use crate::evaluate::{
    copper_thickness, pad_to_mesh, trace_arc_to_mesh, trace_to_mesh, via_to_mesh, zone_to_mesh,
    RawMesh,
};

/// A single colored sub-mesh of a PCB preview.
///
/// Positions/indices/normals follow the usual flat-buffer layout; the GLB
/// exporter turns `color` / `metalness` / `roughness` into a PBR material.
#[derive(Debug, Clone, Serialize)]
pub struct PcbPreviewMesh {
    /// Semantic role: `"mask"`, `"substrate"`, `"copper"` (exposed pads/vias),
    /// `"copper_masked"` (mask-clad traces/pours), `"component"`, or
    /// `"silkscreen"`.
    pub role: String,
    /// Flat vertex positions `[x,y,z, ...]` (mm, board-local, centered on z=0).
    pub positions: Vec<f32>,
    /// Triangle indices.
    pub indices: Vec<u32>,
    /// Per-vertex normals `[nx,ny,nz, ...]`.
    pub normals: Vec<f32>,
    /// Base color RGB, 0..1 (linear).
    pub color: [f32; 3],
    /// PBR metalness, 0..1.
    pub metalness: f32,
    /// PBR roughness, 0..1.
    pub roughness: f32,
    /// Emissive color RGB, 0..1 (linear). `[0,0,0]` = not emissive (LEDs glow).
    pub emissive: [f32; 3],
    /// KHR_materials_clearcoat factor, 0..1 (glossy soldermask wet-look).
    pub clearcoat: f32,
    /// Clearcoat roughness, 0..1.
    pub clearcoat_roughness: f32,
}

// Green soldermask over the bare laminate (the board substrate between copper).
// A satin dielectric: a soft clearcoat sheen carries the highlight, so the base
// color stays a dark saturated green and never blows out to a wet mirror.
// Dark linear albedo on purpose: the up-facing board catches the studio key +
// fill directly, which multiplies the diffuse ~3x, so a light albedo washes out
// to mint. This deep value lands a classic PCB green once lit.
const SOLDERMASK_GREEN: [f32; 3] = [0.010, 0.060, 0.024];
// Soldermask over copper (traces, pours). The mask conforms to a flat copper
// plane, so it sits a touch brighter than mask over the rough weave — which is
// exactly what keeps routing faintly legible on a finished board, instead of
// a sea of bare gold.
const COPPER_MASK_GREEN: [f32; 3] = [0.013, 0.075, 0.030];
// Exposed fiberglass substrate at the board edge — matte tan, no clearcoat;
// the contrast against the mask sells the "real board" read.
const FR4_EDGE_TAN: [f32; 3] = [0.46, 0.38, 0.22];
// Exposed/finished copper (ENIG warm gold) at the mask openings: pads + via
// rings. Satin, not a mirror — the openings are the only bright copper.
const COPPER_ENIG: [f32; 3] = [0.85, 0.66, 0.30];
// Silkscreen white.
const SILK_WHITE: [f32; 3] = [0.92, 0.92, 0.88];

// Soft clearcoat sheen for the soldermask — a satin coat, not the old wet glass
// (the previous 1.0 / 0.08 read as a reflective mirror under the studio IBL).
const MASK_CLEARCOAT: f32 = 0.2;
const MASK_CLEARCOAT_ROUGH: f32 = 0.35;

// Silk line width and reference-designator text height (mm).
const SILK_LINE_WIDTH: f64 = 0.15;
const SILK_TEXT_HEIGHT: f64 = 1.0;
// Lift silk slightly off the copper/board so it never z-fights.
const SILK_LIFT: f64 = 0.06;

/// Build the layered, colored preview meshes for a PCB.
///
/// Returns a compact set (typically 4–7 meshes): one board, one merged copper,
/// one silkscreen, and component bodies grouped by color. An empty board
/// (degenerate outline) yields an empty vec.
pub fn pcb_preview_meshes(pcb: &Pcb) -> Vec<PcbPreviewMesh> {
    let t = pcb.outline.thickness;
    let mut out: Vec<PcbPreviewMesh> = Vec::new();

    // ---- Board: satin green soldermask (faces) + matte tan substrate (edge) ----
    // Splitting the slab by face normal lets the top/bottom carry the soft
    // soldermask clearcoat while the exposed fiberglass edge stays matte tan.
    if let Some(board) = board_slab_buf(pcb) {
        if !board.is_empty() {
            let (mask, edge) = split_faces_by_normal(&board, 0.7);
            if !mask.is_empty() {
                let mut m = mask.finish("mask", SOLDERMASK_GREEN, 0.0, 0.45);
                m.clearcoat = MASK_CLEARCOAT;
                m.clearcoat_roughness = MASK_CLEARCOAT_ROUGH;
                out.push(m);
            }
            if !edge.is_empty() {
                out.push(edge.finish("substrate", FR4_EDGE_TAN, 0.0, 0.85));
            }
        }
    }

    // ---- Copper, split by solder-mask coverage ----
    // A finished board is mostly green: the mask covers traces and pours and
    // opens only over pads and via rings. So traces + pours render as soldermask
    // green (faintly proud of the board, with the same satin coat, so routing
    // stays legible as a lighter-green topography); pads + via rings show the
    // exposed ENIG finish. This is what reads as a real PCB — and it removes the
    // board-spanning bare-gold pour that a low-roughness metal blew out to a
    // wet, glassy sheet under the studio IBL.
    let mut masked = MeshBuf::default(); // traces + pours → mask green
    for trace in &pcb.traces {
        masked.append_raw(&trace_to_mesh(trace, pcb), copper_lift(pcb, trace.layer));
    }
    for arc in &pcb.trace_arcs {
        masked.append_raw(&trace_arc_to_mesh(arc, pcb), copper_lift(pcb, arc.layer));
    }
    for zone in &pcb.zones {
        masked.append_raw(&zone_to_mesh(zone, pcb), copper_lift(pcb, zone.layer));
    }
    if !masked.is_empty() {
        let mut m = masked.finish("copper_masked", COPPER_MASK_GREEN, 0.0, 0.45);
        m.clearcoat = MASK_CLEARCOAT;
        m.clearcoat_roughness = MASK_CLEARCOAT_ROUGH;
        out.push(m);
    }
    // Exposed finish at the mask openings: ENIG gold, satin (not a chrome
    // mirror). Pads, then plated via barrels — vias span the full board height
    // and meet both surfaces flush, so they stay unlifted.
    let mut exposed = MeshBuf::default();
    for fp in &pcb.footprints {
        for pad in &fp.pads {
            let layer = pad_layer(pad, fp);
            exposed.append_raw(&pad_to_mesh(pad, fp, pcb), copper_lift(pcb, layer));
        }
    }
    for via in &pcb.vias {
        exposed.append_raw(&via_to_mesh(via, pcb, 24), 0.0);
    }
    if !exposed.is_empty() {
        out.push(exposed.finish("copper", COPPER_ENIG, 0.7, 0.45));
    }

    // ---- Component bodies (real packages, grouped by full material) ----
    let comp_meshes = vcad_ecad_pcb::component_mesh::generate_component_meshes(pcb);
    // Group by quantized (color, metalness, roughness, emissive) so distinct
    // materials survive while keeping the GLB to a handful of materials.
    let mut groups: HashMap<[u16; 7], MeshBuf> = HashMap::new();
    let mut order: Vec<[u16; 7]> = Vec::new();
    for cm in &comp_meshes {
        let key = [
            quantize(cm.color[0]),
            quantize(cm.color[1]),
            quantize(cm.color[2]),
            quantize(cm.metalness),
            quantize(cm.roughness),
            quantize(cm.emissive[0]),
            // Pack g,b of emissive into one slot to stay within key width; the
            // LED is the only emissive material so this collision is harmless.
            quantize((cm.emissive[1] + cm.emissive[2]) * 0.5),
        ];
        let buf = groups.entry(key).or_insert_with(|| {
            order.push(key);
            MeshBuf::default()
        });
        buf.append_indexed(&cm.positions, &cm.indices, &cm.normals);
        // Stash the exact emissive on first sight (keyed groups share material).
        buf.emissive = cm.emissive;
    }
    // `order` preserves first-seen insertion order for deterministic output.
    for key in &order {
        if let Some(buf) = groups.remove(key) {
            let color = [
                key[0] as f32 / 1000.0,
                key[1] as f32 / 1000.0,
                key[2] as f32 / 1000.0,
            ];
            let metalness = key[3] as f32 / 1000.0;
            let roughness = key[4] as f32 / 1000.0;
            let emissive = buf.emissive;
            let mut m = buf.finish("component", color, metalness, roughness);
            m.emissive = emissive;
            out.push(m);
        }
    }

    // ---- Silkscreen (white): footprint outlines + reference designators ----
    let mut silk = MeshBuf::default();
    for fp in &pcb.footprints {
        add_footprint_silk(&mut silk, fp, t);
    }
    if !silk.is_empty() {
        out.push(silk.finish("silkscreen", SILK_WHITE, 0.0, 0.9));
    }

    // Center every mesh on z=0 to match the merged PcbBoard solid.
    let z_shift = (-t / 2.0) as f32;
    for mesh in &mut out {
        let mut i = 2;
        while i < mesh.positions.len() {
            mesh.positions[i] += z_shift;
            i += 3;
        }
    }

    out
}

/// Z lift applied to copper so it sits proud of the board surface rather than
/// buried inside the slab (the copper helpers place it flush with the surface,
/// which z-fights once board and copper are separate meshes).
fn copper_lift(pcb: &Pcb, layer: PcbLayer) -> f64 {
    let ct = copper_thickness(pcb, layer);
    match layer {
        PcbLayer::FCu => ct,  // raise the top-copper above the top face
        PcbLayer::BCu => -ct, // drop the bottom-copper below the bottom face
        _ => 0.0,
    }
}

/// Resolve which copper layer a pad lives on (mirrors `pad_to_mesh`).
fn pad_layer(pad: &vcad_ir::ecad::Pad, fp: &Footprint) -> PcbLayer {
    pad.layers
        .iter()
        .find(|l| l.is_copper())
        .copied()
        .unwrap_or(if fp.front {
            PcbLayer::FCu
        } else {
            PcbLayer::BCu
        })
}

/// Quantize a 0..1 PBR scalar to a stable integer key (3 decimal places).
fn quantize(v: f32) -> u16 {
    (v.clamp(0.0, 1.0) * 1000.0).round() as u16
}

/// Build the extruded board-outline slab (with cutouts) as a mesh buffer.
fn board_slab_buf(pcb: &Pcb) -> Option<MeshBuf> {
    let verts = &pcb.outline.vertices;
    if verts.len() < 3 {
        return None;
    }
    let segments = ring_segments(verts);
    let origin = vcad_ir::Vec3::new(0.0, 0.0, 0.0);
    let x_dir = vcad_ir::Vec3::new(1.0, 0.0, 0.0);
    let y_dir = vcad_ir::Vec3::new(0.0, 1.0, 0.0);
    let profile = ir_sketch_to_profile(&origin, &x_dir, &y_dir, &segments).ok()?;
    let mut solid = Solid::extrude(profile, Vec3::new(0.0, 0.0, pcb.outline.thickness)).ok()?;

    for cutout in &pcb.outline.cutouts {
        if cutout.len() < 3 {
            continue;
        }
        let cut_segs = ring_segments(cutout);
        if let Ok(cut_profile) = ir_sketch_to_profile(&origin, &x_dir, &y_dir, &cut_segs) {
            // Slightly taller than the board for a clean boolean.
            let cut_dir = Vec3::new(0.0, 0.0, pcb.outline.thickness * 1.1);
            if let Ok(cut_solid) = Solid::extrude(cut_profile, cut_dir) {
                solid = solid.difference(&cut_solid);
            }
        }
    }

    let mesh = solid.to_mesh(32);
    if mesh.indices.is_empty() {
        return None;
    }
    let normals = if mesh.normals.len() == mesh.vertices.len() {
        mesh.normals
    } else {
        Vec::new()
    };
    Some(MeshBuf {
        positions: mesh.vertices,
        indices: mesh.indices,
        normals,
        ..Default::default()
    })
}

/// Build closed-ring `Line` segments for a polygon outline.
fn ring_segments(verts: &[Vec2]) -> Vec<vcad_ir::SketchSegment2D> {
    (0..verts.len())
        .map(|i| vcad_ir::SketchSegment2D::Line {
            start: verts[i],
            end: verts[(i + 1) % verts.len()],
        })
        .collect()
}

/// Append a footprint's silkscreen (outline graphics + reference designator) to
/// `buf`. Silk sits just above the board's top (front parts) or below its
/// bottom (back parts), drawn as flat ribbons facing the viewer.
fn add_footprint_silk(buf: &mut MeshBuf, fp: &Footprint, thickness: f64) {
    let front = fp.front;
    // Silk sits just above the board top (front parts) or below its bottom
    // (back parts); ribbons are double-sided, so no per-element facing flag.
    let z = if front {
        (thickness + SILK_LIFT) as f32
    } else {
        (-SILK_LIFT) as f32
    };

    // Outline shapes defined on a silk layer.
    let hw = SILK_LINE_WIDTH / 2.0;
    for g in &fp.graphics {
        match g {
            FootprintGraphic::Line {
                start, end, layer, ..
            } if is_silk(*layer) => {
                add_stroke(buf, (start.x, start.y), (end.x, end.y), hw, z);
            }
            FootprintGraphic::Rect {
                start, end, layer, ..
            } if is_silk(*layer) => {
                let (x0, y0, x1, y1) = (start.x, start.y, end.x, end.y);
                add_polyline(
                    buf,
                    &[(x0, y0), (x1, y0), (x1, y1), (x0, y1), (x0, y0)],
                    hw,
                    z,
                );
            }
            FootprintGraphic::Circle {
                center,
                radius,
                layer,
                ..
            } if is_silk(*layer) => {
                add_arc(buf, (center.x, center.y), *radius, 0.0, 360.0, hw, z);
            }
            FootprintGraphic::Arc {
                center,
                radius,
                start_angle,
                end_angle,
                layer,
                ..
            } if is_silk(*layer) => {
                add_arc(
                    buf,
                    (center.x, center.y),
                    *radius,
                    *start_angle,
                    *end_angle,
                    hw,
                    z,
                );
            }
            FootprintGraphic::Polygon {
                vertices, layer, ..
            } if is_silk(*layer) && vertices.len() >= 2 => {
                let mut pts: Vec<(f64, f64)> = vertices.iter().map(|v| (v.x, v.y)).collect();
                pts.push(pts[0]);
                add_polyline(buf, &pts, hw, z);
            }
            _ => {}
        }
    }

    // Reference designator, placed just above the component so the body
    // doesn't bury it (a dead-center label sits under the package).
    let label = fp.reference.trim();
    if !label.is_empty() {
        let (fmin, fmax) = vcad_ecad_pcb::geometry::footprint_bounds(fp);
        let w = text_width(label, SILK_TEXT_HEIGHT);
        let ox = (fmin.x + fmax.x) / 2.0 - w / 2.0;
        let oy = fmax.y + 0.3; // clear the component's top edge (board +Y)
        let stroke_hw = (SILK_TEXT_HEIGHT * 0.12).max(0.05) / 2.0;
        for poly in text_strokes(label, SILK_TEXT_HEIGHT) {
            let pts: Vec<(f64, f64)> = poly.into_iter().map(|(x, y)| (ox + x, oy + y)).collect();
            add_polyline(buf, &pts, stroke_hw, z);
        }
    }
}

fn is_silk(layer: PcbLayer) -> bool {
    matches!(layer, PcbLayer::FSilkS | PcbLayer::BSilkS)
}

/// Append a flat ribbon quad for one segment `a→b` of half-width `hw` at z.
fn add_stroke(buf: &mut MeshBuf, a: (f64, f64), b: (f64, f64), hw: f64, z: f32) {
    let dx = b.0 - a.0;
    let dy = b.1 - a.1;
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-9 {
        return;
    }
    let nx = -dy / len * hw;
    let ny = dx / len * hw;
    let corners = [
        (a.0 + nx, a.1 + ny),
        (b.0 + nx, b.1 + ny),
        (b.0 - nx, b.1 - ny),
        (a.0 - nx, a.1 - ny),
    ];
    // Silk ribbons are zero-thickness flat quads. A single winding faces only
    // one way, so a top-down camera sees the culled backface and the silk
    // vanishes. Emit BOTH faces (up- and down-facing) on two separate vertex
    // sets — separate sets so finish()'s per-vertex normals are a clean ±Z
    // instead of cancelling to zero — so the layer reads from either side.
    let top = (buf.positions.len() / 3) as u32;
    for &(px, py) in &corners {
        buf.positions.push(px as f32);
        buf.positions.push(py as f32);
        buf.positions.push(z);
    }
    let bot = (buf.positions.len() / 3) as u32;
    for &(px, py) in &corners {
        buf.positions.push(px as f32);
        buf.positions.push(py as f32);
        buf.positions.push(z);
    }
    // Up-facing (normal +Z) winding, then down-facing (normal -Z) winding.
    buf.indices
        .extend_from_slice(&[top, top + 2, top + 1, top, top + 3, top + 2]);
    buf.indices
        .extend_from_slice(&[bot, bot + 1, bot + 2, bot, bot + 2, bot + 3]);
}

/// Append ribbons for an open polyline.
fn add_polyline(buf: &mut MeshBuf, pts: &[(f64, f64)], hw: f64, z: f32) {
    for w in pts.windows(2) {
        add_stroke(buf, w[0], w[1], hw, z);
    }
}

/// Append ribbons approximating an arc/circle (degrees).
#[allow(clippy::too_many_arguments)]
fn add_arc(
    buf: &mut MeshBuf,
    center: (f64, f64),
    radius: f64,
    start_deg: f64,
    end_deg: f64,
    hw: f64,
    z: f32,
) {
    if radius <= 1e-6 {
        return;
    }
    let sweep = (end_deg - start_deg).abs();
    let segs = ((sweep / 12.0).ceil() as usize).clamp(6, 64);
    let a0 = start_deg.to_radians();
    let a1 = end_deg.to_radians();
    let mut pts = Vec::with_capacity(segs + 1);
    for i in 0..=segs {
        let a = a0 + (a1 - a0) * (i as f64 / segs as f64);
        pts.push((center.0 + radius * a.cos(), center.1 + radius * a.sin()));
    }
    add_polyline(buf, &pts, hw, z);
}

/// Accumulates triangles into flat position/index/normal buffers.
#[derive(Default)]
struct MeshBuf {
    positions: Vec<f32>,
    indices: Vec<u32>,
    /// Provided normals (kept when present); empty means "compute on finish".
    normals: Vec<f32>,
    /// Emissive color stashed for grouped component materials (LEDs).
    emissive: [f32; 3],
}

impl MeshBuf {
    fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// Append a `RawMesh` (f64 verts, no normals), translating z by `dz`.
    fn append_raw(&mut self, raw: &RawMesh, dz: f64) {
        let base = (self.positions.len() / 3) as u32;
        for v in &raw.0 {
            self.positions.push(v[0] as f32);
            self.positions.push(v[1] as f32);
            self.positions.push((v[2] + dz) as f32);
        }
        for tri in &raw.1 {
            self.indices.push(tri[0] + base);
            self.indices.push(tri[1] + base);
            self.indices.push(tri[2] + base);
        }
        // Hand-built copper has no normals — force a recompute on finish.
        self.normals.clear();
    }

    /// Append an indexed mesh that carries its own per-vertex normals.
    fn append_indexed(&mut self, positions: &[f32], indices: &[u32], normals: &[f32]) {
        let base = (self.positions.len() / 3) as u32;
        self.positions.extend_from_slice(positions);
        if normals.len() == positions.len() {
            self.normals.extend_from_slice(normals);
        }
        for &i in indices {
            self.indices.push(i + base);
        }
    }

    fn finish(
        mut self,
        role: &str,
        color: [f32; 3],
        metalness: f32,
        roughness: f32,
    ) -> PcbPreviewMesh {
        if self.normals.len() != self.positions.len() {
            self.normals = compute_normals(&self.positions, &self.indices);
        }
        PcbPreviewMesh {
            role: role.to_string(),
            positions: self.positions,
            indices: self.indices,
            normals: self.normals,
            color,
            metalness,
            roughness,
            emissive: [0.0, 0.0, 0.0],
            clearcoat: 0.0,
            clearcoat_roughness: 0.0,
        }
    }
}

/// Split a mesh buffer into (faces, edges) by per-triangle geometric normal:
/// triangles whose normal is mostly vertical (`|nz| >= z_thresh`) go to the
/// first buffer (board faces / soldermask), the rest to the second (the
/// exposed substrate edge). Vertices are duplicated per triangle and given a
/// flat per-face normal: `to_mesh` averages normals across the 90° face/wall
/// seams, which makes a flat board face shade like a bright fan under a
/// specular light — re-flattening here keeps each face uniformly lit.
fn split_faces_by_normal(src: &MeshBuf, z_thresh: f32) -> (MeshBuf, MeshBuf) {
    let mut faces = MeshBuf::default();
    let mut edges = MeshBuf::default();

    let mut tri = 0;
    while tri + 2 < src.indices.len() {
        let ia = src.indices[tri] as usize;
        let ib = src.indices[tri + 1] as usize;
        let ic = src.indices[tri + 2] as usize;
        tri += 3;
        let p = |i: usize| {
            [
                src.positions[i * 3],
                src.positions[i * 3 + 1],
                src.positions[i * 3 + 2],
            ]
        };
        let (a, b, c) = (p(ia), p(ib), p(ic));
        // Geometric face normal.
        let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let nz = e1[0] * e2[1] - e1[1] * e2[0];
        let nx = e1[1] * e2[2] - e1[2] * e2[1];
        let ny = e1[2] * e2[0] - e1[0] * e2[2];
        let len = (nx * nx + ny * ny + nz * nz).sqrt().max(1e-12);
        let dst = if (nz / len).abs() >= z_thresh {
            &mut faces
        } else {
            &mut edges
        };
        let base = (dst.positions.len() / 3) as u32;
        let fnormal = [nx / len, ny / len, nz / len];
        for &i in &[ia, ib, ic] {
            dst.positions.extend_from_slice(&[
                src.positions[i * 3],
                src.positions[i * 3 + 1],
                src.positions[i * 3 + 2],
            ]);
            dst.normals.extend_from_slice(&fnormal);
        }
        dst.indices.extend_from_slice(&[base, base + 1, base + 2]);
    }
    (faces, edges)
}

/// Compute smooth per-vertex normals by area-weighted face-normal accumulation.
fn compute_normals(positions: &[f32], indices: &[u32]) -> Vec<f32> {
    let mut normals = vec![0.0f32; positions.len()];
    let mut i = 0;
    while i + 2 < indices.len() {
        let ia = indices[i] as usize * 3;
        let ib = indices[i + 1] as usize * 3;
        let ic = indices[i + 2] as usize * 3;
        if ia + 2 >= positions.len() || ib + 2 >= positions.len() || ic + 2 >= positions.len() {
            i += 3;
            continue;
        }
        let ax = positions[ia];
        let ay = positions[ia + 1];
        let az = positions[ia + 2];
        let e1 = [
            positions[ib] - ax,
            positions[ib + 1] - ay,
            positions[ib + 2] - az,
        ];
        let e2 = [
            positions[ic] - ax,
            positions[ic + 1] - ay,
            positions[ic + 2] - az,
        ];
        // Cross product (un-normalized → area weighting).
        let nx = e1[1] * e2[2] - e1[2] * e2[1];
        let ny = e1[2] * e2[0] - e1[0] * e2[2];
        let nz = e1[0] * e2[1] - e1[1] * e2[0];
        for &idx in &[ia, ib, ic] {
            normals[idx] += nx;
            normals[idx + 1] += ny;
            normals[idx + 2] += nz;
        }
        i += 3;
    }
    let mut v = 0;
    while v < normals.len() {
        let len = (normals[v] * normals[v]
            + normals[v + 1] * normals[v + 1]
            + normals[v + 2] * normals[v + 2])
            .sqrt();
        if len > 1e-12 {
            normals[v] /= len;
            normals[v + 1] /= len;
            normals[v + 2] /= len;
        } else {
            normals[v + 2] = 1.0;
        }
        v += 3;
    }
    normals
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_ir::ecad::*;

    fn board_with(footprints: Vec<Footprint>, traces: Vec<Trace>) -> Pcb {
        Pcb {
            outline: BoardOutline {
                vertices: vec![
                    Vec2::new(0.0, 0.0),
                    Vec2::new(40.0, 0.0),
                    Vec2::new(40.0, 30.0),
                    Vec2::new(0.0, 30.0),
                ],
                cutouts: vec![],
                thickness: 1.6,
            },
            stackup: LayerStackup {
                layers: vec![StackupLayer {
                    layer: PcbLayer::FCu,
                    copper_thickness: Some(0.035),
                    dielectric_thickness: Some(1.5),
                    dielectric_er: Some(4.5),
                    material: Some("FR4".into()),
                }],
            },
            nets: vec![],
            rules: DesignRules {
                default_rules: NetClassRules {
                    name: "Default".into(),
                    trace_width: 0.25,
                    clearance: 0.2,
                    via_diameter: 0.8,
                    via_drill: 0.4,
                    diff_pair_gap: None,
                    diff_pair_width: None,
                },
                class_rules: vec![],
                net_class_assignments: Default::default(),
                edge_clearance: 0.5,
                hole_to_hole: 0.5,
                min_annular_ring: 0.15,
                min_drill: 0.2,
            },
            footprints,
            traces,
            trace_arcs: vec![],
            vias: vec![],
            zones: vec![],
            keepouts: vec![],
            net_ties: vec![],
        }
    }

    fn chip(reference: &str, x: f64, y: f64) -> Footprint {
        Footprint {
            reference: reference.into(),
            value: "10k".into(),
            footprint_name: "0805".into(),
            position: Vec2::new(x, y),
            rotation: 0.0,
            front: true,
            pads: vec![Pad {
                number: "1".into(),
                pad_type: PadType::SMD,
                shape: PadShape::Rect {
                    width: 1.0,
                    height: 1.2,
                },
                position: Vec2::new(-1.0, 0.0),
                rotation: 0.0,
                drill: None,
                net: None,
                layers: vec![PcbLayer::FCu],
            }],
            graphics: vec![],
            model_3d: None,
            properties: Default::default(),
        }
    }

    #[test]
    fn empty_board_yields_only_substrate() {
        let pcb = board_with(vec![], vec![]);
        let meshes = pcb_preview_meshes(&pcb);
        // A bare board still gives a satin soldermask + a matte substrate edge.
        let mask = meshes.iter().find(|m| m.role == "mask").unwrap();
        assert!(!mask.positions.is_empty());
        assert_eq!(mask.normals.len(), mask.positions.len());
        assert_eq!(mask.color, SOLDERMASK_GREEN);
        // The mask carries a soft clearcoat sheen (not a wet mirror); the edge
        // is bare fiberglass and has none.
        assert!(
            mask.clearcoat > 0.0 && mask.clearcoat <= 0.6,
            "soldermask should be a satin sheen, got {}",
            mask.clearcoat
        );
        let edge = meshes.iter().find(|m| m.role == "substrate").unwrap();
        assert_eq!(edge.color, FR4_EDGE_TAN);
        assert_eq!(edge.clearcoat, 0.0);
    }

    #[test]
    fn populated_board_has_all_layers() {
        let trace = Trace {
            net: "GND".into(),
            start: Vec2::new(5.0, 5.0),
            end: Vec2::new(20.0, 5.0),
            width: 0.25,
            layer: PcbLayer::FCu,
            source: None,
        };
        let pcb = board_with(
            vec![chip("R1", 10.0, 10.0), chip("R2", 25.0, 20.0)],
            vec![trace],
        );
        let meshes = pcb_preview_meshes(&pcb);

        let roles: Vec<&str> = meshes.iter().map(|m| m.role.as_str()).collect();
        assert!(roles.contains(&"mask"), "roles: {roles:?}");
        assert!(roles.contains(&"substrate"), "roles: {roles:?}");
        // Pads are exposed gold copper; the trace is mask-clad green copper.
        assert!(roles.contains(&"copper"), "roles: {roles:?}");
        assert!(roles.contains(&"copper_masked"), "roles: {roles:?}");
        assert!(roles.contains(&"component"), "roles: {roles:?}");
        assert!(roles.contains(&"silkscreen"), "roles: {roles:?}");

        // Every mesh must be well-formed: matching normals, in-range indices.
        for m in &meshes {
            assert_eq!(m.normals.len(), m.positions.len(), "role {}", m.role);
            let vcount = (m.positions.len() / 3) as u32;
            assert!(m.indices.iter().all(|&i| i < vcount), "role {}", m.role);
            assert!(!m.indices.is_empty(), "role {}", m.role);
        }

        // Board is centered on z=0: top surface near +thickness/2.
        let board = meshes.iter().find(|m| m.role == "mask").unwrap();
        let max_z = board
            .positions
            .iter()
            .skip(2)
            .step_by(3)
            .cloned()
            .fold(f32::MIN, f32::max);
        assert!((max_z - 0.8).abs() < 0.1, "board top z = {max_z}");

        // Copper sits proud of the board top (lifted above +thickness/2).
        let copper = meshes.iter().find(|m| m.role == "copper").unwrap();
        let cu_max_z = copper
            .positions
            .iter()
            .skip(2)
            .step_by(3)
            .cloned()
            .fold(f32::MIN, f32::max);
        assert!(
            cu_max_z >= max_z,
            "copper {cu_max_z} should be >= board {max_z}"
        );
    }

    #[test]
    fn silkscreen_is_double_sided() {
        // Silk is a flat, zero-thickness ribbon. A single winding faces only one
        // way, so it backface-culls from the opposite side — which is exactly why
        // it vanished from a top-down view. Both faces must be emitted, so the
        // layer's per-vertex normals span +Z and -Z.
        let pcb = board_with(vec![chip("R1", 10.0, 10.0)], vec![]);
        let meshes = pcb_preview_meshes(&pcb);
        let silk = meshes.iter().find(|m| m.role == "silkscreen").unwrap();
        let nz: Vec<f32> = silk.normals.iter().skip(2).step_by(3).cloned().collect();
        assert!(
            nz.iter().any(|&n| n > 0.5),
            "silk has no up-facing face (would cull from above)"
        );
        assert!(
            nz.iter().any(|&n| n < -0.5),
            "silk has no down-facing face (would cull from below)"
        );
    }
}
