//! Topology entities: vertex, edge, loop, face, shell, and solid.

use super::{parse_cartesian_point, EntityArgs};
use crate::error::StepError;
use stepperoni::StepFile;
use vcad_kernel_math::Point3;

/// Parsed VERTEX_POINT entity.
#[derive(Debug, Clone)]
pub struct StepVertex {
    /// The entity ID (reserved for debugging/error reporting).
    #[allow(dead_code)]
    pub id: u64,
    /// Vertex position.
    pub point: Point3,
}

/// Parsed EDGE_CURVE entity.
#[derive(Debug, Clone)]
pub struct StepEdge {
    /// The entity ID.
    pub id: u64,
    /// Start vertex entity ID.
    pub start_vertex_id: u64,
    /// End vertex entity ID.
    pub end_vertex_id: u64,
    /// Edge geometry (curve) entity ID (reserved for curved edge support).
    #[allow(dead_code)]
    pub curve_id: u64,
    /// Whether the curve direction matches the edge direction.
    pub same_sense: bool,
}

/// Parsed ORIENTED_EDGE entity.
#[derive(Debug, Clone)]
pub struct StepOrientedEdge {
    /// The entity ID (reserved for debugging/error reporting).
    #[allow(dead_code)]
    pub id: u64,
    /// The underlying edge entity ID.
    pub edge_id: u64,
    /// Orientation of edge within the loop.
    pub orientation: bool,
}

/// Parsed EDGE_LOOP entity.
#[derive(Debug, Clone)]
pub struct StepEdgeLoop {
    /// The entity ID (reserved for debugging/error reporting).
    #[allow(dead_code)]
    pub id: u64,
    /// Oriented edges forming the loop.
    pub edge_ids: Vec<u64>,
}

/// Parsed FACE_BOUND / FACE_OUTER_BOUND entity.
#[derive(Debug, Clone)]
pub struct StepFaceBound {
    /// The entity ID (reserved for debugging/error reporting).
    #[allow(dead_code)]
    pub id: u64,
    /// The loop entity ID.
    pub loop_id: u64,
    /// Whether the bound orientation matches the face orientation (reserved for winding).
    #[allow(dead_code)]
    pub orientation: bool,
    /// Whether this is an outer bound.
    pub is_outer: bool,
}

/// Parsed ADVANCED_FACE entity.
#[derive(Debug, Clone)]
pub struct StepFace {
    /// The entity ID (reserved for debugging/error reporting).
    #[allow(dead_code)]
    pub id: u64,
    /// Face bounds (outer and inner loops).
    pub bounds: Vec<StepFaceBound>,
    /// Surface geometry entity ID.
    pub surface_id: u64,
    /// Whether the face normal matches the surface normal.
    pub same_sense: bool,
}

/// Parsed CLOSED_SHELL / OPEN_SHELL entity.
#[derive(Debug, Clone)]
pub struct StepShell {
    /// The entity ID (reserved for debugging/error reporting).
    #[allow(dead_code)]
    pub id: u64,
    /// Face entity IDs.
    pub face_ids: Vec<u64>,
    /// Whether the shell is closed.
    pub is_closed: bool,
}

/// Parsed MANIFOLD_SOLID_BREP entity.
#[derive(Debug, Clone)]
pub struct StepSolid {
    /// The entity ID (reserved for debugging/error reporting).
    #[allow(dead_code)]
    pub id: u64,
    /// The outer shell entity ID.
    pub outer_shell_id: u64,
}

/// Parse a VERTEX_POINT entity.
pub fn parse_vertex_point(file: &StepFile, id: u64) -> Result<StepVertex, StepError> {
    let entity = file.get(id).ok_or(StepError::MissingEntity(id))?;

    match entity.type_name.as_str() {
        "VERTEX_POINT" => {
            let point_id = entity.entity_ref(1)?;
            let point = parse_cartesian_point(file, point_id)?;
            Ok(StepVertex { id, point })
        }
        other => Err(StepError::type_mismatch("VERTEX_POINT", other)),
    }
}

/// Parse an EDGE_CURVE entity.
pub fn parse_edge_curve(file: &StepFile, id: u64) -> Result<StepEdge, StepError> {
    let entity = file.get(id).ok_or(StepError::MissingEntity(id))?;

    match entity.type_name.as_str() {
        "EDGE_CURVE" => {
            let start_id = entity.entity_ref(1)?;
            let end_id = entity.entity_ref(2)?;
            let curve_id = entity.entity_ref(3)?;
            let same_sense = entity.enumeration(4)? == "T";

            Ok(StepEdge {
                id,
                start_vertex_id: start_id,
                end_vertex_id: end_id,
                curve_id,
                same_sense,
            })
        }
        other => Err(StepError::type_mismatch("EDGE_CURVE", other)),
    }
}

/// Parse an ORIENTED_EDGE entity.
pub fn parse_oriented_edge(file: &StepFile, id: u64) -> Result<StepOrientedEdge, StepError> {
    let entity = file.get(id).ok_or(StepError::MissingEntity(id))?;

    match entity.type_name.as_str() {
        "ORIENTED_EDGE" => {
            // ORIENTED_EDGE has form: (name, *, *, edge_element, orientation)
            // Args 1 and 2 are usually * (derived)
            let edge_id = entity.entity_ref(3)?;
            let orientation = entity.enumeration(4)? == "T";

            Ok(StepOrientedEdge {
                id,
                edge_id,
                orientation,
            })
        }
        other => Err(StepError::type_mismatch("ORIENTED_EDGE", other)),
    }
}

/// Parse an EDGE_LOOP entity (or return empty for VERTEX_LOOP).
///
/// VERTEX_LOOP represents a degenerate boundary (single point, e.g., cone apex).
/// We return an empty edge list for these since we don't support them yet.
pub fn parse_edge_loop(file: &StepFile, id: u64) -> Result<StepEdgeLoop, StepError> {
    let entity = file.get(id).ok_or(StepError::MissingEntity(id))?;

    match entity.type_name.as_str() {
        "EDGE_LOOP" => {
            let edge_ids = entity.entity_ref_list(1)?;
            Ok(StepEdgeLoop { id, edge_ids })
        }
        "VERTEX_LOOP" => {
            // Degenerate loop (single vertex) - return empty edge list
            // This is used for cone apexes and similar degenerate boundaries
            Ok(StepEdgeLoop {
                id,
                edge_ids: vec![],
            })
        }
        other => Err(StepError::type_mismatch("EDGE_LOOP", other)),
    }
}

/// Parse a FACE_BOUND or FACE_OUTER_BOUND entity.
pub fn parse_face_bound(file: &StepFile, id: u64) -> Result<StepFaceBound, StepError> {
    let entity = file.get(id).ok_or(StepError::MissingEntity(id))?;

    match entity.type_name.as_str() {
        "FACE_BOUND" | "FACE_OUTER_BOUND" => {
            let loop_id = entity.entity_ref(1)?;
            let orientation = entity.enumeration(2)? == "T";
            let is_outer = entity.type_name == "FACE_OUTER_BOUND";

            Ok(StepFaceBound {
                id,
                loop_id,
                orientation,
                is_outer,
            })
        }
        other => Err(StepError::type_mismatch("FACE_BOUND", other)),
    }
}

/// Parse an ADVANCED_FACE entity.
pub fn parse_advanced_face(file: &StepFile, id: u64) -> Result<StepFace, StepError> {
    let entity = file.get(id).ok_or(StepError::MissingEntity(id))?;

    match entity.type_name.as_str() {
        "ADVANCED_FACE" => {
            let bound_ids = entity.entity_ref_list(1)?;
            let surface_id = entity.entity_ref(2)?;
            let same_sense = entity.enumeration(3)? == "T";

            let mut bounds = Vec::new();
            for bid in bound_ids {
                bounds.push(parse_face_bound(file, bid)?);
            }

            Ok(StepFace {
                id,
                bounds,
                surface_id,
                same_sense,
            })
        }
        other => Err(StepError::type_mismatch("ADVANCED_FACE", other)),
    }
}

/// Parse a CLOSED_SHELL or OPEN_SHELL entity.
pub fn parse_shell(file: &StepFile, id: u64) -> Result<StepShell, StepError> {
    let entity = file.get(id).ok_or(StepError::MissingEntity(id))?;

    match entity.type_name.as_str() {
        "CLOSED_SHELL" | "OPEN_SHELL" => {
            let face_ids = entity.entity_ref_list(1)?;
            let is_closed = entity.type_name == "CLOSED_SHELL";

            Ok(StepShell {
                id,
                face_ids,
                is_closed,
            })
        }
        other => Err(StepError::type_mismatch("SHELL", other)),
    }
}

/// Parse a MANIFOLD_SOLID_BREP entity.
pub fn parse_manifold_solid_brep(file: &StepFile, id: u64) -> Result<StepSolid, StepError> {
    let entity = file.get(id).ok_or(StepError::MissingEntity(id))?;

    match entity.type_name.as_str() {
        "MANIFOLD_SOLID_BREP" => {
            let outer_shell_id = entity.entity_ref(1)?;
            Ok(StepSolid { id, outer_shell_id })
        }
        other => Err(StepError::type_mismatch("MANIFOLD_SOLID_BREP", other)),
    }
}

/// Write a VERTEX_POINT to STEP format.
pub fn write_vertex_point(name: &str, point_id: u64) -> String {
    format!("VERTEX_POINT('{}', #{})", name, point_id)
}

/// Write an EDGE_CURVE to STEP format.
pub fn write_edge_curve(
    name: &str,
    start_id: u64,
    end_id: u64,
    curve_id: u64,
    same_sense: bool,
) -> String {
    let sense = if same_sense { ".T." } else { ".F." };
    format!(
        "EDGE_CURVE('{}', #{}, #{}, #{}, {})",
        name, start_id, end_id, curve_id, sense
    )
}

/// Write an ORIENTED_EDGE to STEP format.
pub fn write_oriented_edge(name: &str, edge_id: u64, orientation: bool) -> String {
    let orient = if orientation { ".T." } else { ".F." };
    format!("ORIENTED_EDGE('{}', *, *, #{}, {})", name, edge_id, orient)
}

/// Write an EDGE_LOOP to STEP format.
pub fn write_edge_loop(name: &str, edge_ids: &[u64]) -> String {
    let refs: Vec<String> = edge_ids.iter().map(|id| format!("#{id}")).collect();
    format!("EDGE_LOOP('{}', ({}))", name, refs.join(", "))
}

/// Write a FACE_BOUND to STEP format.
pub fn write_face_bound(name: &str, loop_id: u64, orientation: bool, is_outer: bool) -> String {
    let orient = if orientation { ".T." } else { ".F." };
    let entity_type = if is_outer {
        "FACE_OUTER_BOUND"
    } else {
        "FACE_BOUND"
    };
    format!("{}('{}', #{}, {})", entity_type, name, loop_id, orient)
}

/// Write an ADVANCED_FACE to STEP format.
pub fn write_advanced_face(
    name: &str,
    bound_ids: &[u64],
    surface_id: u64,
    same_sense: bool,
) -> String {
    let refs: Vec<String> = bound_ids.iter().map(|id| format!("#{id}")).collect();
    let sense = if same_sense { ".T." } else { ".F." };
    format!(
        "ADVANCED_FACE('{}', ({}), #{}, {})",
        name,
        refs.join(", "),
        surface_id,
        sense
    )
}

/// Write a CLOSED_SHELL to STEP format.
pub fn write_closed_shell(name: &str, face_ids: &[u64]) -> String {
    let refs: Vec<String> = face_ids.iter().map(|id| format!("#{id}")).collect();
    format!("CLOSED_SHELL('{}', ({}))", name, refs.join(", "))
}

/// Write a MANIFOLD_SOLID_BREP to STEP format.
pub fn write_manifold_solid_brep(name: &str, shell_id: u64) -> String {
    format!("MANIFOLD_SOLID_BREP('{}', #{})", name, shell_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use stepperoni::Parser;

    fn parse_step(input: &str) -> StepFile {
        Parser::parse(input.as_bytes()).unwrap()
    }

    #[test]
    fn test_parse_vertex_point() {
        let input = r#"
ISO-10303-21;
HEADER;
ENDSEC;
DATA;
#1 = CARTESIAN_POINT('', (1.0, 2.0, 3.0));
#2 = VERTEX_POINT('', #1);
ENDSEC;
END-ISO-10303-21;
"#;
        let file = parse_step(input);
        let vertex = parse_vertex_point(&file, 2).unwrap();
        assert!((vertex.point.x - 1.0).abs() < 1e-10);
    }

    #[test]
    fn test_parse_edge_loop() {
        let input = r#"
ISO-10303-21;
HEADER;
ENDSEC;
DATA;
#1 = EDGE_LOOP('', (#10, #11, #12, #13));
ENDSEC;
END-ISO-10303-21;
"#;
        let file = parse_step(input);
        let loop_ = parse_edge_loop(&file, 1).unwrap();
        assert_eq!(loop_.edge_ids.len(), 4);
        assert_eq!(loop_.edge_ids[0], 10);
    }

    #[test]
    fn test_parse_advanced_face() {
        let input = r#"
ISO-10303-21;
HEADER;
ENDSEC;
DATA;
#1 = CARTESIAN_POINT('', (0.0, 0.0, 0.0));
#2 = DIRECTION('', (0.0, 0.0, 1.0));
#3 = DIRECTION('', (1.0, 0.0, 0.0));
#4 = AXIS2_PLACEMENT_3D('', #1, #2, #3);
#5 = PLANE('', #4);
#6 = EDGE_LOOP('', (#10, #11, #12, #13));
#7 = FACE_OUTER_BOUND('', #6, .T.);
#8 = ADVANCED_FACE('', (#7), #5, .T.);
ENDSEC;
END-ISO-10303-21;
"#;
        let file = parse_step(input);
        let face = parse_advanced_face(&file, 8).unwrap();
        assert_eq!(face.surface_id, 5);
        assert!(face.same_sense);
        assert_eq!(face.bounds.len(), 1);
        assert!(face.bounds[0].is_outer);
    }

    #[test]
    fn test_write_functions() {
        assert!(write_vertex_point("", 1).contains("VERTEX_POINT"));
        assert!(write_edge_curve("", 1, 2, 3, true).contains("EDGE_CURVE"));
        assert!(write_edge_loop("", &[1, 2, 3]).contains("EDGE_LOOP"));
        assert!(write_closed_shell("", &[1, 2]).contains("CLOSED_SHELL"));
        assert!(write_manifold_solid_brep("", 1).contains("MANIFOLD_SOLID_BREP"));
    }
}
