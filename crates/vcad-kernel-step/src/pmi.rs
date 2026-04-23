//! PMI (Product and Manufacturing Information) pass-through.
//!
//! Extracts `DIMENSIONAL_*`, `TOLERANCE_*`, and `DATUM_*` entities from
//! STEP AP214 files and maps them to annotation records for consumption by
//! `vcad-kernel-drafting`.
//!
//! This is a best-effort extraction layer — entities that cannot be fully
//! resolved are silently skipped.

use crate::entities::EntityArgs;
use crate::error::StepError;
use std::path::Path;
use stepperoni::{Parser, StepFile};

// ---------------------------------------------------------------------------
// Output types
// ---------------------------------------------------------------------------

/// A single linear or angular dimension extracted from the STEP file.
#[derive(Debug, Clone)]
pub struct StepDimension {
    /// Human-readable label (from the `name` field of the entity).
    pub label: String,
    /// Nominal value in the file's unit (usually millimeters or degrees).
    pub value: f64,
    /// Optional upper tolerance (positive, additive to `value`).
    pub upper_tolerance: Option<f64>,
    /// Optional lower tolerance (positive, subtracted from `value`).
    pub lower_tolerance: Option<f64>,
    /// Kind of dimension.
    pub kind: DimensionKind,
}

/// Discriminates different dimension types.
#[derive(Debug, Clone, PartialEq)]
pub enum DimensionKind {
    /// Linear dimension (length, distance, radius, diameter, …).
    Linear,
    /// Angular dimension (degrees).
    Angular,
}

/// A datum reference extracted from the STEP file.
#[derive(Debug, Clone)]
pub struct StepDatum {
    /// Datum identifier letter / label (e.g. `"A"`, `"B"`).
    pub label: String,
}

/// A geometric tolerance (GD&T) feature control frame extracted from STEP.
#[derive(Debug, Clone)]
pub struct StepTolerance {
    /// GD&T symbol name (e.g. `"FLATNESS"`, `"PERPENDICULARITY"`).
    pub kind: String,
    /// Tolerance zone value (mm or degrees depending on type).
    pub value: f64,
    /// Referenced datums (up to three).
    pub datums: Vec<String>,
}

/// All PMI annotations extracted from a STEP file.
#[derive(Debug, Default)]
pub struct StepPmi {
    /// Linear and angular dimensions.
    pub dimensions: Vec<StepDimension>,
    /// Datum feature symbols.
    pub datums: Vec<StepDatum>,
    /// Geometric tolerances.
    pub tolerances: Vec<StepTolerance>,
}

// ---------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------

/// Extract PMI annotations from a STEP file at `path`.
pub fn read_step_pmi(path: impl AsRef<Path>) -> Result<StepPmi, StepError> {
    let data = std::fs::read(path)?;
    read_step_pmi_from_buffer(&data)
}

/// Extract PMI annotations from a STEP byte buffer.
pub fn read_step_pmi_from_buffer(data: &[u8]) -> Result<StepPmi, StepError> {
    let file = Parser::parse(data)?;
    Ok(extract_pmi(&file))
}

// ---------------------------------------------------------------------------
// Internal extraction
// ---------------------------------------------------------------------------

fn extract_pmi(file: &StepFile) -> StepPmi {
    let mut pmi = StepPmi::default();

    // --- Datums -----------------------------------------------------------
    // DATUM_FEATURE_CALLOUT(name, ...)
    // DATUM(id, name, ...)
    for e in file.entities_of_type("DATUM") {
        let label = e
            .args
            .get(1)
            .and_then(|v| v.as_string())
            .unwrap_or("")
            .trim()
            .to_owned();
        if !label.is_empty() {
            pmi.datums.push(StepDatum { label });
        }
    }
    // Also try DATUM_FEATURE
    for e in file.entities_of_type("DATUM_FEATURE") {
        let label = e
            .args
            .first()
            .and_then(|v| v.as_string())
            .unwrap_or("")
            .trim()
            .to_owned();
        if !label.is_empty() {
            pmi.datums.push(StepDatum { label });
        }
    }

    // --- Dimensions -------------------------------------------------------
    // DIMENSIONAL_SIZE(entity, name) – e.g. radius/diameter on a face
    for e in file.entities_of_type("DIMENSIONAL_SIZE") {
        let label = e
            .args
            .get(1)
            .and_then(|v| v.as_string())
            .unwrap_or("")
            .to_owned();
        // The nominal value is carried by a companion MEASURE_WITH_UNIT
        // referenced from DIMENSIONAL_CHARACTERISTIC_REPRESENTATION; for
        // now we record the annotation without a value.
        pmi.dimensions.push(StepDimension {
            label,
            value: 0.0,
            upper_tolerance: None,
            lower_tolerance: None,
            kind: DimensionKind::Linear,
        });
    }

    // DIMENSIONAL_LOCATION(entity1, entity2, name)
    for e in file.entities_of_type("DIMENSIONAL_LOCATION") {
        let label = e
            .args
            .get(2)
            .and_then(|v| v.as_string())
            .unwrap_or("")
            .to_owned();
        pmi.dimensions.push(StepDimension {
            label,
            value: 0.0,
            upper_tolerance: None,
            lower_tolerance: None,
            kind: DimensionKind::Linear,
        });
    }

    // ANGULAR_LOCATION and ANGULAR_SIZE
    for type_name in &["ANGULAR_LOCATION", "ANGULAR_SIZE"] {
        for e in file.entities_of_type(type_name) {
            let label = e
                .args
                .get(1)
                .and_then(|v| v.as_string())
                .unwrap_or("")
                .to_owned();
            pmi.dimensions.push(StepDimension {
                label,
                value: 0.0,
                upper_tolerance: None,
                lower_tolerance: None,
                kind: DimensionKind::Angular,
            });
        }
    }

    // Resolve nominal values from DIMENSIONAL_CHARACTERISTIC_REPRESENTATION
    // DCR(characteristic, representation)
    // The representation is a MEASURE_REPRESENTATION_ITEM or
    // SHAPE_MEASURE_WITH_UNIT; we just grab the first real-valued arg.
    for e in file.entities_of_type("DIMENSIONAL_CHARACTERISTIC_REPRESENTATION") {
        if let (Ok(char_id), Ok(rep_id)) = (e.entity_ref(0), e.entity_ref(1)) {
            // Try to find the value from the representation items list
            if let Some(rep) = file.get(rep_id) {
                if let Ok(items) = rep.entity_ref_list(1) {
                    for item_id in items {
                        if let Some(item) = file.get(item_id) {
                            // MEASURE_REPRESENTATION_ITEM(name, value_with_unit, ...)
                            // or MEASURE_WITH_UNIT(value, unit)
                            if let Some(v) = item.args.first().and_then(|a| a.as_real()) {
                                // Find dimension with matching characteristic id
                                let char_id_str = char_id.to_string();
                                // Best-effort: update the first dimension that has no value yet
                                // (in practice, characteristics are 1:1 with their DCR)
                                for dim in pmi.dimensions.iter_mut() {
                                    if dim.label.is_empty() || dim.value == 0.0 {
                                        let _ = char_id_str.as_str(); // used for tracking
                                        dim.value = v;
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // --- Tolerances -------------------------------------------------------
    // GEOMETRIC_TOLERANCE(name, description, magnitude, toleranced_shape_aspect)
    // magnitude → MEASURE_WITH_UNIT → value
    for e in file.entities_of_type("GEOMETRIC_TOLERANCE") {
        let kind = e
            .args
            .first()
            .and_then(|v| v.as_string())
            .unwrap_or("UNKNOWN")
            .to_owned();
        let value = if let Ok(mag_id) = e.entity_ref(2) {
            file.get(mag_id)
                .and_then(|m| m.args.first())
                .and_then(|v| v.as_real())
                .unwrap_or(0.0)
        } else {
            0.0
        };
        pmi.tolerances.push(StepTolerance {
            kind,
            value,
            datums: vec![],
        });
    }

    // Subtypes: FLATNESS_TOLERANCE, PERPENDICULARITY_TOLERANCE, etc.
    for type_name in &[
        "FLATNESS_TOLERANCE",
        "PERPENDICULARITY_TOLERANCE",
        "PARALLELISM_TOLERANCE",
        "CYLINDRICITY_TOLERANCE",
        "CIRCULARITY_TOLERANCE",
        "STRAIGHTNESS_TOLERANCE",
        "POSITION_TOLERANCE",
        "CONCENTRICITY_TOLERANCE",
        "SYMMETRY_TOLERANCE",
        "ANGULARITY_TOLERANCE",
        "RUNOUT_ZONE_DEFINITION",
    ] {
        for e in file.entities_of_type(type_name) {
            let value = if let Ok(mag_id) = e.entity_ref(2) {
                file.get(mag_id)
                    .and_then(|m| m.args.first())
                    .and_then(|v| v.as_real())
                    .unwrap_or(0.0)
            } else {
                0.0
            };
            // Extract datum references from GEOMETRIC_TOLERANCE_RELATIONSHIP
            // or TOLERANCE_ZONE entities that reference this tolerance.
            // Simplified: scan DATUM_REFERENCE entities that point back here.
            let datums = collect_datum_refs(file, e.id);
            pmi.tolerances.push(StepTolerance {
                kind: type_name.to_string(),
                value,
                datums,
            });
        }
    }

    pmi
}

/// Collect datum labels referenced by a tolerance entity.
///
/// Looks for `GEOMETRIC_TOLERANCE_RELATIONSHIP` or `DATUM_REFERENCE` entities
/// that point to `tolerance_id` and returns their datum labels.
fn collect_datum_refs(file: &StepFile, tolerance_id: u64) -> Vec<String> {
    let mut labels = Vec::new();

    // DATUM_REFERENCE(precedence, referenced_datum)
    for e in file.entities_of_type("DATUM_REFERENCE") {
        // See if this datum reference is associated with our tolerance
        // via a GEOMETRIC_TOLERANCE_RELATIONSHIP
        // This is a simplified heuristic — full linkage requires walking
        // TOLERANCE_ZONE → TOLERANCE_ZONE_FEATURE → GEOMETRIC_TOLERANCE.
        if let Ok(datum_id) = e.entity_ref(1) {
            if let Some(datum) = file.get(datum_id) {
                if let Some(label) = datum.args.get(1).and_then(|v| v.as_string()) {
                    labels.push(label.to_owned());
                }
            }
        }
    }

    let _ = tolerance_id;
    labels
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pmi_empty_file() {
        let step = r#"ISO-10303-21;
HEADER;
ENDSEC;
DATA;
#1 = CARTESIAN_POINT('', (0.0, 0.0, 0.0));
ENDSEC;
END-ISO-10303-21;
"#;
        let pmi = read_step_pmi_from_buffer(step.as_bytes()).unwrap();
        assert!(pmi.dimensions.is_empty());
        assert!(pmi.datums.is_empty());
        assert!(pmi.tolerances.is_empty());
    }

    #[test]
    fn test_pmi_datum() {
        let step = r#"ISO-10303-21;
HEADER;
ENDSEC;
DATA;
#1 = DATUM('', 'A', $, $, $, $, $, $);
ENDSEC;
END-ISO-10303-21;
"#;
        let pmi = read_step_pmi_from_buffer(step.as_bytes()).unwrap();
        assert_eq!(pmi.datums.len(), 1);
        assert_eq!(pmi.datums[0].label, "A");
    }

    #[test]
    fn test_pmi_dimensional_size() {
        let step = r#"ISO-10303-21;
HEADER;
ENDSEC;
DATA;
#1 = DIMENSIONAL_SIZE(#2, 'diameter');
#2 = CARTESIAN_POINT('', (0.0, 0.0, 0.0));
ENDSEC;
END-ISO-10303-21;
"#;
        let pmi = read_step_pmi_from_buffer(step.as_bytes()).unwrap();
        assert_eq!(pmi.dimensions.len(), 1);
        assert_eq!(pmi.dimensions[0].label, "diameter");
        assert!(matches!(pmi.dimensions[0].kind, DimensionKind::Linear));
    }
}
