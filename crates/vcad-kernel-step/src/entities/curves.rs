//! Curve entities: lines, circles, B-spline curves, and trimmed curves.

use super::{parse_any_axis_placement, parse_cartesian_point, parse_vector, EntityArgs};
use crate::error::StepError;
use stepperoni::{StepFile, StepValue};
use vcad_kernel_geom::{Circle3d, Curve3d, Line3d};
use vcad_kernel_math::{Dir3, Point3, Vec3};
use vcad_kernel_nurbs::BSplineCurve;

/// Hard caps for B-spline curve parsing (DoS defence, matching surface limits).
const MAX_BSPLINE_CURVE_KNOTS: usize = 100_000;
const MAX_BSPLINE_CURVE_CPS: usize = 50_000;
const MAX_KNOT_MULTIPLICITY: usize = 10_000;

/// A trimmed (bounded) segment of an underlying curve.
#[derive(Debug, Clone)]
pub struct TrimmedStepCurve {
    /// The underlying full curve.
    pub basis: Box<StepCurve>,
    /// Start parameter on the basis curve.
    pub trim1: f64,
    /// End parameter on the basis curve.
    pub trim2: f64,
    /// If false, the sense of the trimmed curve is reversed relative to the basis.
    pub sense_agreement: bool,
}

/// A curve parsed from STEP.
#[derive(Debug, Clone)]
pub enum StepCurve {
    /// A line defined by a point and direction vector.
    Line(Line3d),
    /// A circle defined by center, radius, and orientation.
    Circle(Circle3d),
    /// A B-spline curve with explicit knots.
    BSpline(BSplineCurve),
    /// A trimmed segment of another curve with explicit parameter bounds.
    Trimmed(TrimmedStepCurve),
}

impl StepCurve {
    /// Resolve through any `Trimmed` wrapper to reach the innermost curve.
    pub fn resolve(&self) -> &StepCurve {
        match self {
            StepCurve::Trimmed(tc) => tc.basis.resolve(),
            other => other,
        }
    }

    /// Evaluate the curve at parameter `t`.
    ///
    /// For `Line`, `t` is the signed distance along the direction vector.
    /// For `Circle`, `t` is the angle in radians.
    /// For `BSpline`, `t` is the standard B-spline parameter.
    /// For `Trimmed`, delegates to the basis curve.
    pub fn evaluate(&self, t: f64) -> Point3 {
        match self {
            StepCurve::Line(l) => {
                let dir = if l.direction.norm() > 1e-15 {
                    l.direction.normalize()
                } else {
                    Vec3::x()
                };
                l.origin + dir * t
            }
            StepCurve::Circle(c) => c.evaluate(t),
            StepCurve::BSpline(b) => b.eval(t),
            StepCurve::Trimmed(tc) => tc.basis.evaluate(t),
        }
    }

    /// Return the parametric domain `(t_min, t_max)` for this curve.
    pub fn domain(&self) -> (f64, f64) {
        match self {
            StepCurve::Line(_) => (0.0, f64::MAX),
            StepCurve::Circle(_) => (0.0, std::f64::consts::TAU),
            StepCurve::BSpline(b) => b.parameter_domain(),
            StepCurve::Trimmed(tc) => (tc.trim1, tc.trim2),
        }
    }
}

/// Parse a LINE entity.
///
/// STEP syntax: `LINE(name, point, vector)`
pub fn parse_line(file: &StepFile, id: u64) -> Result<Line3d, StepError> {
    let entity = file.get(id).ok_or(StepError::MissingEntity(id))?;

    match entity.type_name.as_str() {
        "LINE" => {
            let point_id = entity.entity_ref(1)?;
            let vec_id = entity.entity_ref(2)?;

            let origin = parse_cartesian_point(file, point_id)?;
            let direction = parse_vector(file, vec_id)?;

            Ok(Line3d { origin, direction })
        }
        other => Err(StepError::type_mismatch("LINE", other)),
    }
}

/// Parse a CIRCLE entity.
///
/// STEP syntax: `CIRCLE(name, position, radius)`
pub fn parse_circle(file: &StepFile, id: u64) -> Result<Circle3d, StepError> {
    let entity = file.get(id).ok_or(StepError::MissingEntity(id))?;

    match entity.type_name.as_str() {
        "CIRCLE" => {
            let placement_id = entity.entity_ref(1)?;
            let radius = entity.real(2)?;

            let placement = parse_any_axis_placement(file, placement_id)?;

            Ok(Circle3d {
                center: placement.location,
                radius,
                x_dir: placement.x_axis(),
                y_dir: placement.y_axis(),
                normal: placement.z_axis(),
            })
        }
        other => Err(StepError::type_mismatch("CIRCLE", other)),
    }
}

/// Parse a `B_SPLINE_CURVE_WITH_KNOTS` entity.
///
/// STEP syntax:
/// ```text
/// B_SPLINE_CURVE_WITH_KNOTS(name, degree, control_points_list,
///     curve_form, closed, self_intersect,
///     knot_multiplicities, knots, knot_spec)
/// ```
pub fn parse_bspline_curve(file: &StepFile, id: u64) -> Result<BSplineCurve, StepError> {
    let entity = file.get(id).ok_or(StepError::MissingEntity(id))?;

    let (degree, cp_ids, mults, knot_vals) = match entity.type_name.as_str() {
        "B_SPLINE_CURVE_WITH_KNOTS" => {
            let degree = entity.integer(1)? as usize;
            let cp_ids: Vec<u64> = entity.entity_ref_list(2)?;
            let mults = entity.list(6)?.to_vec();
            let knot_vals = entity.list(7)?.to_vec();
            (degree, cp_ids, mults, knot_vals)
        }
        other => {
            // Try complex entity form: look for B_SPLINE_CURVE and B_SPLINE_CURVE_WITH_KNOTS typed args
            let mut base: Option<(usize, Vec<u64>)> = None;
            let mut knots: Option<(Vec<StepValue>, Vec<StepValue>)> = None;

            for arg in &entity.args {
                if let StepValue::Typed { type_name, args } = arg {
                    if type_name == "B_SPLINE_CURVE" && args.len() >= 3 {
                        let deg = args.get(1).and_then(|v| v.as_integer()).unwrap_or(0) as usize;
                        if let Some(StepValue::List(cps)) = args.get(2) {
                            let ids: Vec<u64> =
                                cps.iter().filter_map(|v| v.as_entity_ref()).collect();
                            base = Some((deg, ids));
                        }
                    } else if type_name == "B_SPLINE_CURVE_WITH_KNOTS" && args.len() >= 5 {
                        if let (Some(StepValue::List(m)), Some(StepValue::List(k))) =
                            (args.get(1), args.get(2))
                        {
                            knots = Some((m.clone(), k.clone()));
                        }
                    }
                }
            }

            match (base, knots) {
                (Some((deg, ids)), Some((m, k))) => (deg, ids, m, k),
                _ => {
                    return Err(StepError::UnsupportedEntity(format!(
                        "B_SPLINE_CURVE (unsupported form: {})",
                        other
                    )));
                }
            }
        }
    };

    if cp_ids.len() > MAX_BSPLINE_CURVE_CPS {
        return Err(StepError::UnsupportedEntity(format!(
            "B_SPLINE_CURVE #{id} has {} control points (cap {MAX_BSPLINE_CURVE_CPS})",
            cp_ids.len()
        )));
    }

    let mut control_points = Vec::with_capacity(cp_ids.len());
    for pt_id in &cp_ids {
        control_points.push(parse_cartesian_point(file, *pt_id)?);
    }

    let expanded = expand_curve_knots(&knot_vals, &mults)?;

    let expected = control_points.len() + degree + 1;
    if expanded.len() != expected {
        return Err(StepError::UnsupportedEntity(format!(
            "B_SPLINE_CURVE #{id} knot count mismatch: got {} expected {expected}",
            expanded.len()
        )));
    }
    if !is_non_decreasing(&expanded) {
        return Err(StepError::UnsupportedEntity(format!(
            "B_SPLINE_CURVE #{id} has non-monotonic knot vector"
        )));
    }

    Ok(BSplineCurve::new(control_points, expanded, degree))
}

/// Expand a knot vector by multiplicities (shared with surface parsing).
fn expand_curve_knots(knots: &[StepValue], mults: &[StepValue]) -> Result<Vec<f64>, StepError> {
    let mut result = Vec::new();
    for (knot, mult) in knots.iter().zip(mults.iter()) {
        let k = knot
            .as_real()
            .ok_or_else(|| StepError::parser(None, "invalid knot value"))?;
        let m_raw = mult
            .as_integer()
            .ok_or_else(|| StepError::parser(None, "invalid multiplicity"))?;
        if m_raw < 0 {
            return Err(StepError::parser(None, "negative knot multiplicity"));
        }
        let m = m_raw as usize;
        if m > MAX_KNOT_MULTIPLICITY {
            return Err(StepError::UnsupportedEntity(format!(
                "knot multiplicity {m} exceeds cap {MAX_KNOT_MULTIPLICITY}"
            )));
        }
        if result.len().saturating_add(m) > MAX_BSPLINE_CURVE_KNOTS {
            return Err(StepError::UnsupportedEntity(format!(
                "expanded knot vector exceeds cap {MAX_BSPLINE_CURVE_KNOTS}"
            )));
        }
        for _ in 0..m {
            result.push(k);
        }
    }
    Ok(result)
}

fn is_non_decreasing(xs: &[f64]) -> bool {
    xs.windows(2).all(|w| w[0] <= w[1])
}

/// Extract the first `PARAMETER_VALUE` from a trim_select set (a `List` argument).
///
/// In STEP, trim parameters look like:
/// `((PARAMETER_VALUE(0.0)))`  ← doubly-nested list
/// or
/// `(PARAMETER_VALUE(0.0))`    ← singly-nested list
///
/// We search recursively for the first `PARAMETER_VALUE(real)` typed entry.
fn extract_trim_param(entity: &stepperoni::StepEntity, idx: usize) -> Option<f64> {
    fn search(values: &[StepValue]) -> Option<f64> {
        for item in values {
            match item {
                StepValue::Typed { type_name, args } if type_name == "PARAMETER_VALUE" => {
                    if let Some(v) = args.first().and_then(|a| a.as_real()) {
                        return Some(v);
                    }
                }
                StepValue::List(inner) => {
                    if let Some(v) = search(inner) {
                        return Some(v);
                    }
                }
                StepValue::Real(v) => return Some(*v),
                _ => {}
            }
        }
        None
    }

    let list = entity.args.get(idx)?.as_list()?;
    search(list)
}

/// Parse a `TRIMMED_CURVE` entity.
///
/// STEP syntax:
/// ```text
/// TRIMMED_CURVE(name, basis_curve, trim_1, trim_2, sense_agreement, master_representation)
/// ```
///
/// `trim_1` and `trim_2` are each a set of `trim_select` values; we extract the
/// `PARAMETER_VALUE` when present (preferred over `CARTESIAN_POINT`).
pub fn parse_trimmed_curve(file: &StepFile, id: u64) -> Result<TrimmedStepCurve, StepError> {
    let entity = file.get(id).ok_or(StepError::MissingEntity(id))?;

    match entity.type_name.as_str() {
        "TRIMMED_CURVE" => {
            let basis_id = entity.entity_ref(1)?;
            let trim1 = extract_trim_param(entity, 2).ok_or_else(|| {
                StepError::parser(Some(id), "TRIMMED_CURVE: no PARAMETER_VALUE in trim_1")
            })?;
            let trim2 = extract_trim_param(entity, 3).ok_or_else(|| {
                StepError::parser(Some(id), "TRIMMED_CURVE: no PARAMETER_VALUE in trim_2")
            })?;
            let sense_agreement = entity
                .args
                .get(4)
                .and_then(|v| v.as_enum())
                .map(|e| e == "T")
                .unwrap_or(true);

            let basis = parse_curve(file, basis_id)?;
            Ok(TrimmedStepCurve {
                basis: Box::new(basis),
                trim1,
                trim2,
                sense_agreement,
            })
        }
        other => Err(StepError::type_mismatch("TRIMMED_CURVE", other)),
    }
}

/// Parse any supported curve entity.
///
/// Handles `LINE`, `CIRCLE`, `B_SPLINE_CURVE_WITH_KNOTS`, and `TRIMMED_CURVE`.
/// `BOUNDED_CURVE` supertypes (which appear as type aliases for `TRIMMED_CURVE`
/// in some exporters) are also resolved.
pub fn parse_curve(file: &StepFile, id: u64) -> Result<StepCurve, StepError> {
    let entity = file.get(id).ok_or(StepError::MissingEntity(id))?;

    match entity.type_name.as_str() {
        "LINE" => Ok(StepCurve::Line(parse_line(file, id)?)),
        "CIRCLE" => Ok(StepCurve::Circle(parse_circle(file, id)?)),
        "B_SPLINE_CURVE_WITH_KNOTS" | "B_SPLINE_CURVE" => {
            Ok(StepCurve::BSpline(parse_bspline_curve(file, id)?))
        }
        "TRIMMED_CURVE" => Ok(StepCurve::Trimmed(parse_trimmed_curve(file, id)?)),
        // ELLIPSE: treat as unsupported for now — callers skip and fall back to straight-line edge
        other => Err(StepError::UnsupportedEntity(other.to_string())),
    }
}

/// Write a LINE to STEP format.
/// Returns (line_entity_string, point_id, vector_id, direction_id).
#[allow(dead_code)]
pub fn write_line(
    line: &Line3d,
    name: &str,
    point_id: u64,
    vec_id: u64,
    dir_id: u64,
) -> (String, String, String) {
    let magnitude = line.direction.norm();
    let dir = if magnitude > 1e-15 {
        Dir3::new_normalize(line.direction)
    } else {
        Dir3::new_normalize(Vec3::x())
    };

    let point_str = format!(
        "CARTESIAN_POINT('', ({:.15E}, {:.15E}, {:.15E}))",
        line.origin.x, line.origin.y, line.origin.z
    );
    let dir_str = format!(
        "DIRECTION('', ({:.15E}, {:.15E}, {:.15E}))",
        dir.as_ref().x,
        dir.as_ref().y,
        dir.as_ref().z
    );
    let vec_str = format!("VECTOR('', #{}, {:.15E})", dir_id, magnitude);
    let line_str = format!("LINE('{}', #{}, #{})", name, point_id, vec_id);

    (
        line_str,
        point_str,
        format!("{}\n#{} = {}", dir_str, vec_id, vec_str),
    )
}

/// Write a CIRCLE to STEP format.
#[allow(dead_code)]
pub fn write_circle(circle: &Circle3d, name: &str, placement_id: u64) -> String {
    format!(
        "CIRCLE('{}', #{}, {:.15E})",
        name, placement_id, circle.radius
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use stepperoni::Parser;

    fn parse_step(input: &str) -> StepFile {
        Parser::parse(input.as_bytes()).unwrap()
    }

    #[test]
    fn test_parse_line() {
        let input = r#"
ISO-10303-21;
HEADER;
ENDSEC;
DATA;
#1 = CARTESIAN_POINT('', (0.0, 0.0, 0.0));
#2 = DIRECTION('', (1.0, 0.0, 0.0));
#3 = VECTOR('', #2, 10.0);
#4 = LINE('', #1, #3);
ENDSEC;
END-ISO-10303-21;
"#;
        let file = parse_step(input);
        let line = parse_line(&file, 4).unwrap();
        assert!(line.origin.x.abs() < 1e-10);
        assert!((line.direction.x - 10.0).abs() < 1e-10);
    }

    #[test]
    fn test_parse_circle() {
        let input = r#"
ISO-10303-21;
HEADER;
ENDSEC;
DATA;
#1 = CARTESIAN_POINT('', (0.0, 0.0, 0.0));
#2 = DIRECTION('', (0.0, 0.0, 1.0));
#3 = DIRECTION('', (1.0, 0.0, 0.0));
#4 = AXIS2_PLACEMENT_3D('', #1, #2, #3);
#5 = CIRCLE('', #4, 5.0);
ENDSEC;
END-ISO-10303-21;
"#;
        let file = parse_step(input);
        let circle = parse_circle(&file, 5).unwrap();
        assert!((circle.radius - 5.0).abs() < 1e-10);
        assert!(circle.center.x.abs() < 1e-10);
    }

    #[test]
    fn test_parse_bspline_curve() {
        let input = r#"
ISO-10303-21;
HEADER;
ENDSEC;
DATA;
#1 = CARTESIAN_POINT('', (0.0, 0.0, 0.0));
#2 = CARTESIAN_POINT('', (1.0, 1.0, 0.0));
#3 = CARTESIAN_POINT('', (2.0, 0.0, 0.0));
#4 = B_SPLINE_CURVE_WITH_KNOTS('', 2, (#1, #2, #3),
     .UNSPECIFIED., .F., .F.,
     (3, 3), (0.0, 1.0), .UNSPECIFIED.);
ENDSEC;
END-ISO-10303-21;
"#;
        let file = parse_step(input);
        let curve = parse_bspline_curve(&file, 4).unwrap();
        assert_eq!(curve.degree, 2);
        assert_eq!(curve.control_points.len(), 3);
        // Evaluate at t=0 should give first control point (clamped)
        let p0 = curve.eval(0.0);
        assert!((p0.x).abs() < 1e-6);
        // Evaluate at t=1 should give last control point (clamped)
        let p1 = curve.eval(1.0);
        assert!((p1.x - 2.0).abs() < 1e-6);
    }

    #[test]
    fn test_parse_trimmed_curve_circle() {
        let input = r#"
ISO-10303-21;
HEADER;
ENDSEC;
DATA;
#1 = CARTESIAN_POINT('', (0.0, 0.0, 0.0));
#2 = DIRECTION('', (0.0, 0.0, 1.0));
#3 = DIRECTION('', (1.0, 0.0, 0.0));
#4 = AXIS2_PLACEMENT_3D('', #1, #2, #3);
#5 = CIRCLE('', #4, 5.0);
#6 = TRIMMED_CURVE('', #5, ((PARAMETER_VALUE(0.0))), ((PARAMETER_VALUE(1.5707963))), .T., .PARAMETER.);
ENDSEC;
END-ISO-10303-21;
"#;
        let file = parse_step(input);
        let tc = parse_trimmed_curve(&file, 6).unwrap();
        assert!((tc.trim1).abs() < 1e-6);
        assert!((tc.trim2 - std::f64::consts::FRAC_PI_2).abs() < 1e-5);
        assert!(tc.sense_agreement);
        assert!(matches!(tc.basis.as_ref(), StepCurve::Circle(_)));
    }

    #[test]
    fn test_parse_curve_dispatch() {
        let input = r#"
ISO-10303-21;
HEADER;
ENDSEC;
DATA;
#1 = CARTESIAN_POINT('', (0.0, 0.0, 0.0));
#2 = DIRECTION('', (1.0, 0.0, 0.0));
#3 = VECTOR('', #2, 5.0);
#4 = LINE('', #1, #3);
ENDSEC;
END-ISO-10303-21;
"#;
        let file = parse_step(input);
        let curve = parse_curve(&file, 4).unwrap();
        assert!(matches!(curve, StepCurve::Line(_)));
    }
}
