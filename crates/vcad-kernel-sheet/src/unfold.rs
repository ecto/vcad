//! Lossless bidirectional unfold.
//!
//! `unfold` and `refold` are **inverses by construction** because both
//! configurations of every panel — `frame_bent` and `frame_flat` — are
//! derivable from the same primary data: the panel-local outline + the bend
//! tree's `(edge_parent, angle, radius, k_factor, direction)` per bend.
//!
//! Concretely:
//!
//! - [`refold`] walks the bend tree from the root and recomputes each
//!   non-root panel's `frame_bent` from its parent's `frame_bent` and the
//!   bend metadata.
//! - [`unfold`] walks the same tree and recomputes each non-root panel's
//!   `frame_flat` from its parent's `frame_flat` and the bend metadata
//!   (with the bend allowance offsetting the child along the parent's
//!   in-plane outward direction).
//!
//! The involution test [`tests::round_trip_is_identity`] proves
//! `refold ∘ unfold = identity` on bent frames within tolerance, and
//! [`tests::flat_round_trip_is_identity`] proves `unfold ∘ refold =
//! identity` on flat frames.
//!
//! The exported [`FlatPattern`] type is the manufacturing-side view —
//! global 2D coordinates of every outline + crease, suitable for DXF
//! export, nesting, and the flat-pattern editor in the UI.

use crate::bend_table::bend_allowance;
use crate::model::{Bend, BendDirection, Frame, PanelId, SheetMetalModel};
use vcad_kernel_math::{Dir3, Point2, Point3, Transform, Vec3};

/// Errors returned by [`unfold`] / [`refold`].
#[derive(Debug, Clone, PartialEq)]
pub enum UnfoldError {
    /// Model is empty (no panels).
    EmptyModel,
    /// Model contains a cycle (not yet supported).
    CycleDetected,
}

impl std::fmt::Display for UnfoldError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UnfoldError::EmptyModel => write!(f, "model has no panels"),
            UnfoldError::CycleDetected => write!(f, "model contains a cycle"),
        }
    }
}

impl std::error::Error for UnfoldError {}

/// Recompute every panel's `frame_flat` by walking the bend tree from the
/// root.
///
/// The root panel keeps its existing `frame_flat`. Each non-root panel's
/// `frame_flat` is its parent's flat frame translated outward by the bend
/// allowance — no rotation, since the panels lie coplanar in the flat
/// pattern.
pub fn unfold(model: &mut SheetMetalModel) -> Result<(), UnfoldError> {
    if model.panels.is_empty() {
        return Err(UnfoldError::EmptyModel);
    }
    walk_and_update(model, FrameKind::Flat)
}

/// Recompute every panel's `frame_bent` by walking the bend tree from the
/// root.
///
/// The root panel keeps its existing `frame_bent`. Each non-root panel's
/// `frame_bent` is its parent's bent frame rotated about the hinge axis by
/// the (signed) bend angle.
pub fn refold(model: &mut SheetMetalModel) -> Result<(), UnfoldError> {
    if model.panels.is_empty() {
        return Err(UnfoldError::EmptyModel);
    }
    walk_and_update(model, FrameKind::Bent)
}

/// Which configuration we're recomputing.
#[derive(Debug, Clone, Copy)]
enum FrameKind {
    Bent,
    Flat,
}

fn walk_and_update(model: &mut SheetMetalModel, kind: FrameKind) -> Result<(), UnfoldError> {
    let order: Vec<(PanelId, Option<usize>)> = model.bfs().collect();
    if order.len() != model.panels.len() {
        return Err(UnfoldError::CycleDetected);
    }
    for &(panel_id, via_bend) in order.iter().skip(1) {
        let bend_id = via_bend.expect("non-root panel must have an incoming bend");
        let bend = model.bends[bend_id].clone();
        let parent_id = if bend.parent == panel_id {
            bend.child
        } else {
            bend.parent
        };
        let parent_frame = match kind {
            FrameKind::Bent => model.panels[parent_id].frame_bent,
            FrameKind::Flat => model.panels[parent_id].frame_flat,
        };
        let new_frame = match kind {
            FrameKind::Bent => child_bent_frame(&parent_frame, &bend),
            FrameKind::Flat => child_flat_frame(&parent_frame, &bend, model.thickness),
        };
        match kind {
            FrameKind::Bent => model.panels[panel_id].frame_bent = new_frame,
            FrameKind::Flat => model.panels[panel_id].frame_flat = new_frame,
        }
    }
    Ok(())
}

/// Compute the child panel's bent frame from the parent's bent frame.
///
/// Mirrors the geometry in [`crate::edge_flange::add_edge_flange`]: rotate
/// the parent's outward in-plane direction about the hinge axis by the
/// signed bend angle. Origin sits at `parent.to_world(edge_parent.0)`,
/// which is on the axis and therefore fixed by the rotation.
fn child_bent_frame(parent_frame: &Frame, bend: &Bend) -> Frame {
    let (p0, p1) = bend.edge_parent;
    let edge_dir_2d = p1 - p0;
    let edge_len = edge_dir_2d.norm();
    let edge_dir_2d = edge_dir_2d / edge_len;
    let outward_2d = vcad_kernel_math::Vec2::new(edge_dir_2d.y, -edge_dir_2d.x);

    let edge_dir_3d = direction_to_world(parent_frame, edge_dir_2d.x, edge_dir_2d.y);
    let outward_3d = direction_to_world(parent_frame, outward_2d.x, outward_2d.y);

    let signed_angle = bend.direction.sign() * bend.angle;
    let axis = Dir3::new_normalize(edge_dir_3d);
    let rot = Transform::rotation_about_axis(&axis, signed_angle);
    let child_y = rot.apply_vec(&outward_3d);
    let child_origin = parent_frame.to_world(p0);
    Frame {
        origin: child_origin,
        x_dir: edge_dir_3d,
        y_dir: child_y,
    }
}

/// Compute the child panel's flat frame from the parent's flat frame.
///
/// In the flat pattern, the child sits coplanar with the parent. The hinge
/// edge in the parent's 2D becomes a crease line, and the child's outline
/// continues on the *outward* side of that crease, separated by the bend
/// allowance.
fn child_flat_frame(parent_frame: &Frame, bend: &Bend, thickness: f64) -> Frame {
    let (p0, p1) = bend.edge_parent;
    let edge_dir_2d = p1 - p0;
    let edge_len = edge_dir_2d.norm();
    let edge_dir_2d = edge_dir_2d / edge_len;
    let outward_2d = vcad_kernel_math::Vec2::new(edge_dir_2d.y, -edge_dir_2d.x);

    let edge_dir_3d = direction_to_world(parent_frame, edge_dir_2d.x, edge_dir_2d.y);
    let outward_3d = direction_to_world(parent_frame, outward_2d.x, outward_2d.y);

    let ba = bend_allowance(bend.angle, bend.radius, bend.k_factor, thickness);
    let parent_hinge_3d = parent_frame.to_world(p0);
    let child_origin = Point3::new(
        parent_hinge_3d.x + outward_3d.x * ba,
        parent_hinge_3d.y + outward_3d.y * ba,
        parent_hinge_3d.z + outward_3d.z * ba,
    );
    Frame {
        origin: child_origin,
        x_dir: edge_dir_3d,
        y_dir: outward_3d,
    }
}

fn direction_to_world(frame: &Frame, dx: f64, dy: f64) -> Vec3 {
    Vec3::new(
        frame.x_dir.x * dx + frame.y_dir.x * dy,
        frame.x_dir.y * dx + frame.y_dir.y * dy,
        frame.x_dir.z * dx + frame.y_dir.z * dy,
    )
}

/// Manufacturing-side flat pattern: global 2D outlines + creases, ready
/// for DXF export, nesting, or the flat-pattern UI editor.
///
/// Constructed by [`FlatPattern::from_model`] which projects every panel's
/// outline through its `frame_flat` into the plane defined by the root
/// panel's flat frame.
#[derive(Debug, Clone, PartialEq)]
pub struct FlatPattern {
    /// Material thickness (mm).
    pub thickness: f64,
    /// Outlines in global flat 2D coords. One entry per panel, in panel-id
    /// order.
    pub panel_outlines_2d: Vec<Vec<Point2>>,
    /// Hole loops per panel.
    pub panel_holes_2d: Vec<Vec<Vec<Point2>>>,
    /// Crease lines.
    pub creases: Vec<FlatCrease>,
    /// Total flat-pattern area (mm²) — sum of panel areas + bend-allowance
    /// rectangles. Used by costing.
    pub area_mm2: f64,
}

/// A crease in the flat pattern (one bend = one crease).
#[derive(Debug, Clone, PartialEq)]
pub struct FlatCrease {
    /// Crease line in global flat 2D coords (start, end).
    pub line: (Point2, Point2),
    /// Bend angle (radians).
    pub angle: f64,
    /// Inside radius (mm).
    pub radius: f64,
    /// K-factor used.
    pub k_factor: f64,
    /// Provenance label of the K-factor (`"builtin:Al-soft/R1.00t1.00"` etc.).
    pub k_factor_source: Option<String>,
    /// Up or Down — drives DXF layer selection.
    pub direction: BendDirection,
    /// Backreference: which `Bend` produced this crease.
    pub bend_id: usize,
    /// Unit normal of the crease in global flat 2D, pointing from the
    /// parent edge toward the child panel (the direction the allowance
    /// strip extends). Computed from the parent's frame — the crease
    /// line's own winding is NOT a reliable source, since chained flat
    /// frames may be orientation-reversing in global 2D.
    pub outward: vcad_kernel_math::Vec2,
}

impl FlatPattern {
    /// Project a sheet-metal model into a global 2D flat pattern using each
    /// panel's `frame_flat`.
    ///
    /// Coordinate system: the root panel's `frame_flat` defines the global
    /// 2D plane. The root panel's outline ends up in its panel-local 2D
    /// coordinates verbatim; other panels are projected.
    pub fn from_model(model: &SheetMetalModel) -> Self {
        let root = &model.panels[model.root];
        let root_frame = root.frame_flat;
        let to_global = |frame: Frame, p: Point2| -> Point2 {
            // Global 2D = ((world - root_origin) · root.x_dir, (world - root_origin) · root.y_dir)
            let world = frame.to_world(p);
            let rel = world - root_frame.origin;
            Point2::new(rel.dot(root_frame.x_dir), rel.dot(root_frame.y_dir))
        };

        let panel_outlines_2d: Vec<Vec<Point2>> = model
            .panels
            .iter()
            .map(|panel| {
                panel
                    .outline
                    .iter()
                    .map(|&p| to_global(panel.frame_flat, p))
                    .collect()
            })
            .collect();

        let panel_holes_2d: Vec<Vec<Vec<Point2>>> = model
            .panels
            .iter()
            .map(|panel| {
                panel
                    .holes
                    .iter()
                    .map(|h| h.iter().map(|&p| to_global(panel.frame_flat, p)).collect())
                    .collect()
            })
            .collect();

        let creases: Vec<FlatCrease> = model
            .bends
            .iter()
            .enumerate()
            .map(|(id, bend)| {
                let parent = &model.panels[bend.parent];
                let (p0, p1) = bend.edge_parent;
                // Child-pointing normal: parent-local outward (CCW outline
                // ⇒ edge rotated 90° CW), lifted through the parent's flat
                // frame and projected into global 2D.
                let edge = (p1 - p0) / (p1 - p0).norm();
                let outward_local = vcad_kernel_math::Vec2::new(edge.y, -edge.x);
                let outward_world =
                    direction_to_world(&parent.frame_flat, outward_local.x, outward_local.y);
                let outward = vcad_kernel_math::Vec2::new(
                    outward_world.dot(root_frame.x_dir),
                    outward_world.dot(root_frame.y_dir),
                );
                FlatCrease {
                    line: (
                        to_global(parent.frame_flat, p0),
                        to_global(parent.frame_flat, p1),
                    ),
                    angle: bend.angle,
                    radius: bend.radius,
                    k_factor: bend.k_factor,
                    k_factor_source: bend.k_factor_source.clone(),
                    direction: bend.direction,
                    bend_id: id,
                    outward,
                }
            })
            .collect();

        let area_mm2 =
            polygon_area_sum(&panel_outlines_2d, &panel_holes_2d) + bend_strip_area(model);

        Self {
            thickness: model.thickness,
            panel_outlines_2d,
            panel_holes_2d,
            creases,
            area_mm2,
        }
    }

    /// 2D bounding box `((min_x, min_y), (max_x, max_y))` of the flat
    /// pattern, including bend allowance gaps.
    pub fn bbox(&self) -> ((f64, f64), (f64, f64)) {
        let mut min = (f64::INFINITY, f64::INFINITY);
        let mut max = (f64::NEG_INFINITY, f64::NEG_INFINITY);
        for outline in &self.panel_outlines_2d {
            for p in outline {
                if p.x < min.0 {
                    min.0 = p.x;
                }
                if p.y < min.1 {
                    min.1 = p.y;
                }
                if p.x > max.0 {
                    max.0 = p.x;
                }
                if p.y > max.1 {
                    max.1 = p.y;
                }
            }
        }
        (min, max)
    }
}

fn polygon_area(loop_pts: &[Point2]) -> f64 {
    if loop_pts.len() < 3 {
        return 0.0;
    }
    let mut sum = 0.0;
    for i in 0..loop_pts.len() {
        let a = loop_pts[i];
        let b = loop_pts[(i + 1) % loop_pts.len()];
        sum += a.x * b.y - b.x * a.y;
    }
    0.5 * sum.abs()
}

fn polygon_area_sum(outlines: &[Vec<Point2>], holes: &[Vec<Vec<Point2>>]) -> f64 {
    let mut sum = 0.0;
    for (outline, hole_set) in outlines.iter().zip(holes) {
        sum += polygon_area(outline);
        for h in hole_set {
            sum -= polygon_area(h);
        }
    }
    sum
}

fn bend_strip_area(model: &SheetMetalModel) -> f64 {
    model
        .bends
        .iter()
        .map(|b| {
            let (p0, p1) = b.edge_parent;
            let edge_len = (p1 - p0).norm();
            edge_len * b.allowance(model.thickness)
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base_flange::base_flange_rect;
    use crate::bend_table::BendTable;
    use crate::edge_flange::{add_edge_flange, EdgeFlangeParams, FlangePosition};
    use std::f64::consts::FRAC_PI_2;

    fn frame_close(a: &Frame, b: &Frame, tol: f64) -> bool {
        (a.origin - b.origin).norm() < tol
            && (a.x_dir - b.x_dir).norm() < tol
            && (a.y_dir - b.y_dir).norm() < tol
    }

    fn make_l_bracket() -> SheetMetalModel {
        let mut m = base_flange_rect(100.0, 50.0, 1.0).unwrap();
        let table = BendTable::builtin();
        add_edge_flange(
            &mut m,
            &table,
            EdgeFlangeParams {
                panel: 0,
                edge_index: 0,
                length: 25.0,
                angle: FRAC_PI_2,
                radius: 1.0,
                direction: BendDirection::Up,
                position: FlangePosition::MaterialInside,
                material: "Al-soft".into(),
                manual_k: None,
            },
        )
        .unwrap();
        m
    }

    fn make_u_channel() -> SheetMetalModel {
        let mut m = base_flange_rect(100.0, 50.0, 1.0).unwrap();
        let table = BendTable::builtin();
        // Edge 0: y=0 side
        add_edge_flange(
            &mut m,
            &table,
            EdgeFlangeParams {
                panel: 0,
                edge_index: 0,
                length: 25.0,
                angle: FRAC_PI_2,
                radius: 1.0,
                direction: BendDirection::Up,
                position: FlangePosition::MaterialInside,
                material: "Al-soft".into(),
                manual_k: None,
            },
        )
        .unwrap();
        // Edge 2: y=50 side
        add_edge_flange(
            &mut m,
            &table,
            EdgeFlangeParams {
                panel: 0,
                edge_index: 2,
                length: 25.0,
                angle: FRAC_PI_2,
                radius: 1.0,
                direction: BendDirection::Up,
                position: FlangePosition::MaterialInside,
                material: "Al-soft".into(),
                manual_k: None,
            },
        )
        .unwrap();
        m
    }

    /// **The legendary involution proof.** Take a model, save its bent
    /// frames, run unfold (which doesn't touch bent frames) then refold
    /// (which recomputes them from scratch using the bend tree). The
    /// recomputed frames must equal the originals within tolerance.
    #[test]
    fn round_trip_is_identity_l_bracket() {
        let mut m = make_l_bracket();
        let originals: Vec<Frame> = m.panels.iter().map(|p| p.frame_bent).collect();
        unfold(&mut m).unwrap();
        // Mutate frame_bent to garbage to prove refold actually rebuilds it.
        for p in &mut m.panels[1..] {
            p.frame_bent = Frame::identity();
        }
        refold(&mut m).unwrap();
        for (i, orig) in originals.iter().enumerate() {
            assert!(
                frame_close(&m.panels[i].frame_bent, orig, 1e-9),
                "panel {i}: {:?} vs {:?}",
                m.panels[i].frame_bent,
                orig
            );
        }
    }

    #[test]
    fn round_trip_is_identity_u_channel() {
        let mut m = make_u_channel();
        let originals: Vec<Frame> = m.panels.iter().map(|p| p.frame_bent).collect();
        unfold(&mut m).unwrap();
        for p in &mut m.panels[1..] {
            p.frame_bent = Frame::identity();
        }
        refold(&mut m).unwrap();
        for (i, orig) in originals.iter().enumerate() {
            assert!(
                frame_close(&m.panels[i].frame_bent, orig, 1e-9),
                "panel {i}",
            );
        }
    }

    /// Symmetric: unfolding and re-unfolding doesn't drift.
    #[test]
    fn flat_round_trip_is_identity() {
        let mut m = make_u_channel();
        let originals: Vec<Frame> = m.panels.iter().map(|p| p.frame_flat).collect();
        // Garbage flat frames, then unfold to recompute, twice.
        for p in &mut m.panels[1..] {
            p.frame_flat = Frame::identity();
        }
        unfold(&mut m).unwrap();
        for (i, orig) in originals.iter().enumerate() {
            assert!(
                frame_close(&m.panels[i].frame_flat, orig, 1e-9),
                "panel {i}"
            );
        }
    }

    /// Stable under repeated round-trips — no drift accumulation.
    #[test]
    fn no_drift_under_repeated_round_trip() {
        let mut m = make_u_channel();
        let originals: Vec<Frame> = m.panels.iter().map(|p| p.frame_bent).collect();
        for _ in 0..10 {
            unfold(&mut m).unwrap();
            refold(&mut m).unwrap();
        }
        for (i, orig) in originals.iter().enumerate() {
            assert!(
                frame_close(&m.panels[i].frame_bent, orig, 1e-9),
                "panel {i} drifted after 10 round-trips",
            );
        }
    }

    #[test]
    fn flat_pattern_root_at_origin() {
        let m = make_l_bracket();
        let fp = FlatPattern::from_model(&m);
        // Root panel's outline starts at panel-local (0,0) → flat (0,0).
        assert!(fp.panel_outlines_2d[0][0].x.abs() < 1e-12);
        assert!(fp.panel_outlines_2d[0][0].y.abs() < 1e-12);
    }

    #[test]
    fn flat_pattern_child_offset_by_ba() {
        let m = make_l_bracket();
        let fp = FlatPattern::from_model(&m);
        let bend = &m.bends[0];
        let ba = bend.allowance(m.thickness);
        // Edge 0 of the root rect is along +x at y=0; outward in 2D is (0,-1).
        // So child outline's first point should be at (0, -BA).
        let p0 = fp.panel_outlines_2d[1][0];
        assert!(p0.x.abs() < 1e-9);
        assert!(
            (p0.y - (-ba)).abs() < 1e-9,
            "expected y={} got {}",
            -ba,
            p0.y
        );
    }

    #[test]
    fn flat_pattern_creases_have_provenance() {
        let m = make_u_channel();
        let fp = FlatPattern::from_model(&m);
        assert_eq!(fp.creases.len(), 2);
        for c in &fp.creases {
            assert!(c.k_factor_source.is_some(), "missing provenance");
        }
    }

    #[test]
    fn flat_pattern_area_includes_bend_strips() {
        let m = make_l_bracket();
        let fp = FlatPattern::from_model(&m);
        let panel_area = 100.0 * 50.0 + 100.0 * 25.0;
        let bend_strip = 100.0 * m.bends[0].allowance(m.thickness);
        let expected = panel_area + bend_strip;
        assert!(
            (fp.area_mm2 - expected).abs() < 1e-6,
            "expected {expected}, got {}",
            fp.area_mm2
        );
    }

    #[test]
    fn empty_model_returns_error() {
        let mut m = SheetMetalModel::new(1.0);
        assert!(matches!(unfold(&mut m), Err(UnfoldError::EmptyModel)));
        assert!(matches!(refold(&mut m), Err(UnfoldError::EmptyModel)));
    }
}
