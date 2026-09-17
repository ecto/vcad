//! Where the part sits on the stock.
//!
//! A part is modelled in its own frame and clamped down in another. The blank
//! is a millimetre proud on one side, the vice jaw is not square to the table,
//! the operator nudged the sheet: the geometry that has to be cut is the
//! modelled geometry *moved*. Until now the only way to say so was to rewrite
//! every coordinate before posting the job, which is exactly the kind of
//! arithmetic that gets done once, by hand, and then not redone when the
//! outline changes.
//!
//! `placement { dx, dy, rotation_deg }` says it once. The geometry of an
//! operation is **rotated `rotation_deg` counter-clockwise about the origin of
//! the stock frame and then shifted by `(dx, dy)`** — rotate, then translate,
//! in that order, which is what "the part is placed at an angle over there"
//! means and what a two-point skew probe hands back.
//!
//! # What moves with it
//!
//! The job-level `options.placement` moves the operations **and the part the
//! verification replays against**, so a skewed job still verifies against the
//! skewed part. A per-operation `placement` overrides it for that operation's
//! geometry *only* — the part does not follow one operation — and the response
//! says so in a note, because an operation that has moved away from the part
//! the oracle is checking will read as a gouge, which is the correct answer to
//! the wrong question.
//!
//! A part the job derives from its own operations is already in stock
//! coordinates by the time it is read, so it is never transformed twice.

use serde::Deserialize;

use crate::types::finite;

/// `{ "dx": 0.0, "dy": 0.0, "rotation_deg": 0.0 }`.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct PlacementReq {
    /// Shift along X after the rotation, mm.
    #[serde(default)]
    pub dx: Option<f64>,
    /// Shift along Y after the rotation, mm.
    #[serde(default)]
    pub dy: Option<f64>,
    /// Rotation about the stock-frame origin, degrees counter-clockwise.
    #[serde(default)]
    pub rotation_deg: Option<f64>,
}

impl PlacementReq {
    /// Validate and convert.
    pub fn build(&self, what: &str) -> Result<Placement, String> {
        Ok(Placement::new(
            finite(&format!("{what}.dx"), self.dx.unwrap_or(0.0))?,
            finite(&format!("{what}.dy"), self.dy.unwrap_or(0.0))?,
            finite(
                &format!("{what}.rotation_deg"),
                self.rotation_deg.unwrap_or(0.0),
            )?,
        ))
    }
}

/// A rigid placement in the XY plane: rotate about the origin, then translate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    /// Shift along X, mm.
    pub dx: f64,
    /// Shift along Y, mm.
    pub dy: f64,
    /// Rotation about the stock-frame origin, degrees counter-clockwise.
    pub rotation_deg: f64,
    cos: f64,
    sin: f64,
}

impl Default for Placement {
    fn default() -> Self {
        Self::identity()
    }
}

impl Placement {
    /// Build from millimetres and degrees.
    pub fn new(dx: f64, dy: f64, rotation_deg: f64) -> Self {
        let radians = rotation_deg.to_radians();
        Self {
            dx,
            dy,
            rotation_deg,
            cos: radians.cos(),
            sin: radians.sin(),
        }
    }

    /// The placement that moves nothing.
    pub fn identity() -> Self {
        Self::new(0.0, 0.0, 0.0)
    }

    /// Whether this placement moves anything at all.
    pub fn is_identity(&self) -> bool {
        self.dx == 0.0 && self.dy == 0.0 && self.rotation_deg == 0.0
    }

    /// Whether this placement turns the part. A rotation is the part of a
    /// placement an axis-aligned rectangle cannot survive.
    pub fn rotates(&self) -> bool {
        self.rotation_deg != 0.0
    }

    /// Move one point.
    pub fn apply(&self, p: [f64; 2]) -> [f64; 2] {
        [
            self.cos * p[0] - self.sin * p[1] + self.dx,
            self.sin * p[0] + self.cos * p[1] + self.dy,
        ]
    }

    /// Move a loop.
    pub fn apply_loop(&self, points: &[[f64; 2]]) -> Vec<[f64; 2]> {
        points.iter().map(|p| self.apply(*p)).collect()
    }

    /// How a caller would describe this placement in a note.
    pub fn describe(&self) -> String {
        format!(
            "{:+.3} mm in X, {:+.3} mm in Y, {:+.3}° about the stock origin",
            self.dx, self.dy, self.rotation_deg
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quarter_turn_sends_x_to_y() {
        let p = Placement::new(0.0, 0.0, 90.0);
        let moved = p.apply([10.0, 0.0]);
        assert!(
            moved[0].abs() < 1e-12 && (moved[1] - 10.0).abs() < 1e-12,
            "90° about the origin sends (10, 0) to (0, 10), not {moved:?}"
        );
    }

    #[test]
    fn the_rotation_happens_before_the_shift() {
        // Were it the other way round the answer would be (0, 11).
        let p = Placement::new(1.0, 0.0, 90.0);
        let moved = p.apply([10.0, 0.0]);
        assert!(
            (moved[0] - 1.0).abs() < 1e-12 && (moved[1] - 10.0).abs() < 1e-12,
            "rotate then translate puts (10, 0) at (1, 10), not {moved:?}"
        );
    }

    #[test]
    fn a_placement_preserves_distance() {
        let p = Placement::new(-3.25, 7.5, 33.0);
        let (a, b) = ([2.0f64, 1.0], [9.0f64, -4.0]);
        let before = (a[0] - b[0]).hypot(a[1] - b[1]);
        let (a, b) = (p.apply(a), p.apply(b));
        let after = (a[0] - b[0]).hypot(a[1] - b[1]);
        assert!(
            (before - after).abs() < 1e-12,
            "a rigid placement cannot resize the part: {before} became {after}"
        );
    }
}
