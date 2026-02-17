//! Core document evaluator.
//!
//! Walks the IR DAG and calls vcad-kernel operations to produce meshes.
//! Ported from `packages/engine/src/evaluate.ts`.

use std::collections::HashMap;

use vcad_ir::{CsgOp, Document, NodeId, PathCurve};
use vcad_kernel::Solid;
use vcad_kernel_geom::Line3d;
use vcad_kernel_math::Vec3;
use vcad_kernel_sweep::{Helix, LoftOptions, SweepOptions};
use vcad_kernel_tessellate::TriangleMesh;
use vcad_kernel_text::{FontRegistry, TextAlignment};

use crate::convert::{ir_sketch_to_profile, to_point3, to_vec3};
use crate::kinematics::solve_forward_kinematics;
use crate::{
    EvalError, EvalOptions, EvaluatedInstance, EvaluatedMesh, EvaluatedPart, EvaluatedPartDef,
    EvaluatedScene,
};

/// Evaluate a full document into an EvaluatedScene.
pub fn evaluate_document(
    doc: &Document,
    options: &EvalOptions,
) -> Result<EvaluatedScene, EvalError> {
    let mut cache: HashMap<NodeId, Option<Solid>> = HashMap::new();

    // Evaluate visible roots
    let mut parts = Vec::new();
    let mut solids = Vec::new();

    for entry in &doc.roots {
        if entry.visible == Some(false) {
            continue;
        }

        // Check for ImportedMesh chain
        if let Some(imported) = find_imported_mesh(entry.root, &doc.nodes) {
            let mesh = transform_imported_mesh(&imported);
            parts.push(EvaluatedPart {
                mesh,
                material: entry.material.clone(),
                solid: None,
            });
            solids.push(None);
            continue;
        }

        let solid = evaluate_node(entry.root, &doc.nodes, &mut cache)?;
        match solid {
            Some(ref s) => {
                let tri = s.to_mesh(32);
                let mesh = tri_to_evaluated(&tri);
                parts.push(EvaluatedPart {
                    mesh,
                    material: entry.material.clone(),
                    solid: Some(s.clone()),
                });
                solids.push(Some(s.clone()));
            }
            None => {
                parts.push(EvaluatedPart {
                    mesh: EvaluatedMesh::empty(),
                    material: entry.material.clone(),
                    solid: None,
                });
                solids.push(None);
            }
        }
    }

    // Assembly mode
    let mut part_defs = None;
    let mut instances = None;

    if let (Some(pd_map), Some(inst_list)) = (&doc.part_defs, &doc.instances) {
        if !pd_map.is_empty() && !inst_list.is_empty() {
            let world_transforms = solve_forward_kinematics(doc);

            let mut eval_part_defs = Vec::new();
            let mut part_def_meshes: HashMap<String, EvaluatedMesh> = HashMap::new();

            for (id, part_def) in pd_map {
                let solid = evaluate_node(part_def.root, &doc.nodes, &mut cache)?;
                let mesh = match solid {
                    Some(s) => tri_to_evaluated(&s.to_mesh(32)),
                    None => EvaluatedMesh::empty(),
                };
                part_def_meshes.insert(id.clone(), mesh.clone());
                eval_part_defs.push(EvaluatedPartDef {
                    id: id.clone(),
                    mesh,
                });
            }

            let mut eval_instances = Vec::new();
            for inst in inst_list {
                let mesh = match part_def_meshes.get(&inst.part_def_id) {
                    Some(m) => m.clone(),
                    None => continue,
                };

                let world_transform = world_transforms.get(&inst.id).cloned().or(inst.transform);

                let part_def = pd_map.get(&inst.part_def_id);
                let material = inst
                    .material
                    .clone()
                    .or_else(|| part_def.and_then(|pd| pd.default_material.clone()))
                    .unwrap_or_else(|| "default".to_string());

                eval_instances.push(EvaluatedInstance {
                    instance_id: inst.id.clone(),
                    part_def_id: inst.part_def_id.clone(),
                    name: inst.name.clone(),
                    mesh,
                    material,
                    transform: world_transform,
                });
            }

            part_defs = Some(eval_part_defs);
            instances = Some(eval_instances);
        }
    }

    // Clash detection
    let mut clashes = Vec::new();
    if !options.skip_clash_detection {
        for i in 0..solids.len() {
            for j in (i + 1)..solids.len() {
                if let (Some(a), Some(b)) = (&solids[i], &solids[j]) {
                    let intersection = a.intersection(b);
                    if !intersection.is_empty() {
                        let tri = intersection.to_mesh(32);
                        if !tri.vertices.is_empty() {
                            clashes.push(tri_to_evaluated(&tri));
                        }
                    }
                }
            }
        }
    }

    Ok(EvaluatedScene {
        parts,
        part_defs,
        instances,
        clashes,
    })
}

/// Recursively evaluate a node, with caching.
pub fn evaluate_node(
    node_id: NodeId,
    nodes: &HashMap<NodeId, vcad_ir::Node>,
    cache: &mut HashMap<NodeId, Option<Solid>>,
) -> Result<Option<Solid>, EvalError> {
    if let Some(cached) = cache.get(&node_id) {
        return Ok(cached.clone());
    }

    let node = nodes.get(&node_id).ok_or(EvalError::MissingNode(node_id))?;

    let result = evaluate_op(&node.op, nodes, cache)?;
    cache.insert(node_id, result.clone());
    Ok(result)
}

fn evaluate_op(
    op: &CsgOp,
    nodes: &HashMap<NodeId, vcad_ir::Node>,
    cache: &mut HashMap<NodeId, Option<Solid>>,
) -> Result<Option<Solid>, EvalError> {
    match op {
        CsgOp::Cube { size } => Ok(Some(Solid::cube(size.x, size.y, size.z))),

        CsgOp::Cylinder {
            radius,
            height,
            segments,
        } => Ok(Some(Solid::cylinder(*radius, *height, *segments))),

        CsgOp::Sphere { radius, segments } => Ok(Some(Solid::sphere(*radius, *segments))),

        CsgOp::Cone {
            radius_bottom,
            radius_top,
            height,
            segments,
        } => Ok(Some(Solid::cone(
            *radius_bottom,
            *radius_top,
            *height,
            *segments,
        ))),

        CsgOp::Empty => Ok(Some(Solid::empty())),

        CsgOp::Union { left, right } => {
            let l = evaluate_node(*left, nodes, cache)?;
            let r = evaluate_node(*right, nodes, cache)?;
            match (l, r) {
                (Some(l), Some(r)) => Ok(Some(l.union(&r))),
                (Some(l), None) => Ok(Some(l)),
                (None, Some(r)) => Ok(Some(r)),
                (None, None) => Ok(None),
            }
        }

        CsgOp::Difference { left, right } => {
            let l = evaluate_node(*left, nodes, cache)?;
            let r = evaluate_node(*right, nodes, cache)?;
            match (l, r) {
                (Some(l), Some(r)) => Ok(Some(l.difference(&r))),
                (Some(l), None) => Ok(Some(l)),
                _ => Ok(None),
            }
        }

        CsgOp::Intersection { left, right } => {
            let l = evaluate_node(*left, nodes, cache)?;
            let r = evaluate_node(*right, nodes, cache)?;
            match (l, r) {
                (Some(l), Some(r)) => Ok(Some(l.intersection(&r))),
                _ => Ok(None),
            }
        }

        CsgOp::Translate { child, offset } => {
            let c = evaluate_node(*child, nodes, cache)?;
            Ok(c.map(|s| s.translate(offset.x, offset.y, offset.z)))
        }

        CsgOp::Rotate { child, angles } => {
            let c = evaluate_node(*child, nodes, cache)?;
            Ok(c.map(|s| s.rotate(angles.x, angles.y, angles.z)))
        }

        CsgOp::Scale { child, factor } => {
            let c = evaluate_node(*child, nodes, cache)?;
            Ok(c.map(|s| s.scale(factor.x, factor.y, factor.z)))
        }

        CsgOp::LinearPattern {
            child,
            direction,
            count,
            spacing,
        } => {
            let c = evaluate_node(*child, nodes, cache)?;
            Ok(c.map(|s| s.linear_pattern(to_vec3(direction), *count, *spacing)))
        }

        CsgOp::CircularPattern {
            child,
            axis_origin,
            axis_dir,
            count,
            angle_deg,
        } => {
            let c = evaluate_node(*child, nodes, cache)?;
            Ok(c.map(|s| {
                s.circular_pattern(
                    to_point3(axis_origin),
                    to_vec3(axis_dir),
                    *count,
                    *angle_deg,
                )
            }))
        }

        CsgOp::Shell { child, thickness } => {
            let c = evaluate_node(*child, nodes, cache)?;
            Ok(c.map(|s| s.shell(*thickness)))
        }

        CsgOp::Fillet { child, radius } => {
            let c = evaluate_node(*child, nodes, cache)?;
            Ok(c.map(|s| s.fillet(*radius)))
        }

        CsgOp::Chamfer { child, distance } => {
            let c = evaluate_node(*child, nodes, cache)?;
            Ok(c.map(|s| s.chamfer(*distance)))
        }

        CsgOp::Sketch2D { .. } => {
            // Sketch nodes don't produce geometry directly.
            // They are consumed by Extrude/Revolve/Sweep/Loft.
            Ok(None)
        }

        CsgOp::Text2D { .. } => {
            // Text nodes don't produce geometry directly.
            // They are consumed by Extrude.
            Ok(None)
        }

        CsgOp::Extrude {
            sketch,
            direction,
            twist_angle,
            scale_end,
        } => {
            let sketch_node = nodes.get(sketch).ok_or(EvalError::MissingNode(*sketch))?;

            let dir = to_vec3(direction);

            // Handle Text2D extrusion
            if let CsgOp::Text2D {
                origin,
                x_dir,
                y_dir,
                text,
                font,
                height,
                letter_spacing,
                line_spacing,
                alignment,
            } = &sketch_node.op
            {
                return evaluate_text_extrude(
                    origin,
                    x_dir,
                    y_dir,
                    text,
                    font,
                    *height,
                    letter_spacing.unwrap_or(1.0),
                    line_spacing.unwrap_or(1.0),
                    *alignment,
                    dir,
                );
            }

            // Handle Sketch2D
            let (s_origin, s_x_dir, s_y_dir, segments) = extract_sketch(&sketch_node.op)?;
            let profile = ir_sketch_to_profile(s_origin, s_x_dir, s_y_dir, segments)
                .map_err(EvalError::Sketch)?;

            let has_twist = twist_angle.is_some_and(|t| t.abs() > 1e-12);
            let has_scale = scale_end.is_some_and(|s| (s - 1.0).abs() > 1e-12);

            let solid = if has_twist || has_scale {
                Solid::extrude_with_options(
                    profile,
                    dir,
                    twist_angle.unwrap_or(0.0),
                    scale_end.unwrap_or(1.0),
                )
                .map_err(EvalError::Sketch)?
            } else {
                Solid::extrude(profile, dir).map_err(EvalError::Sketch)?
            };

            Ok(Some(solid))
        }

        CsgOp::Revolve {
            sketch,
            axis_origin,
            axis_dir,
            angle_deg,
        } => {
            let sketch_node = nodes.get(sketch).ok_or(EvalError::MissingNode(*sketch))?;

            let (s_origin, s_x_dir, s_y_dir, segments) = extract_sketch(&sketch_node.op)?;
            let profile = ir_sketch_to_profile(s_origin, s_x_dir, s_y_dir, segments)
                .map_err(EvalError::Sketch)?;

            let solid = Solid::revolve(
                profile,
                to_point3(axis_origin),
                to_vec3(axis_dir),
                *angle_deg,
            )
            .map_err(EvalError::Sketch)?;

            Ok(Some(solid))
        }

        CsgOp::Sweep {
            sketch,
            path,
            twist_angle,
            scale_start,
            scale_end,
            orientation,
            path_segments,
            arc_segments,
        } => {
            let sketch_node = nodes.get(sketch).ok_or(EvalError::MissingNode(*sketch))?;

            let (s_origin, s_x_dir, s_y_dir, segments) = extract_sketch(&sketch_node.op)?;
            let profile = ir_sketch_to_profile(s_origin, s_x_dir, s_y_dir, segments)
                .map_err(EvalError::Sketch)?;

            let options = SweepOptions {
                twist_angle: twist_angle.unwrap_or(0.0),
                scale_start: scale_start.unwrap_or(1.0),
                scale_end: scale_end.unwrap_or(1.0),
                orientation_angle: orientation.unwrap_or(0.0),
                path_segments: path_segments.unwrap_or(0),
                arc_segments: arc_segments.unwrap_or(8),
            };

            let solid = match path {
                PathCurve::Line { start, end } => {
                    let line = Line3d::from_points(to_point3(start), to_point3(end));
                    Solid::sweep(profile, &line, options).map_err(EvalError::Sweep)?
                }
                PathCurve::Helix {
                    radius,
                    pitch,
                    height,
                    turns,
                } => {
                    let helix = Helix::new(*radius, *pitch, *height, *turns);
                    Solid::sweep(profile, &helix, options).map_err(EvalError::Sweep)?
                }
            };

            Ok(Some(solid))
        }

        CsgOp::Loft { sketches, closed } => {
            let mut profiles = Vec::with_capacity(sketches.len());
            for sketch_id in sketches {
                let sketch_node = nodes
                    .get(sketch_id)
                    .ok_or(EvalError::MissingNode(*sketch_id))?;
                let (s_origin, s_x_dir, s_y_dir, segments) = extract_sketch(&sketch_node.op)?;
                let profile = ir_sketch_to_profile(s_origin, s_x_dir, s_y_dir, segments)
                    .map_err(EvalError::Sketch)?;
                profiles.push(profile);
            }

            let options = LoftOptions {
                closed: closed.unwrap_or(false),
                ..Default::default()
            };

            let solid = Solid::loft(&profiles, options).map_err(EvalError::Loft)?;

            Ok(Some(solid))
        }

        CsgOp::ImportedMesh { .. } => {
            // ImportedMesh is handled at the document level via find_imported_mesh.
            // If we reach here directly, return empty.
            Ok(None)
        }

        CsgOp::StepImport { path } => Ok(Solid::from_step(path).ok()),
    }
}

/// Extract sketch fields from a CsgOp, returning error if not a Sketch2D.
fn extract_sketch(
    op: &CsgOp,
) -> Result<
    (
        &vcad_ir::Vec3,
        &vcad_ir::Vec3,
        &vcad_ir::Vec3,
        &[vcad_ir::SketchSegment2D],
    ),
    EvalError,
> {
    match op {
        CsgOp::Sketch2D {
            origin,
            x_dir,
            y_dir,
            segments,
        } => Ok((origin, x_dir, y_dir, segments)),
        _ => Err(EvalError::InvalidSketchRef),
    }
}

/// Evaluate text extrusion (Text2D + Extrude).
#[allow(clippy::too_many_arguments)]
fn evaluate_text_extrude(
    origin: &vcad_ir::Vec3,
    x_dir: &vcad_ir::Vec3,
    y_dir: &vcad_ir::Vec3,
    text: &str,
    font: &str,
    height: f64,
    letter_spacing: f64,
    line_spacing: f64,
    alignment: vcad_ir::TextAlignment,
    direction: Vec3,
) -> Result<Option<Solid>, EvalError> {
    let align = match alignment {
        vcad_ir::TextAlignment::Left => TextAlignment::Left,
        vcad_ir::TextAlignment::Center => TextAlignment::Center,
        vcad_ir::TextAlignment::Right => TextAlignment::Right,
    };

    let font_ref = match font {
        "sans-serif" | "" => FontRegistry::builtin_sans(),
        other => return Err(EvalError::UnknownFont(other.to_string())),
    };

    let profiles = vcad_kernel_text::text_to_profiles(
        text,
        font_ref,
        height,
        letter_spacing,
        line_spacing,
        align,
    );

    if profiles.is_empty() {
        return Ok(Some(Solid::empty()));
    }

    let origin_pt = to_point3(origin);
    let x_vec = to_vec3(x_dir);
    let y_vec = to_vec3(y_dir);

    // Determine holes by geometric containment
    let n = profiles.len();
    let mut is_hole = vec![false; n];
    for i in 0..n {
        for j in 0..n {
            if i != j && profiles[i].is_contained_in(&profiles[j]) {
                is_hole[i] = true;
                break;
            }
        }
    }

    // Extrude outer profiles, merge meshes
    let mut all_vertices: Vec<f32> = Vec::new();
    let mut all_normals: Vec<f32> = Vec::new();
    let mut all_indices: Vec<u32> = Vec::new();

    for (i, profile) in profiles.iter().enumerate() {
        if is_hole[i] {
            continue;
        }
        let world_profile = profile.transform(origin_pt, x_vec, y_vec);
        if let Ok(solid) = Solid::extrude(world_profile, direction) {
            let mesh = solid.to_mesh(32);
            let offset = (all_vertices.len() / 3) as u32;
            all_vertices.extend_from_slice(&mesh.vertices);
            all_normals.extend_from_slice(&mesh.normals);
            for idx in &mesh.indices {
                all_indices.push(idx + offset);
            }
        }
    }

    let mut result = if !all_vertices.is_empty() {
        let merged = TriangleMesh {
            vertices: all_vertices,
            indices: all_indices,
            normals: all_normals,
        };
        Some(Solid::from_mesh(merged))
    } else {
        None
    };

    // Subtract holes
    if let Some(solid) = result.take() {
        let mut current = solid;
        let hole_dir = direction * 1.1;
        let hole_offset = direction * -0.05;

        for (i, profile) in profiles.iter().enumerate() {
            if !is_hole[i] {
                continue;
            }
            let offset_origin = origin_pt + hole_offset;
            let world_profile = profile.transform(offset_origin, x_vec, y_vec);
            if let Ok(hole_solid) = Solid::extrude(world_profile, hole_dir) {
                current = current.difference(&hole_solid);
            }
        }
        result = Some(current);
    }

    Ok(result)
}

/// Data for an imported mesh chain: mesh data + accumulated transform.
struct ImportedMeshData {
    positions: Vec<f64>,
    indices: Vec<u32>,
    normals: Option<Vec<f64>>,
    translate: [f64; 3],
    rotate_deg: [f64; 3],
    scale: [f64; 3],
}

/// Walk the node chain looking for an ImportedMesh, accumulating transforms.
fn find_imported_mesh(
    root_id: NodeId,
    nodes: &HashMap<NodeId, vcad_ir::Node>,
) -> Option<ImportedMeshData> {
    let mut translate = [0.0; 3];
    let mut rotate_deg = [0.0; 3];
    let mut scale = [1.0; 3];

    let mut current = root_id;
    loop {
        let node = nodes.get(&current)?;
        match &node.op {
            CsgOp::ImportedMesh {
                positions,
                indices,
                normals,
                ..
            } => {
                return Some(ImportedMeshData {
                    positions: positions.clone(),
                    indices: indices.clone(),
                    normals: normals.clone(),
                    translate,
                    rotate_deg,
                    scale,
                });
            }
            CsgOp::Translate { child, offset } => {
                translate = [offset.x, offset.y, offset.z];
                current = *child;
            }
            CsgOp::Rotate { child, angles } => {
                rotate_deg = [angles.x, angles.y, angles.z];
                current = *child;
            }
            CsgOp::Scale { child, factor } => {
                scale = [factor.x, factor.y, factor.z];
                current = *child;
            }
            _ => return None,
        }
    }
}

/// Apply transform to imported mesh positions and normals.
fn transform_imported_mesh(data: &ImportedMeshData) -> EvaluatedMesh {
    let n_verts = data.positions.len() / 3;
    let mut positions = vec![0.0f32; data.positions.len()];

    // Precompute rotation matrix
    let rx = data.rotate_deg[0].to_radians();
    let ry = data.rotate_deg[1].to_radians();
    let rz = data.rotate_deg[2].to_radians();

    let (cx, sx) = (rx.cos(), rx.sin());
    let (cy, sy) = (ry.cos(), ry.sin());
    let (cz, sz) = (rz.cos(), rz.sin());

    let m00 = cy * cz;
    let m01 = sx * sy * cz - cx * sz;
    let m02 = cx * sy * cz + sx * sz;
    let m10 = cy * sz;
    let m11 = sx * sy * sz + cx * cz;
    let m12 = cx * sy * sz - sx * cz;
    let m20 = -sy;
    let m21 = sx * cy;
    let m22 = cx * cy;

    for i in 0..n_verts {
        let x = data.positions[i * 3] * data.scale[0];
        let y = data.positions[i * 3 + 1] * data.scale[1];
        let z = data.positions[i * 3 + 2] * data.scale[2];

        positions[i * 3] = (m00 * x + m01 * y + m02 * z + data.translate[0]) as f32;
        positions[i * 3 + 1] = (m10 * x + m11 * y + m12 * z + data.translate[1]) as f32;
        positions[i * 3 + 2] = (m20 * x + m21 * y + m22 * z + data.translate[2]) as f32;
    }

    let normals = data.normals.as_ref().map(|norms| {
        let mut out = vec![0.0f32; norms.len()];
        for i in 0..(norms.len() / 3) {
            let nx = norms[i * 3];
            let ny = norms[i * 3 + 1];
            let nz = norms[i * 3 + 2];
            out[i * 3] = (m00 * nx + m01 * ny + m02 * nz) as f32;
            out[i * 3 + 1] = (m10 * nx + m11 * ny + m12 * nz) as f32;
            out[i * 3 + 2] = (m20 * nx + m21 * ny + m22 * nz) as f32;
        }
        out
    });

    EvaluatedMesh {
        positions,
        indices: data.indices.clone(),
        normals,
    }
}

/// Convert kernel TriangleMesh to EvaluatedMesh.
fn tri_to_evaluated(tri: &TriangleMesh) -> EvaluatedMesh {
    EvaluatedMesh {
        positions: tri.vertices.clone(),
        indices: tri.indices.clone(),
        normals: if tri.normals.is_empty() {
            None
        } else {
            Some(tri.normals.clone())
        },
    }
}
