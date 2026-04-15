//! Managed sketch editing session.
//!
//! [`SketchSession`] wraps [`Sketch2D`] with the UI state needed to drive a
//! sketch editor: the active drawing tool, pending input points, snapping,
//! hit-testing, selection, undo/redo, and exit-status bookkeeping.
//!
//! This is platform-independent Rust — it is reused by the web app (via
//! `vcad-kernel-wasm`) and the TUI (via `vcad-cli::tui::sketch_mode`) so both
//! frontends get the same behavior and the same Levenberg-Marquardt
//! constraint solver.

use crate::constraint::Constraint;
#[cfg(test)]
use crate::constraint::EntityRef;
use crate::entity::{EntityId, SketchEntity};
use crate::export::ExportError;
use crate::sketch::Sketch2D;
use crate::solver::SolveResult;
use vcad_kernel_math::{Dir3, Point3, Vec3};
use vcad_kernel_sketch::SketchProfile;

/// Sketch plane orientation.
///
/// Axis-aligned planes use canonical basis vectors; `Custom` carries its own
/// origin + basis (which must be orthonormal).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum SketchPlane {
    /// XY plane at origin (Z-up normal).
    #[default]
    XY,
    /// XZ plane at origin (Y normal, Y axis flipped for right-handed basis).
    XZ,
    /// YZ plane at origin (X normal).
    YZ,
    /// Arbitrary plane defined by origin and orthonormal x/y basis vectors.
    Custom {
        /// Plane origin in world coordinates.
        origin: [f64; 3],
        /// Local X axis in world coordinates. Must be a unit vector.
        x_dir: [f64; 3],
        /// Local Y axis in world coordinates. Must be a unit vector.
        y_dir: [f64; 3],
    },
}

impl SketchPlane {
    /// Build a custom plane from an origin and normal. The in-plane basis is
    /// derived deterministically so the same normal always produces the same
    /// axes.
    pub fn custom_from_normal(origin: [f64; 3], normal: [f64; 3]) -> Self {
        let n = normalize(normal);
        // Pick an up vector that isn't parallel to the normal, then
        // Gram-Schmidt.
        let up = if n[2].abs() < 0.9 {
            [0.0, 0.0, 1.0]
        } else {
            [1.0, 0.0, 0.0]
        };
        let x = normalize(cross(up, n));
        let y = cross(n, x);
        SketchPlane::Custom {
            origin,
            x_dir: x,
            y_dir: y,
        }
    }

    /// Plane origin in world coordinates.
    pub fn origin(&self) -> [f64; 3] {
        match self {
            SketchPlane::XY | SketchPlane::XZ | SketchPlane::YZ => [0.0, 0.0, 0.0],
            SketchPlane::Custom { origin, .. } => *origin,
        }
    }

    /// In-plane local X axis in world coordinates (unit vector).
    pub fn x_dir(&self) -> [f64; 3] {
        match self {
            SketchPlane::XY => [1.0, 0.0, 0.0],
            SketchPlane::XZ => [1.0, 0.0, 0.0],
            SketchPlane::YZ => [0.0, 1.0, 0.0],
            SketchPlane::Custom { x_dir, .. } => *x_dir,
        }
    }

    /// In-plane local Y axis in world coordinates (unit vector).
    pub fn y_dir(&self) -> [f64; 3] {
        match self {
            SketchPlane::XY => [0.0, 1.0, 0.0],
            SketchPlane::XZ => [0.0, 0.0, 1.0],
            SketchPlane::YZ => [0.0, 0.0, 1.0],
            SketchPlane::Custom { y_dir, .. } => *y_dir,
        }
    }

    /// Plane normal (unit vector), equal to `x_dir × y_dir`.
    pub fn normal(&self) -> [f64; 3] {
        cross(self.x_dir(), self.y_dir())
    }

    /// Convert a 3D world point to local 2D sketch coordinates via orthogonal
    /// projection onto the plane.
    pub fn world_to_sketch(&self, world: [f64; 3]) -> [f64; 2] {
        let o = self.origin();
        let d = [world[0] - o[0], world[1] - o[1], world[2] - o[2]];
        let x = self.x_dir();
        let y = self.y_dir();
        [dot(d, x), dot(d, y)]
    }

    /// Convert local 2D sketch coordinates to a 3D world point.
    pub fn sketch_to_world(&self, x: f64, y: f64) -> [f64; 3] {
        let o = self.origin();
        let xd = self.x_dir();
        let yd = self.y_dir();
        [
            o[0] + x * xd[0] + y * yd[0],
            o[1] + x * xd[1] + y * yd[1],
            o[2] + x * xd[2] + y * yd[2],
        ]
    }

    /// Intersect a world-space ray with the plane and return the intersection
    /// in local 2D sketch coordinates. Returns `None` if the ray is parallel
    /// to the plane or points away from it.
    pub fn intersect_ray(&self, ray_origin: [f64; 3], ray_dir: [f64; 3]) -> Option<[f64; 2]> {
        let n = self.normal();
        let denom = dot(ray_dir, n);
        if denom.abs() < 1e-12 {
            return None;
        }
        let o = self.origin();
        let diff = [o[0] - ray_origin[0], o[1] - ray_origin[1], o[2] - ray_origin[2]];
        let t = dot(diff, n) / denom;
        if !t.is_finite() {
            return None;
        }
        let hit = [
            ray_origin[0] + t * ray_dir[0],
            ray_origin[1] + t * ray_dir[1],
            ray_origin[2] + t * ray_dir[2],
        ];
        Some(self.world_to_sketch(hit))
    }

    /// Apply this plane's basis to a fresh [`Sketch2D`].
    pub fn apply_to(&self, sketch: &mut Sketch2D) {
        let o = self.origin();
        let x = self.x_dir();
        let y = self.y_dir();
        sketch.origin = Point3::new(o[0], o[1], o[2]);
        sketch.x_dir = Dir3::new_normalize(Vec3::new(x[0], x[1], x[2]));
        sketch.y_dir = Dir3::new_normalize(Vec3::new(y[0], y[1], y[2]));
    }
}

/// Active drawing tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SketchTool {
    /// Pick/selection tool.
    #[default]
    Select,
    /// Polyline — each click adds a line segment to the previous point.
    Line,
    /// Rectangle from two opposite corners.
    Rectangle,
    /// Circle from center + edge point.
    Circle,
    /// Three-point arc (start, end, center).
    Arc,
    /// Standalone point.
    Point,
}

/// Snap configuration.
#[derive(Debug, Clone, Copy)]
pub struct SnapConfig {
    /// Snap the cursor to grid points.
    pub grid_enabled: bool,
    /// Grid spacing in sketch units.
    pub grid_size: f64,
    /// Snap the cursor to existing sketch vertices.
    pub point_enabled: bool,
    /// Radius (in sketch units) within which a vertex snaps the cursor.
    pub point_tolerance: f64,
}

impl Default for SnapConfig {
    fn default() -> Self {
        Self {
            grid_enabled: true,
            grid_size: 1.0,
            point_enabled: true,
            point_tolerance: 5.0,
        }
    }
}

/// Current cursor state, reported by a frontend via [`SketchSession::on_cursor_sketch`]
/// or [`SketchSession::on_cursor_ray`].
#[derive(Debug, Clone, Copy)]
pub struct Cursor {
    /// Cursor position in sketch coordinates after snapping.
    pub x: f64,
    /// Cursor position in sketch coordinates after snapping.
    pub y: f64,
    /// If the cursor snapped to an existing vertex, its coordinates.
    pub snap_target: Option<[f64; 2]>,
}

/// Result of [`SketchSession::on_click`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClickOutcome {
    /// Cursor had no sketch position — nothing happened.
    NoCursor,
    /// The click was absorbed by selection (Select tool).
    Selection,
    /// The click recorded a pending point but the shape isn't complete yet.
    Pending,
    /// The click completed a shape and one or more entities were added.
    Committed,
}

/// Exit status reported from [`SketchSession::exit_status`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitStatus {
    /// The sketch is empty — nothing to keep.
    Empty,
    /// The sketch has at least one segment.
    HasSegments,
}

/// Constraint fullness classification used for UI feedback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstraintStatus {
    /// DOF > 0 — more constraints can still be added.
    Under,
    /// DOF = 0 and the solver converged.
    Solved,
    /// DOF < 0 — sketch is over-constrained.
    Over,
    /// The most recent solve attempt failed to converge.
    Error,
}

/// A flat, serializable view of one drawable sketch entity (line, arc, or
/// circle). Returned by [`SketchSession::segments`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SegmentView {
    /// Position of this segment in the session's segment order. Frontends
    /// use this index to refer to segments in constraints and selections.
    pub index: usize,
    /// Start point in sketch coordinates. For a full circle, same as `end`.
    pub start: [f64; 2],
    /// End point in sketch coordinates. For a full circle, same as `start`.
    pub end: [f64; 2],
    /// Kind-specific data.
    pub kind: SegmentKind,
}

/// Flavor of a [`SegmentView`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SegmentKind {
    /// Straight line between two points.
    Line,
    /// Circular arc with a specific center and winding direction.
    Arc {
        /// Arc center in sketch coordinates.
        center: [f64; 2],
        /// Counter-clockwise from start to end if true.
        ccw: bool,
    },
    /// Full circle. `center` is the center; `radius` is explicit.
    Circle {
        /// Circle center in sketch coordinates.
        center: [f64; 2],
        /// Radius in sketch units.
        radius: f64,
    },
}

/// A snapshot captured for undo/redo.
#[derive(Debug, Clone)]
struct Snapshot {
    sketch: Sketch2D,
    segment_order: Vec<EntityId>,
    pending: Vec<[f64; 2]>,
    selection: Vec<usize>,
    tool: SketchTool,
    solved: bool,
}

/// Maximum number of undo entries kept.
const MAX_HISTORY: usize = 100;

/// Managed sketch editing session.
///
/// See the module-level docs for the motivation and the overall state
/// machine.
#[derive(Debug, Clone)]
pub struct SketchSession {
    plane: SketchPlane,
    sketch: Sketch2D,
    /// Line/arc/circle entity IDs in insertion order. Frontends refer to
    /// segments by their position in this list.
    segment_order: Vec<EntityId>,
    tool: SketchTool,
    pending: Vec<[f64; 2]>,
    selection: Vec<usize>,
    snap: SnapConfig,
    cursor: Option<Cursor>,
    history: Vec<Snapshot>,
    future: Vec<Snapshot>,
    /// `true` if the sketch is currently consistent with its constraints.
    /// Set to `false` when a constraint is added/removed and cleared by
    /// `solve()`.
    solved: bool,
    /// Whether the most recent solve attempt converged.
    last_converged: bool,
}

impl SketchSession {
    /// Create a new, empty session on the given plane.
    pub fn new(plane: SketchPlane) -> Self {
        let mut sketch = Sketch2D::new();
        plane.apply_to(&mut sketch);
        Self {
            plane,
            sketch,
            segment_order: Vec::new(),
            tool: SketchTool::default(),
            pending: Vec::new(),
            selection: Vec::new(),
            snap: SnapConfig::default(),
            cursor: None,
            history: Vec::new(),
            future: Vec::new(),
            solved: true,
            last_converged: true,
        }
    }

    // -------------------------------------------------------------------------
    // Accessors
    // -------------------------------------------------------------------------

    /// The plane this session edits on.
    pub fn plane(&self) -> SketchPlane {
        self.plane
    }
    /// Read-only access to the underlying constraint-aware sketch.
    pub fn sketch(&self) -> &Sketch2D {
        &self.sketch
    }
    /// The currently-active drawing tool.
    pub fn tool(&self) -> SketchTool {
        self.tool
    }
    /// Pending input points accumulated for the active tool.
    pub fn pending_points(&self) -> &[[f64; 2]] {
        &self.pending
    }
    /// Selected segment indices.
    pub fn selection(&self) -> &[usize] {
        &self.selection
    }
    /// Current snap configuration.
    pub fn snap_config(&self) -> &SnapConfig {
        &self.snap
    }
    /// Mutable snap configuration.
    pub fn snap_config_mut(&mut self) -> &mut SnapConfig {
        &mut self.snap
    }
    /// Latest cursor state, if any.
    pub fn cursor(&self) -> Option<Cursor> {
        self.cursor
    }
    /// Number of line/arc/circle segments in the sketch.
    pub fn segment_count(&self) -> usize {
        self.segment_order.len()
    }
    /// Number of constraints currently in the sketch.
    pub fn constraint_count(&self) -> usize {
        self.sketch.constraints.len()
    }
    /// Degrees of freedom of the underlying sketch.
    pub fn degrees_of_freedom(&self) -> i32 {
        self.sketch.degrees_of_freedom()
    }

    // -------------------------------------------------------------------------
    // Tool state
    // -------------------------------------------------------------------------

    /// Switch to a different tool. Clears any pending input.
    pub fn set_tool(&mut self, tool: SketchTool) {
        self.tool = tool;
        self.pending.clear();
    }

    /// Clear pending input (e.g. in response to `Esc`).
    pub fn cancel_pending(&mut self) {
        self.pending.clear();
    }

    // -------------------------------------------------------------------------
    // Cursor & snapping
    // -------------------------------------------------------------------------

    /// Update the cursor from a world-space ray (e.g. camera pick ray).
    pub fn on_cursor_ray(&mut self, ray_origin: [f64; 3], ray_dir: [f64; 3]) {
        match self.plane.intersect_ray(ray_origin, ray_dir) {
            Some([x, y]) => self.on_cursor_sketch(x, y),
            None => self.on_cursor_leave(),
        }
    }

    /// Update the cursor directly from 2D sketch coordinates.
    pub fn on_cursor_sketch(&mut self, x: f64, y: f64) {
        let (snapped_x, snapped_y, target) = self.apply_snap(x, y);
        self.cursor = Some(Cursor {
            x: snapped_x,
            y: snapped_y,
            snap_target: target,
        });
    }

    /// Clear the cursor (pointer left the plane).
    pub fn on_cursor_leave(&mut self) {
        self.cursor = None;
    }

    fn apply_snap(&self, x: f64, y: f64) -> (f64, f64, Option<[f64; 2]>) {
        // Priority 1: vertex snap.
        if self.snap.point_enabled {
            let tol = self.snap.point_tolerance;
            let tol2 = tol * tol;
            for v in self.vertices() {
                let dx = x - v[0];
                let dy = y - v[1];
                if dx * dx + dy * dy < tol2 {
                    return (v[0], v[1], Some(v));
                }
            }
        }
        // Priority 2: grid snap.
        if self.snap.grid_enabled && self.snap.grid_size > 0.0 {
            let g = self.snap.grid_size;
            return ((x / g).round() * g, (y / g).round() * g, None);
        }
        (x, y, None)
    }

    // -------------------------------------------------------------------------
    // Click handling — tool state machine
    // -------------------------------------------------------------------------

    /// Handle a primary-button click at the current cursor position. Drives
    /// the tool state machine.
    pub fn on_click(&mut self) -> ClickOutcome {
        let Some(cursor) = self.cursor else {
            return ClickOutcome::NoCursor;
        };
        let p = [cursor.x, cursor.y];
        match self.tool {
            SketchTool::Select => {
                if let Some(idx) = self.hit_test(p[0], p[1], 2.0) {
                    self.toggle_selection(idx);
                } else {
                    self.selection.clear();
                }
                ClickOutcome::Selection
            }
            SketchTool::Line => {
                if self.pending.is_empty() {
                    self.pending.push(p);
                    ClickOutcome::Pending
                } else {
                    let start = *self.pending.last().unwrap();
                    self.push_history();
                    self.add_line(start, p);
                    self.pending = vec![p];
                    ClickOutcome::Committed
                }
            }
            SketchTool::Rectangle => {
                if self.pending.is_empty() {
                    self.pending.push(p);
                    ClickOutcome::Pending
                } else {
                    let start = self.pending[0];
                    self.push_history();
                    self.add_rectangle(start, p);
                    self.pending.clear();
                    ClickOutcome::Committed
                }
            }
            SketchTool::Circle => {
                if self.pending.is_empty() {
                    self.pending.push(p);
                    ClickOutcome::Pending
                } else {
                    let center = self.pending[0];
                    let radius = distance(center, p);
                    self.pending.clear();
                    if radius > 1e-6 {
                        self.push_history();
                        self.add_circle(center, radius);
                        ClickOutcome::Committed
                    } else {
                        ClickOutcome::Pending
                    }
                }
            }
            SketchTool::Arc => {
                if self.pending.len() < 2 {
                    self.pending.push(p);
                    ClickOutcome::Pending
                } else {
                    let start = self.pending[0];
                    let end = self.pending[1];
                    let center = p;
                    self.pending.clear();
                    self.push_history();
                    self.add_arc(start, end, center, true);
                    ClickOutcome::Committed
                }
            }
            SketchTool::Point => {
                self.push_history();
                self.sketch.add_point(p[0], p[1]);
                ClickOutcome::Committed
            }
        }
    }

    /// Handle a double-click. For the line tool, closes the current polyline.
    pub fn on_double_click(&mut self) {
        if self.tool == SketchTool::Line && self.pending.len() == 1 && self.segment_count() > 0 {
            // Close the polyline by connecting the current pending point to
            // the start of the first line segment in the current run.
            if let Some(first) = self.first_run_start() {
                let current = self.pending[0];
                self.push_history();
                self.add_line(current, first);
            }
        }
        self.pending.clear();
    }

    /// Find the start point of the earliest contiguous line run terminating
    /// at the pending point. Best-effort: returns the first line's start.
    fn first_run_start(&self) -> Option<[f64; 2]> {
        let id = *self.segment_order.first()?;
        if let Some(SketchEntity::Line(l)) = self.sketch.entities.get(id) {
            let (x, y) = self.sketch.get_point(l.start)?;
            Some([x, y])
        } else {
            None
        }
    }

    // -------------------------------------------------------------------------
    // Direct entity construction
    // -------------------------------------------------------------------------

    /// Add a single line segment and return its segment index.
    pub fn add_line(&mut self, a: [f64; 2], b: [f64; 2]) -> usize {
        let pa = self.sketch.add_point(a[0], a[1]);
        let pb = self.sketch.add_point(b[0], b[1]);
        let line = self.sketch.add_line(pa, pb);
        self.segment_order.push(line);
        self.solved = true; // no constraints changed
        self.segment_order.len() - 1
    }

    /// Add four line segments forming an axis-aligned rectangle with opposite
    /// corners `p1` and `p2`. Returns the four segment indices.
    pub fn add_rectangle(&mut self, p1: [f64; 2], p2: [f64; 2]) -> [usize; 4] {
        let min_x = p1[0].min(p2[0]);
        let max_x = p1[0].max(p2[0]);
        let min_y = p1[1].min(p2[1]);
        let max_y = p1[1].max(p2[1]);
        let c0 = [min_x, min_y];
        let c1 = [max_x, min_y];
        let c2 = [max_x, max_y];
        let c3 = [min_x, max_y];
        let s0 = self.add_line(c0, c1);
        let s1 = self.add_line(c1, c2);
        let s2 = self.add_line(c2, c3);
        let s3 = self.add_line(c3, c0);
        [s0, s1, s2, s3]
    }

    /// Add a circle entity. Returns its segment index.
    pub fn add_circle(&mut self, center: [f64; 2], radius: f64) -> usize {
        let c = self.sketch.add_point(center[0], center[1]);
        let circle = self.sketch.add_circle(c, radius);
        self.segment_order.push(circle);
        self.solved = true;
        self.segment_order.len() - 1
    }

    /// Add an arc defined by start, end, and center (with explicit winding).
    /// Returns its segment index.
    pub fn add_arc(&mut self, start: [f64; 2], end: [f64; 2], center: [f64; 2], ccw: bool) -> usize {
        let s = self.sketch.add_point(start[0], start[1]);
        let e = self.sketch.add_point(end[0], end[1]);
        let c = self.sketch.add_point(center[0], center[1]);
        let arc = self.sketch.add_arc(s, e, c, ccw);
        self.segment_order.push(arc);
        self.solved = true;
        self.segment_order.len() - 1
    }

    /// Clear the sketch entirely.
    pub fn clear(&mut self) {
        if self.sketch.entities.is_empty() && self.sketch.constraints.is_empty() {
            return;
        }
        self.push_history();
        let mut fresh = Sketch2D::new();
        self.plane.apply_to(&mut fresh);
        self.sketch = fresh;
        self.segment_order.clear();
        self.pending.clear();
        self.selection.clear();
        self.solved = true;
    }

    // -------------------------------------------------------------------------
    // Hit testing & vertices
    // -------------------------------------------------------------------------

    /// Return every unique point position in the sketch (deduped by
    /// proximity). Used for vertex snapping and for rendering.
    pub fn vertices(&self) -> Vec<[f64; 2]> {
        let mut pts = Vec::new();
        for (_, entity) in self.sketch.entities.iter() {
            if let SketchEntity::Point(p) = entity {
                let x = self.sketch.parameters[p.param_x];
                let y = self.sketch.parameters[p.param_y];
                if !pts.iter().any(|q: &[f64; 2]| {
                    (q[0] - x).abs() < 0.01 && (q[1] - y).abs() < 0.01
                }) {
                    pts.push([x, y]);
                }
            }
        }
        pts
    }

    /// Return every drawable segment (line, arc, circle) in insertion order.
    pub fn segments(&self) -> Vec<SegmentView> {
        self.segment_order
            .iter()
            .enumerate()
            .filter_map(|(index, id)| self.segment_view(index, *id))
            .collect()
    }

    fn segment_view(&self, index: usize, id: EntityId) -> Option<SegmentView> {
        match self.sketch.entities.get(id)? {
            SketchEntity::Line(l) => {
                let a = self.sketch.get_point(l.start)?;
                let b = self.sketch.get_point(l.end)?;
                Some(SegmentView {
                    index,
                    start: [a.0, a.1],
                    end: [b.0, b.1],
                    kind: SegmentKind::Line,
                })
            }
            SketchEntity::Arc(a) => {
                let s = self.sketch.get_point(a.start)?;
                let e = self.sketch.get_point(a.end)?;
                let c = self.sketch.get_point(a.center)?;
                Some(SegmentView {
                    index,
                    start: [s.0, s.1],
                    end: [e.0, e.1],
                    kind: SegmentKind::Arc {
                        center: [c.0, c.1],
                        ccw: a.ccw,
                    },
                })
            }
            SketchEntity::Circle(c) => {
                let ctr = self.sketch.get_point(c.center)?;
                let radius = self.sketch.parameters[c.param_radius];
                let start = [ctr.0 + radius, ctr.1];
                Some(SegmentView {
                    index,
                    start,
                    end: start,
                    kind: SegmentKind::Circle {
                        center: [ctr.0, ctr.1],
                        radius,
                    },
                })
            }
            SketchEntity::Point(_) => None,
        }
    }

    /// Find the index of the segment closest to `(x, y)` within `tolerance`,
    /// or `None` if nothing is close enough.
    pub fn hit_test(&self, x: f64, y: f64, tolerance: f64) -> Option<usize> {
        let mut best: Option<(usize, f64)> = None;
        for view in self.segments() {
            let d = match view.kind {
                SegmentKind::Line => point_segment_distance([x, y], view.start, view.end),
                SegmentKind::Arc { center, .. } => {
                    let r = distance(center, view.start);
                    (distance([x, y], center) - r).abs()
                }
                SegmentKind::Circle { center, radius } => {
                    (distance([x, y], center) - radius).abs()
                }
            };
            if d < tolerance && best.is_none_or(|(_, bd)| d < bd) {
                best = Some((view.index, d));
            }
        }
        best.map(|(i, _)| i)
    }

    // -------------------------------------------------------------------------
    // Selection
    // -------------------------------------------------------------------------

    /// Toggle a segment's membership in the current selection.
    pub fn toggle_selection(&mut self, segment_index: usize) {
        if segment_index >= self.segment_order.len() {
            return;
        }
        if let Some(pos) = self.selection.iter().position(|&i| i == segment_index) {
            self.selection.remove(pos);
        } else {
            self.selection.push(segment_index);
        }
    }

    /// Clear the current selection.
    pub fn clear_selection(&mut self) {
        self.selection.clear();
    }

    // -------------------------------------------------------------------------
    // Constraints
    // -------------------------------------------------------------------------

    /// Resolve a segment index to the EntityId for the line/arc/circle at
    /// that slot.
    pub fn segment_entity(&self, segment_index: usize) -> Option<EntityId> {
        self.segment_order.get(segment_index).copied()
    }

    /// Push a new constraint onto the sketch, marking it unsolved.
    pub fn add_constraint(&mut self, constraint: Constraint) {
        self.push_history();
        self.sketch.add_constraint(constraint);
        self.solved = false;
    }

    /// Remove the constraint at `index`. No-op if the index is out of bounds.
    pub fn remove_constraint(&mut self, index: usize) {
        if index >= self.sketch.constraints.len() {
            return;
        }
        self.push_history();
        self.sketch.constraints.remove(index);
        self.solved = false;
    }

    /// Run the LM solver. Updates internal `solved` / `last_converged` flags.
    pub fn solve(&mut self) -> SolveResult {
        let result = self.sketch.solve_default();
        self.last_converged = result.converged;
        self.solved = result.converged;
        result
    }

    /// Coarse status used for UI feedback.
    pub fn constraint_status(&self) -> ConstraintStatus {
        let dof = self.sketch.degrees_of_freedom();
        if self.sketch.constraints.is_empty() && self.segment_order.is_empty() {
            return ConstraintStatus::Under;
        }
        if dof > 0 {
            ConstraintStatus::Under
        } else if dof < 0 {
            ConstraintStatus::Over
        } else if self.solved {
            ConstraintStatus::Solved
        } else {
            ConstraintStatus::Error
        }
    }

    // -------------------------------------------------------------------------
    // Undo / redo
    // -------------------------------------------------------------------------

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            sketch: self.sketch.clone(),
            segment_order: self.segment_order.clone(),
            pending: self.pending.clone(),
            selection: self.selection.clone(),
            tool: self.tool,
            solved: self.solved,
        }
    }

    fn restore(&mut self, s: Snapshot) {
        self.sketch = s.sketch;
        self.segment_order = s.segment_order;
        self.pending = s.pending;
        self.selection = s.selection;
        self.tool = s.tool;
        self.solved = s.solved;
    }

    fn push_history(&mut self) {
        let snap = self.snapshot();
        self.history.push(snap);
        if self.history.len() > MAX_HISTORY {
            self.history.remove(0);
        }
        self.future.clear();
    }

    /// Undo the last mutation, returning true if something was undone.
    pub fn undo(&mut self) -> bool {
        let Some(prev) = self.history.pop() else {
            return false;
        };
        let current = self.snapshot();
        self.future.push(current);
        self.restore(prev);
        true
    }

    /// Redo the last undone mutation, returning true if something was redone.
    pub fn redo(&mut self) -> bool {
        let Some(next) = self.future.pop() else {
            return false;
        };
        let current = self.snapshot();
        self.history.push(current);
        self.restore(next);
        true
    }

    // -------------------------------------------------------------------------
    // Exit / export
    // -------------------------------------------------------------------------

    /// What exit mode is appropriate for the current state.
    pub fn exit_status(&self) -> ExitStatus {
        if self.segment_order.is_empty() {
            ExitStatus::Empty
        } else {
            ExitStatus::HasSegments
        }
    }

    /// Export the underlying sketch to a [`SketchProfile`] suitable for
    /// extrude/revolve.
    pub fn to_profile(&self) -> Result<SketchProfile, ExportError> {
        self.sketch.to_profile()
    }
}

// -----------------------------------------------------------------------------
// Vector helpers
// -----------------------------------------------------------------------------

fn normalize(v: [f64; 3]) -> [f64; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len < 1e-12 {
        [0.0, 0.0, 1.0]
    } else {
        [v[0] / len, v[1] / len, v[2] / len]
    }
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn distance(a: [f64; 2], b: [f64; 2]) -> f64 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    (dx * dx + dy * dy).sqrt()
}

fn point_segment_distance(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let dx = b[0] - a[0];
    let dy = b[1] - a[1];
    let len2 = dx * dx + dy * dy;
    if len2 < 1e-8 {
        return distance(p, a);
    }
    let t = (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len2).clamp(0.0, 1.0);
    let q = [a[0] + t * dx, a[1] + t * dy];
    distance(p, q)
}

// -----------------------------------------------------------------------------
// Test-only helpers
// -----------------------------------------------------------------------------

#[cfg(test)]
impl SketchSession {
    /// Return the `EntityRef`s for a line entity's endpoints. Used by tests
    /// that want to assert constraints against specific points.
    pub(crate) fn line_endpoints_refs(
        &self,
        line: EntityId,
    ) -> Option<(EntityRef, EntityRef)> {
        let SketchEntity::Line(l) = self.sketch.entities.get(line)? else {
            return None;
        };
        Some((EntityRef::Point(l.start), EntityRef::Point(l.end)))
    }
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constraint::Constraint;

    fn session() -> SketchSession {
        SketchSession::new(SketchPlane::XY)
    }

    #[test]
    fn plane_basis_xy() {
        let p = SketchPlane::XY;
        assert_eq!(p.x_dir(), [1.0, 0.0, 0.0]);
        assert_eq!(p.y_dir(), [0.0, 1.0, 0.0]);
        assert_eq!(p.normal(), [0.0, 0.0, 1.0]);
    }

    #[test]
    fn plane_roundtrip_projection() {
        let p = SketchPlane::custom_from_normal([5.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        let w = p.sketch_to_world(3.0, 4.0);
        let s = p.world_to_sketch(w);
        assert!((s[0] - 3.0).abs() < 1e-9);
        assert!((s[1] - 4.0).abs() < 1e-9);
    }

    #[test]
    fn ray_intersect_xy_plane() {
        let p = SketchPlane::XY;
        let hit = p.intersect_ray([2.0, 3.0, 5.0], [0.0, 0.0, -1.0]);
        assert_eq!(hit, Some([2.0, 3.0]));
    }

    #[test]
    fn ray_parallel_to_plane() {
        let p = SketchPlane::XY;
        assert!(p.intersect_ray([0.0, 0.0, 5.0], [1.0, 0.0, 0.0]).is_none());
    }

    #[test]
    fn rectangle_tool_commits_on_second_click() {
        let mut s = session();
        s.set_tool(SketchTool::Rectangle);
        s.on_cursor_sketch(0.0, 0.0);
        assert_eq!(s.on_click(), ClickOutcome::Pending);
        s.on_cursor_sketch(10.0, 5.0);
        assert_eq!(s.on_click(), ClickOutcome::Committed);
        assert_eq!(s.segment_count(), 4);
    }

    #[test]
    fn line_tool_chains() {
        let mut s = session();
        s.set_tool(SketchTool::Line);
        s.on_cursor_sketch(0.0, 0.0);
        s.on_click();
        s.on_cursor_sketch(5.0, 0.0);
        assert_eq!(s.on_click(), ClickOutcome::Committed);
        s.on_cursor_sketch(5.0, 5.0);
        s.on_click();
        assert_eq!(s.segment_count(), 2);
    }

    #[test]
    fn circle_tool_commits_with_radius() {
        let mut s = session();
        s.set_tool(SketchTool::Circle);
        s.on_cursor_sketch(0.0, 0.0);
        s.on_click();
        s.on_cursor_sketch(3.0, 4.0);
        s.on_click();
        let views = s.segments();
        assert_eq!(views.len(), 1);
        match views[0].kind {
            SegmentKind::Circle { radius, .. } => assert!((radius - 5.0).abs() < 1e-9),
            _ => panic!("expected circle"),
        }
    }

    #[test]
    fn snap_to_vertex_overrides_grid() {
        let mut s = session();
        s.add_line([10.0, 10.0], [20.0, 20.0]);
        s.snap_config_mut().grid_enabled = true;
        s.snap_config_mut().grid_size = 5.0;
        s.snap_config_mut().point_tolerance = 3.0;
        s.on_cursor_sketch(11.0, 11.0);
        let c = s.cursor().unwrap();
        assert_eq!(c.x, 10.0);
        assert_eq!(c.y, 10.0);
        assert_eq!(c.snap_target, Some([10.0, 10.0]));
    }

    #[test]
    fn snap_to_grid_when_no_vertex_nearby() {
        let mut s = session();
        s.snap_config_mut().grid_size = 5.0;
        s.on_cursor_sketch(12.3, 7.1);
        let c = s.cursor().unwrap();
        assert_eq!(c.x, 10.0);
        assert_eq!(c.y, 5.0);
    }

    #[test]
    fn hit_test_finds_line() {
        let mut s = session();
        s.add_line([0.0, 0.0], [10.0, 0.0]);
        assert_eq!(s.hit_test(5.0, 0.3, 1.0), Some(0));
        assert_eq!(s.hit_test(5.0, 2.0, 1.0), None);
    }

    #[test]
    fn undo_redo_roundtrip() {
        let mut s = session();
        s.set_tool(SketchTool::Rectangle);
        s.on_cursor_sketch(0.0, 0.0);
        s.on_click();
        s.on_cursor_sketch(10.0, 5.0);
        s.on_click();
        assert_eq!(s.segment_count(), 4);
        assert!(s.undo());
        assert_eq!(s.segment_count(), 0);
        assert!(s.redo());
        assert_eq!(s.segment_count(), 4);
    }

    #[test]
    fn solver_fixes_rectangle_with_real_constraints() {
        let mut s = session();
        // Intentionally off-axis rectangle that constraints should square up.
        let s0 = s.add_line([0.0, 0.0], [12.0, 1.0]);
        let s1 = s.add_line([12.0, 1.0], [11.0, 8.0]);
        let s2 = s.add_line([11.0, 8.0], [1.0, 7.0]);
        let s3 = s.add_line([1.0, 7.0], [0.0, 0.0]);
        // Fix the origin point (first endpoint of s0).
        let first_line = s.segment_entity(s0).unwrap();
        let (p0_ref, _) = s.line_endpoints_refs(first_line).unwrap();
        s.add_constraint(Constraint::Fixed {
            point: p0_ref,
            x: 0.0,
            y: 0.0,
        });
        s.add_constraint(Constraint::Horizontal {
            line: s.segment_entity(s0).unwrap(),
        });
        s.add_constraint(Constraint::Vertical {
            line: s.segment_entity(s1).unwrap(),
        });
        s.add_constraint(Constraint::Horizontal {
            line: s.segment_entity(s2).unwrap(),
        });
        s.add_constraint(Constraint::Vertical {
            line: s.segment_entity(s3).unwrap(),
        });
        s.add_constraint(Constraint::Length {
            line: s.segment_entity(s0).unwrap(),
            length: 10.0,
        });
        s.add_constraint(Constraint::Length {
            line: s.segment_entity(s1).unwrap(),
            length: 5.0,
        });
        let result = s.solve();
        assert!(result.converged, "solver should converge");
        let views = s.segments();
        // First line should be horizontal, length 10.
        let v0 = &views[0];
        assert!((v0.start[1] - v0.end[1]).abs() < 1e-6);
        assert!((distance(v0.start, v0.end) - 10.0).abs() < 1e-6);
    }

    #[test]
    fn exit_status() {
        let mut s = session();
        assert_eq!(s.exit_status(), ExitStatus::Empty);
        s.add_line([0.0, 0.0], [1.0, 0.0]);
        assert_eq!(s.exit_status(), ExitStatus::HasSegments);
    }
}

