#![warn(missing_docs)]

//! B-spline and NURBS evaluation for the vcad kernel.
//!
//! Provides non-rational B-spline and rational NURBS curves and surfaces,
//! evaluated via De Boor's algorithm. Implements the `Curve3d` and `Surface`
//! traits from `vcad-kernel-geom` so they integrate directly into the B-rep
//! kernel.
//!
//! # Key types
//!
//! - [`BSplineCurve`] — non-rational B-spline curve in 3D
//! - [`BSplineSurface`] — non-rational tensor-product B-spline surface
//! - [`NurbsCurve`] — rational B-spline (NURBS) curve in 3D
//! - [`NurbsSurface`] — rational tensor-product NURBS surface
//!
//! # Algorithms
//!
//! - **De Boor's algorithm** for stable B-spline evaluation
//! - **Boehm's algorithm** for knot insertion (refinement)

use std::any::Any;
use tang::Scalar;
use vcad_kernel_geom::{CurveKind, SurfaceKind};
use vcad_kernel_math::{Dir3, Point2, Point3, Transform, Vec3};

// =============================================================================
// Knot vector utilities
// =============================================================================

/// Validate a knot vector: non-decreasing, length = n_control_points + degree + 1.
fn validate_knots(knots: &[f64], n_points: usize, degree: usize) -> bool {
    if knots.len() != n_points + degree + 1 {
        return false;
    }
    for i in 1..knots.len() {
        if knots[i] < knots[i - 1] {
            return false;
        }
    }
    true
}

/// Find the knot span index for parameter `t`.
///
/// Returns `i` such that `knots[i] <= t < knots[i+1]`, clamped to valid range.
/// For `t` at the end of the domain, returns the last valid span.
fn find_span(knots: &[f64], n: usize, degree: usize, t: f64) -> usize {
    // n = number of control points - 1 (last index)
    if t >= knots[n + 1] {
        return n; // last valid span
    }
    if t <= knots[degree] {
        return degree; // first valid span
    }
    // Binary search
    let mut low = degree;
    let mut high = n + 1;
    let mut mid = (low + high) / 2;
    while t < knots[mid] || t >= knots[mid + 1] {
        if t < knots[mid] {
            high = mid;
        } else {
            low = mid;
        }
        mid = (low + high) / 2;
    }
    mid
}

/// Compute non-zero basis function values at parameter `t`.
///
/// Returns a vector of `degree + 1` values `N[span-degree..=span]` at `t`.
fn basis_functions(knots: &[f64], span: usize, degree: usize, t: f64) -> Vec<f64> {
    let mut n = vec![0.0; degree + 1];
    let mut left = vec![0.0; degree + 1];
    let mut right = vec![0.0; degree + 1];
    n[0] = 1.0;

    for j in 1..=degree {
        left[j] = t - knots[span + 1 - j];
        right[j] = knots[span + j] - t;
        let mut saved = 0.0;
        for r in 0..j {
            let denom = right[r + 1] + left[j - r];
            if denom.abs() < 1e-30 {
                // Zero-length knot interval — avoid division by zero
                n[j] = saved;
                continue;
            }
            let temp = n[r] / denom;
            n[r] = saved + right[r + 1] * temp;
            saved = left[j - r] * temp;
        }
        n[j] = saved;
    }

    n
}

// =============================================================================
// B-spline curve
// =============================================================================

/// A non-rational B-spline curve in 3D.
///
/// Defined by control points, a knot vector, and a polynomial degree.
/// Evaluated using De Boor's algorithm.
#[derive(Debug, Clone)]
pub struct BSplineCurve {
    /// Control points in 3D.
    pub control_points: Vec<Point3>,
    /// Knot vector. Length = control_points.len() + degree + 1.
    pub knots: Vec<f64>,
    /// Polynomial degree (order = degree + 1).
    pub degree: usize,
}

impl BSplineCurve {
    /// Create a B-spline curve.
    ///
    /// # Panics
    /// Panics if the knot vector length doesn't match `n + degree + 1`.
    pub fn new(control_points: Vec<Point3>, knots: Vec<f64>, degree: usize) -> Self {
        assert!(
            validate_knots(&knots, control_points.len(), degree),
            "invalid knot vector: len={} but expected {} (n={}, p={})",
            knots.len(),
            control_points.len() + degree + 1,
            control_points.len(),
            degree
        );
        Self {
            control_points,
            knots,
            degree,
        }
    }

    /// Create a clamped uniform B-spline with the given degree.
    ///
    /// The knot vector is clamped (first and last knots repeated `degree+1` times)
    /// with uniform internal spacing.
    pub fn clamped_uniform(control_points: Vec<Point3>, degree: usize) -> Self {
        let n = control_points.len();
        let m = n + degree + 1;
        let mut knots = vec![0.0; m];

        // Clamped: first (degree+1) = 0, last (degree+1) = 1
        let n_internal = m - 2 * (degree + 1);
        for i in 0..=degree {
            knots[i] = 0.0;
            knots[m - 1 - i] = 1.0;
        }
        for i in 1..=n_internal {
            knots[degree + i] = i as f64 / (n_internal + 1) as f64;
        }

        Self::new(control_points, knots, degree)
    }

    /// Evaluate the curve at parameter `t` using De Boor's algorithm.
    pub fn eval(&self, t: f64) -> Point3 {
        let n = self.control_points.len() - 1;
        let t = t.clamp(self.knots[self.degree], self.knots[n + 1]);
        let span = find_span(&self.knots, n, self.degree, t);
        let basis = basis_functions(&self.knots, span, self.degree, t);

        let mut point = Point3::origin();
        for (i, &b) in basis.iter().enumerate() {
            let idx = span - self.degree + i;
            let cp = &self.control_points[idx];
            point.x += b * cp.x;
            point.y += b * cp.y;
            point.z += b * cp.z;
        }
        point
    }

    /// Compute the tangent vector at parameter `t`.
    ///
    /// Uses the derivative formula: C'(t) = sum of p/Δknot * ΔP * N_{i,p-1}(t).
    /// Falls back to finite differences for robustness.
    pub fn tangent(&self, t: f64) -> Vec3 {
        if self.degree == 0 || self.control_points.len() < 2 {
            return Vec3::zeros();
        }
        let (t_min, t_max) = self.parameter_domain();
        let dt = (t_max - t_min) * 1e-7;
        let p0 = self.eval((t - dt).max(t_min));
        let p1 = self.eval((t + dt).min(t_max));
        (p1 - p0) / (2.0 * dt)
    }

    /// Parameter domain `(t_min, t_max)`.
    pub fn parameter_domain(&self) -> (f64, f64) {
        (
            self.knots[self.degree],
            self.knots[self.control_points.len()],
        )
    }

    /// Number of control points.
    pub fn num_control_points(&self) -> usize {
        self.control_points.len()
    }

    /// Insert a knot value using Boehm's algorithm.
    ///
    /// Returns a new curve with one additional control point.
    pub fn insert_knot(&self, t: f64) -> Self {
        let n = self.control_points.len() - 1;
        let p = self.degree;
        let span = find_span(&self.knots, n, p, t);

        // New knot vector
        let mut new_knots = Vec::with_capacity(self.knots.len() + 1);
        new_knots.extend_from_slice(&self.knots[..=span]);
        new_knots.push(t);
        new_knots.extend_from_slice(&self.knots[span + 1..]);

        // New control points
        let mut new_pts = Vec::with_capacity(self.control_points.len() + 1);

        // Points before the affected range
        for i in 0..=(span.saturating_sub(p)) {
            new_pts.push(self.control_points[i]);
        }

        // Affected points
        for i in (span - p + 1)..=span {
            let alpha = (t - self.knots[i]) / (self.knots[i + p] - self.knots[i]);
            let pt = Point3::new(
                (1.0 - alpha) * self.control_points[i - 1].x + alpha * self.control_points[i].x,
                (1.0 - alpha) * self.control_points[i - 1].y + alpha * self.control_points[i].y,
                (1.0 - alpha) * self.control_points[i - 1].z + alpha * self.control_points[i].z,
            );
            new_pts.push(pt);
        }

        // Points after the affected range
        for i in span..=n {
            new_pts.push(self.control_points[i]);
        }

        Self::new(new_pts, new_knots, p)
    }
}

impl vcad_kernel_geom::Curve3d for BSplineCurve {
    fn evaluate(&self, t: f64) -> Point3 {
        self.eval(t)
    }

    fn tangent(&self, t: f64) -> Vec3 {
        BSplineCurve::tangent(self, t)
    }

    fn domain(&self) -> (f64, f64) {
        self.parameter_domain()
    }

    fn curve_type(&self) -> CurveKind {
        // B-splines don't map to Line or Circle — we'd need a new variant
        // For now, report as Line (general curve)
        CurveKind::Line
    }

    fn clone_box(&self) -> Box<dyn vcad_kernel_geom::Curve3d> {
        Box::new(self.clone())
    }
}

// =============================================================================
// B-spline surface
// =============================================================================

/// A non-rational tensor-product B-spline surface.
///
/// Control points are stored in row-major order: `points[v_idx * n_u + u_idx]`.
/// Knot vectors remain `f64` (they are parametric positions, not differentiated).
#[derive(Debug, Clone)]
pub struct BSplineSurface<S = f64> {
    /// Control points in row-major order.
    pub control_points: Vec<tang::Point3<S>>,
    /// Number of control points in the u direction.
    pub n_u: usize,
    /// Number of control points in the v direction.
    pub n_v: usize,
    /// Knot vector in u. Length = n_u + degree_u + 1.
    pub knots_u: Vec<f64>,
    /// Knot vector in v. Length = n_v + degree_v + 1.
    pub knots_v: Vec<f64>,
    /// Polynomial degree in u.
    pub degree_u: usize,
    /// Polynomial degree in v.
    pub degree_v: usize,
}

impl BSplineSurface {
    /// Create a B-spline surface.
    ///
    /// `control_points` is in row-major order: `[v=0,u=0], [v=0,u=1], ..., [v=1,u=0], ...`
    ///
    /// # Panics
    /// Panics if dimensions don't match.
    pub fn new(
        control_points: Vec<Point3>,
        n_u: usize,
        n_v: usize,
        knots_u: Vec<f64>,
        knots_v: Vec<f64>,
        degree_u: usize,
        degree_v: usize,
    ) -> Self {
        assert_eq!(
            control_points.len(),
            n_u * n_v,
            "control points count mismatch: {} != {} * {}",
            control_points.len(),
            n_u,
            n_v
        );
        assert!(
            validate_knots(&knots_u, n_u, degree_u),
            "invalid u knot vector"
        );
        assert!(
            validate_knots(&knots_v, n_v, degree_v),
            "invalid v knot vector"
        );
        Self {
            control_points,
            n_u,
            n_v,
            knots_u,
            knots_v,
            degree_u,
            degree_v,
        }
    }

    /// Partial derivative with respect to u (finite differences).
    pub fn deriv_u(&self, u: f64, v: f64) -> Vec3 {
        let ((u_min, u_max), _) = self.parameter_domain();
        let du = (u_max - u_min) * 1e-7;
        let p0 = self.eval((u - du).max(u_min), v);
        let p1 = self.eval((u + du).min(u_max), v);
        (p1 - p0) / (2.0 * du)
    }

    /// Partial derivative with respect to v (finite differences).
    pub fn deriv_v(&self, u: f64, v: f64) -> Vec3 {
        let (_, (v_min, v_max)) = self.parameter_domain();
        let dv = (v_max - v_min) * 1e-7;
        let p0 = self.eval(u, (v - dv).max(v_min));
        let p1 = self.eval(u, (v + dv).min(v_max));
        (p1 - p0) / (2.0 * dv)
    }

    /// Promote this B-spline surface to a generic scalar type.
    pub fn lift<T: Scalar>(&self) -> BSplineSurface<T> {
        BSplineSurface {
            control_points: self
                .control_points
                .iter()
                .map(|p| tang::Point3::new(T::from_f64(p.x), T::from_f64(p.y), T::from_f64(p.z)))
                .collect(),
            n_u: self.n_u,
            n_v: self.n_v,
            knots_u: self.knots_u.clone(),
            knots_v: self.knots_v.clone(),
            degree_u: self.degree_u,
            degree_v: self.degree_v,
        }
    }
}

impl<S: Scalar> BSplineSurface<S> {
    /// Get a control point at `(u_idx, v_idx)`.
    fn cp(&self, u_idx: usize, v_idx: usize) -> &tang::Point3<S> {
        &self.control_points[v_idx * self.n_u + u_idx]
    }

    /// Evaluate the surface at `(u, v)` using tensor-product De Boor.
    ///
    /// Basis functions are computed from the f64 knot vectors; the weighted
    /// sum of control points carries the generic scalar type.
    pub fn eval(&self, u: f64, v: f64) -> tang::Point3<S> {
        let nu = self.n_u - 1;
        let nv = self.n_v - 1;
        let u = u.clamp(self.knots_u[self.degree_u], self.knots_u[nu + 1]);
        let v = v.clamp(self.knots_v[self.degree_v], self.knots_v[nv + 1]);

        let span_u = find_span(&self.knots_u, nu, self.degree_u, u);
        let span_v = find_span(&self.knots_v, nv, self.degree_v, v);
        let basis_u = basis_functions(&self.knots_u, span_u, self.degree_u, u);
        let basis_v = basis_functions(&self.knots_v, span_v, self.degree_v, v);

        let mut px = S::ZERO;
        let mut py = S::ZERO;
        let mut pz = S::ZERO;
        for (j, &bv) in basis_v.iter().enumerate() {
            let v_idx = span_v - self.degree_v + j;
            for (i, &bu) in basis_u.iter().enumerate() {
                let u_idx = span_u - self.degree_u + i;
                let w = S::from_f64(bu * bv);
                let cp = self.cp(u_idx, v_idx);
                px += w * cp.x;
                py += w * cp.y;
                pz += w * cp.z;
            }
        }
        tang::Point3::new(px, py, pz)
    }

    /// Parameter domain.
    pub fn parameter_domain(&self) -> ((f64, f64), (f64, f64)) {
        (
            (self.knots_u[self.degree_u], self.knots_u[self.n_u]),
            (self.knots_v[self.degree_v], self.knots_v[self.n_v]),
        )
    }
}

impl vcad_kernel_geom::Surface for BSplineSurface {
    fn evaluate(&self, uv: Point2) -> Point3 {
        self.eval(uv.x, uv.y)
    }

    fn normal(&self, uv: Point2) -> Dir3 {
        let du = self.deriv_u(uv.x, uv.y);
        let dv = self.deriv_v(uv.x, uv.y);
        let n = du.cross(dv);
        if n.norm() < 1e-15 {
            Dir3::new_normalize(Vec3::z())
        } else {
            Dir3::new_normalize(n)
        }
    }

    fn d_du(&self, uv: Point2) -> Vec3 {
        self.deriv_u(uv.x, uv.y)
    }

    fn d_dv(&self, uv: Point2) -> Vec3 {
        self.deriv_v(uv.x, uv.y)
    }

    fn domain(&self) -> ((f64, f64), (f64, f64)) {
        self.parameter_domain()
    }

    fn surface_type(&self) -> SurfaceKind {
        SurfaceKind::BSpline
    }

    fn clone_box(&self) -> Box<dyn vcad_kernel_geom::Surface> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn transform(&self, t: &Transform) -> Box<dyn vcad_kernel_geom::Surface> {
        let new_pts = self
            .control_points
            .iter()
            .map(|p| t.apply_point(p))
            .collect();
        Box::new(BSplineSurface {
            control_points: new_pts,
            n_u: self.n_u,
            n_v: self.n_v,
            knots_u: self.knots_u.clone(),
            knots_v: self.knots_v.clone(),
            degree_u: self.degree_u,
            degree_v: self.degree_v,
        })
    }
}

// =============================================================================
// NURBS (rational B-spline) curve
// =============================================================================

/// A weighted control point for NURBS (homogeneous coordinates).
#[derive(Debug, Clone, Copy)]
pub struct WeightedPoint {
    /// 3D position (in Cartesian coordinates, not weighted).
    pub point: Point3,
    /// Weight (must be > 0).
    pub weight: f64,
}

impl WeightedPoint {
    /// Create a weighted point.
    pub fn new(point: Point3, weight: f64) -> Self {
        Self { point, weight }
    }

    /// Create with unit weight.
    pub fn unweighted(point: Point3) -> Self {
        Self { point, weight: 1.0 }
    }

    /// Convert to homogeneous coordinates: `(w*x, w*y, w*z, w)`.
    pub fn to_homogeneous(&self) -> [f64; 4] {
        [
            self.weight * self.point.x,
            self.weight * self.point.y,
            self.weight * self.point.z,
            self.weight,
        ]
    }

    /// Convert from homogeneous coordinates.
    pub fn from_homogeneous(h: [f64; 4]) -> Self {
        let w = h[3];
        if w.abs() < 1e-30 {
            Self {
                point: Point3::origin(),
                weight: 0.0,
            }
        } else {
            Self {
                point: Point3::new(h[0] / w, h[1] / w, h[2] / w),
                weight: w,
            }
        }
    }
}

/// A rational B-spline (NURBS) curve in 3D.
///
/// Evaluated by computing a 4D non-rational B-spline in homogeneous
/// coordinates and dividing by the weight.
#[derive(Debug, Clone)]
pub struct NurbsCurve {
    /// Weighted control points.
    pub control_points: Vec<WeightedPoint>,
    /// Knot vector.
    pub knots: Vec<f64>,
    /// Polynomial degree.
    pub degree: usize,
}

impl NurbsCurve {
    /// Create a NURBS curve.
    pub fn new(control_points: Vec<WeightedPoint>, knots: Vec<f64>, degree: usize) -> Self {
        assert!(
            validate_knots(&knots, control_points.len(), degree),
            "invalid knot vector"
        );
        Self {
            control_points,
            knots,
            degree,
        }
    }

    /// Evaluate the curve at parameter `t`.
    pub fn eval(&self, t: f64) -> Point3 {
        let n = self.control_points.len() - 1;
        let t = t.clamp(self.knots[self.degree], self.knots[n + 1]);
        let span = find_span(&self.knots, n, self.degree, t);
        let basis = basis_functions(&self.knots, span, self.degree, t);

        let mut hx = 0.0;
        let mut hy = 0.0;
        let mut hz = 0.0;
        let mut hw = 0.0;

        for (i, &b) in basis.iter().enumerate() {
            let idx = span - self.degree + i;
            let h = self.control_points[idx].to_homogeneous();
            hx += b * h[0];
            hy += b * h[1];
            hz += b * h[2];
            hw += b * h[3];
        }

        if hw.abs() < 1e-30 {
            Point3::origin()
        } else {
            Point3::new(hx / hw, hy / hw, hz / hw)
        }
    }

    /// Parameter domain.
    pub fn parameter_domain(&self) -> (f64, f64) {
        (
            self.knots[self.degree],
            self.knots[self.control_points.len()],
        )
    }

    /// Create a NURBS circle in the XY plane.
    ///
    /// A full circle requires 9 control points with degree 2.
    pub fn circle(center: Point3, radius: f64) -> Self {
        let w = 1.0_f64 / 2.0_f64.sqrt(); // cos(45°)
        let r = radius;
        let c = center;

        // 9 control points for a full circle (quadratic NURBS)
        let pts = vec![
            WeightedPoint::new(Point3::new(c.x + r, c.y, c.z), 1.0),
            WeightedPoint::new(Point3::new(c.x + r, c.y + r, c.z), w),
            WeightedPoint::new(Point3::new(c.x, c.y + r, c.z), 1.0),
            WeightedPoint::new(Point3::new(c.x - r, c.y + r, c.z), w),
            WeightedPoint::new(Point3::new(c.x - r, c.y, c.z), 1.0),
            WeightedPoint::new(Point3::new(c.x - r, c.y - r, c.z), w),
            WeightedPoint::new(Point3::new(c.x, c.y - r, c.z), 1.0),
            WeightedPoint::new(Point3::new(c.x + r, c.y - r, c.z), w),
            WeightedPoint::new(Point3::new(c.x + r, c.y, c.z), 1.0),
        ];

        let knots = vec![
            0.0, 0.0, 0.0, 0.25, 0.25, 0.5, 0.5, 0.75, 0.75, 1.0, 1.0, 1.0,
        ];

        Self::new(pts, knots, 2)
    }

    /// Insert a knot using Boehm's algorithm (rational version).
    pub fn insert_knot(&self, t: f64) -> Self {
        let n = self.control_points.len() - 1;
        let p = self.degree;
        let span = find_span(&self.knots, n, p, t);

        // New knot vector
        let mut new_knots = Vec::with_capacity(self.knots.len() + 1);
        new_knots.extend_from_slice(&self.knots[..=span]);
        new_knots.push(t);
        new_knots.extend_from_slice(&self.knots[span + 1..]);

        // New control points — interpolate in homogeneous space
        let mut new_pts = Vec::with_capacity(self.control_points.len() + 1);

        for i in 0..=(span.saturating_sub(p)) {
            new_pts.push(self.control_points[i]);
        }

        for i in (span - p + 1)..=span {
            let alpha = (t - self.knots[i]) / (self.knots[i + p] - self.knots[i]);
            let h0 = self.control_points[i - 1].to_homogeneous();
            let h1 = self.control_points[i].to_homogeneous();
            let h_new = [
                (1.0 - alpha) * h0[0] + alpha * h1[0],
                (1.0 - alpha) * h0[1] + alpha * h1[1],
                (1.0 - alpha) * h0[2] + alpha * h1[2],
                (1.0 - alpha) * h0[3] + alpha * h1[3],
            ];
            new_pts.push(WeightedPoint::from_homogeneous(h_new));
        }

        for i in span..=n {
            new_pts.push(self.control_points[i]);
        }

        Self::new(new_pts, new_knots, p)
    }
}

impl vcad_kernel_geom::Curve3d for NurbsCurve {
    fn evaluate(&self, t: f64) -> Point3 {
        self.eval(t)
    }

    fn tangent(&self, t: f64) -> Vec3 {
        // Finite-difference tangent for NURBS
        let (t_min, t_max) = self.parameter_domain();
        let dt = (t_max - t_min) * 1e-6;
        let p0 = self.eval((t - dt).max(t_min));
        let p1 = self.eval((t + dt).min(t_max));
        (p1 - p0) / (2.0 * dt)
    }

    fn domain(&self) -> (f64, f64) {
        self.parameter_domain()
    }

    fn curve_type(&self) -> CurveKind {
        CurveKind::Circle // NURBS can represent circles exactly
    }

    fn clone_box(&self) -> Box<dyn vcad_kernel_geom::Curve3d> {
        Box::new(self.clone())
    }
}

// =============================================================================
// NURBS surface
// =============================================================================

/// A rational tensor-product NURBS surface.
#[derive(Debug, Clone)]
pub struct NurbsSurface {
    /// Weighted control points in row-major order.
    pub control_points: Vec<WeightedPoint>,
    /// Number of control points in u.
    pub n_u: usize,
    /// Number of control points in v.
    pub n_v: usize,
    /// Knot vector in u.
    pub knots_u: Vec<f64>,
    /// Knot vector in v.
    pub knots_v: Vec<f64>,
    /// Degree in u.
    pub degree_u: usize,
    /// Degree in v.
    pub degree_v: usize,
}

impl NurbsSurface {
    /// Create a NURBS surface.
    pub fn new(
        control_points: Vec<WeightedPoint>,
        n_u: usize,
        n_v: usize,
        knots_u: Vec<f64>,
        knots_v: Vec<f64>,
        degree_u: usize,
        degree_v: usize,
    ) -> Self {
        assert_eq!(control_points.len(), n_u * n_v);
        assert!(validate_knots(&knots_u, n_u, degree_u));
        assert!(validate_knots(&knots_v, n_v, degree_v));
        Self {
            control_points,
            n_u,
            n_v,
            knots_u,
            knots_v,
            degree_u,
            degree_v,
        }
    }

    /// Get a weighted control point at `(u_idx, v_idx)`.
    fn wcp(&self, u_idx: usize, v_idx: usize) -> &WeightedPoint {
        &self.control_points[v_idx * self.n_u + u_idx]
    }

    /// Evaluate at `(u, v)`.
    pub fn eval(&self, u: f64, v: f64) -> Point3 {
        let nu = self.n_u - 1;
        let nv = self.n_v - 1;
        let u = u.clamp(self.knots_u[self.degree_u], self.knots_u[nu + 1]);
        let v = v.clamp(self.knots_v[self.degree_v], self.knots_v[nv + 1]);

        let span_u = find_span(&self.knots_u, nu, self.degree_u, u);
        let span_v = find_span(&self.knots_v, nv, self.degree_v, v);
        let basis_u = basis_functions(&self.knots_u, span_u, self.degree_u, u);
        let basis_v = basis_functions(&self.knots_v, span_v, self.degree_v, v);

        let mut hx = 0.0;
        let mut hy = 0.0;
        let mut hz = 0.0;
        let mut hw = 0.0;

        for (j, &bv) in basis_v.iter().enumerate() {
            let v_idx = span_v - self.degree_v + j;
            for (i, &bu) in basis_u.iter().enumerate() {
                let u_idx = span_u - self.degree_u + i;
                let w = bu * bv;
                let h = self.wcp(u_idx, v_idx).to_homogeneous();
                hx += w * h[0];
                hy += w * h[1];
                hz += w * h[2];
                hw += w * h[3];
            }
        }

        if hw.abs() < 1e-30 {
            Point3::origin()
        } else {
            Point3::new(hx / hw, hy / hw, hz / hw)
        }
    }

    /// Parameter domain.
    pub fn parameter_domain(&self) -> ((f64, f64), (f64, f64)) {
        (
            (self.knots_u[self.degree_u], self.knots_u[self.n_u]),
            (self.knots_v[self.degree_v], self.knots_v[self.n_v]),
        )
    }
}

impl vcad_kernel_geom::Surface for NurbsSurface {
    fn evaluate(&self, uv: Point2) -> Point3 {
        self.eval(uv.x, uv.y)
    }

    fn normal(&self, uv: Point2) -> Dir3 {
        // Finite-difference normal
        let ((u_min, u_max), (v_min, v_max)) = self.parameter_domain();
        let du = (u_max - u_min) * 1e-6;
        let dv = (v_max - v_min) * 1e-6;

        let p0 = self.eval(uv.x, uv.y);
        let pu = self.eval((uv.x + du).min(u_max), uv.y);
        let pv = self.eval(uv.x, (uv.y + dv).min(v_max));

        let du_vec = (pu - p0) / du;
        let dv_vec = (pv - p0) / dv;
        let n = du_vec.cross(dv_vec);
        if n.norm() < 1e-15 {
            Dir3::new_normalize(Vec3::z())
        } else {
            Dir3::new_normalize(n)
        }
    }

    fn d_du(&self, uv: Point2) -> Vec3 {
        let ((u_min, u_max), _) = self.parameter_domain();
        let du = (u_max - u_min) * 1e-6;
        let p0 = self.eval(uv.x, uv.y);
        let pu = self.eval((uv.x + du).min(u_max), uv.y);
        (pu - p0) / du
    }

    fn d_dv(&self, uv: Point2) -> Vec3 {
        let (_, (v_min, v_max)) = self.parameter_domain();
        let dv = (v_max - v_min) * 1e-6;
        let p0 = self.eval(uv.x, uv.y);
        let pv = self.eval(uv.x, (uv.y + dv).min(v_max));
        (pv - p0) / dv
    }

    fn domain(&self) -> ((f64, f64), (f64, f64)) {
        self.parameter_domain()
    }

    fn surface_type(&self) -> SurfaceKind {
        SurfaceKind::BSpline
    }

    fn clone_box(&self) -> Box<dyn vcad_kernel_geom::Surface> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn transform(&self, t: &Transform) -> Box<dyn vcad_kernel_geom::Surface> {
        let new_pts = self
            .control_points
            .iter()
            .map(|wp| WeightedPoint::new(t.apply_point(&wp.point), wp.weight))
            .collect();
        Box::new(NurbsSurface {
            control_points: new_pts,
            n_u: self.n_u,
            n_v: self.n_v,
            knots_u: self.knots_u.clone(),
            knots_v: self.knots_v.clone(),
            degree_u: self.degree_u,
            degree_v: self.degree_v,
        })
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ---- B-spline curve tests ----

    #[test]
    fn test_bspline_line() {
        // Degree 1 B-spline = polyline
        let pts = vec![Point3::new(0.0, 0.0, 0.0), Point3::new(10.0, 0.0, 0.0)];
        let knots = vec![0.0, 0.0, 1.0, 1.0];
        let curve = BSplineCurve::new(pts, knots, 1);

        let p0 = curve.eval(0.0);
        assert!((p0.x - 0.0).abs() < 1e-10);
        let p1 = curve.eval(1.0);
        assert!((p1.x - 10.0).abs() < 1e-10);
        let mid = curve.eval(0.5);
        assert!((mid.x - 5.0).abs() < 1e-10);
    }

    #[test]
    fn test_bspline_quadratic() {
        // Quadratic B-spline with 4 control points
        let pts = vec![
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 2.0, 0.0),
            Point3::new(3.0, 2.0, 0.0),
            Point3::new(4.0, 0.0, 0.0),
        ];
        let knots = vec![0.0, 0.0, 0.0, 0.5, 1.0, 1.0, 1.0];
        let curve = BSplineCurve::new(pts, knots, 2);

        // Endpoints should interpolate
        let start = curve.eval(0.0);
        assert!((start.x - 0.0).abs() < 1e-10);
        assert!((start.y - 0.0).abs() < 1e-10);

        let end = curve.eval(1.0);
        assert!((end.x - 4.0).abs() < 1e-10);
        assert!((end.y - 0.0).abs() < 1e-10);

        // Midpoint should be elevated (y > 0)
        let mid = curve.eval(0.5);
        assert!(mid.y > 0.0, "midpoint y should be positive: {}", mid.y);
    }

    #[test]
    fn test_bspline_clamped_uniform() {
        let pts = vec![
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 1.0, 0.0),
            Point3::new(2.0, 0.0, 0.0),
            Point3::new(3.0, 1.0, 0.0),
            Point3::new(4.0, 0.0, 0.0),
        ];
        let curve = BSplineCurve::clamped_uniform(pts.clone(), 3);

        // Clamped: endpoints should interpolate
        let start = curve.eval(0.0);
        assert!((start - pts[0]).norm() < 1e-10);
        let end = curve.eval(1.0);
        assert!((end - pts[4]).norm() < 1e-10);
    }

    #[test]
    fn test_bspline_tangent() {
        // Straight line — tangent should be constant
        let pts = vec![Point3::new(0.0, 0.0, 0.0), Point3::new(10.0, 0.0, 0.0)];
        let knots = vec![0.0, 0.0, 1.0, 1.0];
        let curve = BSplineCurve::new(pts, knots, 1);

        let t = curve.tangent(0.5);
        assert!((t.x - 10.0).abs() < 1e-8, "tangent x: {}", t.x);
        assert!(t.y.abs() < 1e-8);
    }

    #[test]
    fn test_bspline_knot_insertion() {
        let pts = vec![
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 2.0, 0.0),
            Point3::new(3.0, 2.0, 0.0),
            Point3::new(4.0, 0.0, 0.0),
        ];
        let knots = vec![0.0, 0.0, 0.0, 0.5, 1.0, 1.0, 1.0];
        let curve = BSplineCurve::new(pts, knots, 2);

        // Insert a knot at t=0.25
        let refined = curve.insert_knot(0.25);

        // The refined curve should have one more control point
        assert_eq!(refined.num_control_points(), 5);

        // Evaluate at several points — should match original
        for i in 0..=10 {
            let t = i as f64 / 10.0;
            let p_orig = curve.eval(t);
            let p_ref = refined.eval(t);
            assert!(
                (p_orig - p_ref).norm() < 1e-8,
                "mismatch at t={}: orig={:?}, ref={:?}",
                t,
                p_orig,
                p_ref
            );
        }
    }

    #[test]
    fn test_bspline_curve3d_trait() {
        use vcad_kernel_geom::Curve3d;

        let pts = vec![Point3::new(0.0, 0.0, 0.0), Point3::new(10.0, 0.0, 0.0)];
        let curve = BSplineCurve::clamped_uniform(pts, 1);

        let p = curve.evaluate(0.5);
        assert!((p.x - 5.0).abs() < 1e-10);

        let (t_min, t_max) = curve.domain();
        assert!((t_min - 0.0).abs() < 1e-10);
        assert!((t_max - 1.0).abs() < 1e-10);
    }

    // ---- B-spline surface tests ----

    #[test]
    fn test_bspline_surface_bilinear() {
        // Degree (1,1) surface = bilinear patch
        let pts = vec![
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(10.0, 0.0, 0.0),
            Point3::new(0.0, 10.0, 0.0),
            Point3::new(10.0, 10.0, 0.0),
        ];
        let knots_u = vec![0.0, 0.0, 1.0, 1.0];
        let knots_v = vec![0.0, 0.0, 1.0, 1.0];
        let surf = BSplineSurface::new(pts, 2, 2, knots_u, knots_v, 1, 1);

        let p00 = surf.eval(0.0, 0.0);
        assert!((p00 - Point3::new(0.0, 0.0, 0.0)).norm() < 1e-10);

        let p11 = surf.eval(1.0, 1.0);
        assert!((p11 - Point3::new(10.0, 10.0, 0.0)).norm() < 1e-10);

        let mid = surf.eval(0.5, 0.5);
        assert!((mid - Point3::new(5.0, 5.0, 0.0)).norm() < 1e-10);
    }

    #[test]
    fn test_bspline_surface_quadratic() {
        // 3x3 control grid, degree (2,2) = biquadratic
        let pts = vec![
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(5.0, 0.0, 0.0),
            Point3::new(10.0, 0.0, 0.0),
            Point3::new(0.0, 5.0, 0.0),
            Point3::new(5.0, 5.0, 5.0), // center elevated
            Point3::new(10.0, 5.0, 0.0),
            Point3::new(0.0, 10.0, 0.0),
            Point3::new(5.0, 10.0, 0.0),
            Point3::new(10.0, 10.0, 0.0),
        ];
        let knots_u = vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        let knots_v = vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        let surf = BSplineSurface::new(pts, 3, 3, knots_u, knots_v, 2, 2);

        // Corners should interpolate
        let p00 = surf.eval(0.0, 0.0);
        assert!((p00 - Point3::new(0.0, 0.0, 0.0)).norm() < 1e-10);

        let p11 = surf.eval(1.0, 1.0);
        assert!((p11 - Point3::new(10.0, 10.0, 0.0)).norm() < 1e-10);

        // Center should be elevated (z > 0)
        let mid = surf.eval(0.5, 0.5);
        assert!(mid.z > 0.0, "center z should be positive: {}", mid.z);
    }

    #[test]
    fn test_bspline_surface_normal() {
        // Flat surface in XY plane
        let pts = vec![
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(10.0, 0.0, 0.0),
            Point3::new(0.0, 10.0, 0.0),
            Point3::new(10.0, 10.0, 0.0),
        ];
        let knots_u = vec![0.0, 0.0, 1.0, 1.0];
        let knots_v = vec![0.0, 0.0, 1.0, 1.0];
        let surf = BSplineSurface::new(pts, 2, 2, knots_u, knots_v, 1, 1);

        use vcad_kernel_geom::Surface;
        let n = surf.normal(Point2::new(0.5, 0.5));
        // Normal should point along Z
        assert!(
            (n.as_ref().z.abs() - 1.0).abs() < 1e-8,
            "normal z: {}",
            n.as_ref().z
        );
    }

    #[test]
    fn test_bspline_surface_transform() {
        use vcad_kernel_geom::Surface;

        let pts = vec![
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(10.0, 0.0, 0.0),
            Point3::new(0.0, 10.0, 0.0),
            Point3::new(10.0, 10.0, 0.0),
        ];
        let knots_u = vec![0.0, 0.0, 1.0, 1.0];
        let knots_v = vec![0.0, 0.0, 1.0, 1.0];
        let surf = BSplineSurface::new(pts, 2, 2, knots_u, knots_v, 1, 1);

        let t = Transform::translation(100.0, 0.0, 0.0);
        let surf2 = surf.transform(&t);

        let p = surf2.evaluate(Point2::new(0.0, 0.0));
        assert!((p.x - 100.0).abs() < 1e-10);
    }

    // ---- NURBS curve tests ----

    #[test]
    fn test_nurbs_circle() {
        // A NURBS circle should have all points at radius distance from center
        let circle = NurbsCurve::circle(Point3::origin(), 5.0);

        // Sample points around the circle
        let (t_min, t_max) = circle.parameter_domain();
        for i in 0..=20 {
            let t = t_min + (t_max - t_min) * i as f64 / 20.0;
            let p = circle.eval(t);
            let r = (p.x * p.x + p.y * p.y).sqrt();
            assert!(
                (r - 5.0).abs() < 1e-8,
                "radius at t={}: {} (expected 5.0)",
                t,
                r
            );
            assert!(p.z.abs() < 1e-10);
        }
    }

    #[test]
    fn test_nurbs_circle_specific_points() {
        let circle = NurbsCurve::circle(Point3::origin(), 10.0);

        // t=0: should be at (10, 0, 0)
        let p0 = circle.eval(0.0);
        assert!((p0.x - 10.0).abs() < 1e-8);
        assert!(p0.y.abs() < 1e-8);

        // t=0.25: should be at (0, 10, 0)
        let p25 = circle.eval(0.25);
        assert!(p25.x.abs() < 1e-8, "x at 0.25: {}", p25.x);
        assert!((p25.y - 10.0).abs() < 1e-8, "y at 0.25: {}", p25.y);

        // t=0.5: should be at (-10, 0, 0)
        let p50 = circle.eval(0.5);
        assert!((p50.x + 10.0).abs() < 1e-8, "x at 0.5: {}", p50.x);
        assert!(p50.y.abs() < 1e-8, "y at 0.5: {}", p50.y);
    }

    #[test]
    fn test_nurbs_unit_weights() {
        // NURBS with all weights = 1 should match B-spline
        let pts_bs = vec![
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 2.0, 0.0),
            Point3::new(3.0, 2.0, 0.0),
            Point3::new(4.0, 0.0, 0.0),
        ];
        let knots = vec![0.0, 0.0, 0.0, 0.5, 1.0, 1.0, 1.0];
        let bspline = BSplineCurve::new(pts_bs.clone(), knots.clone(), 2);

        let pts_nurbs: Vec<_> = pts_bs
            .iter()
            .map(|p| WeightedPoint::unweighted(*p))
            .collect();
        let nurbs = NurbsCurve::new(pts_nurbs, knots, 2);

        for i in 0..=10 {
            let t = i as f64 / 10.0;
            let pb = bspline.eval(t);
            let pn = nurbs.eval(t);
            assert!(
                (pb - pn).norm() < 1e-10,
                "mismatch at t={}: bs={:?}, nurbs={:?}",
                t,
                pb,
                pn
            );
        }
    }

    #[test]
    fn test_nurbs_knot_insertion() {
        let circle = NurbsCurve::circle(Point3::origin(), 5.0);

        // Insert a knot at t=0.125
        let refined = circle.insert_knot(0.125);
        assert_eq!(
            refined.control_points.len(),
            circle.control_points.len() + 1
        );

        // Should still be a circle
        let (t_min, t_max) = refined.parameter_domain();
        for i in 0..=20 {
            let t = t_min + (t_max - t_min) * i as f64 / 20.0;
            let p = refined.eval(t);
            let r = (p.x * p.x + p.y * p.y).sqrt();
            assert!(
                (r - 5.0).abs() < 1e-6,
                "radius at t={}: {} (expected 5.0)",
                t,
                r
            );
        }
    }

    // ---- NURBS surface tests ----

    #[test]
    fn test_nurbs_surface_flat() {
        // NURBS surface with unit weights = B-spline surface
        let pts: Vec<_> = vec![
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(10.0, 0.0, 0.0),
            Point3::new(0.0, 10.0, 0.0),
            Point3::new(10.0, 10.0, 0.0),
        ]
        .into_iter()
        .map(WeightedPoint::unweighted)
        .collect();

        let surf = NurbsSurface::new(
            pts,
            2,
            2,
            vec![0.0, 0.0, 1.0, 1.0],
            vec![0.0, 0.0, 1.0, 1.0],
            1,
            1,
        );

        let mid = surf.eval(0.5, 0.5);
        assert!((mid - Point3::new(5.0, 5.0, 0.0)).norm() < 1e-10);
    }

    #[test]
    fn test_nurbs_surface_trait() {
        use vcad_kernel_geom::Surface;

        let pts: Vec<_> = vec![
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(10.0, 0.0, 0.0),
            Point3::new(0.0, 10.0, 0.0),
            Point3::new(10.0, 10.0, 0.0),
        ]
        .into_iter()
        .map(WeightedPoint::unweighted)
        .collect();

        let surf = NurbsSurface::new(
            pts,
            2,
            2,
            vec![0.0, 0.0, 1.0, 1.0],
            vec![0.0, 0.0, 1.0, 1.0],
            1,
            1,
        );

        let p = surf.evaluate(Point2::new(0.5, 0.5));
        assert!((p.x - 5.0).abs() < 1e-10);

        let n = surf.normal(Point2::new(0.5, 0.5));
        assert!((n.as_ref().z.abs() - 1.0).abs() < 1e-6);
    }

    // ---- Knot utilities tests ----

    #[test]
    fn test_find_span() {
        let knots = vec![0.0, 0.0, 0.0, 0.5, 1.0, 1.0, 1.0];
        // 4 control points, degree 2, n=3
        assert_eq!(find_span(&knots, 3, 2, 0.0), 2);
        assert_eq!(find_span(&knots, 3, 2, 0.25), 2);
        assert_eq!(find_span(&knots, 3, 2, 0.5), 3);
        assert_eq!(find_span(&knots, 3, 2, 1.0), 3); // end of domain
    }

    #[test]
    fn test_basis_partition_of_unity() {
        // Basis functions should sum to 1 at any parameter value
        let knots = vec![0.0, 0.0, 0.0, 0.25, 0.5, 0.75, 1.0, 1.0, 1.0];
        let degree = 2;
        let n = 5; // 6 control points, n = last index

        for i in 0..=20 {
            let t = i as f64 / 20.0;
            let t = t.clamp(knots[degree], knots[n + 1]);
            let span = find_span(&knots, n, degree, t);
            let basis = basis_functions(&knots, span, degree, t);
            let sum: f64 = basis.iter().sum();
            assert!(
                (sum - 1.0).abs() < 1e-10,
                "partition of unity failed at t={}: sum={}",
                t,
                sum
            );
        }
    }
}
