//! Detail view generation for magnified regions of projected views.
//!
//! Detail views allow zooming in on a specific area of a technical drawing,
//! showing fine features that would be too small in the main view.

use crate::dimension::{
    ArrowType, DimensionStyle, RenderedArc, RenderedArrow, RenderedDimension, RenderedText,
    TextAlignment,
};
use crate::types::{
    BoundingBox2D, DetailView, DetailViewParams, Point2D, ProjectedEdge, ProjectedView,
};

/// Format a magnification factor as a drawing scale ratio.
///
/// `2.0` → `"2:1"`, `0.5` → `"1:2"`, `1.0` → `"1:1"`. Non-integer ratios keep
/// one decimal place (`2.5` → `"2.5:1"`).
pub fn format_scale(scale: f64) -> String {
    let fmt = |v: f64| -> String {
        if (v - v.round()).abs() < 1e-9 {
            format!("{}", v.round() as i64)
        } else {
            format!("{v:.1}")
        }
    };
    if scale >= 1.0 {
        format!("{}:1", fmt(scale))
    } else if scale > 0.0 {
        format!("1:{}", fmt(1.0 / scale))
    } else {
        "1:1".to_string()
    }
}

/// Render the circular detail bubble on the **parent** view: a circle around
/// the captured region with the detail letter and a short leader.
///
/// Consumers draw the returned primitives on the parent view; the circle uses
/// the region's circumscribed radius so the whole capture rect is enclosed.
pub fn detail_callout(params: &DetailViewParams, style: &DimensionStyle) -> RenderedDimension {
    let mut rd = RenderedDimension::new();
    let radius = params.half_width().hypot(params.half_height());

    rd.add_arc(RenderedArc::new(
        params.center,
        radius,
        0.0,
        std::f64::consts::TAU,
    ));

    // Leader from the circle at 45° up-right, ending at the letter label.
    let dir = std::f64::consts::FRAC_PI_4;
    let start = Point2D::new(
        params.center.x + radius * dir.cos(),
        params.center.y + radius * dir.sin(),
    );
    let leader_len = style.arrow_size * 3.0;
    let end = Point2D::new(
        start.x + leader_len * dir.cos(),
        start.y + leader_len * dir.sin(),
    );
    rd.add_line(start, end);
    rd.add_arrow(RenderedArrow::new(
        start,
        dir + std::f64::consts::PI,
        ArrowType::Open,
        style.arrow_size,
    ));
    rd.add_text(
        RenderedText::new(
            Point2D::new(
                end.x + style.text_height * 0.9,
                end.y + style.text_height * 0.9,
            ),
            params.label.clone(),
            style.text_height * 1.4,
        )
        .with_alignment(TextAlignment::MiddleCenter),
    );

    rd
}

/// Render the caption under a detail view: `DETAIL A (SCALE 2:1)`.
///
/// `position` is the caption anchor (typically centered below the detail
/// view's bounds).
pub fn detail_caption(
    view: &DetailView,
    position: Point2D,
    style: &DimensionStyle,
) -> RenderedDimension {
    let mut rd = RenderedDimension::new();
    rd.add_text(
        RenderedText::new(position, view.caption(), style.text_height * 1.2)
            .with_alignment(TextAlignment::TopCenter),
    );
    rd
}

impl DetailView {
    /// The caption text for this view, e.g. `DETAIL A (SCALE 2:1)`.
    pub fn caption(&self) -> String {
        format!(
            "DETAIL {} (SCALE {})",
            self.params.label,
            format_scale(self.params.scale)
        )
    }
}

/// Create a detail view by clipping and magnifying a region of the parent view.
///
/// # Arguments
///
/// * `parent` - The projected view to create a detail from
/// * `params` - Parameters defining the region and magnification
///
/// # Returns
///
/// A `DetailView` containing the clipped and scaled edges.
pub fn create_detail_view(parent: &ProjectedView, params: &DetailViewParams) -> DetailView {
    let min_x = params.min_x();
    let max_x = params.max_x();
    let min_y = params.min_y();
    let max_y = params.max_y();

    let mut edges = Vec::new();
    let mut bounds = BoundingBox2D::empty();

    for edge in &parent.edges {
        // Clip the edge to the region using Cohen-Sutherland algorithm
        if let Some(clipped) = clip_edge_to_rect(edge, min_x, max_x, min_y, max_y) {
            // Transform to detail view coordinates (center at origin, then scale)
            let transformed = transform_edge(&clipped, params);
            bounds.include_point(transformed.start);
            bounds.include_point(transformed.end);
            edges.push(transformed);
        }
    }

    DetailView::new(edges, bounds, params.clone())
}

/// Cohen-Sutherland outcodes for line clipping.
const INSIDE: u8 = 0;
const LEFT: u8 = 1;
const RIGHT: u8 = 2;
const BOTTOM: u8 = 4;
const TOP: u8 = 8;

/// Compute the outcode for a point relative to a rectangle.
fn compute_outcode(x: f64, y: f64, min_x: f64, max_x: f64, min_y: f64, max_y: f64) -> u8 {
    let mut code = INSIDE;
    if x < min_x {
        code |= LEFT;
    } else if x > max_x {
        code |= RIGHT;
    }
    if y < min_y {
        code |= BOTTOM;
    } else if y > max_y {
        code |= TOP;
    }
    code
}

/// Clip an edge to a rectangle using Cohen-Sutherland algorithm.
///
/// Returns `None` if the edge is entirely outside the rectangle.
fn clip_edge_to_rect(
    edge: &ProjectedEdge,
    min_x: f64,
    max_x: f64,
    min_y: f64,
    max_y: f64,
) -> Option<ProjectedEdge> {
    let mut x0 = edge.start.x;
    let mut y0 = edge.start.y;
    let mut x1 = edge.end.x;
    let mut y1 = edge.end.y;

    let mut outcode0 = compute_outcode(x0, y0, min_x, max_x, min_y, max_y);
    let mut outcode1 = compute_outcode(x1, y1, min_x, max_x, min_y, max_y);

    loop {
        if (outcode0 | outcode1) == 0 {
            // Both points inside - accept
            return Some(ProjectedEdge::new(
                Point2D::new(x0, y0),
                Point2D::new(x1, y1),
                edge.visibility,
                edge.edge_type,
                edge.depth,
            ));
        }

        if (outcode0 & outcode1) != 0 {
            // Both points on same outside region - reject
            return None;
        }

        // At least one point is outside - clip it
        let outcode_out = if outcode0 != 0 { outcode0 } else { outcode1 };

        let (x, y) = if (outcode_out & TOP) != 0 {
            // Point is above the clip rectangle
            let x = x0 + (x1 - x0) * (max_y - y0) / (y1 - y0);
            (x, max_y)
        } else if (outcode_out & BOTTOM) != 0 {
            // Point is below the clip rectangle
            let x = x0 + (x1 - x0) * (min_y - y0) / (y1 - y0);
            (x, min_y)
        } else if (outcode_out & RIGHT) != 0 {
            // Point is to the right of the clip rectangle
            let y = y0 + (y1 - y0) * (max_x - x0) / (x1 - x0);
            (max_x, y)
        } else {
            // Point is to the left of the clip rectangle
            let y = y0 + (y1 - y0) * (min_x - x0) / (x1 - x0);
            (min_x, y)
        };

        // Update the point that was outside
        if outcode_out == outcode0 {
            x0 = x;
            y0 = y;
            outcode0 = compute_outcode(x0, y0, min_x, max_x, min_y, max_y);
        } else {
            x1 = x;
            y1 = y;
            outcode1 = compute_outcode(x1, y1, min_x, max_x, min_y, max_y);
        }
    }
}

/// Transform an edge from parent view coordinates to detail view coordinates.
///
/// The transformation:
/// 1. Translate so the region center is at the origin
/// 2. Scale by the magnification factor
fn transform_edge(edge: &ProjectedEdge, params: &DetailViewParams) -> ProjectedEdge {
    let transform_point = |p: Point2D| -> Point2D {
        Point2D::new(
            (p.x - params.center.x) * params.scale,
            (p.y - params.center.y) * params.scale,
        )
    };

    ProjectedEdge::new(
        transform_point(edge.start),
        transform_point(edge.end),
        edge.visibility,
        edge.edge_type,
        edge.depth,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{EdgeType, ViewDirection, Visibility};

    fn make_test_view() -> ProjectedView {
        let mut view = ProjectedView::new(ViewDirection::Front);
        // Create a square from (0,0) to (100,100)
        view.add_edge(ProjectedEdge::new(
            Point2D::new(0.0, 0.0),
            Point2D::new(100.0, 0.0),
            Visibility::Visible,
            EdgeType::Sharp,
            0.0,
        ));
        view.add_edge(ProjectedEdge::new(
            Point2D::new(100.0, 0.0),
            Point2D::new(100.0, 100.0),
            Visibility::Visible,
            EdgeType::Sharp,
            0.0,
        ));
        view.add_edge(ProjectedEdge::new(
            Point2D::new(100.0, 100.0),
            Point2D::new(0.0, 100.0),
            Visibility::Visible,
            EdgeType::Sharp,
            0.0,
        ));
        view.add_edge(ProjectedEdge::new(
            Point2D::new(0.0, 100.0),
            Point2D::new(0.0, 0.0),
            Visibility::Visible,
            EdgeType::Sharp,
            0.0,
        ));
        // Add a diagonal
        view.add_edge(ProjectedEdge::new(
            Point2D::new(0.0, 0.0),
            Point2D::new(100.0, 100.0),
            Visibility::Hidden,
            EdgeType::Sharp,
            0.0,
        ));
        view
    }

    #[test]
    fn test_format_scale() {
        assert_eq!(format_scale(2.0), "2:1");
        assert_eq!(format_scale(0.5), "1:2");
        assert_eq!(format_scale(1.0), "1:1");
        assert_eq!(format_scale(2.5), "2.5:1");
    }

    #[test]
    fn test_detail_callout_and_caption() {
        let params = DetailViewParams::new(Point2D::new(10.0, 10.0), 2.0, 6.0, 8.0, "B");
        let style = DimensionStyle::default();
        let callout = detail_callout(&params, &style);
        assert_eq!(callout.arcs.len(), 1, "detail bubble circle");
        assert!((callout.arcs[0].radius - 5.0).abs() < 1e-9, "hypot(3,4)");
        assert_eq!(callout.arrows.len(), 1);
        assert!(callout.texts.iter().any(|t| t.text == "B"));

        let view = create_detail_view(&make_test_view(), &params);
        assert_eq!(view.caption(), "DETAIL B (SCALE 2:1)");
        let caption = detail_caption(&view, Point2D::ORIGIN, &style);
        assert_eq!(caption.texts.len(), 1);
        assert_eq!(caption.texts[0].text, "DETAIL B (SCALE 2:1)");
    }

    #[test]
    fn test_detail_view_clips_edges() {
        let view = make_test_view();

        // Create a detail view of the bottom-left corner
        let params = DetailViewParams::new(
            Point2D::new(25.0, 25.0),
            2.0,  // 2x magnification
            50.0, // 50 units wide
            50.0, // 50 units tall
            "A",
        );

        let detail = create_detail_view(&view, &params);

        // Should have clipped edges
        assert!(!detail.edges.is_empty());
        assert_eq!(detail.params.label, "A");
    }

    #[test]
    fn test_clipping_inside_edge() {
        let edge = ProjectedEdge::new(
            Point2D::new(10.0, 10.0),
            Point2D::new(20.0, 20.0),
            Visibility::Visible,
            EdgeType::Sharp,
            0.0,
        );

        let clipped = clip_edge_to_rect(&edge, 0.0, 100.0, 0.0, 100.0);
        assert!(clipped.is_some());
        let clipped = clipped.unwrap();
        assert!((clipped.start.x - 10.0).abs() < 0.001);
        assert!((clipped.end.x - 20.0).abs() < 0.001);
    }

    #[test]
    fn test_clipping_outside_edge() {
        let edge = ProjectedEdge::new(
            Point2D::new(-10.0, -10.0),
            Point2D::new(-5.0, -5.0),
            Visibility::Visible,
            EdgeType::Sharp,
            0.0,
        );

        let clipped = clip_edge_to_rect(&edge, 0.0, 100.0, 0.0, 100.0);
        assert!(clipped.is_none());
    }

    #[test]
    fn test_clipping_crossing_edge() {
        let edge = ProjectedEdge::new(
            Point2D::new(-50.0, 50.0),
            Point2D::new(150.0, 50.0),
            Visibility::Visible,
            EdgeType::Sharp,
            0.0,
        );

        let clipped = clip_edge_to_rect(&edge, 0.0, 100.0, 0.0, 100.0);
        assert!(clipped.is_some());
        let clipped = clipped.unwrap();
        assert!((clipped.start.x - 0.0).abs() < 0.001);
        assert!((clipped.end.x - 100.0).abs() < 0.001);
    }

    #[test]
    fn test_transform_scales_edges() {
        let params = DetailViewParams::new(Point2D::new(50.0, 50.0), 2.0, 100.0, 100.0, "A");

        let edge = ProjectedEdge::new(
            Point2D::new(50.0, 50.0), // At center
            Point2D::new(60.0, 50.0), // 10 units to the right
            Visibility::Visible,
            EdgeType::Sharp,
            0.0,
        );

        let transformed = transform_edge(&edge, &params);

        // Center should be at origin after transform
        assert!((transformed.start.x - 0.0).abs() < 0.001);
        assert!((transformed.start.y - 0.0).abs() < 0.001);

        // 10 units to the right, scaled 2x = 20 units
        assert!((transformed.end.x - 20.0).abs() < 0.001);
    }
}
