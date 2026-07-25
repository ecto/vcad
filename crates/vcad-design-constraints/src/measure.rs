//! Direct measurement of dimensional constraints from document geometry —
//! used for driven-dimension back-annotation and receipt verification.

use vcad_ir::constraints::{Anchor, ConstraintKind, SketchPointRef};
use vcad_ir::{CsgOp, Document, NodeId};

use crate::AnchorResolver;

/// Plane frame for projecting world-space part anchors.
struct Plane {
    origin: [f64; 3],
    x_dir: [f64; 3],
    y_dir: [f64; 3],
}

impl Plane {
    fn xy() -> Self {
        Plane {
            origin: [0.0; 3],
            x_dir: [1.0, 0.0, 0.0],
            y_dir: [0.0, 1.0, 0.0],
        }
    }

    fn project(&self, p: [f64; 3]) -> (f64, f64) {
        let d = [
            p[0] - self.origin[0],
            p[1] - self.origin[1],
            p[2] - self.origin[2],
        ];
        (
            d[0] * self.x_dir[0] + d[1] * self.x_dir[1] + d[2] * self.x_dir[2],
            d[0] * self.y_dir[0] + d[1] * self.y_dir[1] + d[2] * self.y_dir[2],
        )
    }
}

fn plane_for(doc: &Document, node: NodeId) -> Plane {
    match doc.nodes.get(&node).map(|n| &n.op) {
        Some(CsgOp::Sketch2D {
            origin,
            x_dir,
            y_dir,
            ..
        }) => Plane {
            origin: [origin.x, origin.y, origin.z],
            x_dir: [x_dir.x, x_dir.y, x_dir.z],
            y_dir: [y_dir.x, y_dir.y, y_dir.z],
        },
        _ => Plane::xy(),
    }
}

/// The plane a constraint measures in: that of its first free anchor's node.
fn measure_plane(doc: &Document, kind: &ConstraintKind) -> Plane {
    if let ConstraintKind::Rotation { node, .. } = kind {
        return plane_for(doc, *node);
    }
    kind.anchors()
        .iter()
        .find(|a| !matches!(a, Anchor::PartEdge { .. }))
        .map(|a| plane_for(doc, a.node()))
        .unwrap_or_else(Plane::xy)
}

/// In-plane position of a point-like anchor.
fn anchor_point(
    doc: &Document,
    resolver: &dyn AnchorResolver,
    plane: &Plane,
    anchor: &Anchor,
) -> Result<(f64, f64), String> {
    match anchor {
        Anchor::PcbFootprint { node, r#ref, pad } => {
            let pcb = match doc.nodes.get(node).map(|n| &n.op) {
                Some(CsgOp::PcbBoard { board }) => board,
                _ => return Err(format!("node {node} is not a PCB board")),
            };
            let fp = pcb
                .footprints
                .iter()
                .find(|f| f.reference == *r#ref)
                .ok_or_else(|| format!("footprint \"{ref}\" not found", ref = r#ref))?;
            match pad {
                None => Ok((fp.position.x, fp.position.y)),
                Some(pad_name) => {
                    let p = fp
                        .pads
                        .iter()
                        .find(|p| p.number == *pad_name)
                        .ok_or_else(|| format!("pad \"{pad_name}\" not found"))?;
                    let w = vcad_ecad_pcb::geometry::pad_world_position(fp, p);
                    Ok((w.x, w.y))
                }
            }
        }
        Anchor::PcbOutlineVertex { node, index } => {
            let pcb = match doc.nodes.get(node).map(|n| &n.op) {
                Some(CsgOp::PcbBoard { board }) => board,
                _ => return Err(format!("node {node} is not a PCB board")),
            };
            pcb.outline
                .vertices
                .get(*index as usize)
                .map(|v| (v.x, v.y))
                .ok_or_else(|| format!("outline vertex {index} out of range"))
        }
        Anchor::SketchPoint {
            node,
            segment,
            point,
        } => {
            let segments = match doc.nodes.get(node).map(|n| &n.op) {
                Some(CsgOp::Sketch2D { segments, .. }) => segments,
                _ => return Err(format!("node {node} is not a sketch")),
            };
            let seg = segments
                .get(*segment as usize)
                .ok_or_else(|| format!("sketch segment {segment} out of range"))?;
            use vcad_ir::SketchSegment2D as S;
            match (seg, point) {
                (S::Line { start, .. } | S::Arc { start, .. }, SketchPointRef::Start) => {
                    Ok((start.x, start.y))
                }
                (S::Line { end, .. } | S::Arc { end, .. }, SketchPointRef::End) => {
                    Ok((end.x, end.y))
                }
                (S::Arc { center, .. }, SketchPointRef::Center) => Ok((center.x, center.y)),
                _ => Err(format!("segment {segment} has no {point:?} point")),
            }
        }
        Anchor::PartEdge {
            node,
            face_a,
            face_b,
            ..
        } => {
            let (a, b) = resolver.resolve_part_edge(*node, face_a, face_b)?;
            let mid = [
                (a[0] + b[0]) / 2.0,
                (a[1] + b[1]) / 2.0,
                (a[2] + b[2]) / 2.0,
            ];
            Ok(plane.project(mid))
        }
        Anchor::PcbOutlineEdge { .. } | Anchor::SketchSegment { .. } => {
            Err("edge anchor used where a point is required".into())
        }
    }
}

/// A pair of in-plane points (an edge's endpoints).
type EdgePoints = ((f64, f64), (f64, f64));

/// In-plane endpoints of an edge-like anchor.
fn anchor_edge(
    doc: &Document,
    resolver: &dyn AnchorResolver,
    plane: &Plane,
    anchor: &Anchor,
) -> Result<EdgePoints, String> {
    match anchor {
        Anchor::PcbOutlineEdge { node, index } => {
            let pcb = match doc.nodes.get(node).map(|n| &n.op) {
                Some(CsgOp::PcbBoard { board }) => board,
                _ => return Err(format!("node {node} is not a PCB board")),
            };
            let n = pcb.outline.vertices.len();
            let i = *index as usize;
            if i >= n {
                return Err(format!("outline edge {index} out of range"));
            }
            let a = pcb.outline.vertices[i];
            let b = pcb.outline.vertices[(i + 1) % n];
            Ok(((a.x, a.y), (b.x, b.y)))
        }
        Anchor::SketchSegment { node, segment } => {
            let segments = match doc.nodes.get(node).map(|n| &n.op) {
                Some(CsgOp::Sketch2D { segments, .. }) => segments,
                _ => return Err(format!("node {node} is not a sketch")),
            };
            use vcad_ir::SketchSegment2D as S;
            segments
                .get(*segment as usize)
                .map(|seg| match seg {
                    S::Line { start, end } | S::Arc { start, end, .. } => {
                        ((start.x, start.y), (end.x, end.y))
                    }
                })
                .ok_or_else(|| format!("sketch segment {segment} out of range"))
        }
        Anchor::PartEdge {
            node,
            face_a,
            face_b,
            ..
        } => {
            let (a, b) = resolver.resolve_part_edge(*node, face_a, face_b)?;
            Ok((plane.project(a), plane.project(b)))
        }
        _ => Err("point anchor used where an edge is required".into()),
    }
}

/// Measure the current value of a dimensional constraint (mm or degrees).
pub(crate) fn measure_constraint(
    doc: &Document,
    resolver: &dyn AnchorResolver,
    kind: &ConstraintKind,
) -> Result<f64, String> {
    let plane = measure_plane(doc, kind);
    match kind {
        ConstraintKind::Distance { a, b, .. } => {
            let pa = anchor_point(doc, resolver, &plane, a)?;
            let pb = anchor_point(doc, resolver, &plane, b)?;
            Ok(((pa.0 - pb.0).powi(2) + (pa.1 - pb.1).powi(2)).sqrt())
        }
        ConstraintKind::HorizontalDistance { a, .. } => {
            Ok(anchor_point(doc, resolver, &plane, a)?.0)
        }
        ConstraintKind::VerticalDistance { a, .. } => Ok(anchor_point(doc, resolver, &plane, a)?.1),
        ConstraintKind::Length { a, .. } => {
            let (pa, pb) = anchor_edge(doc, resolver, &plane, a)?;
            Ok(((pa.0 - pb.0).powi(2) + (pa.1 - pb.1).powi(2)).sqrt())
        }
        ConstraintKind::Rotation { node, r#ref, .. } => {
            let pcb = match doc.nodes.get(node).map(|n| &n.op) {
                Some(CsgOp::PcbBoard { board }) => board,
                _ => return Err(format!("node {node} is not a PCB board")),
            };
            pcb.footprints
                .iter()
                .find(|f| f.reference == *r#ref)
                .map(|f| f.rotation)
                .ok_or_else(|| format!("footprint \"{ref}\" not found", ref = r#ref))
        }
        ConstraintKind::Angle { a, b, .. } => {
            let (a1, a2) = anchor_edge(doc, resolver, &plane, a)?;
            let (b1, b2) = anchor_edge(doc, resolver, &plane, b)?;
            let da = (a2.0 - a1.0, a2.1 - a1.1);
            let db = (b2.0 - b1.0, b2.1 - b1.1);
            let cross = da.0 * db.1 - da.1 * db.0;
            let dot = da.0 * db.0 + da.1 * db.1;
            Ok(cross.atan2(dot).to_degrees().abs())
        }
        _ => Err("constraint is not dimensional".into()),
    }
}

/// Residual magnitude of any constraint kind against current geometry —
/// used by receipt verification ("does this constraint still hold?").
pub fn constraint_residual(
    doc: &Document,
    resolver: &dyn AnchorResolver,
    kind: &ConstraintKind,
    target: Option<f64>,
) -> Result<f64, String> {
    let plane = measure_plane(doc, kind);
    let point = |a: &Anchor| anchor_point(doc, resolver, &plane, a);
    let edge = |a: &Anchor| anchor_edge(doc, resolver, &plane, a);
    let dir = |e: ((f64, f64), (f64, f64))| {
        let d = ((e.1 .0 - e.0 .0), (e.1 .1 - e.0 .1));
        let len = (d.0 * d.0 + d.1 * d.1).sqrt();
        if len < 1e-12 {
            (0.0, 0.0)
        } else {
            (d.0 / len, d.1 / len)
        }
    };
    match kind {
        ConstraintKind::Coincident { a, b } | ConstraintKind::Concentric { a, b } => {
            let (pa, pb) = (point(a)?, point(b)?);
            Ok(((pa.0 - pb.0).powi(2) + (pa.1 - pb.1).powi(2)).sqrt())
        }
        ConstraintKind::Horizontal { a, b } => {
            if a.is_edge() && a == b {
                let e = edge(a)?;
                Ok((e.1 .1 - e.0 .1).abs())
            } else {
                Ok((point(a)?.1 - point(b)?.1).abs())
            }
        }
        ConstraintKind::Vertical { a, b } => {
            if a.is_edge() && a == b {
                let e = edge(a)?;
                Ok((e.1 .0 - e.0 .0).abs())
            } else {
                Ok((point(a)?.0 - point(b)?.0).abs())
            }
        }
        ConstraintKind::Parallel { a, b } => {
            let (da, db) = (dir(edge(a)?), dir(edge(b)?));
            Ok((da.0 * db.1 - da.1 * db.0).abs())
        }
        ConstraintKind::Perpendicular { a, b } => {
            let (da, db) = (dir(edge(a)?), dir(edge(b)?));
            Ok((da.0 * db.0 + da.1 * db.1).abs())
        }
        ConstraintKind::EqualLength { a, b } => {
            let la = {
                let e = edge(a)?;
                ((e.0 .0 - e.1 .0).powi(2) + (e.0 .1 - e.1 .1).powi(2)).sqrt()
            };
            let lb = {
                let e = edge(b)?;
                ((e.0 .0 - e.1 .0).powi(2) + (e.0 .1 - e.1 .1).powi(2)).sqrt()
            };
            Ok((la - lb).abs())
        }
        ConstraintKind::PointOnEdge { point: p, edge: e } => {
            let pt = point(p)?;
            let ((sx, sy), (ex, ey)) = edge(e)?;
            let (dx, dy) = (ex - sx, ey - sy);
            let len = (dx * dx + dy * dy).sqrt();
            if len < 1e-12 {
                return Err("degenerate edge".into());
            }
            Ok((((pt.0 - sx) * dy - (pt.1 - sy) * dx) / len).abs())
        }
        ConstraintKind::Symmetric { a, b, axis } => {
            let (pa, pb) = (point(a)?, point(b)?);
            let ((sx, sy), (ex, ey)) = edge(axis)?;
            let (dx, dy) = (ex - sx, ey - sy);
            let len = (dx * dx + dy * dy).sqrt();
            if len < 1e-12 {
                return Err("degenerate axis".into());
            }
            let da = ((pa.0 - sx) * dy - (pa.1 - sy) * dx) / len;
            let db = ((pb.0 - sx) * dy - (pb.1 - sy) * dx) / len;
            // Mirror: signed distances sum to zero, midpoint on axis dir.
            Ok((da + db).abs())
        }
        ConstraintKind::Fixed { .. } => Ok(0.0),
        _ => {
            // Dimensional kinds: |measured - target|.
            let measured = measure_constraint(doc, resolver, kind)?;
            let target = target.ok_or("dimensional constraint requires a resolved target")?;
            Ok((measured - target).abs())
        }
    }
}
