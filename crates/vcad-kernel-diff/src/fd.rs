//! Central-difference oracle for the seam.
//!
//! The oracle rebuilds the model at θ ± h and re-evaluates the **same
//! frozen plan** — identical connectivity, identical sample pattern,
//! identical vertex ordering — so node `i` corresponds to node `i` and
//! `(x(θ+h) − x(θ−h)) / 2h` is a legitimate per-node derivative estimate.
//! If either rebuild changes topology, the signature check inside
//! `evaluate_plan` errors instead of producing garbage.

use vcad_kernel_math::Vec3;
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_tessellate::frozen::{evaluate_plan, FrozenError, FrozenPlan};

/// Result of comparing analytic velocities against the FD oracle.
#[derive(Debug, Clone, Copy)]
pub struct FdComparison {
    /// Maximum per-node relative error (see [`compare_velocities`] for the
    /// normalization convention).
    pub max_rel_err: f64,
    /// Maximum per-node absolute error (mm per unit θ).
    pub max_abs_err: f64,
    /// Node index attaining `max_rel_err`.
    pub worst_node: usize,
    /// Largest velocity magnitude across both inputs (the global scale).
    pub velocity_scale: f64,
}

/// Central-difference node velocities: rebuild at θ ± h, evaluate the same
/// frozen plan, difference node-wise.
pub fn fd_velocities(
    build: impl Fn(f64) -> BRepSolid,
    theta: f64,
    h: f64,
    plan: &FrozenPlan,
) -> Result<Vec<Vec3>, FrozenError> {
    let plus = evaluate_plan(&build(theta + h), plan)?;
    let minus = evaluate_plan(&build(theta - h), plan)?;
    Ok(plus
        .positions
        .iter()
        .zip(&minus.positions)
        .map(|(p, m)| (*p - *m) / (2.0 * h))
        .collect())
}

/// Central-difference derivative of the frozen-mesh volume.
pub fn fd_volume_derivative(
    build: impl Fn(f64) -> BRepSolid,
    theta: f64,
    h: f64,
    plan: &FrozenPlan,
) -> Result<f64, FrozenError> {
    let plus = evaluate_plan(&build(theta + h), plan)?;
    let minus = evaluate_plan(&build(theta - h), plan)?;
    Ok((plus.volume() - minus.volume()) / (2.0 * h))
}

/// Compare analytic and FD velocities node-wise.
///
/// The per-node relative error divides by
/// `max(|analytic_i|, |fd_i|, 0.01 · velocity_scale)`: nodes moving slower
/// than 1% of the fastest node are judged against that 1% floor, so the FD
/// roundoff noise on a genuinely stationary node (≈ machine-ε · coordinate
/// / 2h) is not amplified into a spurious relative error.
pub fn compare_velocities(analytic: &[Vec3], fd: &[Vec3]) -> FdComparison {
    assert_eq!(analytic.len(), fd.len(), "node count mismatch");
    let scale = analytic
        .iter()
        .chain(fd)
        .map(|v| v.norm())
        .fold(0.0_f64, f64::max);
    let floor = (0.01 * scale).max(f64::MIN_POSITIVE);

    let mut cmp = FdComparison {
        max_rel_err: 0.0,
        max_abs_err: 0.0,
        worst_node: 0,
        velocity_scale: scale,
    };
    for (i, (a, f)) in analytic.iter().zip(fd).enumerate() {
        let abs = (*a - *f).norm();
        let rel = abs / a.norm().max(f.norm()).max(floor);
        if rel > cmp.max_rel_err {
            cmp.max_rel_err = rel;
            cmp.worst_node = i;
        }
        cmp.max_abs_err = cmp.max_abs_err.max(abs);
    }
    cmp
}
