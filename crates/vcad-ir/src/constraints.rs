//! Document-level design constraints spanning sketches, PCB layout, and
//! mechanical parts.
//!
//! Unlike per-editor constraint state, these live on the
//! [`Document`](crate::Document) (`constraints: Vec<DesignConstraint>`) and
//! reference geometry through the universal [`Anchor`] vocabulary, so one
//! solver pass can keep a board outline dimensioned, footprints aligned, and
//! a connector coincident with an enclosure cutout — and every constraint
//! doubles as a verifiable receipt claim.
//!
//! Dimensional constraints hold an [`Expr`] value, so a dimension can be a
//! formula over named document parameters ("board_width - 2*edge_margin");
//! changing a parameter re-solves the whole set. A constraint with
//! `driven: true` is a reference dimension: it contributes no residuals and
//! its value is back-annotated from the solved geometry after each solve.

use serde::{Deserialize, Serialize};

use crate::parameters::Expr;
use crate::NodeId;

/// Which point of a sketch segment an [`Anchor::SketchPoint`] grips.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
#[serde(rename_all = "camelCase")]
pub enum SketchPointRef {
    /// Segment start point.
    Start,
    /// Segment end point.
    End,
    /// Arc center point.
    Center,
}

/// Geometric snapshot of a part edge for fallback resolution, mirroring the
/// kernel naming crate's `EdgeHint`. Recorded when the anchor is created so
/// the edge can be re-found geometrically if topological names drift.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
#[serde(rename_all = "camelCase")]
pub struct EdgeHintIr {
    /// Edge midpoint in world coordinates (mm).
    pub midpoint: [f64; 3],
    /// Unit direction of the edge.
    pub direction: [f64; 3],
    /// Edge length in mm.
    pub length: f64,
}

/// A stable reference to a piece of geometry a constraint grips.
///
/// Anchors are the universal target vocabulary: PCB footprints and outline
/// geometry, sketch segment points, and named mechanical part edges all
/// address through the same type, which is what lets one constraint relate
/// geometry across domains.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Anchor {
    /// A PCB footprint's origin, or a named pad center when `pad` is set.
    #[serde(rename_all = "camelCase")]
    PcbFootprint {
        /// PcbBoard node id.
        #[cfg_attr(feature = "ts-rs", ts(type = "number"))]
        node: NodeId,
        /// Footprint reference designator ("U1", "J3").
        r#ref: String,
        /// Pad name/number on that footprint; `None` = footprint origin.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts-rs", ts(optional))]
        pad: Option<String>,
    },
    /// A board outline vertex by index into `outline.vertices`.
    ///
    /// Known fragility: rewriting the outline invalidates indices, so
    /// outline-mutating tools drop (and report) referencing constraints.
    #[serde(rename_all = "camelCase")]
    PcbOutlineVertex {
        /// PcbBoard node id.
        #[cfg_attr(feature = "ts-rs", ts(type = "number"))]
        node: NodeId,
        /// 0-based vertex index.
        index: u32,
    },
    /// A board outline edge from vertex `index` to `index + 1` (wrapping).
    #[serde(rename_all = "camelCase")]
    PcbOutlineEdge {
        /// PcbBoard node id.
        #[cfg_attr(feature = "ts-rs", ts(type = "number"))]
        node: NodeId,
        /// 0-based index of the edge's start vertex.
        index: u32,
    },
    /// A point of a sketch segment (start/end/center).
    #[serde(rename_all = "camelCase")]
    SketchPoint {
        /// Sketch2D node id.
        #[cfg_attr(feature = "ts-rs", ts(type = "number"))]
        node: NodeId,
        /// 0-based segment index into the sketch's `segments`.
        segment: u32,
        /// Which point of the segment.
        point: SketchPointRef,
    },
    /// A whole sketch segment as an edge (its start→end chord for arcs).
    #[serde(rename_all = "camelCase")]
    SketchSegment {
        /// Sketch2D node id.
        #[cfg_attr(feature = "ts-rs", ts(type = "number"))]
        node: NodeId,
        /// 0-based segment index into the sketch's `segments`.
        segment: u32,
    },
    /// A mechanical part edge named by its two adjacent faces via the
    /// topological naming system (`vcad-kernel-naming` `FaceName` strings,
    /// e.g. `"cube:top"` / `"cube:front"`). Resolution is fail-closed:
    /// ambiguous or lost names skip the constraint with an error rather
    /// than silently rebinding.
    #[serde(rename_all = "camelCase")]
    PartEdge {
        /// Part root node id.
        #[cfg_attr(feature = "ts-rs", ts(type = "number"))]
        node: NodeId,
        /// First adjacent face name.
        face_a: String,
        /// Second adjacent face name.
        face_b: String,
        /// Geometric snapshot for fallback resolution.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts-rs", ts(optional))]
        hint: Option<EdgeHintIr>,
    },
}

impl Anchor {
    /// The node this anchor references.
    pub fn node(&self) -> NodeId {
        match self {
            Anchor::PcbFootprint { node, .. }
            | Anchor::PcbOutlineVertex { node, .. }
            | Anchor::PcbOutlineEdge { node, .. }
            | Anchor::SketchPoint { node, .. }
            | Anchor::SketchSegment { node, .. }
            | Anchor::PartEdge { node, .. } => *node,
        }
    }

    /// Whether this anchor is edge-like (has a direction), as required by
    /// `Parallel`/`Perpendicular`/`Horizontal`/`Vertical`-on-edge and
    /// `PointOnEdge`/`Symmetric` axes.
    pub fn is_edge(&self) -> bool {
        matches!(
            self,
            Anchor::PcbOutlineEdge { .. } | Anchor::SketchSegment { .. } | Anchor::PartEdge { .. }
        )
    }
}

/// The geometric relationship a [`DesignConstraint`] asserts.
///
/// Dimensional variants carry an [`Expr`] value (literal number or a formula
/// over document parameters). Distances/lengths are mm; angles/rotations are
/// degrees.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ConstraintKind {
    /// Two point anchors at the same position.
    #[serde(rename_all = "camelCase")]
    Coincident {
        /// First point.
        a: Anchor,
        /// Second point.
        b: Anchor,
    },
    /// Euclidean distance between two point anchors equals `value` (mm).
    #[serde(rename_all = "camelCase")]
    Distance {
        /// First point.
        a: Anchor,
        /// Second point.
        b: Anchor,
        /// Target distance in mm.
        value: Expr,
    },
    /// Anchor's X coordinate equals `value` (mm, board/sketch plane frame).
    #[serde(rename_all = "camelCase")]
    HorizontalDistance {
        /// The point.
        a: Anchor,
        /// Target X in mm.
        value: Expr,
    },
    /// Anchor's Y coordinate equals `value` (mm).
    #[serde(rename_all = "camelCase")]
    VerticalDistance {
        /// The point.
        a: Anchor,
        /// Target Y in mm.
        value: Expr,
    },
    /// Two point anchors share the same Y (horizontal alignment), or an
    /// edge anchor lies horizontal when `a` is edge-like and `b` is omitted
    /// via the same edge's endpoints.
    #[serde(rename_all = "camelCase")]
    Horizontal {
        /// First point (or edge start).
        a: Anchor,
        /// Second point (or edge end).
        b: Anchor,
    },
    /// Two point anchors share the same X (vertical alignment).
    #[serde(rename_all = "camelCase")]
    Vertical {
        /// First point.
        a: Anchor,
        /// Second point.
        b: Anchor,
    },
    /// Two edge anchors are parallel.
    #[serde(rename_all = "camelCase")]
    Parallel {
        /// First edge.
        a: Anchor,
        /// Second edge.
        b: Anchor,
    },
    /// Two edge anchors are perpendicular.
    #[serde(rename_all = "camelCase")]
    Perpendicular {
        /// First edge.
        a: Anchor,
        /// Second edge.
        b: Anchor,
    },
    /// Two edge anchors have equal length.
    #[serde(rename_all = "camelCase")]
    EqualLength {
        /// First edge.
        a: Anchor,
        /// Second edge.
        b: Anchor,
    },
    /// Edge anchor's length equals `value` (mm).
    #[serde(rename_all = "camelCase")]
    Length {
        /// The edge.
        a: Anchor,
        /// Target length in mm.
        value: Expr,
    },
    /// A point anchor lies on an edge anchor's carrier line.
    #[serde(rename_all = "camelCase")]
    PointOnEdge {
        /// The point.
        point: Anchor,
        /// The edge.
        edge: Anchor,
    },
    /// Two point anchors at the same position (alias of coincident used for
    /// hole↔boss intent; kept distinct for reporting/UI semantics).
    #[serde(rename_all = "camelCase")]
    Concentric {
        /// First center.
        a: Anchor,
        /// Second center.
        b: Anchor,
    },
    /// Lock a point anchor at its current position.
    #[serde(rename_all = "camelCase")]
    Fixed {
        /// The point.
        a: Anchor,
    },
    /// A footprint's rotation equals `value` (degrees).
    #[serde(rename_all = "camelCase")]
    Rotation {
        /// PcbBoard node id.
        #[cfg_attr(feature = "ts-rs", ts(type = "number"))]
        node: NodeId,
        /// Footprint reference designator.
        r#ref: String,
        /// Target rotation in degrees.
        value: Expr,
    },
    /// Two point anchors are mirror images across an edge anchor axis.
    #[serde(rename_all = "camelCase")]
    Symmetric {
        /// First point.
        a: Anchor,
        /// Second point.
        b: Anchor,
        /// Mirror axis (edge-like anchor).
        axis: Anchor,
    },
    /// Angle between two edge anchors equals `value` (degrees).
    #[serde(rename_all = "camelCase")]
    Angle {
        /// First edge.
        a: Anchor,
        /// Second edge.
        b: Anchor,
        /// Target angle in degrees.
        value: Expr,
    },
}

impl ConstraintKind {
    /// The dimensional value expression, if this kind carries one.
    pub fn value(&self) -> Option<&Expr> {
        match self {
            ConstraintKind::Distance { value, .. }
            | ConstraintKind::HorizontalDistance { value, .. }
            | ConstraintKind::VerticalDistance { value, .. }
            | ConstraintKind::Length { value, .. }
            | ConstraintKind::Rotation { value, .. }
            | ConstraintKind::Angle { value, .. } => Some(value),
            _ => None,
        }
    }

    /// Mutable access to the dimensional value expression (for driven
    /// back-annotation).
    pub fn value_mut(&mut self) -> Option<&mut Expr> {
        match self {
            ConstraintKind::Distance { value, .. }
            | ConstraintKind::HorizontalDistance { value, .. }
            | ConstraintKind::VerticalDistance { value, .. }
            | ConstraintKind::Length { value, .. }
            | ConstraintKind::Rotation { value, .. }
            | ConstraintKind::Angle { value, .. } => Some(value),
            _ => None,
        }
    }

    /// Whether this is a dimensional constraint (carries a value).
    pub fn is_dimensional(&self) -> bool {
        self.value().is_some()
    }

    /// All anchors this constraint references.
    pub fn anchors(&self) -> Vec<&Anchor> {
        match self {
            ConstraintKind::Coincident { a, b }
            | ConstraintKind::Distance { a, b, .. }
            | ConstraintKind::Horizontal { a, b }
            | ConstraintKind::Vertical { a, b }
            | ConstraintKind::Parallel { a, b }
            | ConstraintKind::Perpendicular { a, b }
            | ConstraintKind::EqualLength { a, b }
            | ConstraintKind::Concentric { a, b }
            | ConstraintKind::Angle { a, b, .. } => vec![a, b],
            ConstraintKind::HorizontalDistance { a, .. }
            | ConstraintKind::VerticalDistance { a, .. }
            | ConstraintKind::Length { a, .. }
            | ConstraintKind::Fixed { a } => vec![a],
            ConstraintKind::PointOnEdge { point, edge } => vec![point, edge],
            ConstraintKind::Symmetric { a, b, axis } => vec![a, b, axis],
            ConstraintKind::Rotation { .. } => vec![],
        }
    }
}

/// A persisted, solver-enforced design constraint.
///
/// Stored in `Document.constraints`; solved by `vcad-design-constraints`;
/// re-verified fail-closed as a `constraint.<label-or-id>` receipt claim.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
#[serde(rename_all = "camelCase")]
pub struct DesignConstraint {
    /// Stable id ("c1", "c2", …) for edit/delete and claim identity.
    pub id: String,
    /// Optional human label used for the receipt claim name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-rs", ts(optional))]
    pub label: Option<String>,
    /// The geometric relationship.
    pub kind: ConstraintKind,
    /// Driven (reference) dimension: contributes no residuals to the solve;
    /// its value is back-annotated from solved geometry instead. Only
    /// meaningful on dimensional kinds.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub driven: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp(node: NodeId, r: &str) -> Anchor {
        Anchor::PcbFootprint {
            node,
            r#ref: r.to_string(),
            pad: None,
        }
    }

    #[test]
    fn constraint_roundtrip() {
        let c = DesignConstraint {
            id: "c1".to_string(),
            label: Some("usb-inset".to_string()),
            kind: ConstraintKind::Distance {
                a: fp(3, "J1"),
                b: Anchor::PcbOutlineVertex { node: 3, index: 0 },
                value: Expr::formula("board_width - 2*edge_margin"),
            },
            driven: false,
        };
        let json = serde_json::to_string(&c).unwrap();
        let back: DesignConstraint = serde_json::from_str(&json).unwrap();
        assert_eq!(c, back);
        // driven=false is omitted from the wire form.
        assert!(!json.contains("driven"));
    }

    #[test]
    fn anchor_tags_are_camel_case() {
        let a = Anchor::PartEdge {
            node: 7,
            face_a: "cube:top".to_string(),
            face_b: "cube:front".to_string(),
            hint: None,
        };
        let json = serde_json::to_string(&a).unwrap();
        assert!(json.contains("\"kind\":\"partEdge\""), "{json}");
        assert!(json.contains("\"faceA\""), "{json}");
    }

    #[test]
    fn driven_roundtrip_and_value_access() {
        let mut c = DesignConstraint {
            id: "c2".to_string(),
            label: None,
            kind: ConstraintKind::Distance {
                a: fp(1, "U1"),
                b: fp(1, "U2"),
                value: Expr::num(10.0),
            },
            driven: true,
        };
        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains("\"driven\":true"));
        let back: DesignConstraint = serde_json::from_str(&json).unwrap();
        assert_eq!(c, back);
        *c.kind.value_mut().unwrap() = Expr::num(12.5);
        assert_eq!(c.kind.value().unwrap().as_number(), Some(12.5));
        assert!(c.kind.is_dimensional());
    }

    #[test]
    fn old_documents_without_constraints_parse() {
        // An empty document serializes without a `constraints` key (wire
        // compat), and such JSON deserializes to an empty set.
        let json = serde_json::to_string(&crate::Document::new()).unwrap();
        assert!(!json.contains("\"constraints\""), "{json}");
        let doc: crate::Document = serde_json::from_str(&json).unwrap();
        assert!(doc.constraints.is_empty());
    }
}
