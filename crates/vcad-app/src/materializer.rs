//! Materializer — maps CRDT feature graph to kernel IR.
//!
//! One declarative function replaces the 85+ imperative mutations in TypeScript.
//! Each feature kind is a match arm that reads CRDT params and emits IR nodes.

use std::collections::HashMap;

use vcad_crdt::{CrdtDocument, FeatureId, FeatureState, Value};
use vcad_ir::{
    CsgOp, Document, Instance, Joint, JointKind, JointLimits, Node, NodeId, PartDef,
    SceneEntry, SceneSettings, Transform3D, Vec3,
};

use crate::part_info::PartInfo;

/// Result of materialization: an IR document plus part metadata.
pub struct MaterializeResult {
    /// The IR document ready for evaluation.
    pub document: Document,
    /// Part info for each materialized feature.
    pub parts: Vec<PartInfo>,
}

/// Materialization context — tracks node ID allocation and feature-to-node mapping.
struct Context {
    next_node_id: NodeId,
    /// Maps feature IDs to their root (translate) node ID.
    feature_roots: HashMap<String, NodeId>,
}

impl Context {
    fn new() -> Self {
        Self {
            next_node_id: 1,
            feature_roots: HashMap::new(),
        }
    }

    fn alloc(&mut self) -> NodeId {
        let id = self.next_node_id;
        self.next_node_id += 1;
        id
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
                materialize_part_def(&mut doc, &ctx, fid, feature);
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

        if let Some((part, root_id)) = materialize_feature(&mut doc, &mut ctx, fid, feature, crdt)
        {
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

    MaterializeResult { document: doc, parts }
}

/// Materialize a single feature into IR nodes.
///
/// Returns the PartInfo and the root (outermost translate) node ID, or None
/// if the feature kind is unknown.
fn materialize_feature(
    doc: &mut Document,
    ctx: &mut Context,
    fid: FeatureId,
    feature: &FeatureState,
    crdt: &CrdtDocument,
) -> Option<(PartInfo, NodeId)> {
    let id_str = fid_to_string(fid);
    let name = get_str(feature, "name").unwrap_or_else(|| feature.kind.clone());

    match feature.kind.as_str() {
        "cube" => {
            let sx = get_f64(feature, "size_x").unwrap_or(10.0);
            let sy = get_f64(feature, "size_y").unwrap_or(10.0);
            let sz = get_f64(feature, "size_z").unwrap_or(10.0);

            let prim_id = ctx.alloc();
            let scale_id = ctx.alloc();
            let rotate_id = ctx.alloc();
            let translate_id = ctx.alloc();

            insert_node(doc, prim_id, &name, CsgOp::Cube { size: Vec3::new(sx, sy, sz) });
            insert_transform_chain(doc, ctx, feature, prim_id, scale_id, rotate_id, translate_id);

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
        "cylinder" => {
            let radius = get_f64(feature, "radius").unwrap_or(5.0);
            let height = get_f64(feature, "height").unwrap_or(10.0);
            let segments = get_f64(feature, "segments").map(|v| v as u32).unwrap_or(32);

            let prim_id = ctx.alloc();
            let scale_id = ctx.alloc();
            let rotate_id = ctx.alloc();
            let translate_id = ctx.alloc();

            insert_node(doc, prim_id, &name, CsgOp::Cylinder { radius, height, segments });
            insert_transform_chain(doc, ctx, feature, prim_id, scale_id, rotate_id, translate_id);

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
        "sphere" => {
            let radius = get_f64(feature, "radius").unwrap_or(5.0);
            let segments = get_f64(feature, "segments").map(|v| v as u32).unwrap_or(32);

            let prim_id = ctx.alloc();
            let scale_id = ctx.alloc();
            let rotate_id = ctx.alloc();
            let translate_id = ctx.alloc();

            insert_node(doc, prim_id, &name, CsgOp::Sphere { radius, segments });
            insert_transform_chain(doc, ctx, feature, prim_id, scale_id, rotate_id, translate_id);

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
        "cone" => {
            let radius_bottom = get_f64(feature, "radius_bottom").unwrap_or(5.0);
            let radius_top = get_f64(feature, "radius_top").unwrap_or(0.0);
            let height = get_f64(feature, "height").unwrap_or(10.0);
            let segments = get_f64(feature, "segments").map(|v| v as u32).unwrap_or(32);

            let prim_id = ctx.alloc();
            let scale_id = ctx.alloc();
            let rotate_id = ctx.alloc();
            let translate_id = ctx.alloc();

            insert_node(
                doc,
                prim_id,
                &name,
                CsgOp::Cone { radius_bottom, radius_top, height, segments },
            );
            insert_transform_chain(doc, ctx, feature, prim_id, scale_id, rotate_id, translate_id);

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
        "boolean" => {
            let bool_type = get_str(feature, "boolean_type").unwrap_or_else(|| "union".to_string());
            let input_a = get_str(feature, "input_a").unwrap_or_default();
            let input_b = get_str(feature, "input_b").unwrap_or_default();

            let left = ctx.feature_roots.get(&input_a).copied().unwrap_or(0);
            let right = ctx.feature_roots.get(&input_b).copied().unwrap_or(0);

            let bool_id = ctx.alloc();
            let scale_id = ctx.alloc();
            let rotate_id = ctx.alloc();
            let translate_id = ctx.alloc();

            let op = match bool_type.as_str() {
                "difference" => CsgOp::Difference { left, right },
                "intersection" => CsgOp::Intersection { left, right },
                _ => CsgOp::Union { left, right },
            };
            insert_node(doc, bool_id, &name, op);
            insert_transform_chain(doc, ctx, feature, bool_id, scale_id, rotate_id, translate_id);

            Some((
                PartInfo::Boolean {
                    id: id_str,
                    name,
                    boolean_type: bool_type,
                    boolean_node_id: bool_id,
                    scale_node_id: scale_id,
                    rotate_node_id: rotate_id,
                    translate_node_id: translate_id,
                    source_part_ids: [input_a, input_b],
                },
                translate_id,
            ))
        }
        "fillet" => {
            let input = get_str(feature, "input").unwrap_or_default();
            let radius = get_f64(feature, "radius").unwrap_or(1.0);

            let child = ctx.feature_roots.get(&input).copied().unwrap_or(0);

            let fillet_id = ctx.alloc();
            let scale_id = ctx.alloc();
            let rotate_id = ctx.alloc();
            let translate_id = ctx.alloc();

            insert_node(doc, fillet_id, &name, CsgOp::Fillet { child, radius });
            insert_transform_chain(doc, ctx, feature, fillet_id, scale_id, rotate_id, translate_id);

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
        "chamfer" => {
            let input = get_str(feature, "input").unwrap_or_default();
            let distance = get_f64(feature, "distance").unwrap_or(1.0);

            let child = ctx.feature_roots.get(&input).copied().unwrap_or(0);

            let chamfer_id = ctx.alloc();
            let scale_id = ctx.alloc();
            let rotate_id = ctx.alloc();
            let translate_id = ctx.alloc();

            insert_node(doc, chamfer_id, &name, CsgOp::Chamfer { child, distance });
            insert_transform_chain(
                doc, ctx, feature, chamfer_id, scale_id, rotate_id, translate_id,
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
        "shell" => {
            let input = get_str(feature, "input").unwrap_or_default();
            let thickness = get_f64(feature, "thickness").unwrap_or(1.0);

            let child = ctx.feature_roots.get(&input).copied().unwrap_or(0);

            let shell_id = ctx.alloc();
            let scale_id = ctx.alloc();
            let rotate_id = ctx.alloc();
            let translate_id = ctx.alloc();

            insert_node(doc, shell_id, &name, CsgOp::Shell { child, thickness });
            insert_transform_chain(doc, ctx, feature, shell_id, scale_id, rotate_id, translate_id);

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
        "extrude" => {
            let depth = get_f64(feature, "depth").unwrap_or(10.0);
            let direction = get_vec3(feature, "direction").unwrap_or([0.0, 0.0, 1.0]);

            let sketch_id = ctx.alloc();
            let extrude_id = ctx.alloc();
            let scale_id = ctx.alloc();
            let rotate_id = ctx.alloc();
            let translate_id = ctx.alloc();

            let sketch_op = parse_sketch(feature);
            insert_node(doc, sketch_id, &format!("{name} Sketch"), sketch_op);
            insert_node(
                doc,
                extrude_id,
                &name,
                CsgOp::Extrude {
                    sketch: sketch_id,
                    direction: Vec3::new(
                        direction[0] * depth,
                        direction[1] * depth,
                        direction[2] * depth,
                    ),
                    twist_angle: get_f64(feature, "twist_angle"),
                    scale_end: get_f64(feature, "scale_end"),
                },
            );
            insert_transform_chain(
                doc, ctx, feature, extrude_id, scale_id, rotate_id, translate_id,
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
        "revolve" => {
            let axis_origin = get_vec3(feature, "axis_origin").unwrap_or([0.0, 0.0, 0.0]);
            let axis_dir = get_vec3(feature, "axis_dir").unwrap_or([0.0, 1.0, 0.0]);
            let angle_deg = get_f64(feature, "angle_deg").unwrap_or(360.0);

            let sketch_id = ctx.alloc();
            let revolve_id = ctx.alloc();
            let scale_id = ctx.alloc();
            let rotate_id = ctx.alloc();
            let translate_id = ctx.alloc();

            let sketch_op = parse_sketch(feature);
            insert_node(doc, sketch_id, &format!("{name} Sketch"), sketch_op);
            insert_node(
                doc,
                revolve_id,
                &name,
                CsgOp::Revolve {
                    sketch: sketch_id,
                    axis_origin: Vec3::new(axis_origin[0], axis_origin[1], axis_origin[2]),
                    axis_dir: Vec3::new(axis_dir[0], axis_dir[1], axis_dir[2]),
                    angle_deg,
                },
            );
            insert_transform_chain(
                doc, ctx, feature, revolve_id, scale_id, rotate_id, translate_id,
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
        "sweep" => {
            let path_data = get_str(feature, "path");

            let sketch_id = ctx.alloc();
            let sweep_id = ctx.alloc();
            let scale_id = ctx.alloc();
            let rotate_id = ctx.alloc();
            let translate_id = ctx.alloc();

            let sketch_op = parse_sketch(feature);
            insert_node(doc, sketch_id, &format!("{name} Sketch"), sketch_op);

            let path = path_data
                .and_then(|d| serde_json::from_str::<vcad_ir::PathCurve>(&d).ok())
                .unwrap_or(vcad_ir::PathCurve::Line {
                    start: Vec3::new(0.0, 0.0, 0.0),
                    end: Vec3::new(0.0, 0.0, 50.0),
                });

            insert_node(
                doc,
                sweep_id,
                &name,
                CsgOp::Sweep {
                    sketch: sketch_id,
                    path,
                    twist_angle: get_f64(feature, "twist_angle"),
                    scale_start: get_f64(feature, "scale_start"),
                    scale_end: get_f64(feature, "scale_end"),
                    orientation: None,
                    path_segments: None,
                    arc_segments: None,
                },
            );
            insert_transform_chain(
                doc, ctx, feature, sweep_id, scale_id, rotate_id, translate_id,
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
        "loft" => {
            let sketch_count = get_f64(feature, "sketch_count")
                .map(|v| v as usize)
                .unwrap_or(0);
            let closed = get_bool(feature, "closed").unwrap_or(false);

            let mut sketch_node_ids = Vec::new();
            for i in 0..sketch_count {
                let key = format!("sketch_{i}");
                let sketch_id = ctx.alloc();
                let sketch_op = if let Some(data) = get_str(feature, &key) {
                    serde_json::from_str::<CsgOp>(&data).unwrap_or_else(|_| default_sketch())
                } else {
                    default_sketch()
                };
                insert_node(doc, sketch_id, &format!("{name} Sketch {i}"), sketch_op);
                sketch_node_ids.push(sketch_id);
            }

            // Need at least 2 sketches for a loft
            if sketch_node_ids.len() < 2 {
                return None;
            }

            let loft_id = ctx.alloc();
            let scale_id = ctx.alloc();
            let rotate_id = ctx.alloc();
            let translate_id = ctx.alloc();

            insert_node(
                doc,
                loft_id,
                &name,
                CsgOp::Loft {
                    sketches: sketch_node_ids.clone(),
                    closed: if closed { Some(true) } else { None },
                },
            );
            insert_transform_chain(
                doc, ctx, feature, loft_id, scale_id, rotate_id, translate_id,
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
        "text" => {
            let text = get_str(feature, "text").unwrap_or_else(|| "Text".to_string());
            let height = get_f64(feature, "height").unwrap_or(10.0);
            let depth = get_f64(feature, "depth").unwrap_or(2.0);
            let alignment_str = get_str(feature, "alignment").unwrap_or_else(|| "left".to_string());

            let text_id = ctx.alloc();
            let extrude_id = ctx.alloc();
            let scale_id = ctx.alloc();
            let rotate_id = ctx.alloc();
            let translate_id = ctx.alloc();

            let alignment = match alignment_str.as_str() {
                "center" => vcad_ir::TextAlignment::Center,
                "right" => vcad_ir::TextAlignment::Right,
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
                    letter_spacing: get_f64(feature, "letter_spacing"),
                    line_spacing: get_f64(feature, "line_spacing"),
                    alignment,
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
                doc, ctx, feature, extrude_id, scale_id, rotate_id, translate_id,
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
        "imported-mesh" => {
            let positions_json = get_str(feature, "positions_json").unwrap_or_default();
            let indices_json = get_str(feature, "indices_json").unwrap_or_default();
            let normals_json = get_str(feature, "normals_json");
            let source = get_str(feature, "source");

            let positions: Vec<f64> =
                serde_json::from_str(&positions_json).unwrap_or_default();
            let indices: Vec<u32> =
                serde_json::from_str(&indices_json).unwrap_or_default();
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
                doc, ctx, feature, mesh_id, scale_id, rotate_id, translate_id,
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
        "linear-pattern" => {
            let input = get_str(feature, "input").unwrap_or_default();
            let direction = get_vec3(feature, "direction").unwrap_or([1.0, 0.0, 0.0]);
            let count = get_f64(feature, "count").map(|v| v as u32).unwrap_or(3);
            let spacing = get_f64(feature, "spacing").unwrap_or(20.0);

            let child = ctx.feature_roots.get(&input).copied().unwrap_or(0);

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
                doc, ctx, feature, pattern_id, scale_id, rotate_id, translate_id,
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
        "circular-pattern" => {
            let input = get_str(feature, "input").unwrap_or_default();
            let axis_origin = get_vec3(feature, "axis_origin").unwrap_or([0.0, 0.0, 0.0]);
            let axis_dir = get_vec3(feature, "axis_dir").unwrap_or([0.0, 0.0, 1.0]);
            let count = get_f64(feature, "count").map(|v| v as u32).unwrap_or(4);
            let angle_deg = get_f64(feature, "angle_deg").unwrap_or(360.0);

            let child = ctx.feature_roots.get(&input).copied().unwrap_or(0);

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
                doc, ctx, feature, pattern_id, scale_id, rotate_id, translate_id,
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
        "mirror" => {
            let input = get_str(feature, "input").unwrap_or_default();
            let plane = get_str(feature, "plane").unwrap_or_else(|| "YZ".to_string());

            let child = ctx.feature_roots.get(&input).copied().unwrap_or(0);

            // Mirror via negative scale on the appropriate axis
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
                doc, ctx, feature, mirror_id, scale_id, rotate_id, translate_id,
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
        "pcb-board" => {
            // PCB boards: emit an Empty node for the transform chain,
            // and populate doc.pcb from the "board" JSON param if present.
            let board_id = ctx.alloc();
            let scale_id = ctx.alloc();
            let rotate_id = ctx.alloc();
            let translate_id = ctx.alloc();

            if let Some(json) = get_str(feature, "board") {
                doc.pcb = serde_json::from_str(&json).ok();
            }

            insert_node(doc, board_id, &name, CsgOp::Empty);
            insert_transform_chain(
                doc, ctx, feature, board_id, scale_id, rotate_id, translate_id,
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
        "embroidery-pattern" => {
            let pattern_id = ctx.alloc();
            let scale_id = ctx.alloc();
            let rotate_id = ctx.alloc();
            let translate_id = ctx.alloc();

            let source = get_str(feature, "source");

            // If design data is present, emit a proper EmbroideryPattern node.
            let pattern_op = if let Some(design_json) = get_str(feature, "design") {
                serde_json::from_str::<vcad_ir::EmbroideryDesign>(&design_json)
                    .map(|d| CsgOp::EmbroideryPattern {
                        design: Box::new(d),
                    })
                    .unwrap_or(CsgOp::Empty)
            } else {
                CsgOp::Empty
            };

            insert_node(doc, pattern_id, &name, pattern_op);
            insert_transform_chain(
                doc, ctx, feature, pattern_id, scale_id, rotate_id, translate_id,
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
        // Unknown feature kinds are silently skipped.
        _ => {
            let _ = crdt; // suppress unused warning
            None
        }
    }
}

// -- Assembly materialization --

fn materialize_part_def(
    doc: &mut Document,
    ctx: &Context,
    fid: FeatureId,
    feature: &FeatureState,
) {
    let id = fid_to_string(fid);
    let name = get_str(feature, "name");
    let default_material = get_str(feature, "default_material");

    // Resolve source_feature ref to a root node ID.
    let root = get_str(feature, "source_feature")
        .and_then(|ref_id| ctx.feature_roots.get(&ref_id))
        .copied()
        .unwrap_or(0);

    let part_defs = doc.part_defs.get_or_insert_with(HashMap::new);
    part_defs.insert(
        id.clone(),
        PartDef {
            id,
            name,
            root,
            default_material,
        },
    );
}

fn materialize_instance(
    doc: &mut Document,
    ctx: &Context,
    fid: FeatureId,
    feature: &FeatureState,
) {
    let id = fid_to_string(fid);
    let name = get_str(feature, "name");
    let material = get_str(feature, "material");

    // Resolve part_def ref to a CRDT feature ID string.
    let part_def_id = get_str(feature, "part_def").unwrap_or_default();

    // Parse transform from JSON string if present.
    let transform = get_str(feature, "transform").and_then(|json| {
        serde_json::from_str::<Transform3D>(&json).ok()
    });

    let is_ground = get_bool(feature, "is_ground").unwrap_or(false);
    if is_ground {
        doc.ground_instance_id = Some(id.clone());
    }

    let instances = doc.instances.get_or_insert_with(Vec::new);
    instances.push(Instance {
        id,
        part_def_id,
        name,
        transform,
        material,
    });

    let _ = ctx; // available for future use
}

fn materialize_joint(doc: &mut Document, fid: FeatureId, feature: &FeatureState) {
    let id = fid_to_string(fid);
    let name = get_str(feature, "name");
    let parent_instance_id = get_str(feature, "instance_a");
    let child_instance_id = get_str(feature, "instance_b").unwrap_or_default();

    let parent_anchor = get_vec3(feature, "anchor_a").unwrap_or([0.0; 3]);
    let child_anchor = get_vec3(feature, "anchor_b").unwrap_or([0.0; 3]);
    let state = get_f64(feature, "state").unwrap_or(0.0);

    let kind_str = get_str(feature, "kind").unwrap_or_else(|| "Fixed".to_string());
    let axis = get_vec3(feature, "axis").map(|a| Vec3::new(a[0], a[1], a[2]));
    let limits = get_str(feature, "limits")
        .and_then(|json| serde_json::from_str::<JointLimits>(&json).ok());

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
        name,
        parent_instance_id,
        child_instance_id,
        parent_anchor: Vec3::new(parent_anchor[0], parent_anchor[1], parent_anchor[2]),
        child_anchor: Vec3::new(child_anchor[0], child_anchor[1], child_anchor[2]),
        kind,
        state,
    });
}

fn materialize_schematic(doc: &mut Document, feature: &FeatureState) {
    if let Some(json) = get_str(feature, "sheet") {
        doc.schematic = serde_json::from_str(&json).ok();
    }
}

fn materialize_scene_settings(doc: &mut Document, feature: &FeatureState) {
    let mut scene = SceneSettings::default();

    if let Some(json) = get_str(feature, "environment") {
        scene.environment = serde_json::from_str(&json).ok();
    }
    if let Some(json) = get_str(feature, "lights") {
        scene.lights = serde_json::from_str(&json).ok();
    }
    if let Some(json) = get_str(feature, "background") {
        scene.background = serde_json::from_str(&json).ok();
    }
    if let Some(json) = get_str(feature, "post_processing") {
        scene.post_processing = serde_json::from_str(&json).ok();
    }
    if let Some(json) = get_str(feature, "camera_presets") {
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

/// Parse sketch data from the "sketch" param, falling back to an empty XY sketch.
fn parse_sketch(feature: &FeatureState) -> CsgOp {
    if let Some(data) = get_str(feature, "sketch") {
        serde_json::from_str::<CsgOp>(&data).unwrap_or_else(|_| default_sketch())
    } else {
        default_sketch()
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
            PartInfo::Cube { primitive_node_id, .. } => *primitive_node_id,
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
                ("boolean_type".to_string(), Value::String("difference".to_string())),
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
