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

/// Absolute board-frame center of a pad: the footprint position plus the pad's
/// local offset rotated by the footprint rotation.
///
/// This is the single world-position convention shared by the ratsnest, the
/// routers, and every DRC/DFM geometric check. Skipping the rotation places a
/// rotated footprint's pads at phantom locations that nothing else on the
/// board (routed or hand-laid copper, `get_pad_positions` callers) can ever
/// touch — the root cause of connectivity never crediting manual copper.
pub fn pad_world_center(fp: &vcad_ir::ecad::Footprint, pad: &vcad_ir::ecad::Pad) -> Vec2 {
    let (sin_r, cos_r) = fp.rotation.to_radians().sin_cos();
    Vec2::new(
        fp.position.x + pad.position.x * cos_r - pad.position.y * sin_r,
        fp.position.y + pad.position.x * sin_r + pad.position.y * cos_r,
    )
}

/// Compute bounding box of a footprint's pads: `(min, max)`.
pub fn footprint_bounds(fp: &vcad_ir::ecad::Footprint) -> (Vec2, Vec2) {
    let mut min_x = f64::MAX;
    let mut min_y = f64::MAX;
    let mut max_x = f64::MIN;
    let mut max_y = f64::MIN;

    for pad in &fp.pads {
        let Vec2 { x: wx, y: wy } = pad_world_center(fp, pad);
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
