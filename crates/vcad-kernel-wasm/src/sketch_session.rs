//! WASM bindings for the kernel [`SketchSession`].
//!
//! Exposes two surfaces to the web app:
//!
//! 1. [`WasmSketchSession`] — a stateful editing session used when the
//!    frontend wants full parity with the TUI (tool state machine, cursor
//!    snapping, undo/redo, real constraint solver).
//!
//! 2. [`solve_sketch_segments`] — a stateless one-shot function that takes
//!    a JSON array of segments + a JSON array of constraints, runs the
//!    kernel's Levenberg-Marquardt solver, and returns the updated
//!    segments. This lets the existing `packages/core/sketch-store.ts`
//!    replace its hand-rolled solver without any further migration.
//!
//! Both surfaces speak the same on-the-wire JSON as the TypeScript
//! `SketchSegment2D` / `SketchConstraint` discriminated unions so there is a
//! single, unambiguous contract between the web app and the kernel.

use serde::{Deserialize, Serialize};
use vcad_kernel::vcad_kernel_constraints::{
    Constraint, EntityId, EntityRef, SegmentKind, SketchPlane, SketchSession, SketchTool,
};
use wasm_bindgen::prelude::*;

// ---------------------------------------------------------------------------
// Wire types — mirror the TS `SketchSegment2D` / `SketchConstraint` shapes
// exactly (discriminator "type", camelCase "lineA"/"lineB"/"pointA"/"pointB"
// where applicable).
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone)]
#[serde(tag = "type")]
enum WireSegment {
    Line {
        start: WireVec2,
        end: WireVec2,
    },
    Arc {
        start: WireVec2,
        end: WireVec2,
        center: WireVec2,
        ccw: bool,
    },
}

#[derive(Serialize, Deserialize, Clone, Copy)]
struct WireVec2 {
    x: f64,
    y: f64,
}

#[derive(Serialize, Deserialize, Clone, Copy)]
#[serde(tag = "type")]
enum WireEntityRef {
    Point { index: usize },
    LineStart { index: usize },
    LineEnd { index: usize },
    ArcStart { index: usize },
    ArcEnd { index: usize },
    Center { index: usize },
}

#[derive(Serialize, Deserialize, Clone, Copy)]
#[serde(tag = "type")]
enum WireConstraint {
    Coincident {
        #[serde(rename = "pointA")]
        point_a: WireEntityRef,
        #[serde(rename = "pointB")]
        point_b: WireEntityRef,
    },
    Horizontal {
        line: usize,
    },
    Vertical {
        line: usize,
    },
    Parallel {
        #[serde(rename = "lineA")]
        line_a: usize,
        #[serde(rename = "lineB")]
        line_b: usize,
    },
    Perpendicular {
        #[serde(rename = "lineA")]
        line_a: usize,
        #[serde(rename = "lineB")]
        line_b: usize,
    },
    Fixed {
        point: WireEntityRef,
        x: f64,
        y: f64,
    },
    Distance {
        #[serde(rename = "pointA")]
        point_a: WireEntityRef,
        #[serde(rename = "pointB")]
        point_b: WireEntityRef,
        distance: f64,
    },
    Length {
        line: usize,
        length: f64,
    },
    EqualLength {
        #[serde(rename = "lineA")]
        line_a: usize,
        #[serde(rename = "lineB")]
        line_b: usize,
    },
    Radius {
        circle: usize,
        radius: f64,
    },
    Angle {
        #[serde(rename = "lineA")]
        line_a: usize,
        #[serde(rename = "lineB")]
        line_b: usize,
        #[serde(rename = "angleDeg")]
        angle_deg: f64,
    },
}

// ---------------------------------------------------------------------------
// Session snapshot — serialized state returned to JS on every mutation.
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct WireSnapshot {
    tool: &'static str,
    pending: Vec<[f64; 2]>,
    cursor: Option<WireCursor>,
    segments: Vec<WireSegment>,
    vertices: Vec<[f64; 2]>,
    selection: Vec<usize>,
    #[serde(rename = "constraintStatus")]
    constraint_status: &'static str,
    dof: i32,
    #[serde(rename = "segmentCount")]
    segment_count: usize,
    #[serde(rename = "constraintCount")]
    constraint_count: usize,
}

#[derive(Serialize)]
struct WireCursor {
    x: f64,
    y: f64,
    #[serde(rename = "snapTarget")]
    snap_target: Option<[f64; 2]>,
}

// ---------------------------------------------------------------------------
// Conversions
// ---------------------------------------------------------------------------

fn tool_name(tool: SketchTool) -> &'static str {
    match tool {
        SketchTool::Select => "select",
        SketchTool::Line => "line",
        SketchTool::Rectangle => "rectangle",
        SketchTool::Circle => "circle",
        SketchTool::Arc => "arc",
        SketchTool::Point => "point",
    }
}

fn parse_tool(name: &str) -> Option<SketchTool> {
    Some(match name {
        "select" => SketchTool::Select,
        "line" => SketchTool::Line,
        "rectangle" => SketchTool::Rectangle,
        "circle" => SketchTool::Circle,
        "arc" => SketchTool::Arc,
        "point" => SketchTool::Point,
        _ => return None,
    })
}

/// Convert a session [`SegmentKind`] plus endpoints into a [`WireSegment`].
/// Circles are emitted as four quarter-arcs to match the existing IR /
/// TypeScript shape, which only knows about lines and arcs.
fn segment_to_wire(
    kind: &SegmentKind,
    start: [f64; 2],
    end: [f64; 2],
) -> Vec<WireSegment> {
    match *kind {
        SegmentKind::Line => vec![WireSegment::Line {
            start: WireVec2 { x: start[0], y: start[1] },
            end: WireVec2 { x: end[0], y: end[1] },
        }],
        SegmentKind::Arc { center, ccw } => vec![WireSegment::Arc {
            start: WireVec2 { x: start[0], y: start[1] },
            end: WireVec2 { x: end[0], y: end[1] },
            center: WireVec2 { x: center[0], y: center[1] },
            ccw,
        }],
        SegmentKind::Circle { center, radius } => {
            let n = 4;
            (0..n)
                .map(|i| {
                    let a0 = (i as f64) * std::f64::consts::TAU / (n as f64);
                    let a1 = ((i + 1) as f64) * std::f64::consts::TAU / (n as f64);
                    let s = [
                        center[0] + radius * a0.cos(),
                        center[1] + radius * a0.sin(),
                    ];
                    let e = [
                        center[0] + radius * a1.cos(),
                        center[1] + radius * a1.sin(),
                    ];
                    WireSegment::Arc {
                        start: WireVec2 { x: s[0], y: s[1] },
                        end: WireVec2 { x: e[0], y: e[1] },
                        center: WireVec2 { x: center[0], y: center[1] },
                        ccw: true,
                    }
                })
                .collect()
        }
    }
}

fn session_snapshot(session: &SketchSession) -> WireSnapshot {
    let mut segments = Vec::new();
    for view in session.segments() {
        segments.extend(segment_to_wire(&view.kind, view.start, view.end));
    }
    let cursor = session.cursor().map(|c| WireCursor {
        x: c.x,
        y: c.y,
        snap_target: c.snap_target,
    });
    WireSnapshot {
        tool: tool_name(session.tool()),
        pending: session.pending_points().to_vec(),
        cursor,
        segments,
        vertices: session.vertices(),
        selection: session.selection().to_vec(),
        constraint_status: match session.constraint_status() {
            vcad_kernel::vcad_kernel_constraints::ConstraintStatus::Under => "under",
            vcad_kernel::vcad_kernel_constraints::ConstraintStatus::Solved => "solved",
            vcad_kernel::vcad_kernel_constraints::ConstraintStatus::Over => "over",
            vcad_kernel::vcad_kernel_constraints::ConstraintStatus::Error => "error",
        },
        dof: session.degrees_of_freedom(),
        segment_count: session.segment_count(),
        constraint_count: session.constraint_count(),
    }
}

// ---------------------------------------------------------------------------
// Plane input
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(untagged)]
enum WirePlane {
    Named(String),
    Arbitrary {
        origin: [f64; 3],
        #[serde(rename = "xDir")]
        x_dir: [f64; 3],
        #[serde(rename = "yDir")]
        y_dir: [f64; 3],
    },
}

fn parse_plane(s: &str) -> Result<SketchPlane, JsError> {
    let wire: WirePlane = serde_json::from_str(s)
        .map_err(|e| JsError::new(&format!("invalid plane JSON: {e}")))?;
    Ok(match wire {
        WirePlane::Named(name) => match name.as_str() {
            "XY" => SketchPlane::XY,
            "XZ" => SketchPlane::XZ,
            "YZ" => SketchPlane::YZ,
            other => {
                return Err(JsError::new(&format!("unknown axis-aligned plane: {other}")));
            }
        },
        WirePlane::Arbitrary { origin, x_dir, y_dir } => SketchPlane::Custom {
            origin,
            x_dir,
            y_dir,
        },
    })
}

// ---------------------------------------------------------------------------
// Segment → kernel entity building
// ---------------------------------------------------------------------------

/// Build a session from a list of wire segments. Each segment becomes one
/// line or arc entity in insertion order, so TypeScript segment indices map
/// directly onto `SketchSession` segment indices. Points are created fresh
/// per-segment (no deduplication — the solver will enforce any required
/// coincidence via constraints).
fn build_session_from_wire(plane: SketchPlane, wire: &[WireSegment]) -> SketchSession {
    let mut session = SketchSession::new(plane);
    for seg in wire {
        match *seg {
            WireSegment::Line { start, end } => {
                session.add_line([start.x, start.y], [end.x, end.y]);
            }
            WireSegment::Arc {
                start,
                end,
                center,
                ccw,
            } => {
                session.add_arc(
                    [start.x, start.y],
                    [end.x, end.y],
                    [center.x, center.y],
                    ccw,
                );
            }
        }
    }
    session
}

/// Translate a TypeScript segment-index / endpoint ref into a kernel
/// [`EntityRef`]. Returns `None` if the target segment does not exist or
/// the endpoint role doesn't match the segment kind (e.g. `LineStart` on an
/// arc).
fn wire_ref_to_kernel(session: &SketchSession, r: WireEntityRef) -> Option<EntityRef> {
    use vcad_kernel::vcad_kernel_constraints::SketchEntity;
    let lookup_line_endpoint = |index: usize, start: bool| -> Option<EntityRef> {
        let id = session.segment_entity(index)?;
        if let SketchEntity::Line(l) = session.sketch().entities.get(id)? {
            Some(EntityRef::Point(if start { l.start } else { l.end }))
        } else {
            None
        }
    };
    let lookup_arc_endpoint =
        |index: usize, role: ArcEndpoint| -> Option<EntityRef> {
            let id = session.segment_entity(index)?;
            if let SketchEntity::Arc(a) = session.sketch().entities.get(id)? {
                Some(EntityRef::Point(match role {
                    ArcEndpoint::Start => a.start,
                    ArcEndpoint::End => a.end,
                    ArcEndpoint::Center => a.center,
                }))
            } else {
                None
            }
        };
    match r {
        // TS `Point` refers to a segment index — by convention we treat
        // "point index N" as "start of segment N" so existing sketch-store
        // semantics survive. This matches how the JS-side toy solver indexed
        // points.
        WireEntityRef::Point { index } => lookup_line_endpoint(index, true),
        WireEntityRef::LineStart { index } => lookup_line_endpoint(index, true),
        WireEntityRef::LineEnd { index } => lookup_line_endpoint(index, false),
        WireEntityRef::ArcStart { index } => lookup_arc_endpoint(index, ArcEndpoint::Start),
        WireEntityRef::ArcEnd { index } => lookup_arc_endpoint(index, ArcEndpoint::End),
        WireEntityRef::Center { index } => lookup_arc_endpoint(index, ArcEndpoint::Center),
    }
}

enum ArcEndpoint {
    Start,
    End,
    Center,
}

/// Map a TypeScript segment-index to the kernel [`EntityId`] of that line or
/// arc. Returns `None` if the index is out of range.
fn wire_segment_to_entity(session: &SketchSession, index: usize) -> Option<EntityId> {
    session.segment_entity(index)
}

/// Convert a wire constraint into its kernel counterpart, resolving
/// segment-index references through the session. Returns an error if any
/// referenced segment doesn't exist or doesn't support the endpoint role.
fn wire_constraint_to_kernel(
    session: &SketchSession,
    c: WireConstraint,
) -> Result<Constraint, String> {
    let resolve_ref = |r: WireEntityRef| -> Result<EntityRef, String> {
        wire_ref_to_kernel(session, r)
            .ok_or_else(|| format!("cannot resolve entity ref {:?}", r_debug(r)))
    };
    let resolve_line = |idx: usize| -> Result<EntityId, String> {
        wire_segment_to_entity(session, idx)
            .ok_or_else(|| format!("segment {idx} not found"))
    };
    Ok(match c {
        WireConstraint::Coincident { point_a, point_b } => Constraint::Coincident {
            point_a: resolve_ref(point_a)?,
            point_b: resolve_ref(point_b)?,
        },
        WireConstraint::Horizontal { line } => Constraint::Horizontal {
            line: resolve_line(line)?,
        },
        WireConstraint::Vertical { line } => Constraint::Vertical {
            line: resolve_line(line)?,
        },
        WireConstraint::Parallel { line_a, line_b } => Constraint::Parallel {
            line_a: resolve_line(line_a)?,
            line_b: resolve_line(line_b)?,
        },
        WireConstraint::Perpendicular { line_a, line_b } => Constraint::Perpendicular {
            line_a: resolve_line(line_a)?,
            line_b: resolve_line(line_b)?,
        },
        WireConstraint::Fixed { point, x, y } => Constraint::Fixed {
            point: resolve_ref(point)?,
            x,
            y,
        },
        WireConstraint::Distance {
            point_a,
            point_b,
            distance,
        } => Constraint::Distance {
            point_a: resolve_ref(point_a)?,
            point_b: resolve_ref(point_b)?,
            distance,
        },
        WireConstraint::Length { line, length } => Constraint::Length {
            line: resolve_line(line)?,
            length,
        },
        WireConstraint::EqualLength { line_a, line_b } => Constraint::EqualLength {
            line_a: resolve_line(line_a)?,
            line_b: resolve_line(line_b)?,
        },
        WireConstraint::Radius { circle, radius } => Constraint::Radius {
            circle: resolve_line(circle)?,
            radius,
        },
        WireConstraint::Angle {
            line_a,
            line_b,
            angle_deg,
        } => Constraint::Angle {
            line_a: resolve_line(line_a)?,
            line_b: resolve_line(line_b)?,
            angle_rad: angle_deg.to_radians(),
        },
    })
}

fn r_debug(r: WireEntityRef) -> String {
    match r {
        WireEntityRef::Point { index } => format!("Point({index})"),
        WireEntityRef::LineStart { index } => format!("LineStart({index})"),
        WireEntityRef::LineEnd { index } => format!("LineEnd({index})"),
        WireEntityRef::ArcStart { index } => format!("ArcStart({index})"),
        WireEntityRef::ArcEnd { index } => format!("ArcEnd({index})"),
        WireEntityRef::Center { index } => format!("Center({index})"),
    }
}

// ---------------------------------------------------------------------------
// Stateful session binding
// ---------------------------------------------------------------------------

/// A sketch editing session bound to JavaScript. See module docs.
#[wasm_bindgen]
pub struct WasmSketchSession {
    inner: SketchSession,
}

#[wasm_bindgen]
impl WasmSketchSession {
    /// Construct a new session on the given plane.
    ///
    /// `plane_json` is either a JSON string (`"XY"` / `"XZ"` / `"YZ"`) or a
    /// JSON object `{ origin, xDir, yDir }` for a custom plane.
    #[wasm_bindgen(constructor)]
    pub fn new(plane_json: &str) -> Result<WasmSketchSession, JsError> {
        let plane = parse_plane(plane_json)?;
        Ok(Self {
            inner: SketchSession::new(plane),
        })
    }

    /// Return a JSON snapshot of the full session state. React can mirror
    /// this into its own store on every mutation.
    pub fn snapshot(&self) -> Result<String, JsError> {
        serde_json::to_string(&session_snapshot(&self.inner))
            .map_err(|e| JsError::new(&format!("snapshot serialize: {e}")))
    }

    /// Change the active drawing tool. Unknown names are ignored.
    #[wasm_bindgen(js_name = setTool)]
    pub fn set_tool(&mut self, tool: &str) {
        if let Some(t) = parse_tool(tool) {
            self.inner.set_tool(t);
        }
    }

    /// Configure snapping behavior.
    #[wasm_bindgen(js_name = setSnap)]
    pub fn set_snap(
        &mut self,
        grid_enabled: bool,
        grid_size: f64,
        point_enabled: bool,
        point_tolerance: f64,
    ) {
        let cfg = self.inner.snap_config_mut();
        cfg.grid_enabled = grid_enabled;
        cfg.grid_size = grid_size;
        cfg.point_enabled = point_enabled;
        cfg.point_tolerance = point_tolerance;
    }

    /// Update the cursor directly from 2D sketch coordinates.
    #[wasm_bindgen(js_name = onCursorSketch)]
    pub fn on_cursor_sketch(&mut self, x: f64, y: f64) {
        self.inner.on_cursor_sketch(x, y);
    }

    /// Update the cursor from a world-space ray (e.g. camera pick ray).
    #[wasm_bindgen(js_name = onCursorRay)]
    pub fn on_cursor_ray(
        &mut self,
        ox: f64,
        oy: f64,
        oz: f64,
        dx: f64,
        dy: f64,
        dz: f64,
    ) {
        self.inner.on_cursor_ray([ox, oy, oz], [dx, dy, dz]);
    }

    /// Clear the cursor.
    #[wasm_bindgen(js_name = onCursorLeave)]
    pub fn on_cursor_leave(&mut self) {
        self.inner.on_cursor_leave();
    }

    /// Handle a primary-button click at the current cursor position. Returns
    /// a short outcome string: `"no-cursor"`, `"selection"`, `"pending"`, or
    /// `"committed"`.
    #[wasm_bindgen(js_name = onClick)]
    pub fn on_click(&mut self) -> String {
        use vcad_kernel::vcad_kernel_constraints::ClickOutcome;
        match self.inner.on_click() {
            ClickOutcome::NoCursor => "no-cursor",
            ClickOutcome::Selection => "selection",
            ClickOutcome::Pending => "pending",
            ClickOutcome::Committed => "committed",
        }
        .to_string()
    }

    /// Handle a double-click (closes a polyline for the line tool).
    #[wasm_bindgen(js_name = onDoubleClick)]
    pub fn on_double_click(&mut self) {
        self.inner.on_double_click();
    }

    /// Clear pending input.
    #[wasm_bindgen(js_name = cancelPending)]
    pub fn cancel_pending(&mut self) {
        self.inner.cancel_pending();
    }

    /// Add a line directly (for scripted / MCP use).
    #[wasm_bindgen(js_name = addLine)]
    pub fn add_line(&mut self, x1: f64, y1: f64, x2: f64, y2: f64) {
        self.inner.add_line([x1, y1], [x2, y2]);
    }

    /// Add an axis-aligned rectangle between two corners.
    #[wasm_bindgen(js_name = addRectangle)]
    pub fn add_rectangle(&mut self, x1: f64, y1: f64, x2: f64, y2: f64) {
        self.inner.add_rectangle([x1, y1], [x2, y2]);
    }

    /// Add a full circle.
    #[wasm_bindgen(js_name = addCircle)]
    pub fn add_circle(&mut self, cx: f64, cy: f64, radius: f64) {
        self.inner.add_circle([cx, cy], radius);
    }

    /// Run the constraint solver. Returns `true` if it converged.
    pub fn solve(&mut self) -> bool {
        self.inner.solve().converged
    }

    /// Add a constraint from a JSON object matching the TypeScript
    /// `SketchConstraint` shape.
    #[wasm_bindgen(js_name = addConstraint)]
    pub fn add_constraint(&mut self, json: &str) -> Result<(), JsError> {
        let wire: WireConstraint = serde_json::from_str(json)
            .map_err(|e| JsError::new(&format!("constraint JSON: {e}")))?;
        let kernel = wire_constraint_to_kernel(&self.inner, wire)
            .map_err(|e| JsError::new(&e))?;
        self.inner.add_constraint(kernel);
        Ok(())
    }

    /// Remove the constraint at `index`.
    #[wasm_bindgen(js_name = removeConstraint)]
    pub fn remove_constraint(&mut self, index: usize) {
        self.inner.remove_constraint(index);
    }

    /// Test-select or deselect a segment.
    #[wasm_bindgen(js_name = toggleSelection)]
    pub fn toggle_selection(&mut self, segment_index: usize) {
        self.inner.toggle_selection(segment_index);
    }

    /// Clear the selection.
    #[wasm_bindgen(js_name = clearSelection)]
    pub fn clear_selection(&mut self) {
        self.inner.clear_selection();
    }

    /// Undo the last mutation. Returns `true` if anything was undone.
    pub fn undo(&mut self) -> bool {
        self.inner.undo()
    }

    /// Redo the last undone mutation. Returns `true` if anything was redone.
    pub fn redo(&mut self) -> bool {
        self.inner.redo()
    }

    /// Clear every entity and constraint.
    pub fn clear(&mut self) {
        self.inner.clear();
    }
}

// ---------------------------------------------------------------------------
// Stateless helpers — pure-function wrappers around [`SketchPlane`] and the
// session's snap / hit-test logic. These are what the web frontend imports
// to eliminate its duplicated copies of the same math.
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct WirePlaneBasis {
    origin: [f64; 3],
    #[serde(rename = "xDir")]
    x_dir: [f64; 3],
    #[serde(rename = "yDir")]
    y_dir: [f64; 3],
    normal: [f64; 3],
}

/// Return a plane's `{origin, xDir, yDir, normal}` as JSON. Accepts either a
/// named plane string or a custom-plane object (same shape as
/// [`WasmSketchSession`]'s constructor argument).
#[wasm_bindgen(js_name = sketchPlaneBasis)]
pub fn sketch_plane_basis(plane_json: &str) -> Result<String, JsError> {
    let plane = parse_plane(plane_json)?;
    let basis = WirePlaneBasis {
        origin: plane.origin(),
        x_dir: plane.x_dir(),
        y_dir: plane.y_dir(),
        normal: plane.normal(),
    };
    serde_json::to_string(&basis)
        .map_err(|e| JsError::new(&format!("basis serialize: {e}")))
}

/// Project a 3D world-space point onto a plane, returning 2D sketch
/// coordinates as `[x, y]` JSON.
#[wasm_bindgen(js_name = sketchWorldToSketch)]
pub fn sketch_world_to_sketch(
    plane_json: &str,
    wx: f64,
    wy: f64,
    wz: f64,
) -> Result<String, JsError> {
    let plane = parse_plane(plane_json)?;
    let [sx, sy] = plane.world_to_sketch([wx, wy, wz]);
    serde_json::to_string(&[sx, sy])
        .map_err(|e| JsError::new(&format!("serialize: {e}")))
}

/// Convert 2D sketch coordinates to a 3D world-space point, returning
/// `[x, y, z]` JSON.
#[wasm_bindgen(js_name = sketchToWorld)]
pub fn sketch_to_world(plane_json: &str, sx: f64, sy: f64) -> Result<String, JsError> {
    let plane = parse_plane(plane_json)?;
    let pt = plane.sketch_to_world(sx, sy);
    serde_json::to_string(&pt).map_err(|e| JsError::new(&format!("serialize: {e}")))
}

/// Intersect a world-space ray with a plane and return the hit in 2D
/// sketch coordinates as `[x, y]` JSON, or the literal string `"null"` when
/// the ray is parallel to the plane.
#[wasm_bindgen(js_name = sketchPlaneIntersectRay)]
pub fn sketch_plane_intersect_ray(
    plane_json: &str,
    ox: f64,
    oy: f64,
    oz: f64,
    dx: f64,
    dy: f64,
    dz: f64,
) -> Result<String, JsError> {
    let plane = parse_plane(plane_json)?;
    match plane.intersect_ray([ox, oy, oz], [dx, dy, dz]) {
        Some([x, y]) => serde_json::to_string(&[x, y])
            .map_err(|e| JsError::new(&format!("serialize: {e}"))),
        None => Ok("null".to_string()),
    }
}

#[derive(Serialize)]
struct WireSnapResult {
    x: f64,
    y: f64,
    #[serde(rename = "snapTarget")]
    snap_target: Option<[f64; 2]>,
}

/// Snap a 2D point against a segment list with grid + vertex rules. Returns
/// `{x, y, snapTarget}` JSON — the snapped position plus (if a vertex snap
/// fired) the vertex that was matched.
#[wasm_bindgen(js_name = sketchSnap)]
pub fn sketch_snap(
    segments_json: &str,
    x: f64,
    y: f64,
    grid_enabled: bool,
    grid_size: f64,
    point_enabled: bool,
    point_tolerance: f64,
) -> Result<String, JsError> {
    let wire_segments: Vec<WireSegment> = serde_json::from_str(segments_json)
        .map_err(|e| JsError::new(&format!("segments JSON: {e}")))?;
    let mut session = build_session_from_wire(SketchPlane::XY, &wire_segments);
    {
        let cfg = session.snap_config_mut();
        cfg.grid_enabled = grid_enabled;
        cfg.grid_size = grid_size;
        cfg.point_enabled = point_enabled;
        cfg.point_tolerance = point_tolerance;
    }
    session.on_cursor_sketch(x, y);
    let cursor = session.cursor().expect("cursor set by on_cursor_sketch");
    serde_json::to_string(&WireSnapResult {
        x: cursor.x,
        y: cursor.y,
        snap_target: cursor.snap_target,
    })
    .map_err(|e| JsError::new(&format!("serialize: {e}")))
}

/// Find the segment-index closest to `(x, y)` within `tolerance`. Returns
/// `-1` if no segment is within reach.
#[wasm_bindgen(js_name = sketchHitTest)]
pub fn sketch_hit_test(
    segments_json: &str,
    x: f64,
    y: f64,
    tolerance: f64,
) -> Result<i32, JsError> {
    let wire_segments: Vec<WireSegment> = serde_json::from_str(segments_json)
        .map_err(|e| JsError::new(&format!("segments JSON: {e}")))?;
    let session = build_session_from_wire(SketchPlane::XY, &wire_segments);
    Ok(session
        .hit_test(x, y, tolerance)
        .map(|i| i as i32)
        .unwrap_or(-1))
}

/// Build the four line segments of an axis-aligned rectangle between two
/// opposite corners. Returns a JSON array of `SketchSegment2D`.
#[wasm_bindgen(js_name = sketchRectangleSegments)]
pub fn sketch_rectangle_segments(
    p1x: f64,
    p1y: f64,
    p2x: f64,
    p2y: f64,
) -> Result<String, JsError> {
    let mut session = SketchSession::new(SketchPlane::XY);
    session.add_rectangle([p1x, p1y], [p2x, p2y]);
    let out: Vec<WireSegment> = session
        .segments()
        .iter()
        .flat_map(|v| segment_to_wire(&v.kind, v.start, v.end))
        .collect();
    serde_json::to_string(&out).map_err(|e| JsError::new(&format!("serialize: {e}")))
}

/// Build an N-sided polygonal approximation of a circle as arc segments.
/// Returns a JSON array of `SketchSegment2D`.
#[wasm_bindgen(js_name = sketchCircleSegments)]
pub fn sketch_circle_segments(
    cx: f64,
    cy: f64,
    radius: f64,
    segments: u32,
) -> Result<String, JsError> {
    let n = segments.max(3) as usize;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let a0 = (i as f64) * std::f64::consts::TAU / (n as f64);
        let a1 = ((i + 1) as f64) * std::f64::consts::TAU / (n as f64);
        let s = [cx + radius * a0.cos(), cy + radius * a0.sin()];
        let e = [cx + radius * a1.cos(), cy + radius * a1.sin()];
        out.push(WireSegment::Arc {
            start: WireVec2 { x: s[0], y: s[1] },
            end: WireVec2 { x: e[0], y: e[1] },
            center: WireVec2 { x: cx, y: cy },
            ccw: true,
        });
    }
    serde_json::to_string(&out).map_err(|e| JsError::new(&format!("serialize: {e}")))
}

// ---------------------------------------------------------------------------
// Stateless one-shot solver — the migration entry point for the web store.
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct SolveSketchResult {
    segments: Vec<WireSegment>,
    converged: bool,
}

/// Solve a TS-shaped sketch in one call.
///
/// Takes a JSON array of `SketchSegment2D` and a JSON array of
/// `SketchConstraint`, runs the Levenberg-Marquardt solver, and returns a
/// JSON object `{ segments, converged }` where `segments` is the solved
/// segment list in the same order as the input. Segments that don't belong
/// to the constraint system (e.g. circle-as-arcs that live purely for
/// rendering) pass through unchanged.
#[wasm_bindgen(js_name = solveSketchSegments)]
pub fn solve_sketch_segments(
    segments_json: &str,
    constraints_json: &str,
) -> Result<String, JsError> {
    let wire_segments: Vec<WireSegment> = serde_json::from_str(segments_json)
        .map_err(|e| JsError::new(&format!("segments JSON: {e}")))?;
    let wire_constraints: Vec<WireConstraint> = serde_json::from_str(constraints_json)
        .map_err(|e| JsError::new(&format!("constraints JSON: {e}")))?;

    let mut session = build_session_from_wire(SketchPlane::XY, &wire_segments);

    for wire in &wire_constraints {
        match wire_constraint_to_kernel(&session, *wire) {
            Ok(k) => session.add_constraint(k),
            Err(e) => return Err(JsError::new(&format!("constraint: {e}"))),
        }
    }

    let converged = if wire_constraints.is_empty() {
        true
    } else {
        session.solve().converged
    };

    // Read back the solved segments in insertion order. We keep the original
    // wire type (so arcs stay arcs, lines stay lines) rather than rebuilding
    // from `SegmentView` — this preserves fields that the solver doesn't
    // touch (like `ccw` for arcs) even when the geometry has been nudged.
    let mut out = Vec::with_capacity(wire_segments.len());
    for (i, original) in wire_segments.iter().enumerate() {
        let view = session
            .segments()
            .into_iter()
            .find(|v| v.index == i)
            .ok_or_else(|| JsError::new("internal: missing segment after solve"))?;
        match original {
            WireSegment::Line { .. } => out.push(WireSegment::Line {
                start: WireVec2 {
                    x: view.start[0],
                    y: view.start[1],
                },
                end: WireVec2 {
                    x: view.end[0],
                    y: view.end[1],
                },
            }),
            WireSegment::Arc { ccw, .. } => {
                let center = match view.kind {
                    SegmentKind::Arc { center, .. } => center,
                    _ => [0.0, 0.0],
                };
                out.push(WireSegment::Arc {
                    start: WireVec2 {
                        x: view.start[0],
                        y: view.start[1],
                    },
                    end: WireVec2 {
                        x: view.end[0],
                        y: view.end[1],
                    },
                    center: WireVec2 {
                        x: center[0],
                        y: center[1],
                    },
                    ccw: *ccw,
                });
            }
        }
    }

    serde_json::to_string(&SolveSketchResult {
        segments: out,
        converged,
    })
    .map_err(|e| JsError::new(&format!("result serialize: {e}")))
}

// ---------------------------------------------------------------------------
// Tests (pure Rust — don't exercise wasm_bindgen, but cover all the mapping
// logic that sits between JS and the kernel solver).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_axis_aligned_plane() {
        let p = parse_plane("\"XY\"").expect("plane parse");
        assert_eq!(p, SketchPlane::XY);
        let p = parse_plane("\"YZ\"").expect("plane parse");
        assert_eq!(p, SketchPlane::YZ);
    }

    #[test]
    fn parse_custom_plane() {
        let json = r#"{"origin":[1.0,2.0,3.0],"xDir":[1.0,0.0,0.0],"yDir":[0.0,1.0,0.0]}"#;
        let p = parse_plane(json).expect("custom plane");
        match p {
            SketchPlane::Custom { origin, .. } => assert_eq!(origin, [1.0, 2.0, 3.0]),
            _ => panic!("expected Custom"),
        }
    }

    /// Mirror of [`solve_sketch_segments`] that skips the wasm_bindgen wrapper
    /// so we can unit-test the mapping in a regular Rust target without a
    /// browser.
    fn solve_for_test(segments: &str, constraints: &str) -> serde_json::Value {
        let wire_segments: Vec<WireSegment> = serde_json::from_str(segments).unwrap();
        let wire_constraints: Vec<WireConstraint> = serde_json::from_str(constraints).unwrap();
        let mut session = build_session_from_wire(SketchPlane::XY, &wire_segments);
        for c in &wire_constraints {
            let k = wire_constraint_to_kernel(&session, *c).expect("constraint map");
            session.add_constraint(k);
        }
        let converged = if wire_constraints.is_empty() {
            true
        } else {
            session.solve().converged
        };
        let mut out = Vec::with_capacity(wire_segments.len());
        for (i, original) in wire_segments.iter().enumerate() {
            let view = session
                .segments()
                .into_iter()
                .find(|v| v.index == i)
                .expect("segment after solve");
            match original {
                WireSegment::Line { .. } => out.push(WireSegment::Line {
                    start: WireVec2 { x: view.start[0], y: view.start[1] },
                    end: WireVec2 { x: view.end[0], y: view.end[1] },
                }),
                WireSegment::Arc { ccw, .. } => {
                    let center = match view.kind {
                        SegmentKind::Arc { center, .. } => center,
                        _ => [0.0, 0.0],
                    };
                    out.push(WireSegment::Arc {
                        start: WireVec2 { x: view.start[0], y: view.start[1] },
                        end: WireVec2 { x: view.end[0], y: view.end[1] },
                        center: WireVec2 { x: center[0], y: center[1] },
                        ccw: *ccw,
                    });
                }
            }
        }
        serde_json::json!({
            "segments": serde_json::to_value(&out).unwrap(),
            "converged": converged,
        })
    }

    #[test]
    fn solve_sketch_rectangle_via_wire_api() {
        let segments = r#"[
            {"type":"Line","start":{"x":0.0,"y":0.0},"end":{"x":12.0,"y":1.0}},
            {"type":"Line","start":{"x":12.0,"y":1.0},"end":{"x":11.0,"y":8.0}},
            {"type":"Line","start":{"x":11.0,"y":8.0},"end":{"x":1.0,"y":7.0}},
            {"type":"Line","start":{"x":1.0,"y":7.0},"end":{"x":0.0,"y":0.0}}
        ]"#;
        let constraints = r#"[
            {"type":"Fixed","point":{"type":"LineStart","index":0},"x":0.0,"y":0.0},
            {"type":"Horizontal","line":0},
            {"type":"Vertical","line":1},
            {"type":"Horizontal","line":2},
            {"type":"Vertical","line":3},
            {"type":"Length","line":0,"length":10.0},
            {"type":"Length","line":1,"length":5.0}
        ]"#;
        let parsed = solve_for_test(segments, constraints);
        assert_eq!(parsed["converged"], serde_json::Value::Bool(true));
        let seg0 = &parsed["segments"][0];
        let y0 = seg0["start"]["y"].as_f64().unwrap();
        let y1 = seg0["end"]["y"].as_f64().unwrap();
        assert!((y0 - y1).abs() < 1e-6, "line 0 should be horizontal");
        let x0 = seg0["start"]["x"].as_f64().unwrap();
        let x1 = seg0["end"]["x"].as_f64().unwrap();
        assert!((x1 - x0 - 10.0).abs() < 1e-6, "line 0 should have length 10");
    }

    #[test]
    fn stateless_helpers_roundtrip() {
        // Plane basis of XY
        let basis = sketch_plane_basis("\"XY\"").unwrap();
        let v: serde_json::Value = serde_json::from_str(&basis).unwrap();
        assert_eq!(v["xDir"], serde_json::json!([1.0, 0.0, 0.0]));
        assert_eq!(v["yDir"], serde_json::json!([0.0, 1.0, 0.0]));

        // world ↔ sketch round trip on XY
        let ws = sketch_world_to_sketch("\"XY\"", 3.0, 4.0, 5.0).unwrap();
        assert_eq!(ws, "[3.0,4.0]");
        let sw = sketch_to_world("\"XY\"", 3.0, 4.0).unwrap();
        assert_eq!(sw, "[3.0,4.0,0.0]");

        // Ray intersect
        let hit =
            sketch_plane_intersect_ray("\"XY\"", 2.0, 3.0, 5.0, 0.0, 0.0, -1.0).unwrap();
        assert_eq!(hit, "[2.0,3.0]");
        let miss =
            sketch_plane_intersect_ray("\"XY\"", 0.0, 0.0, 5.0, 1.0, 0.0, 0.0).unwrap();
        assert_eq!(miss, "null");
    }

    #[test]
    fn stateless_rectangle_and_circle_builders() {
        let rect = sketch_rectangle_segments(0.0, 0.0, 10.0, 5.0).unwrap();
        let rv: serde_json::Value = serde_json::from_str(&rect).unwrap();
        assert_eq!(rv.as_array().unwrap().len(), 4);

        let circle = sketch_circle_segments(0.0, 0.0, 5.0, 8).unwrap();
        let cv: serde_json::Value = serde_json::from_str(&circle).unwrap();
        assert_eq!(cv.as_array().unwrap().len(), 8);
        assert_eq!(cv[0]["type"], "Arc");
        assert_eq!(cv[0]["center"]["x"], 0.0);
    }

    #[test]
    fn stateless_snap_and_hit_test() {
        let segments = r#"[
            {"type":"Line","start":{"x":10.0,"y":10.0},"end":{"x":20.0,"y":20.0}}
        ]"#;

        // Snap onto an existing vertex within tolerance.
        let snapped = sketch_snap(segments, 11.0, 11.0, true, 5.0, true, 3.0).unwrap();
        let sv: serde_json::Value = serde_json::from_str(&snapped).unwrap();
        assert_eq!(sv["x"], 10.0);
        assert_eq!(sv["y"], 10.0);
        assert_eq!(sv["snapTarget"], serde_json::json!([10.0, 10.0]));

        // Hit-test that finds the line.
        assert_eq!(sketch_hit_test(segments, 15.0, 15.1, 0.5).unwrap(), 0);
        // Hit-test that misses.
        assert_eq!(sketch_hit_test(segments, 50.0, 50.0, 1.0).unwrap(), -1);
    }

    #[test]
    fn solve_passes_through_unconstrained_segments() {
        let segments = r#"[
            {"type":"Line","start":{"x":0.0,"y":0.0},"end":{"x":5.0,"y":5.0}}
        ]"#;
        let parsed = solve_for_test(segments, "[]");
        assert_eq!(parsed["converged"], serde_json::Value::Bool(true));
        let seg = &parsed["segments"][0];
        assert_eq!(seg["start"]["x"], 0.0);
        assert_eq!(seg["end"]["x"], 5.0);
    }
}
