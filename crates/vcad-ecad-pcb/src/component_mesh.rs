//! Parametric 3D component body mesh generation.
//!
//! Generates simple triangle mesh models for common component packages
//! (chip resistors/capacitors, SOICs, QFPs, DIPs, SOT-23, etc.).

use serde::{Deserialize, Serialize};
use vcad_ir::ecad::{Footprint, Pcb};

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
    /// RGB color `[r, g, b]` in 0..1 range.
    pub color: [f32; 3],
    /// Metalness (0.0 = dielectric, 1.0 = metal).
    pub metalness: f32,
}

/// Generate component body meshes for all footprints on a PCB.
pub fn generate_component_meshes(pcb: &Pcb) -> Vec<ComponentMesh> {
    let board_z = pcb.outline.thickness;
    let mut meshes = Vec::new();

    for fp in &pcb.footprints {
        let z_base = if fp.front { board_z } else { 0.0 };
        let z_dir: f32 = if fp.front { 1.0 } else { -1.0 };

        let name = &fp.footprint_name;
        let parts = generate_for_footprint(fp, name, z_base, z_dir);
        meshes.extend(parts);
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

    // Match footprint name to package type
    if let Some(chip) = parse_chip_size(name) {
        return chip_model(fp, chip, cx, cy, zb, z_dir);
    }
    if let Some(pins) = parse_soic(name) {
        return vec![ic_body_model(
            fp,
            3.9,
            soic_length(pins),
            1.75,
            cx,
            cy,
            zb,
            z_dir,
            IC_COLOR,
        )];
    }
    if let Some(pins) = parse_qfp(name) {
        let body = qfp_body_size(pins);
        return vec![ic_body_model(
            fp, body, body, 1.6, cx, cy, zb, z_dir, IC_COLOR,
        )];
    }
    if let Some(pins) = parse_dip(name) {
        return vec![ic_body_model(
            fp,
            7.62,
            dip_length(pins),
            4.0,
            cx,
            cy,
            zb,
            z_dir,
            IC_COLOR,
        )];
    }
    if name.contains("SOT-23") || name.contains("SOT23") {
        return vec![ic_body_model(
            fp, 2.9, 1.3, 1.1, cx, cy, zb, z_dir, IC_COLOR,
        )];
    }
    if name.contains("SOT-223") || name.contains("SOT223") {
        return vec![ic_body_model(
            fp, 6.5, 3.5, 1.6, cx, cy, zb, z_dir, IC_COLOR,
        )];
    }
    if name.contains("PinHeader") {
        return pin_header_model(fp, cx, cy, zb, z_dir);
    }

    // Fallback: box from pad extents
    fallback_model(fp, cx, cy, zb, z_dir)
}

// ============================================================================
// Package-specific models
// ============================================================================

const IC_COLOR: [f32; 3] = [0.1, 0.1, 0.18]; // dark IC body
const CHIP_BODY_COLOR: [f32; 3] = [0.1, 0.1, 0.1]; // dark chip body
const CHIP_CAP_COLOR: [f32; 3] = [0.75, 0.75, 0.75]; // silver end caps
const PIN_HEADER_BODY: [f32; 3] = [0.07, 0.07, 0.07]; // black housing
const _PIN_HEADER_PIN: [f32; 3] = [0.83, 0.66, 0.26]; // gold pins (reserved for future pin cylinders)

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

/// Chip resistor/capacitor: dark body + metallic end caps.
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
        CHIP_BODY_COLOR,
        0.0,
        &fp.reference,
    ));

    // Left end cap
    let cap_cx_local = -(hw - dims.cap_w / 2.0);
    let cap_cx = cx + cap_cx_local * cos_r;
    let cap_cy = cy + cap_cx_local * sin_r;
    meshes.push(make_box(
        cap_cx,
        cap_cy,
        zb,
        dims.cap_w / 2.0,
        hh,
        h,
        z_dir,
        cos_r,
        sin_r,
        CHIP_CAP_COLOR,
        0.8,
        &fp.reference,
    ));

    // Right end cap
    let cap_cx_local = hw - dims.cap_w / 2.0;
    let cap_cx = cx + cap_cx_local * cos_r;
    let cap_cy = cy + cap_cx_local * sin_r;
    meshes.push(make_box(
        cap_cx,
        cap_cy,
        zb,
        dims.cap_w / 2.0,
        hh,
        h,
        z_dir,
        cos_r,
        sin_r,
        CHIP_CAP_COLOR,
        0.8,
        &fp.reference,
    ));

    meshes
}

/// Generic IC body (SOIC, QFP, DIP, SOT).
#[allow(clippy::too_many_arguments)]
fn ic_body_model(
    fp: &Footprint,
    w: f32,
    l: f32,
    h: f32,
    cx: f32,
    cy: f32,
    zb: f32,
    z_dir: f32,
    color: [f32; 3],
) -> ComponentMesh {
    let rot = (fp.rotation as f32).to_radians();
    make_box(
        cx,
        cy,
        zb,
        w / 2.0,
        l / 2.0,
        h,
        z_dir,
        rot.cos(),
        rot.sin(),
        color,
        0.0,
        &fp.reference,
    )
}

/// Pin header: housing + individual pins.
fn pin_header_model(fp: &Footprint, cx: f32, cy: f32, zb: f32, z_dir: f32) -> Vec<ComponentMesh> {
    let (min, max) = footprint_bounds(fp);
    let w = (max.x - min.x) as f32 + 1.0;
    let l = (max.y - min.y) as f32 + 1.0;
    let h = 8.5_f32;

    let rot = (fp.rotation as f32).to_radians();
    let cos_r = rot.cos();
    let sin_r = rot.sin();

    vec![make_box(
        cx,
        cy,
        zb,
        w / 2.0,
        l / 2.0,
        h,
        z_dir,
        cos_r,
        sin_r,
        PIN_HEADER_BODY,
        0.0,
        &fp.reference,
    )]
}

/// Fallback: box from pad extents.
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
        IC_COLOR,
        0.0,
        &fp.reference,
    )]
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
    color: [f32; 3],
    metalness: f32,
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
        0.0, // actually we need correct normals per face
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
        color,
        metalness,
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
        assert!(!meshes.is_empty());
        // Chip model generates 3 parts: body + 2 end caps
        assert_eq!(meshes.len(), 3);
        for m in &meshes {
            assert_eq!(m.footprint_ref, "R1");
            assert!(!m.positions.is_empty());
            assert!(!m.indices.is_empty());
        }
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
