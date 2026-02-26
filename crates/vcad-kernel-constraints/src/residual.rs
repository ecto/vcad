//! Residual (error) function computation for constraints.
//!
//! Each constraint contributes one or more residual values that should be
//! zero when the constraint is satisfied. The solver minimizes the sum of
//! squared residuals.
//!
//! Note: the solver now uses symbolic residuals via [`crate::symbolic::CompiledSystem`].
//! These functions are retained as a reference implementation for tests.

#![allow(dead_code)]

use crate::constraint::{Constraint, EntityRef};
use crate::entity::{EntityId, SketchEntity};
use slotmap::SlotMap;

/// Compute residuals for a constraint.
///
/// Returns the error values for the constraint given the current parameter
/// values and entity definitions.
pub fn compute_constraint_residuals(
    constraint: &Constraint,
    params: &[f64],
    entities: &SlotMap<EntityId, SketchEntity>,
) -> Vec<f64> {
    match constraint {
        Constraint::Coincident { point_a, point_b } => {
            let (ax, ay) = get_point_coords(*point_a, params, entities);
            let (bx, by) = get_point_coords(*point_b, params, entities);
            vec![ax - bx, ay - by]
        }

        Constraint::PointOnLine { point, line } => {
            let (px, py) = get_point_coords(*point, params, entities);
            let (sx, sy, ex, ey) = get_line_coords(*line, params, entities);
            // Signed distance from point to line
            let dx = ex - sx;
            let dy = ey - sy;
            let len = (dx * dx + dy * dy).sqrt();
            if len < 1e-15 {
                return vec![0.0];
            }
            // Cross product gives signed area, divide by length for distance
            let dist = ((px - sx) * dy - (py - sy) * dx) / len;
            vec![dist]
        }

        Constraint::Parallel { line_a, line_b } => {
            let (s1x, s1y, e1x, e1y) = get_line_coords(*line_a, params, entities);
            let (s2x, s2y, e2x, e2y) = get_line_coords(*line_b, params, entities);
            let d1x = e1x - s1x;
            let d1y = e1y - s1y;
            let d2x = e2x - s2x;
            let d2y = e2y - s2y;
            let len1 = (d1x * d1x + d1y * d1y).sqrt();
            let len2 = (d2x * d2x + d2y * d2y).sqrt();
            if len1 < 1e-15 || len2 < 1e-15 {
                return vec![0.0];
            }
            // Normalized cross product (sin of angle)
            let cross = (d1x * d2y - d1y * d2x) / (len1 * len2);
            vec![cross]
        }

        Constraint::Perpendicular { line_a, line_b } => {
            let (s1x, s1y, e1x, e1y) = get_line_coords(*line_a, params, entities);
            let (s2x, s2y, e2x, e2y) = get_line_coords(*line_b, params, entities);
            let d1x = e1x - s1x;
            let d1y = e1y - s1y;
            let d2x = e2x - s2x;
            let d2y = e2y - s2y;
            let len1 = (d1x * d1x + d1y * d1y).sqrt();
            let len2 = (d2x * d2x + d2y * d2y).sqrt();
            if len1 < 1e-15 || len2 < 1e-15 {
                return vec![0.0];
            }
            // Normalized dot product (cos of angle)
            let dot = (d1x * d2x + d1y * d2y) / (len1 * len2);
            vec![dot]
        }

        Constraint::Horizontal { line } => {
            let (sx, sy, ex, ey) = get_line_coords(*line, params, entities);
            let _ = (sx, ex); // unused, just need y coords
            vec![ey - sy]
        }

        Constraint::Vertical { line } => {
            let (sx, sy, ex, ey) = get_line_coords(*line, params, entities);
            let _ = (sy, ey); // unused, just need x coords
            vec![ex - sx]
        }

        Constraint::Tangent {
            line,
            curve,
            at_point,
        } => {
            let (sx, sy, ex, ey) = get_line_coords(*line, params, entities);
            let (cx, cy) = get_circle_center(*curve, params, entities);
            let (px, py) = get_point_coords(*at_point, params, entities);

            // Line direction
            let ldx = ex - sx;
            let ldy = ey - sy;

            // Radius vector at tangent point
            let rdx = px - cx;
            let rdy = py - cy;

            let line_len = (ldx * ldx + ldy * ldy).sqrt();
            let rad_len = (rdx * rdx + rdy * rdy).sqrt();
            if line_len < 1e-15 || rad_len < 1e-15 {
                return vec![0.0];
            }

            // Tangent when line direction is perpendicular to radius
            let dot = (ldx * rdx + ldy * rdy) / (line_len * rad_len);
            vec![dot]
        }

        Constraint::EqualLength { line_a, line_b } => {
            let (s1x, s1y, e1x, e1y) = get_line_coords(*line_a, params, entities);
            let (s2x, s2y, e2x, e2y) = get_line_coords(*line_b, params, entities);
            let len1 = ((e1x - s1x).powi(2) + (e1y - s1y).powi(2)).sqrt();
            let len2 = ((e2x - s2x).powi(2) + (e2y - s2y).powi(2)).sqrt();
            vec![len1 - len2]
        }

        Constraint::EqualRadius { circle_a, circle_b } => {
            let r1 = get_radius(*circle_a, params, entities);
            let r2 = get_radius(*circle_b, params, entities);
            vec![r1 - r2]
        }

        Constraint::Concentric { circle_a, circle_b } => {
            let (c1x, c1y) = get_circle_center(*circle_a, params, entities);
            let (c2x, c2y) = get_circle_center(*circle_b, params, entities);
            vec![c1x - c2x, c1y - c2y]
        }

        Constraint::Fixed { point, x, y } => {
            let (px, py) = get_point_coords(*point, params, entities);
            vec![px - x, py - y]
        }

        Constraint::PointOnCircle { point, circle } => {
            let (px, py) = get_point_coords(*point, params, entities);
            let (cx, cy) = get_circle_center(*circle, params, entities);
            let radius = get_radius(*circle, params, entities);
            let dist = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
            vec![dist - radius]
        }

        Constraint::LineThroughCenter { line, circle } => {
            let (sx, sy, ex, ey) = get_line_coords(*line, params, entities);
            let (cx, cy) = get_circle_center(*circle, params, entities);
            // Distance from center to line
            let dx = ex - sx;
            let dy = ey - sy;
            let len = (dx * dx + dy * dy).sqrt();
            if len < 1e-15 {
                return vec![0.0];
            }
            let dist = ((cx - sx) * dy - (cy - sy) * dx) / len;
            vec![dist]
        }

        Constraint::Midpoint { point, line } => {
            let (px, py) = get_point_coords(*point, params, entities);
            let (sx, sy, ex, ey) = get_line_coords(*line, params, entities);
            let mx = (sx + ex) / 2.0;
            let my = (sy + ey) / 2.0;
            vec![px - mx, py - my]
        }

        Constraint::Symmetric {
            point_a,
            point_b,
            axis,
        } => {
            let (ax, ay) = get_point_coords(*point_a, params, entities);
            let (bx, by) = get_point_coords(*point_b, params, entities);
            let (sx, sy, ex, ey) = get_line_coords(*axis, params, entities);

            // Midpoint of A and B should be on the axis
            let mx = (ax + bx) / 2.0;
            let my = (ay + by) / 2.0;
            let dx = ex - sx;
            let dy = ey - sy;
            let len = (dx * dx + dy * dy).sqrt();
            if len < 1e-15 {
                return vec![0.0, 0.0];
            }
            let dist_to_axis = ((mx - sx) * dy - (my - sy) * dx) / len;

            // AB should be perpendicular to axis
            let abx = bx - ax;
            let aby = by - ay;
            let ab_len = (abx * abx + aby * aby).sqrt();
            if ab_len < 1e-15 {
                return vec![dist_to_axis, 0.0];
            }
            let perp = (abx * dx + aby * dy) / (ab_len * len);

            vec![dist_to_axis, perp]
        }

        Constraint::Distance {
            point_a,
            point_b,
            distance,
        } => {
            let (ax, ay) = get_point_coords(*point_a, params, entities);
            let (bx, by) = get_point_coords(*point_b, params, entities);
            let actual_dist = ((bx - ax).powi(2) + (by - ay).powi(2)).sqrt();
            vec![actual_dist - distance]
        }

        Constraint::PointLineDistance {
            point,
            line,
            distance,
        } => {
            let (px, py) = get_point_coords(*point, params, entities);
            let (sx, sy, ex, ey) = get_line_coords(*line, params, entities);
            let dx = ex - sx;
            let dy = ey - sy;
            let len = (dx * dx + dy * dy).sqrt();
            if len < 1e-15 {
                return vec![0.0];
            }
            let signed_dist = ((px - sx) * dy - (py - sy) * dx) / len;
            vec![signed_dist.abs() - distance]
        }

        Constraint::Angle {
            line_a,
            line_b,
            angle_rad,
        } => {
            let (s1x, s1y, e1x, e1y) = get_line_coords(*line_a, params, entities);
            let (s2x, s2y, e2x, e2y) = get_line_coords(*line_b, params, entities);
            let d1x = e1x - s1x;
            let d1y = e1y - s1y;
            let d2x = e2x - s2x;
            let d2y = e2y - s2y;
            let len1 = (d1x * d1x + d1y * d1y).sqrt();
            let len2 = (d2x * d2x + d2y * d2y).sqrt();
            if len1 < 1e-15 || len2 < 1e-15 {
                return vec![0.0];
            }
            let cos_angle = (d1x * d2x + d1y * d2y) / (len1 * len2);
            let sin_angle = (d1x * d2y - d1y * d2x) / (len1 * len2);
            let actual_angle = sin_angle.atan2(cos_angle);
            // Normalize angle difference to [-π, π]
            let mut diff = actual_angle - angle_rad;
            while diff > std::f64::consts::PI {
                diff -= 2.0 * std::f64::consts::PI;
            }
            while diff < -std::f64::consts::PI {
                diff += 2.0 * std::f64::consts::PI;
            }
            vec![diff]
        }

        Constraint::Radius { circle, radius } => {
            let actual_r = get_radius(*circle, params, entities);
            vec![actual_r - radius]
        }

        Constraint::Length { line, length } => {
            let (sx, sy, ex, ey) = get_line_coords(*line, params, entities);
            let actual_len = ((ex - sx).powi(2) + (ey - sy).powi(2)).sqrt();
            vec![actual_len - length]
        }

        Constraint::HorizontalDistance { point, x } => {
            let (px, _py) = get_point_coords(*point, params, entities);
            vec![px - x]
        }

        Constraint::VerticalDistance { point, y } => {
            let (_px, py) = get_point_coords(*point, params, entities);
            vec![py - y]
        }

        Constraint::Diameter { circle, diameter } => {
            let radius = get_radius(*circle, params, entities);
            vec![2.0 * radius - diameter]
        }
    }
}

/// Get (x, y) coordinates for a point reference.
fn get_point_coords(
    point_ref: EntityRef,
    params: &[f64],
    entities: &SlotMap<EntityId, SketchEntity>,
) -> (f64, f64) {
    match point_ref {
        EntityRef::Point(id) => {
            if let Some(SketchEntity::Point(p)) = entities.get(id) {
                (params[p.param_x], params[p.param_y])
            } else {
                (0.0, 0.0)
            }
        }
        EntityRef::LineStart(id) => {
            if let Some(SketchEntity::Line(l)) = entities.get(id) {
                get_point_coords(EntityRef::Point(l.start), params, entities)
            } else {
                (0.0, 0.0)
            }
        }
        EntityRef::LineEnd(id) => {
            if let Some(SketchEntity::Line(l)) = entities.get(id) {
                get_point_coords(EntityRef::Point(l.end), params, entities)
            } else {
                (0.0, 0.0)
            }
        }
        EntityRef::Center(id) => get_circle_center(id, params, entities),
        EntityRef::ArcStart(id) => {
            if let Some(SketchEntity::Arc(a)) = entities.get(id) {
                get_point_coords(EntityRef::Point(a.start), params, entities)
            } else {
                (0.0, 0.0)
            }
        }
        EntityRef::ArcEnd(id) => {
            if let Some(SketchEntity::Arc(a)) = entities.get(id) {
                get_point_coords(EntityRef::Point(a.end), params, entities)
            } else {
                (0.0, 0.0)
            }
        }
    }
}

/// Get (start_x, start_y, end_x, end_y) for a line entity.
fn get_line_coords(
    line_id: EntityId,
    params: &[f64],
    entities: &SlotMap<EntityId, SketchEntity>,
) -> (f64, f64, f64, f64) {
    if let Some(SketchEntity::Line(l)) = entities.get(line_id) {
        let (sx, sy) = get_point_coords(EntityRef::Point(l.start), params, entities);
        let (ex, ey) = get_point_coords(EntityRef::Point(l.end), params, entities);
        (sx, sy, ex, ey)
    } else {
        (0.0, 0.0, 0.0, 0.0)
    }
}

/// Get (center_x, center_y) for a circle or arc entity.
fn get_circle_center(
    id: EntityId,
    params: &[f64],
    entities: &SlotMap<EntityId, SketchEntity>,
) -> (f64, f64) {
    match entities.get(id) {
        Some(SketchEntity::Circle(c)) => {
            get_point_coords(EntityRef::Point(c.center), params, entities)
        }
        Some(SketchEntity::Arc(a)) => {
            get_point_coords(EntityRef::Point(a.center), params, entities)
        }
        _ => (0.0, 0.0),
    }
}

/// Get radius for a circle entity.
fn get_radius(id: EntityId, params: &[f64], entities: &SlotMap<EntityId, SketchEntity>) -> f64 {
    match entities.get(id) {
        Some(SketchEntity::Circle(c)) => params[c.param_radius],
        Some(SketchEntity::Arc(a)) => {
            // For arcs, compute radius from center to start point
            let (cx, cy) = get_point_coords(EntityRef::Point(a.center), params, entities);
            let (sx, sy) = get_point_coords(EntityRef::Point(a.start), params, entities);
            ((sx - cx).powi(2) + (sy - cy).powi(2)).sqrt()
        }
        _ => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::{SketchLine, SketchPoint};

    fn setup_two_points() -> (SlotMap<EntityId, SketchEntity>, Vec<f64>) {
        let mut entities = SlotMap::with_key();
        let p1 = entities.insert(SketchEntity::Point(SketchPoint {
            param_x: 0,
            param_y: 1,
        }));
        let p2 = entities.insert(SketchEntity::Point(SketchPoint {
            param_x: 2,
            param_y: 3,
        }));
        // p1 at (0, 0), p2 at (10, 0)
        let params = vec![0.0, 0.0, 10.0, 0.0];
        let _ = (p1, p2); // These are used implicitly via EntityRef::Point
        (entities, params)
    }

    #[test]
    fn test_coincident_residual() {
        let (entities, params) = setup_two_points();
        let ids: Vec<_> = entities.keys().collect();
        let constraint = Constraint::Coincident {
            point_a: EntityRef::Point(ids[0]),
            point_b: EntityRef::Point(ids[1]),
        };
        let res = compute_constraint_residuals(&constraint, &params, &entities);
        assert_eq!(res.len(), 2);
        assert!((res[0] - (-10.0)).abs() < 1e-12); // 0 - 10
        assert!((res[1] - 0.0).abs() < 1e-12); // 0 - 0
    }

    #[test]
    fn test_horizontal_residual() {
        let mut entities = SlotMap::with_key();
        let p1 = entities.insert(SketchEntity::Point(SketchPoint {
            param_x: 0,
            param_y: 1,
        }));
        let p2 = entities.insert(SketchEntity::Point(SketchPoint {
            param_x: 2,
            param_y: 3,
        }));
        let line = entities.insert(SketchEntity::Line(SketchLine { start: p1, end: p2 }));
        // p1 at (0, 0), p2 at (10, 5) - diagonal line
        let params = vec![0.0, 0.0, 10.0, 5.0];

        let constraint = Constraint::Horizontal { line };
        let res = compute_constraint_residuals(&constraint, &params, &entities);
        assert_eq!(res.len(), 1);
        assert!((res[0] - 5.0).abs() < 1e-12); // end.y - start.y
    }

    #[test]
    fn test_distance_residual() {
        let (entities, params) = setup_two_points();
        let ids: Vec<_> = entities.keys().collect();
        let constraint = Constraint::Distance {
            point_a: EntityRef::Point(ids[0]),
            point_b: EntityRef::Point(ids[1]),
            distance: 8.0,
        };
        let res = compute_constraint_residuals(&constraint, &params, &entities);
        assert_eq!(res.len(), 1);
        // Actual distance is 10, target is 8
        assert!((res[0] - 2.0).abs() < 1e-12);
    }

    #[test]
    fn test_fixed_residual() {
        let (entities, params) = setup_two_points();
        let ids: Vec<_> = entities.keys().collect();
        let constraint = Constraint::Fixed {
            point: EntityRef::Point(ids[0]),
            x: 1.0,
            y: 2.0,
        };
        let res = compute_constraint_residuals(&constraint, &params, &entities);
        assert_eq!(res.len(), 2);
        // Point at (0, 0), fixed at (1, 2)
        assert!((res[0] - (-1.0)).abs() < 1e-12);
        assert!((res[1] - (-2.0)).abs() < 1e-12);
    }
}
