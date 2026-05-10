//! [`add_edge_flange`] — extend an existing model with a new flange off an
//! edge of an existing panel.
//!
//! # Coordinate convention
//!
//! Each panel carries a 3D [`Frame`] mapping its panel-local 2D outline into
//! world coordinates. The bend axis coincides with the parent's edge in 3D
//! (zero-radius idealisation): the cylindrical bend region is implied
//! metadata and only materialises during tessellation/BRep generation. This
//! is what makes the panel graph a clean source-of-truth and unfold/refold
//! lossless — we never have to round-trip cylindrical surface fits.
//!
//! For a CCW outline, the *outward* in-plane direction perpendicular to edge
//! `(p, q)` is `rotate-90°-clockwise(q - p) = (dy, -dx)` (right-hand side of
//! the edge as you walk p→q).

use crate::bend_table::{bend_allowance, BendTable, KFactorSource};
use crate::model::{Bend, BendDirection, Frame, Panel, PanelId, SheetMetalModel};
use vcad_kernel_math::{Point2, Point3, Transform, Vec3};

/// Where the bent flange sits relative to the parent panel.
///
/// The naming follows SolidWorks convention. For the foundation tier we
/// support only `MaterialInside`, the simplest case. The other modes shift
/// the child panel's frame by a multiple of the thickness; we'll wire them
/// up alongside the manufacturability checks tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlangePosition {
    /// Inside (concave) face of the flange flush with the parent's outside
    /// edge. The most common default.
    MaterialInside,
}

/// Errors returned by [`add_edge_flange`].
#[derive(Debug, Clone, PartialEq)]
pub enum EdgeFlangeError {
    /// `panel_id` is out of bounds.
    UnknownPanel(PanelId),
    /// `edge_index` is out of bounds for the panel's outline.
    EdgeOutOfRange {
        /// Parent panel id.
        panel: PanelId,
        /// Requested edge index.
        edge_index: usize,
        /// Number of points (= number of edges) in the panel's outline.
        outline_len: usize,
    },
    /// `length`, `radius`, or `angle` is non-positive.
    NonPositive(&'static str, f64),
    /// Bend angle exceeds π (we only model bends in `(0, π]`).
    AngleTooLarge(f64),
    /// No K-factor row found and no manual override given.
    NoKFactor {
        /// Material name that was queried.
        material: String,
        /// Thickness that was queried (mm).
        thickness: f64,
        /// Inside bend radius that was queried (mm).
        radius: f64,
    },
}

impl std::fmt::Display for EdgeFlangeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EdgeFlangeError::UnknownPanel(p) => write!(f, "unknown panel {p}"),
            EdgeFlangeError::EdgeOutOfRange {
                panel,
                edge_index,
                outline_len,
            } => write!(
                f,
                "edge {edge_index} out of range for panel {panel} (outline has {outline_len} edges)"
            ),
            EdgeFlangeError::NonPositive(name, v) => write!(f, "{name} must be > 0, got {v}"),
            EdgeFlangeError::AngleTooLarge(a) => write!(f, "angle must be in (0, π], got {a}"),
            EdgeFlangeError::NoKFactor {
                material,
                thickness,
                radius,
            } => write!(
                f,
                "no K-factor found for material={material:?} t={thickness} R={radius}"
            ),
        }
    }
}

impl std::error::Error for EdgeFlangeError {}

/// Parameters for [`add_edge_flange`].
#[derive(Debug, Clone)]
pub struct EdgeFlangeParams {
    /// Parent panel.
    pub panel: PanelId,
    /// Index of the edge in the parent's outline (0 = outline\[0\]→outline\[1\]).
    pub edge_index: usize,
    /// Flange length perpendicular to the hinge edge (mm).
    pub length: f64,
    /// Bend angle (radians, 0 < angle ≤ π).
    pub angle: f64,
    /// Inside bend radius (mm).
    pub radius: f64,
    /// Direction the flange folds.
    pub direction: BendDirection,
    /// Position mode (foundation tier supports `MaterialInside` only).
    pub position: FlangePosition,
    /// Material name, used to look up the K-factor in `bend_table` if no
    /// `manual_k` is given.
    pub material: String,
    /// Optional manual K-factor override. When set, `bend_table` and
    /// `material` are ignored.
    pub manual_k: Option<f64>,
}

/// Extend `model` with a new flange off `params.edge_index` of `params.panel`.
///
/// Returns the new child [`PanelId`] and the new [`crate::model::BendId`].
pub fn add_edge_flange(
    model: &mut SheetMetalModel,
    bend_table: &BendTable,
    params: EdgeFlangeParams,
) -> Result<(PanelId, crate::model::BendId), EdgeFlangeError> {
    if params.panel >= model.panels.len() {
        return Err(EdgeFlangeError::UnknownPanel(params.panel));
    }
    if params.length <= 0.0 || params.length.is_nan() {
        return Err(EdgeFlangeError::NonPositive("length", params.length));
    }
    if params.radius <= 0.0 || params.radius.is_nan() {
        return Err(EdgeFlangeError::NonPositive("radius", params.radius));
    }
    if params.angle <= 0.0 || params.angle.is_nan() {
        return Err(EdgeFlangeError::NonPositive("angle", params.angle));
    }
    if params.angle > std::f64::consts::PI + 1e-12 {
        return Err(EdgeFlangeError::AngleTooLarge(params.angle));
    }

    let parent = &model.panels[params.panel];
    let n = parent.outline.len();
    if n < 3 || params.edge_index >= n {
        return Err(EdgeFlangeError::EdgeOutOfRange {
            panel: params.panel,
            edge_index: params.edge_index,
            outline_len: n,
        });
    }

    // Resolve K-factor — manual override beats table lookup.
    let (k_factor, source) = match params.manual_k {
        Some(k) => (k, KFactorSource::Manual),
        None => match bend_table.lookup(&params.material, model.thickness, params.radius) {
            Some((k, src)) => (k, src),
            None => {
                return Err(EdgeFlangeError::NoKFactor {
                    material: params.material.clone(),
                    thickness: model.thickness,
                    radius: params.radius,
                });
            }
        },
    };

    // Hinge edge endpoints in parent-local 2D.
    let p0 = parent.outline[params.edge_index];
    let p1 = parent.outline[(params.edge_index + 1) % n];
    let edge_len = (p1 - p0).norm();
    if edge_len < 1e-12 {
        return Err(EdgeFlangeError::NonPositive("edge length", edge_len));
    }

    // Parent-local 2D directions.
    let edge_dir_2d = (p1 - p0) / edge_len;
    // Outward normal of a CCW edge: rotate edge_dir 90° clockwise.
    let outward_2d = vcad_kernel_math::Vec2::new(edge_dir_2d.y, -edge_dir_2d.x);

    // Lift to 3D using the parent's bent frame.
    let parent_frame = parent.frame_bent;
    let edge_dir_3d = direction_to_world(&parent_frame, edge_dir_2d.x, edge_dir_2d.y);
    let outward_3d = direction_to_world(&parent_frame, outward_2d.x, outward_2d.y);

    // Bend axis is along edge_dir_3d, passing through the hinge endpoints in
    // 3D. The child's frame is obtained by rotating the "would-be flat
    // continuation" frame about the axis by `direction.sign() * angle`.
    let signed_angle = params.direction.sign() * params.angle;
    let axis = vcad_kernel_math::Dir3::new_normalize(edge_dir_3d);
    let rot = Transform::rotation_about_axis(&axis, signed_angle);

    // Child y_dir starts as outward_3d (flange would extend in that
    // direction if it were flat) and rotates with the bend.
    let child_y_dir_bent = rot.apply_vec(&outward_3d);

    // Child origin is on the hinge axis, at parent.to_world(p0). Rotation
    // about an axis through that point keeps it fixed.
    let child_origin_3d = parent_frame.to_world(p0);

    let child_frame_bent = Frame {
        origin: child_origin_3d,
        x_dir: edge_dir_3d,
        y_dir: child_y_dir_bent,
    };

    // Flat pose: in the flat layout, the child sits in the same plane as
    // the parent, separated from the hinge edge by the bend allowance.
    let ba = bend_allowance(params.angle, params.radius, k_factor, model.thickness);

    let flat_origin_3d = {
        let parent_flat_origin = parent.frame_flat.to_world(p0);
        let outward_flat_3d = direction_to_world(&parent.frame_flat, outward_2d.x, outward_2d.y);
        point3_offset(parent_flat_origin, outward_flat_3d, ba)
    };
    let flat_x_dir_3d = direction_to_world(&parent.frame_flat, edge_dir_2d.x, edge_dir_2d.y);
    let flat_y_dir_3d = direction_to_world(&parent.frame_flat, outward_2d.x, outward_2d.y);

    let child_frame_flat = Frame {
        origin: flat_origin_3d,
        x_dir: flat_x_dir_3d,
        y_dir: flat_y_dir_3d,
    };

    // Child outline: a rectangle of (edge_len × params.length) in child-local 2D.
    let child_outline = vec![
        Point2::new(0.0, 0.0),
        Point2::new(edge_len, 0.0),
        Point2::new(edge_len, params.length),
        Point2::new(0.0, params.length),
    ];

    let child_panel = Panel {
        outline: child_outline,
        holes: Vec::new(),
        frame_bent: child_frame_bent,
        frame_flat: child_frame_flat,
        incident_bends: Vec::new(),
    };
    let child_id = model.push_panel(child_panel);

    let bend = Bend {
        parent: params.panel,
        child: child_id,
        edge_parent: (p0, p1),
        radius: params.radius,
        angle: params.angle,
        direction: params.direction,
        k_factor,
        k_factor_source: Some(source.label()),
    };
    let bend_id = model.push_bend(bend);

    Ok((child_id, bend_id))
}

/// Lift a panel-local 2D direction `(dx, dy)` into world 3D using `frame`.
fn direction_to_world(frame: &Frame, dx: f64, dy: f64) -> Vec3 {
    Vec3::new(
        frame.x_dir.x * dx + frame.y_dir.x * dy,
        frame.x_dir.y * dx + frame.y_dir.y * dy,
        frame.x_dir.z * dx + frame.y_dir.z * dy,
    )
}

fn point3_offset(p: Point3, v: Vec3, scale: f64) -> Point3 {
    Point3::new(p.x + v.x * scale, p.y + v.y * scale, p.z + v.z * scale)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base_flange::base_flange_rect;
    use std::f64::consts::FRAC_PI_2;

    fn default_table() -> BendTable {
        BendTable::builtin()
    }

    fn al_soft_params(panel: PanelId, edge_index: usize) -> EdgeFlangeParams {
        EdgeFlangeParams {
            panel,
            edge_index,
            length: 25.0,
            angle: FRAC_PI_2,
            radius: 1.0,
            direction: BendDirection::Up,
            position: FlangePosition::MaterialInside,
            material: "Al-soft".to_string(),
            manual_k: None,
        }
    }

    #[test]
    fn adds_panel_and_bend() {
        let mut m = base_flange_rect(100.0, 50.0, 1.0).unwrap();
        let table = default_table();
        let (child_id, bend_id) = add_edge_flange(&mut m, &table, al_soft_params(0, 0)).unwrap();
        assert_eq!(child_id, 1);
        assert_eq!(bend_id, 0);
        assert_eq!(m.panels.len(), 2);
        assert_eq!(m.bends.len(), 1);
        assert_eq!(m.panels[0].incident_bends, vec![0]);
        assert_eq!(m.panels[1].incident_bends, vec![0]);
    }

    #[test]
    fn child_outline_has_correct_dimensions() {
        let mut m = base_flange_rect(100.0, 50.0, 1.0).unwrap();
        let table = default_table();
        // Edge 0 is (0,0)→(100,0); flange length 25.
        add_edge_flange(&mut m, &table, al_soft_params(0, 0)).unwrap();
        let outline = &m.panels[1].outline;
        assert_eq!(outline.len(), 4);
        // edge_len along child's x = 100, flange length along y = 25
        assert!((outline[1].x - 100.0).abs() < 1e-9);
        assert!((outline[2].y - 25.0).abs() < 1e-9);
    }

    #[test]
    fn up_flange_at_90_lifts_above_parent() {
        let mut m = base_flange_rect(100.0, 50.0, 1.0).unwrap();
        let table = default_table();
        // Edge 0: (0,0)→(100,0); outward normal in 2D is (0, -1).
        // After Up bend by π/2 about edge direction (+x), outward (0,-1,0)
        // rotates to (0, 0, -1)? Let's check via the right-hand rule.
        //
        // Actually: rotating (0,-1,0) about +x by +π/2 (right-hand rule)
        // gives (0, 0, -1). So Up = -Z. We just want to verify that the
        // child does *not* lie in the parent plane (z=0).
        let (child_id, _) = add_edge_flange(&mut m, &table, al_soft_params(0, 0)).unwrap();
        let child = &m.panels[child_id];
        // Tip of the flange in child-local: (50, 25).
        let tip = child.frame_bent.to_world(Point2::new(50.0, 25.0));
        // Tip should be off the parent plane (z != 0).
        assert!(
            tip.z.abs() > 1e-6,
            "tip not lifted from parent plane: {tip:?}"
        );
        // Tip's distance from the hinge axis (x-axis) should equal the
        // flange length (25), since the bend is 90°.
        let dist_yz = (tip.y * tip.y + tip.z * tip.z).sqrt();
        assert!((dist_yz - 25.0).abs() < 1e-9, "expected 25.0 got {dist_yz}");
    }

    #[test]
    fn down_flange_mirrors_up() {
        let mut m_up = base_flange_rect(100.0, 50.0, 1.0).unwrap();
        let mut m_dn = base_flange_rect(100.0, 50.0, 1.0).unwrap();
        let table = default_table();
        let mut p_up = al_soft_params(0, 0);
        p_up.direction = BendDirection::Up;
        let mut p_dn = al_soft_params(0, 0);
        p_dn.direction = BendDirection::Down;
        add_edge_flange(&mut m_up, &table, p_up).unwrap();
        add_edge_flange(&mut m_dn, &table, p_dn).unwrap();
        let tip_up = m_up.panels[1].frame_bent.to_world(Point2::new(50.0, 25.0));
        let tip_dn = m_dn.panels[1].frame_bent.to_world(Point2::new(50.0, 25.0));
        // Up and Down should mirror through the parent plane (z=0).
        assert!(
            (tip_up.z + tip_dn.z).abs() < 1e-9,
            "{tip_up:?} vs {tip_dn:?}"
        );
    }

    #[test]
    fn flat_pose_offsets_by_bend_allowance() {
        let mut m = base_flange_rect(100.0, 50.0, 1.0).unwrap();
        let table = default_table();
        let params = al_soft_params(0, 0);
        let (child_id, bend_id) = add_edge_flange(&mut m, &table, params.clone()).unwrap();
        let bend = &m.bends[bend_id];
        let ba = bend.allowance(m.thickness);
        let child = &m.panels[child_id];
        // In flat coords, child origin should sit on outward direction at
        // distance BA from parent origin (parent edge 0 starts at origin).
        // outward_2d for edge 0 of CCW rect is (0, -1).
        let expected = Point3::new(0.0, -ba, 0.0);
        let got = child.frame_flat.origin;
        assert!(
            (got - expected).norm() < 1e-9,
            "expected {expected:?} got {got:?}"
        );
    }

    #[test]
    fn rejects_invalid_inputs() {
        let mut m = base_flange_rect(100.0, 50.0, 1.0).unwrap();
        let table = default_table();

        // Unknown panel
        let mut p = al_soft_params(99, 0);
        p.panel = 99;
        assert!(matches!(
            add_edge_flange(&mut m, &table, p),
            Err(EdgeFlangeError::UnknownPanel(_))
        ));

        // Edge out of range
        assert!(matches!(
            add_edge_flange(&mut m, &table, al_soft_params(0, 99)),
            Err(EdgeFlangeError::EdgeOutOfRange { .. })
        ));

        // Non-positive length
        let mut p = al_soft_params(0, 0);
        p.length = 0.0;
        assert!(matches!(
            add_edge_flange(&mut m, &table, p),
            Err(EdgeFlangeError::NonPositive("length", _))
        ));

        // Angle too large
        let mut p = al_soft_params(0, 0);
        p.angle = 4.0;
        assert!(matches!(
            add_edge_flange(&mut m, &table, p),
            Err(EdgeFlangeError::AngleTooLarge(_))
        ));

        // Unknown material
        let mut p = al_soft_params(0, 0);
        p.material = "Unobtanium".into();
        assert!(matches!(
            add_edge_flange(&mut m, &table, p),
            Err(EdgeFlangeError::NoKFactor { .. })
        ));
    }

    #[test]
    fn manual_k_overrides_table() {
        let mut m = base_flange_rect(100.0, 50.0, 1.0).unwrap();
        let table = default_table();
        let mut p = al_soft_params(0, 0);
        p.manual_k = Some(0.123);
        let (_, bend_id) = add_edge_flange(&mut m, &table, p).unwrap();
        assert!((m.bends[bend_id].k_factor - 0.123).abs() < 1e-12);
        assert_eq!(m.bends[bend_id].k_factor_source.as_deref(), Some("manual"));
    }
}
