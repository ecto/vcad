//! Isotropic ε(x, y) geometry: shapes painted with sub-pixel averaging.
//!
//! Each Yee field component samples ε on its own staggered lattice; a
//! sample's value is the **area-weighted average** of the painted
//! permittivity over the `Δ × Δ` square centered on the sample position.
//! The average is computed by supersampling ([`SUBPIXEL`]² points per
//! square) — exact area weighting in the limit, and honest O(Δ²)-accurate
//! interface placement at the default. This is scalar smoothing only: the
//! anisotropic-tensor treatment that restores second-order convergence at
//! *arbitrary* interfaces (Farjadpour et al., Opt. Lett. 31, 2972 (2006))
//! is a later milestone.
//!
//! Painting is ordered: later shapes overwrite earlier ones where they
//! overlap (each paint blends `f·ε_shape + (1−f)·ε_before` with `f` the
//! covered area fraction of the sample square).

/// Sub-pixel supersampling factor per axis (samples per square = this²).
pub const SUBPIXEL: usize = 4;

/// A paintable 2D region, coordinates in length units.
#[derive(Debug, Clone, PartialEq)]
pub enum Shape2 {
    /// Axis-aligned rectangle `[x0, x1] × [y0, y1]`.
    Rect {
        /// Low-x edge.
        x0: f64,
        /// Low-y edge.
        y0: f64,
        /// High-x edge.
        x1: f64,
        /// High-y edge.
        y1: f64,
    },
    /// Disc of radius `r` centered at `(cx, cy)`.
    Circle {
        /// Center x.
        cx: f64,
        /// Center y.
        cy: f64,
        /// Radius.
        r: f64,
    },
    /// Simple polygon (closed automatically), even-odd fill rule.
    Polygon {
        /// Vertices in order; the last connects back to the first.
        pts: Vec<(f64, f64)>,
    },
}

impl Shape2 {
    /// Rectangle from corner spans.
    pub fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Self {
        Shape2::Rect { x0, y0, x1, y1 }
    }

    /// Disc.
    pub fn circle(cx: f64, cy: f64, r: f64) -> Self {
        Shape2::Circle { cx, cy, r }
    }

    /// Polygon from a vertex list.
    pub fn polygon(pts: Vec<(f64, f64)>) -> Self {
        assert!(pts.len() >= 3, "polygon needs at least 3 vertices");
        Shape2::Polygon { pts }
    }

    /// Point-in-shape test.
    pub fn contains(&self, x: f64, y: f64) -> bool {
        match self {
            Shape2::Rect { x0, y0, x1, y1 } => x >= *x0 && x <= *x1 && y >= *y0 && y <= *y1,
            Shape2::Circle { cx, cy, r } => {
                let dx = x - cx;
                let dy = y - cy;
                dx * dx + dy * dy <= r * r
            }
            Shape2::Polygon { pts } => poly_contains(pts, x, y),
        }
    }

    /// Covered-area fraction of the `delta × delta` square centered at
    /// `(cx, cy)`, by `SUBPIXEL²`-point supersampling.
    pub fn coverage(&self, cx: f64, cy: f64, delta: f64) -> f64 {
        let n = SUBPIXEL;
        let mut hit = 0usize;
        for a in 0..n {
            // Sub-cell centers: offset −Δ/2 + (a+½)·Δ/n. None lie exactly
            // on a half-integer grid line, so a boundary flush with a
            // sample line splits deterministically 50/50.
            let x = cx - 0.5 * delta + (a as f64 + 0.5) * delta / n as f64;
            for b in 0..n {
                let y = cy - 0.5 * delta + (b as f64 + 0.5) * delta / n as f64;
                if self.contains(x, y) {
                    hit += 1;
                }
            }
        }
        hit as f64 / (n * n) as f64
    }
}

/// Even-odd crossing test.
fn poly_contains(pts: &[(f64, f64)], x: f64, y: f64) -> bool {
    let mut inside = false;
    let n = pts.len();
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = pts[i];
        let (xj, yj) = pts[j];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Blend `shape` at permittivity `eps` into the sample lattice `field`,
/// whose sample `(i, j)` sits at `(ox + i·delta, oy + j·delta)`.
///
/// Used by [`crate::sim::Simulation::paint`] once per staggered ε lattice.
pub fn paint_component(
    field: &mut crate::grid::Field2,
    ox: f64,
    oy: f64,
    delta: f64,
    shape: &Shape2,
    eps: f64,
) {
    for i in 0..field.ns_x() {
        let x = ox + i as f64 * delta;
        for j in 0..field.ns_y() {
            let y = oy + j as f64 * delta;
            let f = shape.coverage(x, y, delta);
            if f > 0.0 {
                let old = field.at(i, j);
                *field.at_mut(i, j) = f * eps + (1.0 - f) * old;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::Field2;

    #[test]
    fn halfspace_boundary_sample_is_arithmetic_mean() {
        // ε = 4 half-space with its edge exactly on a sample line: the
        // on-edge sample must average to (1 + 4)/2.
        let mut eps = Field2::filled(11, 3, 1.0);
        let shape = Shape2::rect(5.0, -10.0, 100.0, 10.0);
        paint_component(&mut eps, 0.0, 0.0, 1.0, &shape, 4.0);
        assert_eq!(eps.at(2, 1), 1.0);
        assert_eq!(eps.at(8, 1), 4.0);
        assert!((eps.at(5, 1) - 2.5).abs() < 1e-12);
    }

    #[test]
    fn circle_painted_area_matches_pi_r_squared() {
        // Integrate the painted excess permittivity: Σ(ε − 1)·Δ² should be
        // (ε_c − 1)·πr² to sub-pixel accuracy.
        let delta = 0.1;
        let mut eps = Field2::filled(81, 81, 1.0);
        let shape = Shape2::circle(4.0, 4.0, 2.0);
        paint_component(&mut eps, 0.0, 0.0, delta, &shape, 3.0);
        let painted: f64 = eps
            .as_slice()
            .iter()
            .map(|e| (e - 1.0) * delta * delta)
            .sum();
        let exact = 2.0 * std::f64::consts::PI * 2.0 * 2.0;
        assert!(
            (painted - exact).abs() / exact < 2e-3,
            "painted {painted} vs exact {exact}"
        );
    }

    #[test]
    fn polygon_triangle_contains() {
        let tri = Shape2::polygon(vec![(0.0, 0.0), (2.0, 0.0), (0.0, 2.0)]);
        assert!(tri.contains(0.5, 0.5));
        assert!(!tri.contains(1.5, 1.5));
        assert!(!tri.contains(-0.1, 0.5));
    }

    #[test]
    fn later_paint_wins() {
        let mut eps = Field2::filled(5, 5, 1.0);
        paint_component(
            &mut eps,
            0.0,
            0.0,
            1.0,
            &Shape2::rect(-1.0, -1.0, 5.0, 5.0),
            4.0,
        );
        paint_component(
            &mut eps,
            0.0,
            0.0,
            1.0,
            &Shape2::rect(-1.0, -1.0, 5.0, 5.0),
            2.0,
        );
        assert_eq!(eps.at(2, 2), 2.0);
    }
}
