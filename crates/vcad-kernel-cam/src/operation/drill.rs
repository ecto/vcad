//! Hole making: spot, drill, peck, chip-break, and helical boring.
//!
//! Every cycle here plunges, so every cycle asks the tool whether it cuts
//! across its own centre and how long its flutes are, and refuses when the
//! answer is missing rather than guessing (a flat end mill that is not
//! centre-cutting rubs a plug of metal until it snaps).
//!
//! [`HelicalBore`] is the way a hole larger than the cutter gets made: the
//! stator's Ø2.5 pilots with a Ø2 end mill, spiralled down and finished at
//! full depth, instead of an inside contour whose 0.25 mm offset the cutter
//! cannot follow.

use crate::operation::Point2D;
use crate::ArcDir;
use crate::{CamSettings, CutContext, Tool, ToolGeometry, Toolpath, ToolpathSegment};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Upper bound on the pecks in one hole, so a tiny peck depth cannot turn
/// into an unbounded program.
const MAX_PECKS: usize = 2000;

/// Errors from hole-making operations.
#[derive(Debug, Clone, Error, PartialEq)]
pub enum DrillError {
    /// The operation has no holes.
    #[error("no holes to make")]
    NoHoles,

    /// A depth (shared or per hole) is not a usable number.
    #[error("invalid depth: {0} (must be finite and > 0)")]
    InvalidDepth(f64),

    /// A per-hole depth is not a usable number.
    #[error("hole {index} at ({x:.3}, {y:.3}) has an invalid depth: {depth}")]
    InvalidHoleDepth {
        /// Index of the hole in the operation.
        index: usize,
        /// Hole centre X.
        x: f64,
        /// Hole centre Y.
        y: f64,
        /// The offending depth.
        depth: f64,
    },

    /// A hole centre is not a usable number.
    #[error("hole {index} has a non-finite centre ({x}, {y})")]
    InvalidHoleCentre {
        /// Index of the hole in the operation.
        index: usize,
        /// Hole centre X.
        x: f64,
        /// Hole centre Y.
        y: f64,
    },

    /// The clearance (R) plane is not above the stock top.
    #[error("invalid clearance plane: {0} (must be finite and > 0, above the stock top)")]
    InvalidClearance(f64),

    /// The peck depth is not usable.
    #[error("invalid peck depth: {0} (must be finite and > 0)")]
    InvalidPeckDepth(f64),

    /// The chip-break retreat is not usable.
    #[error("invalid chip-break retreat: {0} (must be finite and > 0)")]
    InvalidRetreat(f64),

    /// The gap kept above the last peck when rapiding back down is not usable.
    #[error("invalid peck clearance: {0} (must be finite and > 0)")]
    InvalidPeckClearance(f64),

    /// The dwell at the bottom of the hole is not usable.
    #[error("invalid dwell: {0} (must be finite and >= 0)")]
    InvalidDwell(f64),

    /// The plunge rate is not usable.
    #[error("invalid plunge rate: {0} (must be finite and > 0)")]
    InvalidPlungeRate(f64),

    /// The feed rate is not usable.
    #[error("invalid feed rate: {0} (must be finite and > 0)")]
    InvalidFeedRate(f64),

    /// A hole would take more pecks than the operation allows.
    #[error("{depth:.2} mm in {peck:.3} mm pecks is over {max} pecks: peck deeper")]
    TooManyPecks {
        /// Depth of the hole in mm.
        depth: f64,
        /// Peck depth in mm.
        peck: f64,
        /// The limit.
        max: usize,
    },

    /// The tool does not cut across its own centre.
    #[error(
        "a {tool} does not cut across its own centre, so it cannot be plunged: use a drill, a centre-cutting end mill, or bore the hole helically"
    )]
    NotCentreCutting {
        /// The tool type.
        tool: &'static str,
    },

    /// Nothing says whether the tool cuts across its own centre.
    #[error(
        "whether this {tool} cuts across its own centre is not declared, so it cannot be plunged: set centre_cutting on the tool geometry, or bore the hole helically"
    )]
    CentreCuttingUnknown {
        /// The tool type.
        tool: &'static str,
    },

    /// Nothing says how long the flutes are.
    #[error(
        "a {depth:.2} mm hole cannot be checked against a flute length nothing declares: set flute_length on the tool geometry"
    )]
    FluteLengthUnknown {
        /// Depth the hole reaches in mm.
        depth: f64,
    },

    /// The hole reaches past the end of the flutes.
    #[error(
        "a {depth:.2} mm hole is {over:.2} mm past the {flute_length:.2} mm flutes: use a longer tool or a shallower hole"
    )]
    DepthBeyondFluteLength {
        /// Depth the hole reaches in mm.
        depth: f64,
        /// Flute length in mm.
        flute_length: f64,
        /// How far past in mm.
        over: f64,
    },

    /// The stock thickness of a through hole is not usable.
    #[error("invalid stock thickness: {0} (must be finite and > 0)")]
    InvalidStockThickness(f64),

    /// The break-through allowance is not usable.
    #[error("invalid break-through allowance: {0} (must be finite and >= 0)")]
    InvalidAllowance(f64),

    /// The declared spoilboard thickness is not usable.
    #[error("invalid spoilboard thickness: {0} (must be finite and > 0)")]
    InvalidSpoilboard(f64),

    /// Cutting past the underside without anything sacrificial under it.
    #[error(
        "this hole sinks {sink:.2} mm past the underside of the stock, which is only legal into a declared spoilboard: declare one, or say what is under the stock"
    )]
    NoSpoilboard {
        /// How far past the underside the tool goes, in mm.
        sink: f64,
    },

    /// The declared spoilboard is thinner than the break-through.
    #[error(
        "this hole sinks {sink:.2} mm past the underside but the spoilboard is only {thickness:.2} mm: it would cut into the bed"
    )]
    SpoilboardTooThin {
        /// How far past the underside the tool goes, in mm.
        sink: f64,
        /// Declared spoilboard thickness in mm.
        thickness: f64,
    },

    /// A through hole and a per-hole depth both claim to set the depth.
    #[error(
        "a through hole takes its depth from the stock thickness, but hole {index} also asks for {depth:.2} mm: drop one of the two"
    )]
    ThroughDepthConflict {
        /// Index of the hole in the operation.
        index: usize,
        /// The per-hole depth that conflicts.
        depth: f64,
    },

    /// A spot drill was asked to go through the stock.
    #[error("a spot is a start for a hole, not a hole: use a drill or peck cycle to go through")]
    SpotThroughStock,

    /// A spot deeper than the tool is wide is not a spot.
    #[error(
        "a {depth:.2} mm spot with a Ø{diameter:.2} tool is a hole, not a spot: use a drill or peck cycle"
    )]
    SpotTooDeep {
        /// Spot depth in mm.
        depth: f64,
        /// Tool diameter in mm.
        diameter: f64,
    },

    /// The bore is not bigger than the cutter.
    #[error(
        "a Ø{hole:.3} hole is not larger than the Ø{tool:.3} cutter: drill or plunge it, or fit a smaller cutter"
    )]
    HoleSmallerThanTool {
        /// Requested hole diameter in mm.
        hole: f64,
        /// Cutter diameter in mm.
        tool: f64,
    },

    /// The bore is so much bigger than the cutter that a helix leaves a post.
    #[error(
        "a Ø{hole:.3} hole with a Ø{tool:.3} cutter leaves a Ø{core:.3} core standing in the middle: a helix only opens a hole under twice the cutter diameter, so pocket it instead"
    )]
    CoreLeft {
        /// Requested hole diameter in mm.
        hole: f64,
        /// Cutter diameter in mm.
        tool: f64,
        /// Diameter of the material the helix leaves standing, in mm.
        core: f64,
    },

    /// Boring a wall needs an end mill.
    #[error("a {tool} cannot bore a wall: use a flat or bull end mill")]
    NotAnEndMill {
        /// The tool type.
        tool: &'static str,
    },

    /// The helix pitch is not usable.
    #[error("invalid helix pitch: {0} (must be finite and > 0)")]
    InvalidPitch(f64),

    /// The finish allowance is more than the helix has room to leave.
    #[error(
        "leaving {stock:.3} mm on the wall is more than the {radial:.3} mm of radial room the helix has"
    )]
    StockToLeaveTooLarge {
        /// Requested stock to leave in mm.
        stock: f64,
        /// Radial room between cutter and wall in mm.
        radial: f64,
    },

    /// The bore diameter is not a usable number.
    #[error("invalid hole diameter: {0} (must be finite and > 0)")]
    InvalidDiameter(f64),

    /// The bore centre is not a usable number.
    #[error("non-finite bore centre ({0}, {1})")]
    InvalidCentre(f64, f64),
}

/// A sacrificial board under the stock. Cutting past the underside of the
/// stock is only legal into one of these.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
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

/// A hole that has to come out the other side.
///
/// The tool has to sink past the underside for the hole to be open at full
/// diameter: by the drill's point length, plus whatever allowance is asked
/// for. That sink is only legal into a declared spoilboard — the same rule
/// the contour bottom allowance follows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BreakThrough {
    /// Stock thickness in mm (the underside sits at Z = -thickness).
    pub stock_thickness: f64,
    /// Extra depth past the underside, in mm, beyond the tool's own point.
    pub allowance: f64,
    /// What is under the stock.
    pub spoilboard: Option<Spoilboard>,
}

impl BreakThrough {
    /// A through hole in stock of the given thickness, with no extra
    /// allowance beyond the tool's own point.
    pub fn new(stock_thickness: f64) -> Self {
        Self {
            stock_thickness,
            allowance: 0.0,
            spoilboard: None,
        }
    }

    /// Ask for extra depth past the underside.
    pub fn with_allowance(mut self, allowance: f64) -> Self {
        self.allowance = allowance;
        self
    }

    /// Declare the sacrificial board under the stock.
    pub fn over(mut self, spoilboard: Spoilboard) -> Self {
        self.spoilboard = Some(spoilboard);
        self
    }
}

/// One hole centre, with an optional depth of its own.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Hole {
    /// Centre X in mm.
    pub x: f64,
    /// Centre Y in mm.
    pub y: f64,
    /// Depth for this hole in mm, overriding the operation's shared depth.
    #[serde(default)]
    pub depth: Option<f64>,
}

impl Hole {
    /// A hole at the operation's shared depth.
    pub fn at(x: f64, y: f64) -> Self {
        Self { x, y, depth: None }
    }

    /// A hole with a depth of its own.
    pub fn deep(x: f64, y: f64, depth: f64) -> Self {
        Self {
            x,
            y,
            depth: Some(depth),
        }
    }
}

impl From<Point2D> for Hole {
    fn from(p: Point2D) -> Self {
        Self::at(p.x, p.y)
    }
}

impl From<(f64, f64)> for Hole {
    fn from((x, y): (f64, f64)) -> Self {
        Self::at(x, y)
    }
}

/// How the tool gets to the bottom of the hole.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum DrillCycle {
    /// A short cone that gives the next tool something to follow.
    Spot,
    /// Straight to depth in one plunge.
    Straight,
    /// Peck with a full retract to the clearance plane between pecks, so the
    /// chips leave the hole.
    Peck {
        /// How much each peck takes, in mm.
        peck_depth: f64,
    },
    /// Peck without leaving the hole: back off by `retreat` to snap the chip,
    /// then carry on.
    ChipBreak {
        /// How much each peck takes, in mm.
        peck_depth: f64,
        /// How far the tool backs off between pecks, in mm.
        retreat: f64,
    },
}

/// Drilling: one cycle applied to a list of hole centres.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Drill {
    /// The hole centres, drilled in the order given.
    pub holes: Vec<Hole>,
    /// Depth below the stock top, in mm, for holes that do not carry one.
    pub depth: f64,
    /// How the tool gets to the bottom.
    pub cycle: DrillCycle,
    /// Height of the clearance (R) plane above the stock top, in mm. The tool
    /// traverses between holes at this height and starts each plunge there.
    pub clearance: f64,
    /// Gap left above the last peck when rapiding back into the hole, in mm.
    /// This is the one number that keeps a rapid out of uncut material.
    pub peck_clearance: f64,
    /// Seconds to dwell at the bottom of each hole (0 for none).
    pub dwell: f64,
    /// Set when the holes have to come out the other side.
    pub through: Option<BreakThrough>,
}

impl Drill {
    /// Drill the given centres straight to depth.
    pub fn new(holes: impl IntoIterator<Item = impl Into<Hole>>, depth: f64) -> Self {
        Self {
            holes: holes.into_iter().map(Into::into).collect(),
            depth,
            cycle: DrillCycle::Straight,
            clearance: 2.0,
            peck_clearance: 0.5,
            dwell: 0.0,
            through: None,
        }
    }

    /// Spot the given centres.
    pub fn spot(holes: impl IntoIterator<Item = impl Into<Hole>>, depth: f64) -> Self {
        Self {
            cycle: DrillCycle::Spot,
            ..Self::new(holes, depth)
        }
    }

    /// Peck the given centres, retracting clear of the hole between pecks.
    pub fn peck(
        holes: impl IntoIterator<Item = impl Into<Hole>>,
        depth: f64,
        peck_depth: f64,
    ) -> Self {
        Self {
            cycle: DrillCycle::Peck { peck_depth },
            ..Self::new(holes, depth)
        }
    }

    /// Peck the given centres without leaving the hole.
    pub fn chip_break(
        holes: impl IntoIterator<Item = impl Into<Hole>>,
        depth: f64,
        peck_depth: f64,
        retreat: f64,
    ) -> Self {
        Self {
            cycle: DrillCycle::ChipBreak {
                peck_depth,
                retreat,
            },
            ..Self::new(holes, depth)
        }
    }

    /// Set the clearance (R) plane height above the stock top.
    pub fn with_clearance(mut self, clearance: f64) -> Self {
        self.clearance = clearance;
        self
    }

    /// Set the gap kept above the last peck when rapiding back down.
    pub fn with_peck_clearance(mut self, peck_clearance: f64) -> Self {
        self.peck_clearance = peck_clearance;
        self
    }

    /// Dwell at the bottom of every hole.
    pub fn with_dwell(mut self, seconds: f64) -> Self {
        self.dwell = seconds;
        self
    }

    /// Make these holes go through the stock.
    pub fn with_break_through(mut self, through: BreakThrough) -> Self {
        self.through = Some(through);
        self
    }

    /// The bottoms of each peck for a hole of this depth, in mm below the
    /// stock top, deepest last.
    pub fn peck_bottoms(&self, depth: f64) -> Result<Vec<f64>, DrillError> {
        let peck = match self.cycle {
            DrillCycle::Spot | DrillCycle::Straight => return Ok(vec![depth]),
            DrillCycle::Peck { peck_depth } | DrillCycle::ChipBreak { peck_depth, .. } => {
                peck_depth
            }
        };
        let count = (depth / peck).ceil() as usize;
        if count > MAX_PECKS {
            return Err(DrillError::TooManyPecks {
                depth,
                peck,
                max: MAX_PECKS,
            });
        }
        let mut bottoms: Vec<f64> = (1..=count.max(1))
            .map(|i| (i as f64 * peck).min(depth))
            .collect();
        // A final peck that lands within a rounding error of the last one is
        // a move that cuts nothing.
        if bottoms.len() >= 2 {
            let last = bottoms[bottoms.len() - 1];
            let prev = bottoms[bottoms.len() - 2];
            if last - prev < 1e-9 {
                bottoms.pop();
            }
        }
        Ok(bottoms)
    }

    /// How deep the deepest hole reaches below the stock top, in mm.
    pub fn deepest(&self, tool: &Tool) -> Result<f64, DrillError> {
        let mut deepest: f64 = 0.0;
        for (index, hole) in self.holes.iter().enumerate() {
            deepest = deepest.max(self.hole_depth(index, hole, tool)?);
        }
        Ok(deepest)
    }

    /// The cut these holes represent, for the tool-geometry checks.
    pub fn cut_context(&self, tool: &Tool) -> Result<CutContext, DrillError> {
        Ok(CutContext::new(self.deepest(tool)?)
            .slotting()
            .with_holder_clearance(self.clearance))
    }

    /// Depth of one hole, including the point and break-through allowance of
    /// a through hole.
    fn hole_depth(&self, index: usize, hole: &Hole, tool: &Tool) -> Result<f64, DrillError> {
        if let Some(through) = &self.through {
            if let Some(depth) = hole.depth {
                return Err(DrillError::ThroughDepthConflict { index, depth });
            }
            return Ok(through.stock_thickness + tip_length(tool) + through.allowance);
        }
        let depth = hole.depth.unwrap_or(self.depth);
        if !depth.is_finite() || depth <= 0.0 {
            return Err(DrillError::InvalidHoleDepth {
                index,
                x: hole.x,
                y: hole.y,
                depth,
            });
        }
        Ok(depth)
    }

    /// Generate the toolpath for these holes.
    pub fn generate(
        &self,
        tool: &Tool,
        geometry: &ToolGeometry,
        settings: &CamSettings,
    ) -> Result<Toolpath, DrillError> {
        self.validate(tool, geometry, settings)?;

        let plunge = settings.plunge_rate;
        let traverse_z = settings.safe_z.max(self.clearance);
        let mut toolpath = Toolpath::new();
        toolpath.push(ToolpathSegment::comment(format!(
            "{}: {} hole(s), Ø{:.2}, depth {:.2} mm",
            self.cycle_name(),
            self.holes.len(),
            tool.diameter(),
            self.deepest(tool)?
        )));

        for (index, hole) in self.holes.iter().enumerate() {
            let depth = self.hole_depth(index, hole, tool)?;
            let bottoms = self.peck_bottoms(depth)?;

            if index == 0 {
                toolpath.push(ToolpathSegment::rapid(hole.x, hole.y, traverse_z));
            }
            // Between holes the tool stays on the clearance plane: it is
            // above the stock top, so XY at this height is over air.
            toolpath.push(ToolpathSegment::rapid(hole.x, hole.y, self.clearance));

            let mut previous: Option<f64> = None;
            for (peck, &bottom) in bottoms.iter().enumerate() {
                if let Some(previous) = previous {
                    match self.cycle {
                        DrillCycle::Peck { .. } => {
                            // Rapid back down, but stop short of the metal:
                            // the gap is measured from the last peck's floor,
                            // never from the new one.
                            let z = (-previous + self.peck_clearance).min(self.clearance);
                            toolpath.push(ToolpathSegment::rapid(hole.x, hole.y, z));
                        }
                        // Chip break never left the hole.
                        DrillCycle::ChipBreak { .. } => {}
                        DrillCycle::Spot | DrillCycle::Straight => unreachable!("single peck"),
                    }
                }
                toolpath.push(ToolpathSegment::linear(hole.x, hole.y, -bottom, plunge));

                let last = peck + 1 == bottoms.len();
                if last {
                    if self.dwell > 0.0 {
                        toolpath.push(ToolpathSegment::dwell(self.dwell));
                    }
                } else {
                    match self.cycle {
                        // Out of the hole entirely: that is what carries the
                        // chips out with it.
                        DrillCycle::Peck { .. } => {
                            toolpath.push(ToolpathSegment::rapid(hole.x, hole.y, self.clearance))
                        }
                        // Just enough to snap the chip.
                        DrillCycle::ChipBreak { retreat, .. } => {
                            toolpath.push(ToolpathSegment::rapid(hole.x, hole.y, -bottom + retreat))
                        }
                        DrillCycle::Spot | DrillCycle::Straight => unreachable!("single peck"),
                    }
                }
                previous = Some(bottom);
            }

            toolpath.push(ToolpathSegment::rapid(hole.x, hole.y, self.clearance));
        }

        if let Some(last) = self.holes.last() {
            if traverse_z > self.clearance {
                toolpath.push(ToolpathSegment::rapid(last.x, last.y, traverse_z));
            }
        }

        Ok(toolpath)
    }

    /// Name of the cycle, for comments and operation lists.
    pub fn cycle_name(&self) -> &'static str {
        match self.cycle {
            DrillCycle::Spot => "Spot",
            DrillCycle::Straight => "Drill",
            DrillCycle::Peck { .. } => "Peck drill",
            DrillCycle::ChipBreak { .. } => "Chip-break drill",
        }
    }

    fn validate(
        &self,
        tool: &Tool,
        geometry: &ToolGeometry,
        settings: &CamSettings,
    ) -> Result<(), DrillError> {
        if self.holes.is_empty() {
            return Err(DrillError::NoHoles);
        }
        for (index, hole) in self.holes.iter().enumerate() {
            if !hole.x.is_finite() || !hole.y.is_finite() {
                return Err(DrillError::InvalidHoleCentre {
                    index,
                    x: hole.x,
                    y: hole.y,
                });
            }
        }
        if self.through.is_none() && (!self.depth.is_finite() || self.depth <= 0.0) {
            // A shared depth that no hole uses is still a mistake worth
            // naming, but only when some hole would fall back to it.
            if self.holes.iter().any(|h| h.depth.is_none()) {
                return Err(DrillError::InvalidDepth(self.depth));
            }
        }
        if !self.clearance.is_finite() || self.clearance <= 0.0 {
            return Err(DrillError::InvalidClearance(self.clearance));
        }
        if !self.peck_clearance.is_finite() || self.peck_clearance <= 0.0 {
            return Err(DrillError::InvalidPeckClearance(self.peck_clearance));
        }
        if !self.dwell.is_finite() || self.dwell < 0.0 {
            return Err(DrillError::InvalidDwell(self.dwell));
        }
        if !settings.plunge_rate.is_finite() || settings.plunge_rate <= 0.0 {
            return Err(DrillError::InvalidPlungeRate(settings.plunge_rate));
        }
        match self.cycle {
            DrillCycle::Peck { peck_depth } => {
                if !peck_depth.is_finite() || peck_depth <= 0.0 {
                    return Err(DrillError::InvalidPeckDepth(peck_depth));
                }
            }
            DrillCycle::ChipBreak {
                peck_depth,
                retreat,
            } => {
                if !peck_depth.is_finite() || peck_depth <= 0.0 {
                    return Err(DrillError::InvalidPeckDepth(peck_depth));
                }
                if !retreat.is_finite() || retreat <= 0.0 {
                    return Err(DrillError::InvalidRetreat(retreat));
                }
            }
            DrillCycle::Spot | DrillCycle::Straight => {}
        }

        // A plunging cycle needs a tool that cuts at its own centre.
        match geometry.centre_cutting_of(tool) {
            Some(true) => {}
            Some(false) => {
                return Err(DrillError::NotCentreCutting {
                    tool: tool.kind_name(),
                })
            }
            None => {
                return Err(DrillError::CentreCuttingUnknown {
                    tool: tool.kind_name(),
                })
            }
        }

        if let Some(through) = &self.through {
            if matches!(self.cycle, DrillCycle::Spot) {
                return Err(DrillError::SpotThroughStock);
            }
            check_break_through(through, tool)?;
        }

        let depth = self.deepest(tool)?;
        if matches!(self.cycle, DrillCycle::Spot) && depth > tool.diameter() {
            return Err(DrillError::SpotTooDeep {
                depth,
                diameter: tool.diameter(),
            });
        }
        check_flutes(tool, geometry, depth)?;
        Ok(())
    }
}

/// Helical boring: open a hole larger than the cutter by spiralling down, then
/// clean the floor and the wall at full depth.
///
/// The wall ends up at exactly the requested diameter and never wider: the
/// helix runs at the finishing radius less whatever is left for the finish
/// pass, and only the finishing circle touches the wall.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HelicalBore {
    /// Hole centre X in mm.
    pub x: f64,
    /// Hole centre Y in mm.
    pub y: f64,
    /// Finished hole diameter in mm.
    pub diameter: f64,
    /// Depth below the stock top in mm.
    pub depth: f64,
    /// Z the helix drops per revolution, in mm. The helix uses this or less,
    /// never more.
    pub pitch: f64,
    /// Height of the clearance plane above the stock top, in mm.
    pub clearance: f64,
    /// Material left on the wall for the finish pass, in mm.
    pub stock_to_leave: f64,
    /// Whether to run the full-depth finishing circle at the wall.
    pub finish_pass: bool,
    /// Set when the hole has to come out the other side.
    pub through: Option<BreakThrough>,
}

impl HelicalBore {
    /// Bore a hole of the given diameter and depth at (x, y).
    pub fn new(x: f64, y: f64, diameter: f64, depth: f64, pitch: f64) -> Self {
        Self {
            x,
            y,
            diameter,
            depth,
            pitch,
            clearance: 2.0,
            stock_to_leave: 0.0,
            finish_pass: true,
            through: None,
        }
    }

    /// Set the clearance plane height above the stock top.
    pub fn with_clearance(mut self, clearance: f64) -> Self {
        self.clearance = clearance;
        self
    }

    /// Leave material on the wall for the finishing circle to take.
    pub fn with_stock_to_leave(mut self, stock_to_leave: f64) -> Self {
        self.stock_to_leave = stock_to_leave;
        self
    }

    /// Turn the full-depth finishing circle off.
    pub fn without_finish_pass(mut self) -> Self {
        self.finish_pass = false;
        self
    }

    /// Make the bore go through the stock.
    pub fn with_break_through(mut self, through: BreakThrough) -> Self {
        self.through = Some(through);
        self
    }

    /// Depth the bore reaches below the stock top, in mm.
    pub fn total_depth(&self, tool: &Tool) -> f64 {
        match &self.through {
            // An end mill has no point to sink: the floor is the underside.
            Some(through) => through.stock_thickness + tip_length(tool) + through.allowance,
            None => self.depth,
        }
    }

    /// The cut this bore represents, for the tool-geometry checks.
    pub fn cut_context(&self, tool: &Tool) -> CutContext {
        CutContext::new(self.total_depth(tool))
            .slotting()
            .with_holder_clearance(self.clearance)
    }

    /// Radius the cutter centre runs at for the finished wall, in mm.
    pub fn finish_radius(&self, tool: &Tool) -> f64 {
        (self.diameter - tool.diameter()) / 2.0
    }

    /// Generate the toolpath for this bore.
    pub fn generate(
        &self,
        tool: &Tool,
        geometry: &ToolGeometry,
        settings: &CamSettings,
    ) -> Result<Toolpath, DrillError> {
        self.validate(tool, geometry, settings)?;

        let depth = self.total_depth(tool);
        let feed = settings.feed_rate;
        let plunge = settings.plunge_rate;
        let traverse_z = settings.safe_z.max(self.clearance);

        let r_finish = self.finish_radius(tool);
        let r_helix = if self.finish_pass {
            r_finish - self.stock_to_leave
        } else {
            r_finish
        };

        // Whole revolutions only, so the helix ends where it started in XY and
        // the floor circle picks up cleanly. Rounding up means the pitch used
        // is the requested pitch or less, never more.
        let revolutions = (depth / self.pitch).ceil().max(1.0) as usize;
        let pitch = depth / revolutions as f64;

        let mut toolpath = Toolpath::new();
        toolpath.push(ToolpathSegment::comment(format!(
            "Helical bore: Ø{:.3} × {:.2} mm deep with a Ø{:.3} cutter, {} rev at {:.3} mm pitch",
            self.diameter,
            depth,
            tool.diameter(),
            revolutions,
            pitch
        )));

        let start = (self.x + r_helix, self.y);
        toolpath.push(ToolpathSegment::rapid(start.0, start.1, traverse_z));
        if traverse_z > self.clearance {
            toolpath.push(ToolpathSegment::rapid(start.0, start.1, self.clearance));
        }
        // Down to the stock top under feed: the helix starts where the metal
        // starts.
        toolpath.push(ToolpathSegment::linear(start.0, start.1, 0.0, plunge));

        // Counter-clockwise inside a hole is climb milling with a
        // right-hand cutter, which is what the rest of the crate does.
        let dir = ArcDir::Ccw;
        let mut z = 0.0;
        for _ in 0..revolutions {
            for quarter in 0..4 {
                z -= pitch / 4.0;
                self.push_quarter(&mut toolpath, r_helix, quarter, z, dir, feed);
            }
        }
        // The helix leaves a ramp on the floor; one flat turn takes it.
        for quarter in 0..4 {
            self.push_quarter(&mut toolpath, r_helix, quarter, -depth, dir, feed);
        }

        if self.finish_pass && r_finish - r_helix > 1e-12 {
            // Out to the wall along a radius, then one turn at full depth.
            toolpath.push(ToolpathSegment::linear(
                self.x + r_finish,
                self.y,
                -depth,
                feed,
            ));
            for quarter in 0..4 {
                self.push_quarter(&mut toolpath, r_finish, quarter, -depth, dir, feed);
            }
        }

        toolpath.push(ToolpathSegment::rapid(
            self.x + if self.finish_pass { r_finish } else { r_helix },
            self.y,
            traverse_z,
        ));

        Ok(toolpath)
    }

    /// One counter-clockwise quarter turn of radius `r`, ending at height `z`.
    fn push_quarter(
        &self,
        toolpath: &mut Toolpath,
        r: f64,
        quarter: usize,
        z: f64,
        dir: ArcDir,
        feed: f64,
    ) {
        let start_angle = std::f64::consts::FRAC_PI_2 * quarter as f64;
        let end_angle = start_angle + std::f64::consts::FRAC_PI_2;
        let (sx, sy) = (
            self.x + r * start_angle.cos(),
            self.y + r * start_angle.sin(),
        );
        let (ex, ey) = (self.x + r * end_angle.cos(), self.y + r * end_angle.sin());
        // The centre is given relative to the start of the arc, GRBL style.
        toolpath.push(ToolpathSegment::arc_xy(
            ex,
            ey,
            z,
            self.x - sx,
            self.y - sy,
            dir,
            feed,
        ));
    }

    fn validate(
        &self,
        tool: &Tool,
        geometry: &ToolGeometry,
        settings: &CamSettings,
    ) -> Result<(), DrillError> {
        if !self.x.is_finite() || !self.y.is_finite() {
            return Err(DrillError::InvalidCentre(self.x, self.y));
        }
        if !self.diameter.is_finite() || self.diameter <= 0.0 {
            return Err(DrillError::InvalidDiameter(self.diameter));
        }
        if !self.clearance.is_finite() || self.clearance <= 0.0 {
            return Err(DrillError::InvalidClearance(self.clearance));
        }
        if !self.pitch.is_finite() || self.pitch <= 0.0 {
            return Err(DrillError::InvalidPitch(self.pitch));
        }
        if !settings.feed_rate.is_finite() || settings.feed_rate <= 0.0 {
            return Err(DrillError::InvalidFeedRate(settings.feed_rate));
        }
        if !settings.plunge_rate.is_finite() || settings.plunge_rate <= 0.0 {
            return Err(DrillError::InvalidPlungeRate(settings.plunge_rate));
        }
        if !matches!(
            tool,
            Tool::FlatEndMill { .. } | Tool::BullEndMill { .. } | Tool::BallEndMill { .. }
        ) {
            return Err(DrillError::NotAnEndMill {
                tool: tool.kind_name(),
            });
        }
        let cutter = tool.diameter();
        if self.diameter <= cutter + 1e-9 {
            return Err(DrillError::HoleSmallerThanTool {
                hole: self.diameter,
                tool: cutter,
            });
        }
        if self.diameter >= 2.0 * cutter {
            return Err(DrillError::CoreLeft {
                hole: self.diameter,
                tool: cutter,
                core: self.diameter - 2.0 * cutter,
            });
        }
        let radial = self.finish_radius(tool);
        if self.finish_pass
            && (!self.stock_to_leave.is_finite()
                || self.stock_to_leave < 0.0
                || self.stock_to_leave >= radial)
        {
            return Err(DrillError::StockToLeaveTooLarge {
                stock: self.stock_to_leave,
                radial,
            });
        }
        if let Some(through) = &self.through {
            check_break_through(through, tool)?;
        } else if !self.depth.is_finite() || self.depth <= 0.0 {
            return Err(DrillError::InvalidDepth(self.depth));
        }
        let depth = self.total_depth(tool);
        if depth / self.pitch > MAX_PECKS as f64 {
            return Err(DrillError::TooManyPecks {
                depth,
                peck: self.pitch,
                max: MAX_PECKS,
            });
        }
        check_flutes(tool, geometry, depth)?;
        Ok(())
    }
}

/// How far the tip of a pointed tool sits below the full-diameter part of the
/// hole it makes, in mm. Flat tools have no point.
pub fn tip_length(tool: &Tool) -> f64 {
    match tool {
        Tool::Drill {
            diameter,
            point_angle,
        }
        | Tool::VBit {
            diameter,
            angle: point_angle,
        } => {
            let half = (point_angle / 2.0).to_radians();
            let tan = half.tan();
            if !tan.is_finite() || tan <= 0.0 {
                0.0
            } else {
                (diameter / 2.0) / tan
            }
        }
        _ => 0.0,
    }
}

/// The through-hole rule: what sinks past the underside must sink into a
/// declared spoilboard, and no deeper than that board is thick.
fn check_break_through(through: &BreakThrough, tool: &Tool) -> Result<(), DrillError> {
    if !through.stock_thickness.is_finite() || through.stock_thickness <= 0.0 {
        return Err(DrillError::InvalidStockThickness(through.stock_thickness));
    }
    if !through.allowance.is_finite() || through.allowance < 0.0 {
        return Err(DrillError::InvalidAllowance(through.allowance));
    }
    let sink = tip_length(tool) + through.allowance;
    if sink <= 0.0 {
        return Ok(());
    }
    let Some(spoilboard) = through.spoilboard else {
        return Err(DrillError::NoSpoilboard { sink });
    };
    if !spoilboard.thickness.is_finite() || spoilboard.thickness <= 0.0 {
        return Err(DrillError::InvalidSpoilboard(spoilboard.thickness));
    }
    if spoilboard.thickness < sink {
        return Err(DrillError::SpoilboardTooThin {
            sink,
            thickness: spoilboard.thickness,
        });
    }
    Ok(())
}

/// A hole no deeper than the flutes are long — and a flute length that is
/// actually known.
fn check_flutes(tool: &Tool, geometry: &ToolGeometry, depth: f64) -> Result<(), DrillError> {
    let Some(flute_length) = geometry.flute_length_of(tool) else {
        return Err(DrillError::FluteLengthUnknown { depth });
    };
    if depth > flute_length {
        return Err(DrillError::DepthBeyondFluteLength {
            depth,
            flute_length,
            over: depth - flute_length,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolpathSegment;

    fn endmill(diameter: f64, flute_length: f64) -> Tool {
        Tool::FlatEndMill {
            diameter,
            flute_length,
            flutes: 2,
        }
    }

    fn centre_cutting(flute_length: f64) -> ToolGeometry {
        ToolGeometry::new()
            .with_centre_cutting(true)
            .with_flute_length(flute_length)
    }

    fn settings() -> CamSettings {
        CamSettings {
            safe_z: 5.0,
            plunge_rate: 40.0,
            feed_rate: 250.0,
            ..CamSettings::default()
        }
    }

    /// Every Z the tool is commanded to, in order, with whether the move was
    /// a rapid.
    fn z_moves(toolpath: &Toolpath) -> Vec<(bool, f64)> {
        toolpath
            .segments
            .iter()
            .filter_map(|s| Some((s.is_rapid(), s.target()?[2])))
            .collect()
    }

    /// The error a refused operation returns.
    fn refusal(result: Result<Toolpath, DrillError>) -> DrillError {
        result.err().expect("expected a refusal")
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    /// The peck cycle's Z sequence, move by move: what it cuts, where it
    /// retracts to, and — the one that matters — that every rapid back into
    /// the hole stops above the floor the last peck left.
    #[test]
    fn test_peck_cycle_z_sequence_is_exact() {
        let tool = endmill(3.0, 20.0);
        let op = Drill::peck([(10.0, 10.0)], 6.0, 2.0)
            .with_clearance(2.0)
            .with_peck_clearance(0.5);
        let toolpath = op
            .generate(&tool, &centre_cutting(20.0), &settings())
            .unwrap();

        let expected = [
            (true, 5.0),   // traverse at the safe height
            (true, 2.0),   // down to the clearance plane
            (false, -2.0), // peck 1
            (true, 2.0),   // full retract, chips out
            (true, -1.5),  // back down, 0.5 above the floor at -2
            (false, -4.0), // peck 2
            (true, 2.0),
            (true, -3.5), // 0.5 above the floor at -4
            (false, -6.0),
            (true, 2.0), // final retract to the clearance plane
            (true, 5.0), // back to the safe height
        ];
        let moves = z_moves(&toolpath);
        assert_eq!(moves.len(), expected.len(), "{moves:?}");
        for (i, ((rapid, z), (want_rapid, want_z))) in moves.iter().zip(expected).enumerate() {
            assert_eq!(*rapid, want_rapid, "move {i}: {moves:?}");
            assert!(close(*z, want_z), "move {i}: Z{z} wanted Z{want_z}");
        }

        // Stated as the rule, not the sequence: no rapid ever ends below the
        // deepest floor already cut.
        let mut cut_to = 0.0f64;
        for (rapid, z) in z_moves(&toolpath) {
            if rapid {
                assert!(
                    z >= -cut_to - 1e-9,
                    "rapid to Z{z} dives into metal standing at Z{:.3}",
                    -cut_to
                );
            } else {
                cut_to = cut_to.max(-z);
            }
        }
    }

    /// The last peck is short when the depth is not a multiple of the peck,
    /// and a peck that would cut nothing is not emitted.
    #[test]
    fn test_peck_bottoms_land_on_the_depth() {
        let op = Drill::peck([(0.0, 0.0)], 5.0, 2.0);
        assert_eq!(op.peck_bottoms(5.0).unwrap(), vec![2.0, 4.0, 5.0]);

        let op = Drill::peck([(0.0, 0.0)], 6.0, 2.0);
        assert_eq!(op.peck_bottoms(6.0).unwrap(), vec![2.0, 4.0, 6.0]);

        let op = Drill::peck([(0.0, 0.0)], 1.0, 2.0);
        assert_eq!(op.peck_bottoms(1.0).unwrap(), vec![1.0]);
    }

    /// Chip break stays in the hole: it backs off by the retreat and no more,
    /// and never returns to the clearance plane between pecks.
    #[test]
    fn test_chip_break_retreats_by_the_stated_amount_only() {
        let tool = endmill(3.0, 20.0);
        let op = Drill::chip_break([(0.0, 0.0)], 6.0, 2.0, 0.25).with_clearance(2.0);
        let toolpath = op
            .generate(&tool, &centre_cutting(20.0), &settings())
            .unwrap();

        let expected = [
            (true, 5.0),
            (true, 2.0),
            (false, -2.0),
            (true, -1.75), // 0.25 up, still in the hole
            (false, -4.0),
            (true, -3.75),
            (false, -6.0),
            (true, 2.0),
            (true, 5.0),
        ];
        let moves = z_moves(&toolpath);
        assert_eq!(moves.len(), expected.len(), "{moves:?}");
        for (i, ((rapid, z), (want_rapid, want_z))) in moves.iter().zip(expected).enumerate() {
            assert_eq!(*rapid, want_rapid, "move {i}: {moves:?}");
            assert!(close(*z, want_z), "move {i}: Z{z} wanted Z{want_z}");
        }
    }

    /// A dwell at the bottom happens once per hole, at the bottom, before the
    /// retract.
    #[test]
    fn test_dwell_sits_at_the_bottom_of_each_hole() {
        let tool = endmill(3.0, 20.0);
        let op = Drill::peck([(0.0, 0.0), (10.0, 0.0)], 4.0, 2.0).with_dwell(0.3);
        let toolpath = op
            .generate(&tool, &centre_cutting(20.0), &settings())
            .unwrap();

        let mut dwells = 0;
        for (i, seg) in toolpath.segments.iter().enumerate() {
            let ToolpathSegment::Dwell { seconds } = seg else {
                continue;
            };
            dwells += 1;
            assert!(close(*seconds, 0.3));
            let before = toolpath.segments[i - 1].target().unwrap();
            assert!(close(before[2], -4.0), "dwelled at {before:?}");
            assert!(!toolpath.segments[i - 1].is_rapid());
            assert!(toolpath.segments[i + 1].is_rapid());
        }
        assert_eq!(dwells, 2);
    }

    /// Between holes the tool moves in XY on the clearance plane, above the
    /// stock, and only the first and last moves use the safe height.
    #[test]
    fn test_holes_traverse_above_the_stock() {
        let tool = endmill(3.0, 20.0);
        let op = Drill::new([(0.0, 0.0), (10.0, 5.0), (20.0, 5.0)], 3.0).with_clearance(1.5);
        let toolpath = op
            .generate(&tool, &centre_cutting(20.0), &settings())
            .unwrap();

        let mut at = [0.0, 0.0, 5.0];
        for seg in &toolpath.segments {
            let Some(to) = seg.target() else { continue };
            let xy = (to[0] - at[0]).hypot(to[1] - at[1]);
            if xy > 1e-9 {
                assert!(
                    at[2] >= 1.5 - 1e-9 && to[2] >= 1.5 - 1e-9,
                    "XY move from {at:?} to {to:?} below the clearance plane"
                );
            }
            at = to;
        }
    }

    /// Per-hole depths override the shared one, hole by hole.
    #[test]
    fn test_per_hole_depth_overrides_the_shared_depth() {
        let tool = endmill(3.0, 20.0);
        let op = Drill::new([Hole::at(0.0, 0.0), Hole::deep(10.0, 0.0, 1.25)], 3.0);
        let toolpath = op
            .generate(&tool, &centre_cutting(20.0), &settings())
            .unwrap();
        let cuts: Vec<f64> = toolpath
            .segments
            .iter()
            .filter(|s| s.is_cutting())
            .filter_map(|s| Some(s.target()?[2]))
            .collect();
        assert_eq!(cuts.len(), 2);
        assert!(close(cuts[0], -3.0), "{cuts:?}");
        assert!(close(cuts[1], -1.25), "{cuts:?}");
    }

    /// A through hole goes past the underside by the drill's own point, which
    /// is only legal into a declared spoilboard thick enough to take it.
    #[test]
    fn test_through_holes_need_a_spoilboard_that_can_take_the_point() {
        let drill = Tool::Drill {
            diameter: 3.0,
            point_angle: 118.0,
        };
        let geom = ToolGeometry::new().with_flute_length(30.0);
        // A 118° point on Ø3 sinks 0.901 mm.
        let point = tip_length(&drill);
        assert!((point - 0.9012).abs() < 1e-3, "point {point}");

        let bare = Drill::new([(0.0, 0.0)], 1.0).with_break_through(BreakThrough::new(6.0));
        assert_eq!(
            refusal(bare.generate(&drill, &geom, &settings())),
            DrillError::NoSpoilboard { sink: point }
        );

        let thin = Drill::new([(0.0, 0.0)], 1.0).with_break_through(
            BreakThrough::new(6.0)
                .with_allowance(0.5)
                .over(Spoilboard::new(1.0)),
        );
        assert!(matches!(
            thin.generate(&drill, &geom, &settings()),
            Err(DrillError::SpoilboardTooThin { .. })
        ));

        let ok = Drill::new([(0.0, 0.0)], 1.0).with_break_through(
            BreakThrough::new(6.0)
                .with_allowance(0.5)
                .over(Spoilboard::new(6.0)),
        );
        let toolpath = ok.generate(&drill, &geom, &settings()).unwrap();
        let deepest = toolpath
            .segments
            .iter()
            .filter(|s| s.is_cutting())
            .filter_map(|s| Some(s.target()?[2]))
            .fold(f64::INFINITY, f64::min);
        // Stock 6 + point 0.901 + 0.5 allowance.
        assert!(close(deepest, -(6.0 + point + 0.5)), "{deepest}");
    }

    /// A through hole takes its depth from the stock; a hole that also names
    /// one is a contradiction, not a preference.
    #[test]
    fn test_through_hole_refuses_a_per_hole_depth() {
        let drill = Tool::Drill {
            diameter: 3.0,
            point_angle: 118.0,
        };
        let op = Drill::new([Hole::deep(0.0, 0.0, 2.0)], 1.0)
            .with_break_through(BreakThrough::new(6.0).over(Spoilboard::new(6.0)));
        assert!(matches!(
            op.generate(
                &drill,
                &ToolGeometry::new().with_flute_length(30.0),
                &settings()
            ),
            Err(DrillError::ThroughDepthConflict { index: 0, .. })
        ));
    }

    /// A tool that does not cut across its own centre cannot be plunged, and
    /// one that will not say is refused just the same.
    #[test]
    fn test_plunging_refuses_tools_that_cannot_plunge() {
        let mill = endmill(3.0, 20.0);
        let op = Drill::new([(0.0, 0.0)], 3.0);

        assert_eq!(
            refusal(op.generate(&mill, &ToolGeometry::new(), &settings())),
            DrillError::CentreCuttingUnknown {
                tool: "flat end mill"
            }
        );
        assert_eq!(
            refusal(
                op.generate(
                    &mill,
                    &ToolGeometry::new()
                        .with_flute_length(20.0)
                        .with_centre_cutting(false),
                    &settings()
                )
            ),
            DrillError::NotCentreCutting {
                tool: "flat end mill"
            }
        );
        let face = Tool::FaceMill {
            diameter: 50.0,
            inserts: 4,
        };
        assert_eq!(
            refusal(op.generate(
                &face,
                &ToolGeometry::new().with_flute_length(20.0),
                &settings()
            )),
            DrillError::NotCentreCutting { tool: "face mill" }
        );
        // Declared centre-cutting, and it goes.
        assert!(op
            .generate(&mill, &centre_cutting(20.0), &settings())
            .is_ok());
    }

    /// Depth past the flutes is refused, and an undeclared flute length is
    /// refused rather than assumed.
    #[test]
    fn test_depth_beyond_flute_length_is_refused() {
        let tool = endmill(3.0, 8.0);
        let geom = ToolGeometry::new().with_centre_cutting(true);

        assert!(Drill::new([(0.0, 0.0)], 8.0)
            .generate(&tool, &geom, &settings())
            .is_ok());
        assert!(matches!(
            Drill::new([(0.0, 0.0)], 8.001).generate(&tool, &geom, &settings()),
            Err(DrillError::DepthBeyondFluteLength { over, .. }) if (over - 0.001).abs() < 1e-9
        ));

        let drill = Tool::Drill {
            diameter: 3.0,
            point_angle: 118.0,
        };
        assert!(matches!(
            Drill::new([(0.0, 0.0)], 5.0).generate(&drill, &ToolGeometry::new(), &settings()),
            Err(DrillError::FluteLengthUnknown { .. })
        ));
    }

    /// A spot is a start, not a hole.
    #[test]
    fn test_spot_refuses_to_become_a_hole() {
        let drill = Tool::Drill {
            diameter: 3.0,
            point_angle: 118.0,
        };
        let geom = ToolGeometry::new().with_flute_length(30.0);
        assert!(Drill::spot([(0.0, 0.0)], 1.0)
            .generate(&drill, &geom, &settings())
            .is_ok());
        assert!(matches!(
            Drill::spot([(0.0, 0.0)], 4.0).generate(&drill, &geom, &settings()),
            Err(DrillError::SpotTooDeep { .. })
        ));
        assert_eq!(
            refusal(
                Drill::spot([(0.0, 0.0)], 1.0)
                    .with_break_through(BreakThrough::new(6.0).over(Spoilboard::new(6.0)))
                    .generate(&drill, &geom, &settings())
            ),
            DrillError::SpotThroughStock
        );
    }

    /// Empty and nonsense inputs are named, not worked around.
    #[test]
    fn test_drill_refuses_nonsense() {
        let tool = endmill(3.0, 20.0);
        let geom = centre_cutting(20.0);
        let holes: [Hole; 0] = [];
        assert_eq!(
            refusal(Drill::new(holes, 3.0).generate(&tool, &geom, &settings())),
            DrillError::NoHoles
        );
        assert_eq!(
            refusal(Drill::new([(0.0, 0.0)], 0.0).generate(&tool, &geom, &settings())),
            DrillError::InvalidDepth(0.0)
        );
        assert_eq!(
            refusal(Drill::new([(0.0, 0.0)], 3.0).with_clearance(0.0).generate(
                &tool,
                &geom,
                &settings()
            )),
            DrillError::InvalidClearance(0.0)
        );
        assert_eq!(
            refusal(Drill::peck([(0.0, 0.0)], 3.0, 0.0).generate(&tool, &geom, &settings())),
            DrillError::InvalidPeckDepth(0.0)
        );
        assert!(matches!(
            Drill::peck([(0.0, 0.0)], 20.0, 0.0001).generate(&tool, &geom, &settings()),
            Err(DrillError::TooManyPecks { .. })
        ));
    }

    /// Radius and pitch of the helix, measured off the path: every point on
    /// the wall side sits at exactly the radius that leaves the requested
    /// diameter, and each revolution drops the pitch actually used — which is
    /// the requested pitch or less.
    #[test]
    fn test_helical_bore_radius_and_pitch() {
        // The stator's pilots: Ø2.5 with a Ø2 cutter.
        let tool = endmill(2.0, 10.0);
        let geom = ToolGeometry::new().with_flute_length(10.0);
        let bore = HelicalBore::new(7.5, 12.5, 2.5, 1.0, 0.3);
        let toolpath = bore.generate(&tool, &geom, &settings()).unwrap();

        let r = 0.25; // (2.5 - 2) / 2
        let arcs: Vec<[f64; 3]> = toolpath
            .segments
            .iter()
            .filter(|s| matches!(s, ToolpathSegment::Arc { .. }))
            .filter_map(|s| s.target())
            .collect();
        // 4 revolutions of 4 quarters (1.0 / 0.3 rounded up), plus the flat
        // turn on the floor.
        assert_eq!(arcs.len(), 4 * 4 + 4, "{} arcs", arcs.len());
        for p in &arcs {
            let radius = (p[0] - 7.5).hypot(p[1] - 12.5);
            assert!(
                (radius - r).abs() < 1e-12,
                "arc at radius {radius}, wanted {r}"
            );
        }

        // Pitch: Z per revolution, and never more than asked for.
        let pitch = 1.0 / 4.0;
        for rev in 0..4 {
            let top = if rev == 0 { 0.0 } else { arcs[rev * 4 - 1][2] };
            let bottom = arcs[rev * 4 + 3][2];
            assert!(
                close(top - bottom, pitch),
                "revolution {rev} dropped {}, wanted {pitch}",
                top - bottom
            );
        }
        assert!(pitch <= 0.3 + 1e-12);
        assert!(close(arcs[15][2], -1.0), "helix ended at {}", arcs[15][2]);
        // The floor turn stays at depth.
        for p in &arcs[16..] {
            assert!(close(p[2], -1.0));
        }
    }

    /// The finished wall is exactly the diameter asked for and never wider —
    /// including when the helix runs inside a finish allowance.
    #[test]
    fn test_helical_bore_wall_diameter_is_never_oversize() {
        let tool = endmill(2.0, 10.0);
        let geom = ToolGeometry::new().with_flute_length(10.0);
        for stock in [0.0, 0.1, 0.2] {
            let bore = HelicalBore::new(0.0, 0.0, 3.5, 2.0, 0.5).with_stock_to_leave(stock);
            let toolpath = bore.generate(&tool, &geom, &settings()).unwrap();
            let cutting: Vec<[f64; 3]> = toolpath
                .segments
                .iter()
                .filter(|s| s.is_cutting())
                .filter_map(|s| s.target())
                .collect();
            let max_r = cutting
                .iter()
                .map(|p| p[0].hypot(p[1]))
                .fold(0.0f64, f64::max);
            let wall = 2.0 * max_r + tool.diameter();
            assert!(
                wall <= 3.5 + 1e-9,
                "stock {stock}: wall came out Ø{wall}, over Ø3.5"
            );
            assert!(
                (wall - 3.5).abs() < 1e-9,
                "stock {stock}: wall came out Ø{wall}, wanted Ø3.5"
            );
            // Only the finishing turn touches the wall: the helix stays
            // inside it by the allowance.
            let helix_max = cutting
                .iter()
                .filter(|p| p[2] > -2.0 + 1e-12)
                .map(|p| p[0].hypot(p[1]))
                .fold(0.0f64, f64::max);
            assert!(
                (helix_max - (0.75 - stock)).abs() < 1e-9,
                "stock {stock}: helix ran at {helix_max}"
            );
        }
    }

    /// Without a finishing pass the helix itself is the wall, still exactly
    /// on size.
    #[test]
    fn test_helical_bore_without_finish_pass_is_on_size() {
        let tool = endmill(2.0, 10.0);
        let geom = ToolGeometry::new().with_flute_length(10.0);
        let bore = HelicalBore::new(0.0, 0.0, 3.0, 1.0, 0.5).without_finish_pass();
        let toolpath = bore.generate(&tool, &geom, &settings()).unwrap();
        let max_r = toolpath
            .segments
            .iter()
            .filter(|s| s.is_cutting())
            .filter_map(|s| s.target())
            .map(|p| p[0].hypot(p[1]))
            .fold(0.0f64, f64::max);
        assert!(close(2.0 * max_r + 2.0, 3.0), "wall Ø{}", 2.0 * max_r + 2.0);
    }

    /// A hole the cutter does not fit into, and a hole so big the helix would
    /// leave a post standing, are both refused.
    #[test]
    fn test_helical_bore_refuses_holes_it_cannot_make() {
        let tool = endmill(2.0, 10.0);
        let geom = ToolGeometry::new().with_flute_length(10.0);

        assert_eq!(
            refusal(HelicalBore::new(0.0, 0.0, 1.5, 1.0, 0.3).generate(&tool, &geom, &settings())),
            DrillError::HoleSmallerThanTool {
                hole: 1.5,
                tool: 2.0
            }
        );
        assert_eq!(
            refusal(HelicalBore::new(0.0, 0.0, 2.0, 1.0, 0.3).generate(&tool, &geom, &settings())),
            DrillError::HoleSmallerThanTool {
                hole: 2.0,
                tool: 2.0
            }
        );
        assert!(matches!(
            HelicalBore::new(0.0, 0.0, 4.0, 1.0, 0.3).generate(&tool, &geom, &settings()),
            Err(DrillError::CoreLeft { .. })
        ));
        assert!(matches!(
            HelicalBore::new(0.0, 0.0, 2.5, 1.0, 0.3).generate(
                &Tool::Drill {
                    diameter: 2.0,
                    point_angle: 118.0
                },
                &geom,
                &settings()
            ),
            Err(DrillError::NotAnEndMill { .. })
        ));
        assert!(matches!(
            HelicalBore::new(0.0, 0.0, 2.5, 1.0, 0.0).generate(&tool, &geom, &settings()),
            Err(DrillError::InvalidPitch(_))
        ));
        assert!(matches!(
            HelicalBore::new(0.0, 0.0, 2.5, 1.0, 0.3)
                .with_stock_to_leave(0.25)
                .generate(&tool, &geom, &settings()),
            Err(DrillError::StockToLeaveTooLarge { .. })
        ));
        assert!(matches!(
            HelicalBore::new(0.0, 0.0, 2.5, 12.0, 0.3).generate(&tool, &geom, &settings()),
            Err(DrillError::DepthBeyondFluteLength { .. })
        ));
    }

    /// A bore that goes through follows the same spoilboard rule.
    #[test]
    fn test_helical_bore_through_needs_a_spoilboard_only_when_it_sinks() {
        let tool = endmill(2.0, 10.0);
        let geom = ToolGeometry::new().with_flute_length(10.0);

        // A flat cutter with no allowance stops at the underside: nothing
        // sinks, so nothing sacrificial is needed.
        let flush =
            HelicalBore::new(0.0, 0.0, 2.5, 1.0, 0.3).with_break_through(BreakThrough::new(1.0));
        assert!(flush.generate(&tool, &geom, &settings()).is_ok());

        let past = HelicalBore::new(0.0, 0.0, 2.5, 1.0, 0.3)
            .with_break_through(BreakThrough::new(1.0).with_allowance(0.2));
        assert_eq!(
            refusal(past.generate(&tool, &geom, &settings())),
            DrillError::NoSpoilboard { sink: 0.2 }
        );

        let ok = HelicalBore::new(0.0, 0.0, 2.5, 1.0, 0.3).with_break_through(
            BreakThrough::new(1.0)
                .with_allowance(0.2)
                .over(Spoilboard::new(12.0)),
        );
        let toolpath = ok.generate(&tool, &geom, &settings()).unwrap();
        let deepest = toolpath
            .segments
            .iter()
            .filter(|s| s.is_cutting())
            .filter_map(|s| Some(s.target()?[2]))
            .fold(f64::INFINITY, f64::min);
        assert!(close(deepest, -1.2), "{deepest}");
    }

    #[test]
    fn test_drill_serialization() {
        let op = Drill::chip_break([(1.0, 2.0)], 5.0, 1.0, 0.25)
            .with_dwell(0.2)
            .with_break_through(BreakThrough::new(5.0).over(Spoilboard::new(12.0)));
        let json = serde_json::to_string(&op).unwrap();
        let parsed: Drill = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, op);

        let bore = HelicalBore::new(1.0, 2.0, 2.5, 1.0, 0.3);
        let json = serde_json::to_string(&bore).unwrap();
        assert_eq!(serde_json::from_str::<HelicalBore>(&json).unwrap(), bore);
    }
}
