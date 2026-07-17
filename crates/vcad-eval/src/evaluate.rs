//! Core document evaluator.
//!
//! Walks the IR DAG and calls vcad-kernel operations to produce meshes.
//! Ported from `packages/engine/src/evaluate.ts`.

use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};

use vcad_ir::ecad::{Footprint, Pad, PadShape, Pcb, PcbLayer, Trace, TraceArc, Via, Zone};
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

thread_local! {
    /// When set, foundation-tier sheet-metal ops (rect/polygon base flange + edge
    /// flange) fold to a real BRep solid during `evaluate_document`. OFF by
    /// default: the web/MCP engine REQUIRES sheet-metal roots to evaluate empty
    /// here so its own `evaluateSheetMetalChain` (unfold/DXF/DFM) takes over. Only
    /// the kernel-direct native FFI opts in, via `evaluate_document_with_sheet_metal`.
    static FOLD_SHEET_METAL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// RAII guard: turn sheet-metal folding on for the current thread and restore it
/// on drop (panic-safe — the flag never leaks past the call).
struct FoldGuard(bool);
impl FoldGuard {
    fn enable() -> Self {
        FoldGuard(FOLD_SHEET_METAL.with(|f| f.replace(true)))
    }
}
impl Drop for FoldGuard {
    fn drop(&mut self) {
        FOLD_SHEET_METAL.with(|f| f.set(self.0));
    }
}

/// Like [`evaluate_document`], but foundation-tier sheet-metal ops fold to real
/// BRep solids (the kernel-direct path used by the native app). The default
/// [`evaluate_document`] leaves them empty so the web/MCP engine's sheet-metal
/// fallback can take over — do NOT use this from the WASM/web path.
pub fn evaluate_document_with_sheet_metal(
    doc: &Document,
    options: &EvalOptions,
) -> Result<EvaluatedScene, EvalError> {
    let _guard = FoldGuard::enable();
    evaluate_document(doc, options)
}

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

    // Clash detection. Broadphase first: blind pairwise `intersection`
    // across all roots is O(n²) BRep booleans, which is fatal for
    // many-root scenes (an imported chip die has ~90k roots). Compute
    // each solid's AABB once, sort by min-x, and sweep — only pairs whose
    // boxes overlap reach the kernel. AABB-disjoint pairs intersect to
    // empty anyway, so skipping them is behavior-preserving.
    let mut clashes = Vec::new();
    let t_clash = clock.map(|c| c.now_ms());
    if !options.skip_clash_detection && solids.len() >= 2 {
        let boxes: Vec<Option<([f64; 3], [f64; 3])>> = solids
            .iter()
            .map(|s| s.as_ref().map(|s| s.bounding_box()))
            .collect();
        let mut order: Vec<usize> = (0..solids.len()).filter(|&i| boxes[i].is_some()).collect();
        order.sort_by(|&a, &b| {
            let ax = boxes[a].as_ref().unwrap().0[0];
            let bx = boxes[b].as_ref().unwrap().0[0];
            ax.partial_cmp(&bx).unwrap_or(std::cmp::Ordering::Equal)
        });
        for (k, &i) in order.iter().enumerate() {
            let (min_i, max_i) = boxes[i].as_ref().unwrap();
            for &j in &order[k + 1..] {
                let (min_j, max_j) = boxes[j].as_ref().unwrap();
                if min_j[0] > max_i[0] {
                    break; // sorted by min-x: no later j can overlap i
                }
                if min_j[1] > max_i[1]
                    || max_j[1] < min_i[1]
                    || min_j[2] > max_i[2]
                    || max_j[2] < min_i[2]
                {
                    continue;
                }
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

    let mut result = evaluate_op(&node.op, nodes, cache)?;
    scope_primitive_names(node_id, &node.op, &mut result);
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
    let mut result = evaluate_op_timed(&node.op, nodes, cache, clock, timings)?;
    scope_primitive_names(node_id, &node.op, &mut result);
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

        CsgOp::Torus {
            major_radius,
            minor_radius,
            segments,
        } => Ok(Some(Solid::torus(*major_radius, *minor_radius, *segments))),

        CsgOp::Wedge { size } => Ok(Some(Solid::wedge(size.x, size.y, size.z))),

        CsgOp::Prism {
            sides,
            radius,
            height,
        } => Ok(Some(Solid::prism(*sides, *radius, *height))),

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

        CsgOp::Mirror {
            child,
            plane_origin,
            plane_normal,
        } => {
            let c = eval_child(*child, cache)?;
            Ok(c.map(|s| {
                s.mirror(
                    [plane_origin.x, plane_origin.y, plane_origin.z],
                    [plane_normal.x, plane_normal.y, plane_normal.z],
                )
            }))
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

        CsgOp::EdgeBlend {
            child,
            edges,
            profile,
        } => {
            let c = eval_child(*child, cache)?;
            if let vcad_ir::EdgeQuery::Named { face_a, face_b } = edges {
                let keys = kernel_blend_keys(profile);
                return match c {
                    Some(s) => s
                        .edge_blend_named(face_a, face_b, &keys)
                        .map(Some)
                        .map_err(|e| EvalError::NamedEdge {
                            face_a: face_a.clone(),
                            face_b: face_b.clone(),
                            message: e.to_string(),
                        }),
                    None => Ok(None),
                };
            }
            let (query, keys) = kernel_blend_args(edges, profile);
            Ok(c.map(|s| s.edge_blend(&query, &keys)))
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
            let (s_origin, s_x_dir, s_y_dir, segments, holes) = extract_sketch(&sketch_node.op)?;
            let profile = ir_sketch_to_profile(s_origin, s_x_dir, s_y_dir, segments)
                .map_err(EvalError::Sketch)?;

            let has_twist = twist_angle.is_some_and(|t| t.abs() > 1e-12);
            let has_scale = scale_end.is_some_and(|s| (s - 1.0).abs() > 1e-12);

            let solid = if !holes.is_empty() {
                if has_twist || has_scale {
                    return Err(EvalError::Sketch(
                        vcad_kernel_sketch::SketchError::HolesUnsupported(
                            "extrude with twist or taper",
                        ),
                    ));
                }
                let hole_loops = crate::convert::ir_holes_to_segments(holes);
                Solid::extrude_with_holes(profile, &hole_loops, dir).map_err(EvalError::Sketch)?
            } else if has_twist || has_scale {
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

            let (s_origin, s_x_dir, s_y_dir, segments, holes) = extract_sketch(&sketch_node.op)?;
            reject_holes(holes, "revolve")?;
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

            let (s_origin, s_x_dir, s_y_dir, segments, holes) = extract_sketch(&sketch_node.op)?;
            reject_holes(holes, "sweep")?;
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
                let (s_origin, s_x_dir, s_y_dir, segments, holes) =
                    extract_sketch(&sketch_node.op)?;
                reject_holes(holes, "loft")?;
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
                let Some((min_x, min_y, max_x, max_y)) = footprint_component_world_bbox(fp) else {
                    continue;
                };
                let w = max_x - min_x;
                let h = max_y - min_y;
                let comp_h = 1.0; // component height estimate (mm)
                let cx = (min_x + max_x) / 2.0;
                let cy = (min_y + max_y) / 2.0;

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
            for arc in &board.trace_arcs {
                let m = trace_arc_to_mesh(arc, board);
                if !m.0.is_empty() {
                    copper_meshes.push(m);
                }
            }
            for via in &board.vias {
                copper_meshes.push(via_to_mesh(via, board, 16, 0.0));
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

            // Center the board on z=0 so its top surface lands at +thickness/2,
            // where PcbScene draws the copper (layerZ = thickness/2 + …) and
            // where the legacy PcbBoardMesh sat. The outline is extruded from
            // z=0, so shift the whole board (slab + components + copper) down by
            // thickness/2.
            let board_solid = board_solid.apply_transform(&Transform::translation(
                0.0,
                0.0,
                -board.outline.thickness / 2.0,
            ));

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

        // Foundation-tier sheet metal can fold to a real BRep solid — but ONLY on
        // the opt-in kernel-direct path (the native FFI). By DEFAULT we return
        // `Ok(None)`, because the web/MCP engine's contract is that a sheet-metal
        // root evaluates EMPTY here so its `positions.length === 0` fallback routes
        // the chain to the dedicated `evaluateSheetMetalChain` (which does unfold +
        // DXF + DFM). Returning a solid (or an error) here breaks that detection.
        // The opt-in is `evaluate_document_with_sheet_metal` → `FOLD_SHEET_METAL`.
        CsgOp::SheetMetalBaseFlangeRect { .. }
        | CsgOp::SheetMetalBaseFlangePolygon { .. }
        | CsgOp::SheetMetalEdgeFlange { .. }
            if FOLD_SHEET_METAL.with(|f| f.get()) =>
        {
            let model = build_sheet_model(op, nodes)?;
            // 8 segments is plenty for a small-radius bend arc, and keeps the
            // fold's booleans cheap enough for interactive re-solve.
            let solid =
                vcad_kernel::folded_sheet_solid(&model, 8).map_err(EvalError::SheetMetal)?;
            Ok(Some(solid))
        }

        // Default path (web/MCP) + any not-yet-buildable op (hem/jog/relief, even
        // under the fold flag): return empty so the engine's sheet-metal fallback
        // takes over. Never a sub-solid, never an error — preserves the contract.
        CsgOp::SheetMetalBaseFlangeRect { .. }
        | CsgOp::SheetMetalBaseFlangePolygon { .. }
        | CsgOp::SheetMetalEdgeFlange { .. }
        | CsgOp::SheetMetalHem { .. }
        | CsgOp::SheetMetalJog { .. }
        | CsgOp::SheetMetalBendRelief { .. } => Ok(None),
    }
}

/// Resolve IR engraving primitives to root-panel-local polylines (text →
/// single-stroke font polylines via the kernel).
fn resolve_ir_engravings(
    engravings: Option<&Vec<vcad_ir::SheetMetalEngraving>>,
) -> Result<Vec<Vec<vcad_kernel_math::Point2>>, String> {
    use vcad_kernel_math::Point2;
    let mut out = Vec::new();
    for (i, e) in engravings.into_iter().flatten().enumerate() {
        match e {
            vcad_ir::SheetMetalEngraving::Polyline { points } => {
                if points.len() < 2 {
                    return Err(format!("engraving #{i}: polyline needs >= 2 points"));
                }
                out.push(points.iter().map(|p| Point2::new(p.x, p.y)).collect());
            }
            vcad_ir::SheetMetalEngraving::Text {
                text,
                x,
                y,
                height,
                angle,
            } => {
                let strokes = vcad_kernel::vcad_kernel_sheet::text_to_polylines(
                    text, *x, *y, *height, *angle,
                )
                .map_err(|e| format!("engraving #{i} ({text:?}): {e}"))?;
                out.extend(strokes);
            }
        }
    }
    Ok(out)
}

/// Build a [`vcad_kernel_sheet::SheetMetalModel`] from a sheet-metal op chain.
/// Edge flanges reference their `parent` node, so this recurses down the chain
/// (mirroring [`collect_transform_chain`]) and applies each flange in order.
/// Handles rectangular + polygon base flanges and edge flanges; the model's
/// `material` is threaded from the IR (NOT hardcoded), so the bend table picks
/// the right K-factor when no `manual_k` override is given.
fn build_sheet_model(
    op: &CsgOp,
    nodes: &HashMap<NodeId, vcad_ir::Node>,
) -> Result<vcad_kernel::vcad_kernel_sheet::SheetMetalModel, EvalError> {
    use vcad_kernel::vcad_kernel_sheet::{
        add_edge_flange, base_flange_polygon_with_holes, base_flange_rect,
        edge_flange::EdgeFlangeParams, BendDirection, BendTable, FlangePosition,
    };
    use vcad_kernel_math::Point2;
    let sm = |e: String| EvalError::SheetMetal(e);
    match op {
        CsgOp::SheetMetalBaseFlangeRect {
            width,
            depth,
            thickness,
            material,
            engravings,
            ..
        } => {
            let mut model =
                base_flange_rect(*width, *depth, *thickness).map_err(|e| sm(e.to_string()))?;
            model.material = material.clone();
            model.engravings = resolve_ir_engravings(engravings.as_ref()).map_err(sm)?;
            Ok(model)
        }

        CsgOp::SheetMetalBaseFlangePolygon {
            outline,
            holes,
            thickness,
            material,
            engravings,
            ..
        } => {
            let to_pts = |v: &Vec<vcad_ir::Vec2>| {
                v.iter().map(|p| Point2::new(p.x, p.y)).collect::<Vec<_>>()
            };
            let outer = to_pts(outline);
            let hole_loops: Vec<Vec<Point2>> = holes.iter().map(to_pts).collect();
            let mut model = base_flange_polygon_with_holes(outer, hole_loops, *thickness)
                .map_err(|e| sm(e.to_string()))?;
            model.material = material.clone();
            model.engravings = resolve_ir_engravings(engravings.as_ref()).map_err(sm)?;
            Ok(model)
        }

        CsgOp::SheetMetalEdgeFlange {
            parent,
            panel_id,
            edge_index,
            length,
            angle,
            radius,
            direction,
            manual_k,
        } => {
            let parent_op = &nodes.get(parent).ok_or(EvalError::MissingNode(*parent))?.op;
            let mut model = build_sheet_model(parent_op, nodes)?;
            let r = radius.unwrap_or(model.thickness);
            let material = model.material.clone();
            add_edge_flange(
                &mut model,
                &BendTable::builtin(),
                EdgeFlangeParams {
                    panel: *panel_id,
                    edge_index: *edge_index,
                    length: *length,
                    angle: *angle,
                    radius: r,
                    direction: match direction {
                        vcad_ir::SheetMetalDirection::Up => BendDirection::Up,
                        vcad_ir::SheetMetalDirection::Down => BendDirection::Down,
                    },
                    position: FlangePosition::MaterialInside,
                    // Inherit the base flange's material so the bend-table
                    // K-factor matches the actual sheet (steel ≠ aluminium).
                    material,
                    manual_k: *manual_k,
                },
            )
            .map_err(|e| sm(e.to_string()))?;
            Ok(model)
        }

        _ => Err(sm(
            "sheet-metal chain references hem/jog/bend-relief, not yet buildable in \
             kernel-direct eval"
                .into(),
        )),
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

/// Error out when a sketch with interior holes reaches an operation that
/// doesn't support them (only extrude does today).
fn reject_holes(
    holes: &[Vec<vcad_ir::SketchSegment2D>],
    op_name: &'static str,
) -> Result<(), EvalError> {
    if holes.is_empty() {
        Ok(())
    } else {
        Err(EvalError::Sketch(
            vcad_kernel_sketch::SketchError::HolesUnsupported(op_name),
        ))
    }
}

/// Borrowed fields of a `Sketch2D`: origin, x-dir, y-dir, outer segments,
/// and interior hole loops (empty when absent).
type SketchFields<'a> = (
    &'a vcad_ir::Vec3,
    &'a vcad_ir::Vec3,
    &'a vcad_ir::Vec3,
    &'a [vcad_ir::SketchSegment2D],
    &'a [Vec<vcad_ir::SketchSegment2D>],
);

/// Extract sketch fields from a CsgOp, returning error if not a Sketch2D.
fn extract_sketch(op: &CsgOp) -> Result<SketchFields<'_>, EvalError> {
    match op {
        CsgOp::Sketch2D {
            origin,
            x_dir,
            y_dir,
            segments,
            holes,
        } => Ok((
            origin,
            x_dir,
            y_dir,
            segments,
            holes.as_deref().unwrap_or(&[]),
        )),
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

/// Convert IR edge-blend arguments to their kernel equivalents.
fn kernel_blend_args(
    edges: &vcad_ir::EdgeQuery,
    profile: &vcad_ir::BlendProfile,
) -> (
    vcad_kernel::vcad_kernel_fillet::EdgeQuery,
    Vec<vcad_kernel::vcad_kernel_fillet::BlendKey>,
) {
    use vcad_kernel::vcad_kernel_fillet as kf;
    let q = match edges {
        vcad_ir::EdgeQuery::All => kf::EdgeQuery::All,
        vcad_ir::EdgeQuery::Near { point } => kf::EdgeQuery::Near {
            point: vcad_kernel_math::Point3::new(point.x, point.y, point.z),
        },
        vcad_ir::EdgeQuery::Direction { axis, tol_deg } => kf::EdgeQuery::Direction {
            axis: vcad_kernel_math::Vec3::new(axis.x, axis.y, axis.z),
            tol_deg: *tol_deg,
        },
        // Named queries carry no geometry — they resolve against the child
        // solid's name map and never reach this translation (the EdgeBlend
        // arm routes them through `Solid::edge_blend_named`).
        vcad_ir::EdgeQuery::Named { .. } => {
            unreachable!("Named edge queries are handled before kernel_blend_args")
        }
    };
    (q, kernel_blend_keys(profile))
}

/// Translate an IR blend profile into kernel blend keys.
fn kernel_blend_keys(
    profile: &vcad_ir::BlendProfile,
) -> Vec<vcad_kernel::vcad_kernel_fillet::BlendKey> {
    use vcad_kernel::vcad_kernel_fillet as kf;
    let keys = match profile {
        vcad_ir::BlendProfile::Constant { size, shape } => vec![kf::BlendKey {
            t: 0.0,
            section: kf::BlendSection {
                size: *size,
                shape: *shape,
            },
        }],
        vcad_ir::BlendProfile::Keyed { keys } => keys
            .iter()
            .map(|k| kf::BlendKey {
                t: k.t,
                section: kf::BlendSection {
                    size: k.size,
                    shape: k.shape,
                },
            })
            .collect(),
    };
    keys
}

/// After a primitive op evaluates, rewrite its face-name scope to the
/// document node id so names stay unique across the DAG (`cube:top` →
/// `n3:top`) and stable across rebuilds (node ids persist in the .vcad
/// document).
fn scope_primitive_names(node_id: NodeId, op: &CsgOp, result: &mut Option<Solid>) {
    let is_primitive = matches!(
        op,
        CsgOp::Cube { .. }
            | CsgOp::Cylinder { .. }
            | CsgOp::Sphere { .. }
            | CsgOp::Cone { .. }
            | CsgOp::Torus { .. }
            | CsgOp::Wedge { .. }
            | CsgOp::Prism { .. }
    );
    if !is_primitive {
        return;
    }
    if let Some(solid) = result {
        solid.set_name_scope(&format!("n{node_id}"));
    }
}

/// Get a short human-readable name for a CsgOp variant.
fn op_name(op: &CsgOp) -> String {
    match op {
        CsgOp::Cube { .. } => "Cube",
        CsgOp::Cylinder { .. } => "Cylinder",
        CsgOp::Sphere { .. } => "Sphere",
        CsgOp::Cone { .. } => "Cone",
        CsgOp::Torus { .. } => "Torus",
        CsgOp::Wedge { .. } => "Wedge",
        CsgOp::Prism { .. } => "Prism",
        CsgOp::Mirror { .. } => "Mirror",
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
        CsgOp::EdgeBlend { .. } => "EdgeBlend",
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
        CsgOp::SheetMetalHem { .. } => "SheetMetalHem",
        CsgOp::SheetMetalJog { .. } => "SheetMetalJog",
        CsgOp::SheetMetalBaseFlangePolygon { .. } => "SheetMetalBaseFlangePolygon",
        CsgOp::SheetMetalBendRelief { .. } => "SheetMetalBendRelief",
    }
    .to_string()
}

/// Estimate the width/height extent of a pad shape.
/// World-space AABB `(min_x, min_y, max_x, max_y)` of a footprint's estimated
/// component box, derived from pad extents in the footprint-local frame and
/// rotated by the footprint rotation. Returns `None` for footprints with no pads.
fn footprint_component_world_bbox(fp: &vcad_ir::ecad::Footprint) -> Option<(f64, f64, f64, f64)> {
    if fp.pads.is_empty() {
        return None;
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
    let (sin_r, cos_r) = fp.rotation.to_radians().sin_cos();
    let mut wmin_x = f64::INFINITY;
    let mut wmin_y = f64::INFINITY;
    let mut wmax_x = f64::NEG_INFINITY;
    let mut wmax_y = f64::NEG_INFINITY;
    for (lx, ly) in [
        (min_x, min_y),
        (min_x, max_y),
        (max_x, min_y),
        (max_x, max_y),
    ] {
        let wx = fp.position.x + lx * cos_r - ly * sin_r;
        let wy = fp.position.y + lx * sin_r + ly * cos_r;
        wmin_x = wmin_x.min(wx);
        wmax_x = wmax_x.max(wx);
        wmin_y = wmin_y.min(wy);
        wmax_y = wmax_y.max(wy);
    }
    Some((wmin_x, wmin_y, wmax_x, wmax_y))
}

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
pub(crate) type RawMesh = (Vec<[f64; 3]>, Vec<[u32; 3]>);

/// Default copper thickness (mm) if not specified in stackup.
const DEFAULT_COPPER_THICKNESS: f64 = 0.035;

/// Z offset for the top of a copper layer.
pub(crate) fn layer_z_top(pcb: &Pcb, layer: PcbLayer) -> f64 {
    match layer {
        PcbLayer::FCu => pcb.outline.thickness,
        PcbLayer::BCu => 0.0,
        _ => pcb.outline.thickness / 2.0,
    }
}

/// Copper thickness from the stackup, falling back to default.
pub(crate) fn copper_thickness(pcb: &Pcb, layer: PcbLayer) -> f64 {
    pcb.stackup
        .layers
        .iter()
        .find(|l| l.layer == layer)
        .and_then(|l| l.copper_thickness)
        .unwrap_or(DEFAULT_COPPER_THICKNESS)
}

/// Top/bottom Z of the copper slab a trace occupies on `layer`, ordered
/// so `z_hi > z_lo` regardless of which side of the board the layer is on.
fn trace_z_span(pcb: &Pcb, layer: PcbLayer) -> (f64, f64) {
    let z_top = layer_z_top(pcb, layer);
    let ct = copper_thickness(pcb, layer);
    let z_other = if layer == PcbLayer::FCu {
        z_top - ct
    } else {
        z_top + ct
    };
    (z_top.min(z_other), z_top.max(z_other))
}

/// Segment count for a round trace end cap.
const CAP_SEGS: usize = 12;

/// Append an outward-facing cylinder (round trace end cap / joint disc) of
/// radius `r` centered at `(cx, cy)`, spanning `z_lo..z_hi`.
///
/// The fans and the side wall use *separate* vertex sets so smooth-normal
/// recomputation keeps the fans flat (±z) instead of blending them with the
/// wall — a shared ring shades every joint as a bump. The cap also overshoots
/// the slab by a hair so its coplanar fan never z-fights the segment's top.
fn append_endcap(
    verts: &mut Vec<[f64; 3]>,
    tris: &mut Vec<[u32; 3]>,
    cx: f64,
    cy: f64,
    r: f64,
    z_lo: f64,
    z_hi: f64,
) {
    let eps = (z_hi - z_lo) * 0.05;
    let (z_lo, z_hi) = (z_lo - eps, z_hi + eps);
    let n = CAP_SEGS as u32;
    let ring = |verts: &mut Vec<[f64; 3]>, z: f64| {
        let start = verts.len() as u32;
        for i in 0..CAP_SEGS {
            let a = 2.0 * std::f64::consts::PI * i as f64 / CAP_SEGS as f64;
            verts.push([cx + r * a.cos(), cy + r * a.sin(), z]);
        }
        start
    };

    // Top fan (+z).
    let tc = verts.len() as u32;
    verts.push([cx, cy, z_hi]);
    let top = ring(verts, z_hi);
    // Bottom fan (-z).
    let bc = verts.len() as u32;
    verts.push([cx, cy, z_lo]);
    let bot = ring(verts, z_lo);
    // Side wall (its own rings).
    let wt = ring(verts, z_hi);
    let wb = ring(verts, z_lo);

    for i in 0..n {
        let j = (i + 1) % n;
        tris.push([tc, top + i, top + j]);
        tris.push([bc, bot + j, bot + i]);
        tris.push([wt + i, wb + j, wt + j]);
        tris.push([wt + i, wb + i, wb + j]);
    }
}

/// Generate a box mesh for a trace segment (oriented ribbon at layer Z) with
/// round end caps, so chained segments (arc approximations, routed polylines)
/// read as continuous copper instead of disjoint tiles with gaps at joints.
/// All faces wind outward. Returns (vertices [x,y,z], triangle indices).
pub(crate) fn trace_to_mesh(trace: &Trace, pcb: &Pcb) -> RawMesh {
    let (z_lo, z_hi) = trace_z_span(pcb, trace.layer);

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

    // Corner positions: 4 top, 4 bottom.
    let c = [
        [s.x + nx, s.y + ny, z_hi], // 0
        [e.x + nx, e.y + ny, z_hi], // 1
        [e.x - nx, e.y - ny, z_hi], // 2
        [s.x - nx, s.y - ny, z_hi], // 3
        [s.x + nx, s.y + ny, z_lo], // 4
        [e.x + nx, e.y + ny, z_lo], // 5
        [e.x - nx, e.y - ny, z_lo], // 6
        [s.x - nx, s.y - ny, z_lo], // 7
    ];

    // Each face gets its own 4 vertices so smooth-normal recomputation keeps
    // faces flat — shared corners average top with walls and shade every
    // segment as a rounded sausage. Quads are outward-wound (a, b, c, d) →
    // (a,b,c) + (a,c,d).
    let quads: [[usize; 4]; 6] = [
        [0, 3, 2, 1], // top (+z)
        [4, 5, 6, 7], // bottom (-z)
        [0, 1, 5, 4], // +normal side
        [3, 7, 6, 2], // -normal side
        [0, 4, 7, 3], // start end
        [1, 2, 6, 5], // end end
    ];
    let mut verts: Vec<[f64; 3]> = Vec::with_capacity(24);
    let mut tris: Vec<[u32; 3]> = Vec::with_capacity(12);
    for q in quads {
        let b = verts.len() as u32;
        for &i in &q {
            verts.push(c[i]);
        }
        tris.push([b, b + 1, b + 2]);
        tris.push([b, b + 2, b + 3]);
    }

    append_endcap(&mut verts, &mut tris, s.x, s.y, hw, z_lo, z_hi);
    append_endcap(&mut verts, &mut tris, e.x, e.y, hw, z_lo, z_hi);

    (verts, tris)
}

/// Generate a mesh for an arc trace: a continuous swept ribbon (shared
/// vertex rings between sectors — no gaps or overlapping tiles) with round
/// end caps, matching `trace_to_mesh`'s copper slab Z span.
pub(crate) fn trace_arc_to_mesh(arc: &TraceArc, pcb: &Pcb) -> RawMesh {
    if arc.radius <= 1e-9 || arc.width <= 0.0 {
        return (vec![], vec![]);
    }
    let (z_lo, z_hi) = trace_z_span(pcb, arc.layer);
    let hw = (arc.width / 2.0).min(arc.radius - 1e-9);
    let r_in = arc.radius - hw;
    let r_out = arc.radius + hw;

    // Sample with increasing angle (an arc trace has no direction), so the
    // winding below is uniform.
    let (a0, a1) = if arc.end_angle >= arc.start_angle {
        (arc.start_angle, arc.end_angle)
    } else {
        (arc.end_angle, arc.start_angle)
    };
    let sweep = a1 - a0;
    if sweep < 1e-9 {
        return (vec![], vec![]);
    }
    let segs = ((sweep / 6.0).ceil() as usize).clamp(4, 256);

    let mut verts: Vec<[f64; 3]> = Vec::with_capacity((segs + 1) * 8 + 2 * (CAP_SEGS * 4 + 2));
    let mut tris: Vec<[u32; 3]> = Vec::with_capacity(segs * 8);

    // Ring layout per sample i (8 verts): the top face, bottom face, and the
    // two walls each get their own copies of the shared ring positions, so
    // smooth-normal recomputation keeps the top flat instead of blending it
    // with the walls into a rounded tube.
    //   0 outer-top / 1 inner-top      (top face)
    //   2 outer-bot / 3 inner-bot      (bottom face)
    //   4 outer-top / 5 outer-bot      (outer wall)
    //   6 inner-top / 7 inner-bot      (inner wall)
    for i in 0..=segs {
        let a = (a0 + sweep * i as f64 / segs as f64).to_radians();
        let (c, s) = (a.cos(), a.sin());
        let ox = arc.center.x + r_out * c;
        let oy = arc.center.y + r_out * s;
        let ix = arc.center.x + r_in * c;
        let iy = arc.center.y + r_in * s;
        verts.push([ox, oy, z_hi]);
        verts.push([ix, iy, z_hi]);
        verts.push([ox, oy, z_lo]);
        verts.push([ix, iy, z_lo]);
        verts.push([ox, oy, z_hi]);
        verts.push([ox, oy, z_lo]);
        verts.push([ix, iy, z_hi]);
        verts.push([ix, iy, z_lo]);
    }
    for i in 0..segs as u32 {
        let b = i * 8;
        let j = b + 8;
        // Top (+z), bottom (-z).
        tris.push([b, j, j + 1]);
        tris.push([b, j + 1, b + 1]);
        tris.push([b + 2, j + 3, j + 2]);
        tris.push([b + 2, b + 3, j + 3]);
        // Outer wall (radially out), inner wall (radially in).
        tris.push([b + 4, j + 5, j + 4]);
        tris.push([b + 4, b + 5, j + 5]);
        tris.push([b + 6, j + 6, j + 7]);
        tris.push([b + 6, j + 7, b + 7]);
    }

    // Flat end faces close the ribbon (watertight even without the caps).
    // Start face outward = -tangent, end face outward = +tangent. They reuse
    // wall verts — the faces sit under the round caps, so their shading is
    // never visible.
    let last = segs as u32 * 8;
    tris.push([4, 6, 7]);
    tris.push([4, 7, 5]);
    tris.push([last + 4, last + 7, last + 6]);
    tris.push([last + 4, last + 5, last + 7]);

    // Round caps at both arc endpoints (centerline).
    for a in [a0.to_radians(), a1.to_radians()] {
        let cx = arc.center.x + arc.radius * a.cos();
        let cy = arc.center.y + arc.radius * a.sin();
        append_endcap(&mut verts, &mut tris, cx, cy, hw, z_lo, z_hi);
    }

    (verts, tris)
}

/// Generate a cylindrical via mesh (outer cylinder + top/bottom annular
/// rings). `z_over` extends the barrel past both board faces — the layered
/// preview uses it to punch the annular rings through the soldermask shells.
pub(crate) fn via_to_mesh(via: &Via, pcb: &Pcb, n_seg: usize, z_over: f64) -> RawMesh {
    let z_top = pcb.outline.thickness + z_over;
    let z_bot = -z_over;
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
pub(crate) fn pad_to_mesh(pad: &Pad, fp: &Footprint, pcb: &Pcb) -> RawMesh {
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
pub(crate) fn zone_to_mesh(zone: &Zone, pcb: &Pcb) -> RawMesh {
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

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_ir::ecad::PadType;
    use vcad_ir::Vec2;

    fn pad(number: &str, x: f64, y: f64, w: f64, h: f64) -> Pad {
        Pad {
            number: number.to_string(),
            pad_type: PadType::SMD,
            shape: PadShape::Rect {
                width: w,
                height: h,
            },
            position: Vec2 { x, y },
            rotation: 0.0,
            drill: None,
            net: None,
            layers: vec![PcbLayer::FCu],
        }
    }

    fn two_pad_footprint(rotation: f64) -> Footprint {
        Footprint {
            reference: "R1".to_string(),
            value: "10k".to_string(),
            footprint_name: "R_0805".to_string(),
            position: Vec2 { x: 10.0, y: 5.0 },
            rotation,
            front: true,
            pads: vec![pad("1", -1.0, 0.0, 1.0, 1.4), pad("2", 1.0, 0.0, 1.0, 1.4)],
            graphics: Vec::new(),
            model_3d: None,
            properties: Default::default(),
        }
    }

    /// End-to-end: an EdgeBlend node evaluates through the IR pipeline
    /// and actually removes material — keyed chamfer→fillet lands between
    /// the pure-chamfer and pure-fillet volumes for the same size.
    #[test]
    fn edge_blend_evaluates_through_ir() {
        let mut nodes: HashMap<NodeId, vcad_ir::Node> = HashMap::new();
        nodes.insert(
            0,
            vcad_ir::Node {
                id: 0,
                name: None,
                op: CsgOp::Cube {
                    size: vcad_ir::Vec3::new(10.0, 10.0, 10.0),
                },
            },
        );
        nodes.insert(
            1,
            vcad_ir::Node {
                id: 1,
                name: None,
                op: CsgOp::EdgeBlend {
                    child: 0,
                    edges: vcad_ir::EdgeQuery::Near {
                        point: vcad_ir::Vec3::new(0.0, 0.0, 0.0),
                    },
                    profile: vcad_ir::BlendProfile::Keyed {
                        keys: vec![
                            vcad_ir::BlendKey {
                                t: 0.0,
                                size: 2.0,
                                shape: 0.0,
                            },
                            vcad_ir::BlendKey {
                                t: 1.0,
                                size: 2.0,
                                shape: 1.0,
                            },
                        ],
                    },
                },
            },
        );
        let mut cache = HashMap::new();
        let solid = evaluate_node(1, &nodes, &mut cache)
            .expect("eval ok")
            .expect("solid produced");
        let vol = solid.volume();
        let pi = std::f64::consts::PI;
        let (s, l) = (2.0, 10.0);
        let v_chamfer = l * l * l - s * s / 2.0 * l;
        let v_fillet = l * l * l - (1.0 - pi / 4.0) * s * s * l;
        assert!(
            vol > v_chamfer && vol < v_fillet,
            "loft volume {vol} not in ({v_chamfer}, {v_fillet})"
        );
    }

    /// M2 topological-naming regression: a fillet referencing the edge by
    /// persistent face names (`n0:top` / `n0:right`) stays on the intended
    /// edge when the parent box's dimension changes — the classic
    /// FreeCAD-style breakage this scheme exists to prevent.
    #[test]
    fn named_edge_blend_survives_box_resize() {
        let build = |sx: f64| {
            let mut nodes: HashMap<NodeId, vcad_ir::Node> = HashMap::new();
            nodes.insert(
                0,
                vcad_ir::Node {
                    id: 0,
                    name: None,
                    op: CsgOp::Cube {
                        size: vcad_ir::Vec3::new(sx, 10.0, 10.0),
                    },
                },
            );
            nodes.insert(
                1,
                vcad_ir::Node {
                    id: 1,
                    name: None,
                    op: CsgOp::EdgeBlend {
                        child: 0,
                        edges: vcad_ir::EdgeQuery::Named {
                            face_a: "n0:top".to_string(),
                            face_b: "n0:right".to_string(),
                        },
                        profile: vcad_ir::BlendProfile::Constant {
                            size: 2.0,
                            shape: 1.0,
                        },
                    },
                },
            );
            let mut cache = HashMap::new();
            evaluate_node(1, &nodes, &mut cache)
                .expect("eval ok")
                .expect("solid produced")
        };

        for sx in [10.0, 14.0] {
            let solid = build(sx);
            // Filleting one edge of an sx×10×10 box with r=2 removes
            // (4 − π)·r²/4 · 10 of material — from the intended edge.
            let expected = sx * 10.0 * 10.0 - (1.0 - std::f64::consts::PI / 4.0) * 4.0 * 10.0;
            let vol = solid.volume();
            assert!(
                (vol - expected).abs() < 0.5,
                "volume {vol} != expected {expected} at sx={sx}"
            );
            // The blend landed on the x = sx / z = 10 edge: no vertex
            // remains on that corner line, the opposite edge stays sharp.
            let brep = solid.as_brep().expect("brep result");
            let on_target = brep
                .topology
                .vertices
                .iter()
                .filter(|(_, v)| (v.point.x - sx).abs() < 1e-9 && (v.point.z - 10.0).abs() < 1e-9)
                .count();
            assert_eq!(on_target, 0, "blend must remove the named edge at sx={sx}");
            let on_opposite = brep
                .topology
                .vertices
                .iter()
                .filter(|(_, v)| v.point.x.abs() < 1e-9 && (v.point.z - 10.0).abs() < 1e-9)
                .count();
            assert!(on_opposite >= 2, "opposite edge must stay sharp at sx={sx}");
        }
    }

    /// Fail-closed at the IR level: a Named reference that cannot resolve
    /// is an EvalError, never a silently rebound blend.
    #[test]
    fn named_edge_blend_fails_closed_on_lost_reference() {
        let mut nodes: HashMap<NodeId, vcad_ir::Node> = HashMap::new();
        nodes.insert(
            0,
            vcad_ir::Node {
                id: 0,
                name: None,
                op: CsgOp::Cube {
                    size: vcad_ir::Vec3::new(10.0, 10.0, 10.0),
                },
            },
        );
        nodes.insert(
            1,
            vcad_ir::Node {
                id: 1,
                name: None,
                op: CsgOp::EdgeBlend {
                    child: 0,
                    edges: vcad_ir::EdgeQuery::Named {
                        face_a: "n0:top".to_string(),
                        face_b: "n0:no_such_face".to_string(),
                    },
                    profile: vcad_ir::BlendProfile::Constant {
                        size: 2.0,
                        shape: 1.0,
                    },
                },
            },
        );
        let mut cache = HashMap::new();
        let err = evaluate_node(1, &nodes, &mut cache).expect_err("must fail closed");
        assert!(matches!(err, EvalError::NamedEdge { .. }), "got {err:?}");
    }

    #[test]
    fn component_bbox_unrotated() {
        let (min_x, min_y, max_x, max_y) =
            footprint_component_world_bbox(&two_pad_footprint(0.0)).unwrap();
        assert!((max_x - min_x - 3.0).abs() < 1e-9);
        assert!((max_y - min_y - 1.4).abs() < 1e-9);
        assert!(((min_x + max_x) / 2.0 - 10.0).abs() < 1e-9);
        assert!(((min_y + max_y) / 2.0 - 5.0).abs() < 1e-9);
    }

    #[test]
    fn component_bbox_rotated_90_swaps_extents() {
        let (min_x, min_y, max_x, max_y) =
            footprint_component_world_bbox(&two_pad_footprint(90.0)).unwrap();
        // 90° rotation swaps width/height of the local pad-extent AABB.
        assert!((max_x - min_x - 1.4).abs() < 1e-9);
        assert!((max_y - min_y - 3.0).abs() < 1e-9);
        assert!(((min_x + max_x) / 2.0 - 10.0).abs() < 1e-9);
        assert!(((min_y + max_y) / 2.0 - 5.0).abs() < 1e-9);
    }

    #[test]
    fn component_bbox_no_pads_is_none() {
        let mut fp = two_pad_footprint(0.0);
        fp.pads.clear();
        assert!(footprint_component_world_bbox(&fp).is_none());
    }

    // ── Copper mesh winding (divergence-theorem signed volume) ──
    // A closed mesh wound outward has positive signed volume. The old
    // trace_to_mesh emitted inside-out boxes on FCu, which backface-culled
    // to a broken "dashed" look in the GLB preview.

    fn signed_volume(mesh: &RawMesh) -> f64 {
        let (verts, tris) = mesh;
        tris.iter()
            .map(|t| {
                let a = verts[t[0] as usize];
                let b = verts[t[1] as usize];
                let c = verts[t[2] as usize];
                (a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
                    + a[2] * (b[0] * c[1] - b[1] * c[0]))
                    / 6.0
            })
            .sum()
    }

    fn bare_pcb() -> Pcb {
        use vcad_ir::ecad::*;
        Pcb {
            outline: BoardOutline {
                vertices: vec![
                    Vec2 { x: 0.0, y: 0.0 },
                    Vec2 { x: 50.0, y: 0.0 },
                    Vec2 { x: 50.0, y: 50.0 },
                    Vec2 { x: 0.0, y: 50.0 },
                ],
                cutouts: vec![],
                thickness: 1.6,
            },
            stackup: LayerStackup { layers: vec![] },
            nets: vec![],
            rules: DesignRules {
                default_rules: NetClassRules {
                    name: "Default".into(),
                    trace_width: 0.25,
                    clearance: 0.2,
                    via_diameter: 0.8,
                    via_drill: 0.4,
                    diff_pair_gap: None,
                    diff_pair_width: None,
                },
                class_rules: vec![],
                net_class_assignments: Default::default(),
                edge_clearance: 0.5,
                hole_to_hole: 0.5,
                min_annular_ring: 0.15,
                min_drill: 0.2,
            },
            footprints: vec![],
            traces: vec![],
            trace_arcs: vec![],
            vias: vec![],
            zones: vec![],
            keepouts: vec![],
            net_ties: vec![],
        }
    }

    #[test]
    fn trace_mesh_winds_outward_on_both_sides() {
        let pcb = bare_pcb();
        for layer in [PcbLayer::FCu, PcbLayer::BCu] {
            let trace = Trace {
                net: "N1".into(),
                start: Vec2 { x: 5.0, y: 5.0 },
                end: Vec2 { x: 25.0, y: 5.0 },
                width: 0.5,
                layer,
                source: None,
            };
            let mesh = trace_to_mesh(&trace, &pcb);
            let vol = signed_volume(&mesh);
            // Box: 20 × 0.5 × 0.035 = 0.35 mm³ plus two round caps.
            assert!(
                vol > 0.3,
                "{layer:?} trace mesh should wind outward, signed volume {vol}"
            );
        }
    }

    #[test]
    fn trace_arc_mesh_is_continuous_and_outward() {
        let pcb = bare_pcb();
        let arc = TraceArc {
            center: Vec2 { x: 25.0, y: 25.0 },
            radius: 10.0,
            start_angle: 0.0,
            end_angle: 180.0,
            width: 0.5,
            layer: PcbLayer::FCu,
            net: "N1".into(),
        };
        let mesh = trace_arc_to_mesh(&arc, &pcb);
        assert!(!mesh.0.is_empty());
        // Half annulus: π·((10.25)² − (9.75)²)·0.035 ≈ 0.55 mm³ + caps.
        let vol = signed_volume(&mesh);
        assert!(
            (vol - 0.55).abs() < 0.1,
            "arc trace signed volume {vol} should be ≈ swept copper volume"
        );
        // Reversed angle order must yield the same copper, not an
        // inside-out ribbon.
        let rev = TraceArc {
            start_angle: 180.0,
            end_angle: 0.0,
            ..arc
        };
        let vol_rev = signed_volume(&trace_arc_to_mesh(&rev, &pcb));
        assert!((vol - vol_rev).abs() < 1e-9);
    }
}
