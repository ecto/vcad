//! Surface entities: planes, cylinders, cones, spheres, B-splines, and
//! swept/offset surfaces (revolution, linear extrusion, offset).

use super::{
    curves::{parse_curve, StepCurve},
    parse_any_axis_placement, parse_cartesian_point, parse_vector, write_cartesian_point,
    AxisPlacement, EntityArgs,
};
use crate::error::StepError;
use stepperoni::{StepFile, StepValue};
use vcad_kernel_geom::{
    BilinearSurface, ConeSurface, CylinderSurface, Plane, SphereSurface, Surface, TorusSurface,
};
use vcad_kernel_math::{Dir3, Point3, Vec3};
use vcad_kernel_nurbs::{BSplineSurface, NurbsCurve, NurbsSurface, WeightedPoint};

/// Cap the expanded knot-vector length to defend against adversarial STEP
/// inputs whose per-knot multiplicities sum to a huge number; without this,
/// `expand_knots` happily tries to push billions of entries.
const MAX_EXPANDED_KNOTS: usize = 1_000_000;
/// Per-multiplicity cap; a single knot with multiplicity > this is rejected
/// outright (real files never need this many duplicates).
const MAX_KNOT_MULTIPLICITY: usize = 10_000;
/// Largest B-spline surface grid we will accept from a STEP file; beyond this
/// the memory / compute cost to build the surface is attacker-profitable DoS.
const MAX_BSPLINE_GRID: usize = 1_000_000;
/// Maximum surface-in-surface nesting depth (OFFSET_SURFACE referencing
/// another surface). Guards against reference cycles in malicious files
/// (`#1 = OFFSET_SURFACE('', #1, ...)`) that would otherwise recurse forever.
const MAX_SURFACE_RECURSION_DEPTH: usize = 8;
/// Grid resolution (per direction) for the sampled NURBS fallback used when
/// an OFFSET_SURFACE base has no exact analytic offset.
const OFFSET_FIT_SAMPLES: usize = 24;
/// Tolerance for direction classification (parallel/perpendicular tests on
/// unit vectors).
const DIR_EPS: f64 = 1e-9;
/// Tolerance for positional coincidence (points on axis, coplanarity).
/// Geometry is conventionally in millimeters.
const POS_EPS: f64 = 1e-8;

/// A surface parsed from STEP.
#[derive(Debug, Clone)]
pub enum StepSurface {
    /// A planar surface.
    Plane(Plane),
    /// A cylindrical surface.
    Cylinder(CylinderSurface),
    /// A conical surface.
    Cone(ConeSurface),
    /// A spherical surface.
    Sphere(SphereSurface),
    /// A toroidal surface.
    Torus(TorusSurface),
    /// A B-spline surface.
    BSpline(BSplineSurface),
    /// A rational NURBS surface (produced by converting swept surfaces —
    /// revolution / linear extrusion — that have no analytic equivalent).
    Nurbs(NurbsSurface),
}

impl StepSurface {
    /// Convert to a boxed Surface trait object.
    pub fn into_box(self) -> Box<dyn Surface> {
        match self {
            StepSurface::Plane(p) => Box::new(p),
            StepSurface::Cylinder(c) => Box::new(c),
            StepSurface::Cone(c) => Box::new(c),
            StepSurface::Sphere(s) => Box::new(s),
            StepSurface::Torus(t) => Box::new(t),
            StepSurface::BSpline(b) => Box::new(b),
            StepSurface::Nurbs(n) => Box::new(n),
        }
    }

    /// Convert a boxed Surface trait object back into a typed variant.
    ///
    /// Used to re-wrap the result of `Surface::offset()` (which returns a
    /// trait object) into the enum. Returns `None` for surface types with
    /// no corresponding variant.
    fn from_box(surface: Box<dyn Surface>) -> Option<StepSurface> {
        let any = surface.as_any();
        if let Some(p) = any.downcast_ref::<Plane>() {
            return Some(StepSurface::Plane(p.clone()));
        }
        if let Some(c) = any.downcast_ref::<CylinderSurface>() {
            return Some(StepSurface::Cylinder(c.clone()));
        }
        if let Some(c) = any.downcast_ref::<ConeSurface>() {
            return Some(StepSurface::Cone(c.clone()));
        }
        if let Some(s) = any.downcast_ref::<SphereSurface>() {
            return Some(StepSurface::Sphere(s.clone()));
        }
        if let Some(t) = any.downcast_ref::<TorusSurface>() {
            return Some(StepSurface::Torus(t.clone()));
        }
        if let Some(b) = any.downcast_ref::<BSplineSurface>() {
            return Some(StepSurface::BSpline(b.clone()));
        }
        if let Some(n) = any.downcast_ref::<NurbsSurface>() {
            return Some(StepSurface::Nurbs(n.clone()));
        }
        None
    }
}

/// Parse a PLANE entity.
///
/// STEP syntax: `PLANE(name, position)`
pub fn parse_plane(file: &StepFile, id: u64) -> Result<Plane, StepError> {
    let entity = file.get(id).ok_or(StepError::MissingEntity(id))?;

    match entity.type_name.as_str() {
        "PLANE" => {
            let placement_id = entity.entity_ref(1)?;
            let placement = parse_any_axis_placement(file, placement_id)?;

            Ok(Plane::new(
                placement.location,
                *placement.x_axis().as_ref(),
                *placement.y_axis().as_ref(),
            ))
        }
        other => Err(StepError::type_mismatch("PLANE", other)),
    }
}

/// Parse a CYLINDRICAL_SURFACE entity.
///
/// STEP syntax: `CYLINDRICAL_SURFACE(name, position, radius)`
pub fn parse_cylindrical_surface(file: &StepFile, id: u64) -> Result<CylinderSurface, StepError> {
    let entity = file.get(id).ok_or(StepError::MissingEntity(id))?;

    match entity.type_name.as_str() {
        "CYLINDRICAL_SURFACE" => {
            let placement_id = entity.entity_ref(1)?;
            let radius = entity.real(2)?;

            let placement = parse_any_axis_placement(file, placement_id)?;

            Ok(CylinderSurface {
                center: placement.location,
                axis: placement.z_axis(),
                ref_dir: placement.x_axis(),
                radius,
            })
        }
        other => Err(StepError::type_mismatch("CYLINDRICAL_SURFACE", other)),
    }
}

/// Parse a CONICAL_SURFACE entity.
///
/// STEP syntax: `CONICAL_SURFACE(name, position, radius, semi_angle)`
pub fn parse_conical_surface(file: &StepFile, id: u64) -> Result<ConeSurface, StepError> {
    let entity = file.get(id).ok_or(StepError::MissingEntity(id))?;

    match entity.type_name.as_str() {
        "CONICAL_SURFACE" => {
            let placement_id = entity.entity_ref(1)?;
            let radius = entity.real(2)?;
            let semi_angle = entity.real(3)?; // In radians

            let placement = parse_any_axis_placement(file, placement_id)?;

            // STEP conical surface is defined with apex at position + radius along axis
            // Our ConeSurface needs the apex location
            // The apex is along the negative axis direction from the position
            let apex_dist = if semi_angle.tan().abs() > 1e-15 {
                radius / semi_angle.tan()
            } else {
                0.0
            };
            let apex = placement.location - apex_dist * placement.z_axis().as_ref();

            Ok(ConeSurface {
                apex,
                axis: placement.z_axis(),
                ref_dir: placement.x_axis(),
                half_angle: semi_angle,
            })
        }
        other => Err(StepError::type_mismatch("CONICAL_SURFACE", other)),
    }
}

/// Parse a SPHERICAL_SURFACE entity.
///
/// STEP syntax: `SPHERICAL_SURFACE(name, position, radius)`
pub fn parse_spherical_surface(file: &StepFile, id: u64) -> Result<SphereSurface, StepError> {
    let entity = file.get(id).ok_or(StepError::MissingEntity(id))?;

    match entity.type_name.as_str() {
        "SPHERICAL_SURFACE" => {
            let placement_id = entity.entity_ref(1)?;
            let radius = entity.real(2)?;

            let placement = parse_any_axis_placement(file, placement_id)?;

            Ok(SphereSurface {
                center: placement.location,
                radius,
                ref_dir: placement.x_axis(),
                axis: placement.z_axis(),
            })
        }
        other => Err(StepError::type_mismatch("SPHERICAL_SURFACE", other)),
    }
}

/// Parse a TOROIDAL_SURFACE entity.
///
/// STEP syntax: `TOROIDAL_SURFACE(name, position, major_radius, minor_radius)`
pub fn parse_toroidal_surface(file: &StepFile, id: u64) -> Result<TorusSurface, StepError> {
    let entity = file.get(id).ok_or(StepError::MissingEntity(id))?;

    match entity.type_name.as_str() {
        "TOROIDAL_SURFACE" => {
            let placement_id = entity.entity_ref(1)?;
            let major_radius = entity.real(2)?;
            let minor_radius = entity.real(3)?;

            let placement = parse_any_axis_placement(file, placement_id)?;

            Ok(TorusSurface {
                center: placement.location,
                axis: placement.z_axis(),
                ref_dir: placement.x_axis(),
                major_radius,
                minor_radius,
            })
        }
        other => Err(StepError::type_mismatch("TOROIDAL_SURFACE", other)),
    }
}

/// Parse a B_SPLINE_SURFACE_WITH_KNOTS entity or complex entity containing one.
///
/// STEP syntax:
/// ```text
/// B_SPLINE_SURFACE_WITH_KNOTS(name, u_degree, v_degree, control_points,
///     surface_form, u_closed, v_closed, self_intersect,
///     u_multiplicities, v_multiplicities, u_knots, v_knots, knot_spec)
/// ```
///
/// Often appears as complex entity:
/// ```text
/// (BOUNDED_SURFACE() B_SPLINE_SURFACE(name, u_degree, v_degree, control_points, ...)
///  B_SPLINE_SURFACE_WITH_KNOTS(name, ..., u_mults, v_mults, u_knots, v_knots, ...))
/// ```
pub fn parse_bspline_surface(file: &StepFile, id: u64) -> Result<BSplineSurface, StepError> {
    let entity = file.get(id).ok_or(StepError::MissingEntity(id))?;

    // For complex entities, we need to find B_SPLINE_SURFACE and B_SPLINE_SURFACE_WITH_KNOTS data
    // The args may contain Typed variants with the actual surface data

    // Extract degrees, control points from B_SPLINE_SURFACE portion
    // Extract knot data from B_SPLINE_SURFACE_WITH_KNOTS portion

    // Try to find B_SPLINE_SURFACE_WITH_KNOTS data (has the full info)
    let (u_degree, v_degree, control_points_list, u_mults, v_mults, u_knots, v_knots) =
        if entity.type_name == "B_SPLINE_SURFACE_WITH_KNOTS" {
            // Simple entity - all data in args
            let u_degree = entity.integer(1)? as usize;
            let v_degree = entity.integer(2)? as usize;
            let cp_list = entity.list(3)?.to_vec();
            let u_mults = entity.list(8)?.to_vec();
            let v_mults = entity.list(9)?.to_vec();
            let u_knots = entity.list(10)?.to_vec();
            let v_knots = entity.list(11)?.to_vec();
            (
                u_degree, v_degree, cp_list, u_mults, v_mults, u_knots, v_knots,
            )
        } else {
            // Complex entity - look for B_SPLINE_SURFACE and B_SPLINE_SURFACE_WITH_KNOTS in args
            let mut bspline_data: Option<(&Vec<StepValue>, usize, usize)> = None;
            #[allow(clippy::type_complexity)]
            let mut knots_data: Option<(
                &Vec<StepValue>,
                &Vec<StepValue>,
                &Vec<StepValue>,
                &Vec<StepValue>,
            )> = None;

            for arg in &entity.args {
                if let StepValue::Typed { type_name, args } = arg {
                    if type_name == "B_SPLINE_SURFACE" && args.len() >= 4 {
                        let u_deg = args.get(1).and_then(|v| v.as_integer()).unwrap_or(0) as usize;
                        let v_deg = args.get(2).and_then(|v| v.as_integer()).unwrap_or(0) as usize;
                        if let Some(StepValue::List(cp)) = args.get(3) {
                            bspline_data = Some((cp, u_deg, v_deg));
                        }
                    } else if type_name == "B_SPLINE_SURFACE_WITH_KNOTS" && args.len() >= 5 {
                        // Args: name, u_mults, v_mults, u_knots, v_knots, knot_spec
                        // (indices may vary depending on whether it includes inherited attrs)
                        if let (
                            Some(StepValue::List(um)),
                            Some(StepValue::List(vm)),
                            Some(StepValue::List(uk)),
                            Some(StepValue::List(vk)),
                        ) = (args.get(1), args.get(2), args.get(3), args.get(4))
                        {
                            knots_data = Some((um, vm, uk, vk));
                        }
                    }
                }
            }

            match (bspline_data, knots_data) {
                (Some((cp_list, u_deg, v_deg)), Some((u_mults, v_mults, u_knots, v_knots))) => (
                    u_deg,
                    v_deg,
                    cp_list.clone(),
                    u_mults.clone(),
                    v_mults.clone(),
                    u_knots.clone(),
                    v_knots.clone(),
                ),
                _ => {
                    // Can't extract B-spline data - treat as unsupported
                    return Err(StepError::UnsupportedEntity(format!(
                        "B_SPLINE_SURFACE (complex entity #{})",
                        id
                    )));
                }
            }
        };

    // Parse control points - it's a 2D list of entity refs
    let mut control_points = Vec::new();
    let mut n_v = 0;
    let mut n_u = 0;

    for row in &control_points_list {
        if let StepValue::List(row_points) = row {
            n_v += 1;
            let mut row_count = 0;
            for pt_ref in row_points {
                if let Some(pt_id) = pt_ref.as_entity_ref() {
                    let pt = parse_cartesian_point(file, pt_id)?;
                    control_points.push(pt);
                    row_count += 1;
                }
            }
            if n_v == 1 {
                n_u = row_count;
            }
        }
    }

    // Expand knots according to multiplicities
    let expanded_u_knots = expand_knots(&u_knots, &u_mults)?;
    let expanded_v_knots = expand_knots(&v_knots, &v_mults)?;

    // Validate knot vectors before constructing (BSplineSurface::new panics on invalid)
    let expected_u_knots = n_u + u_degree + 1;
    let expected_v_knots = n_v + v_degree + 1;

    if expanded_u_knots.len() != expected_u_knots || expanded_v_knots.len() != expected_v_knots {
        return Err(StepError::UnsupportedEntity(format!(
            "B_SPLINE_SURFACE #{} (invalid knot vector: u={}/{} v={}/{})",
            id,
            expanded_u_knots.len(),
            expected_u_knots,
            expanded_v_knots.len(),
            expected_v_knots
        )));
    }

    // Guard against attacker-controlled sizes before calling BSplineSurface::new
    // (which asserts on mismatched dimensions and invalid knot vectors).
    let expected_cp = n_u.checked_mul(n_v).ok_or_else(|| {
        StepError::UnsupportedEntity(format!(
            "B_SPLINE_SURFACE #{} (control-point grid n_u*n_v overflows usize)",
            id
        ))
    })?;
    if expected_cp > MAX_BSPLINE_GRID {
        return Err(StepError::UnsupportedEntity(format!(
            "B_SPLINE_SURFACE #{} (grid {} exceeds cap {})",
            id, expected_cp, MAX_BSPLINE_GRID
        )));
    }
    if control_points.len() != expected_cp {
        return Err(StepError::UnsupportedEntity(format!(
            "B_SPLINE_SURFACE #{} (control point mismatch: {} != {}x{})",
            id,
            control_points.len(),
            n_u,
            n_v
        )));
    }
    // Knot vectors must be non-decreasing; BSplineSurface::new asserts this.
    if !is_non_decreasing(&expanded_u_knots) || !is_non_decreasing(&expanded_v_knots) {
        return Err(StepError::UnsupportedEntity(format!(
            "B_SPLINE_SURFACE #{} (non-monotonic knot vector)",
            id
        )));
    }

    Ok(BSplineSurface::new(
        control_points,
        n_u,
        n_v,
        expanded_u_knots,
        expanded_v_knots,
        u_degree,
        v_degree,
    ))
}

/// Expand knot vector by multiplicities.
fn expand_knots(knots: &[StepValue], mults: &[StepValue]) -> Result<Vec<f64>, StepError> {
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
                "knot multiplicity {} exceeds cap {}",
                m, MAX_KNOT_MULTIPLICITY
            )));
        }
        if result.len().saturating_add(m) > MAX_EXPANDED_KNOTS {
            return Err(StepError::UnsupportedEntity(format!(
                "expanded knot vector exceeds cap {}",
                MAX_EXPANDED_KNOTS
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

/// Compress an expanded knot vector into (unique_knots, multiplicities).
///
/// This is the inverse of [`expand_knots`]. For example:
/// `[0, 0, 0.5, 1, 1]` → `([0, 0.5, 1], [2, 1, 2])`
pub fn compress_knots(expanded: &[f64]) -> (Vec<f64>, Vec<usize>) {
    let mut knots = Vec::new();
    let mut mults = Vec::new();
    if expanded.is_empty() {
        return (knots, mults);
    }
    let mut current = expanded[0];
    let mut count = 1usize;
    for &k in &expanded[1..] {
        if (k - current).abs() < 1e-15 {
            count += 1;
        } else {
            knots.push(current);
            mults.push(count);
            current = k;
            count = 1;
        }
    }
    knots.push(current);
    mults.push(count);
    (knots, mults)
}

/// Write a B_SPLINE_SURFACE_WITH_KNOTS entity string.
///
/// `control_point_ids` is a 2D array (v-major) of STEP entity IDs for the control points.
/// The surface form, closedness, and knot spec use `.UNSPECIFIED.`.
#[allow(clippy::too_many_arguments)]
pub fn write_bspline_surface_with_knots(
    name: &str,
    degree_u: usize,
    degree_v: usize,
    control_point_ids: &[Vec<u64>],
    u_knots: &[f64],
    u_mults: &[usize],
    v_knots: &[f64],
    v_mults: &[usize],
) -> String {
    // Format control points as 2D list: ((#1, #2), (#3, #4))
    let rows: Vec<String> = control_point_ids
        .iter()
        .map(|row| {
            let refs: Vec<String> = row.iter().map(|id| format!("#{}", id)).collect();
            format!("({})", refs.join(", "))
        })
        .collect();
    let cp_str = format!("({})", rows.join(", "));

    // Format knot multiplicities
    let u_mults_str: Vec<String> = u_mults.iter().map(|m| m.to_string()).collect();
    let v_mults_str: Vec<String> = v_mults.iter().map(|m| m.to_string()).collect();
    let u_knots_str: Vec<String> = u_knots.iter().map(|k| format!("{:.15E}", k)).collect();
    let v_knots_str: Vec<String> = v_knots.iter().map(|k| format!("{:.15E}", k)).collect();

    format!(
        "B_SPLINE_SURFACE_WITH_KNOTS('{}', {}, {}, {}, .UNSPECIFIED., .U., .U., .U., ({}), ({}), ({}), ({}), .UNSPECIFIED.)",
        name,
        degree_u,
        degree_v,
        cp_str,
        u_mults_str.join(", "),
        v_mults_str.join(", "),
        u_knots_str.join(", "),
        v_knots_str.join(", "),
    )
}

/// Write control points for a B-spline surface, returning 2D array of STEP entity IDs.
///
/// Points are stored row-major in the BSplineSurface: `points[v_idx * n_u + u_idx]`.
pub fn write_bspline_control_points(
    points: &[Point3],
    n_u: usize,
    n_v: usize,
    emit: &mut impl FnMut(&str) -> u64,
) -> Vec<Vec<u64>> {
    let mut rows = Vec::with_capacity(n_v);
    for v_idx in 0..n_v {
        let mut row_ids = Vec::with_capacity(n_u);
        for u_idx in 0..n_u {
            let pt = &points[v_idx * n_u + u_idx];
            let entity = write_cartesian_point(pt, "");
            let id = emit(&entity);
            row_ids.push(id);
        }
        rows.push(row_ids);
    }
    rows
}

/// Convert a planar bilinear surface to a STEP PLANE placement.
pub fn bilinear_planar_to_placement(bilinear: &BilinearSurface) -> AxisPlacement {
    use vcad_kernel_geom::Plane;
    let plane = Plane::new(
        bilinear.p00,
        *vcad_kernel_math::Dir3::new_normalize(bilinear.p10 - bilinear.p00).as_ref(),
        *vcad_kernel_math::Dir3::new_normalize(bilinear.p01 - bilinear.p00).as_ref(),
    );
    plane_to_placement(&plane)
}

/// Parse any supported surface entity (reserved for callers that don't
/// consume face `same_sense` flags).
///
/// Convenience wrapper around [`parse_surface_oriented`] that discards the
/// normal-orientation flag. Prefer the oriented variant when the caller
/// honors face `same_sense` flags (the reader does).
#[allow(dead_code)]
pub fn parse_surface(file: &StepFile, id: u64) -> Result<StepSurface, StepError> {
    parse_surface_oriented(file, id).map(|(surface, _)| surface)
}

/// Parse any supported surface entity, returning the surface together with a
/// normal-orientation flag.
///
/// The flag is `true` when the returned surface's natural normal points
/// opposite to the STEP surface normal (`∂u × ∂v` of the STEP
/// parameterization). This happens when a swept surface maps onto an
/// analytic type whose normal convention is structural — e.g. a
/// `SURFACE_OF_REVOLUTION` of a line running *anti-parallel* to the axis is
/// still a cylinder, but the STEP normal points inward while
/// [`CylinderSurface`]'s normal always points outward. Callers that consume
/// `ADVANCED_FACE.same_sense` must XOR it with this flag.
pub fn parse_surface_oriented(file: &StepFile, id: u64) -> Result<(StepSurface, bool), StepError> {
    parse_surface_inner(file, id, 0)
}

fn parse_surface_inner(
    file: &StepFile,
    id: u64,
    depth: usize,
) -> Result<(StepSurface, bool), StepError> {
    if depth > MAX_SURFACE_RECURSION_DEPTH {
        return Err(StepError::UnsupportedEntity(format!(
            "surface #{} exceeds nesting depth {} (reference cycle?)",
            id, MAX_SURFACE_RECURSION_DEPTH
        )));
    }
    let entity = file.get(id).ok_or(StepError::MissingEntity(id))?;

    match entity.type_name.as_str() {
        "PLANE" => Ok((StepSurface::Plane(parse_plane(file, id)?), false)),
        "CYLINDRICAL_SURFACE" => Ok((
            StepSurface::Cylinder(parse_cylindrical_surface(file, id)?),
            false,
        )),
        "CONICAL_SURFACE" => Ok((StepSurface::Cone(parse_conical_surface(file, id)?), false)),
        "SPHERICAL_SURFACE" => Ok((
            StepSurface::Sphere(parse_spherical_surface(file, id)?),
            false,
        )),
        "TOROIDAL_SURFACE" => Ok((StepSurface::Torus(parse_toroidal_surface(file, id)?), false)),
        "B_SPLINE_SURFACE_WITH_KNOTS" => Ok((
            StepSurface::BSpline(parse_bspline_surface(file, id)?),
            false,
        )),
        "SURFACE_OF_REVOLUTION" => parse_surface_of_revolution(file, id),
        "SURFACE_OF_LINEAR_EXTRUSION" => parse_surface_of_linear_extrusion(file, id),
        "OFFSET_SURFACE" => parse_offset_surface(file, id, depth),
        // Complex entities often have BOUNDED_SURFACE as first type
        "BOUNDED_SURFACE" | "B_SPLINE_SURFACE" => {
            // Check if it's a complex entity with B-spline data
            let has_bspline = entity.args.iter().any(|arg| {
                matches!(arg, StepValue::Typed { type_name, .. }
                    if type_name == "B_SPLINE_SURFACE_WITH_KNOTS" || type_name == "B_SPLINE_SURFACE")
            });
            if has_bspline {
                Ok((
                    StepSurface::BSpline(parse_bspline_surface(file, id)?),
                    false,
                ))
            } else {
                Err(StepError::UnsupportedEntity(entity.type_name.clone()))
            }
        }
        other => Err(StepError::UnsupportedEntity(other.to_string())),
    }
}

// =============================================================================
// Swept surfaces (revolution / linear extrusion) and offset surfaces
// =============================================================================

/// Parse a SURFACE_OF_REVOLUTION entity.
///
/// STEP syntax: `SURFACE_OF_REVOLUTION(name, swept_curve, axis_position)`
///
/// The swept curve is revolved a full turn about the axis. Analytic special
/// cases map to analytic surfaces (the rest of the kernel — booleans,
/// fillets — works best on those):
/// - line parallel to the axis → cylinder
/// - line perpendicular to the axis → plane
/// - line intersecting the axis obliquely → cone
/// - circle coplanar with the axis and centered on it → sphere
/// - circle coplanar with the axis, off it → torus
///
/// Everything else (B-spline profiles, skew lines, tilted circles) converts
/// to an exact rational NURBS surface of revolution.
///
/// Returns the surface and a flag that is `true` when the surface's natural
/// normal is opposite the STEP `∂u × ∂v` normal (see
/// [`parse_surface_oriented`]).
pub fn parse_surface_of_revolution(
    file: &StepFile,
    id: u64,
) -> Result<(StepSurface, bool), StepError> {
    let entity = file.get(id).ok_or(StepError::MissingEntity(id))?;
    if entity.type_name != "SURFACE_OF_REVOLUTION" {
        return Err(StepError::type_mismatch(
            "SURFACE_OF_REVOLUTION",
            &entity.type_name,
        ));
    }
    let curve_id = entity.entity_ref(1)?;
    let axis_id = entity.entity_ref(2)?;
    let curve = parse_curve(file, curve_id)?;
    let placement = parse_any_axis_placement(file, axis_id)?;
    let axis_point = placement.location;
    let axis = placement.z_axis();
    let a = *axis.as_ref();

    let analytic = match curve.resolve() {
        StepCurve::Line(line) => revolve_line(&curve, line, axis_point, a, id)?,
        StepCurve::Circle(circle) => revolve_circle(&curve, circle, axis_point, a, id)?,
        _ => None,
    };
    if let Some(result) = analytic {
        return Ok(result);
    }

    let (profile, flipped) = profile_to_nurbs(&curve, id)?;
    Ok((
        StepSurface::Nurbs(NurbsSurface::revolution(&profile, axis_point, axis)),
        flipped,
    ))
}

/// Classify the revolution of a line about an axis.
///
/// Returns `Ok(None)` when the result is not an analytic surface (a skew
/// line sweeps a hyperboloid of one sheet) so the caller falls back to the
/// NURBS construction.
fn revolve_line(
    curve: &StepCurve,
    line: &vcad_kernel_geom::Line3d,
    axis_point: Point3,
    a: Vec3,
    id: u64,
) -> Result<Option<(StepSurface, bool)>, StepError> {
    let dir_norm = line.direction.norm();
    if dir_norm < 1e-12 {
        return Err(StepError::UnsupportedEntity(format!(
            "SURFACE_OF_REVOLUTION #{id}: zero-length line direction"
        )));
    }
    // Effective traversal direction (trimmed-curve sense flips it).
    let d = line.direction * (curve_sense_sign(curve) / dir_norm);
    let d_cross_a = d.cross(a);

    if d_cross_a.norm() < DIR_EPS {
        // Line parallel to the axis → cylinder.
        let rel = line.origin - axis_point;
        let radial = rel - a * rel.dot(a);
        let radius = radial.norm();
        if radius < POS_EPS {
            return Err(StepError::UnsupportedEntity(format!(
                "SURFACE_OF_REVOLUTION #{id}: revolving a line lying on the axis"
            )));
        }
        let cylinder = CylinderSurface {
            center: axis_point,
            axis: Dir3::new_normalize(a),
            ref_dir: Dir3::new_normalize(radial),
            radius,
        };
        // The STEP normal (∂angle × ∂curve) points radially outward when
        // the line runs along +axis, inward when anti-parallel;
        // CylinderSurface's normal is always outward.
        let flipped = d.dot(a) < 0.0;
        return Ok(Some((StepSurface::Cylinder(cylinder), flipped)));
    }

    if d.dot(a).abs() < DIR_EPS {
        // Line perpendicular to the axis: every point of the line sits at
        // the same height along the axis, so the sweep is a plane.
        let h = (line.origin - axis_point).dot(a);
        let origin = axis_point + a * h;
        // STEP normal ∝ -axis·(d · ρ); sample a point where the line is off
        // the axis to fix the sign.
        let mut sign = 0.0;
        for t in curve_sample_ts(curve) {
            let rho = curve.evaluate(t) - origin;
            let dp = d.dot(rho);
            if dp.abs() > POS_EPS {
                sign = dp.signum();
                break;
            }
        }
        if sign == 0.0 {
            return Err(StepError::UnsupportedEntity(format!(
                "SURFACE_OF_REVOLUTION #{id}: degenerate planar revolution"
            )));
        }
        let normal = a * (-sign);
        // x along the line, y completes the frame so that x × y = normal.
        let plane = Plane::new(origin, d, normal.cross(d));
        return Ok(Some((StepSurface::Plane(plane), false)));
    }

    // Oblique line: only analytic (a cone) when it intersects the axis;
    // a skew line sweeps a hyperboloid of one sheet.
    let line_axis_dist = ((line.origin - axis_point).dot(d_cross_a) / d_cross_a.norm()).abs();
    if line_axis_dist > POS_EPS {
        return Ok(None);
    }

    // Apex = intersection of the line with the axis.
    let w = axis_point - line.origin;
    let t_apex = w.cross(a).dot(d_cross_a) / d_cross_a.dot(d_cross_a);
    let apex = line.origin + d * t_apex;

    // Sample a profile point away from the apex to pick the nappe.
    let mut sample = None;
    for t in curve_sample_ts(curve) {
        let p = curve.evaluate(t);
        if (p - apex).norm() > 1e-6 {
            sample = Some(p);
            break;
        }
    }
    let sample = sample.ok_or_else(|| {
        StepError::UnsupportedEntity(format!(
            "SURFACE_OF_REVOLUTION #{id}: degenerate conical revolution"
        ))
    })?;
    let rel = sample - apex;
    let cone_axis = if rel.dot(a) >= 0.0 { a } else { -a };
    let axial = rel.dot(cone_axis);
    let radial = rel - cone_axis * axial;
    let half_angle = radial.norm().atan2(axial);
    let cone = ConeSurface {
        apex,
        axis: Dir3::new_normalize(cone_axis),
        ref_dir: Dir3::new_normalize(radial),
        half_angle,
    };
    // Compare the STEP normal with the cone's outward normal at the sample.
    let n_cone = radial.normalize() * half_angle.cos() - cone_axis * half_angle.sin();
    let n_step = revolution_step_normal(sample, d, axis_point, a);
    let flipped = n_step.dot(n_cone) < 0.0;
    Ok(Some((StepSurface::Cone(cone), flipped)))
}

/// Classify the revolution of a circle about an axis.
///
/// Returns `Ok(None)` when the result is not an analytic surface (the
/// circle is tilted out of the axis plane, or the sweep self-intersects).
fn revolve_circle(
    curve: &StepCurve,
    circle: &vcad_kernel_geom::Circle3d,
    axis_point: Point3,
    a: Vec3,
    id: u64,
) -> Result<Option<(StepSurface, bool)>, StepError> {
    if circle.radius < POS_EPS {
        return Err(StepError::UnsupportedEntity(format!(
            "SURFACE_OF_REVOLUTION #{id}: revolving a degenerate circle"
        )));
    }
    let n_c = *circle.normal.as_ref();
    // Analytic only when the axis lies in the circle's plane: the axis
    // direction is perpendicular to the circle normal, and the axis point
    // is in the plane.
    let coplanar =
        n_c.dot(a).abs() < DIR_EPS && (axis_point - circle.center).dot(n_c).abs() < POS_EPS;
    if !coplanar {
        return Ok(None);
    }

    let rel = circle.center - axis_point;
    let axial = rel.dot(a);
    let radial = rel - a * axial;
    let major = radial.norm();

    if major < POS_EPS {
        // Circle centered on the axis → sphere.
        let sphere = SphereSurface {
            center: circle.center,
            radius: circle.radius,
            ref_dir: Dir3::new_normalize(perpendicular_dir(a)),
            axis: Dir3::new_normalize(a),
        };
        let flipped = revolved_circle_flipped(curve, axis_point, a, circle.center);
        return Ok(Some((StepSurface::Sphere(sphere), flipped)));
    }
    if major <= circle.radius + POS_EPS {
        // The tube crosses the axis (lemon/apple shape) — a self-intersecting
        // torus, which TorusSurface does not model. Use the NURBS fallback.
        return Ok(None);
    }

    let torus = TorusSurface {
        center: axis_point + a * axial,
        axis: Dir3::new_normalize(a),
        ref_dir: Dir3::new_normalize(radial),
        major_radius: major,
        minor_radius: circle.radius,
    };
    // At the profile's angular position the tube center is the circle
    // center, so the torus's natural normal there is `p - circle.center`.
    let flipped = revolved_circle_flipped(curve, axis_point, a, circle.center);
    Ok(Some((StepSurface::Torus(torus), flipped)))
}

/// Whether the natural (outward-from-`natural_center`) normal of a revolved
/// circle disagrees with the STEP `∂u × ∂v` normal.
fn revolved_circle_flipped(
    curve: &StepCurve,
    axis_point: Point3,
    a: Vec3,
    natural_center: Point3,
) -> bool {
    for t in curve_sample_ts(curve) {
        let p = curve.evaluate(t);
        // Skip samples on the axis: the sweep direction degenerates there.
        let rel = p - axis_point;
        let rho = rel - a * rel.dot(a);
        if rho.norm() < 1e-6 {
            continue;
        }
        let n_step = revolution_step_normal(p, curve_tangent(curve, t), axis_point, a);
        let n_natural = p - natural_center;
        if n_step.norm() < 1e-12 || n_natural.norm() < 1e-12 {
            continue;
        }
        return n_step.dot(n_natural) < 0.0;
    }
    false
}

/// The (unnormalized) STEP surface normal of a surface of revolution at a
/// point on the profile: `∂σ/∂u × ∂σ/∂v = (axis × ρ) × C'(v)`, where `ρ` is
/// the radial offset of the point from the axis.
fn revolution_step_normal(point: Point3, tangent: Vec3, axis_point: Point3, a: Vec3) -> Vec3 {
    let rel = point - axis_point;
    let rho = rel - a * rel.dot(a);
    a.cross(rho).cross(tangent)
}

/// Parse a SURFACE_OF_LINEAR_EXTRUSION entity.
///
/// STEP syntax: `SURFACE_OF_LINEAR_EXTRUSION(name, swept_curve, extrusion_axis)`
/// where `extrusion_axis` is a VECTOR (direction with magnitude).
///
/// Analytic special cases:
/// - line → plane
/// - circle extruded along its normal → cylinder
///
/// Everything else converts to an exact NURBS extrusion surface.
///
/// Returns the surface and a flag that is `true` when the surface's natural
/// normal is opposite the STEP `C'(u) × V` normal (see
/// [`parse_surface_oriented`]).
pub fn parse_surface_of_linear_extrusion(
    file: &StepFile,
    id: u64,
) -> Result<(StepSurface, bool), StepError> {
    let entity = file.get(id).ok_or(StepError::MissingEntity(id))?;
    if entity.type_name != "SURFACE_OF_LINEAR_EXTRUSION" {
        return Err(StepError::type_mismatch(
            "SURFACE_OF_LINEAR_EXTRUSION",
            &entity.type_name,
        ));
    }
    let curve_id = entity.entity_ref(1)?;
    let vec_id = entity.entity_ref(2)?;
    let curve = parse_curve(file, curve_id)?;
    let v = parse_vector(file, vec_id)?;
    let v_norm = v.norm();
    if v_norm < 1e-12 {
        return Err(StepError::UnsupportedEntity(format!(
            "SURFACE_OF_LINEAR_EXTRUSION #{id}: zero-length extrusion vector"
        )));
    }
    let sense = curve_sense_sign(&curve);

    match curve.resolve() {
        StepCurve::Line(line) => {
            let dir_norm = line.direction.norm();
            if dir_norm < 1e-12 {
                return Err(StepError::UnsupportedEntity(format!(
                    "SURFACE_OF_LINEAR_EXTRUSION #{id}: zero-length line direction"
                )));
            }
            let d = line.direction * (sense / dir_norm);
            let n = d.cross(v);
            if n.norm() < DIR_EPS * v_norm {
                return Err(StepError::UnsupportedEntity(format!(
                    "SURFACE_OF_LINEAR_EXTRUSION #{id}: extruding a line along itself"
                )));
            }
            // Plane whose normal is the STEP normal C'(u) × V.
            let normal = n.normalize();
            let plane = Plane::new(line.origin, d, normal.cross(d));
            Ok((StepSurface::Plane(plane), false))
        }
        StepCurve::Circle(circle) => {
            let n_c = *circle.normal.as_ref();
            if n_c.cross(v).norm() < DIR_EPS * v_norm {
                // Extruded along its own normal → cylinder.
                let cylinder = CylinderSurface {
                    center: circle.center,
                    axis: circle.normal,
                    ref_dir: circle.x_dir,
                    radius: circle.radius,
                };
                // STEP normal C'(u) × V is outward for a CCW circle extruded
                // along its normal, inward when the vector (or traversal)
                // runs the other way.
                let flipped = v.dot(n_c) * sense < 0.0;
                Ok((StepSurface::Cylinder(cylinder), flipped))
            } else {
                let (profile, flipped) = profile_to_nurbs(&curve, id)?;
                Ok((
                    StepSurface::Nurbs(NurbsSurface::extrusion(&profile, v)),
                    flipped,
                ))
            }
        }
        _ => {
            let (profile, flipped) = profile_to_nurbs(&curve, id)?;
            Ok((
                StepSurface::Nurbs(NurbsSurface::extrusion(&profile, v)),
                flipped,
            ))
        }
    }
}

/// Parse an OFFSET_SURFACE entity.
///
/// STEP syntax: `OFFSET_SURFACE(name, basis_surface, distance, self_intersect)`
///
/// The offset distance is measured along the basis surface's STEP normal.
/// Analytic bases offset exactly via [`Surface::offset`] (a plane shifts its
/// origin, cylinder/sphere change radius, a cone shifts its apex, a torus
/// changes its minor radius). B-spline/NURBS bases fall back to a sampled
/// B-spline fit of the offset points.
fn parse_offset_surface(
    file: &StepFile,
    id: u64,
    depth: usize,
) -> Result<(StepSurface, bool), StepError> {
    let entity = file.get(id).ok_or(StepError::MissingEntity(id))?;
    if entity.type_name != "OFFSET_SURFACE" {
        return Err(StepError::type_mismatch(
            "OFFSET_SURFACE",
            &entity.type_name,
        ));
    }
    let basis_id = entity.entity_ref(1)?;
    let distance = entity.real(2)?;
    if !distance.is_finite() {
        return Err(StepError::UnsupportedEntity(format!(
            "OFFSET_SURFACE #{id}: non-finite offset distance"
        )));
    }
    let (base, base_flipped) = parse_surface_inner(file, basis_id, depth + 1)?;
    // The STEP distance is measured along the STEP basis normal; when our
    // base surface's natural normal is flipped relative to it, negate.
    let d = if base_flipped { -distance } else { distance };

    match base {
        StepSurface::BSpline(_) | StepSurface::Nurbs(_) => {
            let boxed = base.into_box();
            let fitted = fit_offset_surface(boxed.as_ref(), d, id)?;
            Ok((StepSurface::BSpline(fitted), base_flipped))
        }
        analytic => {
            let offset = analytic.into_box().offset(d).ok_or_else(|| {
                StepError::UnsupportedEntity(format!(
                    "OFFSET_SURFACE #{id}: offset by {distance} degenerates the base surface"
                ))
            })?;
            let surface = StepSurface::from_box(offset).ok_or_else(|| {
                StepError::UnsupportedEntity(format!(
                    "OFFSET_SURFACE #{id}: unsupported offset result type"
                ))
            })?;
            Ok((surface, base_flipped))
        }
    }
}

/// Approximate the offset of a surface with a bicubic B-spline fit.
///
/// Samples a grid of `base(u,v) + d·n(u,v)` points and uses them as control
/// points of a clamped uniform bicubic patch. The control points are not
/// interpolated, so this is an approximation — it is only used when the base
/// has no exact analytic offset (B-spline/NURBS bases).
fn fit_offset_surface(
    base: &dyn Surface,
    distance: f64,
    id: u64,
) -> Result<BSplineSurface, StepError> {
    let ((u0, u1), (v0, v1)) = base.domain();
    if !(u1 - u0).is_finite() || !(v1 - v0).is_finite() {
        return Err(StepError::UnsupportedEntity(format!(
            "OFFSET_SURFACE #{id}: base surface has an unbounded domain"
        )));
    }
    let n = OFFSET_FIT_SAMPLES;
    let mut pts = Vec::with_capacity((n + 1) * (n + 1));
    for j in 0..=n {
        let v = v0 + (v1 - v0) * j as f64 / n as f64;
        for i in 0..=n {
            let u = u0 + (u1 - u0) * i as f64 / n as f64;
            let uv = vcad_kernel_math::Point2::new(u, v);
            let p = base.evaluate(uv) + *base.normal(uv).as_ref() * distance;
            if !p.x.is_finite() || !p.y.is_finite() || !p.z.is_finite() {
                return Err(StepError::UnsupportedEntity(format!(
                    "OFFSET_SURFACE #{id}: offset sampling produced non-finite points"
                )));
            }
            pts.push(p);
        }
    }
    let knots = clamped_uniform_knots(n + 1, 3);
    Ok(BSplineSurface::new(
        pts,
        n + 1,
        n + 1,
        knots.clone(),
        knots,
        3,
        3,
    ))
}

/// Clamped uniform knot vector for `n_pts` control points at `degree`.
fn clamped_uniform_knots(n_pts: usize, degree: usize) -> Vec<f64> {
    let m = n_pts + degree + 1;
    let mut knots = vec![0.0; m];
    let n_internal = m - 2 * (degree + 1);
    for i in 0..=degree {
        knots[i] = 0.0;
        knots[m - 1 - i] = 1.0;
    }
    for i in 1..=n_internal {
        knots[degree + i] = i as f64 / (n_internal + 1) as f64;
    }
    knots
}

/// Convert a parsed STEP curve into a NURBS profile curve for sweeping,
/// along with a flag that is `true` when the profile's parameter direction
/// runs against the effective (sense-adjusted) traversal — in that case the
/// swept surface's constructed normal is opposite the STEP normal.
///
/// Lines become degree-1 segments over their trim range (or one
/// direction-vector length when untrimmed); circles become exact rational
/// quadratics (full circle — the face's bounds trim it in 3D); B-splines
/// convert with unit weights.
fn profile_to_nurbs(curve: &StepCurve, id: u64) -> Result<(NurbsCurve, bool), StepError> {
    let sense_flipped = curve_sense_sign(curve) < 0.0;
    match curve.resolve() {
        StepCurve::Line(line) => {
            let len = line.direction.norm();
            if len < 1e-12 {
                return Err(StepError::UnsupportedEntity(format!(
                    "swept surface #{id}: zero-length line direction"
                )));
            }
            let (t0, t1) = match curve {
                StepCurve::Trimmed(tc) => (tc.trim1, tc.trim2),
                _ => (0.0, len),
            };
            let p0 = curve.evaluate(t0);
            let p1 = curve.evaluate(t1);
            if (p1 - p0).norm() < 1e-12 {
                return Err(StepError::UnsupportedEntity(format!(
                    "swept surface #{id}: degenerate line profile"
                )));
            }
            let profile = NurbsCurve::new(
                vec![WeightedPoint::unweighted(p0), WeightedPoint::unweighted(p1)],
                vec![0.0, 0.0, 1.0, 1.0],
                1,
            );
            // Direction is baked from the trim order, so no flip.
            Ok((profile, false))
        }
        StepCurve::Circle(circle) => Ok((
            NurbsCurve::circle_in_frame(circle.center, circle.x_dir, circle.y_dir, circle.radius),
            sense_flipped,
        )),
        StepCurve::BSpline(bspline) => {
            let profile = NurbsCurve::new(
                bspline
                    .control_points
                    .iter()
                    .map(|p| WeightedPoint::unweighted(*p))
                    .collect(),
                bspline.knots.clone(),
                bspline.degree,
            );
            Ok((profile, sense_flipped))
        }
        // `resolve()` never returns a Trimmed variant.
        StepCurve::Trimmed(_) => Err(StepError::UnsupportedEntity(format!(
            "swept surface #{id}: unresolvable trimmed profile"
        ))),
    }
}

/// Tangent (direction of increasing parameter) of a STEP curve at `t`, in
/// the effective traversal direction: trimmed-curve `sense_agreement = false`
/// flips the sign relative to the basis parameterization.
fn curve_tangent(curve: &StepCurve, t: f64) -> Vec3 {
    match curve {
        StepCurve::Line(line) => line.direction,
        StepCurve::Circle(circle) => {
            ((*circle.y_dir.as_ref()) * t.cos() - (*circle.x_dir.as_ref()) * t.sin())
                * circle.radius
        }
        StepCurve::BSpline(bspline) => bspline.tangent(t),
        StepCurve::Trimmed(tc) => {
            let tangent = curve_tangent(&tc.basis, t);
            if tc.sense_agreement {
                tangent
            } else {
                -tangent
            }
        }
    }
}

/// Product of `sense_agreement` signs through any trimmed-curve wrappers:
/// -1.0 when the effective traversal runs against the basis parameterization.
fn curve_sense_sign(curve: &StepCurve) -> f64 {
    match curve {
        StepCurve::Trimmed(tc) => {
            (if tc.sense_agreement { 1.0 } else { -1.0 }) * curve_sense_sign(&tc.basis)
        }
        _ => 1.0,
    }
}

/// Candidate parameter values on a curve for orientation sampling, in the
/// curve's own (basis) parameterization. Trimmed curves sample inside the
/// trim range, since that is where the face actually lies.
fn curve_sample_ts(curve: &StepCurve) -> Vec<f64> {
    use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI};
    match curve {
        StepCurve::Trimmed(tc) => {
            let (t1, t2) = (tc.trim1, tc.trim2);
            vec![
                0.5 * (t1 + t2),
                t1 + 0.75 * (t2 - t1),
                t1 + 0.25 * (t2 - t1),
            ]
        }
        StepCurve::Line(line) => {
            let len = line.direction.norm().max(1.0);
            vec![0.5 * len, len, -len, 2.0 * len]
        }
        StepCurve::Circle(_) => vec![FRAC_PI_4, FRAC_PI_2, 0.75 * PI],
        StepCurve::BSpline(bspline) => {
            let (t0, t1) = bspline.parameter_domain();
            vec![
                0.5 * (t0 + t1),
                t0 + 0.75 * (t1 - t0),
                t0 + 0.25 * (t1 - t0),
            ]
        }
    }
}

/// Any unit vector perpendicular to `v` (for building reference frames).
fn perpendicular_dir(v: Vec3) -> Vec3 {
    let arbitrary = if v.x.abs() < 0.9 {
        Vec3::x()
    } else {
        Vec3::y()
    };
    (arbitrary - v * arbitrary.dot(v)).normalize()
}

/// Create an AxisPlacement from a Plane for writing.
pub fn plane_to_placement(plane: &Plane) -> AxisPlacement {
    AxisPlacement {
        location: plane.origin,
        axis: Some(plane.normal_dir),
        ref_direction: Some(plane.x_dir),
    }
}

/// Create an AxisPlacement from a CylinderSurface for writing.
pub fn cylinder_to_placement(cyl: &CylinderSurface) -> AxisPlacement {
    AxisPlacement {
        location: cyl.center,
        axis: Some(cyl.axis),
        ref_direction: Some(cyl.ref_dir),
    }
}

/// Create an AxisPlacement from a SphereSurface for writing.
pub fn sphere_to_placement(sphere: &SphereSurface) -> AxisPlacement {
    AxisPlacement {
        location: sphere.center,
        axis: Some(sphere.axis),
        ref_direction: Some(sphere.ref_dir),
    }
}

/// Create an AxisPlacement from a TorusSurface for writing.
pub fn torus_to_placement(torus: &TorusSurface) -> AxisPlacement {
    AxisPlacement {
        location: torus.center,
        axis: Some(torus.axis),
        ref_direction: Some(torus.ref_dir),
    }
}

/// Write a PLANE to STEP format.
pub fn write_plane(name: &str, placement_id: u64) -> String {
    format!("PLANE('{}', #{})", name, placement_id)
}

/// Write a CYLINDRICAL_SURFACE to STEP format.
pub fn write_cylindrical_surface(radius: f64, name: &str, placement_id: u64) -> String {
    format!(
        "CYLINDRICAL_SURFACE('{}', #{}, {:.15E})",
        name, placement_id, radius
    )
}

/// Write a CONICAL_SURFACE to STEP format.
pub fn write_conical_surface(
    radius: f64,
    semi_angle: f64,
    name: &str,
    placement_id: u64,
) -> String {
    format!(
        "CONICAL_SURFACE('{}', #{}, {:.15E}, {:.15E})",
        name, placement_id, radius, semi_angle
    )
}

/// Write a SPHERICAL_SURFACE to STEP format.
pub fn write_spherical_surface(radius: f64, name: &str, placement_id: u64) -> String {
    format!(
        "SPHERICAL_SURFACE('{}', #{}, {:.15E})",
        name, placement_id, radius
    )
}

/// Write a TOROIDAL_SURFACE to STEP format.
///
/// STEP syntax: `TOROIDAL_SURFACE(name, position, major_radius, minor_radius)`
pub fn write_toroidal_surface(
    major_radius: f64,
    minor_radius: f64,
    name: &str,
    placement_id: u64,
) -> String {
    format!(
        "TOROIDAL_SURFACE('{}', #{}, {:.15E}, {:.15E})",
        name, placement_id, major_radius, minor_radius
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use stepperoni::Parser;
    use vcad_kernel_math::Point3;

    fn parse_step(input: &str) -> StepFile {
        Parser::parse(input.as_bytes()).unwrap()
    }

    #[test]
    fn test_compress_knots() {
        let expanded = vec![0.0, 0.0, 0.5, 1.0, 1.0];
        let (knots, mults) = compress_knots(&expanded);
        assert_eq!(knots, vec![0.0, 0.5, 1.0]);
        assert_eq!(mults, vec![2, 1, 2]);
    }

    #[test]
    fn test_compress_knots_uniform() {
        let expanded = vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        let (knots, mults) = compress_knots(&expanded);
        assert_eq!(knots, vec![0.0, 1.0]);
        assert_eq!(mults, vec![3, 3]);
    }

    #[test]
    fn test_compress_knots_roundtrip() {
        // Compress then expand should be identity
        let original = vec![0.0, 0.0, 0.0, 0.25, 0.5, 0.75, 1.0, 1.0, 1.0];
        let (knots, mults) = compress_knots(&original);
        let mut re_expanded = Vec::new();
        for (k, m) in knots.iter().zip(mults.iter()) {
            for _ in 0..*m {
                re_expanded.push(*k);
            }
        }
        assert_eq!(original.len(), re_expanded.len());
        for (a, b) in original.iter().zip(re_expanded.iter()) {
            assert!((a - b).abs() < 1e-15);
        }
    }

    #[test]
    fn test_write_bspline_surface() {
        let cp_ids = vec![vec![1, 2, 3], vec![4, 5, 6]];
        let u_knots = vec![0.0, 1.0];
        let u_mults = vec![3, 3];
        let v_knots = vec![0.0, 1.0];
        let v_mults = vec![2, 2];
        let entity = write_bspline_surface_with_knots(
            "", 2, 1, &cp_ids, &u_knots, &u_mults, &v_knots, &v_mults,
        );
        assert!(entity.contains("B_SPLINE_SURFACE_WITH_KNOTS"));
        assert!(entity.contains("(#1, #2, #3)"));
        assert!(entity.contains("(#4, #5, #6)"));
        assert!(entity.contains(".UNSPECIFIED."));
    }

    #[test]
    fn test_write_bilinear_planar_placement() {
        let bilinear = BilinearSurface::new(
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(0.0, 1.0, 0.0),
            Point3::new(1.0, 1.0, 0.0),
        );
        assert!(bilinear.is_planar());
        let placement = bilinear_planar_to_placement(&bilinear);
        assert!((placement.location.z).abs() < 1e-10);
    }

    #[test]
    fn test_parse_plane() {
        let input = r#"
ISO-10303-21;
HEADER;
ENDSEC;
DATA;
#1 = CARTESIAN_POINT('', (0.0, 0.0, 5.0));
#2 = DIRECTION('', (0.0, 0.0, 1.0));
#3 = DIRECTION('', (1.0, 0.0, 0.0));
#4 = AXIS2_PLACEMENT_3D('', #1, #2, #3);
#5 = PLANE('', #4);
ENDSEC;
END-ISO-10303-21;
"#;
        let file = parse_step(input);
        let plane = parse_plane(&file, 5).unwrap();
        assert!((plane.origin.z - 5.0).abs() < 1e-10);
        assert!((plane.normal_dir.as_ref().z - 1.0).abs() < 1e-10);
    }

    #[test]
    fn test_parse_cylindrical_surface() {
        let input = r#"
ISO-10303-21;
HEADER;
ENDSEC;
DATA;
#1 = CARTESIAN_POINT('', (0.0, 0.0, 0.0));
#2 = DIRECTION('', (0.0, 0.0, 1.0));
#3 = DIRECTION('', (1.0, 0.0, 0.0));
#4 = AXIS2_PLACEMENT_3D('', #1, #2, #3);
#5 = CYLINDRICAL_SURFACE('', #4, 10.0);
ENDSEC;
END-ISO-10303-21;
"#;
        let file = parse_step(input);
        let cyl = parse_cylindrical_surface(&file, 5).unwrap();
        assert!((cyl.radius - 10.0).abs() < 1e-10);
    }

    #[test]
    fn test_parse_spherical_surface() {
        let input = r#"
ISO-10303-21;
HEADER;
ENDSEC;
DATA;
#1 = CARTESIAN_POINT('', (1.0, 2.0, 3.0));
#2 = DIRECTION('', (0.0, 0.0, 1.0));
#3 = DIRECTION('', (1.0, 0.0, 0.0));
#4 = AXIS2_PLACEMENT_3D('', #1, #2, #3);
#5 = SPHERICAL_SURFACE('', #4, 7.5);
ENDSEC;
END-ISO-10303-21;
"#;
        let file = parse_step(input);
        let sphere = parse_spherical_surface(&file, 5).unwrap();
        assert!((sphere.radius - 7.5).abs() < 1e-10);
        assert!((sphere.center.x - 1.0).abs() < 1e-10);
    }

    const FIXTURE_HEADER: &str = "ISO-10303-21;\nHEADER;\nENDSEC;\nDATA;\n";
    const FIXTURE_FOOTER: &str = "ENDSEC;\nEND-ISO-10303-21;\n";

    fn fixture(body: &str) -> StepFile {
        parse_step(&format!("{FIXTURE_HEADER}{body}{FIXTURE_FOOTER}"))
    }

    #[test]
    fn test_revolution_of_parallel_line_is_cylinder() {
        // Line at x=10 running along +Z, revolved about the Z axis.
        let file = fixture(
            "#1 = CARTESIAN_POINT('', (10.0, 0.0, 0.0));
#2 = DIRECTION('', (0.0, 0.0, 1.0));
#3 = VECTOR('', #2, 1.0);
#4 = LINE('', #1, #3);
#5 = CARTESIAN_POINT('', (0.0, 0.0, 0.0));
#6 = DIRECTION('', (0.0, 0.0, 1.0));
#7 = AXIS1_PLACEMENT('', #5, #6);
#8 = SURFACE_OF_REVOLUTION('', #4, #7);
",
        );
        let (surface, flipped) = parse_surface_of_revolution(&file, 8).unwrap();
        assert!(!flipped);
        match surface {
            StepSurface::Cylinder(c) => {
                assert!((c.radius - 10.0).abs() < 1e-10);
                assert!((c.axis.as_ref().z.abs() - 1.0).abs() < 1e-10);
                assert!(c.center.x.abs() < 1e-10 && c.center.y.abs() < 1e-10);
            }
            other => panic!("expected cylinder, got {:?}", other),
        }
    }

    #[test]
    fn test_revolution_of_antiparallel_line_flips_sense() {
        // Same cylinder but the line runs along -Z: the STEP normal points
        // inward while CylinderSurface's normal is outward.
        let file = fixture(
            "#1 = CARTESIAN_POINT('', (10.0, 0.0, 20.0));
#2 = DIRECTION('', (0.0, 0.0, -1.0));
#3 = VECTOR('', #2, 1.0);
#4 = LINE('', #1, #3);
#5 = CARTESIAN_POINT('', (0.0, 0.0, 0.0));
#6 = DIRECTION('', (0.0, 0.0, 1.0));
#7 = AXIS1_PLACEMENT('', #5, #6);
#8 = SURFACE_OF_REVOLUTION('', #4, #7);
",
        );
        let (surface, flipped) = parse_surface_of_revolution(&file, 8).unwrap();
        assert!(flipped, "anti-parallel line should flip the sense");
        assert!(matches!(surface, StepSurface::Cylinder(_)));
    }

    #[test]
    fn test_revolution_of_perpendicular_line_is_plane() {
        // Radial line at height z=5 pointing outward (+X), revolved about Z.
        let file = fixture(
            "#1 = CARTESIAN_POINT('', (2.0, 0.0, 5.0));
#2 = DIRECTION('', (1.0, 0.0, 0.0));
#3 = VECTOR('', #2, 1.0);
#4 = LINE('', #1, #3);
#5 = CARTESIAN_POINT('', (0.0, 0.0, 0.0));
#6 = DIRECTION('', (0.0, 0.0, 1.0));
#7 = AXIS1_PLACEMENT('', #5, #6);
#8 = SURFACE_OF_REVOLUTION('', #4, #7);
",
        );
        let (surface, flipped) = parse_surface_of_revolution(&file, 8).unwrap();
        assert!(!flipped);
        match surface {
            StepSurface::Plane(p) => {
                assert!((p.origin.z - 5.0).abs() < 1e-10);
                // Outward-pointing line → STEP normal is -axis.
                assert!((p.normal_dir.as_ref().z + 1.0).abs() < 1e-10);
            }
            other => panic!("expected plane, got {:?}", other),
        }
    }

    #[test]
    fn test_revolution_of_intersecting_line_is_cone() {
        // Line from (10,0,0) toward the apex at (0,0,20), revolved about Z.
        let file = fixture(
            "#1 = CARTESIAN_POINT('', (10.0, 0.0, 0.0));
#2 = DIRECTION('', (-0.4472135954999579, 0.0, 0.8944271909999159));
#3 = VECTOR('', #2, 1.0);
#4 = LINE('', #1, #3);
#5 = CARTESIAN_POINT('', (0.0, 0.0, 0.0));
#6 = DIRECTION('', (0.0, 0.0, 1.0));
#7 = AXIS1_PLACEMENT('', #5, #6);
#8 = SURFACE_OF_REVOLUTION('', #4, #7);
",
        );
        let (surface, _flipped) = parse_surface_of_revolution(&file, 8).unwrap();
        match surface {
            StepSurface::Cone(c) => {
                assert!(c.apex.x.abs() < 1e-8);
                assert!((c.apex.z - 20.0).abs() < 1e-8);
                // half-angle = atan(10/20)
                assert!((c.half_angle - (0.5f64).atan()).abs() < 1e-8);
            }
            other => panic!("expected cone, got {:?}", other),
        }
    }

    #[test]
    fn test_revolution_of_skew_line_is_nurbs_hyperboloid() {
        // Line offset from the axis and tilted: sweeps a hyperboloid of one
        // sheet, which has no analytic type — expect the NURBS fallback.
        let file = fixture(
            "#1 = CARTESIAN_POINT('', (10.0, 5.0, 0.0));
#2 = DIRECTION('', (0.0, 0.7071067811865476, 0.7071067811865476));
#3 = VECTOR('', #2, 1.0);
#4 = LINE('', #1, #3);
#5 = CARTESIAN_POINT('', (0.0, 0.0, 0.0));
#6 = DIRECTION('', (0.0, 0.0, 1.0));
#7 = AXIS1_PLACEMENT('', #5, #6);
#8 = SURFACE_OF_REVOLUTION('', #4, #7);
",
        );
        let (surface, _) = parse_surface_of_revolution(&file, 8).unwrap();
        assert!(matches!(surface, StepSurface::Nurbs(_)));
    }

    #[test]
    fn test_revolution_of_offset_circle_is_torus() {
        // Circle r=3 centered at (10,0,0) in the XZ plane, revolved about Z.
        let file = fixture(
            "#1 = CARTESIAN_POINT('', (10.0, 0.0, 0.0));
#2 = DIRECTION('', (0.0, -1.0, 0.0));
#3 = DIRECTION('', (1.0, 0.0, 0.0));
#4 = AXIS2_PLACEMENT_3D('', #1, #2, #3);
#5 = CIRCLE('', #4, 3.0);
#6 = CARTESIAN_POINT('', (0.0, 0.0, 0.0));
#7 = DIRECTION('', (0.0, 0.0, 1.0));
#8 = AXIS1_PLACEMENT('', #6, #7);
#9 = SURFACE_OF_REVOLUTION('', #5, #8);
",
        );
        let (surface, _) = parse_surface_of_revolution(&file, 9).unwrap();
        match surface {
            StepSurface::Torus(t) => {
                assert!((t.major_radius - 10.0).abs() < 1e-10);
                assert!((t.minor_radius - 3.0).abs() < 1e-10);
                assert!(t.center.x.abs() < 1e-10 && t.center.z.abs() < 1e-10);
            }
            other => panic!("expected torus, got {:?}", other),
        }
    }

    #[test]
    fn test_revolution_of_centered_circle_is_sphere() {
        // Circle r=7 centered on the Z axis in the XZ plane → sphere.
        let file = fixture(
            "#1 = CARTESIAN_POINT('', (0.0, 0.0, 4.0));
#2 = DIRECTION('', (0.0, -1.0, 0.0));
#3 = DIRECTION('', (1.0, 0.0, 0.0));
#4 = AXIS2_PLACEMENT_3D('', #1, #2, #3);
#5 = CIRCLE('', #4, 7.0);
#6 = CARTESIAN_POINT('', (0.0, 0.0, 0.0));
#7 = DIRECTION('', (0.0, 0.0, 1.0));
#8 = AXIS1_PLACEMENT('', #6, #7);
#9 = SURFACE_OF_REVOLUTION('', #5, #8);
",
        );
        let (surface, _) = parse_surface_of_revolution(&file, 9).unwrap();
        match surface {
            StepSurface::Sphere(s) => {
                assert!((s.radius - 7.0).abs() < 1e-10);
                assert!((s.center.z - 4.0).abs() < 1e-10);
            }
            other => panic!("expected sphere, got {:?}", other),
        }
    }

    #[test]
    fn test_revolution_of_bspline_profile_is_nurbs() {
        let file = fixture(
            "#1 = CARTESIAN_POINT('', (10.0, 0.0, 0.0));
#2 = CARTESIAN_POINT('', (12.0, 0.0, 10.0));
#3 = CARTESIAN_POINT('', (10.0, 0.0, 20.0));
#4 = B_SPLINE_CURVE_WITH_KNOTS('', 2, (#1, #2, #3),
     .UNSPECIFIED., .F., .F.,
     (3, 3), (0.0, 1.0), .UNSPECIFIED.);
#5 = CARTESIAN_POINT('', (0.0, 0.0, 0.0));
#6 = DIRECTION('', (0.0, 0.0, 1.0));
#7 = AXIS1_PLACEMENT('', #5, #6);
#8 = SURFACE_OF_REVOLUTION('', #4, #7);
",
        );
        let (surface, flipped) = parse_surface_of_revolution(&file, 8).unwrap();
        assert!(!flipped);
        match surface {
            StepSurface::Nurbs(n) => {
                // u=0 slice must reproduce the profile: at v ends, radius 10
                let p0 = n.eval(0.0, 0.0);
                assert!((p0.x - 10.0).abs() < 1e-8 && p0.z.abs() < 1e-8);
                let p1 = n.eval(0.0, 1.0);
                assert!((p1.x - 10.0).abs() < 1e-8 && (p1.z - 20.0).abs() < 1e-8);
                // Half a turn: profile start rotated to (-10, 0, 0)
                let half = n.eval(std::f64::consts::PI, 0.0);
                assert!((half.x + 10.0).abs() < 1e-8, "x at half turn: {}", half.x);
            }
            other => panic!("expected NURBS, got {:?}", other),
        }
    }

    #[test]
    fn test_extrusion_of_line_is_plane() {
        // Line along X extruded along Z → XZ-ish plane with normal -Y
        // (d × v = X × Z = -Y).
        let file = fixture(
            "#1 = CARTESIAN_POINT('', (0.0, 3.0, 0.0));
#2 = DIRECTION('', (1.0, 0.0, 0.0));
#3 = VECTOR('', #2, 1.0);
#4 = LINE('', #1, #3);
#5 = DIRECTION('', (0.0, 0.0, 1.0));
#6 = VECTOR('', #5, 15.0);
#7 = SURFACE_OF_LINEAR_EXTRUSION('', #4, #6);
",
        );
        let (surface, flipped) = parse_surface_of_linear_extrusion(&file, 7).unwrap();
        assert!(!flipped);
        match surface {
            StepSurface::Plane(p) => {
                assert!((p.origin.y - 3.0).abs() < 1e-10);
                assert!((p.normal_dir.as_ref().y + 1.0).abs() < 1e-10);
            }
            other => panic!("expected plane, got {:?}", other),
        }
    }

    #[test]
    fn test_extrusion_of_circle_is_cylinder() {
        let file = fixture(
            "#1 = CARTESIAN_POINT('', (1.0, 2.0, 0.0));
#2 = DIRECTION('', (0.0, 0.0, 1.0));
#3 = DIRECTION('', (1.0, 0.0, 0.0));
#4 = AXIS2_PLACEMENT_3D('', #1, #2, #3);
#5 = CIRCLE('', #4, 6.0);
#6 = DIRECTION('', (0.0, 0.0, 1.0));
#7 = VECTOR('', #6, 30.0);
#8 = SURFACE_OF_LINEAR_EXTRUSION('', #5, #7);
",
        );
        let (surface, flipped) = parse_surface_of_linear_extrusion(&file, 8).unwrap();
        assert!(!flipped);
        match surface {
            StepSurface::Cylinder(c) => {
                assert!((c.radius - 6.0).abs() < 1e-10);
                assert!((c.center.x - 1.0).abs() < 1e-10);
                assert!((c.axis.as_ref().z - 1.0).abs() < 1e-10);
            }
            other => panic!("expected cylinder, got {:?}", other),
        }
    }

    #[test]
    fn test_extrusion_of_circle_against_normal_flips_sense() {
        // Extruding a CCW circle against its normal flips the STEP normal
        // inward.
        let file = fixture(
            "#1 = CARTESIAN_POINT('', (0.0, 0.0, 10.0));
#2 = DIRECTION('', (0.0, 0.0, 1.0));
#3 = DIRECTION('', (1.0, 0.0, 0.0));
#4 = AXIS2_PLACEMENT_3D('', #1, #2, #3);
#5 = CIRCLE('', #4, 6.0);
#6 = DIRECTION('', (0.0, 0.0, -1.0));
#7 = VECTOR('', #6, 10.0);
#8 = SURFACE_OF_LINEAR_EXTRUSION('', #5, #7);
",
        );
        let (surface, flipped) = parse_surface_of_linear_extrusion(&file, 8).unwrap();
        assert!(flipped);
        assert!(matches!(surface, StepSurface::Cylinder(_)));
    }

    #[test]
    fn test_extrusion_of_tilted_circle_is_nurbs() {
        // Circle extruded obliquely (not along its normal) → oblique
        // cylinder, no analytic type.
        let file = fixture(
            "#1 = CARTESIAN_POINT('', (0.0, 0.0, 0.0));
#2 = DIRECTION('', (0.0, 0.0, 1.0));
#3 = DIRECTION('', (1.0, 0.0, 0.0));
#4 = AXIS2_PLACEMENT_3D('', #1, #2, #3);
#5 = CIRCLE('', #4, 6.0);
#6 = DIRECTION('', (0.7071067811865476, 0.0, 0.7071067811865476));
#7 = VECTOR('', #6, 10.0);
#8 = SURFACE_OF_LINEAR_EXTRUSION('', #5, #7);
",
        );
        let (surface, _) = parse_surface_of_linear_extrusion(&file, 8).unwrap();
        match surface {
            StepSurface::Nurbs(n) => {
                // v=0 edge is the base circle; v=1 is translated by the vector
                let p0 = n.eval(0.0, 0.0);
                assert!((p0.x - 6.0).abs() < 1e-8 && p0.z.abs() < 1e-8);
                let p1 = n.eval(0.0, 1.0);
                let shift = 10.0 * std::f64::consts::FRAC_1_SQRT_2;
                assert!((p1.x - 6.0 - shift).abs() < 1e-8);
                assert!((p1.z - shift).abs() < 1e-8);
            }
            other => panic!("expected NURBS, got {:?}", other),
        }
    }

    #[test]
    fn test_extrusion_of_bspline_is_nurbs() {
        let file = fixture(
            "#1 = CARTESIAN_POINT('', (0.0, 0.0, 0.0));
#2 = CARTESIAN_POINT('', (5.0, 8.0, 0.0));
#3 = CARTESIAN_POINT('', (10.0, 0.0, 0.0));
#4 = B_SPLINE_CURVE_WITH_KNOTS('', 2, (#1, #2, #3),
     .UNSPECIFIED., .F., .F.,
     (3, 3), (0.0, 1.0), .UNSPECIFIED.);
#5 = DIRECTION('', (0.0, 0.0, 1.0));
#6 = VECTOR('', #5, 20.0);
#7 = SURFACE_OF_LINEAR_EXTRUSION('', #4, #6);
",
        );
        let (surface, _) = parse_surface_of_linear_extrusion(&file, 7).unwrap();
        match surface {
            StepSurface::Nurbs(n) => {
                let p0 = n.eval(0.0, 0.0);
                assert!(p0.x.abs() < 1e-8 && p0.z.abs() < 1e-8);
                let p1 = n.eval(1.0, 1.0);
                assert!((p1.x - 10.0).abs() < 1e-8 && (p1.z - 20.0).abs() < 1e-8);
            }
            other => panic!("expected NURBS, got {:?}", other),
        }
    }

    #[test]
    fn test_offset_of_cylinder_grows_radius() {
        let file = fixture(
            "#1 = CARTESIAN_POINT('', (0.0, 0.0, 0.0));
#2 = DIRECTION('', (0.0, 0.0, 1.0));
#3 = DIRECTION('', (1.0, 0.0, 0.0));
#4 = AXIS2_PLACEMENT_3D('', #1, #2, #3);
#5 = CYLINDRICAL_SURFACE('', #4, 8.0);
#6 = OFFSET_SURFACE('', #5, 2.0, .F.);
",
        );
        let (surface, flipped) = parse_surface_oriented(&file, 6).unwrap();
        assert!(!flipped);
        match surface {
            StepSurface::Cylinder(c) => assert!((c.radius - 10.0).abs() < 1e-10),
            other => panic!("expected cylinder, got {:?}", other),
        }
    }

    #[test]
    fn test_offset_of_plane_shifts_origin() {
        let file = fixture(
            "#1 = CARTESIAN_POINT('', (0.0, 0.0, 5.0));
#2 = DIRECTION('', (0.0, 0.0, 1.0));
#3 = DIRECTION('', (1.0, 0.0, 0.0));
#4 = AXIS2_PLACEMENT_3D('', #1, #2, #3);
#5 = PLANE('', #4);
#6 = OFFSET_SURFACE('', #5, -3.0, .F.);
",
        );
        let (surface, _) = parse_surface_oriented(&file, 6).unwrap();
        match surface {
            StepSurface::Plane(p) => assert!((p.origin.z - 2.0).abs() < 1e-10),
            other => panic!("expected plane, got {:?}", other),
        }
    }

    #[test]
    fn test_offset_of_sphere_and_torus() {
        let file = fixture(
            "#1 = CARTESIAN_POINT('', (0.0, 0.0, 0.0));
#2 = DIRECTION('', (0.0, 0.0, 1.0));
#3 = DIRECTION('', (1.0, 0.0, 0.0));
#4 = AXIS2_PLACEMENT_3D('', #1, #2, #3);
#5 = SPHERICAL_SURFACE('', #4, 5.0);
#6 = OFFSET_SURFACE('', #5, 1.5, .F.);
#7 = TOROIDAL_SURFACE('', #4, 10.0, 3.0);
#8 = OFFSET_SURFACE('', #7, -1.0, .F.);
",
        );
        let (sphere, _) = parse_surface_oriented(&file, 6).unwrap();
        match sphere {
            StepSurface::Sphere(s) => assert!((s.radius - 6.5).abs() < 1e-10),
            other => panic!("expected sphere, got {:?}", other),
        }
        let (torus, _) = parse_surface_oriented(&file, 8).unwrap();
        match torus {
            StepSurface::Torus(t) => {
                assert!((t.major_radius - 10.0).abs() < 1e-10);
                assert!((t.minor_radius - 2.0).abs() < 1e-10);
            }
            other => panic!("expected torus, got {:?}", other),
        }
    }

    #[test]
    fn test_offset_degenerate_is_unsupported() {
        // Offsetting a radius-8 cylinder inward by 8 collapses it.
        let file = fixture(
            "#1 = CARTESIAN_POINT('', (0.0, 0.0, 0.0));
#2 = DIRECTION('', (0.0, 0.0, 1.0));
#3 = DIRECTION('', (1.0, 0.0, 0.0));
#4 = AXIS2_PLACEMENT_3D('', #1, #2, #3);
#5 = CYLINDRICAL_SURFACE('', #4, 8.0);
#6 = OFFSET_SURFACE('', #5, -8.0, .F.);
",
        );
        assert!(matches!(
            parse_surface_oriented(&file, 6),
            Err(StepError::UnsupportedEntity(_))
        ));
    }

    #[test]
    fn test_offset_of_flipped_swept_base_negates_distance() {
        // Base: revolution of an anti-parallel line → cylinder r=10 with a
        // flipped sense (STEP normal points inward). An offset of +2 along
        // the STEP normal must therefore SHRINK the radius to 8.
        let file = fixture(
            "#1 = CARTESIAN_POINT('', (10.0, 0.0, 20.0));
#2 = DIRECTION('', (0.0, 0.0, -1.0));
#3 = VECTOR('', #2, 1.0);
#4 = LINE('', #1, #3);
#5 = CARTESIAN_POINT('', (0.0, 0.0, 0.0));
#6 = DIRECTION('', (0.0, 0.0, 1.0));
#7 = AXIS1_PLACEMENT('', #5, #6);
#8 = SURFACE_OF_REVOLUTION('', #4, #7);
#9 = OFFSET_SURFACE('', #8, 2.0, .F.);
",
        );
        let (surface, flipped) = parse_surface_oriented(&file, 9).unwrap();
        assert!(flipped, "offset inherits the base's flipped sense");
        match surface {
            StepSurface::Cylinder(c) => assert!((c.radius - 8.0).abs() < 1e-10),
            other => panic!("expected cylinder, got {:?}", other),
        }
    }

    #[test]
    fn test_offset_of_bspline_fits_nurbs() {
        // A flat bilinear B-spline patch at z=0 offset by 2 should sit at
        // z≈2 everywhere (flat base → the fit is exact up to fp error).
        let file = fixture(
            "#1 = CARTESIAN_POINT('', (0.0, 0.0, 0.0));
#2 = CARTESIAN_POINT('', (10.0, 0.0, 0.0));
#3 = CARTESIAN_POINT('', (0.0, 10.0, 0.0));
#4 = CARTESIAN_POINT('', (10.0, 10.0, 0.0));
#5 = B_SPLINE_SURFACE_WITH_KNOTS('', 1, 1, ((#1, #2), (#3, #4)),
     .UNSPECIFIED., .F., .F., .F.,
     (2, 2), (2, 2), (0.0, 1.0), (0.0, 1.0), .UNSPECIFIED.);
#6 = OFFSET_SURFACE('', #5, 2.0, .F.);
",
        );
        let (surface, _) = parse_surface_oriented(&file, 6).unwrap();
        match surface {
            StepSurface::BSpline(b) => {
                use vcad_kernel_geom::Surface as _;
                let ((u0, u1), (v0, v1)) = b.domain();
                for i in 0..=8 {
                    for j in 0..=8 {
                        let u = u0 + (u1 - u0) * i as f64 / 8.0;
                        let v = v0 + (v1 - v0) * j as f64 / 8.0;
                        let p = b.eval(u, v);
                        assert!((p.z - 2.0).abs() < 1e-8, "z at ({u},{v}): {}", p.z);
                    }
                }
            }
            other => panic!("expected B-spline fit, got {:?}", other),
        }
    }

    #[test]
    fn test_offset_surface_cycle_is_rejected() {
        // Self-referential offset must hit the depth cap, not recurse forever.
        let file = fixture("#1 = OFFSET_SURFACE('', #1, 2.0, .F.);\n");
        assert!(matches!(
            parse_surface_oriented(&file, 1),
            Err(StepError::UnsupportedEntity(_))
        ));
    }

    #[test]
    fn test_parse_surface_dispatches_new_types() {
        // The untyped entry point must route the new entities too.
        let file = fixture(
            "#1 = CARTESIAN_POINT('', (10.0, 0.0, 0.0));
#2 = DIRECTION('', (0.0, 0.0, 1.0));
#3 = VECTOR('', #2, 1.0);
#4 = LINE('', #1, #3);
#5 = CARTESIAN_POINT('', (0.0, 0.0, 0.0));
#6 = DIRECTION('', (0.0, 0.0, 1.0));
#7 = AXIS1_PLACEMENT('', #5, #6);
#8 = SURFACE_OF_REVOLUTION('', #4, #7);
",
        );
        let surface = parse_surface(&file, 8).unwrap();
        assert!(matches!(surface, StepSurface::Cylinder(_)));
    }

    #[test]
    fn test_parse_toroidal_surface() {
        let input = r#"
ISO-10303-21;
HEADER;
ENDSEC;
DATA;
#1 = CARTESIAN_POINT('', (0.0, 0.0, 5.0));
#2 = DIRECTION('', (0.0, 0.0, 1.0));
#3 = DIRECTION('', (1.0, 0.0, 0.0));
#4 = AXIS2_PLACEMENT_3D('', #1, #2, #3);
#5 = TOROIDAL_SURFACE('', #4, 10.0, 3.0);
ENDSEC;
END-ISO-10303-21;
"#;
        let file = parse_step(input);
        let torus = parse_toroidal_surface(&file, 5).unwrap();
        assert!((torus.major_radius - 10.0).abs() < 1e-10);
        assert!((torus.minor_radius - 3.0).abs() < 1e-10);
        assert!((torus.center.z - 5.0).abs() < 1e-10);
        assert!((torus.axis.as_ref().z - 1.0).abs() < 1e-10);
    }
}
