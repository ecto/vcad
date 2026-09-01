//! Assembly mates: declarative checks over a **posed** assembly.
//!
//! A mate is not a constraint the solver satisfies — it is an assertion the
//! author writes down and the checker re-verifies against the poses that are
//! already in the document. The document's instance transforms stay the single
//! source of truth; a mate says what those transforms are *supposed* to
//! achieve, so a wrong transform is caught at model time instead of at glue-up.
//!
//! Three kinds ship today:
//!
//! - [`MateKind::Coaxial`] — two parts' reference axes are the same line.
//! - [`MateKind::PlanarOffset`] — two parts sit a stated distance apart along
//!   an axis (the z-stack of a layered machine, written down once).
//! - [`MateKind::PatternPhase`] — two parts each carrying an `n`-fold circular
//!   pattern are **phase-aligned** under whatever flip/clock the poses apply.
//!
//! `PatternPhase` is the one that pays for the module. A 10-pole rotor pair
//! assembled "flipped, clocked 60°" misaligns its poles by 12° (60 mod 36 =
//! 24, wrapped to ±18 → −12°) — arithmetic that is invisible in prose and
//! trivially wrong by hand. Clocking 180° gives 180 mod 36 = 0: exact
//! alignment. The checker does that modular arithmetic from the poses.
//!
//! Kinematic degrees of freedom are deliberately **out of scope** here: a mate
//! never moves anything, and nothing in this module knows about joints. Motion
//! stays with [`crate::Joint`].

use serde::{Deserialize, Serialize};

use crate::Vec3;

/// Default angular tolerance for mate checks, in degrees.
pub const DEFAULT_ANGULAR_TOLERANCE_DEG: f64 = 0.5;

/// Default linear tolerance for mate checks, in millimeters.
pub const DEFAULT_LINEAR_TOLERANCE_MM: f64 = 0.01;

/// A declarative mate between two instances of an assembly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub struct Mate {
    /// Unique identifier, used to name the check in reports and receipts.
    pub id: String,
    /// Optional human-readable name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-rs", ts(optional))]
    pub name: Option<String>,
    /// First instance id.
    #[serde(rename = "instanceA")]
    #[cfg_attr(feature = "ts-rs", ts(rename = "instanceA"))]
    pub instance_a: String,
    /// Second instance id.
    #[serde(rename = "instanceB")]
    #[cfg_attr(feature = "ts-rs", ts(rename = "instanceB"))]
    pub instance_b: String,
    /// What is being asserted.
    pub kind: MateKind,
}

/// The assertion a [`Mate`] makes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub enum MateKind {
    /// The two parts' reference axes lie on one line.
    ///
    /// Each part's reference axis is `axis` in that part's own local frame,
    /// passing through the part's local origin. The check compares the posed
    /// directions (parallel *or* antiparallel — a flipped part is still
    /// coaxial) and the perpendicular distance between the two lines.
    Coaxial {
        /// Reference axis in each part's local frame. Normalized on use.
        axis: Vec3,
        /// Maximum permitted perpendicular distance between the axis lines, mm.
        #[serde(default = "default_linear_tol", rename = "toleranceMm")]
        #[cfg_attr(feature = "ts-rs", ts(rename = "toleranceMm"))]
        tolerance_mm: f64,
        /// Maximum permitted angle between the axis directions, degrees.
        #[serde(default = "default_angular_tol", rename = "toleranceDeg")]
        #[cfg_attr(feature = "ts-rs", ts(rename = "toleranceDeg"))]
        tolerance_deg: f64,
    },
    /// Instance B's origin sits `offset` mm from instance A's origin, measured
    /// along `axis` in world coordinates.
    ///
    /// This is the z-stack of a layered machine written down as data: the
    /// number in the design table and the number in the transform can no
    /// longer disagree silently.
    PlanarOffset {
        /// World-frame axis the offset is measured along. Normalized on use.
        axis: Vec3,
        /// Expected signed distance from A to B along `axis`, mm.
        offset: f64,
        /// Maximum permitted deviation from `offset`, mm.
        #[serde(default = "default_linear_tol", rename = "toleranceMm")]
        #[cfg_attr(feature = "ts-rs", ts(rename = "toleranceMm"))]
        tolerance_mm: f64,
    },
    /// Two parts carrying `n_fold` circular patterns about `axis` are
    /// phase-aligned once posed.
    ///
    /// Both patterns are described relative to a **reference feature
    /// direction**: local `+X`, rotated by `phase_a_deg` / `phase_b_deg` about
    /// `axis`. Pattern features therefore sit at
    /// `phase + k · 360/n_fold` for integer `k`.
    ///
    /// The check poses both reference directions, projects them onto the plane
    /// perpendicular to the world axis, and requires the two world phases to
    /// agree **modulo the pattern pitch** `360/n_fold`. Because the feature set
    /// is invariant under a whole-pitch shift, the residue is folded into
    /// `(−pitch/2, pitch/2]` and compared against `tolerance_deg`.
    PatternPhase {
        /// Number of features in each part's circular pattern. Must be ≥ 1.
        #[serde(rename = "nFold")]
        #[cfg_attr(feature = "ts-rs", ts(rename = "nFold"))]
        n_fold: u32,
        /// Pattern axis in each part's local frame. Normalized on use.
        axis: Vec3,
        /// Angular offset of A's first feature from local `+X`, degrees.
        #[serde(default, rename = "phaseADeg")]
        #[cfg_attr(feature = "ts-rs", ts(rename = "phaseADeg"))]
        phase_a_deg: f64,
        /// Angular offset of B's first feature from local `+X`, degrees.
        #[serde(default, rename = "phaseBDeg")]
        #[cfg_attr(feature = "ts-rs", ts(rename = "phaseBDeg"))]
        phase_b_deg: f64,
        /// Optional documented clocking. When present, the measured relative
        /// rotation of B about the world axis must equal this (modulo 360)
        /// within `tolerance_deg` — catching a design table and a transform
        /// that have drifted apart, independently of whether the poles happen
        /// to line up.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            rename = "expectedClockDeg"
        )]
        #[cfg_attr(feature = "ts-rs", ts(rename = "expectedClockDeg", optional))]
        expected_clock_deg: Option<f64>,
        /// Maximum permitted phase error, degrees.
        #[serde(default = "default_angular_tol", rename = "toleranceDeg")]
        #[cfg_attr(feature = "ts-rs", ts(rename = "toleranceDeg"))]
        tolerance_deg: f64,
    },
}

fn default_linear_tol() -> f64 {
    DEFAULT_LINEAR_TOLERANCE_MM
}

fn default_angular_tol() -> f64 {
    DEFAULT_ANGULAR_TOLERANCE_DEG
}

impl MateKind {
    /// Short kind name for reports (`"coaxial"`, `"planar-offset"`,
    /// `"pattern-phase"`).
    pub fn label(&self) -> &'static str {
        match self {
            MateKind::Coaxial { .. } => "coaxial",
            MateKind::PlanarOffset { .. } => "planar-offset",
            MateKind::PatternPhase { .. } => "pattern-phase",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mate_round_trips_through_json() {
        let mate = Mate {
            id: "rotor-poles".into(),
            name: None,
            instance_a: "rotor-rear".into(),
            instance_b: "rotor-front".into(),
            kind: MateKind::PatternPhase {
                n_fold: 10,
                axis: Vec3::new(0.0, 0.0, 1.0),
                phase_a_deg: 0.0,
                phase_b_deg: 0.0,
                expected_clock_deg: Some(180.0),
                tolerance_deg: 0.5,
            },
        };
        let json = serde_json::to_string(&mate).unwrap();
        assert!(json.contains("\"nFold\":10"));
        let back: Mate = serde_json::from_str(&json).unwrap();
        assert_eq!(mate, back);
    }

    #[test]
    fn tolerances_default_when_absent() {
        let json = r#"{
            "id": "m", "instanceA": "a", "instanceB": "b",
            "kind": {"type": "Coaxial", "axis": {"x":0,"y":0,"z":1}}
        }"#;
        let mate: Mate = serde_json::from_str(json).unwrap();
        match mate.kind {
            MateKind::Coaxial {
                tolerance_mm,
                tolerance_deg,
                ..
            } => {
                assert_eq!(tolerance_mm, DEFAULT_LINEAR_TOLERANCE_MM);
                assert_eq!(tolerance_deg, DEFAULT_ANGULAR_TOLERANCE_DEG);
            }
            _ => panic!("wrong kind"),
        }
    }
}
