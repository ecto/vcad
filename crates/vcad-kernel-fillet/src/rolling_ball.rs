//! NURBS rolling ball fillet blend.
//!
//! Computes a B-spline blend surface between two general surfaces by rolling
//! a ball of the given radius along the intersection edge.

use vcad_kernel_geom::Surface;
use vcad_kernel_math::{Point3, Vec3};
use vcad_kernel_nurbs::BSplineSurface;

use crate::closest_point::closest_point_uv;

/// Compute a rolling ball fillet blend surface between two general surfaces.
///
/// The algorithm:
/// 1. Traces the intersection edge curve using alternating projection
/// 2. At each sample, finds the ball center equidistant from both surfaces
/// 3. Records contact points on each surface
/// 4. Fits a B-spline surface through the contact point grid via interpolation
///
/// Returns `None` if the rolling ball cannot be computed.
pub fn rolling_ball_blend(
    surface_a: &dyn Surface,
    surface_b: &dyn Surface,
    edge_start: Point3,
    edge_end: Point3,
    radius: f64,
    num_samples_along: usize,
    num_samples_across: usize,
) -> Option<BSplineSurface> {
    if num_samples_along < 2 || num_samples_across < 2 {
        return None;
    }

    let edge_len = (edge_end - edge_start).norm();
    if edge_len < 1e-12 {
        return None;
    }

    // Trace edge curve using alternating projection
    let edge_points = trace_edge_curve(
        surface_a,
        surface_b,
        edge_start,
        edge_end,
        num_samples_along,
    )?;

    // Sample points along the traced edge
    let mut blend_points = Vec::new();

    for edge_point in &edge_points {
        // Find closest points and normals on both surfaces at this edge position
        let uv_a = closest_point_uv(surface_a, edge_point, 1e-6)?;
        let uv_b = closest_point_uv(surface_b, edge_point, 1e-6)?;

        let n_a = surface_a.normal(uv_a);
        let n_b = surface_b.normal(uv_b);

        let bisector = (*n_a.as_ref() + *n_b.as_ref()).normalize();
        if bisector.norm() < 1e-12 {
            return None;
        }

        let ball_center = refine_ball_center(surface_a, surface_b, edge_point, &bisector, radius)?;

        // Contact points: closest points on each surface to the ball center
        let uv_ca = closest_point_uv(surface_a, &ball_center, 1e-6)?;
        let uv_cb = closest_point_uv(surface_b, &ball_center, 1e-6)?;
        let contact_a = surface_a.evaluate(uv_ca);
        let contact_b = surface_b.evaluate(uv_cb);

        // Generate blend points across the fillet
        for j in 0..num_samples_across {
            let s = j as f64 / (num_samples_across - 1) as f64;
            let dir_a = (contact_a - ball_center).normalize();
            let dir_b = (contact_b - ball_center).normalize();
            let blend_dir = slerp(&dir_a, &dir_b, s);
            let blend_pt = ball_center + radius * blend_dir;
            blend_points.push(blend_pt);
        }
    }

    // Interpolate a B-spline surface through the blend points
    interpolate_bspline_surface(&blend_points, num_samples_along, num_samples_across)
}

/// Trace the intersection edge curve using alternating projection.
///
/// Instead of assuming the edge is a straight line, we project each initial
/// guess onto both surfaces alternately to find points near the intersection.
fn trace_edge_curve(
    surface_a: &dyn Surface,
    surface_b: &dyn Surface,
    start: Point3,
    end: Point3,
    num_samples: usize,
) -> Option<Vec<Point3>> {
    let mut points = Vec::with_capacity(num_samples);
    let edge_dir = end - start;

    for i in 0..num_samples {
        let t = i as f64 / (num_samples - 1) as f64;
        let initial = start + t * edge_dir;

        // Alternating projection: project onto A, then B, take midpoint, repeat
        let mut pt = initial;
        for _ in 0..10 {
            let uv_a = closest_point_uv(surface_a, &pt, 1e-8)?;
            let pt_a = surface_a.evaluate(uv_a);

            let uv_b = closest_point_uv(surface_b, &pt_a, 1e-8)?;
            let pt_b = surface_b.evaluate(uv_b);

            let midpoint = Point3::from((pt_a.to_vec() + pt_b.to_vec()) * 0.5);

            // Check convergence: both projections are close
            let dist = (pt_a - pt_b).norm();
            pt = midpoint;
            if dist < 1e-6 {
                break;
            }
        }
        points.push(pt);
    }

    Some(points)
}

/// Refine the rolling ball center using Newton-like iteration with trust region.
fn refine_ball_center(
    surface_a: &dyn Surface,
    surface_b: &dyn Surface,
    initial_pos: &Point3,
    direction: &Vec3,
    radius: f64,
) -> Option<Point3> {
    let mut center = *initial_pos + radius * direction;
    let max_step = radius * 0.5;

    for _ in 0..20 {
        // Find closest points on both surfaces
        let uv_a = closest_point_uv(surface_a, &center, 1e-6)?;
        let uv_b = closest_point_uv(surface_b, &center, 1e-6)?;
        let pt_a = surface_a.evaluate(uv_a);
        let pt_b = surface_b.evaluate(uv_b);

        let dist_a = (center - pt_a).norm();
        let dist_b = (center - pt_b).norm();

        // Check convergence
        if (dist_a - radius).abs() < 1e-6 && (dist_b - radius).abs() < 1e-6 {
            return Some(center);
        }

        // Normals from surfaces to center
        let n_a = if dist_a > 1e-12 {
            (center - pt_a) / dist_a
        } else {
            *surface_a.normal(uv_a).as_ref()
        };
        let n_b = if dist_b > 1e-12 {
            (center - pt_b) / dist_b
        } else {
            *surface_b.normal(uv_b).as_ref()
        };

        let err_a = dist_a - radius;
        let err_b = dist_b - radius;

        // Bisector plane constraint: center should lie on the bisector of the two normals
        let bisector = (n_a + n_b).normalize();
        let bisector_err = if bisector.norm() > 1e-12 {
            let mid_pt = Point3::from((pt_a.to_vec() + pt_b.to_vec()) * 0.5);
            let d = center - mid_pt;
            // Project error onto bisector direction
            let along_bisector = d.dot(&bisector);
            let target_along =
                ((radius * radius - ((pt_a - pt_b).norm() * 0.5).powi(2)).max(0.0)).sqrt();
            (along_bisector - target_along) * bisector
        } else {
            Vec3::zeros()
        };

        // Combined correction
        let mut correction = -0.5 * (err_a * n_a + err_b * n_b) - 0.3 * bisector_err;

        // Trust region: clamp step magnitude
        let step_mag = correction.norm();
        if step_mag > max_step {
            correction *= max_step / step_mag;
        }

        center += correction;
    }

    // Return best estimate even if not fully converged
    Some(center)
}

/// Spherical linear interpolation between two unit vectors.
pub(crate) fn slerp(a: &Vec3, b: &Vec3, t: f64) -> Vec3 {
    let dot = a.dot(b).clamp(-1.0, 1.0);
    let theta = dot.acos();

    if theta.abs() < 1e-10 {
        return ((1.0 - t) * a + t * b).normalize();
    }

    let sin_theta = theta.sin();
    let wa = ((1.0 - t) * theta).sin() / sin_theta;
    let wb = (t * theta).sin() / sin_theta;
    (wa * a + wb * b).normalize()
}

/// Interpolate a B-spline surface through a grid of points.
///
/// Uses chord-length parameterization and solves for control points
/// so that the surface passes exactly through all data points.
fn interpolate_bspline_surface(
    points: &[Point3],
    n_along: usize,
    n_across: usize,
) -> Option<BSplineSurface> {
    if points.len() != n_along * n_across {
        return None;
    }

    let degree_u = (n_along - 1).min(3);
    let degree_v = (n_across - 1).min(3);

    // If the grid is small enough that control points == data points at degree,
    // use chord-length parameterization with averaging knots and solve the
    // interpolation system.

    // Compute chord-length parameters along u (rows)
    let params_u = chord_length_params(points, n_along, n_across, true);
    let params_v = chord_length_params(points, n_along, n_across, false);

    // Generate knot vectors via averaging
    let knots_u = averaging_knots(&params_u, n_along, degree_u);
    let knots_v = averaging_knots(&params_v, n_across, degree_v);

    // Solve interpolation: first along u-direction (rows), then along v-direction (columns)
    // Step 1: For each row j, solve for intermediate control points R[i][j]
    let mut intermediate = vec![Point3::origin(); n_along * n_across];
    for j in 0..n_across {
        // Extract row j data points
        let row_points: Vec<Point3> = (0..n_along).map(|i| points[i * n_across + j]).collect();
        let row_ctrl = solve_1d_interpolation(&row_points, &params_u, &knots_u, degree_u)?;
        for (i, pt) in row_ctrl.into_iter().enumerate() {
            intermediate[i * n_across + j] = pt;
        }
    }

    // Step 2: For each column i, solve for final control points P[i][j]
    let mut control_points = vec![Point3::origin(); n_along * n_across];
    for i in 0..n_along {
        let col_points: Vec<Point3> = (0..n_across)
            .map(|j| intermediate[i * n_across + j])
            .collect();
        let col_ctrl = solve_1d_interpolation(&col_points, &params_v, &knots_v, degree_v)?;
        for (j, pt) in col_ctrl.into_iter().enumerate() {
            control_points[i * n_across + j] = pt;
        }
    }

    // Apply G1 tangency enforcement at v=0 and v=1 boundaries
    enforce_g1_tangency(
        &mut control_points,
        n_along,
        n_across,
        degree_v,
        &knots_v,
        points,
    );

    Some(BSplineSurface::new(
        control_points,
        n_along,
        n_across,
        knots_u,
        knots_v,
        degree_u,
        degree_v,
    ))
}

/// Compute chord-length parameters for either rows (along_u=true) or columns (along_u=false).
fn chord_length_params(
    points: &[Point3],
    n_along: usize,
    n_across: usize,
    along_u: bool,
) -> Vec<f64> {
    let n = if along_u { n_along } else { n_across };
    let m = if along_u { n_across } else { n_along };

    // Average chord lengths across all rows/columns
    let mut avg_params = vec![0.0; n];

    for k in 0..m {
        let mut cumulative = vec![0.0; n];
        for i in 1..n {
            let (idx_prev, idx_curr) = if along_u {
                ((i - 1) * n_across + k, i * n_across + k)
            } else {
                (k * n_across + (i - 1), k * n_across + i)
            };
            cumulative[i] = cumulative[i - 1] + (points[idx_curr] - points[idx_prev]).norm();
        }
        let total = cumulative[n - 1];
        if total > 1e-15 {
            for (p, c) in avg_params.iter_mut().zip(cumulative.iter()) {
                *p += c / total;
            }
        } else {
            for (i, p) in avg_params.iter_mut().enumerate() {
                *p += i as f64 / (n - 1) as f64;
            }
        }
    }

    for p in &mut avg_params {
        *p /= m as f64;
    }
    // Force endpoints
    avg_params[0] = 0.0;
    avg_params[n - 1] = 1.0;

    avg_params
}

/// Generate knot vector using the averaging method.
fn averaging_knots(params: &[f64], n: usize, degree: usize) -> Vec<f64> {
    let m = n + degree + 1;
    let mut knots = vec![0.0; m];

    // Clamped: first degree+1 knots = 0 (already 0.0 from vec init)
    // Clamped: last degree+1 knots = 1
    for knot in knots.iter_mut().skip(n) {
        *knot = 1.0;
    }
    // Interior knots: averaging
    for j in 1..n - degree {
        let sum: f64 = params[j..j + degree].iter().sum();
        knots[j + degree] = sum / degree as f64;
    }

    knots
}

/// Solve 1D B-spline interpolation problem.
///
/// Given data points Q[i] at parameters t[i], find control points P[i] such that
/// C(t[i]) = Q[i] where C is the B-spline curve defined by the control points.
fn solve_1d_interpolation(
    data_points: &[Point3],
    params: &[f64],
    knots: &[f64],
    degree: usize,
) -> Option<Vec<Point3>> {
    let n = data_points.len();
    if n <= 1 {
        return Some(data_points.to_vec());
    }
    if n == 2 {
        // Linear interpolation — control points are data points
        return Some(data_points.to_vec());
    }

    // Build collocation matrix N[i][j] = B_j(t_i)
    let mut matrix = vec![vec![0.0; n]; n];
    for (i, row) in matrix.iter_mut().enumerate() {
        let t = params[i].clamp(knots[0], knots[knots.len() - 1] - 1e-15);
        for (j, cell) in row.iter_mut().enumerate() {
            *cell = basis_function(knots, j, degree, t);
        }
    }

    // Solve for each coordinate independently using Gaussian elimination
    let mut ctrl_x = vec![0.0; n];
    let mut ctrl_y = vec![0.0; n];
    let mut ctrl_z = vec![0.0; n];

    let rhs_x: Vec<f64> = data_points.iter().map(|p| p.x).collect();
    let rhs_y: Vec<f64> = data_points.iter().map(|p| p.y).collect();
    let rhs_z: Vec<f64> = data_points.iter().map(|p| p.z).collect();

    gauss_solve(&matrix, &rhs_x, &mut ctrl_x)?;
    gauss_solve(&matrix, &rhs_y, &mut ctrl_y)?;
    gauss_solve(&matrix, &rhs_z, &mut ctrl_z)?;

    let control_points: Vec<Point3> = (0..n)
        .map(|i| Point3::new(ctrl_x[i], ctrl_y[i], ctrl_z[i]))
        .collect();

    Some(control_points)
}

/// Evaluate B-spline basis function B_{i,p}(t) using Cox-de Boor recursion.
fn basis_function(knots: &[f64], i: usize, p: usize, t: f64) -> f64 {
    if p == 0 {
        return if t >= knots[i] && t < knots[i + 1] {
            1.0
        } else if i + 1 == knots.len() - 1 && (t - knots[i + 1]).abs() < 1e-15 {
            // Special case for last knot
            1.0
        } else {
            0.0
        };
    }

    let mut result = 0.0;

    let denom1 = knots[i + p] - knots[i];
    if denom1.abs() > 1e-15 {
        result += (t - knots[i]) / denom1 * basis_function(knots, i, p - 1, t);
    }

    let denom2 = knots[i + p + 1] - knots[i + 1];
    if denom2.abs() > 1e-15 {
        result += (knots[i + p + 1] - t) / denom2 * basis_function(knots, i + 1, p - 1, t);
    }

    result
}

/// Gaussian elimination with partial pivoting.
#[allow(clippy::needless_range_loop)]
fn gauss_solve(matrix: &[Vec<f64>], rhs: &[f64], result: &mut [f64]) -> Option<()> {
    let n = rhs.len();
    let mut aug: Vec<Vec<f64>> = matrix
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let mut r = row.clone();
            r.push(rhs[i]);
            r
        })
        .collect();

    // Forward elimination with partial pivoting
    for col in 0..n {
        // Find pivot
        let mut max_val = aug[col][col].abs();
        let mut max_row = col;
        for row in col + 1..n {
            if aug[row][col].abs() > max_val {
                max_val = aug[row][col].abs();
                max_row = row;
            }
        }
        if max_val < 1e-15 {
            return None; // Singular matrix
        }
        if max_row != col {
            aug.swap(col, max_row);
        }

        let pivot = aug[col][col];
        for row in col + 1..n {
            let factor = aug[row][col] / pivot;
            for j in col..=n {
                aug[row][j] -= factor * aug[col][j];
            }
        }
    }

    // Back substitution
    for i in (0..n).rev() {
        let mut sum = aug[i][n];
        for j in i + 1..n {
            sum -= aug[i][j] * result[j];
        }
        result[i] = sum / aug[i][i];
    }

    Some(())
}

/// Enforce G1 tangency at blend boundaries (v=0 and v=1).
///
/// Adjusts the second row (for v=0 boundary) and second-to-last row (for v=1 boundary)
/// of control points so that the cross-derivative at the boundary matches the
/// surface normal direction.
fn enforce_g1_tangency(
    control_points: &mut [Point3],
    n_along: usize,
    n_across: usize,
    degree_v: usize,
    knots_v: &[f64],
    data_points: &[Point3],
) {
    if degree_v < 2 || n_across < 3 {
        return;
    }

    // Delta knot for the first span
    let delta_v0 = knots_v[degree_v + 1] - knots_v[degree_v];
    // Delta knot for the last span
    let delta_v_last =
        knots_v[knots_v.len() - degree_v - 1] - knots_v[knots_v.len() - degree_v - 2];

    if delta_v0.abs() < 1e-15 || delta_v_last.abs() < 1e-15 {
        return;
    }

    for i in 0..n_along {
        // At v=0 boundary: tangent direction from data points
        let p0 = data_points[i * n_across];
        let p1 = data_points[i * n_across + 1];
        let tangent_v0 = p1 - p0;
        let tangent_len = tangent_v0.norm();
        if tangent_len > 1e-12 {
            let tangent_dir = tangent_v0 / tangent_len;
            // P[i][1] = P[i][0] + (delta_v / degree) * scaled_tangent
            let boundary_pt = control_points[i * n_across];
            let scale = tangent_len * (delta_v0 / degree_v as f64) * (n_across as f64 - 1.0);
            control_points[i * n_across + 1] = boundary_pt + scale * tangent_dir;
        }

        // At v=1 boundary: tangent direction from data points
        let pn_1 = data_points[i * n_across + n_across - 2];
        let pn = data_points[i * n_across + n_across - 1];
        let tangent_v1 = pn - pn_1;
        let tangent_len = tangent_v1.norm();
        if tangent_len > 1e-12 {
            let tangent_dir = tangent_v1 / tangent_len;
            let boundary_pt = control_points[i * n_across + n_across - 1];
            let scale = tangent_len * (delta_v_last / degree_v as f64) * (n_across as f64 - 1.0);
            control_points[i * n_across + n_across - 2] = boundary_pt - scale * tangent_dir;
        }
    }
}
