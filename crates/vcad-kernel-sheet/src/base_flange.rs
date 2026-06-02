//! [`SheetMetalModel`] constructors — the "base flange" operations.
//!
//! Foundation tier supports a rectangular base flange and a generic
//! polygon outline. Sketch-driven flanges (via `vcad-kernel-sketch`) come
//! later; we don't drag the sketch crate into the foundation.

use crate::model::{Frame, Panel, SheetMetalModel};
use vcad_kernel_math::Point2;

/// Errors returned by base-flange construction.
#[derive(Debug, Clone, PartialEq)]
pub enum BaseFlangeError {
    /// Thickness must be > 0.
    InvalidThickness(f64),
    /// Outline must have at least 3 distinct points.
    OutlineTooSmall(usize),
    /// Width / depth must be > 0.
    NonPositiveDimension(&'static str, f64),
}

impl std::fmt::Display for BaseFlangeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BaseFlangeError::InvalidThickness(t) => write!(f, "thickness must be > 0, got {t}"),
            BaseFlangeError::OutlineTooSmall(n) => {
                write!(f, "outline needs >= 3 points, got {n}")
            }
            BaseFlangeError::NonPositiveDimension(name, v) => {
                write!(f, "{name} must be > 0, got {v}")
            }
        }
    }
}

impl std::error::Error for BaseFlangeError {}

/// Build a sheet-metal model from a closed polygon outline.
///
/// The outline lies in the XY plane (CCW); the panel's outside face is on
/// +Z and inside face on -Z. No holes — use
/// [`base_flange_polygon_with_holes`] when you need pierces.
pub fn base_flange_polygon(
    outline: Vec<Point2>,
    thickness: f64,
) -> Result<SheetMetalModel, BaseFlangeError> {
    base_flange_polygon_with_holes(outline, Vec::new(), thickness)
}

/// Build a sheet-metal model from an outline plus interior hole loops.
///
/// Hole loops must be CW (opposite of the outline) to match the
/// half-edge convention used downstream by unfold / DXF / cost. The
/// kernel doesn't enforce that here — pass them in correctly.
pub fn base_flange_polygon_with_holes(
    outline: Vec<Point2>,
    holes: Vec<Vec<Point2>>,
    thickness: f64,
) -> Result<SheetMetalModel, BaseFlangeError> {
    if thickness <= 0.0 || thickness.is_nan() {
        return Err(BaseFlangeError::InvalidThickness(thickness));
    }
    if outline.len() < 3 {
        return Err(BaseFlangeError::OutlineTooSmall(outline.len()));
    }
    for hole in &holes {
        if hole.len() < 3 {
            return Err(BaseFlangeError::OutlineTooSmall(hole.len()));
        }
    }
    let mut model = SheetMetalModel::new(thickness);
    let panel = Panel {
        outline,
        holes,
        frame_bent: Frame::identity(),
        frame_flat: Frame::identity(),
        incident_bends: Vec::new(),
    };
    model.root = model.push_panel(panel);
    Ok(model)
}

/// Build a sheet-metal model from an axis-aligned rectangle in the XY plane.
///
/// Corner at origin, extending into +X and +Y by `(width, depth)`. The panel's
/// outside face points along +Z.
pub fn base_flange_rect(
    width: f64,
    depth: f64,
    thickness: f64,
) -> Result<SheetMetalModel, BaseFlangeError> {
    if width <= 0.0 || width.is_nan() {
        return Err(BaseFlangeError::NonPositiveDimension("width", width));
    }
    if depth <= 0.0 || depth.is_nan() {
        return Err(BaseFlangeError::NonPositiveDimension("depth", depth));
    }
    base_flange_polygon(
        vec![
            Point2::new(0.0, 0.0),
            Point2::new(width, 0.0),
            Point2::new(width, depth),
            Point2::new(0.0, depth),
        ],
        thickness,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_creates_single_panel() {
        let m = base_flange_rect(100.0, 50.0, 1.0).unwrap();
        assert_eq!(m.panels.len(), 1);
        assert_eq!(m.bends.len(), 0);
        assert_eq!(m.root, 0);
        assert_eq!(m.thickness, 1.0);
        assert_eq!(m.panels[0].outline.len(), 4);
    }

    #[test]
    fn rect_rejects_zero_thickness() {
        assert!(matches!(
            base_flange_rect(100.0, 50.0, 0.0),
            Err(BaseFlangeError::InvalidThickness(_))
        ));
    }

    #[test]
    fn rect_rejects_negative_dim() {
        assert!(matches!(
            base_flange_rect(-1.0, 50.0, 1.0),
            Err(BaseFlangeError::NonPositiveDimension("width", _))
        ));
        assert!(matches!(
            base_flange_rect(100.0, -1.0, 1.0),
            Err(BaseFlangeError::NonPositiveDimension("depth", _))
        ));
    }

    #[test]
    fn polygon_rejects_degenerate_outline() {
        assert!(matches!(
            base_flange_polygon(vec![Point2::new(0.0, 0.0), Point2::new(1.0, 0.0)], 1.0),
            Err(BaseFlangeError::OutlineTooSmall(2))
        ));
    }

    #[test]
    fn polygon_with_holes_keeps_them() {
        let outline = vec![
            Point2::new(0.0, 0.0),
            Point2::new(20.0, 0.0),
            Point2::new(20.0, 10.0),
            Point2::new(0.0, 10.0),
        ];
        // CW hole inside the panel.
        let hole = vec![
            Point2::new(5.0, 3.0),
            Point2::new(5.0, 7.0),
            Point2::new(8.0, 7.0),
            Point2::new(8.0, 3.0),
        ];
        let m = base_flange_polygon_with_holes(outline, vec![hole], 1.0).unwrap();
        assert_eq!(m.panels[0].holes.len(), 1);
        assert_eq!(m.panels[0].holes[0].len(), 4);
    }

    #[test]
    fn polygon_supports_l_shape() {
        // L-bracket outline.
        let outline = vec![
            Point2::new(0.0, 0.0),
            Point2::new(40.0, 0.0),
            Point2::new(40.0, 10.0),
            Point2::new(10.0, 10.0),
            Point2::new(10.0, 40.0),
            Point2::new(0.0, 40.0),
        ];
        let m = base_flange_polygon(outline, 1.0).unwrap();
        assert_eq!(m.panels[0].outline.len(), 6);
    }

    #[test]
    fn rect_outline_is_ccw() {
        // Signed area of a CCW polygon is positive.
        let m = base_flange_rect(10.0, 5.0, 1.0).unwrap();
        let outline = &m.panels[0].outline;
        let area: f64 = outline
            .windows(2)
            .map(|w| w[0].x * w[1].y - w[1].x * w[0].y)
            .sum::<f64>()
            + (outline.last().unwrap().x * outline.first().unwrap().y
                - outline.first().unwrap().x * outline.last().unwrap().y);
        assert!(area > 0.0, "outline not CCW: signed area {area}");
    }
}
