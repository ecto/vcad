//! Parametric 3D component body mesh generation.
//!
//! Generates triangle-mesh models for common component packages (chip
//! resistors/capacitors, SOICs, QFPs, DIPs, SOT-23, QFN, pin headers, LEDs,
//! …) plus the small details that make a populated board read as *real*:
//! bright-tin solder joints on every SMD pad, matte-black epoxy IC bodies
//! (neutral and dark so they don't bloom purple under a bright studio IBL),
//! ceramic-tan MLCC bodies, pin-1 markers, and emissive LED lenses.
//!
//! Materials are authored as **linear** RGB `[0,1]` tuned for an ACES-filmic
//! tonemapped, studio-IBL lit scene (the vcad viewport / MCP viewer rig). The
//! cardinal rule for dark dielectrics: keep them *neutral* (R≈G≈B) and *rough*
//! so the specular lobe never concentrates the cool key light into a visible
//! colored glint.

use serde::{Deserialize, Serialize};
use vcad_ir::ecad::{Footprint, PadShape, PadType, Pcb};

use crate::geometry::footprint_bounds;

/// A generated 3D component mesh.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentMesh {
    /// Footprint reference designator.
    pub footprint_ref: String,
    /// Flat vertex positions `[x,y,z, ...]`.
    pub positions: Vec<f32>,
    /// Triangle indices.
    pub indices: Vec<u32>,
    /// Vertex normals `[nx,ny,nz, ...]`.
    pub normals: Vec<f32>,
    /// RGB color `[r, g, b]` in 0..1 range (linear).
    pub color: [f32; 3],
    /// Metalness (0.0 = dielectric, 1.0 = metal).
    pub metalness: f32,
    /// Roughness (0..1). Defaults preserved for older deserialized payloads.
    #[serde(default = "default_roughness")]
    pub roughness: f32,
    /// Emissive color `[r, g, b]` in 0..1 (linear); `[0,0,0]` = not emissive.
    #[serde(default)]
    pub emissive: [f32; 3],
}

fn default_roughness() -> f32 {
    0.5
}

/// Generate component body meshes for all footprints on a PCB.
pub fn generate_component_meshes(pcb: &Pcb) -> Vec<ComponentMesh> {
    let board_z = pcb.outline.thickness;
    let mut meshes = Vec::new();

    for fp in &pcb.footprints {
        let z_base = if fp.front { board_z } else { 0.0 };
        let z_dir: f32 = if fp.front { 1.0 } else { -1.0 };

        let name = &fp.footprint_name;
        meshes.extend(generate_for_footprint(fp, name, z_base, z_dir));

        // Bright-tin solder joints on every SMD pad — the single biggest cue
        // that separates a "render" from a real, reflowed board.
        add_pad_solder(fp, z_base as f32, z_dir, &mut meshes);
    }

    meshes
}

fn generate_for_footprint(
    fp: &Footprint,
    name: &str,
    z_base: f64,
    z_dir: f32,
) -> Vec<ComponentMesh> {
    let cx = fp.position.x as f32;
    let cy = fp.position.y as f32;
    let zb = z_base as f32;

    // LEDs first — an "LED" token can also carry a chip size (e.g. LED_0805),
    // so route it before the generic chip matcher.
    if is_led(name) {
        return led_model(fp, name, cx, cy, zb, z_dir);
    }

    // Match footprint name to package type
    if let Some(chip) = parse_chip_size(name) {
        return chip_model(fp, chip, cx, cy, zb, z_dir);
    }
    if let Some(pins) = parse_soic(name) {
        return ic_with_marker(fp, 3.9, soic_length(pins), 1.75, cx, cy, zb, z_dir);
    }
    if let Some(pins) = parse_qfp(name) {
        let body = qfp_body_size(pins);
        return ic_with_marker(fp, body, body, 1.6, cx, cy, zb, z_dir);
    }
    // QFN/no-lead: source the 3D body from the SAME parametric generator that
    // produced the land pattern, so footprint and body cannot drift.
    if let Some((pins, pitch, body)) = parse_qfn(name) {
        if let Ok(d) =
            vcad_ecad_package::derive(&vcad_ecad_package::presets::qfn(pins, pitch, body, 0.0))
        {
            let bb = d.body.aabb();
            let (w, l, h) = (
                (bb.max.x - bb.min.x) as f32,
                (bb.max.y - bb.min.y) as f32,
                bb.height() as f32,
            );
            return ic_with_marker(fp, w, l, h, cx, cy, zb, z_dir);
        }
    }
    if let Some(pins) = parse_dip(name) {
        return ic_with_marker(fp, 7.62, dip_length(pins), 4.0, cx, cy, zb, z_dir);
    }
    if name.contains("SOT-23") || name.contains("SOT23") {
        return ic_with_marker(fp, 2.9, 1.3, 1.1, cx, cy, zb, z_dir);
    }
    if name.contains("SOT-223") || name.contains("SOT223") {
        return ic_with_marker(fp, 6.5, 3.5, 1.6, cx, cy, zb, z_dir);
    }
    if name.contains("PinHeader") {
        return pin_header_model(fp, cx, cy, zb, z_dir);
    }

    // Fallback: box from pad extents
    fallback_model(fp, cx, cy, zb, z_dir)
}

// ============================================================================
// Materials (linear RGB, tuned for ACES + studio IBL — see module docs)
// ============================================================================

/// A PBR material: linear base color, metalness, roughness, emissive.
#[derive(Clone, Copy)]
struct Mat {
    color: [f32; 3],
    metal: f32,
    rough: f32,
    emissive: [f32; 3],
}

impl Mat {
    const fn new(color: [f32; 3], metal: f32, rough: f32) -> Self {
        Mat {
            color,
            metal,
            rough,
            emissive: [0.0, 0.0, 0.0],
        }
    }
    const fn emissive(color: [f32; 3], rough: f32, emissive: [f32; 3]) -> Self {
        Mat {
            color,
            metal: 0.0,
            rough,
            emissive,
        }
    }
}

// Matte-black epoxy IC body. THE fix for the lavender bug: neutral (R≈G≈B),
// very dark so the diffuse term is ~0, and rough enough that the spec lobe
// never concentrates the cool key light into a purple glint.
const IC_BODY: Mat = Mat::new([0.035, 0.035, 0.037], 0.0, 0.62);
// Thick-film resistor body (warm-neutral near-black glaze).
const RESISTOR_BODY: Mat = Mat::new([0.045, 0.042, 0.040], 0.0, 0.55);
// MLCC ceramic capacitor body (sintered tan/beige).
const MLCC_BODY: Mat = Mat::new([0.52, 0.43, 0.30], 0.0, 0.62);
// Inductor / ferrite body (dark charcoal).
const INDUCTOR_BODY: Mat = Mat::new([0.06, 0.06, 0.065], 0.0, 0.58);
// Terminated chip end-cap (solder-over-Ni, tin family).
const END_CAP: Mat = Mat::new([0.58, 0.59, 0.60], 1.0, 0.40);
// Fresh solder joint (SAC/tin — NOT chrome, NOT gold).
const SOLDER: Mat = Mat::new([0.62, 0.64, 0.66], 1.0, 0.32);
// Black plastic connector / pin-header housing.
const HOUSING_BLACK: Mat = Mat::new([0.05, 0.05, 0.055], 0.0, 0.55);
// Pin-1 marker dot — a touch lighter than the body so it reads on black epoxy.
const PIN1_DOT: Mat = Mat::new([0.22, 0.22, 0.24], 0.0, 0.5);

struct ChipDims {
    body_w: f32,
    body_h: f32,
    height: f32,
    cap_w: f32,
}

fn parse_chip_size(name: &str) -> Option<ChipDims> {
    if name.contains("0402") {
        Some(ChipDims {
            body_w: 1.0,
            body_h: 0.5,
            height: 0.35,
            cap_w: 0.25,
        })
    } else if name.contains("0603") {
        Some(ChipDims {
            body_w: 1.6,
            body_h: 0.8,
            height: 0.45,
            cap_w: 0.3,
        })
    } else if name.contains("0805") {
        Some(ChipDims {
            body_w: 2.0,
            body_h: 1.25,
            height: 0.5,
            cap_w: 0.4,
        })
    } else if name.contains("1206") {
        Some(ChipDims {
            body_w: 3.2,
            body_h: 1.6,
            height: 0.55,
            cap_w: 0.5,
        })
    } else {
        None
    }
}

fn parse_soic(name: &str) -> Option<u32> {
    if name.contains("SOIC-8") || name.contains("SOIC8") {
        Some(8)
    } else if name.contains("SOIC-14") || name.contains("SOIC14") {
        Some(14)
    } else if name.contains("SOIC-16") || name.contains("SOIC16") {
        Some(16)
    } else {
        None
    }
}

fn parse_qfp(name: &str) -> Option<u32> {
    if name.contains("QFP-32") || name.contains("QFP32") {
        Some(32)
    } else if name.contains("QFP-48") || name.contains("QFP48") {
        Some(48)
    } else if name.contains("QFP-64") || name.contains("QFP64") {
        Some(64)
    } else {
        None
    }
}

/// Parse `(pins, pitch_mm, body_mm)` from a QFN/no-lead footprint id such as
/// `"QFN-40_5x5mm_P0.4mm"`. Pitch and body fall back to sane defaults when the
/// id omits them.
fn parse_qfn(name: &str) -> Option<(u32, f64, f64)> {
    let markers = ["QFN-", "VQFN-", "UQFN-", "WQFN-", "TQFN-", "DHVQFN-"];
    let pins = markers.iter().find_map(|m| uint_after(name, m))?;
    if pins < 4 {
        return None;
    }
    let pitch = float_after(name, "_P").unwrap_or(0.5);
    // Body: look for a "<w>x<h>mm" token; else estimate from the pad ring.
    let body = name
        .split(['_', ':'])
        .find_map(parse_size_token)
        .unwrap_or((pins as f64 / 4.0) * pitch + 1.0);
    Some((pins, pitch, body))
}

/// First unsigned integer immediately following `marker` in `s`.
fn uint_after(s: &str, marker: &str) -> Option<u32> {
    let rest = &s[s.find(marker)? + marker.len()..];
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// First float immediately following `marker` in `s`.
fn float_after(s: &str, marker: &str) -> Option<f64> {
    let rest = &s[s.find(marker)? + marker.len()..];
    let num: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    num.parse().ok()
}

/// Parse the leading width from a `"5x5mm"` / `"3.5x3.5mm"` token.
fn parse_size_token(tok: &str) -> Option<f64> {
    let t = tok.strip_suffix("mm").unwrap_or(tok);
    let (w, h) = t.split_once('x')?;
    let w: f64 = w.parse().ok()?;
    let _h: f64 = h.parse().ok()?;
    Some(w)
}

fn parse_dip(name: &str) -> Option<u32> {
    for n in &[40u32, 28, 16, 14, 8] {
        if name.contains(&format!("DIP-{n}")) || name.contains(&format!("DIP{n}")) {
            return Some(*n);
        }
    }
    None
}

fn soic_length(pins: u32) -> f32 {
    let half = pins / 2;
    half as f32 * 1.27
}

fn qfp_body_size(pins: u32) -> f32 {
    match pins {
        32 => 7.0,
        48 => 9.0,
        _ => 12.0,
    }
}

fn dip_length(pins: u32) -> f32 {
    let half = pins / 2;
    half as f32 * 2.54
}

/// Is this footprint a light-emitting diode? Matches a delimited `LED` token
/// (e.g. `LED_SMD:LED_0805`, `LED_THT:LED_D5.0mm`) rather than any incidental
/// "LED" substring, so a package id that merely contains those letters can't
/// hijack the chip/IC matchers.
fn is_led(name: &str) -> bool {
    let up = name.to_ascii_uppercase();
    up.starts_with("LED") || up.contains("LED_") || up.contains("_LED") || up.contains(":LED")
}

// ============================================================================
// Package-specific models
// ============================================================================

/// Pick the chip body material from the reference designator: `C…` is an MLCC
/// (ceramic tan), `L…` an inductor (charcoal), everything else a resistor.
fn chip_body_mat(reference: &str) -> Mat {
    match reference.chars().next().map(|c| c.to_ascii_uppercase()) {
        Some('C') => MLCC_BODY,
        Some('L') => INDUCTOR_BODY,
        _ => RESISTOR_BODY,
    }
}

/// Chip resistor/capacitor/inductor: type-colored body + metallic end caps.
fn chip_model(
    fp: &Footprint,
    dims: ChipDims,
    cx: f32,
    cy: f32,
    zb: f32,
    z_dir: f32,
) -> Vec<ComponentMesh> {
    let hw = dims.body_w / 2.0;
    let hh = dims.body_h / 2.0;
    let h = dims.height;

    let rot = (fp.rotation as f32).to_radians();
    let cos_r = rot.cos();
    let sin_r = rot.sin();

    let body_mat = chip_body_mat(&fp.reference);
    let mut meshes = Vec::new();

    // Main body (center section)
    let inner_hw = hw - dims.cap_w;
    meshes.push(make_box(
        cx,
        cy,
        zb,
        inner_hw,
        hh,
        h,
        z_dir,
        cos_r,
        sin_r,
        body_mat,
        &fp.reference,
    ));

    // End caps (left and right), proud-overlapping the body like real terminations.
    for sign in [-1.0f32, 1.0] {
        let cap_local = sign * (hw - dims.cap_w / 2.0);
        let cap_cx = cx + cap_local * cos_r;
        let cap_cy = cy + cap_local * sin_r;
        meshes.push(make_box(
            cap_cx,
            cap_cy,
            zb,
            dims.cap_w / 2.0,
            hh * 1.02,
            h * 1.02,
            z_dir,
            cos_r,
            sin_r,
            END_CAP,
            &fp.reference,
        ));
    }

    meshes
}

/// IC body (SOIC/QFP/DIP/SOT/QFN) plus a pin-1 marker dot on the top face.
#[allow(clippy::too_many_arguments)]
fn ic_with_marker(
    fp: &Footprint,
    w: f32,
    l: f32,
    h: f32,
    cx: f32,
    cy: f32,
    zb: f32,
    z_dir: f32,
) -> Vec<ComponentMesh> {
    let rot = (fp.rotation as f32).to_radians();
    let cos_r = rot.cos();
    let sin_r = rot.sin();

    let mut meshes = vec![make_box(
        cx,
        cy,
        zb,
        w / 2.0,
        l / 2.0,
        h,
        z_dir,
        cos_r,
        sin_r,
        IC_BODY,
        &fp.reference,
    )];

    // Pin-1 dot near one corner of the top face, sitting just proud of the body.
    let dot_r = (w.min(l) * 0.09).clamp(0.12, 0.4);
    let inset = dot_r + 0.18;
    let lx = -(w / 2.0 - inset);
    let ly = -(l / 2.0 - inset);
    let dot_cx = cx + lx * cos_r - ly * sin_r;
    let dot_cy = cy + lx * sin_r + ly * cos_r;
    let dot_top = zb + h * z_dir;
    meshes.push(make_box(
        dot_cx,
        dot_cy,
        dot_top,
        dot_r,
        dot_r,
        0.03,
        z_dir,
        cos_r,
        sin_r,
        PIN1_DOT,
        &fp.reference,
    ));

    meshes
}

/// LED: emissive chip lens. Uses chip dimensions when the id carries a size,
/// else a small default body.
fn led_model(
    fp: &Footprint,
    name: &str,
    cx: f32,
    cy: f32,
    zb: f32,
    z_dir: f32,
) -> Vec<ComponentMesh> {
    let dims = parse_chip_size(name).unwrap_or(ChipDims {
        body_w: 1.6,
        body_h: 0.8,
        height: 0.7,
        cap_w: 0.3,
    });
    let rot = (fp.rotation as f32).to_radians();
    let cos_r = rot.cos();
    let sin_r = rot.sin();

    // Warm-white phosphor lens that genuinely glows under bright IBL.
    let lens = Mat::emissive([0.95, 0.85, 0.55], 0.25, [1.0, 0.72, 0.32]);
    vec![make_box(
        cx,
        cy,
        zb,
        dims.body_w / 2.0,
        dims.body_h / 2.0,
        dims.height,
        z_dir,
        cos_r,
        sin_r,
        lens,
        &fp.reference,
    )]
}

/// Pin header: black plastic housing.
fn pin_header_model(fp: &Footprint, cx: f32, cy: f32, zb: f32, z_dir: f32) -> Vec<ComponentMesh> {
    let (min, max) = footprint_bounds(fp);
    let w = (max.x - min.x) as f32 + 1.0;
    let l = (max.y - min.y) as f32 + 1.0;
    let h = 8.5_f32;

    let rot = (fp.rotation as f32).to_radians();
    vec![make_box(
        cx,
        cy,
        zb,
        w / 2.0,
        l / 2.0,
        h,
        z_dir,
        rot.cos(),
        rot.sin(),
        HOUSING_BLACK,
        &fp.reference,
    )]
}

/// Fallback: box from pad extents (dark IC-ish body).
fn fallback_model(fp: &Footprint, cx: f32, cy: f32, zb: f32, z_dir: f32) -> Vec<ComponentMesh> {
    let (min, max) = footprint_bounds(fp);
    let w = (max.x - min.x) as f32;
    let l = (max.y - min.y) as f32;
    if w < 0.1 || l < 0.1 {
        return vec![];
    }

    let rot = (fp.rotation as f32).to_radians();
    vec![make_box(
        cx,
        cy,
        zb,
        w / 2.0,
        l / 2.0,
        1.0,
        z_dir,
        rot.cos(),
        rot.sin(),
        IC_BODY,
        &fp.reference,
    )]
}

// ============================================================================
// Solder joints
// ============================================================================

/// Full extents `(width, height)` in mm of a pad's bounding box plus its center
/// `(cx, cy)` in pad-local coordinates. Standard shapes are centered on the pad
/// origin (`(0, 0)`); `Custom` polygons return their true bbox and centroid, so
/// an off-origin polygon's solder joint lands on the polygon, not the origin.
fn pad_extent(shape: &PadShape) -> (f64, f64, f64, f64) {
    match shape {
        PadShape::Circle { diameter } => (*diameter, *diameter, 0.0, 0.0),
        PadShape::Rect { width, height }
        | PadShape::Oval { width, height }
        | PadShape::RoundRect { width, height, .. } => (*width, *height, 0.0, 0.0),
        PadShape::Custom { vertices } => {
            let mut min_x = f64::MAX;
            let mut min_y = f64::MAX;
            let mut max_x = f64::MIN;
            let mut max_y = f64::MIN;
            for v in vertices {
                min_x = min_x.min(v.x);
                max_x = max_x.max(v.x);
                min_y = min_y.min(v.y);
                max_y = max_y.max(v.y);
            }
            if !min_x.is_finite() {
                return (0.2, 0.2, 0.0, 0.0);
            }
            (
                (max_x - min_x).max(0.2),
                (max_y - min_y).max(0.2),
                (min_x + max_x) / 2.0,
                (min_y + max_y) / 2.0,
            )
        }
    }
}

/// Append a bright-tin solder joint over every SMD pad of `fp`. A low box
/// slightly inset from the pad, proud of the copper — under a studio IBL the
/// metallic tin reads as a reflowed joint against the matte board.
fn add_pad_solder(fp: &Footprint, zb: f32, z_dir: f32, out: &mut Vec<ComponentMesh>) {
    for pad in &fp.pads {
        if pad.pad_type != PadType::SMD {
            continue;
        }
        let (pw, ph, lcx, lcy) = pad_extent(&pad.shape);
        if pw < 1e-3 || ph < 1e-3 {
            continue;
        }
        let world = crate::geometry::pad_world_position(fp, pad);
        let (px, py) = (world.x, world.y);
        let pad_rot = ((fp.rotation + pad.rotation) as f32).to_radians();
        // Shift the joint to the pad-local bbox center (nonzero only for an
        // off-origin Custom polygon), rotated into world by the pad rotation.
        let (cr, sr) = (pad_rot.cos() as f64, pad_rot.sin() as f64);
        let cx = px + lcx * cr - lcy * sr;
        let cy = py + lcx * sr + lcy * cr;

        out.push(make_box(
            cx as f32,
            cy as f32,
            zb,
            (pw as f32 / 2.0) * 0.92,
            (ph as f32 / 2.0) * 0.92,
            0.14,
            z_dir,
            pad_rot.cos(),
            pad_rot.sin(),
            SOLDER,
            &fp.reference,
        ));
    }
}

// ============================================================================
// Box mesh primitive
// ============================================================================

/// Create a rotated box mesh centered at `(cx, cy)` with given half-extents.
#[allow(clippy::too_many_arguments)]
fn make_box(
    cx: f32,
    cy: f32,
    z_base: f32,
    hw: f32,
    hh: f32,
    height: f32,
    z_dir: f32,
    cos_r: f32,
    sin_r: f32,
    mat: Mat,
    fp_ref: &str,
) -> ComponentMesh {
    let z0 = z_base;
    let z1 = z_base + height * z_dir;
    let (z_lo, z_hi) = if z0 < z1 { (z0, z1) } else { (z1, z0) };

    // 4 corners in local space, rotated to world
    let corners: [(f32, f32); 4] = [(-hw, -hh), (hw, -hh), (hw, hh), (-hw, hh)];

    let mut positions = Vec::with_capacity(24 * 3);
    let mut normals = Vec::with_capacity(24 * 3);
    let mut indices = Vec::with_capacity(36);

    // Transform corner to world
    let t = |lx: f32, ly: f32| -> (f32, f32) {
        (cx + lx * cos_r - ly * sin_r, cy + lx * sin_r + ly * cos_r)
    };

    // 8 unique positions, but we need 24 (4 per face) for correct normals
    // Face order: top, bottom, front, back, left, right

    // Helper to add a quad face (4 vertices, 2 triangles)
    let mut add_face = |v: [(f32, f32, f32); 4], nx: f32, ny: f32, nz: f32| {
        let base = (positions.len() / 3) as u32;
        for (vx, vy, vz) in &v {
            positions.extend_from_slice(&[*vx, *vy, *vz]);
            normals.extend_from_slice(&[nx, ny, nz]);
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    };

    let c = corners.map(|(lx, ly)| t(lx, ly));

    // Top face (+Z)
    add_face(
        [
            (c[0].0, c[0].1, z_hi),
            (c[1].0, c[1].1, z_hi),
            (c[2].0, c[2].1, z_hi),
            (c[3].0, c[3].1, z_hi),
        ],
        0.0,
        0.0,
        1.0,
    );
    // Bottom face (-Z)
    add_face(
        [
            (c[3].0, c[3].1, z_lo),
            (c[2].0, c[2].1, z_lo),
            (c[1].0, c[1].1, z_lo),
            (c[0].0, c[0].1, z_lo),
        ],
        0.0,
        0.0,
        -1.0,
    );
    // Front face (edge 0-1)
    add_face(
        [
            (c[0].0, c[0].1, z_lo),
            (c[1].0, c[1].1, z_lo),
            (c[1].0, c[1].1, z_hi),
            (c[0].0, c[0].1, z_hi),
        ],
        -sin_r,
        cos_r,
        0.0,
    );
    // Back face (edge 2-3)
    add_face(
        [
            (c[2].0, c[2].1, z_lo),
            (c[3].0, c[3].1, z_lo),
            (c[3].0, c[3].1, z_hi),
            (c[2].0, c[2].1, z_hi),
        ],
        sin_r,
        -cos_r,
        0.0,
    );
    // Right face (edge 1-2)
    add_face(
        [
            (c[1].0, c[1].1, z_lo),
            (c[2].0, c[2].1, z_lo),
            (c[2].0, c[2].1, z_hi),
            (c[1].0, c[1].1, z_hi),
        ],
        cos_r,
        sin_r,
        0.0,
    );
    // Left face (edge 3-0)
    add_face(
        [
            (c[3].0, c[3].1, z_lo),
            (c[0].0, c[0].1, z_lo),
            (c[0].0, c[0].1, z_hi),
            (c[3].0, c[3].1, z_hi),
        ],
        -cos_r,
        -sin_r,
        0.0,
    );

    ComponentMesh {
        footprint_ref: fp_ref.to_string(),
        positions,
        indices,
        normals,
        color: mat.color,
        metalness: mat.metal,
        roughness: mat.rough,
        emissive: mat.emissive,
    }
}

/// Get the expected component height for a footprint name.
/// Used by the evaluator to set accurate bounding boxes.
pub fn package_height(footprint_name: &str) -> f64 {
    if footprint_name.contains("0402") {
        return 0.35;
    }
    if footprint_name.contains("0603") {
        return 0.45;
    }
    if footprint_name.contains("0805") {
        return 0.5;
    }
    if footprint_name.contains("1206") {
        return 0.55;
    }
    if footprint_name.contains("SOIC") {
        return 1.75;
    }
    if footprint_name.contains("QFP") {
        return 1.6;
    }
    if footprint_name.contains("DIP") {
        return 4.0;
    }
    if footprint_name.contains("SOT-23") || footprint_name.contains("SOT23") {
        return 1.1;
    }
    if footprint_name.contains("SOT-223") || footprint_name.contains("SOT223") {
        return 1.6;
    }
    if footprint_name.contains("PinHeader") {
        return 8.5;
    }
    1.0 // default fallback
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_ir::ecad::*;
    use vcad_ir::Vec2;

    fn test_pcb() -> Pcb {
        Pcb {
            outline: BoardOutline {
                vertices: vec![
                    Vec2::new(0.0, 0.0),
                    Vec2::new(50.0, 0.0),
                    Vec2::new(50.0, 50.0),
                    Vec2::new(0.0, 50.0),
                ],
                cutouts: vec![],
                thickness: 1.6,
            },
            stackup: LayerStackup {
                layers: vec![
                    StackupLayer {
                        layer: PcbLayer::FCu,
                        copper_thickness: Some(0.035),
                        dielectric_thickness: Some(1.5),
                        dielectric_er: Some(4.5),
                        material: Some("FR4".into()),
                    },
                    StackupLayer {
                        layer: PcbLayer::BCu,
                        copper_thickness: Some(0.035),
                        dielectric_thickness: None,
                        dielectric_er: None,
                        material: None,
                    },
                ],
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
                    target_impedance: None,
                    target_diff_impedance: None,
                },
                class_rules: vec![],
                net_class_assignments: Default::default(),
                edge_clearance: 0.5,
                hole_to_hole: 0.5,
                min_annular_ring: 0.15,
                min_drill: 0.2,
            },
            footprints: vec![Footprint {
                reference: "R1".into(),
                value: "10k".into(),
                footprint_name: "0805".into(),
                position: Vec2::new(25.0, 25.0),
                rotation: 0.0,
                front: true,
                pads: vec![
                    Pad {
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
                    },
                    Pad {
                        number: "2".into(),
                        pad_type: PadType::SMD,
                        shape: PadShape::Rect {
                            width: 1.0,
                            height: 1.2,
                        },
                        position: Vec2::new(1.0, 0.0),
                        rotation: 0.0,
                        drill: None,
                        net: None,
                        layers: vec![PcbLayer::FCu],
                    },
                ],
                graphics: vec![],
                model_3d: None,
                properties: Default::default(),
            }],
            traces: vec![],
            trace_arcs: vec![],
            vias: vec![],
            zones: vec![],
            keepouts: vec![],
            net_ties: vec![],
        }
    }

    #[test]
    fn generates_chip_meshes() {
        let pcb = test_pcb();
        let meshes = generate_component_meshes(&pcb);
        // Chip model: body + 2 end caps; plus 1 solder joint per SMD pad (2).
        assert_eq!(meshes.len(), 5);
        for m in &meshes {
            assert_eq!(m.footprint_ref, "R1");
            assert!(!m.positions.is_empty());
            assert!(!m.indices.is_empty());
            assert_eq!(m.normals.len(), m.positions.len());
        }
        // A resistor body is the dark thick-film material (never the old
        // blue-tinted box that bloomed purple under IBL).
        let body = &meshes[0];
        assert!(
            body.color[2] <= body.color[0] + 0.01,
            "body must not be blue-biased"
        );
        assert!(
            body.color.iter().all(|&c| c < 0.1),
            "resistor body must be dark"
        );
        // Solder joints are metallic tin.
        assert!(
            meshes
                .iter()
                .any(|m| m.metalness > 0.9 && m.roughness < 0.4),
            "expected metallic solder joints"
        );
    }

    #[test]
    fn mlcc_capacitor_is_tan() {
        let mut pcb = test_pcb();
        pcb.footprints[0].reference = "C3".into();
        let meshes = generate_component_meshes(&pcb);
        let body = &meshes[0];
        // Ceramic tan: red channel clearly above blue.
        assert!(
            body.color[0] > body.color[2] + 0.1,
            "MLCC body should be warm/tan"
        );
    }

    #[test]
    fn led_is_emissive() {
        let mut pcb = test_pcb();
        pcb.footprints[0].reference = "D1".into();
        pcb.footprints[0].footprint_name = "LED_0805".into();
        let meshes = generate_component_meshes(&pcb);
        assert!(
            meshes.iter().any(|m| m.emissive.iter().any(|&e| e > 0.1)),
            "LED lens should be emissive"
        );
    }

    #[test]
    fn qfn_parses_and_body_from_generator() {
        let (pins, pitch, body) = parse_qfn("Package_DFN_QFN:QFN-40_5x5mm_P0.4mm").unwrap();
        assert_eq!(pins, 40);
        assert!((pitch - 0.4).abs() < 1e-9);
        assert!((body - 5.0).abs() < 1e-9);
        let d = vcad_ecad_package::derive(&vcad_ecad_package::presets::qfn(pins, pitch, body, 0.0))
            .unwrap();
        let bb = d.body.aabb();
        // Body is the 5×5mm package, ~0.9mm tall — sourced from the generator.
        assert!((bb.max.x - bb.min.x - 5.0).abs() < 1e-6);
        assert!((bb.height() - 0.9).abs() < 1e-6);
    }

    #[test]
    fn package_heights() {
        assert!((package_height("0805") - 0.5).abs() < 0.01);
        assert!((package_height("SOIC-8") - 1.75).abs() < 0.01);
        assert!((package_height("DIP-14") - 4.0).abs() < 0.01);
        assert!((package_height("SOT-23") - 1.1).abs() < 0.01);
        assert!((package_height("unknown") - 1.0).abs() < 0.01);
    }
}
