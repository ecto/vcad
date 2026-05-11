//! Core document evaluator.
//!
//! Walks the IR DAG and calls vcad-kernel operations to produce meshes.
//! Ported from `packages/engine/src/evaluate.ts`.

use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};

use vcad_ir::ecad::{Footprint, Pad, PadShape, Pcb, PcbLayer, Trace, Via, Zone};
use vcad_ir::{CsgOp, Document, NodeId, PathCurve};
use vcad_kernel::Solid;
use vcad_kernel_geom::Line3d;
use vcad_kernel_math::{Transform, Vec3};
use vcad_kernel_sweep::{Helix, LoftOptions, SweepOptions};
use vcad_kernel_tessellate::TriangleMesh;
use vcad_kernel_text::{FontRegistry, TextAlignment};

use crate::convert::{ir_sketch_to_profile, to_point3, to_vec3};
use crate::kinematics::solve_forward_kinematics;
use crate::{
    Clock, EvalError, EvalOptions, EvalTiming, EvaluatedInstance, EvaluatedMesh, EvaluatedPart,
    EvaluatedPartDef, EvaluatedScene, NodeTiming, RootFailure,
};

/// Evaluate a full document into an EvaluatedScene.
///
/// If the document declares `parameters` or `bindings`, the pre-pass
/// [`crate::resolve::resolve_document_cloned`] is invoked to produce a
/// concretized copy before the kernel walk begins.
pub fn evaluate_document(
    doc: &Document,
    options: &EvalOptions,
) -> Result<EvaluatedScene, EvalError> {
    let clock = options.clock.as_deref();
    let t_start = clock.map(|c| c.now_ms());

    // Resolve parameters + bindings into concrete numeric fields. When the
    // doc has no parameters or bindings this is a cheap no-op.
    let resolved_owned;
    let doc: &Document = if doc.parameters.is_empty() && doc.bindings.is_empty() {
        doc
    } else {
        let (d, _env) = crate::resolve::resolve_document_cloned(doc)
            .map_err(|e| EvalError::ResolveBindings(e.to_string()))?;
        resolved_owned = d;
        &resolved_owned
    };

    let mut cache: HashMap<NodeId, Option<Solid>> = HashMap::new();
    let mut node_timings: HashMap<String, NodeTiming> = HashMap::new();
    let mut tessellate_ms: f64 = 0.0;
    let mut failures: Vec<RootFailure> = Vec::new();

    // Evaluate visible roots
    let mut parts = Vec::new();
    let mut solids = Vec::new();

    for (idx, entry) in doc.roots.iter().enumerate() {
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

        // Wrap eval + tessellate in catch_unwind so a kernel assertion
        // (e.g. `add_loop` invariant) turns into a per-root failure
        // instead of aborting the whole scene. AssertUnwindSafe is safe
        // here: cache/timings populated for earlier nodes stay valid
        // even if this root's evaluation panics mid-way.
        let eval_outcome = catch_unwind(AssertUnwindSafe(
            || -> Result<(EvaluatedMesh, Option<Solid>), EvalError> {
                match evaluate_node_timed(
                    entry.root,
                    &doc.nodes,
                    &mut cache,
                    clock,
                    &mut node_timings,
                )? {
                    Some(s) => {
                        let t_mesh = clock.map(|c| c.now_ms());
                        let tri = s.to_mesh(32);
                        if let Some(t0) = t_mesh {
                            let ms = clock.unwrap().now_ms() - t0;
                            tessellate_ms += ms;
                            if let Some(nt) = node_timings.get_mut(&entry.root.to_string()) {
                                nt.mesh_ms = ms;
                            }
                        }
                        Ok((tri_to_evaluated_render(tri), Some(s)))
                    }
                    None => Ok((EvaluatedMesh::empty(), None)),
                }
            },
        ));

        match eval_outcome {
            Ok(Ok((mesh, solid))) => {
                parts.push(EvaluatedPart {
                    mesh,
                    material: entry.material.clone(),
                    solid: solid.clone(),
                });
                solids.push(solid);
            }
            Ok(Err(err)) => {
                failures.push(RootFailure {
                    scope: format!("root[{idx}]"),
                    node_id: entry.root,
                    error: err.to_string(),
                });
                parts.push(EvaluatedPart {
                    mesh: EvaluatedMesh::empty(),
                    material: entry.material.clone(),
                    solid: None,
                });
                solids.push(None);
            }
            Err(panic_payload) => {
                failures.push(RootFailure {
                    scope: format!("root[{idx}]"),
                    node_id: entry.root,
                    error: format!("kernel panic: {}", panic_message(&panic_payload)),
                });
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
    let t_assembly = clock.map(|c| c.now_ms());

    if let (Some(pd_map), Some(inst_list)) = (&doc.part_defs, &doc.instances) {
        if !pd_map.is_empty() && !inst_list.is_empty() {
            let world_transforms = solve_forward_kinematics(doc);

            let mut eval_part_defs = Vec::new();
            let mut part_def_meshes: HashMap<String, EvaluatedMesh> = HashMap::new();

            for (id, part_def) in pd_map {
                // ImportedMesh (e.g. browser-loaded STL/DAE for a URDF link)
                // bypasses the CSG evaluator: the kernel can't turn a
                // triangle soup back into a BRep solid, so we apply the
                // accumulated transform chain to the raw vertex data and
                // hand the result to the renderer directly. URDFs use this
                // path for every link whose <visual> is a <mesh>.
                if let Some(imported) = find_imported_mesh(part_def.root, &doc.nodes) {
                    let mesh = transform_imported_mesh(&imported);
                    part_def_meshes.insert(id.clone(), mesh.clone());
                    eval_part_defs.push(EvaluatedPartDef {
                        id: id.clone(),
                        mesh,
                    });
                    continue;
                }

                let outcome =
                    catch_unwind(AssertUnwindSafe(|| -> Result<EvaluatedMesh, EvalError> {
                        match evaluate_node_timed(
                            part_def.root,
                            &doc.nodes,
                            &mut cache,
                            clock,
                            &mut node_timings,
                        )? {
                            Some(s) => {
                                let t_mesh = clock.map(|c| c.now_ms());
                                let tri = s.to_mesh(32);
                                if let Some(t0) = t_mesh {
                                    tessellate_ms += clock.unwrap().now_ms() - t0;
                                }
                                Ok(tri_to_evaluated_render(tri))
                            }
                            None => Ok(EvaluatedMesh::empty()),
                        }
                    }));
                let mesh = match outcome {
                    Ok(Ok(m)) => m,
                    Ok(Err(err)) => {
                        failures.push(RootFailure {
                            scope: format!("partDef[{id:?}]"),
                            node_id: part_def.root,
                            error: err.to_string(),
                        });
                        EvaluatedMesh::empty()
                    }
                    Err(panic_payload) => {
                        failures.push(RootFailure {
                            scope: format!("partDef[{id:?}]"),
                            node_id: part_def.root,
                            error: format!("kernel panic: {}", panic_message(&panic_payload)),
                        });
                        EvaluatedMesh::empty()
                    }
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

    let assembly_ms = match (t_assembly, clock) {
        (Some(t0), Some(c)) => c.now_ms() - t0,
        _ => 0.0,
    };

    // Clash detection
    let mut clashes = Vec::new();
    let t_clash = clock.map(|c| c.now_ms());
    if !options.skip_clash_detection && solids.len() >= 2 {
        for i in 0..solids.len() {
            for j in (i + 1)..solids.len() {
                if let (Some(a), Some(b)) = (&solids[i], &solids[j]) {
                    let intersection = a.intersection(b);
                    if !intersection.is_empty() {
                        let tri = intersection.to_mesh(16);
                        if !tri.vertices.is_empty() {
                            clashes.push(tri_to_evaluated(&tri));
                        }
                    }
                }
            }
        }
    }
    let clash_ms = match (t_clash, clock) {
        (Some(t0), Some(c)) => c.now_ms() - t0,
        _ => 0.0,
    };

    let timing = t_start.map(|t0| EvalTiming {
        total_ms: clock.unwrap().now_ms() - t0,
        parse_ms: None,
        serialize_ms: None,
        tessellate_ms,
        clash_ms,
        assembly_ms,
        nodes: node_timings,
    });

    Ok(EvaluatedScene {
        parts,
        part_defs,
        instances,
        clashes,
        failures,
        timing,
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

/// Recursively evaluate a node with timing instrumentation.
fn evaluate_node_timed(
    node_id: NodeId,
    nodes: &HashMap<NodeId, vcad_ir::Node>,
    cache: &mut HashMap<NodeId, Option<Solid>>,
    clock: Option<&dyn Clock>,
    timings: &mut HashMap<String, NodeTiming>,
) -> Result<Option<Solid>, EvalError> {
    if let Some(cached) = cache.get(&node_id) {
        return Ok(cached.clone());
    }

    let node = nodes.get(&node_id).ok_or(EvalError::MissingNode(node_id))?;

    let t0 = clock.map(|c| c.now_ms());
    let result = evaluate_op_timed(&node.op, nodes, cache, clock, timings)?;
    if let Some(t0) = t0 {
        let eval_ms = clock.unwrap().now_ms() - t0;
        timings.insert(
            node_id.to_string(),
            NodeTiming {
                op: op_name(&node.op),
                eval_ms,
                mesh_ms: 0.0, // filled in by caller during tessellation
            },
        );
    }

    cache.insert(node_id, result.clone());
    Ok(result)
}

fn evaluate_op(
    op: &CsgOp,
    nodes: &HashMap<NodeId, vcad_ir::Node>,
    cache: &mut HashMap<NodeId, Option<Solid>>,
) -> Result<Option<Solid>, EvalError> {
    evaluate_op_timed(op, nodes, cache, None, &mut HashMap::new())
}

fn evaluate_op_timed(
    op: &CsgOp,
    nodes: &HashMap<NodeId, vcad_ir::Node>,
    cache: &mut HashMap<NodeId, Option<Solid>>,
    clock: Option<&dyn Clock>,
    timings: &mut HashMap<String, NodeTiming>,
) -> Result<Option<Solid>, EvalError> {
    // Helper to evaluate child nodes with timing
    let mut eval_child = |id: NodeId,
                          cache: &mut HashMap<NodeId, Option<Solid>>|
     -> Result<Option<Solid>, EvalError> {
        evaluate_node_timed(id, nodes, cache, clock, timings)
    };

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
            let l = eval_child(*left, cache)?;
            let r = eval_child(*right, cache)?;
            match (l, r) {
                (Some(l), Some(r)) => Ok(Some(l.union(&r))),
                (Some(l), None) => Ok(Some(l)),
                (None, Some(r)) => Ok(Some(r)),
                (None, None) => Ok(None),
            }
        }

        CsgOp::Difference { left, right } => {
            let l = eval_child(*left, cache)?;
            let r = eval_child(*right, cache)?;
            match (l, r) {
                (Some(l), Some(r)) => Ok(Some(l.difference(&r))),
                (Some(l), None) => Ok(Some(l)),
                _ => Ok(None),
            }
        }

        CsgOp::Intersection { left, right } => {
            let l = eval_child(*left, cache)?;
            let r = eval_child(*right, cache)?;
            match (l, r) {
                (Some(l), Some(r)) => Ok(Some(l.intersection(&r))),
                _ => Ok(None),
            }
        }

        CsgOp::Translate { .. } | CsgOp::Rotate { .. } | CsgOp::Scale { .. } => {
            // Fuse chains of Translate/Rotate/Scale into a single transform
            // to avoid cloning the BRep once per transform node.
            let (composed, inner_child) = collect_transform_chain(op, nodes);
            let c = eval_child(inner_child, cache)?;
            Ok(c.map(|s| s.apply_transform(&composed)))
        }

        CsgOp::LinearPattern {
            child,
            direction,
            count,
            spacing,
        } => {
            let c = eval_child(*child, cache)?;
            Ok(c.map(|s| s.linear_pattern(to_vec3(direction), *count, *spacing)))
        }

        CsgOp::CircularPattern {
            child,
            axis_origin,
            axis_dir,
            count,
            angle_deg,
        } => {
            let c = eval_child(*child, cache)?;
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
            let c = eval_child(*child, cache)?;
            Ok(c.map(|s| s.shell(*thickness)))
        }

        CsgOp::Fillet { child, radius } => {
            let c = eval_child(*child, cache)?;
            Ok(c.map(|s| s.fillet(*radius)))
        }

        CsgOp::Chamfer { child, distance } => {
            let c = eval_child(*child, cache)?;
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

        // STL meshes are loaded by the physics path directly; the editor's
        // CSG evaluator currently has no path from a triangle soup to a
        // BRep solid, so we surface a None and let downstream code skip the
        // mesh-derived steps for this part.
        CsgOp::MeshImport { .. } => Ok(None),

        CsgOp::PcbBoard { board } => {
            // Extrude the board outline into a 3D solid.
            let verts = &board.outline.vertices;
            if verts.len() < 3 {
                return Ok(None);
            }

            // Build a Sketch2D profile from the outline vertices (XY plane).
            let mut segments = Vec::with_capacity(verts.len());
            for i in 0..verts.len() {
                let next = (i + 1) % verts.len();
                segments.push(vcad_ir::SketchSegment2D::Line {
                    start: verts[i],
                    end: verts[next],
                });
            }

            let origin = vcad_ir::Vec3::new(0.0, 0.0, 0.0);
            let x_dir = vcad_ir::Vec3::new(1.0, 0.0, 0.0);
            let y_dir = vcad_ir::Vec3::new(0.0, 1.0, 0.0);

            let profile = ir_sketch_to_profile(&origin, &x_dir, &y_dir, &segments)
                .map_err(EvalError::Sketch)?;

            let dir = Vec3::new(0.0, 0.0, board.outline.thickness);
            let mut board_solid = Solid::extrude(profile, dir).map_err(EvalError::Sketch)?;

            // Subtract cutout holes
            for cutout in &board.outline.cutouts {
                if cutout.len() < 3 {
                    continue;
                }
                let mut cut_segs = Vec::with_capacity(cutout.len());
                for i in 0..cutout.len() {
                    let next = (i + 1) % cutout.len();
                    cut_segs.push(vcad_ir::SketchSegment2D::Line {
                        start: cutout[i],
                        end: cutout[next],
                    });
                }
                if let Ok(cut_profile) = ir_sketch_to_profile(&origin, &x_dir, &y_dir, &cut_segs) {
                    // Extrude slightly taller to ensure clean boolean
                    let cut_dir = Vec3::new(0.0, 0.0, board.outline.thickness * 1.1);
                    if let Ok(cut_solid) = Solid::extrude(cut_profile, cut_dir) {
                        board_solid = board_solid.difference(&cut_solid);
                    }
                }
            }

            // Add component bounding boxes estimated from footprint pad extents
            for fp in &board.footprints {
                if fp.pads.is_empty() {
                    continue;
                }
                let mut min_x = f64::INFINITY;
                let mut min_y = f64::INFINITY;
                let mut max_x = f64::NEG_INFINITY;
                let mut max_y = f64::NEG_INFINITY;
                for pad in &fp.pads {
                    let (pw, ph) = pad_extent(&pad.shape);
                    min_x = min_x.min(pad.position.x - pw / 2.0);
                    max_x = max_x.max(pad.position.x + pw / 2.0);
                    min_y = min_y.min(pad.position.y - ph / 2.0);
                    max_y = max_y.max(pad.position.y + ph / 2.0);
                }
                let w = max_x - min_x;
                let h = max_y - min_y;
                let comp_h = 1.0; // component height estimate (mm)
                let cx = fp.position.x + (min_x + max_x) / 2.0;
                let cy = fp.position.y + (min_y + max_y) / 2.0;

                let comp_box = Solid::cube(w, h, comp_h);
                let z_off = if fp.front {
                    board.outline.thickness
                } else {
                    -comp_h
                };
                let placed = comp_box.apply_transform(&Transform::translation(cx, cy, z_off));
                board_solid = board_solid.union(&placed);
            }

            // Generate copper feature meshes
            let mut copper_meshes: Vec<RawMesh> = Vec::new();
            for trace in &board.traces {
                let m = trace_to_mesh(trace, board);
                if !m.0.is_empty() {
                    copper_meshes.push(m);
                }
            }
            for via in &board.vias {
                copper_meshes.push(via_to_mesh(via, board, 16));
            }
            for fp in &board.footprints {
                for pad in &fp.pads {
                    let m = pad_to_mesh(pad, fp, board);
                    if !m.0.is_empty() {
                        copper_meshes.push(m);
                    }
                }
            }
            for zone in &board.zones {
                let m = zone_to_mesh(zone, board);
                if !m.0.is_empty() {
                    copper_meshes.push(m);
                }
            }

            // Merge copper into the board solid's mesh
            if !copper_meshes.is_empty() {
                let (copper_positions, copper_indices) = merge_copper_meshes(&copper_meshes);
                let board_mesh = board_solid.to_mesh(32);

                // Combine board mesh + copper mesh
                let mut all_verts = board_mesh.vertices.clone();
                let mut all_indices = board_mesh.indices.clone();
                let vert_offset = (all_verts.len() / 3) as u32;

                all_verts.extend_from_slice(&copper_positions);
                for idx in &copper_indices {
                    all_indices.push(idx + vert_offset);
                }

                let merged = TriangleMesh {
                    vertices: all_verts,
                    indices: all_indices,
                    normals: vec![],
                    face_kinds: vec![],
                };
                board_solid = Solid::from_mesh(merged);
            }

            Ok(Some(board_solid))
        }

        CsgOp::EmbroideryPattern { .. } => {
            // Embroidery is 2D — no 3D solid.
            Ok(None)
        }

        CsgOp::PartInstance { .. } => {
            // PartInstance is expanded by the engine (TS) before kernel evaluation.
            // If we see one here it's a usage error, not an internal invariant —
            // surface nothing rather than crashing so the kernel can still partially evaluate.
            Ok(None)
        }

        CsgOp::SheetMetalBaseFlangeRect { .. } | CsgOp::SheetMetalEdgeFlange { .. } => {
            // Sheet-metal ops bypass the BRep Solid pipeline — the engine
            // detects them at root level and routes the chain to the
            // sheet-metal kernel. Returning `None` here is safe: it just
            // means nothing combines as a sub-solid, which is what we want.
            Ok(None)
        }
    }
}

/// Walk a chain of Translate/Rotate/Scale nodes and compose into a single Transform.
/// Returns the composed transform and the innermost non-transform child node ID.
fn collect_transform_chain(
    op: &CsgOp,
    nodes: &HashMap<NodeId, vcad_ir::Node>,
) -> (Transform, NodeId) {
    let mut composed = Transform::identity();
    let mut current_op = op;

    loop {
        match current_op {
            CsgOp::Translate { child, offset } => {
                composed = composed.then(&Transform::translation(offset.x, offset.y, offset.z));
                match nodes.get(child) {
                    Some(node) if is_transform_op(&node.op) => current_op = &node.op,
                    _ => return (composed, *child),
                }
            }
            CsgOp::Rotate { child, angles } => {
                let rx = Transform::rotation_x(angles.x.to_radians());
                let ry = Transform::rotation_y(angles.y.to_radians());
                let rz = Transform::rotation_z(angles.z.to_radians());
                composed = composed.then(&rx.then(&ry).then(&rz));
                match nodes.get(child) {
                    Some(node) if is_transform_op(&node.op) => current_op = &node.op,
                    _ => return (composed, *child),
                }
            }
            CsgOp::Scale { child, factor } => {
                composed = composed.then(&Transform::scale(factor.x, factor.y, factor.z));
                match nodes.get(child) {
                    Some(node) if is_transform_op(&node.op) => current_op = &node.op,
                    _ => return (composed, *child),
                }
            }
            _ => unreachable!("collect_transform_chain called with non-transform op"),
        }
    }
}

fn is_transform_op(op: &CsgOp) -> bool {
    matches!(
        op,
        CsgOp::Translate { .. } | CsgOp::Rotate { .. } | CsgOp::Scale { .. }
    )
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
            face_kinds: Vec::new(),
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
        face_kinds: None,
    }
}

/// Get a short human-readable name for a CsgOp variant.
fn op_name(op: &CsgOp) -> String {
    match op {
        CsgOp::Cube { .. } => "Cube",
        CsgOp::Cylinder { .. } => "Cylinder",
        CsgOp::Sphere { .. } => "Sphere",
        CsgOp::Cone { .. } => "Cone",
        CsgOp::Empty => "Empty",
        CsgOp::Union { .. } => "Union",
        CsgOp::Difference { .. } => "Difference",
        CsgOp::Intersection { .. } => "Intersection",
        CsgOp::Translate { .. } => "Translate",
        CsgOp::Rotate { .. } => "Rotate",
        CsgOp::Scale { .. } => "Scale",
        CsgOp::LinearPattern { .. } => "LinearPattern",
        CsgOp::CircularPattern { .. } => "CircularPattern",
        CsgOp::Shell { .. } => "Shell",
        CsgOp::Fillet { .. } => "Fillet",
        CsgOp::Chamfer { .. } => "Chamfer",
        CsgOp::Sketch2D { .. } => "Sketch2D",
        CsgOp::Text2D { .. } => "Text2D",
        CsgOp::Extrude { .. } => "Extrude",
        CsgOp::Revolve { .. } => "Revolve",
        CsgOp::Sweep { .. } => "Sweep",
        CsgOp::Loft { .. } => "Loft",
        CsgOp::ImportedMesh { .. } => "ImportedMesh",
        CsgOp::StepImport { .. } => "StepImport",
        CsgOp::MeshImport { .. } => "MeshImport",
        CsgOp::PcbBoard { .. } => "PcbBoard",
        CsgOp::EmbroideryPattern { .. } => "EmbroideryPattern",
        CsgOp::PartInstance { .. } => "PartInstance",
        CsgOp::SheetMetalBaseFlangeRect { .. } => "SheetMetalBaseFlangeRect",
        CsgOp::SheetMetalEdgeFlange { .. } => "SheetMetalEdgeFlange",
    }
    .to_string()
}

/// Estimate the width/height extent of a pad shape.
fn pad_extent(shape: &PadShape) -> (f64, f64) {
    match shape {
        PadShape::Circle { diameter } => (*diameter, *diameter),
        PadShape::Rect { width, height }
        | PadShape::Oval { width, height }
        | PadShape::RoundRect { width, height, .. } => (*width, *height),
        PadShape::Custom { vertices } => {
            if vertices.is_empty() {
                return (0.5, 0.5);
            }
            let mut min_x = f64::INFINITY;
            let mut max_x = f64::NEG_INFINITY;
            let mut min_y = f64::INFINITY;
            let mut max_y = f64::NEG_INFINITY;
            for v in vertices {
                min_x = min_x.min(v.x);
                max_x = max_x.max(v.x);
                min_y = min_y.min(v.y);
                max_y = max_y.max(v.y);
            }
            (max_x - min_x, max_y - min_y)
        }
    }
}

// ============================================================================
// Copper mesh generation helpers
// ============================================================================

/// A raw triangle mesh: vertices as [x,y,z] and triangle indices as [a,b,c].
type RawMesh = (Vec<[f64; 3]>, Vec<[u32; 3]>);

/// Default copper thickness (mm) if not specified in stackup.
const DEFAULT_COPPER_THICKNESS: f64 = 0.035;

/// Z offset for the top of a copper layer.
fn layer_z_top(pcb: &Pcb, layer: PcbLayer) -> f64 {
    match layer {
        PcbLayer::FCu => pcb.outline.thickness,
        PcbLayer::BCu => 0.0,
        _ => pcb.outline.thickness / 2.0,
    }
}

/// Copper thickness from the stackup, falling back to default.
fn copper_thickness(pcb: &Pcb, layer: PcbLayer) -> f64 {
    pcb.stackup
        .layers
        .iter()
        .find(|l| l.layer == layer)
        .and_then(|l| l.copper_thickness)
        .unwrap_or(DEFAULT_COPPER_THICKNESS)
}

/// Generate a box mesh for a trace segment (oriented ribbon at layer Z).
/// Returns (vertices [x,y,z], triangle indices).
fn trace_to_mesh(trace: &Trace, pcb: &Pcb) -> RawMesh {
    let z_top = layer_z_top(pcb, trace.layer);
    let ct = copper_thickness(pcb, trace.layer);
    let z_bot = if trace.layer == PcbLayer::FCu {
        z_top - ct
    } else {
        z_top + ct
    };

    let dx = trace.end.x - trace.start.x;
    let dy = trace.end.y - trace.start.y;
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-9 {
        return (vec![], vec![]);
    }

    // Perpendicular half-width offset
    let hw = trace.width / 2.0;
    let nx = -dy / len * hw;
    let ny = dx / len * hw;

    let s = trace.start;
    let e = trace.end;

    // 8 vertices: 4 top, 4 bottom
    let verts = vec![
        // top face
        [s.x + nx, s.y + ny, z_top], // 0
        [e.x + nx, e.y + ny, z_top], // 1
        [e.x - nx, e.y - ny, z_top], // 2
        [s.x - nx, s.y - ny, z_top], // 3
        // bottom face
        [s.x + nx, s.y + ny, z_bot], // 4
        [e.x + nx, e.y + ny, z_bot], // 5
        [e.x - nx, e.y - ny, z_bot], // 6
        [s.x - nx, s.y - ny, z_bot], // 7
    ];

    let tris = vec![
        // top
        [0, 1, 2],
        [0, 2, 3],
        // bottom
        [4, 6, 5],
        [4, 7, 6],
        // front
        [0, 5, 1],
        [0, 4, 5],
        // back
        [2, 7, 3],
        [2, 6, 7],
        // left
        [0, 3, 7],
        [0, 7, 4],
        // right
        [1, 5, 6],
        [1, 6, 2],
    ];

    (verts, tris)
}

/// Generate a cylindrical via mesh (outer cylinder + top/bottom annular rings).
fn via_to_mesh(via: &Via, pcb: &Pcb, n_seg: usize) -> RawMesh {
    let z_top = pcb.outline.thickness;
    let z_bot = 0.0;
    let r_outer = via.diameter / 2.0;
    let r_inner = via.drill / 2.0;
    let cx = via.position.x;
    let cy = via.position.y;

    let mut verts = Vec::new();
    let mut tris = Vec::new();

    // Generate circle points
    let angles: Vec<f64> = (0..n_seg)
        .map(|i| 2.0 * std::f64::consts::PI * i as f64 / n_seg as f64)
        .collect();

    // Outer cylinder: top ring (0..n_seg), bottom ring (n_seg..2*n_seg)
    for &a in &angles {
        let x = cx + r_outer * a.cos();
        let y = cy + r_outer * a.sin();
        verts.push([x, y, z_top]);
    }
    for &a in &angles {
        let x = cx + r_outer * a.cos();
        let y = cy + r_outer * a.sin();
        verts.push([x, y, z_bot]);
    }

    // Outer cylinder side faces
    let n = n_seg as u32;
    for i in 0..n {
        let next = (i + 1) % n;
        // top-ring[i], top-ring[next], bot-ring[next], bot-ring[i]
        tris.push([i, next, n + next]);
        tris.push([i, n + next, n + i]);
    }

    // Inner cylinder (drill hole): top ring, bottom ring
    let inner_top_start = verts.len() as u32;
    for &a in &angles {
        let x = cx + r_inner * a.cos();
        let y = cy + r_inner * a.sin();
        verts.push([x, y, z_top]);
    }
    let inner_bot_start = verts.len() as u32;
    for &a in &angles {
        let x = cx + r_inner * a.cos();
        let y = cy + r_inner * a.sin();
        verts.push([x, y, z_bot]);
    }

    // Inner cylinder side faces (reversed winding — faces inward)
    for i in 0..n {
        let next = (i + 1) % n;
        tris.push([
            inner_top_start + i,
            inner_bot_start + next,
            inner_top_start + next,
        ]);
        tris.push([
            inner_top_start + i,
            inner_bot_start + i,
            inner_bot_start + next,
        ]);
    }

    // Top annular ring: connects outer top ring to inner top ring
    for i in 0..n {
        let next = (i + 1) % n;
        tris.push([i, inner_top_start + next, inner_top_start + i]);
        tris.push([i, next, inner_top_start + next]);
    }

    // Bottom annular ring: connects outer bottom ring to inner bottom ring
    for i in 0..n {
        let next = (i + 1) % n;
        tris.push([n + i, inner_bot_start + i, inner_bot_start + next]);
        tris.push([n + i, inner_bot_start + next, n + next]);
    }

    (verts, tris)
}

/// Generate a mesh for a pad on a footprint, positioned in board space.
fn pad_to_mesh(pad: &Pad, fp: &Footprint, pcb: &Pcb) -> RawMesh {
    let (pw, ph) = pad_extent(&pad.shape);
    if pw < 1e-9 || ph < 1e-9 {
        return (vec![], vec![]);
    }

    // Determine which copper layer this pad lives on
    let layer = pad
        .layers
        .iter()
        .find(|l| l.is_copper())
        .copied()
        .unwrap_or(if fp.front {
            PcbLayer::FCu
        } else {
            PcbLayer::BCu
        });

    let z_top = layer_z_top(pcb, layer);
    let ct = copper_thickness(pcb, layer);
    let z_bot = if layer == PcbLayer::FCu {
        z_top - ct
    } else {
        z_top + ct
    };

    // Pad position in board space (footprint position + pad offset)
    let rot_rad = fp.rotation.to_radians();
    let cos_r = rot_rad.cos();
    let sin_r = rot_rad.sin();
    let px = fp.position.x + pad.position.x * cos_r - pad.position.y * sin_r;
    let py = fp.position.y + pad.position.x * sin_r + pad.position.y * cos_r;

    let hw = pw / 2.0;
    let hh = ph / 2.0;

    // Total rotation = footprint rotation + pad rotation
    let pad_rot = (fp.rotation + pad.rotation).to_radians();
    let cp = pad_rot.cos();
    let sp = pad_rot.sin();

    // 4 corners of the pad rectangle, rotated
    let corners = [(-hw, -hh), (hw, -hh), (hw, hh), (-hw, hh)];

    let mut verts = Vec::with_capacity(8);
    for &(lx, ly) in &corners {
        let rx = lx * cp - ly * sp + px;
        let ry = lx * sp + ly * cp + py;
        verts.push([rx, ry, z_top]);
    }
    for &(lx, ly) in &corners {
        let rx = lx * cp - ly * sp + px;
        let ry = lx * sp + ly * cp + py;
        verts.push([rx, ry, z_bot]);
    }

    let tris = vec![
        [0, 1, 2],
        [0, 2, 3], // top
        [4, 6, 5],
        [4, 7, 6], // bottom
        [0, 5, 1],
        [0, 4, 5], // front
        [2, 7, 3],
        [2, 6, 7], // back
        [0, 3, 7],
        [0, 7, 4], // left
        [1, 5, 6],
        [1, 6, 2], // right
    ];

    (verts, tris)
}

/// Generate a mesh for a copper zone (fan-triangulated polygon extruded by copper thickness).
fn zone_to_mesh(zone: &Zone, pcb: &Pcb) -> RawMesh {
    if zone.outline.len() < 3 {
        return (vec![], vec![]);
    }

    let z_top = layer_z_top(pcb, zone.layer);
    let ct = copper_thickness(pcb, zone.layer);
    let z_bot = if zone.layer == PcbLayer::FCu {
        z_top - ct
    } else {
        z_top + ct
    };

    let n = zone.outline.len();
    let mut verts = Vec::with_capacity(n * 2);
    let mut tris = Vec::new();

    // Top vertices
    for v in &zone.outline {
        verts.push([v.x, v.y, z_top]);
    }
    // Bottom vertices
    for v in &zone.outline {
        verts.push([v.x, v.y, z_bot]);
    }

    let nu = n as u32;
    // Top face (fan triangulation)
    for i in 1..nu - 1 {
        tris.push([0, i, i + 1]);
    }
    // Bottom face (reversed winding)
    for i in 1..nu - 1 {
        tris.push([nu, nu + i + 1, nu + i]);
    }
    // Side faces
    for i in 0..nu {
        let next = (i + 1) % nu;
        tris.push([i, next, nu + next]);
        tris.push([i, nu + next, nu + i]);
    }

    (verts, tris)
}

/// Merge multiple meshes into a single flat vertex/index buffer (f32 positions, u32 indices).
fn merge_copper_meshes(meshes: &[RawMesh]) -> (Vec<f32>, Vec<u32>) {
    let total_verts: usize = meshes.iter().map(|(v, _)| v.len()).sum();
    let total_tris: usize = meshes.iter().map(|(_, t)| t.len()).sum();

    let mut positions = Vec::with_capacity(total_verts * 3);
    let mut indices = Vec::with_capacity(total_tris * 3);
    let mut vert_offset: u32 = 0;

    for (verts, tris) in meshes {
        for v in verts {
            positions.push(v[0] as f32);
            positions.push(v[1] as f32);
            positions.push(v[2] as f32);
        }
        for tri in tris {
            indices.push(tri[0] + vert_offset);
            indices.push(tri[1] + vert_offset);
            indices.push(tri[2] + vert_offset);
        }
        vert_offset += verts.len() as u32;
    }

    (positions, indices)
}

/// Best-effort extraction of a human-readable message from a panic payload.
fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown kernel panic".to_string()
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
        face_kinds: if tri.face_kinds.len() == tri.indices.len() / 3 {
            Some(tri.face_kinds.clone())
        } else {
            None
        },
    }
}

/// Convert kernel TriangleMesh to EvaluatedMesh after running the
/// render-bake pipeline.
///
/// Use this at every call site that produces a mesh for the renderer,
/// STL/GLB export, or the ray tracer so they all receive a single
/// consistent shading pipeline (crease-aware vertex normals today, more
/// render-only transforms in the future) independent of which tessellator
/// produced the mesh. The output is unindexed.
fn tri_to_evaluated_render(mut tri: TriangleMesh) -> EvaluatedMesh {
    vcad_kernel_tessellate::render_bake_default(&mut tri);
    let tri_count = tri.indices.len() / 3;
    EvaluatedMesh {
        positions: tri.vertices,
        indices: tri.indices,
        normals: if tri.normals.is_empty() {
            None
        } else {
            Some(tri.normals)
        },
        face_kinds: if tri.face_kinds.len() == tri_count {
            Some(tri.face_kinds)
        } else {
            None
        },
    }
}
