//! Unified feature schema — single source of truth for all feature kinds.
//!
//! `FeatureInput` defines every feature kind and its typed parameters.
//! Encoding (`to_crdt_params`) and decoding (`from_crdt_params`) are both
//! derived from this enum, eliminating the duplicated schema between
//! TypeScript and Rust.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use vcad_crdt::{Value, HLC};

/// Feature parameters stored in a CRDT feature.
pub type FeatureParams = HashMap<String, (Value, HLC)>;

/// Boolean operation type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BooleanType {
    /// Union (add).
    Union,
    /// Difference (subtract).
    Difference,
    /// Intersection.
    Intersection,
}

impl BooleanType {
    /// Convert to the string representation used in CRDT params.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Union => "union",
            Self::Difference => "difference",
            Self::Intersection => "intersection",
        }
    }

    /// Parse from a CRDT param string.
    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "difference" => Self::Difference,
            "intersection" => Self::Intersection,
            _ => Self::Union,
        }
    }
}

/// Every feature kind and its typed parameters.
///
/// This is THE schema — encoding, decoding, validation, and the WASM API
/// are all derived from this enum.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum FeatureInput {
    /// Box primitive.
    Cube {
        /// Width (X).
        size_x: f64,
        /// Depth (Y).
        size_y: f64,
        /// Height (Z).
        size_z: f64,
    },
    /// Cylinder primitive.
    Cylinder {
        /// Radius.
        radius: f64,
        /// Height along Z.
        height: f64,
        /// Tessellation segments.
        #[serde(skip_serializing_if = "Option::is_none")]
        segments: Option<u32>,
    },
    /// Sphere primitive.
    Sphere {
        /// Radius.
        radius: f64,
        /// Tessellation segments.
        #[serde(skip_serializing_if = "Option::is_none")]
        segments: Option<u32>,
    },
    /// Cone/frustum primitive.
    Cone {
        /// Bottom radius.
        radius_bottom: f64,
        /// Top radius (0 for a point).
        radius_top: f64,
        /// Height along Z.
        height: f64,
        /// Tessellation segments.
        #[serde(skip_serializing_if = "Option::is_none")]
        segments: Option<u32>,
    },
    /// Boolean operation between two features.
    Boolean {
        /// Union, difference, or intersection.
        boolean_type: BooleanType,
        /// First input feature ID.
        input_a: String,
        /// Second input feature ID.
        input_b: String,
    },
    /// Extrude a sketch profile.
    Extrude {
        /// Sketch data (JSON-serialized CsgOp::Sketch2D).
        sketch: String,
        /// Extrusion depth.
        depth: f64,
        /// Extrusion direction (unit vector).
        direction: [f64; 3],
        /// Optional twist angle (degrees).
        #[serde(skip_serializing_if = "Option::is_none")]
        twist_angle: Option<f64>,
        /// Optional end scale factor.
        #[serde(skip_serializing_if = "Option::is_none")]
        scale_end: Option<f64>,
    },
    /// Revolve a sketch profile around an axis.
    Revolve {
        /// Sketch data (JSON-serialized CsgOp::Sketch2D).
        sketch: String,
        /// Axis origin point.
        axis_origin: [f64; 3],
        /// Axis direction vector.
        axis_dir: [f64; 3],
        /// Angle of revolution (degrees).
        angle_deg: f64,
    },
    /// Sweep a sketch along a path.
    Sweep {
        /// Sketch data (JSON-serialized CsgOp::Sketch2D).
        sketch: String,
        /// Path curve (JSON-serialized PathCurve).
        #[serde(skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        /// Optional twist angle.
        #[serde(skip_serializing_if = "Option::is_none")]
        twist_angle: Option<f64>,
        /// Optional start scale.
        #[serde(skip_serializing_if = "Option::is_none")]
        scale_start: Option<f64>,
        /// Optional end scale.
        #[serde(skip_serializing_if = "Option::is_none")]
        scale_end: Option<f64>,
    },
    /// Loft between multiple sketch profiles.
    Loft {
        /// Sketch data for each profile (JSON-serialized CsgOp::Sketch2D).
        profiles: Vec<String>,
        /// Whether the loft is closed (wraps around).
        #[serde(skip_serializing_if = "Option::is_none")]
        closed: Option<bool>,
    },
    /// Fillet (round edges).
    Fillet {
        /// Source feature ID.
        input: String,
        /// Fillet radius.
        radius: f64,
    },
    /// Chamfer (bevel edges).
    Chamfer {
        /// Source feature ID.
        input: String,
        /// Chamfer distance.
        distance: f64,
    },
    /// Shell (hollow out a solid).
    Shell {
        /// Source feature ID.
        input: String,
        /// Wall thickness.
        thickness: f64,
    },
    /// Linear pattern (array of copies along a direction).
    LinearPattern {
        /// Source feature ID.
        input: String,
        /// Pattern direction vector.
        direction: [f64; 3],
        /// Number of copies.
        count: u32,
        /// Spacing between copies.
        spacing: f64,
    },
    /// Circular pattern (array of copies around an axis).
    CircularPattern {
        /// Source feature ID.
        input: String,
        /// Axis origin point.
        axis_origin: [f64; 3],
        /// Axis direction vector.
        axis_dir: [f64; 3],
        /// Number of copies.
        count: u32,
        /// Total angle (degrees).
        angle_deg: f64,
    },
    /// Mirror across a plane.
    Mirror {
        /// Source feature ID.
        input: String,
        /// Mirror plane ("XY", "XZ", or "YZ").
        plane: String,
    },
    /// 3D text (extruded from 2D glyphs).
    Text {
        /// The text content.
        text: String,
        /// Font height.
        height: f64,
        /// Extrusion depth.
        depth: f64,
        /// Text alignment ("left", "center", "right").
        #[serde(skip_serializing_if = "Option::is_none")]
        alignment: Option<String>,
        /// Letter spacing multiplier.
        #[serde(skip_serializing_if = "Option::is_none")]
        letter_spacing: Option<f64>,
        /// Line spacing multiplier.
        #[serde(skip_serializing_if = "Option::is_none")]
        line_spacing: Option<f64>,
    },
    /// Imported triangle mesh.
    ImportedMesh {
        /// Vertex positions as JSON array of f64.
        positions_json: String,
        /// Triangle indices as JSON array of u32.
        indices_json: String,
        /// Vertex normals as JSON array of f64.
        #[serde(skip_serializing_if = "Option::is_none")]
        normals_json: Option<String>,
        /// Source filename.
        #[serde(skip_serializing_if = "Option::is_none")]
        source: Option<String>,
    },
    /// PCB board.
    PcbBoard {
        /// Board data (JSON-serialized Pcb).
        #[serde(skip_serializing_if = "Option::is_none")]
        board: Option<String>,
    },
    /// Embroidery pattern.
    EmbroideryPattern {
        /// Design data (JSON-serialized EmbroideryDesign).
        #[serde(skip_serializing_if = "Option::is_none")]
        design: Option<String>,
        /// Source filename.
        #[serde(skip_serializing_if = "Option::is_none")]
        source: Option<String>,
    },
    /// Stdlib or user-published part instance.
    ///
    /// Materializes to [`vcad_ir::CsgOp::PartInstance`] with the same
    /// path / version / params. The engine expands the node at
    /// evaluation time by calling the kernel's `buildPart` export.
    PartInstance {
        /// Part source path, e.g. `"std:fastener.bolt.socket-head"`.
        path: String,
        /// Pinned version string, e.g. `"1.0"`.
        version: String,
        /// Parameters as a JSON-serialized object.
        params_json: String,
    },
    /// Assembly: part definition.
    PartDef {
        /// Source feature ID that defines the geometry.
        source_feature: String,
        /// Optional human-readable name.
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    /// Assembly: instance of a part definition.
    Instance {
        /// Part definition feature ID.
        part_def: String,
        /// Optional name.
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        /// Transform data (JSON-serialized Transform3D).
        #[serde(skip_serializing_if = "Option::is_none")]
        transform: Option<String>,
    },
    /// Assembly: joint between instances.
    Joint {
        /// Joint kind ("Fixed", "Revolute", "Slider", "Cylindrical", "Ball").
        kind: String,
        /// Child instance feature ID.
        child_instance: String,
        /// Parent instance feature ID.
        #[serde(skip_serializing_if = "Option::is_none")]
        parent_instance: Option<String>,
        /// Parent anchor point.
        anchor_a: [f64; 3],
        /// Child anchor point.
        anchor_b: [f64; 3],
        /// Joint axis (for revolute/slider/cylindrical).
        #[serde(skip_serializing_if = "Option::is_none")]
        axis: Option<[f64; 3]>,
        /// Optional name.
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        /// Joint limits (JSON-serialized JointLimits).
        #[serde(skip_serializing_if = "Option::is_none")]
        limits: Option<String>,
    },
    /// Scene rendering settings.
    SceneSettings {
        /// Environment settings (JSON).
        #[serde(skip_serializing_if = "Option::is_none")]
        environment: Option<String>,
        /// Light definitions (JSON).
        #[serde(skip_serializing_if = "Option::is_none")]
        lights: Option<String>,
        /// Background settings (JSON).
        #[serde(skip_serializing_if = "Option::is_none")]
        background: Option<String>,
        /// Post-processing settings (JSON).
        #[serde(skip_serializing_if = "Option::is_none")]
        post_processing: Option<String>,
        /// Camera presets (JSON).
        #[serde(skip_serializing_if = "Option::is_none")]
        camera_presets: Option<String>,
    },
    /// Schematic sheet.
    Schematic {
        /// Sheet title.
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        /// Sheet data (JSON-serialized SchematicSheet).
        #[serde(skip_serializing_if = "Option::is_none")]
        sheet: Option<String>,
    },
    /// Drawing sheet settings (title block, section lines, BOM visibility).
    DrawingSettings {
        /// Title block fields (JSON-serialized `DrawingTitleBlock`).
        #[serde(skip_serializing_if = "Option::is_none")]
        title_block: Option<String>,
        /// Section lines (JSON-serialized `Vec<DrawingSectionLine>`).
        #[serde(skip_serializing_if = "Option::is_none")]
        sections: Option<String>,
        /// BOM table visibility (JSON bool).
        #[serde(skip_serializing_if = "Option::is_none")]
        show_bom: Option<String>,
    },
    /// Atomic / molecular system (the `molecule` document domain).
    Molecule {
        /// System data (JSON-serialized `MoleculeSystem`).
        #[serde(skip_serializing_if = "Option::is_none")]
        system: Option<String>,
    },
    /// Persisted Analyze-mode solver studies (singleton, like scene
    /// settings): JSON-serialized `Vec<AnalysisStudy>`.
    AnalysisStudies {
        /// Studies data (JSON-serialized `Vec<AnalysisStudy>`).
        #[serde(skip_serializing_if = "Option::is_none")]
        studies: Option<String>,
    },
    /// Persisted document-level design constraints (singleton, like scene
    /// settings): JSON-serialized `Vec<DesignConstraint>`.
    DesignConstraints {
        /// Constraints data (JSON-serialized `Vec<DesignConstraint>`).
        #[serde(skip_serializing_if = "Option::is_none")]
        constraints: Option<String>,
    },
    /// Document animation timeline (singleton, like scene settings):
    /// JSON-serialized `Timeline`.
    Timeline {
        /// Timeline data (JSON-serialized `vcad_ir::Timeline`).
        #[serde(skip_serializing_if = "Option::is_none")]
        timeline: Option<String>,
    },
}

impl FeatureInput {
    /// Get the CRDT feature kind string for this input.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Cube { .. } => "cube",
            Self::Cylinder { .. } => "cylinder",
            Self::Sphere { .. } => "sphere",
            Self::Cone { .. } => "cone",
            Self::Boolean { .. } => "boolean",
            Self::Extrude { .. } => "extrude",
            Self::Revolve { .. } => "revolve",
            Self::Sweep { .. } => "sweep",
            Self::Loft { .. } => "loft",
            Self::Fillet { .. } => "fillet",
            Self::Chamfer { .. } => "chamfer",
            Self::Shell { .. } => "shell",
            Self::LinearPattern { .. } => "linear-pattern",
            Self::CircularPattern { .. } => "circular-pattern",
            Self::Mirror { .. } => "mirror",
            Self::Text { .. } => "text",
            Self::ImportedMesh { .. } => "imported-mesh",
            Self::PcbBoard { .. } => "pcb-board",
            Self::EmbroideryPattern { .. } => "embroidery-pattern",
            Self::PartInstance { .. } => "part-instance",
            Self::PartDef { .. } => "part-def",
            Self::Instance { .. } => "instance",
            Self::Joint { .. } => "joint",
            Self::SceneSettings { .. } => "scene-settings",
            Self::DrawingSettings { .. } => "drawing-settings",
            Self::Schematic { .. } => "schematic",
            Self::Molecule { .. } => "molecule",
            Self::AnalysisStudies { .. } => "analysis-studies",
            Self::DesignConstraints { .. } => "design-constraints",
            Self::Timeline { .. } => "timeline",
        }
    }

    /// Convert to CRDT parameters for storage.
    ///
    /// Returns `(kind, params)` where kind is the feature type string
    /// and params is a map of parameter names to CRDT values.
    pub fn to_crdt_params(&self) -> (&'static str, HashMap<String, Value>) {
        let kind = self.kind();
        let mut p = HashMap::new();

        match self {
            Self::Cube {
                size_x,
                size_y,
                size_z,
            } => {
                p.insert("size_x".into(), Value::F64(*size_x));
                p.insert("size_y".into(), Value::F64(*size_y));
                p.insert("size_z".into(), Value::F64(*size_z));
            }
            Self::Cylinder {
                radius,
                height,
                segments,
            } => {
                p.insert("radius".into(), Value::F64(*radius));
                p.insert("height".into(), Value::F64(*height));
                if let Some(s) = segments {
                    p.insert("segments".into(), Value::F64(*s as f64));
                }
            }
            Self::Sphere { radius, segments } => {
                p.insert("radius".into(), Value::F64(*radius));
                if let Some(s) = segments {
                    p.insert("segments".into(), Value::F64(*s as f64));
                }
            }
            Self::Cone {
                radius_bottom,
                radius_top,
                height,
                segments,
            } => {
                p.insert("radius_bottom".into(), Value::F64(*radius_bottom));
                p.insert("radius_top".into(), Value::F64(*radius_top));
                p.insert("height".into(), Value::F64(*height));
                if let Some(s) = segments {
                    p.insert("segments".into(), Value::F64(*s as f64));
                }
            }
            Self::Boolean {
                boolean_type,
                input_a,
                input_b,
            } => {
                p.insert(
                    "boolean_type".into(),
                    Value::String(boolean_type.as_str().into()),
                );
                p.insert("input_a".into(), Value::FeatureRef(input_a.clone()));
                p.insert("input_b".into(), Value::FeatureRef(input_b.clone()));
            }
            Self::Extrude {
                sketch,
                depth,
                direction,
                twist_angle,
                scale_end,
            } => {
                p.insert("sketch".into(), Value::String(sketch.clone()));
                p.insert("depth".into(), Value::F64(*depth));
                p.insert("direction".into(), Value::Vec3(*direction));
                if let Some(v) = twist_angle {
                    p.insert("twist_angle".into(), Value::F64(*v));
                }
                if let Some(v) = scale_end {
                    p.insert("scale_end".into(), Value::F64(*v));
                }
            }
            Self::Revolve {
                sketch,
                axis_origin,
                axis_dir,
                angle_deg,
            } => {
                p.insert("sketch".into(), Value::String(sketch.clone()));
                p.insert("axis_origin".into(), Value::Vec3(*axis_origin));
                p.insert("axis_dir".into(), Value::Vec3(*axis_dir));
                p.insert("angle_deg".into(), Value::F64(*angle_deg));
            }
            Self::Sweep {
                sketch,
                path,
                twist_angle,
                scale_start,
                scale_end,
            } => {
                p.insert("sketch".into(), Value::String(sketch.clone()));
                if let Some(v) = path {
                    p.insert("path".into(), Value::String(v.clone()));
                }
                if let Some(v) = twist_angle {
                    p.insert("twist_angle".into(), Value::F64(*v));
                }
                if let Some(v) = scale_start {
                    p.insert("scale_start".into(), Value::F64(*v));
                }
                if let Some(v) = scale_end {
                    p.insert("scale_end".into(), Value::F64(*v));
                }
            }
            Self::Loft { profiles, closed } => {
                p.insert("sketch_count".into(), Value::F64(profiles.len() as f64));
                for (i, profile) in profiles.iter().enumerate() {
                    p.insert(format!("sketch_{i}"), Value::String(profile.clone()));
                }
                if let Some(c) = closed {
                    p.insert("closed".into(), Value::Bool(*c));
                }
            }
            Self::Fillet { input, radius } => {
                p.insert("input".into(), Value::FeatureRef(input.clone()));
                p.insert("radius".into(), Value::F64(*radius));
            }
            Self::Chamfer { input, distance } => {
                p.insert("input".into(), Value::FeatureRef(input.clone()));
                p.insert("distance".into(), Value::F64(*distance));
            }
            Self::Shell { input, thickness } => {
                p.insert("input".into(), Value::FeatureRef(input.clone()));
                p.insert("thickness".into(), Value::F64(*thickness));
            }
            Self::LinearPattern {
                input,
                direction,
                count,
                spacing,
            } => {
                p.insert("input".into(), Value::FeatureRef(input.clone()));
                p.insert("direction".into(), Value::Vec3(*direction));
                p.insert("count".into(), Value::F64(*count as f64));
                p.insert("spacing".into(), Value::F64(*spacing));
            }
            Self::CircularPattern {
                input,
                axis_origin,
                axis_dir,
                count,
                angle_deg,
            } => {
                p.insert("input".into(), Value::FeatureRef(input.clone()));
                p.insert("axis_origin".into(), Value::Vec3(*axis_origin));
                p.insert("axis_dir".into(), Value::Vec3(*axis_dir));
                p.insert("count".into(), Value::F64(*count as f64));
                p.insert("angle_deg".into(), Value::F64(*angle_deg));
            }
            Self::Mirror { input, plane } => {
                p.insert("input".into(), Value::FeatureRef(input.clone()));
                p.insert("plane".into(), Value::String(plane.clone()));
            }
            Self::Text {
                text,
                height,
                depth,
                alignment,
                letter_spacing,
                line_spacing,
            } => {
                p.insert("text".into(), Value::String(text.clone()));
                p.insert("height".into(), Value::F64(*height));
                p.insert("depth".into(), Value::F64(*depth));
                if let Some(v) = alignment {
                    p.insert("alignment".into(), Value::String(v.clone()));
                }
                if let Some(v) = letter_spacing {
                    p.insert("letter_spacing".into(), Value::F64(*v));
                }
                if let Some(v) = line_spacing {
                    p.insert("line_spacing".into(), Value::F64(*v));
                }
            }
            Self::ImportedMesh {
                positions_json,
                indices_json,
                normals_json,
                source,
            } => {
                p.insert(
                    "positions_json".into(),
                    Value::String(positions_json.clone()),
                );
                p.insert("indices_json".into(), Value::String(indices_json.clone()));
                if let Some(v) = normals_json {
                    p.insert("normals_json".into(), Value::String(v.clone()));
                }
                if let Some(v) = source {
                    p.insert("source".into(), Value::String(v.clone()));
                }
            }
            Self::PcbBoard { board } => {
                if let Some(v) = board {
                    p.insert("board".into(), Value::String(v.clone()));
                }
            }
            Self::EmbroideryPattern { design, source } => {
                if let Some(v) = design {
                    p.insert("design".into(), Value::String(v.clone()));
                }
                if let Some(v) = source {
                    p.insert("source".into(), Value::String(v.clone()));
                }
            }
            Self::PartInstance {
                path,
                version,
                params_json,
            } => {
                p.insert("path".into(), Value::String(path.clone()));
                p.insert("version".into(), Value::String(version.clone()));
                p.insert("params_json".into(), Value::String(params_json.clone()));
            }
            Self::PartDef {
                source_feature,
                name,
            } => {
                p.insert(
                    "source_feature".into(),
                    Value::FeatureRef(source_feature.clone()),
                );
                if let Some(v) = name {
                    p.insert("name".into(), Value::String(v.clone()));
                }
            }
            Self::Instance {
                part_def,
                name,
                transform,
            } => {
                p.insert("part_def".into(), Value::FeatureRef(part_def.clone()));
                if let Some(v) = name {
                    p.insert("name".into(), Value::String(v.clone()));
                }
                if let Some(v) = transform {
                    p.insert("transform".into(), Value::String(v.clone()));
                }
            }
            Self::Joint {
                kind,
                child_instance,
                parent_instance,
                anchor_a,
                anchor_b,
                axis,
                name,
                limits,
            } => {
                p.insert("kind".into(), Value::String(kind.clone()));
                p.insert(
                    "instance_b".into(),
                    Value::FeatureRef(child_instance.clone()),
                );
                if let Some(v) = parent_instance {
                    p.insert("instance_a".into(), Value::FeatureRef(v.clone()));
                }
                p.insert("anchor_a".into(), Value::Vec3(*anchor_a));
                p.insert("anchor_b".into(), Value::Vec3(*anchor_b));
                if let Some(v) = axis {
                    p.insert("axis".into(), Value::Vec3(*v));
                }
                if let Some(v) = name {
                    p.insert("name".into(), Value::String(v.clone()));
                }
                if let Some(v) = limits {
                    p.insert("limits".into(), Value::String(v.clone()));
                }
            }
            Self::SceneSettings {
                environment,
                lights,
                background,
                post_processing,
                camera_presets,
            } => {
                if let Some(v) = environment {
                    p.insert("environment".into(), Value::String(v.clone()));
                }
                if let Some(v) = lights {
                    p.insert("lights".into(), Value::String(v.clone()));
                }
                if let Some(v) = background {
                    p.insert("background".into(), Value::String(v.clone()));
                }
                if let Some(v) = post_processing {
                    p.insert("post_processing".into(), Value::String(v.clone()));
                }
                if let Some(v) = camera_presets {
                    p.insert("camera_presets".into(), Value::String(v.clone()));
                }
            }
            Self::DrawingSettings {
                title_block,
                sections,
                show_bom,
            } => {
                if let Some(v) = title_block {
                    p.insert("title_block".into(), Value::String(v.clone()));
                }
                if let Some(v) = sections {
                    p.insert("sections".into(), Value::String(v.clone()));
                }
                if let Some(v) = show_bom {
                    p.insert("show_bom".into(), Value::String(v.clone()));
                }
            }
            Self::Schematic { title, sheet } => {
                if let Some(v) = title {
                    p.insert("title".into(), Value::String(v.clone()));
                }
                if let Some(v) = sheet {
                    p.insert("sheet".into(), Value::String(v.clone()));
                }
            }
            Self::Molecule { system } => {
                if let Some(v) = system {
                    p.insert("system".into(), Value::String(v.clone()));
                }
            }
            Self::AnalysisStudies { studies } => {
                if let Some(v) = studies {
                    p.insert("studies".into(), Value::String(v.clone()));
                }
            }
            Self::DesignConstraints { constraints } => {
                if let Some(v) = constraints {
                    p.insert("constraints".into(), Value::String(v.clone()));
                }
            }
            Self::Timeline { timeline } => {
                if let Some(v) = timeline {
                    p.insert("timeline".into(), Value::String(v.clone()));
                }
            }
        }

        (kind, p)
    }

    /// Reconstruct a `FeatureInput` from CRDT feature kind + params.
    ///
    /// Returns `None` if the kind is unrecognized.
    pub fn from_crdt_params(kind: &str, params: &FeatureParams) -> Option<Self> {
        Some(match kind {
            "cube" => Self::Cube {
                size_x: get_f64(params, "size_x").unwrap_or(10.0),
                size_y: get_f64(params, "size_y").unwrap_or(10.0),
                size_z: get_f64(params, "size_z").unwrap_or(10.0),
            },
            "cylinder" => Self::Cylinder {
                radius: get_f64(params, "radius").unwrap_or(5.0),
                height: get_f64(params, "height").unwrap_or(10.0),
                segments: get_f64(params, "segments").map(|v| v as u32),
            },
            "sphere" => Self::Sphere {
                radius: get_f64(params, "radius").unwrap_or(5.0),
                segments: get_f64(params, "segments").map(|v| v as u32),
            },
            "cone" => Self::Cone {
                radius_bottom: get_f64(params, "radius_bottom").unwrap_or(5.0),
                radius_top: get_f64(params, "radius_top").unwrap_or(0.0),
                height: get_f64(params, "height").unwrap_or(10.0),
                segments: get_f64(params, "segments").map(|v| v as u32),
            },
            "boolean" => Self::Boolean {
                boolean_type: BooleanType::from_str_lossy(
                    &get_str(params, "boolean_type").unwrap_or_default(),
                ),
                input_a: get_str(params, "input_a").unwrap_or_default(),
                input_b: get_str(params, "input_b").unwrap_or_default(),
            },
            "extrude" => Self::Extrude {
                sketch: get_str(params, "sketch").unwrap_or_default(),
                depth: get_f64(params, "depth").unwrap_or(10.0),
                direction: get_vec3(params, "direction").unwrap_or([0.0, 0.0, 1.0]),
                twist_angle: get_f64(params, "twist_angle"),
                scale_end: get_f64(params, "scale_end"),
            },
            "revolve" => Self::Revolve {
                sketch: get_str(params, "sketch").unwrap_or_default(),
                axis_origin: get_vec3(params, "axis_origin").unwrap_or([0.0, 0.0, 0.0]),
                axis_dir: get_vec3(params, "axis_dir").unwrap_or([0.0, 1.0, 0.0]),
                angle_deg: get_f64(params, "angle_deg").unwrap_or(360.0),
            },
            "sweep" => Self::Sweep {
                sketch: get_str(params, "sketch").unwrap_or_default(),
                path: get_str(params, "path"),
                twist_angle: get_f64(params, "twist_angle"),
                scale_start: get_f64(params, "scale_start"),
                scale_end: get_f64(params, "scale_end"),
            },
            "loft" => {
                let count = get_f64(params, "sketch_count")
                    .map(|v| v as usize)
                    .unwrap_or(0);
                let profiles = (0..count)
                    .map(|i| get_str(params, &format!("sketch_{i}")).unwrap_or_default())
                    .collect();
                Self::Loft {
                    profiles,
                    closed: get_bool(params, "closed"),
                }
            }
            "fillet" => Self::Fillet {
                input: get_str(params, "input").unwrap_or_default(),
                radius: get_f64(params, "radius").unwrap_or(1.0),
            },
            "chamfer" => Self::Chamfer {
                input: get_str(params, "input").unwrap_or_default(),
                distance: get_f64(params, "distance").unwrap_or(1.0),
            },
            "shell" => Self::Shell {
                input: get_str(params, "input").unwrap_or_default(),
                thickness: get_f64(params, "thickness").unwrap_or(1.0),
            },
            "linear-pattern" => Self::LinearPattern {
                input: get_str(params, "input").unwrap_or_default(),
                direction: get_vec3(params, "direction").unwrap_or([1.0, 0.0, 0.0]),
                count: get_f64(params, "count").map(|v| v as u32).unwrap_or(3),
                spacing: get_f64(params, "spacing").unwrap_or(20.0),
            },
            "circular-pattern" => Self::CircularPattern {
                input: get_str(params, "input").unwrap_or_default(),
                axis_origin: get_vec3(params, "axis_origin").unwrap_or([0.0, 0.0, 0.0]),
                axis_dir: get_vec3(params, "axis_dir").unwrap_or([0.0, 0.0, 1.0]),
                count: get_f64(params, "count").map(|v| v as u32).unwrap_or(4),
                angle_deg: get_f64(params, "angle_deg").unwrap_or(360.0),
            },
            "mirror" => Self::Mirror {
                input: get_str(params, "input").unwrap_or_default(),
                plane: get_str(params, "plane").unwrap_or_else(|| "YZ".into()),
            },
            "text" => Self::Text {
                text: get_str(params, "text").unwrap_or_else(|| "Text".into()),
                height: get_f64(params, "height").unwrap_or(10.0),
                depth: get_f64(params, "depth").unwrap_or(2.0),
                alignment: get_str(params, "alignment"),
                letter_spacing: get_f64(params, "letter_spacing"),
                line_spacing: get_f64(params, "line_spacing"),
            },
            "imported-mesh" => Self::ImportedMesh {
                positions_json: get_str(params, "positions_json").unwrap_or_default(),
                indices_json: get_str(params, "indices_json").unwrap_or_default(),
                normals_json: get_str(params, "normals_json"),
                source: get_str(params, "source"),
            },
            "pcb-board" => Self::PcbBoard {
                board: get_str(params, "board"),
            },
            "embroidery-pattern" => Self::EmbroideryPattern {
                design: get_str(params, "design"),
                source: get_str(params, "source"),
            },
            "part-instance" => Self::PartInstance {
                path: get_str(params, "path").unwrap_or_default(),
                version: get_str(params, "version").unwrap_or_else(|| "1.0".into()),
                params_json: get_str(params, "params_json").unwrap_or_else(|| "{}".into()),
            },
            "part-def" => Self::PartDef {
                source_feature: get_str(params, "source_feature").unwrap_or_default(),
                name: get_str(params, "name"),
            },
            "instance" => Self::Instance {
                part_def: get_str(params, "part_def").unwrap_or_default(),
                name: get_str(params, "name"),
                transform: get_str(params, "transform"),
            },
            "joint" => Self::Joint {
                kind: get_str(params, "kind").unwrap_or_else(|| "Fixed".into()),
                child_instance: get_str(params, "instance_b").unwrap_or_default(),
                parent_instance: get_str(params, "instance_a"),
                anchor_a: get_vec3(params, "anchor_a").unwrap_or([0.0; 3]),
                anchor_b: get_vec3(params, "anchor_b").unwrap_or([0.0; 3]),
                axis: get_vec3(params, "axis"),
                name: get_str(params, "name"),
                limits: get_str(params, "limits"),
            },
            "drawing-settings" => Self::DrawingSettings {
                title_block: get_str(params, "title_block"),
                sections: get_str(params, "sections"),
                show_bom: get_str(params, "show_bom"),
            },
            "scene-settings" => Self::SceneSettings {
                environment: get_str(params, "environment"),
                lights: get_str(params, "lights"),
                background: get_str(params, "background"),
                post_processing: get_str(params, "post_processing"),
                camera_presets: get_str(params, "camera_presets"),
            },
            "schematic" => Self::Schematic {
                title: get_str(params, "title"),
                sheet: get_str(params, "sheet"),
            },
            "molecule" => Self::Molecule {
                system: get_str(params, "system"),
            },
            "analysis-studies" => Self::AnalysisStudies {
                studies: get_str(params, "studies"),
            },
            "design-constraints" => Self::DesignConstraints {
                constraints: get_str(params, "constraints"),
            },
            "timeline" => Self::Timeline {
                timeline: get_str(params, "timeline"),
            },
            _ => return None,
        })
    }
}

// -- Helpers for reading from FeatureParams --

fn get_f64(params: &FeatureParams, key: &str) -> Option<f64> {
    match &params.get(key)?.0 {
        Value::F64(v) => Some(*v),
        _ => None,
    }
}

fn get_str(params: &FeatureParams, key: &str) -> Option<String> {
    match &params.get(key)?.0 {
        Value::String(v) => Some(v.clone()),
        Value::FeatureRef(v) => Some(v.clone()),
        _ => None,
    }
}

fn get_vec3(params: &FeatureParams, key: &str) -> Option<[f64; 3]> {
    match &params.get(key)?.0 {
        Value::Vec3(v) => Some(*v),
        _ => None,
    }
}

fn get_bool(params: &FeatureParams, key: &str) -> Option<bool> {
    match &params.get(key)?.0 {
        Value::Bool(v) => Some(*v),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_crdt::ReplicaId;

    fn dummy_hlc() -> HLC {
        HLC::new(ReplicaId(0))
    }

    fn to_feature_params(params: HashMap<String, Value>) -> FeatureParams {
        let hlc = dummy_hlc();
        params.into_iter().map(|(k, v)| (k, (v, hlc))).collect()
    }

    #[test]
    fn test_cube_roundtrip() {
        let input = FeatureInput::Cube {
            size_x: 10.0,
            size_y: 20.0,
            size_z: 30.0,
        };
        let (kind, params) = input.to_crdt_params();
        assert_eq!(kind, "cube");

        let fp = to_feature_params(params);

        let restored = FeatureInput::from_crdt_params(kind, &fp).unwrap();
        match restored {
            FeatureInput::Cube {
                size_x,
                size_y,
                size_z,
            } => {
                assert_eq!(size_x, 10.0);
                assert_eq!(size_y, 20.0);
                assert_eq!(size_z, 30.0);
            }
            _ => panic!("expected Cube"),
        }
    }

    #[test]
    fn test_boolean_roundtrip() {
        let input = FeatureInput::Boolean {
            boolean_type: BooleanType::Difference,
            input_a: "1:0".into(),
            input_b: "1:1".into(),
        };
        let (kind, params) = input.to_crdt_params();
        assert_eq!(kind, "boolean");

        let fp = to_feature_params(params);

        let restored = FeatureInput::from_crdt_params(kind, &fp).unwrap();
        match restored {
            FeatureInput::Boolean {
                boolean_type,
                input_a,
                input_b,
            } => {
                assert_eq!(boolean_type, BooleanType::Difference);
                assert_eq!(input_a, "1:0");
                assert_eq!(input_b, "1:1");
            }
            _ => panic!("expected Boolean"),
        }
    }

    #[test]
    fn test_loft_roundtrip() {
        let input = FeatureInput::Loft {
            profiles: vec!["sketch1".into(), "sketch2".into(), "sketch3".into()],
            closed: Some(true),
        };
        let (kind, params) = input.to_crdt_params();
        assert_eq!(kind, "loft");

        let fp = to_feature_params(params);

        let restored = FeatureInput::from_crdt_params(kind, &fp).unwrap();
        match restored {
            FeatureInput::Loft { profiles, closed } => {
                assert_eq!(profiles.len(), 3);
                assert_eq!(profiles[0], "sketch1");
                assert_eq!(closed, Some(true));
            }
            _ => panic!("expected Loft"),
        }
    }

    #[test]
    fn test_serde_json_roundtrip() {
        let input = FeatureInput::Extrude {
            sketch: "{}".into(),
            depth: 15.0,
            direction: [0.0, 0.0, 1.0],
            twist_angle: Some(45.0),
            scale_end: None,
        };
        let json = serde_json::to_string(&input).unwrap();
        assert!(json.contains("\"type\":\"Extrude\""));

        let restored: FeatureInput = serde_json::from_str(&json).unwrap();
        match restored {
            FeatureInput::Extrude {
                depth, twist_angle, ..
            } => {
                assert_eq!(depth, 15.0);
                assert_eq!(twist_angle, Some(45.0));
            }
            _ => panic!("expected Extrude"),
        }
    }

    #[test]
    fn test_unknown_kind_returns_none() {
        let fp: FeatureParams = HashMap::new();
        assert!(FeatureInput::from_crdt_params("unknown_kind", &fp).is_none());
    }

    #[test]
    fn test_kind_strings_match_materializer() {
        // Verify all kind() returns match the strings used in the materializer
        let cases: Vec<(FeatureInput, &str)> = vec![
            (
                FeatureInput::Cube {
                    size_x: 1.0,
                    size_y: 1.0,
                    size_z: 1.0,
                },
                "cube",
            ),
            (
                FeatureInput::Cylinder {
                    radius: 1.0,
                    height: 1.0,
                    segments: None,
                },
                "cylinder",
            ),
            (
                FeatureInput::Sphere {
                    radius: 1.0,
                    segments: None,
                },
                "sphere",
            ),
            (
                FeatureInput::Fillet {
                    input: "".into(),
                    radius: 1.0,
                },
                "fillet",
            ),
            (
                FeatureInput::Chamfer {
                    input: "".into(),
                    distance: 1.0,
                },
                "chamfer",
            ),
            (
                FeatureInput::Shell {
                    input: "".into(),
                    thickness: 1.0,
                },
                "shell",
            ),
            (
                FeatureInput::LinearPattern {
                    input: "".into(),
                    direction: [1.0, 0.0, 0.0],
                    count: 3,
                    spacing: 10.0,
                },
                "linear-pattern",
            ),
            (
                FeatureInput::CircularPattern {
                    input: "".into(),
                    axis_origin: [0.0; 3],
                    axis_dir: [0.0, 0.0, 1.0],
                    count: 4,
                    angle_deg: 360.0,
                },
                "circular-pattern",
            ),
            (
                FeatureInput::Mirror {
                    input: "".into(),
                    plane: "YZ".into(),
                },
                "mirror",
            ),
            (FeatureInput::PcbBoard { board: None }, "pcb-board"),
            (
                FeatureInput::EmbroideryPattern {
                    design: None,
                    source: None,
                },
                "embroidery-pattern",
            ),
        ];
        for (input, expected_kind) in cases {
            assert_eq!(input.kind(), expected_kind);
        }
    }

    #[test]
    fn test_defaults_when_params_missing() {
        let empty: FeatureParams = HashMap::new();

        let cube = FeatureInput::from_crdt_params("cube", &empty).unwrap();
        match cube {
            FeatureInput::Cube {
                size_x,
                size_y,
                size_z,
            } => {
                assert_eq!(size_x, 10.0);
                assert_eq!(size_y, 10.0);
                assert_eq!(size_z, 10.0);
            }
            _ => panic!("expected Cube"),
        }
    }
}
