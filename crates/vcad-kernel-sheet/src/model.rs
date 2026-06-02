//! Core sheet-metal data structures.
//!
//! A [`SheetMetalModel`] is a graph of [`Panel`]s (flat regions) connected by
//! [`Bend`]s (cylindrical patches). The graph is a tree rooted at the
//! reference panel; cycles will be supported when multi-body welded
//! sheet-metal lands.

use vcad_kernel_math::{Point2, Point3, Vec3};

/// Index handle for a [`Panel`] inside a [`SheetMetalModel`].
pub type PanelId = usize;

/// Index handle for a [`Bend`] inside a [`SheetMetalModel`].
pub type BendId = usize;

/// 3D pose of a panel: an origin and an orthonormal basis.
///
/// `x_dir` and `y_dir` span the panel's mid-plane in 3D. `x_dir × y_dir`
/// points away from the *outside* face (the side that becomes longer when
/// bent).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    /// 3D position of the panel-local origin.
    pub origin: Point3,
    /// Unit vector for panel-local +X in 3D.
    pub x_dir: Vec3,
    /// Unit vector for panel-local +Y in 3D.
    pub y_dir: Vec3,
}

impl Frame {
    /// Identity frame at the world origin with axes aligned to world axes.
    pub fn identity() -> Self {
        Self {
            origin: Point3::origin(),
            x_dir: Vec3::new(1.0, 0.0, 0.0),
            y_dir: Vec3::new(0.0, 1.0, 0.0),
        }
    }

    /// Outward (away from the *inside* face) normal of the panel.
    ///
    /// Equal to `x_dir × y_dir`.
    pub fn normal(&self) -> Vec3 {
        self.x_dir.cross(self.y_dir)
    }

    /// Lift a panel-local 2D point into world 3D coordinates.
    pub fn to_world(&self, p: Point2) -> Point3 {
        Point3::new(
            self.origin.x + self.x_dir.x * p.x + self.y_dir.x * p.y,
            self.origin.y + self.x_dir.y * p.x + self.y_dir.y * p.y,
            self.origin.z + self.x_dir.z * p.x + self.y_dir.z * p.y,
        )
    }
}

/// A flat planar region of the sheet.
///
/// Geometry is stored in a panel-local 2D frame (see [`Frame`]); the same
/// outline is used for both the bent and unfolded views — only the 3D pose
/// changes.
#[derive(Debug, Clone, PartialEq)]
pub struct Panel {
    /// Closed polygon outline in panel-local 2D coords (CCW when viewed from
    /// the outside face). The first and last points are *not* duplicated.
    pub outline: Vec<Point2>,
    /// Holes in the panel (CW when viewed from the outside face).
    pub holes: Vec<Vec<Point2>>,
    /// 3D pose of this panel in the **bent** configuration.
    pub frame_bent: Frame,
    /// 3D pose of this panel in the **unfolded** (flat) configuration.
    /// Computed lazily by [`crate::unfold::unfold`].
    pub frame_flat: Frame,
    /// Bends incident to this panel.
    pub incident_bends: Vec<BendId>,
}

/// Direction of a bend relative to the parent panel.
///
/// `Up` means the child panel rises out of the parent's outside face;
/// `Down` means it descends out of the inside face. This corresponds to the
/// red / blue convention used in flat-pattern DXF layers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum BendDirection {
    /// Child rises out of the parent's outside face.
    Up,
    /// Child descends out of the parent's inside face.
    Down,
}

impl BendDirection {
    /// Sign in `{-1, +1}` matching the convention used by rotation maths.
    pub fn sign(self) -> f64 {
        match self {
            BendDirection::Up => 1.0,
            BendDirection::Down => -1.0,
        }
    }
}

/// A cylindrical bend connecting two panels along a shared edge.
///
/// The bend is defined by its inside radius, angle (always positive), and
/// direction. The hinge edge is stored in *parent-panel-local* 2D coords;
/// the child panel's edge is implicitly the same edge after applying the
/// bend's transformation.
#[derive(Debug, Clone, PartialEq)]
pub struct Bend {
    /// The panel the bend "comes off of" (closer to the model root).
    pub parent: PanelId,
    /// The panel created by the bend (further from the root).
    pub child: PanelId,
    /// Hinge edge in parent-panel-local 2D coords (start, end).
    ///
    /// Right-hand rule: when looking along (end - start) in the parent's
    /// frame, an `Up` bend rotates the child counter-clockwise.
    pub edge_parent: (Point2, Point2),
    /// Inside bend radius (mm).
    pub radius: f64,
    /// Bend angle (radians, always > 0). For a right-angle flange this is
    /// `π/2`.
    pub angle: f64,
    /// Direction the child panel folds relative to the parent.
    pub direction: BendDirection,
    /// K-factor used to compute the bend allowance for this bend.
    /// Carries provenance back to the [`crate::bend_table::BendTable`] row
    /// that produced it.
    pub k_factor: f64,
    /// Optional human-readable provenance tag (e.g. `"Al-1mm/R1.5"` or
    /// `"shop:override"`). Surfaced in the property panel as the colored dot.
    pub k_factor_source: Option<String>,
}

impl Bend {
    /// Bend allowance: arc length of the neutral axis through this bend.
    ///
    /// `BA = θ · (R + K · t)` where `t` is the material thickness.
    pub fn allowance(&self, thickness: f64) -> f64 {
        self.angle * (self.radius + self.k_factor * thickness)
    }

    /// Append a tag to the K-factor source label.
    ///
    /// Operations layered on top of [`crate::add_edge_flange`] (hems,
    /// jogs) use this to mark their bends so the UI / DXF / agent can
    /// label them as `hem`, `jog`, etc. rather than as generic flanges.
    /// A bend with no prior source is treated as `"manual"`.
    pub fn append_source_tag(&mut self, suffix: &str) {
        let base = self
            .k_factor_source
            .clone()
            .unwrap_or_else(|| "manual".to_string());
        self.k_factor_source = Some(format!("{base}{suffix}"));
    }
}

/// A complete sheet-metal model.
///
/// Owns all panels and bends, plus the material thickness that's constant
/// across the part. The root panel is the reference: it stays at its
/// `frame_bent` pose during refold and at its `frame_flat` pose during
/// unfold (both default to identity for a freshly created model).
#[derive(Debug, Clone, PartialEq)]
pub struct SheetMetalModel {
    /// Material thickness (mm). Constant across the part.
    pub thickness: f64,
    /// Material name (key into [`crate::materials`] registry, e.g.
    /// `"al-soft"`, `"steel-mild"`). Empty string means "unspecified" —
    /// callers should treat that as an unknown alloy.
    pub material: String,
    /// All panels in the model. Index by [`PanelId`].
    pub panels: Vec<Panel>,
    /// All bends in the model. Index by [`BendId`].
    pub bends: Vec<Bend>,
    /// The reference panel — stays put during unfold/refold.
    pub root: PanelId,
}

impl SheetMetalModel {
    /// Construct an empty model with the given thickness. Useful as a starting
    /// point for tests; production code should go through
    /// [`crate::base_flange::base_flange_rect`] etc.
    pub fn new(thickness: f64) -> Self {
        Self {
            thickness,
            material: String::new(),
            panels: Vec::new(),
            bends: Vec::new(),
            root: 0,
        }
    }

    /// Material properties for this model's alloy, looked up via
    /// [`crate::materials::lookup_or_unknown`]. `None` when the model
    /// was created without specifying a material — callers should treat
    /// that as "defer to the shop / fall back to neutral defaults"
    /// rather than picking an arbitrary alloy.
    pub fn material_properties(&self) -> Option<crate::materials::MaterialProperties> {
        if self.material.is_empty() {
            None
        } else {
            Some(crate::materials::lookup_or_unknown(&self.material))
        }
    }

    /// Estimated springback per radian for this model's material. Zero
    /// when no material is set, so callers can multiply by an angle
    /// without branching.
    pub fn springback_per_radian(&self) -> f64 {
        self.material_properties()
            .map_or(0.0, |m| m.springback_per_radian)
    }

    /// Append a panel and return its [`PanelId`].
    pub fn push_panel(&mut self, panel: Panel) -> PanelId {
        let id = self.panels.len();
        self.panels.push(panel);
        id
    }

    /// Append a bend and update both incident panels' adjacency lists.
    pub fn push_bend(&mut self, bend: Bend) -> BendId {
        let id = self.bends.len();
        let parent = bend.parent;
        let child = bend.child;
        self.bends.push(bend);
        self.panels[parent].incident_bends.push(id);
        self.panels[child].incident_bends.push(id);
        id
    }

    /// Walk the panel/bend graph from the root, yielding each `(panel_id,
    /// parent_bend_id_or_none)` in BFS order.
    ///
    /// The first yielded item is `(root, None)`. Used by [`crate::unfold`] to
    /// propagate the unfolded frame outward from the reference panel.
    pub fn bfs(&self) -> impl Iterator<Item = (PanelId, Option<BendId>)> + '_ {
        BfsIter::new(self)
    }
}

struct BfsIter<'a> {
    model: &'a SheetMetalModel,
    visited: Vec<bool>,
    queue: std::collections::VecDeque<(PanelId, Option<BendId>)>,
}

impl<'a> BfsIter<'a> {
    fn new(model: &'a SheetMetalModel) -> Self {
        let mut visited = vec![false; model.panels.len()];
        let mut queue = std::collections::VecDeque::new();
        if !model.panels.is_empty() {
            visited[model.root] = true;
            queue.push_back((model.root, None));
        }
        Self {
            model,
            visited,
            queue,
        }
    }
}

impl Iterator for BfsIter<'_> {
    type Item = (PanelId, Option<BendId>);

    fn next(&mut self) -> Option<Self::Item> {
        let (panel, via) = self.queue.pop_front()?;
        for &bend_id in &self.model.panels[panel].incident_bends {
            let bend = &self.model.bends[bend_id];
            let other = if bend.parent == panel {
                bend.child
            } else {
                bend.parent
            };
            if !self.visited[other] {
                self.visited[other] = true;
                self.queue.push_back((other, Some(bend_id)));
            }
        }
        Some((panel, via))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_identity_lifts_2d_to_3d() {
        let f = Frame::identity();
        assert_eq!(
            f.to_world(Point2::new(2.0, 3.0)),
            Point3::new(2.0, 3.0, 0.0)
        );
    }

    #[test]
    fn frame_normal_is_x_cross_y() {
        let f = Frame::identity();
        let n = f.normal();
        assert!((n - Vec3::new(0.0, 0.0, 1.0)).norm() < 1e-12);
    }

    #[test]
    fn bend_allowance_matches_formula() {
        let bend = Bend {
            parent: 0,
            child: 1,
            edge_parent: (Point2::new(0.0, 0.0), Point2::new(10.0, 0.0)),
            radius: 1.0,
            angle: std::f64::consts::FRAC_PI_2,
            direction: BendDirection::Up,
            k_factor: 0.42,
            k_factor_source: None,
        };
        // BA = (π/2) · (1.0 + 0.42·1.0) = (π/2) · 1.42
        let ba = bend.allowance(1.0);
        let expected = std::f64::consts::FRAC_PI_2 * 1.42;
        assert!((ba - expected).abs() < 1e-12);
    }

    #[test]
    fn bfs_visits_all_panels_in_a_tree() {
        let mut m = SheetMetalModel::new(1.0);
        let mk_panel = || Panel {
            outline: vec![],
            holes: vec![],
            frame_bent: Frame::identity(),
            frame_flat: Frame::identity(),
            incident_bends: vec![],
        };
        let p0 = m.push_panel(mk_panel());
        let p1 = m.push_panel(mk_panel());
        let p2 = m.push_panel(mk_panel());
        m.root = p0;
        m.push_bend(Bend {
            parent: p0,
            child: p1,
            edge_parent: (Point2::new(0.0, 0.0), Point2::new(1.0, 0.0)),
            radius: 1.0,
            angle: std::f64::consts::FRAC_PI_2,
            direction: BendDirection::Up,
            k_factor: 0.42,
            k_factor_source: None,
        });
        m.push_bend(Bend {
            parent: p1,
            child: p2,
            edge_parent: (Point2::new(0.0, 0.0), Point2::new(1.0, 0.0)),
            radius: 1.0,
            angle: std::f64::consts::FRAC_PI_2,
            direction: BendDirection::Up,
            k_factor: 0.42,
            k_factor_source: None,
        });
        let visited: Vec<_> = m.bfs().map(|(p, _)| p).collect();
        assert_eq!(visited, vec![p0, p1, p2]);
    }
}
