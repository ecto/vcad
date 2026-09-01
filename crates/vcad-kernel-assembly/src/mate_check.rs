//! Checking [`vcad_ir::Mate`] assertions against a [`PosedAssembly`].
//!
//! Every check reads the poses and reports a measured value next to the
//! asserted one. Nothing here moves a part: a failing mate is a modelling
//! error to fix in the document, not a residual for a solver to drive to zero.

use vcad_ir::{Mate, MateKind, Vec3};

use crate::pose::{Affine, PosedAssembly, PosedPart};

/// Outcome of checking one [`Mate`].
#[derive(Debug, Clone, PartialEq)]
pub struct MateCheck {
    /// The mate's id.
    pub id: String,
    /// Kind label (`"coaxial"`, `"planar-offset"`, `"pattern-phase"`).
    pub kind: &'static str,
    /// Instance ids, in the mate's order.
    pub instances: (String, String),
    /// Whether the assertion holds.
    pub pass: bool,
    /// What the check measured from the poses.
    pub measured: f64,
    /// What the mate asserted.
    pub expected: f64,
    /// Tolerance the comparison used.
    pub tolerance: f64,
    /// Unit of `measured` / `expected` / `tolerance` (`"mm"` or `"deg"`).
    pub unit: &'static str,
    /// Human-readable explanation — always populated, so a passing check is
    /// as legible in a report as a failing one.
    pub detail: String,
}

impl MateCheck {
    /// One-line report form.
    pub fn summary(&self) -> String {
        format!(
            "{} {} [{}] {} — {}",
            if self.pass { "PASS" } else { "FAIL" },
            self.kind,
            self.id,
            format_args!("{} ↔ {}", self.instances.0, self.instances.1),
            self.detail
        )
    }
}

/// Why a mate could not be checked at all (as distinct from failing).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MateError {
    /// The mate names an instance that is not in the assembly.
    UnknownInstance {
        /// The mate's id.
        mate_id: String,
        /// The instance id that could not be resolved.
        instance_id: String,
    },
    /// A mate parameter is unusable (zero-length axis, `n_fold` of zero).
    BadParameter {
        /// The mate's id.
        mate_id: String,
        /// What is wrong.
        reason: String,
    },
}

impl std::fmt::Display for MateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MateError::UnknownInstance {
                mate_id,
                instance_id,
            } => write!(
                f,
                "mate {mate_id:?}: no instance {instance_id:?} in assembly"
            ),
            MateError::BadParameter { mate_id, reason } => {
                write!(f, "mate {mate_id:?}: {reason}")
            }
        }
    }
}

impl std::error::Error for MateError {}

/// Check every mate in `mates` against `posed`.
///
/// Returns one [`MateCheck`] per mate, in order. An unresolvable mate is an
/// error rather than a failing check — the document is malformed, not wrong.
pub fn check_mates(posed: &PosedAssembly, mates: &[Mate]) -> Result<Vec<MateCheck>, MateError> {
    mates.iter().map(|m| check_mate(posed, m)).collect()
}

/// Check a single mate.
pub fn check_mate(posed: &PosedAssembly, mate: &Mate) -> Result<MateCheck, MateError> {
    let a = lookup(posed, mate, &mate.instance_a)?;
    let b = lookup(posed, mate, &mate.instance_b)?;
    match &mate.kind {
        MateKind::Coaxial {
            axis,
            tolerance_mm,
            tolerance_deg,
        } => coaxial(mate, a, b, *axis, *tolerance_mm, *tolerance_deg),
        MateKind::PlanarOffset {
            axis,
            offset,
            tolerance_mm,
        } => planar_offset(mate, a, b, *axis, *offset, *tolerance_mm),
        MateKind::PatternPhase {
            n_fold,
            axis,
            phase_a_deg,
            phase_b_deg,
            expected_clock_deg,
            tolerance_deg,
        } => pattern_phase(
            mate,
            a,
            b,
            *n_fold,
            *axis,
            *phase_a_deg,
            *phase_b_deg,
            *expected_clock_deg,
            *tolerance_deg,
        ),
    }
}

fn lookup<'a>(posed: &'a PosedAssembly, mate: &Mate, id: &str) -> Result<&'a PosedPart, MateError> {
    posed.get(id).ok_or_else(|| MateError::UnknownInstance {
        mate_id: mate.id.clone(),
        instance_id: id.to_string(),
    })
}

// ---------------------------------------------------------------------------
// Small vector helpers — three components, no dependency worth taking.
// ---------------------------------------------------------------------------

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn norm(a: [f64; 3]) -> f64 {
    dot(a, a).sqrt()
}

fn normalize(mate: &Mate, v: Vec3) -> Result<[f64; 3], MateError> {
    let a = [v.x, v.y, v.z];
    let n = norm(a);
    if n < 1e-12 {
        return Err(MateError::BadParameter {
            mate_id: mate.id.clone(),
            reason: "axis has zero length".into(),
        });
    }
    Ok([a[0] / n, a[1] / n, a[2] / n])
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// Fold an angle into `(-180, 180]`.
fn wrap_signed(mut deg: f64) -> f64 {
    deg %= 360.0;
    if deg > 180.0 {
        deg -= 360.0;
    } else if deg <= -180.0 {
        deg += 360.0;
    }
    deg
}

/// Fold `deg` into `(-period/2, period/2]`.
fn wrap_to_period(deg: f64, period: f64) -> f64 {
    let mut r = deg % period;
    if r > period / 2.0 {
        r -= period;
    } else if r <= -period / 2.0 {
        r += period;
    }
    r
}

// ---------------------------------------------------------------------------
// Coaxial
// ---------------------------------------------------------------------------

fn coaxial(
    mate: &Mate,
    a: &PosedPart,
    b: &PosedPart,
    axis: Vec3,
    tol_mm: f64,
    tol_deg: f64,
) -> Result<MateCheck, MateError> {
    let local = normalize(mate, axis)?;
    let da = unit_direction(mate, &a.transform, local)?;
    let db = unit_direction(mate, &b.transform, local)?;

    // Antiparallel is still coaxial — a flipped part shares the same line.
    let cos = dot(da, db).clamp(-1.0, 1.0);
    let angle = cos.abs().acos().to_degrees();

    // Perpendicular distance between the two axis lines through the posed
    // origins. Parallel lines fall back to the rejection of the offset.
    let pa = a.transform.point([0.0; 3]);
    let pb = b.transform.point([0.0; 3]);
    let w = sub(pb, pa);
    let n = cross(da, db);
    let dist = if norm(n) < 1e-9 {
        let along = dot(w, da);
        norm(sub(w, [da[0] * along, da[1] * along, da[2] * along]))
    } else {
        (dot(w, n) / norm(n)).abs()
    };

    let angle_ok = angle <= tol_deg;
    let dist_ok = dist <= tol_mm;
    Ok(MateCheck {
        id: mate.id.clone(),
        kind: mate.kind.label(),
        instances: (mate.instance_a.clone(), mate.instance_b.clone()),
        pass: angle_ok && dist_ok,
        measured: dist,
        expected: 0.0,
        tolerance: tol_mm,
        unit: "mm",
        detail: format!(
            "axis offset {dist:.4} mm (tol {tol_mm:.4}), axis skew {angle:.4}° (tol {tol_deg:.4}°)"
        ),
    })
}

fn unit_direction(mate: &Mate, t: &Affine, local: [f64; 3]) -> Result<[f64; 3], MateError> {
    let d = t.direction(local);
    let n = norm(d);
    if n < 1e-12 {
        return Err(MateError::BadParameter {
            mate_id: mate.id.clone(),
            reason: "instance transform collapses the mate axis to zero length".into(),
        });
    }
    Ok([d[0] / n, d[1] / n, d[2] / n])
}

// ---------------------------------------------------------------------------
// Planar offset
// ---------------------------------------------------------------------------

fn planar_offset(
    mate: &Mate,
    a: &PosedPart,
    b: &PosedPart,
    axis: Vec3,
    offset: f64,
    tol_mm: f64,
) -> Result<MateCheck, MateError> {
    let n = normalize(mate, axis)?;
    let pa = a.transform.point([0.0; 3]);
    let pb = b.transform.point([0.0; 3]);
    let measured = dot(sub(pb, pa), n);
    let err = measured - offset;
    Ok(MateCheck {
        id: mate.id.clone(),
        kind: mate.kind.label(),
        instances: (mate.instance_a.clone(), mate.instance_b.clone()),
        pass: err.abs() <= tol_mm,
        measured,
        expected: offset,
        tolerance: tol_mm,
        unit: "mm",
        detail: format!(
            "offset {measured:.4} mm along axis, expected {offset:.4} mm \
             (error {err:+.4}, tol {tol_mm:.4})"
        ),
    })
}

// ---------------------------------------------------------------------------
// Pattern phase — the 10-pole flip-and-clock check
// ---------------------------------------------------------------------------

/// Angular position, about `axis_world`, of the part's pattern reference
/// direction once posed.
///
/// The reference direction is local `+X` rotated by `phase_deg` about the
/// local pattern axis; the result is the angle of its posed image measured in
/// the plane perpendicular to `axis_world`, in a frame spanned by `u` and `v`.
fn posed_phase(
    mate: &Mate,
    part: &PosedPart,
    local_axis: [f64; 3],
    phase_deg: f64,
    u: [f64; 3],
    v: [f64; 3],
) -> Result<f64, MateError> {
    // Reference direction in the part's own frame: +X spun by `phase_deg`
    // about the local pattern axis (Rodrigues).
    let r = seed_direction(mate, local_axis)?;
    let t = phase_deg.to_radians();
    let (c, s) = (t.cos(), t.sin());
    let k = local_axis;
    let kxr = cross(k, r);
    let kdr = dot(k, r);
    let local_ref = [
        r[0] * c + kxr[0] * s + k[0] * kdr * (1.0 - c),
        r[1] * c + kxr[1] * s + k[1] * kdr * (1.0 - c),
        r[2] * c + kxr[2] * s + k[2] * kdr * (1.0 - c),
    ];

    let world = part.transform.direction(local_ref);
    let (x, y) = (dot(world, u), dot(world, v));
    if x.hypot(y) < 1e-9 {
        return Err(MateError::BadParameter {
            mate_id: mate.id.clone(),
            reason: "pattern reference direction is parallel to the pattern axis once posed".into(),
        });
    }
    Ok(y.atan2(x).to_degrees())
}

/// A unit vector perpendicular to `axis`, used as the pattern's local seed
/// direction. Local `+X` unless the axis is (nearly) `+X` itself.
fn seed_direction(mate: &Mate, axis: [f64; 3]) -> Result<[f64; 3], MateError> {
    let candidate = if axis[0].abs() > 0.9 {
        [0.0, 1.0, 0.0]
    } else {
        [1.0, 0.0, 0.0]
    };
    let along = dot(candidate, axis);
    let perp = [
        candidate[0] - axis[0] * along,
        candidate[1] - axis[1] * along,
        candidate[2] - axis[2] * along,
    ];
    let n = norm(perp);
    if n < 1e-9 {
        return Err(MateError::BadParameter {
            mate_id: mate.id.clone(),
            reason: "cannot build a reference direction perpendicular to the pattern axis".into(),
        });
    }
    Ok([perp[0] / n, perp[1] / n, perp[2] / n])
}

#[allow(clippy::too_many_arguments)]
fn pattern_phase(
    mate: &Mate,
    a: &PosedPart,
    b: &PosedPart,
    n_fold: u32,
    axis: Vec3,
    phase_a_deg: f64,
    phase_b_deg: f64,
    expected_clock_deg: Option<f64>,
    tol_deg: f64,
) -> Result<MateCheck, MateError> {
    if n_fold == 0 {
        return Err(MateError::BadParameter {
            mate_id: mate.id.clone(),
            reason: "n_fold must be at least 1".into(),
        });
    }
    let local_axis = normalize(mate, axis)?;
    let da = unit_direction(mate, &a.transform, local_axis)?;
    let db = unit_direction(mate, &b.transform, local_axis)?;

    // The two parts must at least share an axis line, or "phase" is
    // meaningless. Antiparallel is fine and expected — that IS the flip.
    let skew = dot(da, db).clamp(-1.0, 1.0).abs().acos().to_degrees();
    if skew > 1.0 {
        return Ok(MateCheck {
            id: mate.id.clone(),
            kind: mate.kind.label(),
            instances: (mate.instance_a.clone(), mate.instance_b.clone()),
            pass: false,
            measured: skew,
            expected: 0.0,
            tolerance: tol_deg,
            unit: "deg",
            detail: format!(
                "pattern axes are not collinear once posed ({skew:.4}° apart) — \
                 phase is undefined"
            ),
        });
    }

    // Measure both phases about A's posed axis, in one shared frame.
    let u = seed_direction(mate, da)?;
    let v = cross(da, u);
    let pa = posed_phase(mate, a, local_axis, phase_a_deg, u, v)?;
    let pb = posed_phase(mate, b, local_axis, phase_b_deg, u, v)?;

    // Features of an n-fold pattern sit every `pitch` degrees, so the pattern
    // is invariant under a whole-pitch shift and only the residue matters. A
    // flip reverses the sense of θ, which maps the feature SET onto itself —
    // it never rescues a wrong clocking.
    let pitch = 360.0 / f64::from(n_fold);
    let clock = wrap_signed(pb - pa);
    let residual = wrap_to_period(pb - pa, pitch);
    let mut pass = residual.abs() <= tol_deg;

    let mut detail = format!(
        "n={n_fold}, pitch {pitch:.4}°, relative clocking {clock:.4}° → \
         pole misalignment {residual:+.4}° (tol {tol_deg:.4}°)"
    );
    if let Some(expected) = expected_clock_deg {
        let clock_err = wrap_signed(clock - expected);
        if clock_err.abs() > tol_deg {
            pass = false;
            detail.push_str(&format!(
                "; documented clocking {expected:.4}° disagrees with the poses \
                 by {clock_err:+.4}°"
            ));
        } else {
            detail.push_str(&format!("; matches documented clocking {expected:.4}°"));
        }
    }

    Ok(MateCheck {
        id: mate.id.clone(),
        kind: mate.kind.label(),
        instances: (mate.instance_a.clone(), mate.instance_b.clone()),
        pass,
        measured: residual,
        expected: 0.0,
        tolerance: tol_deg,
        unit: "deg",
        detail,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_to_period_folds_into_half_open_window() {
        // 60° of clocking against a 36° pitch: 60 mod 36 = 24 → −12.
        assert!((wrap_to_period(60.0, 36.0) + 12.0).abs() < 1e-9);
        assert!(wrap_to_period(180.0, 36.0).abs() < 1e-9);
        assert!((wrap_to_period(300.0, 36.0) - 12.0).abs() < 1e-9);
    }
}
