//! Materializer — maps CRDT feature graph to kernel IR.
//!
//! One declarative function replaces the 85+ imperative mutations in TypeScript.
//! Each feature kind is a match arm that reads CRDT params and emits IR nodes.
//!
//! Param extraction is handled by `FeatureInput::from_crdt_params()` — the
//! materializer only does IR node allocation and CSG tree construction.

use std::collections::HashMap;

use vcad_crdt::{CrdtDocument, FeatureId, FeatureState, Value};
use vcad_ir::{
    CsgOp, Document, Instance, Joint, JointKind, JointLimits, Node, NodeId, PartDef, SceneEntry,
    SceneSettings, Transform3D, Vec3,
};

use crate::feature::{BooleanType, FeatureInput};
use crate::part_info::PartInfo;

/// Result of materialization: an IR document plus part metadata.
pub struct MaterializeResult {
    /// The IR document ready for evaluation.
    pub document: Document,
    /// Part info for each materialized feature.
    pub parts: Vec<PartInfo>,
    /// Non-fatal problems detected while building the document (e.g.
    /// features that reference inputs that no longer exist). Each entry is
    /// a short human-readable sentence. Consumers should log these so
    /// users know why a feature is missing from the scene.
    pub warnings: Vec<String>,
}

/// Materialization context — tracks node ID allocation and feature-to-node mapping.
struct Context {
    next_node_id: NodeId,
    /// Maps feature IDs to their root (translate) node ID.
    feature_roots: HashMap<String, NodeId>,
    /// Non-fatal warnings accumulated during materialization.
    warnings: Vec<String>,
}

impl Context {
    fn new() -> Self {
        Self {
            next_node_id: 1,
            feature_roots: HashMap::new(),
            warnings: Vec::new(),
        }
    }

    fn alloc(&mut self) -> NodeId {
        let id = self.next_node_id;
        self.next_node_id += 1;
        id
    }

    /// Resolve a feature id or record a warning that describes the miss.
    /// Use this when a feature has a required input reference — the caller
    /// should return `None` when this returns `None`, skipping the feature.
    fn feature_root_or_warn(
        &mut self,
        input: &str,
        owner_kind: &str,
        owner_id: &str,
    ) -> Option<NodeId> {
        match self.feature_roots.get(input).copied() {
            Some(id) => Some(id),
            None => {
                self.warnings.push(format!(
                    "{owner_kind} feature {owner_id} references input {input:?} which no longer resolves — skipped"
                ));
                None
            }
        }
    }
}

/// Materialize the full CRDT document into an IR document and parts list.
pub fn materialize(crdt: &CrdtDocument) -> MaterializeResult {
    let mut doc = Document::new();
    let mut parts = Vec::new();
    let mut ctx = Context::new();

    for (fid, feature) in crdt.ordered_features() {
        // Non-geometry features: assembly metadata, scene settings.
        match feature.kind.as_str() {
            "part-def" => {
                materialize_part_def(&mut doc, &mut ctx, fid, feature);
                continue;
            }
            "instance" => {
                materialize_instance(&mut doc, &ctx, fid, feature);
                continue;
            }
            "joint" => {
                materialize_joint(&mut doc, fid, feature);
                continue;
            }
            "scene-settings" => {
                materialize_scene_settings(&mut doc, feature);
                continue;
            }
            "schematic" => {
                materialize_schematic(&mut doc, feature);
                continue;
            }
            _ => {}
        }

        if let Some((part, root_id)) = materialize_feature(&mut doc, &mut ctx, fid, feature, crdt) {
            let material = get_str(feature, "material").unwrap_or_else(|| "default".to_string());
            let visible = get_bool(feature, "visible");
            doc.roots.push(SceneEntry {
                root: root_id,
                material,
                visible,
            });
            ctx.feature_roots.insert(fid_to_string(fid), root_id);
            parts.push(part);
        }
    }

    MaterializeResult {
        document: doc,
        parts,
        warnings: ctx.warnings,
    }
}

/// Materialize a single geometry feature into IR nodes.
///
/// Uses `FeatureInput::from_crdt_params()` for typed param extraction,
/// then allocates IR nodes and constructs the CSG tree.
///
/// Returns the PartInfo and the root (outermost translate) node ID, or None
/// if the feature kind is unknown.
fn materialize_feature(
    doc: &mut Document,
    ctx: &mut Context,
    fid: FeatureId,
    feature: &FeatureState,
    _crdt: &CrdtDocument,
) -> Option<(PartInfo, NodeId)> {
    let input = FeatureInput::from_crdt_params(&feature.kind, &feature.params)?;

    // Non-geometry variants are handled earlier in materialize(); skip here.
    match &input {
        FeatureInput::PartDef { .. }
        | FeatureInput::Instance { .. }
        | FeatureInput::Joint { .. }
        | FeatureInput::SceneSettings { .. }
        | FeatureInput::Schematic { .. } => return None,
        _ => {}
    }

    let id_str = fid_to_string(fid);
    let name = get_str(feature, "name").unwrap_or_else(|| feature.kind.clone());

    match input {
        FeatureInput::Cube {
            size_x,
            size_y,
            size_z,
        } => {
            let prim_id = ctx.alloc();
            let scale_id = ctx.alloc();
            let rotate_id = ctx.alloc();
            let translate_id = ctx.alloc();

            insert_node(
                doc,
                prim_id,
                &name,
                CsgOp::Cube {
                    size: Vec3::new(size_x, size_y, size_z),
                },
            );
            insert_transform_chain(
                doc,
                ctx,
                feature,
                prim_id,
                scale_id,
                rotate_id,
                translate_id,
            );

            Some((
                PartInfo::Cube {
                    id: id_str,
                    name,
                    primitive_node_id: prim_id,
                    scale_node_id: scale_id,
                    rotate_node_id: rotate_id,
                    translate_node_id: translate_id,
                },
                translate_id,
            ))
        }
        FeatureInput::Cylinder {
            radius,
            height,
            segments,
        } => {
            let segments = segments.unwrap_or(32);

            let prim_id = ctx.alloc();
            let scale_id = ctx.alloc();
            let rotate_id = ctx.alloc();
            let translate_id = ctx.alloc();

            insert_node(
                doc,
                prim_id,
                &name,
                CsgOp::Cylinder {
                    radius,
                    height,
                    segments,
                },
            );
            insert_transform_chain(
                doc,
                ctx,
                feature,
                prim_id,
                scale_id,
                rotate_id,
                translate_id,
            );

            Some((
                PartInfo::Cylinder {
                    id: id_str,
                    name,
                    primitive_node_id: prim_id,
                    scale_node_id: scale_id,
                    rotate_node_id: rotate_id,
                    translate_node_id: translate_id,
                },
                translate_id,
            ))
        }
        FeatureInput::Sphere { radius, segments } => {
            let segments = segments.unwrap_or(32);

            let prim_id = ctx.alloc();
            let scale_id = ctx.alloc();
            let rotate_id = ctx.alloc();
            let translate_id = ctx.alloc();

            insert_node(doc, prim_id, &name, CsgOp::Sphere { radius, segments });
            insert_transform_chain(
                doc,
                ctx,
                feature,
                prim_id,
                scale_id,
                rotate_id,
                translate_id,
            );

            Some((
                PartInfo::Sphere {
                    id: id_str,
                    name,
                    primitive_node_id: prim_id,
                    scale_node_id: scale_id,
                    rotate_node_id: rotate_id,
                    translate_node_id: translate_id,
                },
                translate_id,
            ))
        }
        FeatureInput::Cone {
            radius_bottom,
            radius_top,
            height,
            segments,
        } => {
            let segments = segments.unwrap_or(32);

            let prim_id = ctx.alloc();
            let scale_id = ctx.alloc();
            let rotate_id = ctx.alloc();
            let translate_id = ctx.alloc();

            insert_node(
                doc,
                prim_id,
                &name,
                CsgOp::Cone {
                    radius_bottom,
                    radius_top,
                    height,
                    segments,
                },
            );
            insert_transform_chain(
                doc,
                ctx,
                feature,
                prim_id,
                scale_id,
                rotate_id,
                translate_id,
            );

            Some((
                PartInfo::Cone {
                    id: id_str,
                    name,
                    primitive_node_id: prim_id,
                    scale_node_id: scale_id,
                    rotate_node_id: rotate_id,
                    translate_node_id: translate_id,
                },
                translate_id,
            ))
        }
        FeatureInput::Boolean {
            boolean_type,
            input_a,
            input_b,
        } => {
            let left = ctx.feature_root_or_warn(&input_a, "boolean", &id_str)?;
            let right = ctx.feature_root_or_warn(&input_b, "boolean", &id_str)?;

            let bool_id = ctx.alloc();
            let scale_id = ctx.alloc();
            let rotate_id = ctx.alloc();
            let translate_id = ctx.alloc();

            let op = match boolean_type {
                BooleanType::Difference => CsgOp::Difference { left, right },
                BooleanType::Intersection => CsgOp::Intersection { left, right },
                BooleanType::Union => CsgOp::Union { left, right },
            };
            insert_node(doc, bool_id, &name, op);
            insert_transform_chain(
                doc,
                ctx,
                feature,
                bool_id,
                scale_id,
                rotate_id,
                translate_id,
            );

            Some((
                PartInfo::Boolean {
                    id: id_str,
                    name,
                    boolean_type: boolean_type.as_str().to_string(),
                    boolean_node_id: bool_id,
                    scale_node_id: scale_id,
                    rotate_node_id: rotate_id,
                    translate_node_id: translate_id,
                    source_part_ids: [input_a, input_b],
                },
                translate_id,
            ))
        }
        FeatureInput::Fillet { input, radius } => {
            let child = ctx.feature_root_or_warn(&input, "fillet", &id_str)?;

            let fillet_id = ctx.alloc();
            let scale_id = ctx.alloc();
            let rotate_id = ctx.alloc();
            let translate_id = ctx.alloc();

            insert_node(doc, fillet_id, &name, CsgOp::Fillet { child, radius });
            insert_transform_chain(
                doc,
                ctx,
                feature,
                fillet_id,
                scale_id,
                rotate_id,
                translate_id,
            );

            Some((
                PartInfo::Fillet {
                    id: id_str,
                    name,
                    source_part_id: input,
                    fillet_node_id: fillet_id,
                    scale_node_id: scale_id,
                    rotate_node_id: rotate_id,
                    translate_node_id: translate_id,
                },
                translate_id,
            ))
        }
        FeatureInput::Chamfer { input, distance } => {
            let child = ctx.feature_root_or_warn(&input, "chamfer", &id_str)?;

            let chamfer_id = ctx.alloc();
            let scale_id = ctx.alloc();
            let rotate_id = ctx.alloc();
            let translate_id = ctx.alloc();

            insert_node(doc, chamfer_id, &name, CsgOp::Chamfer { child, distance });
            insert_transform_chain(
                doc,
                ctx,
                feature,
                chamfer_id,
                scale_id,
                rotate_id,
                translate_id,
            );

            Some((
                PartInfo::Chamfer {
                    id: id_str,
                    name,
                    source_part_id: input,
                    chamfer_node_id: chamfer_id,
                    scale_node_id: scale_id,
                    rotate_node_id: rotate_id,
                    translate_node_id: translate_id,
                },
                translate_id,
            ))
        }
        FeatureInput::Shell { input, thickness } => {
            let child = ctx.feature_root_or_warn(&input, "shell", &id_str)?;

            let shell_id = ctx.alloc();
            let scale_id = ctx.alloc();
            let rotate_id = ctx.alloc();
            let translate_id = ctx.alloc();

            insert_node(doc, shell_id, &name, CsgOp::Shell { child, thickness });
            insert_transform_chain(
                doc,
                ctx,
                feature,
                shell_id,
                scale_id,
                rotate_id,
                translate_id,
            );

            Some((
                PartInfo::Shell {
                    id: id_str,
                    name,
                    source_part_id: input,
                    shell_node_id: shell_id,
                    scale_node_id: scale_id,
                    rotate_node_id: rotate_id,
                    translate_node_id: translate_id,
                },
                translate_id,
            ))
        }
        FeatureInput::Extrude {
            sketch,
            depth,
            direction,
            twist_angle,
            scale_end,
        } => {
            let sketch_id = ctx.alloc();
            let extrude_id = ctx.alloc();
            let scale_id = ctx.alloc();
            let rotate_id = ctx.alloc();
            let translate_id = ctx.alloc();

            let sketch_op = parse_sketch_str(&sketch);
            let extrude_op = if sketch_is_empty(&sketch_op) {
                // Profile not yet authored — skip the kernel roundtrip to avoid
                // the empty-loop panic. The feature still materializes (fillets
                // and booleans that reference it keep resolving) but renders
                // as nothing until segments are added.
                CsgOp::Empty
            } else {
                CsgOp::Extrude {
                    sketch: sketch_id,
                    direction: Vec3::new(
                        direction[0] * depth,
                        direction[1] * depth,
                        direction[2] * depth,
                    ),
                    twist_angle,
                    scale_end,
                }
            };
            insert_node(doc, sketch_id, &format!("{name} Sketch"), sketch_op);
            insert_node(doc, extrude_id, &name, extrude_op);
            insert_transform_chain(
                doc,
                ctx,
                feature,
                extrude_id,
                scale_id,
                rotate_id,
                translate_id,
            );

            Some((
                PartInfo::Extrude {
                    id: id_str,
                    name,
                    sketch_node_id: sketch_id,
                    extrude_node_id: extrude_id,
                    scale_node_id: scale_id,
                    rotate_node_id: rotate_id,
                    translate_node_id: translate_id,
                },
                translate_id,
            ))
        }
        FeatureInput::Revolve {
            sketch,
            axis_origin,
            axis_dir,
            angle_deg,
        } => {
            let sketch_id = ctx.alloc();
            let revolve_id = ctx.alloc();
            let scale_id = ctx.alloc();
            let rotate_id = ctx.alloc();
            let translate_id = ctx.alloc();

            let sketch_op = parse_sketch_str(&sketch);
            let revolve_op = if sketch_is_empty(&sketch_op) {
                CsgOp::Empty
            } else {
                CsgOp::Revolve {
                    sketch: sketch_id,
                    axis_origin: Vec3::new(axis_origin[0], axis_origin[1], axis_origin[2]),
                    axis_dir: Vec3::new(axis_dir[0], axis_dir[1], axis_dir[2]),
                    angle_deg,
                }
            };
            insert_node(doc, sketch_id, &format!("{name} Sketch"), sketch_op);
            insert_node(doc, revolve_id, &name, revolve_op);
            insert_transform_chain(
                doc,
                ctx,
                feature,
                revolve_id,
                scale_id,
                rotate_id,
                translate_id,
            );

            Some((
                PartInfo::Revolve {
                    id: id_str,
                    name,
                    sketch_node_id: sketch_id,
                    revolve_node_id: revolve_id,
                    scale_node_id: scale_id,
                    rotate_node_id: rotate_id,
                    translate_node_id: translate_id,
                },
                translate_id,
            ))
        }
        FeatureInput::Sweep {
            sketch,
            path,
            twist_angle,
            scale_start,
            scale_end,
        } => {
            let sketch_id = ctx.alloc();
            let sweep_id = ctx.alloc();
            let scale_id = ctx.alloc();
            let rotate_id = ctx.alloc();
            let translate_id = ctx.alloc();

            let sketch_op = parse_sketch_str(&sketch);
            let sketch_empty = sketch_is_empty(&sketch_op);
            insert_node(doc, sketch_id, &format!("{name} Sketch"), sketch_op);

            let path_curve = path
                .and_then(|d| serde_json::from_str::<vcad_ir::PathCurve>(&d).ok())
                .unwrap_or(vcad_ir::PathCurve::Line {
                    start: Vec3::new(0.0, 0.0, 0.0),
                    end: Vec3::new(0.0, 0.0, 50.0),
                });

            let sweep_op = if sketch_empty {
                CsgOp::Empty
            } else {
                CsgOp::Sweep {
                    sketch: sketch_id,
                    path: path_curve,
                    twist_angle,
                    scale_start,
                    scale_end,
                    orientation: None,
                    path_segments: None,
                    arc_segments: None,
                }
            };
            insert_node(doc, sweep_id, &name, sweep_op);
            insert_transform_chain(
                doc,
                ctx,
                feature,
                sweep_id,
                scale_id,
                rotate_id,
                translate_id,
            );

            Some((
                PartInfo::Sweep {
                    id: id_str,
                    name,
                    sketch_node_id: sketch_id,
                    sweep_node_id: sweep_id,
                    scale_node_id: scale_id,
                    rotate_node_id: rotate_id,
                    translate_node_id: translate_id,
                },
                translate_id,
            ))
        }
        FeatureInput::Loft { profiles, closed } => {
            let mut sketch_node_ids = Vec::new();
            let mut any_empty = false;
            for (i, profile_data) in profiles.iter().enumerate() {
                let sketch_id = ctx.alloc();
                let sketch_op = if profile_data.is_empty() {
                    default_sketch()
                } else {
                    serde_json::from_str::<CsgOp>(profile_data).unwrap_or_else(|_| default_sketch())
                };
                if sketch_is_empty(&sketch_op) {
                    any_empty = true;
                }
                insert_node(doc, sketch_id, &format!("{name} Sketch {i}"), sketch_op);
                sketch_node_ids.push(sketch_id);
            }

            if sketch_node_ids.len() < 2 {
                return None;
            }

            let loft_id = ctx.alloc();
            let scale_id = ctx.alloc();
            let rotate_id = ctx.alloc();
            let translate_id = ctx.alloc();

            let closed_flag = closed.unwrap_or(false);
            // Any empty profile would break loft (no ring to loft to/from);
            // emit Empty so the feature still materializes but renders blank.
            let loft_op = if any_empty {
                CsgOp::Empty
            } else {
                CsgOp::Loft {
                    sketches: sketch_node_ids.clone(),
                    closed: if closed_flag { Some(true) } else { None },
                }
            };
            insert_node(doc, loft_id, &name, loft_op);
            insert_transform_chain(
                doc,
                ctx,
                feature,
                loft_id,
                scale_id,
                rotate_id,
                translate_id,
            );

            Some((
                PartInfo::Loft {
                    id: id_str,
                    name,
                    sketch_node_ids,
                    loft_node_id: loft_id,
                    scale_node_id: scale_id,
                    rotate_node_id: rotate_id,
                    translate_node_id: translate_id,
                },
                translate_id,
            ))
        }
        FeatureInput::Text {
            text,
            height,
            depth,
            alignment,
            letter_spacing,
            line_spacing,
        } => {
            let text_id = ctx.alloc();
            let extrude_id = ctx.alloc();
            let scale_id = ctx.alloc();
            let rotate_id = ctx.alloc();
            let translate_id = ctx.alloc();

            let align = match alignment.as_deref() {
                Some("center") => vcad_ir::TextAlignment::Center,
                Some("right") => vcad_ir::TextAlignment::Right,
                _ => vcad_ir::TextAlignment::Left,
            };

            insert_node(
                doc,
                text_id,
                &format!("{name} Text"),
                CsgOp::Text2D {
                    origin: Vec3::new(0.0, 0.0, 0.0),
                    x_dir: Vec3::new(1.0, 0.0, 0.0),
                    y_dir: Vec3::new(0.0, 1.0, 0.0),
                    text,
                    font: "sans-serif".to_string(),
                    height,
                    letter_spacing,
                    line_spacing,
                    alignment: align,
                },
            );
            insert_node(
                doc,
                extrude_id,
                &name,
                CsgOp::Extrude {
                    sketch: text_id,
                    direction: Vec3::new(0.0, 0.0, depth),
                    twist_angle: None,
                    scale_end: None,
                },
            );
            insert_transform_chain(
                doc,
                ctx,
                feature,
                extrude_id,
                scale_id,
                rotate_id,
                translate_id,
            );

            Some((
                PartInfo::Text {
                    id: id_str,
                    name,
                    text_node_id: text_id,
                    extrude_node_id: extrude_id,
                    scale_node_id: scale_id,
                    rotate_node_id: rotate_id,
                    translate_node_id: translate_id,
                },
                translate_id,
            ))
        }
        FeatureInput::ImportedMesh {
            positions_json,
            indices_json,
            normals_json,
            source,
        } => {
            let positions: Vec<f64> = serde_json::from_str(&positions_json).unwrap_or_default();
            let indices: Vec<u32> = serde_json::from_str(&indices_json).unwrap_or_default();
            let normals: Option<Vec<f64>> =
                normals_json.and_then(|s| serde_json::from_str(&s).ok());

            let mesh_id = ctx.alloc();
            let scale_id = ctx.alloc();
            let rotate_id = ctx.alloc();
            let translate_id = ctx.alloc();

            insert_node(
                doc,
                mesh_id,
                &name,
                CsgOp::ImportedMesh {
                    positions,
                    indices,
                    normals,
                    source: source.clone(),
                },
            );
            insert_transform_chain(
                doc,
                ctx,
                feature,
                mesh_id,
                scale_id,
                rotate_id,
                translate_id,
            );

            Some((
                PartInfo::ImportedMesh {
                    id: id_str,
                    name,
                    mesh_node_id: mesh_id,
                    scale_node_id: scale_id,
                    rotate_node_id: rotate_id,
                    translate_node_id: translate_id,
                    source,
                },
                translate_id,
            ))
        }
        FeatureInput::LinearPattern {
            input,
            direction,
            count,
            spacing,
        } => {
            let child = ctx.feature_root_or_warn(&input, "linear-pattern", &id_str)?;

            let pattern_id = ctx.alloc();
            let scale_id = ctx.alloc();
            let rotate_id = ctx.alloc();
            let translate_id = ctx.alloc();

            insert_node(
                doc,
                pattern_id,
                &name,
                CsgOp::LinearPattern {
                    child,
                    direction: Vec3::new(direction[0], direction[1], direction[2]),
                    count,
                    spacing,
                },
            );
            insert_transform_chain(
                doc,
                ctx,
                feature,
                pattern_id,
                scale_id,
                rotate_id,
                translate_id,
            );

            Some((
                PartInfo::LinearPattern {
                    id: id_str,
                    name,
                    source_part_id: input,
                    pattern_node_id: pattern_id,
                    scale_node_id: scale_id,
                    rotate_node_id: rotate_id,
                    translate_node_id: translate_id,
                },
                translate_id,
            ))
        }
        FeatureInput::CircularPattern {
            input,
            axis_origin,
            axis_dir,
            count,
            angle_deg,
        } => {
            let child = ctx.feature_root_or_warn(&input, "circular-pattern", &id_str)?;

            let pattern_id = ctx.alloc();
            let scale_id = ctx.alloc();
            let rotate_id = ctx.alloc();
            let translate_id = ctx.alloc();

            insert_node(
                doc,
                pattern_id,
                &name,
                CsgOp::CircularPattern {
                    child,
                    axis_origin: Vec3::new(axis_origin[0], axis_origin[1], axis_origin[2]),
                    axis_dir: Vec3::new(axis_dir[0], axis_dir[1], axis_dir[2]),
                    count,
                    angle_deg,
                },
            );
            insert_transform_chain(
                doc,
                ctx,
                feature,
                pattern_id,
                scale_id,
                rotate_id,
                translate_id,
            );

            Some((
                PartInfo::CircularPattern {
                    id: id_str,
                    name,
                    source_part_id: input,
                    pattern_node_id: pattern_id,
                    scale_node_id: scale_id,
                    rotate_node_id: rotate_id,
                    translate_node_id: translate_id,
                },
                translate_id,
            ))
        }
        FeatureInput::Mirror { input, plane } => {
            let child = ctx.feature_root_or_warn(&input, "mirror", &id_str)?;

            let mirror_factor = match plane.as_str() {
                "XY" => Vec3::new(1.0, 1.0, -1.0),
                "XZ" => Vec3::new(1.0, -1.0, 1.0),
                _ /* YZ */ => Vec3::new(-1.0, 1.0, 1.0),
            };

            let mirror_id = ctx.alloc();
            let scale_id = ctx.alloc();
            let rotate_id = ctx.alloc();
            let translate_id = ctx.alloc();

            insert_node(
                doc,
                mirror_id,
                &name,
                CsgOp::Scale {
                    child,
                    factor: mirror_factor,
                },
            );
            insert_transform_chain(
                doc,
                ctx,
                feature,
                mirror_id,
                scale_id,
                rotate_id,
                translate_id,
            );

            Some((
                PartInfo::Mirror {
                    id: id_str,
                    name,
                    source_part_id: input,
                    mirror_node_id: mirror_id,
                    scale_node_id: scale_id,
                    rotate_node_id: rotate_id,
                    translate_node_id: translate_id,
                },
                translate_id,
            ))
        }
        FeatureInput::PcbBoard { board } => {
            let board_id = ctx.alloc();
            let scale_id = ctx.alloc();
            let rotate_id = ctx.alloc();
            let translate_id = ctx.alloc();

            if let Some(json) = &board {
                doc.pcb = serde_json::from_str(json).ok();
            }

            insert_node(doc, board_id, &name, CsgOp::Empty);
            insert_transform_chain(
                doc,
                ctx,
                feature,
                board_id,
                scale_id,
                rotate_id,
                translate_id,
            );

            Some((
                PartInfo::PcbBoard {
                    id: id_str,
                    name,
                    board_node_id: board_id,
                    scale_node_id: scale_id,
                    rotate_node_id: rotate_id,
                    translate_node_id: translate_id,
                },
                translate_id,
            ))
        }
        FeatureInput::EmbroideryPattern { design, source } => {
            let pattern_id = ctx.alloc();
            let scale_id = ctx.alloc();
            let rotate_id = ctx.alloc();
            let translate_id = ctx.alloc();

            let pattern_op = if let Some(design_json) = &design {
                serde_json::from_str::<vcad_ir::EmbroideryDesign>(design_json)
                    .map(|d| CsgOp::EmbroideryPattern {
                        design: Box::new(d),
                    })
                    .unwrap_or(CsgOp::Empty)
            } else {
                CsgOp::Empty
            };

            insert_node(doc, pattern_id, &name, pattern_op);
            insert_transform_chain(
                doc,
                ctx,
                feature,
                pattern_id,
                scale_id,
                rotate_id,
                translate_id,
            );

            Some((
                PartInfo::EmbroideryPattern {
                    id: id_str,
                    name,
                    pattern_node_id: pattern_id,
                    scale_node_id: scale_id,
                    rotate_node_id: rotate_id,
                    translate_node_id: translate_id,
                    source,
                },
                translate_id,
            ))
        }
        // Non-geometry variants already filtered above.
        _ => None,
    }
}

// -- Assembly materialization --

fn materialize_part_def(
    doc: &mut Document,
    ctx: &mut Context,
    fid: FeatureId,
    feature: &FeatureState,
) {
    let input = match FeatureInput::from_crdt_params(&feature.kind, &feature.params) {
        Some(FeatureInput::PartDef {
            source_feature,
            name,
        }) => (source_feature, name),
        _ => return,
    };
    let (source_feature, part_name) = input;

    let id = fid_to_string(fid);
    let default_material = get_str(feature, "default_material");

    // If the source feature hasn't been materialized, skip emitting this part def —
    // otherwise the PartDef would reference NodeId(0) and evaluation would fail.
    let Some(root) = ctx.feature_root_or_warn(&source_feature, "part-def", &fid_to_string(fid))
    else {
        return;
    };

    let part_defs = doc.part_defs.get_or_insert_with(HashMap::new);
    part_defs.insert(
        id.clone(),
        PartDef {
            id,
            name: part_name,
            root,
            default_material,
        },
    );
}

fn materialize_instance(doc: &mut Document, ctx: &Context, fid: FeatureId, feature: &FeatureState) {
    let input = match FeatureInput::from_crdt_params(&feature.kind, &feature.params) {
        Some(FeatureInput::Instance {
            part_def,
            name,
            transform,
        }) => (part_def, name, transform),
        _ => return,
    };
    let (part_def_id, inst_name, transform_json) = input;

    let id = fid_to_string(fid);
    let material = get_str(feature, "material");

    let transform = transform_json.and_then(|json| serde_json::from_str::<Transform3D>(&json).ok());

    let is_ground = get_bool(feature, "is_ground").unwrap_or(false);
    if is_ground {
        doc.ground_instance_id = Some(id.clone());
    }

    let instances = doc.instances.get_or_insert_with(Vec::new);
    instances.push(Instance {
        id,
        part_def_id,
        name: inst_name,
        transform,
        material,
    });

    let _ = ctx;
}

fn materialize_joint(doc: &mut Document, fid: FeatureId, feature: &FeatureState) {
    let input = match FeatureInput::from_crdt_params(&feature.kind, &feature.params) {
        Some(FeatureInput::Joint {
            kind,
            child_instance,
            parent_instance,
            anchor_a,
            anchor_b,
            axis,
            name,
            limits,
        }) => (
            kind,
            child_instance,
            parent_instance,
            anchor_a,
            anchor_b,
            axis,
            name,
            limits,
        ),
        _ => return,
    };
    let (
        kind_str,
        child_instance_id,
        parent_instance_id,
        parent_anchor,
        child_anchor,
        axis_arr,
        joint_name,
        limits_json,
    ) = input;

    let id = fid_to_string(fid);
    let state = get_f64(feature, "state").unwrap_or(0.0);

    let axis = axis_arr.map(|a| Vec3::new(a[0], a[1], a[2]));
    let limits = limits_json.and_then(|json| serde_json::from_str::<JointLimits>(&json).ok());

    let kind = match kind_str.as_str() {
        "Revolute" => JointKind::Revolute {
            axis: axis.unwrap_or(Vec3::new(0.0, 0.0, 1.0)),
            limits,
        },
        "Slider" => JointKind::Slider {
            axis: axis.unwrap_or(Vec3::new(0.0, 0.0, 1.0)),
            limits,
        },
        "Cylindrical" => JointKind::Cylindrical {
            axis: axis.unwrap_or(Vec3::new(0.0, 0.0, 1.0)),
        },
        "Ball" => JointKind::Ball,
        _ => JointKind::Fixed,
    };

    let joints = doc.joints.get_or_insert_with(Vec::new);
    joints.push(Joint {
        id,
        name: joint_name,
        parent_instance_id,
        child_instance_id,
        parent_anchor: Vec3::new(parent_anchor[0], parent_anchor[1], parent_anchor[2]),
        child_anchor: Vec3::new(child_anchor[0], child_anchor[1], child_anchor[2]),
        kind,
        state,
    });
}

fn materialize_schematic(doc: &mut Document, feature: &FeatureState) {
    if let Some(FeatureInput::Schematic {
        sheet: Some(json), ..
    }) = FeatureInput::from_crdt_params(&feature.kind, &feature.params)
    {
        doc.schematic = serde_json::from_str(&json).ok();
    }
}

fn materialize_scene_settings(doc: &mut Document, feature: &FeatureState) {
    let input = match FeatureInput::from_crdt_params(&feature.kind, &feature.params) {
        Some(FeatureInput::SceneSettings {
            environment,
            lights,
            background,
            post_processing,
            camera_presets,
        }) => (
            environment,
            lights,
            background,
            post_processing,
            camera_presets,
        ),
        _ => return,
    };
    let (environment, lights, background, post_processing, camera_presets) = input;

    let mut scene = SceneSettings::default();
    if let Some(json) = environment {
        scene.environment = serde_json::from_str(&json).ok();
    }
    if let Some(json) = lights {
        scene.lights = serde_json::from_str(&json).ok();
    }
    if let Some(json) = background {
        scene.background = serde_json::from_str(&json).ok();
    }
    if let Some(json) = post_processing {
        scene.post_processing = serde_json::from_str(&json).ok();
    }
    if let Some(json) = camera_presets {
        scene.camera_presets = serde_json::from_str(&json).ok();
    }
    doc.scene = Some(scene);
}

// -- Helpers --

fn fid_to_string(fid: FeatureId) -> String {
    format!("{}:{}", fid.0 .0, fid.1)
}

fn get_f64(feature: &FeatureState, key: &str) -> Option<f64> {
    match &feature.params.get(key)?.0 {
        Value::F64(v) => Some(*v),
        _ => None,
    }
}

fn get_str(feature: &FeatureState, key: &str) -> Option<String> {
    match &feature.params.get(key)?.0 {
        Value::String(v) => Some(v.clone()),
        Value::FeatureRef(v) => Some(v.clone()),
        _ => None,
    }
}

fn get_vec3(feature: &FeatureState, key: &str) -> Option<[f64; 3]> {
    match &feature.params.get(key)?.0 {
        Value::Vec3(v) => Some(*v),
        _ => None,
    }
}

fn get_bool(feature: &FeatureState, key: &str) -> Option<bool> {
    match &feature.params.get(key)?.0 {
        Value::Bool(v) => Some(*v),
        _ => None,
    }
}

/// Parse sketch data from a JSON string, falling back to an empty XY sketch.
fn parse_sketch_str(data: &str) -> CsgOp {
    if data.is_empty() {
        default_sketch()
    } else {
        serde_json::from_str::<CsgOp>(data).unwrap_or_else(|e| {
            panic!(
                "parse_sketch_str: failed to parse sketch JSON: {e}\ninput ({} bytes): {}",
                data.len(),
                data
            );
        })
    }
}

/// Default empty sketch on the XY plane.
fn default_sketch() -> CsgOp {
    CsgOp::Sketch2D {
        origin: Vec3::new(0.0, 0.0, 0.0),
        x_dir: Vec3::new(1.0, 0.0, 0.0),
        y_dir: Vec3::new(0.0, 1.0, 0.0),
        segments: Vec::new(),
    }
}

/// Does this sketch op carry zero segments?
///
/// An Extrude/Revolve/Sweep fed an empty profile would either surface
/// `SketchError::EmptyProfile` in the evaluator or (worse) hit the kernel's
/// `add_loop` assertion and crash the whole WASM module. The materializer
/// detects this up front and swaps the op-under-construction for `Empty`.
fn sketch_is_empty(op: &CsgOp) -> bool {
    matches!(op, CsgOp::Sketch2D { segments, .. } if segments.is_empty())
}

fn insert_node(doc: &mut Document, id: NodeId, name: &str, op: CsgOp) {
    doc.nodes.insert(
        id,
        Node {
            id,
            name: Some(name.to_string()),
            op,
        },
    );
}

/// Insert the standard transform chain: child → Scale → Rotate → Translate.
fn insert_transform_chain(
    doc: &mut Document,
    ctx: &mut Context,
    feature: &FeatureState,
    child_id: NodeId,
    scale_id: NodeId,
    rotate_id: NodeId,
    translate_id: NodeId,
) {
    let _ = ctx; // ctx available for future use

    let scale = get_vec3(feature, "scale").unwrap_or([1.0, 1.0, 1.0]);
    let rotation = get_vec3(feature, "rotation").unwrap_or([0.0, 0.0, 0.0]);
    let offset = get_vec3(feature, "offset").unwrap_or([0.0, 0.0, 0.0]);

    doc.nodes.insert(
        scale_id,
        Node {
            id: scale_id,
            name: None,
            op: CsgOp::Scale {
                child: child_id,
                factor: Vec3::new(scale[0], scale[1], scale[2]),
            },
        },
    );
    doc.nodes.insert(
        rotate_id,
        Node {
            id: rotate_id,
            name: None,
            op: CsgOp::Rotate {
                child: scale_id,
                angles: Vec3::new(rotation[0], rotation[1], rotation[2]),
            },
        },
    );
    doc.nodes.insert(
        translate_id,
        Node {
            id: translate_id,
            name: None,
            op: CsgOp::Translate {
                child: rotate_id,
                offset: Vec3::new(offset[0], offset[1], offset[2]),
            },
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_crdt::{FractionalIndex, ReplicaId};

    #[test]
    fn test_materialize_cube() {
        let mut crdt = CrdtDocument::new(ReplicaId(1));
        crdt.create_feature(
            "cube",
            FractionalIndex::between(None, None),
            HashMap::from([
                ("size_x".to_string(), Value::F64(10.0)),
                ("size_y".to_string(), Value::F64(20.0)),
                ("size_z".to_string(), Value::F64(30.0)),
            ]),
        );

        let result = materialize(&crdt);
        assert_eq!(result.parts.len(), 1);
        assert_eq!(result.document.roots.len(), 1);
        assert_eq!(result.document.nodes.len(), 4); // prim + scale + rotate + translate

        // Check the primitive node has correct dimensions
        let prim_node_id = match &result.parts[0] {
            PartInfo::Cube {
                primitive_node_id, ..
            } => *primitive_node_id,
            _ => panic!("expected cube part"),
        };
        let prim = result.document.nodes.get(&prim_node_id).unwrap();
        match &prim.op {
            CsgOp::Cube { size } => {
                assert_eq!(size.x, 10.0);
                assert_eq!(size.y, 20.0);
                assert_eq!(size.z, 30.0);
            }
            _ => panic!("expected Cube op"),
        }
    }

    #[test]
    fn test_materialize_multiple_features() {
        let mut crdt = CrdtDocument::new(ReplicaId(1));
        crdt.create_feature(
            "cube",
            FractionalIndex::between(None, None),
            HashMap::from([("size_x".to_string(), Value::F64(10.0))]),
        );
        crdt.create_feature(
            "cylinder",
            FractionalIndex::between(None, None),
            HashMap::from([
                ("radius".to_string(), Value::F64(5.0)),
                ("height".to_string(), Value::F64(20.0)),
            ]),
        );

        let result = materialize(&crdt);
        assert_eq!(result.parts.len(), 2);
        assert_eq!(result.document.roots.len(), 2);
    }

    #[test]
    fn test_materialize_with_transforms() {
        let mut crdt = CrdtDocument::new(ReplicaId(1));
        crdt.create_feature(
            "cube",
            FractionalIndex::between(None, None),
            HashMap::from([
                ("size_x".to_string(), Value::F64(10.0)),
                ("size_y".to_string(), Value::F64(10.0)),
                ("size_z".to_string(), Value::F64(10.0)),
                ("offset".to_string(), Value::Vec3([5.0, 10.0, 15.0])),
                ("rotation".to_string(), Value::Vec3([0.0, 0.0, 45.0])),
            ]),
        );

        let result = materialize(&crdt);
        let translate_id = result.parts[0].root_node_id();
        let translate = result.document.nodes.get(&translate_id).unwrap();
        match &translate.op {
            CsgOp::Translate { offset, .. } => {
                assert_eq!(offset.x, 5.0);
                assert_eq!(offset.y, 10.0);
                assert_eq!(offset.z, 15.0);
            }
            _ => panic!("expected Translate"),
        }
    }

    #[test]
    fn test_materialize_boolean() {
        let mut crdt = CrdtDocument::new(ReplicaId(1));
        let pos1 = FractionalIndex::between(None, None);
        let (fid1, _) = crdt.create_feature("cube", pos1.clone(), HashMap::new());

        let pos2 = FractionalIndex::between(Some(&pos1), None);
        let (fid2, _) = crdt.create_feature("cylinder", pos2.clone(), HashMap::new());

        let id1_str = fid_to_string(fid1);
        let id2_str = fid_to_string(fid2);

        let pos3 = FractionalIndex::between(Some(&pos2), None);
        crdt.create_feature(
            "boolean",
            pos3,
            HashMap::from([
                (
                    "boolean_type".to_string(),
                    Value::String("difference".to_string()),
                ),
                ("input_a".to_string(), Value::FeatureRef(id1_str)),
                ("input_b".to_string(), Value::FeatureRef(id2_str)),
            ]),
        );

        let result = materialize(&crdt);
        assert_eq!(result.parts.len(), 3);
        match &result.parts[2] {
            PartInfo::Boolean { boolean_type, .. } => {
                assert_eq!(boolean_type, "difference");
            }
            _ => panic!("expected boolean part"),
        }
    }

    #[test]
    fn test_materialize_fillet() {
        let mut crdt = CrdtDocument::new(ReplicaId(1));
        let pos1 = FractionalIndex::between(None, None);
        let (fid, _) = crdt.create_feature("cube", pos1.clone(), HashMap::new());
        let id_str = fid_to_string(fid);

        let pos2 = FractionalIndex::between(Some(&pos1), None);
        crdt.create_feature(
            "fillet",
            pos2,
            HashMap::from([
                ("input".to_string(), Value::FeatureRef(id_str.clone())),
                ("radius".to_string(), Value::F64(2.0)),
            ]),
        );

        let result = materialize(&crdt);
        assert_eq!(result.parts.len(), 2);
        match &result.parts[1] {
            PartInfo::Fillet {
                source_part_id,
                fillet_node_id,
                ..
            } => {
                assert_eq!(source_part_id, &id_str);
                let node = result.document.nodes.get(fillet_node_id).unwrap();
                match &node.op {
                    CsgOp::Fillet { radius, .. } => assert_eq!(*radius, 2.0),
                    _ => panic!("expected Fillet op"),
                }
            }
            _ => panic!("expected fillet part"),
        }
    }

    #[test]
    fn test_fillet_with_missing_input_is_skipped() {
        // Regression: previously the materializer wrote `Fillet.child = 0`
        // when the input feature couldn't be resolved, producing a dangling
        // NodeId reference that crashed evaluation of the whole document.
        let mut crdt = CrdtDocument::new(ReplicaId(1));
        crdt.create_feature(
            "fillet",
            FractionalIndex::between(None, None),
            HashMap::from([
                (
                    "input".to_string(),
                    Value::FeatureRef("nonexistent-feature-id".to_string()),
                ),
                ("radius".to_string(), Value::F64(2.0)),
            ]),
        );

        let result = materialize(&crdt);
        assert_eq!(result.parts.len(), 0, "fillet must not be emitted");
        assert_eq!(
            result.document.roots.len(),
            0,
            "no scene root should be created for the unreachable fillet"
        );
        // And — critically — no node with a dangling child reference.
        for node in result.document.nodes.values() {
            if let CsgOp::Fillet { child, .. } = node.op {
                assert_ne!(child, 0, "no node should reference NodeId(0)");
            }
        }
        assert!(
            result.warnings.iter().any(|w| w.contains("fillet")
                && w.contains("nonexistent-feature-id")),
            "warnings should mention the skipped fillet: {:?}",
            result.warnings
        );
    }

    #[test]
    fn test_extrude_with_empty_sketch_renders_as_empty() {
        // Regression: an extrude with zero-segment sketch data used to crash
        // the kernel (add_loop assertion) when evaluated. Now the materializer
        // swaps the Extrude op for Empty so neither the kernel nor the
        // sketch validator ever sees the degenerate profile.
        let mut crdt = CrdtDocument::new(ReplicaId(1));
        let empty_sketch_json = serde_json::to_string(&vcad_ir::CsgOp::Sketch2D {
            origin: Vec3::new(0.0, 0.0, 0.0),
            x_dir: Vec3::new(1.0, 0.0, 0.0),
            y_dir: Vec3::new(0.0, 1.0, 0.0),
            segments: Vec::new(),
        })
        .unwrap();
        crdt.create_feature(
            "extrude",
            FractionalIndex::between(None, None),
            HashMap::from([
                ("sketch".to_string(), Value::String(empty_sketch_json)),
                ("depth".to_string(), Value::F64(10.0)),
                ("direction".to_string(), Value::Vec3([0.0, 0.0, 1.0])),
            ]),
        );

        let result = materialize(&crdt);
        // Feature still materializes so dependents can reference it.
        assert_eq!(result.parts.len(), 1);
        let extrude_node_id = match &result.parts[0] {
            PartInfo::Extrude {
                extrude_node_id, ..
            } => *extrude_node_id,
            _ => panic!("expected extrude part"),
        };
        // Inner op must be Empty, not Extrude — otherwise eval would fail
        // (or panic) on the empty profile.
        let node = result.document.nodes.get(&extrude_node_id).unwrap();
        assert!(
            matches!(node.op, CsgOp::Empty),
            "expected Empty op for empty-sketch extrude, got {:?}",
            node.op
        );
    }

    #[test]
    fn test_unknown_feature_skipped() {
        let mut crdt = CrdtDocument::new(ReplicaId(1));
        crdt.create_feature(
            "unknown_thing",
            FractionalIndex::between(None, None),
            HashMap::new(),
        );

        let result = materialize(&crdt);
        assert_eq!(result.parts.len(), 0);
        assert_eq!(result.document.roots.len(), 0);
    }

    #[test]
    fn test_materialize_revolve() {
        let mut crdt = CrdtDocument::new(ReplicaId(1));
        let sketch_json = serde_json::to_string(&vcad_ir::CsgOp::Sketch2D {
            origin: Vec3::new(0.0, 0.0, 0.0),
            x_dir: Vec3::new(1.0, 0.0, 0.0),
            y_dir: Vec3::new(0.0, 1.0, 0.0),
            segments: Vec::new(),
        })
        .unwrap();
        crdt.create_feature(
            "revolve",
            FractionalIndex::between(None, None),
            HashMap::from([
                ("sketch".to_string(), Value::String(sketch_json)),
                ("axis_origin".to_string(), Value::Vec3([0.0, 0.0, 0.0])),
                ("axis_dir".to_string(), Value::Vec3([0.0, 1.0, 0.0])),
                ("angle_deg".to_string(), Value::F64(180.0)),
            ]),
        );

        let result = materialize(&crdt);
        assert_eq!(result.parts.len(), 1);
        assert!(matches!(&result.parts[0], PartInfo::Revolve { .. }));
        // sketch + revolve + scale + rotate + translate = 5 nodes
        assert_eq!(result.document.nodes.len(), 5);
    }

    #[test]
    fn test_materialize_sweep() {
        let mut crdt = CrdtDocument::new(ReplicaId(1));
        let path_json = serde_json::to_string(&vcad_ir::PathCurve::Line {
            start: Vec3::new(0.0, 0.0, 0.0),
            end: Vec3::new(0.0, 0.0, 50.0),
        })
        .unwrap();
        crdt.create_feature(
            "sweep",
            FractionalIndex::between(None, None),
            HashMap::from([("path".to_string(), Value::String(path_json))]),
        );

        let result = materialize(&crdt);
        assert_eq!(result.parts.len(), 1);
        assert!(matches!(&result.parts[0], PartInfo::Sweep { .. }));
        assert_eq!(result.document.nodes.len(), 5);
    }

    #[test]
    fn test_materialize_text() {
        let mut crdt = CrdtDocument::new(ReplicaId(1));
        crdt.create_feature(
            "text",
            FractionalIndex::between(None, None),
            HashMap::from([
                ("text".to_string(), Value::String("Hello".to_string())),
                ("height".to_string(), Value::F64(12.0)),
                ("depth".to_string(), Value::F64(3.0)),
            ]),
        );

        let result = materialize(&crdt);
        assert_eq!(result.parts.len(), 1);
        assert!(matches!(&result.parts[0], PartInfo::Text { .. }));
        // text2d + extrude + scale + rotate + translate = 5 nodes
        assert_eq!(result.document.nodes.len(), 5);
    }

    #[test]
    fn test_materialize_imported_mesh() {
        let mut crdt = CrdtDocument::new(ReplicaId(1));
        crdt.create_feature(
            "imported-mesh",
            FractionalIndex::between(None, None),
            HashMap::from([
                (
                    "positions_json".to_string(),
                    Value::String("[0,0,0, 1,0,0, 0,1,0]".to_string()),
                ),
                (
                    "indices_json".to_string(),
                    Value::String("[0,1,2]".to_string()),
                ),
                ("source".to_string(), Value::String("test.stl".to_string())),
            ]),
        );

        let result = materialize(&crdt);
        assert_eq!(result.parts.len(), 1);
        match &result.parts[0] {
            PartInfo::ImportedMesh { source, .. } => {
                assert_eq!(source.as_deref(), Some("test.stl"));
            }
            _ => panic!("expected imported-mesh part"),
        }
    }

    #[test]
    fn test_materialize_linear_pattern() {
        let mut crdt = CrdtDocument::new(ReplicaId(1));
        let pos1 = FractionalIndex::between(None, None);
        let (fid, _) = crdt.create_feature("cube", pos1.clone(), HashMap::new());
        let id_str = fid_to_string(fid);

        let pos2 = FractionalIndex::between(Some(&pos1), None);
        crdt.create_feature(
            "linear-pattern",
            pos2,
            HashMap::from([
                ("input".to_string(), Value::FeatureRef(id_str)),
                ("direction".to_string(), Value::Vec3([1.0, 0.0, 0.0])),
                ("count".to_string(), Value::F64(5.0)),
                ("spacing".to_string(), Value::F64(15.0)),
            ]),
        );

        let result = materialize(&crdt);
        assert_eq!(result.parts.len(), 2);
        match &result.parts[1] {
            PartInfo::LinearPattern { .. } => {}
            _ => panic!("expected linear-pattern part"),
        }
    }

    #[test]
    fn test_materialize_circular_pattern() {
        let mut crdt = CrdtDocument::new(ReplicaId(1));
        let pos1 = FractionalIndex::between(None, None);
        let (fid, _) = crdt.create_feature("cube", pos1.clone(), HashMap::new());
        let id_str = fid_to_string(fid);

        let pos2 = FractionalIndex::between(Some(&pos1), None);
        crdt.create_feature(
            "circular-pattern",
            pos2,
            HashMap::from([
                ("input".to_string(), Value::FeatureRef(id_str)),
                ("axis_origin".to_string(), Value::Vec3([0.0, 0.0, 0.0])),
                ("axis_dir".to_string(), Value::Vec3([0.0, 0.0, 1.0])),
                ("count".to_string(), Value::F64(6.0)),
                ("angle_deg".to_string(), Value::F64(360.0)),
            ]),
        );

        let result = materialize(&crdt);
        assert_eq!(result.parts.len(), 2);
        match &result.parts[1] {
            PartInfo::CircularPattern { .. } => {}
            _ => panic!("expected circular-pattern part"),
        }
    }

    #[test]
    fn test_materialize_mirror() {
        let mut crdt = CrdtDocument::new(ReplicaId(1));
        let pos1 = FractionalIndex::between(None, None);
        let (fid, _) = crdt.create_feature("cube", pos1.clone(), HashMap::new());
        let id_str = fid_to_string(fid);

        let pos2 = FractionalIndex::between(Some(&pos1), None);
        crdt.create_feature(
            "mirror",
            pos2,
            HashMap::from([
                ("input".to_string(), Value::FeatureRef(id_str)),
                ("plane".to_string(), Value::String("YZ".to_string())),
            ]),
        );

        let result = materialize(&crdt);
        assert_eq!(result.parts.len(), 2);
        match &result.parts[1] {
            PartInfo::Mirror { mirror_node_id, .. } => {
                let node = result.document.nodes.get(mirror_node_id).unwrap();
                match &node.op {
                    CsgOp::Scale { factor, .. } => {
                        assert_eq!(factor.x, -1.0);
                        assert_eq!(factor.y, 1.0);
                        assert_eq!(factor.z, 1.0);
                    }
                    _ => panic!("expected Scale op for mirror"),
                }
            }
            _ => panic!("expected mirror part"),
        }
    }

    #[test]
    fn test_materialize_pcb_board_stub() {
        let mut crdt = CrdtDocument::new(ReplicaId(1));
        crdt.create_feature(
            "pcb-board",
            FractionalIndex::between(None, None),
            HashMap::new(),
        );

        let result = materialize(&crdt);
        assert_eq!(result.parts.len(), 1);
        assert!(matches!(&result.parts[0], PartInfo::PcbBoard { .. }));
    }

    #[test]
    fn test_materialize_embroidery_pattern_stub() {
        let mut crdt = CrdtDocument::new(ReplicaId(1));
        crdt.create_feature(
            "embroidery-pattern",
            FractionalIndex::between(None, None),
            HashMap::from([("source".to_string(), Value::String("test.pes".to_string()))]),
        );

        let result = materialize(&crdt);
        assert_eq!(result.parts.len(), 1);
        match &result.parts[0] {
            PartInfo::EmbroideryPattern { source, .. } => {
                assert_eq!(source.as_deref(), Some("test.pes"));
            }
            _ => panic!("expected embroidery-pattern part"),
        }
    }
}
