//! PCB geometry utilities.
//!
//! Layer Z offsets, pad dimensions, coordinate conversion, and related helpers.

use vcad_ir::ecad::{PadShape, PcbLayer};
use vcad_ir::Vec2;

// ============================================================================
// Layer Z offsets
// ============================================================================

/// Base Z offset for each layer (normalized, not physical mm).
fn layer_z_base(layer: PcbLayer) -> f64 {
    match layer {
        PcbLayer::FCu => 0.8,
        PcbLayer::In1Cu => 0.6,
        PcbLayer::In2Cu => 0.4,
        PcbLayer::In3Cu => 0.2,
        PcbLayer::In4Cu => 0.0,
        PcbLayer::In5Cu => -0.2,
        PcbLayer::In6Cu => -0.4,
        PcbLayer::BCu => -0.8,
        PcbLayer::FSilkS => 0.9,
        PcbLayer::BSilkS => -0.9,
        PcbLayer::FMask => 0.85,
        PcbLayer::BMask => -0.85,
        PcbLayer::EdgeCuts => 0.0,
        PcbLayer::FCrtYd => 0.95,
        PcbLayer::BCrtYd => -0.95,
        PcbLayer::FFab => 0.92,
        PcbLayer::BFab => -0.92,
        _ => 0.0,
    }
}

/// Z offset for a layer in kernel Z-up space.
pub fn layer_z_offset(layer: PcbLayer, explosion: f64) -> f64 {
    let base = layer_z_base(layer);
    base * (1.0 + explosion * 4.0)
}

/// Z position in kernel space (board surface + layer offset).
pub fn layer_z(layer: PcbLayer, board_thickness: f64, explosion: f64) -> f64 {
    board_thickness / 2.0 + layer_z_offset(layer, explosion)
}

// ============================================================================
// Pad geometry
// ============================================================================

/// Pad dimensions `(width, height)` for scaling a unit geometry.
pub fn pad_dimensions(shape: &PadShape) -> (f64, f64) {
    match shape {
        PadShape::Circle { diameter } => (*diameter, *diameter),
        PadShape::Rect { width, height }
        | PadShape::Oval { width, height }
        | PadShape::RoundRect { width, height, .. } => (*width, *height),
        PadShape::Custom { .. } => (2.0, 2.0),
    }
}

/// Pad radius (half the max dimension) for hit testing.
pub fn pad_radius(shape: &PadShape) -> f64 {
    match shape {
        PadShape::Circle { diameter } => diameter / 2.0,
        PadShape::Rect { width, height }
        | PadShape::Oval { width, height }
        | PadShape::RoundRect { width, height, .. } => width.max(*height) / 2.0,
        PadShape::Custom { .. } => 1.0,
    }
}

/// Absolute board-frame center of a pad: the footprint origin plus the pad's
/// local offset rotated by the footprint rotation (degrees, CCW).
///
/// This is the canonical pad placement transform — the same one the Gerber /
/// Excellon writers and the TypeScript `get_pad_positions` MCP tool apply:
/// `world = fp + R(θ)·pad`. Every consumer that positions pad copper must go
/// through it; adding `fp.position + pad.position` without the rotation
/// misplaces every pad on a rotated footprint.
pub fn pad_world_position(fp: &vcad_ir::ecad::Footprint, pad: &vcad_ir::ecad::Pad) -> Vec2 {
    let (sin_r, cos_r) = fp.rotation.to_radians().sin_cos();
    Vec2::new(
        fp.position.x + pad.position.x * cos_r - pad.position.y * sin_r,
        fp.position.y + pad.position.x * sin_r + pad.position.y * cos_r,
    )
}

// ============================================================================
// Coordinate conversion
// ============================================================================

/// Convert a point in kernel space to PCB coordinates with optional grid snap.
pub fn world_to_pcb(x: f64, y: f64, grid_size: f64, snap: bool) -> Vec2 {
    if snap && grid_size > 0.0 {
        Vec2::new(
            (x / grid_size).round() * grid_size,
            (y / grid_size).round() * grid_size,
        )
    } else {
        Vec2::new(x, y)
    }
}

// ============================================================================
// Layer helpers
// ============================================================================

/// Copper layer ordering for Z offset computation.
const COPPER_LAYER_ORDER: [PcbLayer; 8] = [
    PcbLayer::FCu,
    PcbLayer::In1Cu,
    PcbLayer::In2Cu,
    PcbLayer::In3Cu,
    PcbLayer::In4Cu,
    PcbLayer::In5Cu,
    PcbLayer::In6Cu,
    PcbLayer::BCu,
];

/// Copper layer index for consistent ordering.
pub fn copper_layer_index(layer: PcbLayer) -> usize {
    COPPER_LAYER_ORDER
        .iter()
        .position(|&l| l == layer)
        .unwrap_or(99)
}

/// Compute bounding box of a footprint's pads: `(min, max)`.
pub fn footprint_bounds(fp: &vcad_ir::ecad::Footprint) -> (Vec2, Vec2) {
    let mut min_x = f64::MAX;
    let mut min_y = f64::MAX;
    let mut max_x = f64::MIN;
    let mut max_y = f64::MIN;

    for pad in &fp.pads {
        let Vec2 { x: wx, y: wy } = pad_world_position(fp, pad);
        let (pw, ph) = pad_dimensions(&pad.shape);
        let r = pw.max(ph) / 2.0;

        min_x = min_x.min(wx - r);
        min_y = min_y.min(wy - r);
        max_x = max_x.max(wx + r);
        max_y = max_y.max(wy + r);
    }

    if min_x > max_x {
        // No pads — use footprint position
        return (fp.position, fp.position);
    }

    (Vec2::new(min_x, min_y), Vec2::new(max_x, max_y))
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_ir::ecad::{Footprint, Pad, PadType};

    fn rotated_fixture() -> Footprint {
        Footprint {
            reference: "J1".to_string(),
            value: "CONN".to_string(),
            footprint_name: "JST_XH_4".to_string(),
            position: Vec2::new(10.0, 20.0),
            rotation: 190.0,
            front: true,
            pads: vec![
                Pad {
                    number: "1".to_string(),
                    pad_type: PadType::THT,
                    shape: PadShape::Circle { diameter: 1.7 },
                    position: Vec2::new(7.62, 0.0),
                    rotation: 0.0,
                    drill: None,
                    net: Some("1".to_string()),
                    layers: vec![PcbLayer::FCu],
                },
                Pad {
                    number: "2".to_string(),
                    pad_type: PadType::THT,
                    shape: PadShape::Circle { diameter: 1.7 },
                    position: Vec2::new(2.54, -1.27),
                    rotation: 0.0,
                    drill: None,
                    net: Some("2".to_string()),
                    layers: vec![PcbLayer::FCu],
                },
            ],
            graphics: vec![],
            model_3d: None,
            properties: std::collections::HashMap::new(),
        }
    }

    /// Cross-language regression: these constants are also asserted by the
    /// TypeScript `get_pad_positions` test ("agrees with the Rust
    /// pad_world_position transform") in
    /// `packages/mcp/src/__tests__/ecad.test.ts`. Both sides pin the same
    /// footprint fixture (origin (10, 20), rotation 190°) to the same world
    /// coordinates, so the Rust copper pipeline and the TS reporting tool
    /// cannot silently drift apart. If you change one, change both.
    #[test]
    fn pad_world_position_rotated_matches_ts_tool() {
        let fp = rotated_fixture();
        let a = pad_world_position(&fp, &fp.pads[0]);
        assert!((a.x - 2.495764922046975).abs() < 1e-9, "a.x = {}", a.x);
        assert!((a.y - 18.67680088617799).abs() < 1e-9, "a.y = {}", a.y);
        let b = pad_world_position(&fp, &fp.pads[1]);
        assert!((b.x - 7.27805512171199).abs() < 1e-9, "b.x = {}", b.x);
        assert!((b.y - 20.8096394750515).abs() < 1e-9, "b.y = {}", b.y);
    }

    #[test]
    fn pad_world_position_zero_rotation_is_plain_offset() {
        let mut fp = rotated_fixture();
        fp.rotation = 0.0;
        let a = pad_world_position(&fp, &fp.pads[0]);
        assert!((a.x - 17.62).abs() < 1e-12);
        assert!((a.y - 20.0).abs() < 1e-12);
    }

    #[test]
    fn layer_z_fcu() {
        let z = layer_z(PcbLayer::FCu, 1.6, 0.0);
        assert!((z - 1.6).abs() < 0.01); // thickness/2 + 0.8
    }

    #[test]
    fn layer_z_bcu() {
        let z = layer_z(PcbLayer::BCu, 1.6, 0.0);
        assert!((z - 0.0).abs() < 0.01); // thickness/2 + (-0.8) = 0.0
    }

    #[test]
    fn layer_z_explosion() {
        let normal = layer_z(PcbLayer::FCu, 1.6, 0.0);
        let exploded = layer_z(PcbLayer::FCu, 1.6, 1.0);
        assert!(exploded > normal);
    }

    #[test]
    fn pad_dims_circle() {
        let shape = PadShape::Circle { diameter: 2.0 };
        assert_eq!(pad_dimensions(&shape), (2.0, 2.0));
        assert!((pad_radius(&shape) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn pad_dims_rect() {
        let shape = PadShape::Rect {
            width: 1.0,
            height: 2.0,
        };
        assert_eq!(pad_dimensions(&shape), (1.0, 2.0));
        assert!((pad_radius(&shape) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn world_to_pcb_snap() {
        let p = world_to_pcb(1.3, 2.7, 1.0, true);
        assert!((p.x - 1.0).abs() < 1e-9);
        assert!((p.y - 3.0).abs() < 1e-9);
    }

    #[test]
    fn world_to_pcb_no_snap() {
        let p = world_to_pcb(1.3, 2.7, 1.0, false);
        assert!((p.x - 1.3).abs() < 1e-9);
        assert!((p.y - 2.7).abs() < 1e-9);
    }

    #[test]
    fn copper_layer_ordering() {
        assert_eq!(copper_layer_index(PcbLayer::FCu), 0);
        assert_eq!(copper_layer_index(PcbLayer::BCu), 7);
        assert_eq!(copper_layer_index(PcbLayer::FSilkS), 99);
    }
}
