//! Surface entities: planes, cylinders, cones, spheres, B-splines.

use super::{
    parse_any_axis_placement, parse_cartesian_point, write_cartesian_point, AxisPlacement,
    EntityArgs,
};
use crate::error::StepError;
use stepperoni::{StepFile, StepValue};
use vcad_kernel_geom::{
    BilinearSurface, ConeSurface, CylinderSurface, Plane, SphereSurface, Surface, TorusSurface,
};
use vcad_kernel_math::Point3;
use vcad_kernel_nurbs::BSplineSurface;

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
        }
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

    if control_points.len() != n_u * n_v {
        return Err(StepError::UnsupportedEntity(format!(
            "B_SPLINE_SURFACE #{} (control point mismatch: {} != {}x{})",
            id,
            control_points.len(),
            n_u,
            n_v
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
        let m = mult
            .as_integer()
            .ok_or_else(|| StepError::parser(None, "invalid multiplicity"))?
            as usize;
        for _ in 0..m {
            result.push(k);
        }
    }
    Ok(result)
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

/// Parse any supported surface entity.
pub fn parse_surface(file: &StepFile, id: u64) -> Result<StepSurface, StepError> {
    let entity = file.get(id).ok_or(StepError::MissingEntity(id))?;

    match entity.type_name.as_str() {
        "PLANE" => Ok(StepSurface::Plane(parse_plane(file, id)?)),
        "CYLINDRICAL_SURFACE" => Ok(StepSurface::Cylinder(parse_cylindrical_surface(file, id)?)),
        "CONICAL_SURFACE" => Ok(StepSurface::Cone(parse_conical_surface(file, id)?)),
        "SPHERICAL_SURFACE" => Ok(StepSurface::Sphere(parse_spherical_surface(file, id)?)),
        "TOROIDAL_SURFACE" => Ok(StepSurface::Torus(parse_toroidal_surface(file, id)?)),
        "B_SPLINE_SURFACE_WITH_KNOTS" => Ok(StepSurface::BSpline(parse_bspline_surface(file, id)?)),
        // Complex entities often have BOUNDED_SURFACE as first type
        "BOUNDED_SURFACE" | "B_SPLINE_SURFACE" => {
            // Check if it's a complex entity with B-spline data
            let has_bspline = entity.args.iter().any(|arg| {
                matches!(arg, StepValue::Typed { type_name, .. }
                    if type_name == "B_SPLINE_SURFACE_WITH_KNOTS" || type_name == "B_SPLINE_SURFACE")
            });
            if has_bspline {
                Ok(StepSurface::BSpline(parse_bspline_surface(file, id)?))
            } else {
                Err(StepError::UnsupportedEntity(entity.type_name.clone()))
            }
        }
        other => Err(StepError::UnsupportedEntity(other.to_string())),
    }
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
