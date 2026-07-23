//! Lowering of document constraints into per-plane `Sketch2D` solves.

use std::collections::HashMap;

use vcad_ir::constraints::{Anchor, ConstraintKind, DesignConstraint, SketchPointRef};
use vcad_ir::ecad::Pcb;
use vcad_ir::expr_parser::parse_and_eval;
use vcad_ir::parameters::{resolve_parameters, Expr};
use vcad_ir::{CsgOp, Document, NodeId, SketchSegment2D};

use vcad_kernel_constraints::{Constraint, EntityId, EntityRef, Sketch2D, SolveStatus};

use crate::measure::measure_constraint;
use crate::report::{ConstraintResidual, DesignSolveReport, DrivenValue, GroupReport};
use crate::{AnchorResolver, SolveOptions};

/// Tolerance for "did this coordinate move" reporting.
const MOVE_EPS: f64 = 1e-9;
/// Tolerance for detecting shared sketch endpoints.
const WELD_EPS: f64 = 1e-6;

/// What kind of plane a solve group lives on.
enum GroupDomain {
    Pcb,
    Sketch {
        origin: [f64; 3],
        x_dir: [f64; 3],
        y_dir: [f64; 3],
    },
}

/// One planar solve group under construction.
struct Group {
    node: NodeId,
    domain: GroupDomain,
    sketch: Sketch2D,
    /// footprint ref → origin point entity.
    fp_points: HashMap<String, EntityId>,
    /// footprint ref → rotation pseudo-circle entity.
    fp_rotations: HashMap<String, EntityId>,
    /// footprint ref → pad name → pad point entity.
    pad_points: HashMap<(String, String), EntityId>,
    /// outline vertex index → point entity.
    outline_points: HashMap<u32, EntityId>,
    /// outline edge index → line entity.
    outline_edges: HashMap<u32, EntityId>,
    /// (segment, point) → point entity, for sketch groups.
    sketch_points: HashMap<(u32, SketchPointRef), EntityId>,
    /// segment index → line entity, for sketch groups (line segments only).
    sketch_lines: HashMap<u32, EntityId>,
    /// Lowered driving-constraint count (for the report).
    constraint_count: usize,
}

impl Group {
    fn new(node: NodeId, domain: GroupDomain) -> Self {
        Group {
            node,
            domain,
            sketch: Sketch2D::new(),
            fp_points: HashMap::new(),
            fp_rotations: HashMap::new(),
            pad_points: HashMap::new(),
            outline_points: HashMap::new(),
            outline_edges: HashMap::new(),
            sketch_points: HashMap::new(),
            sketch_lines: HashMap::new(),
            constraint_count: 0,
        }
    }

    /// Project a world point into this group's plane coordinates.
    fn project(&self, p: [f64; 3]) -> (f64, f64) {
        match &self.domain {
            GroupDomain::Pcb => (p[0], p[1]),
            GroupDomain::Sketch {
                origin,
                x_dir,
                y_dir,
            } => {
                let d = [p[0] - origin[0], p[1] - origin[1], p[2] - origin[2]];
                (
                    d[0] * x_dir[0] + d[1] * x_dir[1] + d[2] * x_dir[2],
                    d[0] * y_dir[0] + d[1] * y_dir[1] + d[2] * y_dir[2],
                )
            }
        }
    }
}

fn pcb_of(doc: &Document, node: NodeId) -> Option<&Pcb> {
    match &doc.nodes.get(&node)?.op {
        CsgOp::PcbBoard { board } => Some(board),
        _ => None,
    }
}

fn pcb_of_mut(doc: &mut Document, node: NodeId) -> Option<&mut Pcb> {
    match &mut doc.nodes.get_mut(&node)?.op {
        CsgOp::PcbBoard { board } => Some(board),
        _ => None,
    }
}

/// Evaluate a dimensional value expression against the parameter env.
fn eval_expr(expr: &Expr, env: &HashMap<String, f64>) -> Result<f64, String> {
    match expr {
        Expr::Number(v) => Ok(*v),
        Expr::Formula(f) => parse_and_eval(f, env).map_err(|e| format!("formula \"{f}\": {e}")),
    }
}

/// The node a constraint's free (non-part) anchors belong to, validated to
/// be unique. Part-edge anchors are fixed references and may live anywhere.
fn group_node(kind: &ConstraintKind) -> Result<NodeId, String> {
    if let ConstraintKind::Rotation { node, .. } = kind {
        return Ok(*node);
    }
    let free: Vec<NodeId> = kind
        .anchors()
        .iter()
        .filter(|a| !matches!(a, Anchor::PartEdge { .. }))
        .map(|a| a.node())
        .collect();
    let Some(first) = free.first().copied() else {
        return Err("constraint references only fixed part geometry; nothing to solve".into());
    };
    if free.iter().any(|n| *n != first) {
        return Err(format!(
            "free anchors span multiple nodes ({free:?}); one constraint may only drive one board or sketch"
        ));
    }
    Ok(first)
}

/// Entity handle for a lowered anchor: a point ref, or a line entity.
enum Lowered {
    Point(EntityRef),
    Line(EntityId),
}

impl Lowered {
    fn point(&self) -> Result<EntityRef, String> {
        match self {
            Lowered::Point(p) => Ok(*p),
            Lowered::Line(_) => Err("expected a point anchor, got an edge".into()),
        }
    }
    fn line(&self) -> Result<EntityId, String> {
        match self {
            Lowered::Line(l) => Ok(*l),
            Lowered::Point(_) => Err("expected an edge anchor, got a point".into()),
        }
    }
}

/// Lower one anchor into the group, creating entities on demand.
#[allow(clippy::too_many_lines)]
fn lower_anchor(
    doc: &Document,
    group: &mut Group,
    resolver: &dyn AnchorResolver,
    anchor: &Anchor,
    free_rotation_refs: &[String],
    warnings: &mut Vec<String>,
) -> Result<Lowered, String> {
    match anchor {
        Anchor::PcbFootprint { node, r#ref, pad } => {
            let pcb =
                pcb_of(doc, *node).ok_or_else(|| format!("node {node} is not a PCB board"))?;
            let fp = pcb
                .footprints
                .iter()
                .find(|f| f.reference == *r#ref)
                .ok_or_else(
                    || format!("footprint \"{ref}\" not found on board {node}", ref = r#ref),
                )?;
            let origin = *group
                .fp_points
                .entry(r#ref.clone())
                .or_insert_with(|| group.sketch.add_point(fp.position.x, fp.position.y));
            let Some(pad_name) = pad else {
                return Ok(Lowered::Point(EntityRef::Point(origin)));
            };
            let pad_obj = fp.pads.iter().find(|p| p.number == *pad_name).ok_or_else(
                || format!("pad \"{pad_name}\" not found on footprint \"{ref}\"", ref = r#ref),
            )?;
            let key = (r#ref.clone(), pad_name.clone());
            if let Some(id) = group.pad_points.get(&key) {
                return Ok(Lowered::Point(EntityRef::Point(*id)));
            }
            // Pad rides the origin through a rigid offset evaluated at the
            // footprint's current rotation. If that rotation is itself free
            // in this solve, the offset is an approximation — warn.
            if free_rotation_refs.contains(r#ref) {
                warnings.push(format!(
                    "pad anchor {ref}.{pad_name} uses the footprint's current rotation; combined with a free rotation constraint the offset is approximate",
                    ref = r#ref
                ));
            }
            let world = vcad_ecad_pcb::geometry::pad_world_position(fp, pad_obj);
            let (dx, dy) = (world.x - fp.position.x, world.y - fp.position.y);
            let pad_pt = group.sketch.add_point(world.x, world.y);
            group.sketch.add_constraint(Constraint::OffsetCoincident {
                point_a: EntityRef::Point(origin),
                point_b: EntityRef::Point(pad_pt),
                dx,
                dy,
            });
            group.pad_points.insert(key, pad_pt);
            Ok(Lowered::Point(EntityRef::Point(pad_pt)))
        }
        Anchor::PcbOutlineVertex { node, index } => {
            let pcb =
                pcb_of(doc, *node).ok_or_else(|| format!("node {node} is not a PCB board"))?;
            let n = pcb.outline.vertices.len() as u32;
            if *index >= n {
                return Err(format!(
                    "outline vertex {index} out of range (outline has {n} vertices)"
                ));
            }
            let v = pcb.outline.vertices[*index as usize];
            let id = *group
                .outline_points
                .entry(*index)
                .or_insert_with(|| group.sketch.add_point(v.x, v.y));
            Ok(Lowered::Point(EntityRef::Point(id)))
        }
        Anchor::PcbOutlineEdge { node, index } => {
            let pcb =
                pcb_of(doc, *node).ok_or_else(|| format!("node {node} is not a PCB board"))?;
            let n = pcb.outline.vertices.len() as u32;
            if *index >= n {
                return Err(format!(
                    "outline edge {index} out of range (outline has {n} vertices)"
                ));
            }
            if let Some(id) = group.outline_edges.get(index) {
                return Ok(Lowered::Line(*id));
            }
            let j = (*index + 1) % n;
            let a = pcb.outline.vertices[*index as usize];
            let b = pcb.outline.vertices[j as usize];
            let pa = *group
                .outline_points
                .entry(*index)
                .or_insert_with(|| group.sketch.add_point(a.x, a.y));
            let pb = *group
                .outline_points
                .entry(j)
                .or_insert_with(|| group.sketch.add_point(b.x, b.y));
            let line = group.sketch.add_line(pa, pb);
            group.outline_edges.insert(*index, line);
            Ok(Lowered::Line(line))
        }
        Anchor::SketchPoint {
            node,
            segment,
            point,
        } => {
            // Sketch groups lower every segment up front (see
            // `ensure_sketch_group`), so the entity must already exist.
            let _ = node;
            group
                .sketch_points
                .get(&(*segment, *point))
                .map(|id| Lowered::Point(EntityRef::Point(*id)))
                .ok_or_else(|| format!("sketch segment {segment} has no {point:?} point"))
        }
        Anchor::SketchSegment { node, segment } => {
            let _ = node;
            group
                .sketch_lines
                .get(segment)
                .map(|id| Lowered::Line(*id))
                .ok_or_else(|| {
                    format!(
                        "sketch segment {segment} is not a line segment (arcs can't be edge anchors)"
                    )
                })
        }
        Anchor::PartEdge {
            node,
            face_a,
            face_b,
            ..
        } => {
            let (a, b) = resolver.resolve_part_edge(*node, face_a, face_b)?;
            let (ax, ay) = group.project(a);
            let (bx, by) = group.project(b);
            let pa = group.sketch.add_point(ax, ay);
            let pb = group.sketch.add_point(bx, by);
            group.sketch.add_constraint(Constraint::Fixed {
                point: EntityRef::Point(pa),
                x: ax,
                y: ay,
            });
            group.sketch.add_constraint(Constraint::Fixed {
                point: EntityRef::Point(pb),
                x: bx,
                y: by,
            });
            let line = group.sketch.add_line(pa, pb);
            Ok(Lowered::Line(line))
        }
    }
}

/// Midpoint of a part edge, as a point entity (for point-context part
/// anchors like Coincident-with-part-edge).
fn lower_anchor_as_point(
    doc: &Document,
    group: &mut Group,
    resolver: &dyn AnchorResolver,
    anchor: &Anchor,
    free_rotation_refs: &[String],
    warnings: &mut Vec<String>,
) -> Result<EntityRef, String> {
    if let Anchor::PartEdge {
        node,
        face_a,
        face_b,
        ..
    } = anchor
    {
        let (a, b) = resolver.resolve_part_edge(*node, face_a, face_b)?;
        let mid = [
            (a[0] + b[0]) / 2.0,
            (a[1] + b[1]) / 2.0,
            (a[2] + b[2]) / 2.0,
        ];
        let (mx, my) = group.project(mid);
        let p = group.sketch.add_point(mx, my);
        group.sketch.add_constraint(Constraint::Fixed {
            point: EntityRef::Point(p),
            x: mx,
            y: my,
        });
        return Ok(EntityRef::Point(p));
    }
    lower_anchor(doc, group, resolver, anchor, free_rotation_refs, warnings)?.point()
}

/// A point-or-edge anchor lowered for line contexts: part edges and outline
/// edges give their line; anything else is an error.
fn lower_anchor_as_line(
    doc: &Document,
    group: &mut Group,
    resolver: &dyn AnchorResolver,
    anchor: &Anchor,
    warnings: &mut Vec<String>,
) -> Result<EntityId, String> {
    lower_anchor(doc, group, resolver, anchor, &[], warnings)?.line()
}

/// Pre-populate a sketch group with every segment of the sketch, welding
/// endpoints that coincide by value so the profile stays connected.
fn ensure_sketch_group(group: &mut Group, segments: &[SketchSegment2D]) {
    let mut welded: Vec<(f64, f64, EntityId)> = Vec::new();
    let mut get_point = |group: &mut Group, x: f64, y: f64| -> EntityId {
        for (wx, wy, id) in &welded {
            if (wx - x).abs() < WELD_EPS && (wy - y).abs() < WELD_EPS {
                return *id;
            }
        }
        let id = group.sketch.add_point(x, y);
        welded.push((x, y, id));
        id
    };
    for (i, seg) in segments.iter().enumerate() {
        let i = i as u32;
        match seg {
            SketchSegment2D::Line { start, end } => {
                let s = get_point(group, start.x, start.y);
                let e = get_point(group, end.x, end.y);
                group.sketch_points.insert((i, SketchPointRef::Start), s);
                group.sketch_points.insert((i, SketchPointRef::End), e);
                let line = group.sketch.add_line(s, e);
                group.sketch_lines.insert(i, line);
            }
            SketchSegment2D::Arc {
                start, end, center, ..
            } => {
                let s = get_point(group, start.x, start.y);
                let e = get_point(group, end.x, end.y);
                let c = get_point(group, center.x, center.y);
                group.sketch_points.insert((i, SketchPointRef::Start), s);
                group.sketch_points.insert((i, SketchPointRef::End), e);
                group.sketch_points.insert((i, SketchPointRef::Center), c);
                group.sketch.add_arc(s, e, c, true);
            }
        }
    }
}

/// Lower one driving constraint into its group. Returns Err to skip it.
fn lower_constraint(
    doc: &Document,
    group: &mut Group,
    resolver: &dyn AnchorResolver,
    kind: &ConstraintKind,
    env: &HashMap<String, f64>,
    free_rotation_refs: &[String],
    warnings: &mut Vec<String>,
) -> Result<(), String> {
    let mut point = |group: &mut Group, a: &Anchor| {
        lower_anchor_as_point(doc, group, resolver, a, free_rotation_refs, warnings)
    };
    match kind {
        ConstraintKind::Coincident { a, b } | ConstraintKind::Concentric { a, b } => {
            let (pa, pb) = (point(group, a)?, point(group, b)?);
            group.sketch.add_constraint(Constraint::Coincident {
                point_a: pa,
                point_b: pb,
            });
        }
        ConstraintKind::Distance { a, b, value } => {
            let d = eval_expr(value, env)?;
            let (pa, pb) = (point(group, a)?, point(group, b)?);
            group.sketch.add_constraint(Constraint::Distance {
                point_a: pa,
                point_b: pb,
                distance: d,
            });
        }
        ConstraintKind::HorizontalDistance { a, value } => {
            let x = eval_expr(value, env)?;
            let p = point(group, a)?;
            group
                .sketch
                .add_constraint(Constraint::HorizontalDistance { point: p, x });
        }
        ConstraintKind::VerticalDistance { a, value } => {
            let y = eval_expr(value, env)?;
            let p = point(group, a)?;
            group
                .sketch
                .add_constraint(Constraint::VerticalDistance { point: p, y });
        }
        ConstraintKind::Horizontal { a, b } => {
            let line = pair_line(doc, group, resolver, a, b, free_rotation_refs, warnings)?;
            group.sketch.add_constraint(Constraint::Horizontal { line });
        }
        ConstraintKind::Vertical { a, b } => {
            let line = pair_line(doc, group, resolver, a, b, free_rotation_refs, warnings)?;
            group.sketch.add_constraint(Constraint::Vertical { line });
        }
        ConstraintKind::Parallel { a, b } => {
            let la = lower_anchor_as_line(doc, group, resolver, a, warnings)?;
            let lb = lower_anchor_as_line(doc, group, resolver, b, warnings)?;
            group.sketch.add_constraint(Constraint::Parallel {
                line_a: la,
                line_b: lb,
            });
        }
        ConstraintKind::Perpendicular { a, b } => {
            let la = lower_anchor_as_line(doc, group, resolver, a, warnings)?;
            let lb = lower_anchor_as_line(doc, group, resolver, b, warnings)?;
            group.sketch.add_constraint(Constraint::Perpendicular {
                line_a: la,
                line_b: lb,
            });
        }
        ConstraintKind::EqualLength { a, b } => {
            let la = lower_anchor_as_line(doc, group, resolver, a, warnings)?;
            let lb = lower_anchor_as_line(doc, group, resolver, b, warnings)?;
            group.sketch.add_constraint(Constraint::EqualLength {
                line_a: la,
                line_b: lb,
            });
        }
        ConstraintKind::Length { a, value } => {
            let len = eval_expr(value, env)?;
            let la = lower_anchor_as_line(doc, group, resolver, a, warnings)?;
            group.sketch.add_constraint(Constraint::Length {
                line: la,
                length: len,
            });
        }
        ConstraintKind::PointOnEdge { point: p, edge } => {
            let pt = point(group, p)?;
            let line = lower_anchor_as_line(doc, group, resolver, edge, warnings)?;
            group
                .sketch
                .add_constraint(Constraint::PointOnLine { point: pt, line });
        }
        ConstraintKind::Fixed { a } => {
            let p = point(group, a)?;
            let (x, y) = current_point(group, p)?;
            group
                .sketch
                .add_constraint(Constraint::Fixed { point: p, x, y });
        }
        ConstraintKind::Rotation { node, r#ref, value } => {
            let deg = eval_expr(value, env)?;
            let circle = ensure_rotation_circle(doc, group, *node, r#ref)?;
            group.sketch.add_constraint(Constraint::Radius {
                circle,
                radius: deg,
            });
        }
        ConstraintKind::Symmetric { a, b, axis } => {
            let (pa, pb) = (point(group, a)?, point(group, b)?);
            let ax = lower_anchor_as_line(doc, group, resolver, axis, warnings)?;
            group.sketch.add_constraint(Constraint::Symmetric {
                point_a: pa,
                point_b: pb,
                axis: ax,
            });
        }
        ConstraintKind::Angle { a, b, value } => {
            let deg = eval_expr(value, env)?;
            let la = lower_anchor_as_line(doc, group, resolver, a, warnings)?;
            let lb = lower_anchor_as_line(doc, group, resolver, b, warnings)?;
            group.sketch.add_constraint(Constraint::Angle {
                line_a: la,
                line_b: lb,
                angle_rad: deg.to_radians(),
            });
        }
    }
    group.constraint_count += 1;
    Ok(())
}

/// For Horizontal/Vertical: a single edge-like anchor passed twice means
/// "this edge"; otherwise the two point anchors get a connecting line.
fn pair_line(
    doc: &Document,
    group: &mut Group,
    resolver: &dyn AnchorResolver,
    a: &Anchor,
    b: &Anchor,
    free_rotation_refs: &[String],
    warnings: &mut Vec<String>,
) -> Result<EntityId, String> {
    if a.is_edge() && a == b {
        return lower_anchor_as_line(doc, group, resolver, a, warnings);
    }
    let pa = lower_anchor_as_point(doc, group, resolver, a, free_rotation_refs, warnings)?;
    let pb = lower_anchor_as_point(doc, group, resolver, b, free_rotation_refs, warnings)?;
    let (ia, ib) = (deref_point(pa)?, deref_point(pb)?);
    Ok(group.sketch.add_line(ia, ib))
}

fn deref_point(r: EntityRef) -> Result<EntityId, String> {
    match r {
        EntityRef::Point(id) => Ok(id),
        _ => Err("expected a plain point entity".into()),
    }
}

fn current_point(group: &Group, r: EntityRef) -> Result<(f64, f64), String> {
    let id = match r {
        EntityRef::Point(id) => id,
        _ => return Err("expected a plain point entity".into()),
    };
    group
        .sketch
        .get_point(id)
        .ok_or_else(|| "entity is not a point".into())
}

/// The pseudo-circle whose radius parameter carries a footprint's rotation
/// in degrees.
fn ensure_rotation_circle(
    doc: &Document,
    group: &mut Group,
    node: NodeId,
    r#ref: &str,
) -> Result<EntityId, String> {
    if let Some(id) = group.fp_rotations.get(r#ref) {
        return Ok(*id);
    }
    let pcb = pcb_of(doc, node).ok_or_else(|| format!("node {node} is not a PCB board"))?;
    let fp = pcb
        .footprints
        .iter()
        .find(|f| f.reference == r#ref)
        .ok_or_else(|| format!("footprint \"{ref}\" not found on board {node}", ref = r#ref))?;
    let origin = *group
        .fp_points
        .entry(r#ref.to_string())
        .or_insert_with(|| group.sketch.add_point(fp.position.x, fp.position.y));
    let circle = group.sketch.add_circle(origin, fp.rotation);
    group.fp_rotations.insert(r#ref.to_string(), circle);
    Ok(circle)
}

/// Refs that have a driving Rotation constraint (rotation is a free param).
fn free_rotation_refs(constraints: &[DesignConstraint]) -> Vec<String> {
    constraints
        .iter()
        .filter(|c| !c.driven)
        .filter_map(|c| match &c.kind {
            ConstraintKind::Rotation { r#ref, .. } => Some(r#ref.clone()),
            _ => None,
        })
        .collect()
}

fn status_str(s: SolveStatus) -> (&'static str, bool) {
    match s {
        SolveStatus::Converged => ("converged", true),
        SolveStatus::MaxIterations => ("maxIterations", false),
        SolveStatus::LambdaOverflow => ("lambdaOverflow", false),
        SolveStatus::NoConstraints => ("noConstraints", true),
        SolveStatus::NoParameters => ("noParameters", true),
        SolveStatus::SingularMatrix => ("singularMatrix", false),
    }
}

/// The main lowering + solve + write-back pass shared by solve and check.
pub(crate) fn run(
    doc: &mut Document,
    resolver: &dyn AnchorResolver,
    options: &SolveOptions,
    write_back: bool,
) -> DesignSolveReport {
    let mut report = DesignSolveReport {
        converged: true,
        ..Default::default()
    };
    if doc.constraints.is_empty() && options.extra_fixed.is_empty() {
        return report;
    }

    // 1. Parameter environment (fail-closed per formula, not globally).
    let env = match resolve_parameters(&doc.parameters) {
        Ok(env) => env,
        Err(e) => {
            report
                .errors
                .push(format!("parameter resolution failed: {e}"));
            HashMap::new()
        }
    };

    let free_rot = free_rotation_refs(&doc.constraints);

    // 2. Partition into groups.
    let mut groups: Vec<Group> = Vec::new();
    let mut group_index: HashMap<u64, usize> = HashMap::new();
    let constraints = doc.constraints.clone();
    for c in &constraints {
        if c.driven {
            continue; // driven dims contribute nothing to the solve
        }
        let node = match group_node(&c.kind) {
            Ok(n) => n,
            Err(e) => {
                report.errors.push(format!("constraint {}: {e}", c.id));
                continue;
            }
        };
        let idx = match group_index.get(&node) {
            Some(i) => *i,
            None => {
                let domain = match doc.nodes.get(&node).map(|n| &n.op) {
                    Some(CsgOp::PcbBoard { .. }) => GroupDomain::Pcb,
                    Some(CsgOp::Sketch2D {
                        origin,
                        x_dir,
                        y_dir,
                        ..
                    }) => GroupDomain::Sketch {
                        origin: [origin.x, origin.y, origin.z],
                        x_dir: [x_dir.x, x_dir.y, x_dir.z],
                        y_dir: [y_dir.x, y_dir.y, y_dir.z],
                    },
                    _ => {
                        report.errors.push(format!(
                            "constraint {}: node {node} is neither a PCB board nor a sketch",
                            c.id
                        ));
                        continue;
                    }
                };
                let mut group = Group::new(node, domain);
                if let Some(CsgOp::Sketch2D { segments, .. }) = doc.nodes.get(&node).map(|n| &n.op)
                {
                    ensure_sketch_group(&mut group, segments);
                }
                groups.push(group);
                group_index.insert(node, groups.len() - 1);
                groups.len() - 1
            }
        };
        let mut warnings = Vec::new();
        if let Err(e) = lower_constraint(
            doc,
            &mut groups[idx],
            resolver,
            &c.kind,
            &env,
            &free_rot,
            &mut warnings,
        ) {
            report.errors.push(format!("constraint {}: {e}", c.id));
        }
        report.warnings.extend(warnings);
    }

    // 3. Interactive extra-fixed pins.
    for (node, r#ref) in &options.extra_fixed {
        let Some(idx) = group_index.get(node) else {
            continue; // no constraints on that board — nothing to pin against
        };
        let group = &mut groups[*idx];
        let Some(pcb) = pcb_of(doc, *node) else {
            continue;
        };
        let Some(fp) = pcb.footprints.iter().find(|f| f.reference == *r#ref) else {
            continue;
        };
        let origin = *group
            .fp_points
            .entry(r#ref.clone())
            .or_insert_with(|| group.sketch.add_point(fp.position.x, fp.position.y));
        group.sketch.add_constraint(Constraint::Fixed {
            point: EntityRef::Point(origin),
            x: fp.position.x,
            y: fp.position.y,
        });
    }

    // 4. Solve each group and write back.
    for group in &mut groups {
        let result = group.sketch.solve_default();
        let (status, ok) = status_str(result.status);
        report.groups.push(GroupReport {
            node: group.node,
            status: status.to_string(),
            converged: ok,
            iterations: result.iterations,
            residual_norm: result.residual_norm,
            dof: i64::from(group.sketch.degrees_of_freedom()),
            constraint_count: group.constraint_count,
        });
        if !ok {
            report.converged = false;
            continue;
        }
        if !write_back {
            continue;
        }
        write_back_group(doc, group, &mut report);
    }

    // 5. Measure dimensional values (driven always; all dims on check).
    for c in &constraints {
        let measure_this = c.driven || !write_back;
        if !measure_this || !c.kind.is_dimensional() {
            continue;
        }
        match measure_constraint(doc, resolver, &c.kind) {
            Ok(v) => report.driven_values.push(DrivenValue {
                id: c.id.clone(),
                value: v,
            }),
            Err(e) => report
                .errors
                .push(format!("constraint {}: cannot measure: {e}", c.id)),
        }
    }

    // 5b. Per-constraint residuals against the (possibly just-solved)
    // geometry — the receipt layer's Holds/Violated evidence.
    for c in &constraints {
        let target = c.kind.value().and_then(|v| eval_expr(v, &env).ok());
        match crate::measure::constraint_residual(doc, resolver, &c.kind, target) {
            Ok(r) => report.residuals.push(ConstraintResidual {
                id: c.id.clone(),
                residual: r,
                driven: c.driven,
            }),
            Err(e) => report
                .errors
                .push(format!("constraint {}: cannot verify: {e}", c.id)),
        }
    }

    // 6. Back-annotate driven dims into the document.
    if write_back {
        for dv in &report.driven_values {
            if let Some(c) = doc.constraints.iter_mut().find(|c| c.id == dv.id) {
                if c.driven {
                    if let Some(v) = c.kind.value_mut() {
                        *v = Expr::Number(dv.value);
                    }
                }
            }
        }
    }

    report
}

/// Copy solved entity positions back into the document.
fn write_back_group(doc: &mut Document, group: &Group, report: &mut DesignSolveReport) {
    match &group.domain {
        GroupDomain::Pcb => {
            let node = group.node;
            // Collect solved values first (immutable borrow of group).
            let fps: Vec<(String, f64, f64)> = group
                .fp_points
                .iter()
                .filter_map(|(r, id)| group.sketch.get_point(*id).map(|(x, y)| (r.clone(), x, y)))
                .collect();
            let rots: Vec<(String, f64)> = group
                .fp_rotations
                .iter()
                .filter_map(|(r, id)| group.sketch.get_radius(*id).map(|deg| (r.clone(), deg)))
                .collect();
            let verts: Vec<(u32, f64, f64)> = group
                .outline_points
                .iter()
                .filter_map(|(i, id)| group.sketch.get_point(*id).map(|(x, y)| (*i, x, y)))
                .collect();
            let Some(pcb) = pcb_of_mut(doc, node) else {
                return;
            };
            for (r, x, y) in fps {
                if let Some(fp) = pcb.footprints.iter_mut().find(|f| f.reference == r) {
                    if (fp.position.x - x).abs() > MOVE_EPS || (fp.position.y - y).abs() > MOVE_EPS
                    {
                        fp.position.x = x;
                        fp.position.y = y;
                        if !report.moved_footprints.contains(&r) {
                            report.moved_footprints.push(r.clone());
                        }
                    }
                }
            }
            for (r, deg) in rots {
                if let Some(fp) = pcb.footprints.iter_mut().find(|f| f.reference == r) {
                    if (fp.rotation - deg).abs() > MOVE_EPS {
                        fp.rotation = deg;
                        if !report.moved_footprints.contains(&r) {
                            report.moved_footprints.push(r.clone());
                        }
                    }
                }
            }
            for (i, x, y) in verts {
                if let Some(v) = pcb.outline.vertices.get_mut(i as usize) {
                    if (v.x - x).abs() > MOVE_EPS || (v.y - y).abs() > MOVE_EPS {
                        v.x = x;
                        v.y = y;
                        report.moved_vertices.push(format!("{node}:{i}"));
                    }
                }
            }
        }
        GroupDomain::Sketch { .. } => {
            let node = group.node;
            let solved: Vec<((u32, SketchPointRef), (f64, f64))> = group
                .sketch_points
                .iter()
                .filter_map(|(k, id)| group.sketch.get_point(*id).map(|p| (*k, p)))
                .collect();
            let Some(CsgOp::Sketch2D { segments, .. }) =
                doc.nodes.get_mut(&node).map(|n| &mut n.op)
            else {
                return;
            };
            let mut moved = false;
            for ((seg, which), (x, y)) in solved {
                let Some(segment) = segments.get_mut(seg as usize) else {
                    continue;
                };
                let target = match (segment, which) {
                    (SketchSegment2D::Line { start, .. }, SketchPointRef::Start) => Some(start),
                    (SketchSegment2D::Line { end, .. }, SketchPointRef::End) => Some(end),
                    (SketchSegment2D::Arc { start, .. }, SketchPointRef::Start) => Some(start),
                    (SketchSegment2D::Arc { end, .. }, SketchPointRef::End) => Some(end),
                    (SketchSegment2D::Arc { center, .. }, SketchPointRef::Center) => Some(center),
                    _ => None,
                };
                if let Some(v) = target {
                    if (v.x - x).abs() > MOVE_EPS || (v.y - y).abs() > MOVE_EPS {
                        v.x = x;
                        v.y = y;
                        moved = true;
                    }
                }
            }
            if moved {
                report.moved_sketches.push(node);
            }
        }
    }
}
