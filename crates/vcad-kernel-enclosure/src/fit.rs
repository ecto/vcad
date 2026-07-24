//! Cross-domain PCB ↔ enclosure verification core.
//!
//! Given a board outline + features and the enclosure cavity (extracted from a
//! solid mesh by [`crate::extract_enclosure_features`]), answer four questions
//! a fab house can't:
//!
//!   1. Does the board fit the cavity with clearance?
//!   2. Do tall components clear the lid? (stack height vs cavity depth)
//!   3. Do the mounting holes land on the case standoffs?
//!   4. Do edge connectors line up with the wall cutouts?
//!
//! Frames: enclosure features are in **enclosure-world** (Z-up, mm). A board
//! is authored in its own **board-local** frame (origin-corner outline, board
//! bottom at z=0, top at z=thickness). A [`BoardPlacement`] maps board-local →
//! world (Z-rotation then translation).
//!
//! All serde field names match the original TypeScript interfaces in
//! `@vcad/engine` — the WASM boundary round-trips the same JSON.

use serde::ser::SerializeMap;
use serde::{Deserialize, Serialize};

use crate::{fmt_num, round2};

// ===========================================================================
// Types
// ===========================================================================

/// 2D point (mm).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Vec2 {
    /// X component.
    pub x: f64,
    /// Y component.
    pub y: f64,
}

/// 3D point (mm).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Vec3 {
    /// X component.
    pub x: f64,
    /// Y component.
    pub y: f64,
    /// Z component.
    pub z: f64,
}

/// Closed board outline polygon (board-local, mm).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoardOutline {
    /// Outline vertices (last connects to first).
    pub vertices: Vec<Vec2>,
    /// Board thickness in mm.
    #[serde(default)]
    pub thickness: f64,
}

/// Axis-aligned interior void of an enclosure, in enclosure-world coords.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EnclosureCavity {
    /// Minimum X of the void.
    #[serde(rename = "minX")]
    pub min_x: f64,
    /// Maximum X of the void.
    #[serde(rename = "maxX")]
    pub max_x: f64,
    /// Minimum Y of the void.
    #[serde(rename = "minY")]
    pub min_y: f64,
    /// Maximum Y of the void.
    #[serde(rename = "maxY")]
    pub max_y: f64,
    /// Top surface of the cavity floor (Z); the board's underside rests above.
    #[serde(rename = "floorZ")]
    pub floor_z: f64,
    /// Underside of the lid / top of the usable cavity (Z).
    #[serde(rename = "ceilZ")]
    pub ceil_z: f64,
    /// True when a closed top was detected; false for an open-top case.
    #[serde(rename = "hasLid")]
    pub has_lid: bool,
}

/// A boss/post rising from the cavity floor that a screw threads into.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Standoff {
    /// Post center X (world).
    pub x: f64,
    /// Post center Y (world).
    pub y: f64,
    /// Z of the post's top face (where the board lands).
    #[serde(rename = "topZ")]
    pub top_z: f64,
    /// Approximate post radius (mm).
    pub radius: f64,
}

/// Which outer wall a feature sits against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WallEdge {
    /// The −X wall.
    #[serde(rename = "minX")]
    MinX,
    /// The +X wall.
    #[serde(rename = "maxX")]
    MaxX,
    /// The −Y wall.
    #[serde(rename = "minY")]
    MinY,
    /// The +Y wall.
    #[serde(rename = "maxY")]
    MaxY,
}

/// An opening cut through a wall (e.g. a USB/JST port).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WallOpening {
    /// Which wall the opening is in.
    pub edge: WallEdge,
    /// Center of the opening along the wall (world XY).
    pub center: Vec2,
    /// Opening width along the wall tangent (mm).
    pub width: f64,
    /// Bottom of the opening (Z).
    #[serde(rename = "zMin")]
    pub z_min: f64,
    /// Top of the opening (Z).
    #[serde(rename = "zMax")]
    pub z_max: f64,
}

/// Maps board-local coordinates into the enclosure-world frame.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BoardPlacement {
    /// Board-local origin in enclosure-world coordinates.
    pub offset: Vec3,
    /// CCW rotation about Z, in degrees.
    #[serde(rename = "rotationDeg")]
    pub rotation_deg: f64,
}

/// A board mounting hole, in board-local coordinates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MountingHole {
    /// Hole center X (board-local).
    pub x: f64,
    /// Hole center Y (board-local).
    pub y: f64,
    /// Drill diameter (mm).
    pub diameter: f64,
    /// Designator of the footprint that declared the hole.
    #[serde(rename = "ref", default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}

/// An edge connector, in board-local coordinates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConnectorRef {
    /// Designator (e.g. `J1`).
    #[serde(rename = "ref")]
    pub reference: String,
    /// Connector origin X (board-local).
    pub x: f64,
    /// Connector origin Y (board-local).
    pub y: f64,
    /// Nearest board edge the connector exits through (board-local AABB).
    pub edge: Option<WallEdge>,
    /// Component body height above the board (mm); 0 when unknown.
    #[serde(default)]
    pub height: f64,
}

/// Per-component vertical extent in board-local Z (board bottom = 0, top =
/// thickness). Front parts sit above `thickness`; back parts dip below 0.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComponentExtent {
    /// Designator.
    #[serde(rename = "ref")]
    pub reference: String,
    /// True for front-side parts (rising toward the lid).
    pub front: bool,
    /// Top of the body (board-local Z).
    #[serde(rename = "topZ")]
    pub top_z: f64,
    /// Bottom of the body (board-local Z).
    #[serde(rename = "bottomZ")]
    pub bottom_z: f64,
}

/// Verdict of one verification line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    /// The check ran and holds.
    Pass,
    /// The check ran and is violated.
    Fail,
    /// A detection gap, not a hard failure.
    Warn,
    /// Inputs for the check were absent.
    Skip,
}

/// One measurement value (JS `number | string | boolean`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MeasurementValue {
    /// Numeric measurement.
    Num(f64),
    /// String measurement (e.g. a designator or `"n/a"`).
    Str(String),
    /// Boolean measurement.
    Bool(bool),
}

impl From<f64> for MeasurementValue {
    fn from(v: f64) -> Self {
        Self::Num(v)
    }
}
impl From<usize> for MeasurementValue {
    fn from(v: usize) -> Self {
        Self::Num(v as f64)
    }
}
impl From<&str> for MeasurementValue {
    fn from(v: &str) -> Self {
        Self::Str(v.to_string())
    }
}
impl From<String> for MeasurementValue {
    fn from(v: String) -> Self {
        Self::Str(v)
    }
}

/// Ordered string-keyed measurement map. Serializes as a JSON object with
/// insertion order preserved (matching the original TS object literals).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Measurements(pub Vec<(String, MeasurementValue)>);

impl Measurements {
    /// Value for `key`, if present.
    pub fn get(&self, key: &str) -> Option<&MeasurementValue> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }
}

impl Serialize for Measurements {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (k, v) in &self.0 {
            map.serialize_entry(k, v)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for Measurements {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = Measurements;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a map of measurements")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut access: A,
            ) -> Result<Self::Value, A::Error> {
                let mut out = Vec::new();
                while let Some((k, v)) = access.next_entry()? {
                    out.push((k, v));
                }
                Ok(Measurements(out))
            }
        }
        deserializer.deserialize_map(V)
    }
}

/// One cross-domain verification line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnclosureFitCheck {
    /// Stable machine id (`board_fit`, `lid_clearance`, …).
    pub id: String,
    /// Human label.
    pub label: String,
    /// Verdict.
    pub status: CheckStatus,
    /// Human-readable explanation with the governing numbers.
    pub detail: String,
    /// Structured measurements backing the verdict.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measurements: Option<Measurements>,
}

/// The full cross-domain verdict.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnclosureFitReport {
    /// True when no check failed (warnings do not flip this; they are surfaced).
    pub ok: bool,
    /// True only when nothing failed AND nothing warned (fully verified).
    pub verified: bool,
    /// One-line rollup.
    pub summary: String,
    /// Clearance requirement the checks ran with (mm).
    pub clearance: f64,
    /// The placement the checks ran with (given or auto-fit).
    pub placement: BoardPlacement,
    /// The individual check lines.
    pub checks: Vec<EnclosureFitCheck>,
}

/// Input to the pure verification core.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnclosureFitInput {
    /// Board outline polygon (board-local).
    pub outline: BoardOutline,
    /// The enclosure cavity to verify against.
    pub cavity: EnclosureCavity,
    /// Detected standoffs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub standoffs: Option<Vec<Standoff>>,
    /// Detected wall openings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openings: Option<Vec<WallOpening>>,
    /// Board mounting holes (board-local).
    #[serde(
        rename = "mountingHoles",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub mounting_holes: Option<Vec<MountingHole>>,
    /// Board edge connectors (board-local).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connectors: Option<Vec<ConnectorRef>>,
    /// Per-component vertical extents (board-local Z).
    #[serde(
        rename = "componentExtents",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub component_extents: Option<Vec<ComponentExtent>>,
    /// Where the board sits; auto-fit (centered, on standoffs) when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement: Option<BoardPlacement>,
    /// All-round clearance the board needs from the cavity walls (mm). Default 0.5.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clearance: Option<f64>,
    /// Board-bottom lift above the floor when no standoffs are given (mm). Default 0.
    #[serde(
        rename = "standoffHeight",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub standoff_height: Option<f64>,
    /// Hole-to-standoff alignment tolerance (mm). Default 0.6.
    #[serde(
        rename = "holeTolerance",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub hole_tolerance: Option<f64>,
}

// ===========================================================================
// Geometry helpers
// ===========================================================================

const DEFAULT_CLEARANCE: f64 = 0.5;
const DEFAULT_HOLE_TOL: f64 = 0.6;

/// Axis-aligned bounds of a polygon.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Aabb2 {
    /// Minimum X.
    #[serde(rename = "minX")]
    pub min_x: f64,
    /// Maximum X.
    #[serde(rename = "maxX")]
    pub max_x: f64,
    /// Minimum Y.
    #[serde(rename = "minY")]
    pub min_y: f64,
    /// Maximum Y.
    #[serde(rename = "maxY")]
    pub max_y: f64,
}

/// Axis-aligned bounds of a board outline.
pub fn outline_aabb(outline: &BoardOutline) -> Aabb2 {
    let mut b = Aabb2 {
        min_x: f64::INFINITY,
        max_x: f64::NEG_INFINITY,
        min_y: f64::INFINITY,
        max_y: f64::NEG_INFINITY,
    };
    for v in &outline.vertices {
        b.min_x = b.min_x.min(v.x);
        b.max_x = b.max_x.max(v.x);
        b.min_y = b.min_y.min(v.y);
        b.max_y = b.max_y.max(v.y);
    }
    b
}

/// Map a board-local point into the enclosure-world frame.
pub fn to_world(x: f64, y: f64, z: f64, placement: &BoardPlacement) -> Vec3 {
    let t = placement.rotation_deg.to_radians();
    let (sin, cos) = (t.sin(), t.cos());
    Vec3 {
        x: placement.offset.x + x * cos - y * sin,
        y: placement.offset.y + x * sin + y * cos,
        z: placement.offset.z + z,
    }
}

/// Nearest cavity (or AABB) edge to a point, by perpendicular distance. Ties
/// resolve in `minX, maxX, minY, maxY` order (stable sort, insertion order),
/// matching the TS implementation.
pub(crate) fn nearest_edge(
    x: f64,
    y: f64,
    min_x: f64,
    max_x: f64,
    min_y: f64,
    max_y: f64,
) -> WallEdge {
    let mut d = [
        (WallEdge::MinX, (x - min_x).abs()),
        (WallEdge::MaxX, (x - max_x).abs()),
        (WallEdge::MinY, (y - min_y).abs()),
        (WallEdge::MaxY, (y - max_y).abs()),
    ];
    d.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    d[0].0
}

// ===========================================================================
// Verification core
// ===========================================================================

/// Auto-fit placement: center the board in the cavity, resting on standoffs.
fn auto_placement(input: &EnclosureFitInput) -> BoardPlacement {
    let cavity = &input.cavity;
    let a = outline_aabb(&input.outline);
    let board_w = a.max_x - a.min_x;
    let board_h = a.max_y - a.min_y;
    let cav_w = cavity.max_x - cavity.min_x;
    let cav_h = cavity.max_y - cavity.min_y;
    // Center the outline AABB inside the cavity (board-local origin offset so
    // the outline's min corner lands at the centered position).
    let off_x = cavity.min_x + (cav_w - board_w) / 2.0 - a.min_x;
    let off_y = cavity.min_y + (cav_h - board_h) / 2.0 - a.min_y;
    let standoff_top = match &input.standoffs {
        Some(s) if !s.is_empty() => s.iter().map(|s| s.top_z).fold(f64::NEG_INFINITY, f64::max),
        _ => cavity.floor_z + input.standoff_height.unwrap_or(0.0),
    };
    BoardPlacement {
        offset: Vec3 {
            x: round2(off_x),
            y: round2(off_y),
            z: round2(standoff_top),
        },
        rotation_deg: 0.0,
    }
}

/// Check 1 — board fits the cavity footprint with clearance on all sides.
fn check_board_fit(
    input: &EnclosureFitInput,
    placement: &BoardPlacement,
    clearance: f64,
) -> EnclosureFitCheck {
    let cavity = &input.cavity;
    let mut min_x = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for v in &input.outline.vertices {
        let w = to_world(v.x, v.y, 0.0, placement);
        min_x = min_x.min(w.x);
        max_x = max_x.max(w.x);
        min_y = min_y.min(w.y);
        max_y = max_y.max(w.y);
    }
    let margin_min_x = min_x - cavity.min_x;
    let margin_max_x = cavity.max_x - max_x;
    let margin_min_y = min_y - cavity.min_y;
    let margin_max_y = cavity.max_y - max_y;
    let worst = margin_min_x
        .min(margin_max_x)
        .min(margin_min_y)
        .min(margin_max_y);
    let sides = [
        ("-X", margin_min_x),
        ("+X", margin_max_x),
        ("-Y", margin_min_y),
        ("+Y", margin_max_y),
    ];
    let tight: Vec<&str> = sides
        .iter()
        .filter(|(_, m)| *m < clearance)
        .map(|(s, _)| *s)
        .collect();
    let ok = worst >= clearance - 1e-6;
    let detail = if ok {
        format!(
            "Board fits with {}mm worst-case clearance (need {}mm)",
            fmt_num(round2(worst)),
            fmt_num(clearance)
        )
    } else if worst < 0.0 {
        format!(
            "Board overhangs the cavity by {}mm on {}",
            fmt_num(round2(-worst)),
            tight.join(", ")
        )
    } else {
        format!(
            "Clearance on {} is {}mm < required {}mm",
            tight.join(", "),
            fmt_num(round2(worst)),
            fmt_num(clearance)
        )
    };
    EnclosureFitCheck {
        id: "board_fit".to_string(),
        label: "Board fits cavity with clearance".to_string(),
        status: if ok {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        },
        detail,
        measurements: Some(Measurements(vec![
            ("worst_clearance_mm".into(), round2(worst).into()),
            ("margin_minus_x".into(), round2(margin_min_x).into()),
            ("margin_plus_x".into(), round2(margin_max_x).into()),
            ("margin_minus_y".into(), round2(margin_min_y).into()),
            ("margin_plus_y".into(), round2(margin_max_y).into()),
            ("board_w".into(), round2(max_x - min_x).into()),
            ("board_h".into(), round2(max_y - min_y).into()),
            (
                "cavity_w".into(),
                round2(cavity.max_x - cavity.min_x).into(),
            ),
            (
                "cavity_h".into(),
                round2(cavity.max_y - cavity.min_y).into(),
            ),
        ])),
    }
}

/// Check 2 — tall components clear the lid; back parts clear the floor.
fn check_lid_clearance(
    input: &EnclosureFitInput,
    placement: &BoardPlacement,
    clearance: f64,
) -> EnclosureFitCheck {
    let cavity = &input.cavity;
    let empty: Vec<ComponentExtent> = vec![];
    let extents = input.component_extents.as_ref().unwrap_or(&empty);
    if extents.is_empty() {
        return EnclosureFitCheck {
            id: "lid_clearance".to_string(),
            label: "Components clear the lid".to_string(),
            status: CheckStatus::Skip,
            detail: "No component heights available (kernel component meshes unavailable)"
                .to_string(),
            measurements: None,
        };
    }
    let cavity_depth = cavity.ceil_z - cavity.floor_z;
    // Front parts rise above the board top into the cavity.
    let front: Vec<&ComponentExtent> = extents.iter().filter(|e| e.front).collect();
    let back: Vec<&ComponentExtent> = extents.iter().filter(|e| !e.front).collect();
    let mut tallest: (String, f64) = (String::new(), f64::NEG_INFINITY);
    for e in &front {
        let top = placement.offset.z + e.top_z;
        if top > tallest.1 {
            tallest = (e.reference.clone(), top);
        }
    }
    let lid_gap = cavity.ceil_z - tallest.1; // free space above tallest part
    let top_ok = front.is_empty() || lid_gap >= clearance - 1e-6;

    // Back parts dip below the board into the standoff gap toward the floor.
    let mut lowest: (String, f64) = (String::new(), f64::INFINITY);
    for e in &back {
        let bot = placement.offset.z + e.bottom_z;
        if bot < lowest.1 {
            lowest = (e.reference.clone(), bot);
        }
    }
    let floor_gap = if !back.is_empty() {
        lowest.1 - cavity.floor_z
    } else {
        f64::INFINITY
    };
    let bot_ok = back.is_empty() || floor_gap >= -1e-6;

    let ok = top_ok && bot_ok;
    let detail = if ok {
        let mut d = format!(
            "Tallest part {} leaves {}mm under the lid (cavity depth {}mm)",
            if tallest.0.is_empty() {
                "—"
            } else {
                tallest.0.as_str()
            },
            fmt_num(round2(lid_gap)),
            fmt_num(round2(cavity_depth))
        );
        if !back.is_empty() && floor_gap.is_finite() {
            d.push_str(&format!(
                "; back-side {} clears floor by {}mm",
                lowest.0,
                fmt_num(round2(floor_gap))
            ));
        }
        d
    } else if !top_ok {
        format!(
            "{} is {}mm too tall — it {} the lid (cavity depth {}mm)",
            tallest.0,
            fmt_num(round2(-lid_gap + clearance)),
            if lid_gap < 0.0 {
                "punches through"
            } else {
                "is within clearance of"
            },
            fmt_num(round2(cavity_depth))
        )
    } else {
        format!(
            "Back-side {} collides with the floor by {}mm — raise the standoffs",
            lowest.0,
            fmt_num(round2(-floor_gap))
        )
    };
    let finite_or_na = |v: f64| -> MeasurementValue {
        if v.is_finite() {
            round2(v).into()
        } else {
            "n/a".into()
        }
    };
    EnclosureFitCheck {
        id: "lid_clearance".to_string(),
        label: "Components clear the lid".to_string(),
        status: if ok {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        },
        detail,
        measurements: Some(Measurements(vec![
            ("cavity_depth_mm".into(), round2(cavity_depth).into()),
            (
                "tallest_ref".into(),
                if tallest.0.is_empty() {
                    "none".into()
                } else {
                    tallest.0.clone().into()
                },
            ),
            ("lid_gap_mm".into(), finite_or_na(lid_gap)),
            ("stack_top_z".into(), finite_or_na(tallest.1)),
            ("floor_gap_mm".into(), finite_or_na(floor_gap)),
        ])),
    }
}

/// Check 3 — every mounting hole lands on a case standoff.
fn check_mounting_holes(
    input: &EnclosureFitInput,
    placement: &BoardPlacement,
) -> EnclosureFitCheck {
    let empty_h: Vec<MountingHole> = vec![];
    let empty_s: Vec<Standoff> = vec![];
    let holes = input.mounting_holes.as_ref().unwrap_or(&empty_h);
    let standoffs = input.standoffs.as_ref().unwrap_or(&empty_s);
    let tol = input.hole_tolerance.unwrap_or(DEFAULT_HOLE_TOL);
    let label = "Mounting holes land on standoffs".to_string();
    if holes.is_empty() {
        return EnclosureFitCheck {
            id: "mounting_holes".to_string(),
            label,
            status: CheckStatus::Skip,
            detail: "Board declares no mounting holes".to_string(),
            measurements: None,
        };
    }
    if standoffs.is_empty() {
        return EnclosureFitCheck {
            id: "mounting_holes".to_string(),
            label,
            status: CheckStatus::Skip,
            detail: format!(
                "Board has {} mounting hole(s) but no standoffs were detected in the enclosure",
                holes.len()
            ),
            measurements: None,
        };
    }
    let mut matched = 0usize;
    let mut worst = 0.0f64;
    let mut misses: Vec<String> = Vec::new();
    for h in holes {
        let w = to_world(h.x, h.y, 0.0, placement);
        let mut best = f64::INFINITY;
        for s in standoffs {
            let d = (w.x - s.x).hypot(w.y - s.y);
            if d < best {
                best = d;
            }
        }
        if best <= tol {
            matched += 1;
            if best > worst {
                worst = best;
            }
        } else {
            misses.push(format!(
                "{}@({},{}) is {}mm off",
                h.reference.as_deref().unwrap_or("hole"),
                fmt_num(round2(w.x)),
                fmt_num(round2(w.y)),
                fmt_num(round2(best))
            ));
        }
    }
    let ok = matched == holes.len();
    EnclosureFitCheck {
        id: "mounting_holes".to_string(),
        label,
        status: if ok {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        },
        detail: if ok {
            format!(
                "All {} mounting holes align to standoffs (worst offset {}mm, tol {}mm)",
                holes.len(),
                fmt_num(round2(worst)),
                fmt_num(tol)
            )
        } else {
            format!(
                "{}/{} holes align — {}",
                matched,
                holes.len(),
                misses.join("; ")
            )
        },
        measurements: Some(Measurements(vec![
            ("holes_total".into(), holes.len().into()),
            ("holes_matched".into(), matched.into()),
            ("standoffs".into(), standoffs.len().into()),
            ("tolerance_mm".into(), tol.into()),
            ("worst_offset_mm".into(), round2(worst).into()),
        ])),
    }
}

/// Check 4 — edge connectors line up with wall cutouts.
fn check_connectors(
    input: &EnclosureFitInput,
    placement: &BoardPlacement,
    clearance: f64,
) -> EnclosureFitCheck {
    let empty_c: Vec<ConnectorRef> = vec![];
    let empty_o: Vec<WallOpening> = vec![];
    let conns = input.connectors.as_ref().unwrap_or(&empty_c);
    let openings = input.openings.as_ref().unwrap_or(&empty_o);
    let label = "Connectors align to wall cutouts".to_string();
    if conns.is_empty() {
        return EnclosureFitCheck {
            id: "connector_cutouts".to_string(),
            label,
            status: CheckStatus::Skip,
            detail: "Board declares no edge connectors".to_string(),
            measurements: None,
        };
    }
    // Connector world positions and which cavity wall each faces.
    let cav = &input.cavity;
    let mut aligned = 0usize;
    let mut problems: Vec<String> = Vec::new();
    for c in conns {
        let w = to_world(c.x, c.y, 0.0, placement);
        let wall_edge = nearest_edge(w.x, w.y, cav.min_x, cav.max_x, cav.min_y, cav.max_y);
        // The lateral coordinate along that wall.
        let along = if matches!(wall_edge, WallEdge::MinX | WallEdge::MaxX) {
            w.y
        } else {
            w.x
        };
        let on_wall: Vec<&WallOpening> = openings.iter().filter(|o| o.edge == wall_edge).collect();
        if on_wall.is_empty() {
            problems.push(format!(
                "{} faces the {} wall but it has no cutout",
                c.reference,
                edge_name(wall_edge)
            ));
            continue;
        }
        let lateral = |o: &WallOpening| {
            if matches!(o.edge, WallEdge::MinX | WallEdge::MaxX) {
                o.center.y
            } else {
                o.center.x
            }
        };
        let hit = on_wall
            .iter()
            .any(|o| (along - lateral(o)).abs() <= o.width / 2.0 + clearance);
        if hit {
            aligned += 1;
        } else {
            let nearest = on_wall.iter().fold(f64::INFINITY, |best, o| {
                let off = (along - lateral(o)).abs() - o.width / 2.0;
                if off < best {
                    off
                } else {
                    best
                }
            });
            problems.push(format!(
                "{} on {} misses its cutout by {}mm",
                c.reference,
                edge_name(wall_edge),
                fmt_num(round2(nearest))
            ));
        }
    }
    let ok = aligned == conns.len();
    // No openings detected anywhere is a detection gap, not a hard failure.
    let status = if ok {
        CheckStatus::Pass
    } else if openings.is_empty() {
        CheckStatus::Warn
    } else {
        CheckStatus::Fail
    };
    EnclosureFitCheck {
        id: "connector_cutouts".to_string(),
        label,
        status,
        detail: if ok {
            format!("All {} connector(s) line up with wall cutouts", conns.len())
        } else if openings.is_empty() {
            format!(
                "No wall cutouts detected; {} connector(s) would be enclosed: {}",
                conns.len(),
                problems.join("; ")
            )
        } else {
            format!(
                "{}/{} connectors aligned — {}",
                aligned,
                conns.len(),
                problems.join("; ")
            )
        },
        measurements: Some(Measurements(vec![
            ("connectors_total".into(), conns.len().into()),
            ("connectors_aligned".into(), aligned.into()),
            ("wall_openings".into(), openings.len().into()),
        ])),
    }
}

fn edge_name(e: WallEdge) -> &'static str {
    match e {
        WallEdge::MinX => "minX",
        WallEdge::MaxX => "maxX",
        WallEdge::MinY => "minY",
        WallEdge::MaxY => "maxY",
    }
}

/// Run the four cross-domain checks and assemble the verdict. Pure: pass it
/// extracted features and it returns a report — no kernel, no I/O.
pub fn check_enclosure_fit(input: &EnclosureFitInput) -> EnclosureFitReport {
    let clearance = input.clearance.unwrap_or(DEFAULT_CLEARANCE);
    let placement = input.placement.unwrap_or_else(|| auto_placement(input));

    let checks = vec![
        check_board_fit(input, &placement, clearance),
        check_lid_clearance(input, &placement, clearance),
        check_mounting_holes(input, &placement),
        check_connectors(input, &placement, clearance),
    ];

    let failed: Vec<&EnclosureFitCheck> = checks
        .iter()
        .filter(|c| c.status == CheckStatus::Fail)
        .collect();
    let warned: Vec<&EnclosureFitCheck> = checks
        .iter()
        .filter(|c| c.status == CheckStatus::Warn)
        .collect();
    let passed: Vec<&EnclosureFitCheck> = checks
        .iter()
        .filter(|c| c.status == CheckStatus::Pass)
        .collect();
    let ok = failed.is_empty();
    let verified = ok && warned.is_empty();

    let summary = if !failed.is_empty() {
        format!(
            "Enclosure fit: FAIL — {}",
            failed
                .iter()
                .map(|c| c.label.to_lowercase())
                .collect::<Vec<_>>()
                .join("; ")
        )
    } else if !warned.is_empty() {
        format!(
            "Enclosure fit: UNVERIFIED — {} passed, {} warning(s): {}",
            passed.len(),
            warned.len(),
            warned
                .iter()
                .map(|c| c.detail.clone())
                .collect::<Vec<_>>()
                .join("; ")
        )
    } else {
        format!(
            "Enclosure fit: PASS — {}/{} checks ({})",
            passed.len(),
            checks.len(),
            passed
                .iter()
                .map(|c| c.label.to_lowercase())
                .collect::<Vec<_>>()
                .join(", ")
        )
    };

    EnclosureFitReport {
        ok,
        verified,
        summary,
        clearance,
        placement,
        checks,
    }
}

// ===========================================================================
// Auto-derive a board from the cavity (the co-design starting point)
// ===========================================================================

/// Options for [`derive_board_from_cavity`].
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct DeriveBoardOptions {
    /// All-round wall clearance (mm). Default 0.5.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clearance: Option<f64>,
    /// Board thickness (mm). Default 1.6.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thickness: Option<f64>,
    /// Mounting-hole diameter (mm). Default 3.2 (M3 clearance).
    #[serde(
        rename = "holeDiameter",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub hole_diameter: Option<f64>,
    /// Board lift above the floor when there are no standoffs (mm). Default 0.
    #[serde(
        rename = "standoffHeight",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub standoff_height: Option<f64>,
}

/// A board seeded from an enclosure cavity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DerivedBoard {
    /// Rectangular outline inset by the clearance (board-local).
    pub outline: BoardOutline,
    /// Mounting holes over each detected standoff (board-local).
    #[serde(rename = "mountingHoles")]
    pub mounting_holes: Vec<MountingHole>,
    /// Placement that drops the board back into the case.
    pub placement: BoardPlacement,
}

/// Seed a board from an enclosure cavity: a rectangular outline inset by the
/// clearance, mounting holes over each detected standoff, and the placement
/// that drops it back into the case. The mirror of [`check_enclosure_fit`] —
/// derive, then verify the result holds.
pub fn derive_board_from_cavity(
    cavity: &EnclosureCavity,
    standoffs: &[Standoff],
    opts: &DeriveBoardOptions,
) -> DerivedBoard {
    let clearance = opts.clearance.unwrap_or(DEFAULT_CLEARANCE);
    let thickness = opts.thickness.unwrap_or(1.6);
    let hole_dia = opts.hole_diameter.unwrap_or(3.2);
    let w = round2(cavity.max_x - cavity.min_x - 2.0 * clearance);
    let h = round2(cavity.max_y - cavity.min_y - 2.0 * clearance);
    let outline = BoardOutline {
        vertices: vec![
            Vec2 { x: 0.0, y: 0.0 },
            Vec2 { x: w, y: 0.0 },
            Vec2 { x: w, y: h },
            Vec2 { x: 0.0, y: h },
        ],
        thickness,
    };
    let standoff_top = if !standoffs.is_empty() {
        standoffs
            .iter()
            .map(|s| s.top_z)
            .fold(f64::NEG_INFINITY, f64::max)
    } else {
        cavity.floor_z + opts.standoff_height.unwrap_or(0.0)
    };
    let off_x = cavity.min_x + clearance;
    let off_y = cavity.min_y + clearance;
    let placement = BoardPlacement {
        offset: Vec3 {
            x: round2(off_x),
            y: round2(off_y),
            z: round2(standoff_top),
        },
        rotation_deg: 0.0,
    };
    // Holes in board-local coords; keep only those inside the outline.
    let mut mounting_holes = Vec::new();
    for s in standoffs {
        let lx = round2(s.x - off_x);
        let ly = round2(s.y - off_y);
        if lx >= 0.0 && lx <= w && ly >= 0.0 && ly <= h {
            mounting_holes.push(MountingHole {
                x: lx,
                y: ly,
                diameter: hole_dia,
                reference: None,
            });
        }
    }
    DerivedBoard {
        outline,
        mounting_holes,
        placement,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cavity() -> EnclosureCavity {
        EnclosureCavity {
            min_x: 1.0,
            max_x: 39.0,
            min_y: 1.0,
            max_y: 39.0,
            floor_z: 2.0,
            ceil_z: 12.0,
            has_lid: false,
        }
    }

    fn standoffs() -> Vec<Standoff> {
        [(4.75, 4.75), (35.25, 4.75), (4.75, 35.25), (35.25, 35.25)]
            .iter()
            .map(|&(x, y)| Standoff {
                x,
                y,
                top_z: 5.0,
                radius: 1.6,
            })
            .collect()
    }

    fn openings() -> Vec<WallOpening> {
        vec![WallOpening {
            edge: WallEdge::MaxX,
            center: Vec2 { x: 38.0, y: 20.0 },
            width: 10.0,
            z_min: 2.0,
            z_max: 12.0,
        }]
    }

    fn outline() -> BoardOutline {
        BoardOutline {
            vertices: vec![
                Vec2 { x: 0.0, y: 0.0 },
                Vec2 { x: 36.0, y: 0.0 },
                Vec2 { x: 36.0, y: 36.0 },
                Vec2 { x: 0.0, y: 36.0 },
            ],
            thickness: 1.6,
        }
    }

    fn mounting_holes() -> Vec<MountingHole> {
        [(2.75, 2.75), (33.25, 2.75), (2.75, 33.25), (33.25, 33.25)]
            .iter()
            .map(|&(x, y)| MountingHole {
                x,
                y,
                diameter: 3.2,
                reference: None,
            })
            .collect()
    }

    fn placement() -> BoardPlacement {
        BoardPlacement {
            offset: Vec3 {
                x: 2.0,
                y: 2.0,
                z: 5.0,
            },
            rotation_deg: 0.0,
        }
    }

    fn component_extents() -> Vec<ComponentExtent> {
        vec![
            ComponentExtent {
                reference: "U1".into(),
                front: true,
                top_z: 1.6 + 1.0,
                bottom_z: 1.6,
            },
            ComponentExtent {
                reference: "J1".into(),
                front: true,
                top_z: 1.6 + 3.0,
                bottom_z: 1.6,
            },
        ]
    }

    fn connectors() -> Vec<ConnectorRef> {
        vec![ConnectorRef {
            reference: "J1".into(),
            x: 36.0,
            y: 18.0,
            edge: Some(WallEdge::MaxX),
            height: 3.0,
        }]
    }

    fn base_input() -> EnclosureFitInput {
        EnclosureFitInput {
            outline: outline(),
            cavity: cavity(),
            standoffs: None,
            openings: None,
            mounting_holes: None,
            connectors: None,
            component_extents: None,
            placement: Some(placement()),
            clearance: Some(0.5),
            standoff_height: None,
            hole_tolerance: None,
        }
    }

    fn status_of<'a>(r: &'a EnclosureFitReport, id: &str) -> &'a EnclosureFitCheck {
        r.checks.iter().find(|c| c.id == id).unwrap()
    }

    #[test]
    fn passes_a_fitting_board() {
        let mut input = base_input();
        input.standoffs = Some(standoffs());
        input.openings = Some(openings());
        input.mounting_holes = Some(mounting_holes());
        input.connectors = Some(connectors());
        input.component_extents = Some(component_extents());
        let r = check_enclosure_fit(&input);
        assert!(r.ok);
        assert!(r.verified);
        assert!(r.summary.contains("PASS"), "{}", r.summary);
        for id in [
            "board_fit",
            "lid_clearance",
            "mounting_holes",
            "connector_cutouts",
        ] {
            assert_eq!(status_of(&r, id).status, CheckStatus::Pass, "{id}");
        }
        assert_eq!(
            status_of(&r, "mounting_holes")
                .measurements
                .as_ref()
                .unwrap()
                .get("holes_matched"),
            Some(&MeasurementValue::Num(4.0))
        );
    }

    #[test]
    fn fails_on_overhang() {
        let mut input = base_input();
        input.outline.vertices = outline()
            .vertices
            .iter()
            .map(|v| Vec2 {
                x: v.x * 1.2,
                y: v.y * 1.2,
            })
            .collect();
        let r = check_enclosure_fit(&input);
        let fit = status_of(&r, "board_fit");
        assert_eq!(fit.status, CheckStatus::Fail);
        assert!(!r.ok);
        assert!(fit.detail.contains("overhangs"), "{}", fit.detail);
    }

    #[test]
    fn fails_when_component_punches_lid() {
        let mut input = base_input();
        input.component_extents = Some(vec![ComponentExtent {
            reference: "C1".into(),
            front: true,
            top_z: 1.6 + 12.0,
            bottom_z: 1.6,
        }]);
        let r = check_enclosure_fit(&input);
        let lid = status_of(&r, "lid_clearance");
        assert_eq!(lid.status, CheckStatus::Fail);
        assert!(lid.detail.contains("C1"), "{}", lid.detail);
    }

    #[test]
    fn fails_when_hole_misses_standoffs() {
        let mut input = base_input();
        input.clearance = None;
        input.standoffs = Some(standoffs());
        input.mounting_holes = Some(vec![MountingHole {
            x: 10.0,
            y: 10.0,
            diameter: 3.2,
            reference: Some("H1".into()),
        }]);
        let r = check_enclosure_fit(&input);
        let mh = status_of(&r, "mounting_holes");
        assert_eq!(mh.status, CheckStatus::Fail);
        assert_eq!(
            mh.measurements.as_ref().unwrap().get("holes_matched"),
            Some(&MeasurementValue::Num(0.0))
        );
    }

    #[test]
    fn warns_when_no_cutouts_exist() {
        let mut input = base_input();
        input.clearance = None;
        input.connectors = Some(connectors());
        input.openings = Some(vec![]);
        let r = check_enclosure_fit(&input);
        assert_eq!(status_of(&r, "connector_cutouts").status, CheckStatus::Warn);
        assert!(!r.verified);
        assert!(r.ok); // a warning is not a hard failure
    }

    #[test]
    fn skips_checks_with_absent_inputs() {
        let input = base_input();
        let r = check_enclosure_fit(&input);
        for id in ["lid_clearance", "mounting_holes", "connector_cutouts"] {
            assert_eq!(status_of(&r, id).status, CheckStatus::Skip, "{id}");
        }
    }

    #[test]
    fn auto_fit_centers_on_standoffs() {
        let mut input = base_input();
        input.placement = None;
        input.clearance = None;
        input.standoffs = Some(standoffs());
        input.mounting_holes = Some(mounting_holes());
        let r = check_enclosure_fit(&input);
        assert!((r.placement.offset.z - 5.0).abs() < 0.05);
        assert_eq!(status_of(&r, "board_fit").status, CheckStatus::Pass);
    }

    #[test]
    fn derive_round_trips_through_check() {
        let d = derive_board_from_cavity(
            &cavity(),
            &standoffs(),
            &DeriveBoardOptions {
                clearance: Some(0.5),
                hole_diameter: Some(3.2),
                ..Default::default()
            },
        );
        assert!((d.outline.vertices[2].x - 37.0).abs() < 0.05); // 38 - 2*0.5
        assert_eq!(d.mounting_holes.len(), 4);
        let input = EnclosureFitInput {
            outline: d.outline,
            cavity: cavity(),
            standoffs: Some(standoffs()),
            openings: None,
            mounting_holes: Some(d.mounting_holes),
            connectors: None,
            component_extents: None,
            placement: Some(d.placement),
            clearance: Some(0.5),
            standoff_height: None,
            hole_tolerance: None,
        };
        let r = check_enclosure_fit(&input);
        assert_eq!(status_of(&r, "board_fit").status, CheckStatus::Pass);
        assert_eq!(status_of(&r, "mounting_holes").status, CheckStatus::Pass);
    }

    #[test]
    fn report_serializes_with_ts_field_names() {
        let mut input = base_input();
        input.standoffs = Some(standoffs());
        input.mounting_holes = Some(mounting_holes());
        let r = check_enclosure_fit(&input);
        // Round-trip through the wire *text* (serde_json::to_value would
        // alphabetize the measurement map; the string path preserves order).
        let text = serde_json::to_string(&r).unwrap();
        assert!(
            text.contains("\"worst_clearance_mm\":1.0,\"margin_minus_x\""),
            "measurement insertion order lost: {text}"
        );
        assert!(text.contains("\"rotationDeg\":0.0"));
        let back: EnclosureFitReport = serde_json::from_str(&text).unwrap();
        assert_eq!(back, r);
        // Input deserializes from TS-shaped JSON.
        let input_json = serde_json::json!({
            "outline": { "vertices": [{"x": 0.0, "y": 0.0}], "thickness": 1.6 },
            "cavity": { "minX": 0.0, "maxX": 1.0, "minY": 0.0, "maxY": 1.0,
                         "floorZ": 0.0, "ceilZ": 1.0, "hasLid": false },
            "mountingHoles": [{"x": 1.0, "y": 2.0, "diameter": 3.2, "ref": "H1"}],
            "holeTolerance": 0.7
        });
        let parsed: EnclosureFitInput = serde_json::from_value(input_json).unwrap();
        assert_eq!(parsed.hole_tolerance, Some(0.7));
        assert_eq!(
            parsed.mounting_holes.unwrap()[0].reference.as_deref(),
            Some("H1")
        );
    }

    #[test]
    fn round2_matches_js_math_round() {
        assert_eq!(crate::round2(2.675), 2.68); // 2.675*100 = 267.49999... in fp
        assert_eq!(crate::round2(-0.005), -0.0); // JS Math.round(-0.5) → -0
        assert_eq!(crate::round2(1.0 / 3.0), 0.33);
    }
}
