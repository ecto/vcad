//! The material under the cutter: the stock, what is under it, and how far
//! past the part the job is allowed to go.
//!
//! Wave 1 wrote this three times. `operation::drill` had a `Spoilboard`
//! struct, `operation::contour` had a bare `spoilboard_thickness: Option<f64>`
//! and [`verify2d::JobSpec`](crate::verify2d::JobSpec) has a `spoilboard:
//! bool` — three spellings of one fact, and the rule they enforce (a cut past
//! the underside of the stock is only legal into a declared sacrificial board
//! at least that thick) written out separately each time. It is written here
//! once.

use serde::{Deserialize, Serialize};

/// A sacrificial board under the stock. Cutting past the underside of the
/// stock is only legal into one of these.
///
/// Deserialises from either `{"thickness": 3.0}` or a bare `3.0`, so the
/// shape a caller had before this type existed still loads.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Spoilboard {
    /// Thickness of the sacrificial material, in mm.
    pub thickness: f64,
}

impl Spoilboard {
    /// Declare a spoilboard of the given thickness.
    pub fn new(thickness: f64) -> Self {
        Self { thickness }
    }
}

impl From<f64> for Spoilboard {
    fn from(thickness: f64) -> Self {
        Self::new(thickness)
    }
}

impl<'de> Deserialize<'de> for Spoilboard {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        /// The old spelling was the thickness on its own.
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Either {
            Thickness(f64),
            Full { thickness: f64 },
        }
        Ok(match Either::deserialize(deserializer)? {
            Either::Thickness(thickness) | Either::Full { thickness } => Self::new(thickness),
        })
    }
}

/// The material the job is cut from.
///
/// Z0 is the stock top and the underside sits at `-thickness`, the house
/// convention everything else in this crate already assumes. The outline is
/// optional: when it is not given, callers grow the part's own bounds by
/// [`Stock::margin`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Stock {
    /// Thickness in mm. The underside is at Z = `-thickness`.
    pub thickness: f64,
    /// Outline in the stock frame, `[min_x, min_y, max_x, max_y]` in mm.
    #[serde(default)]
    pub bbox: Option<[f64; 4]>,
    /// Material assumed around the part when `bbox` is not given, in mm.
    #[serde(default = "default_margin")]
    pub margin: f64,
    /// What is under the stock, when anything is.
    #[serde(default)]
    pub spoilboard: Option<Spoilboard>,
}

fn default_margin() -> f64 {
    5.0
}

impl Stock {
    /// Stock of the given thickness, with no outline and nothing under it.
    pub fn new(thickness: f64) -> Self {
        Self {
            thickness,
            bbox: None,
            margin: default_margin(),
            spoilboard: None,
        }
    }

    /// Give the stock an outline in the stock frame.
    pub fn with_bbox(mut self, bbox: [f64; 4]) -> Self {
        self.bbox = Some(bbox);
        self
    }

    /// Set the material assumed around the part when there is no outline.
    pub fn with_margin(mut self, margin: f64) -> Self {
        self.margin = margin;
        self
    }

    /// Declare the sacrificial board under the stock.
    pub fn over(mut self, spoilboard: impl Into<Spoilboard>) -> Self {
        self.spoilboard = Some(spoilboard.into());
        self
    }

    /// Thickness of the declared spoilboard, or zero when none is declared.
    pub fn spoilboard_thickness(&self) -> f64 {
        self.spoilboard.map_or(0.0, |s| s.thickness)
    }

    /// Z of the underside of the stock (negative).
    pub fn underside_z(&self) -> f64 {
        -self.thickness
    }

    /// The outline, falling back to the part's bounds grown by
    /// [`Stock::margin`].
    pub fn bbox_or_around(&self, part_bbox: [f64; 4]) -> [f64; 4] {
        self.bbox.unwrap_or([
            part_bbox[0] - self.margin,
            part_bbox[1] - self.margin,
            part_bbox[2] + self.margin,
            part_bbox[3] + self.margin,
        ])
    }

    /// The verification oracle's view of this stock: a [`JobSpec`] ready for
    /// [`verify_toolpath`](crate::verify2d::verify_toolpath).
    ///
    /// The bridge exists so the FFI does not have to know that the oracle
    /// spells a spoilboard as a `bool` and this side spells it as a
    /// thickness.
    pub fn job_spec(
        &self,
        part: crate::verify2d::PartRegion,
        tool_diameter: f64,
        allowance: BottomAllowance,
    ) -> crate::verify2d::JobSpec {
        let mut spec = crate::verify2d::JobSpec::new(part, self.thickness, tool_diameter);
        spec.stock_bbox = Some(self.bbox_or_around(part_bbox(&spec)));
        spec.bottom_allowance = allowance.0;
        spec.spoilboard = self.spoilboard.is_some();
        spec
    }
}

/// The part's own bounds, for the fallback outline.
fn part_bbox(spec: &crate::verify2d::JobSpec) -> [f64; 4] {
    spec.part.bbox()
}

/// How far the cut stops short of the underside of the stock, in mm.
///
/// One sign convention, stated once and shared by every operation that has a
/// floor:
///
/// - **positive** leaves an onion skin: the cut stops that far *above* the
///   depth asked for, and the part is still held by a membrane.
/// - **zero** cuts to the depth asked for and no further.
/// - **negative** is a deliberate break-through: the cutter sinks that far
///   *past* the underside so the wall is open at full depth. This is only
///   legal into a spoilboard at least that thick — the check is
///   [`BottomAllowance::check`], and it fails closed.
///
/// Drilling spells the same idea as [`crate::BreakThrough`], which adds the
/// drill's own point length to the sink; the allowance here is what is asked
/// for *beyond* that.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BottomAllowance(pub f64);

/// Why a bottom allowance was refused.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AllowanceRefusal {
    /// The cut goes `overcut` mm past the underside of the stock, and the
    /// declared spoilboard is only `spoilboard` mm thick.
    BreakThroughWithoutSpoilboard {
        /// How far past the underside the cut would go, in mm.
        overcut: f64,
        /// Thickness declared, in mm.
        spoilboard: f64,
    },
    /// The skin is as thick as the part: nothing would be cut.
    ExceedsDepth {
        /// The allowance asked for, in mm.
        allowance: f64,
        /// The depth asked for, in mm.
        depth: f64,
    },
}

impl BottomAllowance {
    /// No allowance: cut to the depth asked for.
    pub const NONE: Self = Self(0.0);

    /// Leave `skin` mm of material below the cut.
    pub fn skin(skin: f64) -> Self {
        Self(skin)
    }

    /// Cut `overcut` mm past the underside of the stock.
    pub fn break_through(overcut: f64) -> Self {
        Self(-overcut)
    }

    /// Whether this allowance cuts past the underside of the stock.
    pub fn is_break_through(&self) -> bool {
        self.0 < 0.0
    }

    /// How far past the underside the cut goes (zero when it does not).
    pub fn overcut(&self) -> f64 {
        (-self.0).max(0.0)
    }

    /// Depth actually cut, given the depth asked for.
    pub fn final_depth(&self, depth: f64) -> f64 {
        depth - self.0
    }

    /// The whole rule in one place: a break-through needs a board that can
    /// take it, and a skin may not eat the whole cut.
    pub fn check(
        &self,
        depth: f64,
        spoilboard: Option<Spoilboard>,
    ) -> Result<f64, AllowanceRefusal> {
        if self.is_break_through() {
            let overcut = self.overcut();
            let declared = spoilboard.map_or(0.0, |s| s.thickness);
            if declared < overcut - 1e-9 {
                return Err(AllowanceRefusal::BreakThroughWithoutSpoilboard {
                    overcut,
                    spoilboard: declared,
                });
            }
        }
        let final_depth = self.final_depth(depth);
        if final_depth <= 1e-9 {
            return Err(AllowanceRefusal::ExceedsDepth {
                allowance: self.0,
                depth,
            });
        }
        Ok(final_depth)
    }
}

impl From<f64> for BottomAllowance {
    fn from(value: f64) -> Self {
        Self(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape `Spoilboard` had before it moved here was a bare number on
    /// `Contour2D`; both spellings have to load and mean the same thing.
    #[test]
    fn a_spoilboard_loads_from_a_bare_thickness_or_a_struct() {
        let bare: Spoilboard = serde_json::from_str("3.0").unwrap();
        let full: Spoilboard = serde_json::from_str(r#"{"thickness": 3.0}"#).unwrap();
        assert_eq!(bare, full);
        assert!((bare.thickness - 3.0).abs() < 1e-12);
        // And it still writes the struct form.
        assert_eq!(
            serde_json::to_string(&bare).unwrap(),
            r#"{"thickness":3.0}"#
        );
    }

    /// A break-through into nothing is refused, into a board that is too thin
    /// is refused, and into one that can take it is allowed — and the numbers
    /// in the refusal are the ones a machinist needs.
    #[test]
    fn a_break_through_needs_a_board_that_can_take_it() {
        let allowance = BottomAllowance::break_through(0.3);
        assert_eq!(
            allowance.check(5.0, None),
            Err(AllowanceRefusal::BreakThroughWithoutSpoilboard {
                overcut: 0.3,
                spoilboard: 0.0
            })
        );
        assert_eq!(
            allowance.check(5.0, Some(Spoilboard::new(0.2))),
            Err(AllowanceRefusal::BreakThroughWithoutSpoilboard {
                overcut: 0.3,
                spoilboard: 0.2
            })
        );
        assert_eq!(allowance.check(5.0, Some(Spoilboard::new(3.0))), Ok(5.3));
    }

    /// A skin as thick as the part leaves nothing to cut.
    #[test]
    fn a_skin_may_not_eat_the_whole_cut() {
        assert_eq!(
            BottomAllowance::skin(5.0).check(5.0, None),
            Err(AllowanceRefusal::ExceedsDepth {
                allowance: 5.0,
                depth: 5.0
            })
        );
        assert_eq!(BottomAllowance::skin(0.2).check(5.0, None), Ok(4.8));
    }

    /// The bridge to the verification oracle: the FFI builds a `JobSpec` from
    /// the shared types without knowing how the oracle spells any of it.
    #[test]
    fn stock_builds_the_oracle_a_job_spec() {
        let part = crate::verify2d::PartRegion::new(
            vec![[10.0, 10.0], [40.0, 10.0], [40.0, 30.0], [10.0, 30.0]],
            Vec::new(),
        )
        .unwrap();
        let stock = Stock::new(6.0).with_bbox([0.0, 0.0, 50.0, 40.0]).over(3.0);
        let spec = stock.job_spec(part.clone(), 3.175, BottomAllowance::break_through(0.3));

        assert!((spec.stock_thickness - 6.0).abs() < 1e-12);
        assert_eq!(spec.stock_bbox, Some([0.0, 0.0, 50.0, 40.0]));
        assert!((spec.tool_diameter - 3.175).abs() < 1e-12);
        assert!((spec.bottom_allowance + 0.3).abs() < 1e-12);
        assert!(spec.spoilboard, "a declared board is a declared board");
        // The floor the job means to reach: 0.3 mm past the underside.
        assert!((spec.floor_z() + 6.3).abs() < 1e-12);

        // No board declared, no break-through allowed to claim one.
        let bare = Stock::new(6.0).job_spec(part, 3.175, BottomAllowance::skin(0.2));
        assert!(!bare.spoilboard);
        assert!((bare.floor_z() + 5.8).abs() < 1e-12);
        // And with no outline, the oracle gets the part plus the margin.
        assert_eq!(bare.stock_bbox, Some([5.0, 5.0, 45.0, 35.0]));
    }

    /// The outline falls back to the part's bounds grown by the margin, and a
    /// declared outline wins.
    #[test]
    fn stock_bounds_fall_back_to_the_part_plus_margin() {
        let stock = Stock::new(6.0).with_margin(2.0);
        assert_eq!(
            stock.bbox_or_around([10.0, 10.0, 20.0, 30.0]),
            [8.0, 8.0, 22.0, 32.0]
        );
        let declared = stock.with_bbox([0.0, 0.0, 100.0, 100.0]);
        assert_eq!(
            declared.bbox_or_around([10.0, 10.0, 20.0, 30.0]),
            [0.0, 0.0, 100.0, 100.0]
        );
        assert!((declared.underside_z() + 6.0).abs() < 1e-12);
    }
}
