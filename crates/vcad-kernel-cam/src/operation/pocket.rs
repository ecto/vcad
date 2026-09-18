//! 2D pocket clearing: concentric offset rings from the wall inward.
//!
//! Two things were wrong with this before wave 2.
//!
//! The first is the one [`Contour2D`](crate::Contour2D) had until yesterday:
//! on `wasm32` the rings were offsets of the pocket's **bounding box**, so
//! every pocket the browser or the MCP server generated for a shape that was
//! not a rectangle or a circle was the wrong shape, silently. The rings now
//! come from [`fit::offset_loop`](crate::fit::offset_loop) — pure Rust, round
//! joins, the same answer on every target. `geo-clipper` survives only as the
//! reference the parity tests hold that answer to.
//!
//! The second is that it was not really a pocket. It followed the first piece
//! the offsetter returned and dropped the rest; it plunged straight down at the
//! seam; it linked one ring to the next with a move that could cross metal
//! nothing had cut yet; and it had no idea what it left behind. What is here
//! now clears a region:
//!
//! - **Islands.** A pocket with holes in it is material to keep. The rings are
//!   the level sets of the distance to the *whole* region boundary — the wall
//!   offset inward, the islands offset outward, each trimmed where the other is
//!   nearer.
//! - **Splits.** When the inward offset falls into several regions, every one
//!   of them is pocketed. On the contour side a split is a refusal
//!   ([`CamError::ContourSplit`]) because a profile has to be one loop; here it
//!   is ordinary, and the thing that must not happen is a piece going uncut
//!   without a word.
//! - **Links that stay down.** The move from one ring to the next goes to the
//!   nearest point of that ring, which is at most a stepover away; when the
//!   stepover is at most the tool radius that whole move lies inside the disc
//!   the cutter has just swept, so it can never travel through uncut metal. A
//!   larger stepover makes that impossible rather than merely undesirable — the
//!   strip between the two swept bands is metal no earlier move touched — so it
//!   is refused, not quietly cut.
//! - **Entry.** A ramp along the innermost ring, which on a round ring is a
//!   helix. Never a straight plunge unless the caller asks for one.

use crate::fit::{offset_loop, OffsetOptions, UnreachableCorner};
use crate::geom2d;
use crate::operation::Contour;
use crate::stock::{AllowanceRefusal, BottomAllowance, Spoilboard};
use crate::verify2d::{march, Loop2, Poly};
use crate::{CamError, CamSettings, CutDirection, EntryStyle, Tool, Toolpath, ToolpathSegment};
use serde::{Deserialize, Serialize};

/// How far apart the rings are.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Stepover {
    /// A fraction of the cutter's diameter.
    Fraction {
        /// The fraction. Half is the most a stay-down link allows; 0.45 is the
        /// usual choice.
        of_diameter: f64,
    },
    /// An absolute distance, in mm.
    Absolute {
        /// The distance, in mm.
        mm: f64,
    },
}

impl Stepover {
    /// A fraction of the cutter's diameter.
    pub fn fraction(of_diameter: f64) -> Self {
        Self::Fraction { of_diameter }
    }

    /// An absolute distance, in mm.
    pub fn mm(mm: f64) -> Self {
        Self::Absolute { mm }
    }

    /// The spacing in mm for a cutter of this diameter.
    pub fn distance(&self, diameter: f64) -> f64 {
        match self {
            Self::Fraction { of_diameter } => of_diameter * diameter,
            Self::Absolute { mm } => *mm,
        }
    }
}

/// Metal a pocket leaves standing: a corner tighter than the cutter, or a
/// passage it could not get into at all.
pub type UncutPatch = UnreachableCorner;

/// What a [`Pocket2D`] actually did: the numbers a machinist needs to decide
/// whether to run it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PocketReport {
    /// Depth actually cut below Z0, in mm (positive), after the bottom
    /// allowance.
    pub final_depth: f64,
    /// Distance between rings, in mm.
    pub stepover: f64,
    /// Z levels the pocket was taken down in.
    pub z_levels: usize,
    /// Distinct ring offsets, including the finishing offset when there is one.
    pub ring_levels: usize,
    /// Connected regions the pocket falls into at the first roughing offset.
    /// More than one means the cutter cannot travel between them.
    pub regions: usize,
    /// Ring laps emitted, over every Z level.
    pub rings: usize,
    /// Ring-to-ring links: moves that stay inside metal already cut.
    pub links: usize,
    /// Entries made by ramping down along a ring.
    pub ramp_entries: usize,
    /// Entries that could only go straight down.
    pub plunge_entries: usize,
    /// Finishing laps on the true offset, at full depth.
    pub finish_passes: usize,
    /// Plan area of the pocket itself: the wall less its islands (mm²).
    pub pocket_area: f64,
    /// Plan area the cutter sweeps out of it (mm²), measured on a grid.
    pub cleared_area: f64,
    /// Plan area left standing (mm²).
    pub leftover_area: f64,
    /// Each leftover patch, largest first.
    pub leftover: Vec<UncutPatch>,
    /// Grid pitch the areas were measured on (mm).
    pub measure_grid: f64,
}

/// 2D pocket clearing operation.
///
/// Clears the material inside a closed contour — less any islands — with
/// concentric offset rings, from the wall inward.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pocket2D {
    /// The wall: the closed boundary of the material to remove.
    pub contour: Contour,
    /// Islands inside it: material to keep. Each is a closed contour lying
    /// inside `contour`.
    #[serde(default)]
    pub islands: Vec<Contour>,
    /// Depth to cut (positive value, measured from Z=0).
    pub depth: f64,
    /// Stock to leave on the wall and on the islands, in mm. Taken off by a
    /// full-depth finishing lap on the true offset.
    pub stock_to_leave: f64,
    /// Ring spacing. `None` takes [`CamSettings::stepover`], which is how this
    /// operation has always been driven.
    #[serde(default)]
    pub stepover: Option<Stepover>,
    /// Climb or conventional, chosen independently of the contour's winding.
    #[serde(default)]
    pub direction: CutDirection,
    /// How the cutter gets down to the depth of a pass.
    #[serde(default)]
    pub entry: EntryStyle,
    /// Ramp angle in degrees, the same convention as [`Contour2D`](crate::Contour2D).
    #[serde(default = "default_ramp_angle")]
    pub ramp_angle: f64,
    /// Feed for the finishing lap (mm/min). Falls back to the job's feed rate.
    #[serde(default)]
    pub finish_feed: Option<f64>,
    /// Positive leaves an onion skin, negative cuts that far past the underside
    /// of the stock. [`BottomAllowance`]'s sign convention, written out once
    /// there.
    #[serde(default)]
    pub bottom_allowance: f64,
    /// The sacrificial board under the stock. Required before a negative bottom
    /// allowance is allowed.
    #[serde(default, alias = "spoilboard_thickness")]
    pub spoilboard: Option<Spoilboard>,
    /// Grid pitch for the cleared and leftover areas in the report (mm). `None`
    /// picks one from the tool.
    #[serde(default)]
    pub measure_grid: Option<f64>,
}

fn default_ramp_angle() -> f64 {
    3.0
}

/// Sampling for the offsetter.
///
/// The same numbers [`Contour2D`](crate::Contour2D) uses, for the reason
/// measured there: at a 0.01 mm step the stator's offset still moves 0.0135 mm
/// between one refinement and the next, all of it at the cusps where the curve
/// is trimmed; at 0.002 mm it is within 0.14 µm of converged. The result is
/// thinned to 1 µm so the machine is not handed moves shorter than it can
/// accelerate through.
fn offset_options() -> OffsetOptions {
    OffsetOptions {
        step: 0.002,
        simplify: 1e-3,
        ..OffsetOptions::default()
    }
}

/// Slack on the test that trims one candidate curve where another is nearer.
///
/// The candidates are exact offset samples — the offsetter keeps only points
/// within a nanometre of the offset distance, and thinning drops points rather
/// than moving them — so this only has to swallow the sagitta of a stitched
/// crossing.
const LEVEL_TOLERANCE: f64 = 1e-4;

/// Most laps a ramp entry may take before it gives up and goes straight down.
const MAX_RAMP_LAPS: usize = 64;

/// Sanity bound on the number of ring offsets, so a pathological stepover
/// cannot spin.
const MAX_RING_LEVELS: usize = 4096;

impl Pocket2D {
    /// Create a new pocket operation.
    pub fn new(contour: Contour, depth: f64) -> Self {
        Self {
            contour,
            islands: Vec::new(),
            depth,
            stock_to_leave: 0.0,
            stepover: None,
            direction: CutDirection::default(),
            entry: EntryStyle::default(),
            ramp_angle: default_ramp_angle(),
            finish_feed: None,
            bottom_allowance: 0.0,
            spoilboard: None,
            measure_grid: None,
        }
    }

    /// Set stock to leave on the wall.
    pub fn with_stock_to_leave(mut self, stock: f64) -> Self {
        self.stock_to_leave = stock;
        self
    }

    /// Keep an island: material inside the pocket that stays.
    pub fn with_island(mut self, island: Contour) -> Self {
        self.islands.push(island);
        self
    }

    /// Set the ring spacing.
    pub fn with_stepover(mut self, stepover: Stepover) -> Self {
        self.stepover = Some(stepover);
        self
    }

    /// Climb or conventional milling.
    pub fn with_direction(mut self, direction: CutDirection) -> Self {
        self.direction = direction;
        self
    }

    /// Ramp angle for entries, in degrees.
    pub fn with_ramp_angle(mut self, degrees: f64) -> Self {
        self.ramp_angle = degrees;
        self
    }

    /// Enter straight down instead of ramping. An end mill has no cutting edge
    /// at its centre; only ask for this when the tool can take it.
    pub fn with_plunge_entry(mut self) -> Self {
        self.entry = EntryStyle::Plunge;
        self
    }

    /// Feed for the finishing lap (mm/min).
    pub fn with_finish_feed(mut self, feed: f64) -> Self {
        self.finish_feed = Some(feed);
        self
    }

    /// Leave an onion skin (positive) or break through (negative), in mm.
    pub fn with_bottom_allowance(mut self, allowance: f64) -> Self {
        self.bottom_allowance = allowance;
        self
    }

    /// Declare the sacrificial board under the stock, in mm.
    pub fn with_spoilboard(mut self, thickness: f64) -> Self {
        self.spoilboard = Some(Spoilboard::new(thickness));
        self
    }

    /// How far below the stock top the flutes really go, in mm.
    ///
    /// This, not [`Pocket2D::depth`], is what the tool-geometry checks have to
    /// see: a negative allowance is a break-through and sinks the cutter
    /// *past* the depth asked for.
    pub fn reached_depth(&self) -> f64 {
        BottomAllowance(self.bottom_allowance).final_depth(self.depth)
    }

    /// Grid pitch for the areas in the report, in mm.
    pub fn with_measure_grid(mut self, grid: f64) -> Self {
        self.measure_grid = Some(grid);
        self
    }

    /// Create a rectangular pocket.
    pub fn rectangle(x: f64, y: f64, width: f64, height: f64, depth: f64) -> Self {
        Self::new(Contour::rectangle(x, y, width, height), depth)
    }

    /// Create a circular pocket.
    pub fn circle(cx: f64, cy: f64, radius: f64, depth: f64) -> Self {
        Self::new(Contour::circle(cx, cy, radius), depth)
    }

    /// Generate the toolpath for this pocket operation.
    pub fn generate(&self, tool: &Tool, settings: &CamSettings) -> Result<Toolpath, CamError> {
        self.generate_reported(tool, settings).map(|(path, _)| path)
    }

    /// Generate the toolpath and a report of what it does: rings, regions,
    /// entries, and how much metal is left where.
    pub fn generate_reported(
        &self,
        tool: &Tool,
        settings: &CamSettings,
    ) -> Result<(Toolpath, PocketReport), CamError> {
        let radius = tool.radius();
        let stepover = self.check(tool, settings)?;

        // The stock's underside stays at -depth however the allowance moves the
        // cut: a break-through eats into the spoilboard, an onion skin stops
        // short.
        let final_depth = BottomAllowance(self.bottom_allowance)
            .check(self.depth, self.spoilboard)
            .map_err(|refusal| match refusal {
                AllowanceRefusal::BreakThroughWithoutSpoilboard {
                    overcut,
                    spoilboard,
                } => CamError::BreakThroughWithoutSpoilboard {
                    overcut,
                    spoilboard,
                },
                AllowanceRefusal::SpoilboardNotAThickness { declared } => {
                    CamError::SpoilboardNotAThickness { declared }
                }
                AllowanceRefusal::ExceedsDepth { allowance, depth } => {
                    CamError::BottomAllowanceExceedsDepth { allowance, depth }
                }
            })?;

        let region = self.region()?;
        let rings = self.rings(&region, radius, stepover)?;
        let z_levels = levels(
            final_depth,
            (final_depth / settings.stepdown).ceil() as usize,
        );

        let mut report = PocketReport {
            final_depth,
            stepover,
            z_levels: z_levels.len(),
            ring_levels: rings.levels.len(),
            regions: rings.regions,
            ..PocketReport::default()
        };

        let mut toolpath = Toolpath::new();
        toolpath.push(ToolpathSegment::comment(format!(
            "Pocket 2D: depth={:.3}mm, {}, stepover={:.3}mm, {} ring offset(s), {} region(s), \
             {} island(s), stock_to_leave={:.3}mm",
            final_depth,
            match self.direction {
                CutDirection::Climb => "climb",
                CutDirection::Conventional => "conventional",
            },
            stepover,
            rings.levels.len(),
            rings.regions,
            self.islands.len(),
            self.stock_to_leave,
        )));
        if self.bottom_allowance > 0.0 {
            toolpath.push(ToolpathSegment::comment(format!(
                "bottom allowance: {:.3}mm of skin left below the cut",
                self.bottom_allowance
            )));
        } else if self.bottom_allowance < 0.0 {
            toolpath.push(ToolpathSegment::comment(format!(
                "break-through: {:.3}mm past the stock into a {:.3}mm spoilboard",
                -self.bottom_allowance,
                self.spoilboard.map_or(0.0, |s| s.thickness)
            )));
        }

        let mut cut = Cutter {
            toolpath: &mut toolpath,
            settings,
            radius,
            feed: settings.feed_rate,
            finish_feed: self.finish_feed.unwrap_or(settings.feed_rate),
            ramp_angle: self.ramp_angle,
            entry: self.entry,
            at: None,
            report: &mut report,
        };

        let mut entry_z = 0.0;
        for (index, z) in z_levels.iter().enumerate() {
            let final_z = index + 1 == z_levels.len();
            cut.toolpath
                .push(ToolpathSegment::comment(format!("Z level: {z:.3}")));
            for root in &rings.roots {
                cut.subtree(&rings, *root, None, *z, entry_z, final_z);
                cut.retract();
            }
            entry_z = *z;
        }

        self.measure(&region, &rings, radius, tool, &mut report);
        for patch in report.leftover.iter().take(4) {
            toolpath.push(ToolpathSegment::comment(format!(
                "metal left: {:.3}mm\u{b2} at ({:.2}, {:.2}), standing {:.3}mm off the wall",
                patch.area, patch.centroid[0], patch.centroid[1], patch.standoff
            )));
        }

        Ok((toolpath, report))
    }

    /// Everything that has to be true before a ring is computed. Returns the
    /// stepover in mm.
    fn check(&self, tool: &Tool, settings: &CamSettings) -> Result<f64, CamError> {
        let radius = tool.radius();
        if radius <= 0.0 {
            return Err(CamError::InvalidToolDiameter(tool.diameter()));
        }
        if self.depth <= 0.0 {
            return Err(CamError::InvalidDepth(self.depth));
        }
        if settings.stepdown <= 0.0 {
            return Err(CamError::InvalidStepdown(settings.stepdown));
        }
        if settings.feed_rate <= 0.0 {
            return Err(CamError::InvalidFeedRate(settings.feed_rate));
        }
        if let Some(feed) = self.finish_feed {
            if feed <= 0.0 {
                return Err(CamError::InvalidFeedRate(feed));
            }
        }
        if self.ramp_angle <= 0.0 || self.ramp_angle >= 90.0 {
            return Err(CamError::InvalidRampAngle(self.ramp_angle));
        }
        if self.stock_to_leave < 0.0 {
            return Err(CamError::InvalidStockToLeave(self.stock_to_leave));
        }
        if !self.contour.is_closed(0.01) {
            let gap = self.contour.start.distance_to(&self.contour.end_point());
            return Err(CamError::NotClosed(gap));
        }
        self.contour.check_arcs()?;
        for island in &self.islands {
            if !island.is_closed(0.01) {
                let gap = island.start.distance_to(&island.end_point());
                return Err(CamError::NotClosed(gap));
            }
            island.check_arcs()?;
        }

        let stepover = self
            .stepover
            .map_or(settings.stepover, |s| s.distance(tool.diameter()));
        if stepover <= 0.0 || stepover > tool.diameter() {
            return Err(CamError::InvalidStepover(stepover));
        }
        // A ring is a stepover from the next one, and the cutter sweeps a
        // radius either side of itself. Past that the strip between the two
        // swept bands is metal nothing has touched, and the move from one ring
        // to the next has to cut its way across — a slot the width of the
        // cutter, taken blind, at the feed of a finishing pass. No ordering of
        // the rings avoids it, so this is refused rather than made an option.
        if stepover > radius + 1e-9 {
            return Err(CamError::Operation(format!(
                "a stepover of {stepover:.3} mm is more than this cutter's {radius:.3} mm radius, \
                 so the move from one ring to the next would cut through metal no earlier pass \
                 reached. Use {radius:.3} mm or less (45% of the diameter is the default)."
            )));
        }
        Ok(stepover)
    }

    /// The pocket as one indexed region: the wall, then the islands as holes.
    fn region(&self) -> Result<Poly, CamError> {
        let wall = ring_of(&self.contour);
        let mut loops = vec![wall.clone()];
        for island in &self.islands {
            let l = ring_of(island);
            if l.len() < 3 {
                return Err(CamError::EmptyContour);
            }
            // An island outside the wall has a silent, wrong answer: the
            // offsetter would carve a second pocket around it.
            if !l.iter().all(|p| geom2d::point_in_loop(*p, &wall)) {
                return Err(CamError::Operation(
                    "an island reaches outside the pocket wall; islands are material kept inside \
                     the pocket, not around it"
                        .into(),
                ));
            }
            loops.push(l);
        }
        Poly::new(loops).map_err(|e| CamError::Operation(e.to_string()))
    }

    /// The tool-centre curve at clearance `a` from the region boundary: the
    /// wall offset inward and the islands offset outward, each trimmed where
    /// the other is nearer, then stitched back together at the crossings.
    ///
    /// With no islands this is exactly
    /// [`fit::offset_loop`](crate::fit::offset_loop): there is nothing to trim
    /// against, and the candidates come back untouched.
    fn centre_loops(&self, region: &Poly, a: f64) -> Vec<Loop2> {
        let opts = offset_options();
        let mut candidates: Vec<Loop2> = offset_loop(&ring_of(&self.contour), -a, &opts);
        if self.islands.is_empty() {
            return candidates;
        }
        for island in &self.islands {
            for mut l in offset_loop(&ring_of(island), a, &opts) {
                // Grown outward the offsetter hands back a counter-clockwise
                // loop; as a hole of the clear region it winds the other way.
                l.reverse();
                candidates.push(l);
            }
        }
        trim_and_stitch(candidates, region, a)
    }

    /// Every ring the pocket needs, and which ring each one steps out to.
    fn rings(&self, region: &Poly, radius: f64, stepover: f64) -> Result<RingSet, CamError> {
        let finishing = self.stock_to_leave > 1e-9;
        let first_rough = radius + self.stock_to_leave;

        let mut levels: Vec<f64> = Vec::new();
        let mut rings: Vec<Ring> = Vec::new();
        let mut regions = 0usize;

        if finishing {
            let loops = self.centre_loops(region, radius);
            if loops.is_empty() {
                return Err(CamError::EmptyPocketOffset);
            }
            levels.push(radius);
            push_level(&mut rings, loops, 0, true, self.direction)?;
        }

        let mut a = first_rough;
        loop {
            let loops = self.centre_loops(region, a);
            if loops.is_empty() {
                break;
            }
            let level = levels.len();
            if level == usize::from(finishing) {
                // Connected pieces of the first roughing offset: how many
                // separate pockets the cutter has to be entered into.
                regions = loops
                    .iter()
                    .filter(|l| geom2d::signed_area(l) > 0.0)
                    .count();
            }
            levels.push(a);
            push_level(&mut rings, loops, level, false, self.direction)?;
            a += stepover;
            if levels.len() > MAX_RING_LEVELS {
                return Err(CamError::Operation(format!(
                    "the pocket needs more than {MAX_RING_LEVELS} rings at a {stepover:.4} mm \
                     stepover; the stepover is almost certainly wrong"
                )));
            }
        }

        if rings.is_empty() {
            // Nothing survives being eroded by the tool radius: the cutter does
            // not fit in this pocket at all.
            return Err(CamError::EmptyPocketOffset);
        }
        if finishing && levels.len() == 1 {
            // The finishing offset fits but the roughing one does not. Cutting
            // only the finish lap would leave the middle of the pocket full.
            return Err(CamError::Operation(format!(
                "the pocket takes a lap at {radius:.3} mm from the wall but nothing at \
                 {first_rough:.3} mm: {:.3} mm of stock to leave is more than it has room for",
                self.stock_to_leave
            )));
        }

        let (children, roots) = link_levels(&rings, levels.len());
        Ok(RingSet {
            rings,
            levels,
            regions,
            children,
            roots,
        })
    }

    /// Grid measurement of what the rings sweep and what they leave: the same
    /// marching-squares reading [`fit::fit_contour`](crate::fit::fit_contour)
    /// takes, but against the path actually generated rather than an idealised
    /// centre region — a ring is a curve, and dilating the region it came from
    /// would flatter the job by up to one stepover.
    fn measure(
        &self,
        region: &Poly,
        rings: &RingSet,
        radius: f64,
        tool: &Tool,
        report: &mut PocketReport,
    ) {
        let grid = self
            .measure_grid
            .unwrap_or_else(|| (tool.diameter() / 20.0).clamp(0.02, 0.1));
        report.measure_grid = grid;
        report.pocket_area = geom2d::signed_area(&ring_of(&self.contour)).abs()
            - self
                .islands
                .iter()
                .map(|i| geom2d::signed_area(&ring_of(i)).abs())
                .sum::<f64>();

        let mut segments: Vec<([f64; 2], [f64; 2])> = Vec::new();
        for ring in &rings.rings {
            let n = ring.pts.len();
            for k in 0..n {
                segments.push((ring.pts[k], ring.pts[(k + 1) % n]));
            }
        }
        let swept = Poly::from_segments(segments);
        let b = region.bbox();
        let bbox = [b[0] - grid, b[1] - grid, b[2] + grid, b[3] + grid];
        let mut patches: Vec<UncutPatch> = march(bbox, grid, |p| {
            let inside = region.signed_distance(p);
            (inside.min(swept.distance(p) - radius), inside)
        })
        .into_iter()
        // A speck this size is the fringe the grid shaves off a real corner's
        // tip rather than metal — `fit`'s filter, for `fit`'s reason.
        .filter(|c| c.area >= 0.01)
        .map(|c| UncutPatch {
            area: c.area,
            centroid: c.centroid,
            standoff: c.peak_aux,
        })
        .collect();
        patches.sort_by(|a, b| b.area.total_cmp(&a.area));
        report.leftover_area = patches.iter().map(|c| c.area).sum();
        report.cleared_area = report.pocket_area - report.leftover_area;
        report.leftover = patches;
    }
}

// ---------------------------------------------------------------------------
// Rings
// ---------------------------------------------------------------------------

/// One closed ring at one offset, measured once so it can be walked from any
/// point and asked how far away anything is.
struct Ring {
    pts: Loop2,
    /// Path length from `pts[0]` to each vertex; the last entry is the lap.
    cum: Vec<f64>,
    level: usize,
    /// The finishing lap on the true offset, cut once at full depth.
    finish: bool,
    poly: Poly,
}

impl Ring {
    fn new(pts: Loop2, level: usize, finish: bool) -> Result<Self, CamError> {
        let poly = Poly::new(vec![pts.clone()]).map_err(|e| CamError::Operation(e.to_string()))?;
        let mut cum = Vec::with_capacity(pts.len() + 1);
        let mut s = 0.0;
        cum.push(0.0);
        for k in 0..pts.len() {
            s += geom2d::distance(pts[k], pts[(k + 1) % pts.len()]);
            cum.push(s);
        }
        Ok(Self {
            pts,
            cum,
            level,
            finish,
            poly,
        })
    }

    fn total(&self) -> f64 {
        *self.cum.last().unwrap_or(&0.0)
    }

    /// The point at path length `s`; it wraps.
    fn at(&self, s: f64) -> [f64; 2] {
        let total = self.total();
        if total <= 0.0 {
            return self.pts[0];
        }
        let s = s.rem_euclid(total);
        let k = match self.cum.binary_search_by(|c| c.total_cmp(&s)) {
            Ok(k) => k.min(self.pts.len() - 1),
            Err(k) => k.saturating_sub(1).min(self.pts.len() - 1),
        };
        let (a, b) = (self.pts[k], self.pts[(k + 1) % self.pts.len()]);
        let len = self.cum[k + 1] - self.cum[k];
        if len <= 0.0 {
            return a;
        }
        let t = (s - self.cum[k]) / len;
        [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
    }

    /// Vertex path lengths strictly between `a` and `b`, wrapping as often as
    /// the span needs.
    fn vertices_between(&self, a: f64, b: f64) -> Vec<f64> {
        let total = self.total();
        let mut out = Vec::new();
        if total <= 0.0 || b <= a {
            return out;
        }
        let first = (a / total).floor() as i64;
        let last = (b / total).floor() as i64;
        for m in first..=last {
            let base = m as f64 * total;
            for c in &self.cum[..self.cum.len() - 1] {
                let s = c + base;
                if s > a + 1e-12 && s < b - 1e-12 {
                    out.push(s);
                }
            }
        }
        out.sort_by(f64::total_cmp);
        out
    }

    /// Distance from `p` to the ring, and the path length of the point that
    /// answers it.
    fn nearest(&self, p: [f64; 2]) -> (f64, f64) {
        let n = self.pts.len();
        let mut best = (f64::INFINITY, 0.0);
        for k in 0..n {
            let (a, b) = (self.pts[k], self.pts[(k + 1) % n]);
            let q = geom2d::nearest_on_segment(p, a, b);
            let d = geom2d::distance(p, q);
            if d < best.0 {
                best = (d, self.cum[k] + geom2d::distance(a, q));
            }
        }
        best
    }
}

/// Every ring of a pocket, and which ring each one steps out to.
struct RingSet {
    rings: Vec<Ring>,
    levels: Vec<f64>,
    regions: usize,
    children: Vec<Vec<usize>>,
    roots: Vec<usize>,
}

/// The contour as a bare closed loop.
fn ring_of(contour: &Contour) -> Loop2 {
    let polygon = contour.to_geo_polygon();
    let ring: Loop2 = polygon.exterior().0.iter().map(|c| [c.x, c.y]).collect();
    geom2d::clean_loop(&ring)
}

/// Equal Z steps down to `depth`, deepest last, as negative Z values.
fn levels(depth: f64, count: usize) -> Vec<f64> {
    let n = count.max(1);
    (1..=n).map(|k| -depth * k as f64 / n as f64).collect()
}

/// Wind each loop of one level for the cut direction and add it to the set.
///
/// The winding the offsetter produced decides nothing. Climb puts the material
/// on the right of the direction of travel, which around the pocket wall — an
/// opening — is counter-clockwise, and around an island — a part — is
/// clockwise.
fn push_level(
    rings: &mut Vec<Ring>,
    loops: Vec<Loop2>,
    level: usize,
    finish: bool,
    direction: CutDirection,
) -> Result<(), CamError> {
    for l in loops {
        if l.len() < 3 {
            continue;
        }
        let ccw = geom2d::signed_area(&l) > 0.0;
        let want_ccw = (direction == CutDirection::Climb) == ccw;
        let mut pts = l;
        if ccw != want_ccw {
            pts.reverse();
        }
        rings.push(Ring::new(pts, level, finish)?);
    }
    Ok(())
}

/// Which rings step out to which, and the rings that step out to nothing.
///
/// A ring is a stepover from the ring it came from, so its parent is the
/// nearest ring at the offset outside it. Parents are claimed shortest-first
/// and each is claimed once before any is claimed twice: where a region pinches
/// in two, the halves then take the two rings of the annulus they came from
/// instead of both taking the same one and leaving the other to be entered
/// from scratch.
fn link_levels(rings: &[Ring], levels: usize) -> (Vec<Vec<usize>>, Vec<usize>) {
    let mut parent: Vec<Option<usize>> = vec![None; rings.len()];
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); rings.len()];
    for level in 1..levels {
        let kids: Vec<usize> = (0..rings.len())
            .filter(|&i| rings[i].level == level)
            .collect();
        let ups: Vec<usize> = (0..rings.len())
            .filter(|&i| rings[i].level == level - 1)
            .collect();
        if ups.is_empty() {
            continue;
        }
        let mut pairs: Vec<(f64, usize, usize)> = Vec::with_capacity(kids.len() * ups.len());
        for &k in &kids {
            for &u in &ups {
                let d = rings[k]
                    .pts
                    .iter()
                    .map(|p| rings[u].poly.distance(*p))
                    .fold(f64::INFINITY, f64::min);
                pairs.push((d, k, u));
            }
        }
        pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut taken = vec![false; rings.len()];
        for &(_, k, u) in &pairs {
            if parent[k].is_none() && !taken[u] {
                parent[k] = Some(u);
                taken[u] = true;
            }
        }
        for &(_, k, u) in &pairs {
            if parent[k].is_none() {
                parent[k] = Some(u);
            }
        }
        for &k in &kids {
            if let Some(u) = parent[k] {
                children[u].push(k);
            }
        }
    }
    let roots = (0..rings.len()).filter(|&i| parent[i].is_none()).collect();
    (children, roots)
}

// ---------------------------------------------------------------------------
// Trimming one level's candidate curves against each other
// ---------------------------------------------------------------------------

/// Keep only the parts of each candidate curve that really are `a` from the
/// nearest piece of the region boundary, and stitch what survives back into
/// closed loops.
///
/// The same shape as the trim inside
/// [`fit::offset_loop`](crate::fit::offset_loop) — sample, drop what is nearer
/// to something else, rejoin the runs at their crossings — applied between
/// curves instead of within one.
fn trim_and_stitch(candidates: Vec<Loop2>, region: &Poly, a: f64) -> Vec<Loop2> {
    let mut whole: Vec<Loop2> = Vec::new();
    let mut runs: Vec<Loop2> = Vec::new();
    for cand in candidates {
        if cand.len() < 3 {
            continue;
        }
        let keep: Vec<bool> = cand
            .iter()
            .map(|p| region.signed_distance(*p) >= a - LEVEL_TOLERANCE)
            .collect();
        if keep.iter().all(|k| *k) {
            whole.push(cand);
            continue;
        }
        if keep.iter().all(|k| !k) {
            continue;
        }
        let m = cand.len();
        let start = (0..m).find(|&i| keep[i] && !keep[(i + m - 1) % m]).unwrap();
        let mut cur: Loop2 = Vec::new();
        for k in 0..m {
            let i = (start + k) % m;
            if keep[i] {
                cur.push(cand[i]);
            } else if !cur.is_empty() {
                runs.push(std::mem::take(&mut cur));
            }
        }
        if !cur.is_empty() {
            runs.push(cur);
        }
    }
    runs.retain(|r| r.len() >= 2);
    if runs.is_empty() {
        return whole;
    }

    // Each run's end meets another run's start at the crossing where one
    // candidate curve gives way to the other. Shortest pairs first, globally,
    // for the reason `offset_loop` gives: taking each run's nearest free start
    // in program order lets an early run steal a partner and stitch a chord
    // across a corner.
    let mut pairs: Vec<(f64, usize, usize)> = Vec::with_capacity(runs.len() * runs.len());
    for (i, run) in runs.iter().enumerate() {
        let end = *run.last().unwrap();
        for (j, other) in runs.iter().enumerate() {
            pairs.push((geom2d::distance(end, other[0]), i, j));
        }
    }
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut next = vec![usize::MAX; runs.len()];
    let mut used = vec![false; runs.len()];
    for (_, i, j) in pairs {
        if next[i] == usize::MAX && !used[j] {
            next[i] = j;
            used[j] = true;
        }
    }

    let mut seen = vec![false; runs.len()];
    let mut out = whole;
    for i in 0..runs.len() {
        if seen[i] {
            continue;
        }
        let mut pts: Loop2 = Vec::new();
        let mut j = i;
        while j != usize::MAX && !seen[j] {
            seen[j] = true;
            pts.extend_from_slice(&runs[j]);
            let k = next[j];
            if k != usize::MAX {
                if let Some(x) = crossing(&runs[j], &runs[k], region, a) {
                    pts.push(x);
                }
            }
            j = k;
        }
        let l = geom2d::clean_loop(&pts);
        if l.len() >= 3 && geom2d::signed_area(&l).abs() > 1e-9 {
            out.push(l);
        }
    }
    out
}

/// Where two runs' curves cross: the last segment of one against the first of
/// the other. A join that is not at the offset distance, or further off than
/// the gap it is bridging, is not a crossing.
fn crossing(a: &[[f64; 2]], b: &[[f64; 2]], region: &Poly, at: f64) -> Option<[f64; 2]> {
    if a.len() < 2 || b.len() < 2 {
        return None;
    }
    let (p1, p2) = (a[a.len() - 2], a[a.len() - 1]);
    let (q1, q2) = (b[1], b[0]);
    let r1 = [p2[0] - p1[0], p2[1] - p1[1]];
    let r2 = [q2[0] - q1[0], q2[1] - q1[1]];
    let den = r1[0] * r2[1] - r1[1] * r2[0];
    if den.abs() < 1e-15 {
        return None;
    }
    let t = ((q2[0] - p2[0]) * r2[1] - (q2[1] - p2[1]) * r2[0]) / den;
    if t < 0.0 {
        return None;
    }
    let x = [p2[0] + r1[0] * t, p2[1] + r1[1] * t];
    let gap = geom2d::distance(p2, q2);
    if geom2d::distance(x, p2) > 4.0 * gap + 1e-6 {
        return None;
    }
    (region.signed_distance(x) >= at - 1e-3).then_some(x)
}

// ---------------------------------------------------------------------------
// Emission
// ---------------------------------------------------------------------------

/// The cutter's state while one operation is written out.
struct Cutter<'a> {
    toolpath: &'a mut Toolpath,
    settings: &'a CamSettings,
    radius: f64,
    feed: f64,
    finish_feed: f64,
    ramp_angle: f64,
    entry: EntryStyle,
    /// Where the cutter is, when it is down in the cut. `None` means it is up
    /// at the safe height and the next ring needs an entry of its own.
    at: Option<[f64; 2]>,
    report: &'a mut PocketReport,
}

impl Cutter<'_> {
    /// One ring and everything inside it, innermost first.
    ///
    /// Each child is cut whole before the next one starts, because the cutter
    /// cannot get from one child to another without crossing metal it has not
    /// cut. Only the last child ends beside this ring, so only it steps out to
    /// it; the others are lifted out of the cut where they finish.
    fn subtree(
        &mut self,
        rings: &RingSet,
        index: usize,
        exit_to: Option<usize>,
        z: f64,
        entry_z: f64,
        final_z: bool,
    ) {
        let kids = &rings.children[index];
        let cut_here = final_z || !rings.rings[index].finish;
        for (k, child) in kids.iter().enumerate() {
            let last = k + 1 == kids.len();
            let exit = if last && cut_here { Some(index) } else { None };
            self.subtree(rings, *child, exit, z, entry_z, final_z);
            if !last || !cut_here {
                self.retract();
            }
        }
        if cut_here {
            self.lap(rings, index, exit_to, z, entry_z);
        }
    }

    /// One lap of a ring: get onto it, go round it, and come off it where the
    /// next ring is nearest.
    fn lap(&mut self, rings: &RingSet, index: usize, exit_to: Option<usize>, z: f64, entry_z: f64) {
        let ring = &rings.rings[index];
        let total = ring.total();
        if total <= 0.0 {
            return;
        }
        let feed = if ring.finish {
            self.finish_feed
        } else {
            self.feed
        };

        let arrival = self.at.map(|p| ring.nearest(p));
        let linked = matches!(arrival, Some((d, _)) if d <= self.radius + 1e-6);
        let mut s_in = arrival.map_or(0.0, |(_, s)| s);

        if linked {
            // The move is shorter than the cutter's radius and starts where the
            // cutter just was, so every point of it lies inside the disc the
            // cutter has already swept there.
            let p = ring.at(s_in);
            self.toolpath
                .push(ToolpathSegment::comment("link to the next ring"));
            self.toolpath
                .push(ToolpathSegment::linear(p[0], p[1], z, feed));
            self.report.links += 1;
        } else {
            self.retract();
            s_in = self.enter(ring, s_in, z, entry_z, feed);
        }

        let s_out = match exit_to {
            Some(next) => {
                let q = rings.rings[next].at(rings.rings[next].nearest(ring.at(s_in)).1);
                ring.nearest(q).1
            }
            None => s_in,
        };
        // A full lap, and then on round to the point the next ring is nearest.
        let end = s_in + total + (s_out - s_in).rem_euclid(total);

        self.toolpath.push(ToolpathSegment::comment(if ring.finish {
            "finish lap on the true offset"
        } else {
            "ring"
        }));
        self.run(ring, s_in, end, z, z, feed);
        if ring.finish {
            self.report.finish_passes += 1;
        }
        self.report.rings += 1;
        self.at = Some(ring.at(end));
    }

    /// Get down to `z` on a ring with uncut metal beside it: ramp along the
    /// ring, round and round when one lap is not long enough. On a round ring
    /// that is a helix.
    fn enter(&mut self, ring: &Ring, s_in: f64, z: f64, entry_z: f64, feed: f64) -> f64 {
        let p = ring.at(s_in);
        self.toolpath.push(ToolpathSegment::comment("approach"));
        self.toolpath
            .push(ToolpathSegment::rapid(p[0], p[1], self.settings.safe_z));
        // Down to where the last Z level finished — air, all of it — and only
        // then into the metal.
        self.toolpath.push(ToolpathSegment::linear(
            p[0],
            p[1],
            entry_z,
            self.settings.plunge_rate,
        ));
        let drop = entry_z - z;
        let total = ring.total();
        let needed = if self.entry == EntryStyle::Ramp && drop > 1e-9 {
            drop / self.ramp_angle.to_radians().tan()
        } else {
            0.0
        };
        if needed <= 1e-9 || needed > total * MAX_RAMP_LAPS as f64 {
            self.toolpath
                .push(ToolpathSegment::comment(if needed > 1e-9 {
                    "plunge entry: this ring is too short to ramp down"
                } else {
                    "plunge entry"
                }));
            self.toolpath.push(ToolpathSegment::linear(
                p[0],
                p[1],
                z,
                self.settings.plunge_rate,
            ));
            self.report.plunge_entries += 1;
            self.at = Some(p);
            return s_in;
        }
        self.toolpath.push(ToolpathSegment::comment(format!(
            "ramp entry: {:.1} deg, {:.2} mm, {:.1} lap(s) of the ring",
            self.ramp_angle,
            needed,
            needed / total
        )));
        self.run(ring, s_in, s_in + needed, entry_z, z, feed);
        self.report.ramp_entries += 1;
        let end = s_in + needed;
        self.at = Some(ring.at(end));
        end
    }

    /// Walk the ring from `s0` to `s1`, descending in step with the distance
    /// travelled.
    fn run(&mut self, ring: &Ring, s0: f64, s1: f64, z0: f64, z1: f64, feed: f64) {
        let span = s1 - s0;
        if span <= 1e-12 {
            return;
        }
        for s in ring
            .vertices_between(s0, s1)
            .into_iter()
            .chain(std::iter::once(s1))
        {
            let t = ((s - s0) / span).clamp(0.0, 1.0);
            let p = ring.at(s);
            self.toolpath.push(ToolpathSegment::linear(
                p[0],
                p[1],
                z0 + (z1 - z0) * t,
                feed,
            ));
        }
    }

    /// Straight up out of the cut. Never a rapid that travels in XY at depth —
    /// the move the contour operation had to have fixed out of it.
    fn retract(&mut self) {
        if let Some(p) = self.at.take() {
            self.toolpath
                .push(ToolpathSegment::rapid(p[0], p[1], self.settings.safe_z));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operation::Point2D;
    use crate::verify2d::{verify_toolpath, JobSpec, PartRegion, Severity, VerifyOptions};

    fn mill(diameter: f64) -> Tool {
        Tool::FlatEndMill {
            diameter,
            flute_length: 25.0,
            flutes: 2,
        }
    }

    fn cam(stepdown: f64) -> CamSettings {
        CamSettings {
            stepover: 3.0,
            stepdown,
            feed_rate: 600.0,
            plunge_rate: 150.0,
            spindle_rpm: 12000.0,
            safe_z: 5.0,
            retract_z: 10.0,
        }
    }

    fn polyline(points: &[(f64, f64)]) -> Contour {
        let mut c = Contour::new(Point2D::new(points[0].0, points[0].1));
        for p in points.iter().skip(1).chain(std::iter::once(&points[0])) {
            c.line_to(Point2D::new(p.0, p.1));
        }
        c
    }

    fn contour_of(points: &[[f64; 2]]) -> Contour {
        let mut c = Contour::new(Point2D::new(points[0][0], points[0][1]));
        for p in points.iter().skip(1).chain(std::iter::once(&points[0])) {
            c.line_to(Point2D::new(p[0], p[1]));
        }
        c
    }

    /// One motion of the toolpath, with the comment that introduced it. The
    /// comments say which part of the operation a move belongs to (`ramp
    /// entry`, `link to the next ring`, `ring`), so a test can measure each on
    /// its own.
    #[derive(Debug, Clone)]
    struct Move {
        tag: String,
        from: [f64; 3],
        to: [f64; 3],
        rapid: bool,
    }

    impl Move {
        fn a(&self) -> [f64; 2] {
            [self.from[0], self.from[1]]
        }
        fn b(&self) -> [f64; 2] {
            [self.to[0], self.to[1]]
        }
    }

    fn moves(toolpath: &Toolpath) -> Vec<Move> {
        let mut out = Vec::new();
        let mut tag = String::new();
        let mut at = [0.0, 0.0, 0.0];
        for seg in &toolpath.segments {
            match seg {
                ToolpathSegment::Comment { text } => tag = text.clone(),
                _ => {
                    if let Some(to) = seg.target() {
                        out.push(Move {
                            tag: tag.clone(),
                            from: at,
                            to,
                            rapid: seg.is_rapid(),
                        });
                        at = to;
                    }
                }
            }
        }
        out
    }

    /// Cutting moves that reach `z` or below at both ends.
    fn cuts_at(toolpath: &Toolpath, z: f64) -> Vec<([f64; 2], [f64; 2])> {
        moves(toolpath)
            .iter()
            .filter(|m| {
                !m.rapid
                    && m.from[2] <= z + 1e-9
                    && m.to[2] <= z + 1e-9
                    && (m.to[0] - m.from[0]).hypot(m.to[1] - m.from[1]) > 1e-12
            })
            .map(|m| (m.a(), m.b()))
            .collect()
    }

    /// The fixture's loops, largest area first, read the way a caller would.
    fn stator_loops() -> Vec<Loop2> {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/cam-fixtures/stator-outline.dxf"
        );
        let text = std::fs::read_to_string(path).expect("the stator fixture");
        let outline = crate::outline::read_dxf(&text).expect("the fixture parses");
        let mut loops: Vec<Loop2> = Vec::new();
        for region in &outline.regions {
            loops.push(region.outer.points.iter().map(|p| [p.x, p.y]).collect());
            for hole in &region.holes {
                loops.push(hole.points.iter().map(|p| [p.x, p.y]).collect());
            }
        }
        assert_eq!(loops.len(), 5, "1 outer + 4 holes");
        loops.sort_by(|a, b| {
            geom2d::signed_area(b)
                .abs()
                .total_cmp(&geom2d::signed_area(a).abs())
        });
        loops
    }

    /// The same, moved so the outline's lower-left corner is the stock origin.
    fn stator_in_stock_frame() -> Vec<Loop2> {
        let loops = stator_loops();
        let (ox, oy) = loops[0]
            .iter()
            .fold((f64::INFINITY, f64::INFINITY), |(x, y), p| {
                (x.min(p[0]), y.min(p[1]))
            });
        loops
            .iter()
            .map(|l| l.iter().map(|p| [p[0] - ox, p[1] - oy]).collect())
            .collect()
    }

    /// Two 20 mm lobes joined by a neck of the given width.
    fn dumbbell(neck: f64) -> Contour {
        let half = neck / 2.0;
        polyline(&[
            (0.0, 0.0),
            (20.0, 0.0),
            (20.0, 10.0 - half),
            (30.0, 10.0 - half),
            (30.0, 0.0),
            (50.0, 0.0),
            (50.0, 20.0),
            (30.0, 20.0),
            (30.0, 10.0 + half),
            (20.0, 10.0 + half),
            (20.0, 20.0),
            (0.0, 20.0),
        ])
    }

    // -----------------------------------------------------------------------
    // The rings themselves
    // -----------------------------------------------------------------------

    /// The bug this package exists for: on `wasm32` the rings used to be
    /// offsets of the pocket's bounding box. They are now the same pure-Rust
    /// offset on every target, and this holds it to `geo-clipper` on a shape a
    /// bounding box gets badly wrong — a pocket with two facing notches — and
    /// on the real stator opening.
    #[cfg(not(target_arch = "wasm32"))]
    mod ring_parity {
        use super::*;
        use geo_clipper::Clipper;

        /// `geo-clipper`'s inward offset. `Round(100.0)` at a scale of 100 000
        /// is an arc tolerance of a micron, so the reference is the one being
        /// held to a standard here, not the thing under test.
        fn clipper_inward(ring: &Loop2, a: f64) -> Vec<Loop2> {
            let coords: Vec<geo::Coord<f64>> = ring
                .iter()
                .map(|p| geo::Coord { x: p[0], y: p[1] })
                .collect();
            let poly = geo::Polygon::new(geo::LineString::from(coords), vec![]);
            poly.offset(
                -a,
                geo_clipper::JoinType::Round(100.0),
                geo_clipper::EndType::ClosedPolygon,
                100_000.0,
            )
            .0
            .iter()
            .map(|p| p.exterior().0.iter().map(|c| [c.x, c.y]).collect())
            .collect()
        }

        fn one_way(a: &[Loop2], b: &[Loop2]) -> f64 {
            a.iter()
                .flatten()
                .map(|p| {
                    b.iter()
                        .map(|ring| geom2d::distance_to_loop(*p, ring))
                        .fold(f64::INFINITY, f64::min)
                })
                .fold(0.0, f64::max)
        }

        fn hausdorff(a: &[Loop2], b: &[Loop2]) -> f64 {
            one_way(a, b).max(one_way(b, a))
        }

        fn check(name: &str, ring: &Loop2, a: f64) {
            let pocket = Pocket2D::new(contour_of(ring), 1.0);
            let region = pocket.region().unwrap();
            let ours = pocket.centre_loops(&region, a);
            let theirs = clipper_inward(&geom2d::clean_loop(ring), a);
            assert_eq!(
                ours.len(),
                theirs.len(),
                "{name} at {a} mm: {} rings pure-Rust, {} with clipper",
                ours.len(),
                theirs.len()
            );
            if ours.is_empty() {
                return;
            }
            let h = hausdorff(&ours, &theirs);
            assert!(h <= 0.01, "{name} at {a} mm: the rings differ by {h:.5} mm");
            println!(
                "{name} at {a} mm: {} ring(s), Hausdorff {h:.5} mm",
                ours.len()
            );
        }

        /// A pocket with a 4 mm waist between two notches: a bounding-box
        /// offset misses it entirely, and past the waist the ring falls in two.
        fn notched() -> Loop2 {
            vec![
                [0.0, 0.0],
                [13.0, 0.0],
                [13.0, 8.0],
                [17.0, 8.0],
                [17.0, 0.0],
                [30.0, 0.0],
                [30.0, 20.0],
                [17.0, 20.0],
                [17.0, 12.0],
                [13.0, 12.0],
                [13.0, 20.0],
                [0.0, 20.0],
            ]
        }

        #[test]
        fn rings_match_clipper_on_a_non_convex_pocket() {
            let ring = notched();
            for a in [1.0, 1.5, 2.5, 4.0] {
                check("notched", &ring, a);
            }
            // The interesting case really did fire: past the 4 mm waist the
            // ring is two rings, not one.
            let pocket = Pocket2D::new(contour_of(&ring), 1.0);
            let region = pocket.region().unwrap();
            assert_eq!(pocket.centre_loops(&region, 1.5).len(), 1);
            assert!(pocket.centre_loops(&region, 2.5).len() > 1);
        }

        #[test]
        fn rings_match_clipper_on_the_stator_opening() {
            let loops = stator_loops();
            for a in [1.0, 1.8, 3.0] {
                check("stator bore", &loops[1], a);
            }
        }
    }

    // -----------------------------------------------------------------------
    // What the toolpath does
    // -----------------------------------------------------------------------

    /// The stepover guarantee, measured: every point of the pocket the cutter
    /// can stand on is within a tool radius of a cutting move at full depth, so
    /// no strip wider than the cutter is left between the rings.
    ///
    /// Mutation check: the same measurement over rings built two diameters
    /// apart — the spacing this operation refuses — leaves points more than a
    /// radius from anything, and the assertion fires.
    #[test]
    fn no_strip_is_left_between_the_rings() {
        let ring = stator_loops().remove(1);
        let tool = mill(2.0);
        let radius = tool.radius();
        let pocket = Pocket2D::new(contour_of(&ring), 1.0)
            .with_stepover(Stepover::fraction(0.45))
            .with_measure_grid(0.05);
        let (toolpath, report) = pocket.generate_reported(&tool, &cam(0.5)).unwrap();

        let cuts = cuts_at(&toolpath, -report.final_depth);
        let region = pocket.region().unwrap();

        let worst = |cuts: &[([f64; 2], [f64; 2])]| {
            let b = region.bbox();
            let mut worst: f64 = 0.0;
            let mut y = b[1];
            while y <= b[3] {
                let mut x = b[0];
                while x <= b[2] {
                    let p = [x, y];
                    if region.signed_distance(p) >= radius {
                        let d = cuts
                            .iter()
                            .map(|(a, b)| geom2d::point_segment_distance(p, *a, *b))
                            .fold(f64::INFINITY, f64::min);
                        worst = worst.max(d);
                    }
                    x += 0.2;
                }
                y += 0.2;
            }
            worst
        };
        let gap = worst(&cuts);
        assert!(
            gap <= radius + 1e-9,
            "a reachable point is {gap:.4} mm from the nearest pass; the cutter reaches {radius}"
        );
        // Not vacuous: half the stepover is what the guarantee is made of, and
        // the measurement uses most of it.
        assert!(gap > report.stepover / 2.0 * 0.8, "{gap:.4}");

        // The mutation: rings two diameters apart instead of 0.45 of one.
        let sparse = pocket.rings(&region, radius, 4.0).unwrap();
        let mutated: Vec<([f64; 2], [f64; 2])> = sparse
            .rings
            .iter()
            .flat_map(|r| {
                let n = r.pts.len();
                (0..n).map(move |k| (r.pts[k], r.pts[(k + 1) % n]))
            })
            .collect();
        assert!(
            worst(&mutated) > radius,
            "a 4 mm spacing has to leave metal a 2 mm cutter cannot reach"
        );
    }

    /// The worst distance any link move runs outside metal already cut.
    fn worst_link(moves: &[Move], radius: f64) -> f64 {
        let mut swept: Vec<(f64, [f64; 2], [f64; 2])> = Vec::new();
        let mut worst: f64 = f64::NEG_INFINITY;
        for m in moves {
            let z = m.from[2].max(m.to[2]);
            if m.tag.starts_with("link") {
                for k in 0..=20 {
                    let t = k as f64 / 20.0;
                    let p = [
                        m.from[0] + (m.to[0] - m.from[0]) * t,
                        m.from[1] + (m.to[1] - m.from[1]) * t,
                    ];
                    let d = swept
                        .iter()
                        // Only what was cut at this depth or below: a pass
                        // higher up left metal under it.
                        .filter(|(sz, _, _)| *sz <= z + 1e-9)
                        .map(|(_, a, b)| geom2d::point_segment_distance(p, *a, *b))
                        .fold(f64::INFINITY, f64::min);
                    worst = worst.max(d - radius);
                }
            }
            if !m.rapid {
                swept.push((m.from[2].max(m.to[2]), m.a(), m.b()));
            }
        }
        worst
    }

    /// Ring-to-ring links stay inside metal already cut.
    ///
    /// Each link is replayed against everything cut before it at that depth:
    /// the whole move has to lie within a tool radius of an earlier pass, which
    /// is exactly "the cutter has been here". A fresh entry is the other case,
    /// and is made from the safe height rather than across the work.
    ///
    /// Mutation check: splicing one straight move from one side of the pocket
    /// to the other into the finished path makes the same checker fail.
    #[test]
    fn a_link_never_travels_through_uncut_metal() {
        let ring = stator_loops().remove(1);
        let tool = mill(2.0);
        let radius = tool.radius();
        let (toolpath, report) = Pocket2D::new(contour_of(&ring), 1.0)
            .with_stepover(Stepover::fraction(0.45))
            .with_measure_grid(0.1)
            .generate_reported(&tool, &cam(0.5))
            .unwrap();
        assert!(report.links > 10, "{} links", report.links);

        let worst = worst_link(&moves(&toolpath), radius);
        assert!(
            worst <= 1e-6,
            "a link runs {worst:.4} mm beyond the metal already cut"
        );

        // The same of the two shapes where the rings are not one nested family:
        // a pocket round an island, and one that falls into two lobes.
        for (name, path) in [
            (
                "island",
                Pocket2D::rectangle(0.0, 0.0, 50.0, 34.0, 2.0)
                    .with_island(contour_of(&[
                        [18.0, 12.0],
                        [32.0, 12.0],
                        [32.0, 22.0],
                        [18.0, 22.0],
                    ]))
                    .with_stepover(Stepover::fraction(0.45))
                    .with_measure_grid(0.1)
                    .generate(&mill(4.0), &cam(1.0))
                    .unwrap(),
            ),
            (
                "dumbbell",
                Pocket2D::new(dumbbell(1.4), 1.0)
                    .with_stepover(Stepover::fraction(0.45))
                    .with_measure_grid(0.1)
                    .generate(&tool, &cam(0.5))
                    .unwrap(),
            ),
        ] {
            let r = if name == "island" { 2.0 } else { radius };
            let worst = worst_link(&moves(&path), r);
            assert!(
                worst <= 1e-6,
                "{name}: a link runs {worst:.4} mm beyond the metal already cut"
            );
        }

        // No rapid crosses the work at depth either: an entry comes from above.
        for m in moves(&toolpath).into_iter().skip(1) {
            if m.rapid && (m.to[0] - m.from[0]).hypot(m.to[1] - m.from[1]) > 1e-9 {
                assert!(
                    m.from[2] >= 5.0 - 1e-9 && m.to[2] >= 5.0 - 1e-9,
                    "a rapid travels in XY at Z {:.3}..{:.3}",
                    m.from[2],
                    m.to[2]
                );
            }
        }

        // The mutation: one move straight across the pocket, tagged as a link,
        // spliced in as soon as the first ring is cut — when almost all of the
        // pocket is still solid. (Later on the rings have covered so much that
        // a move anywhere lands near something already cut, which is the point:
        // the rule bites where it matters, at the start.)
        let mut spliced = moves(&toolpath);
        let bad = spliced
            .iter()
            .position(|m| m.tag == "ring")
            .expect("a ring lap");
        let there = spliced[bad].to;
        spliced.insert(
            bad + 1,
            Move {
                tag: "link to the next ring".into(),
                from: there,
                to: [there[0] + 12.0, there[1] + 12.0, there[2]],
                rapid: false,
            },
        );
        assert!(
            worst_link(&spliced, radius) > 1.0,
            "the check has to notice a move straight across the pocket"
        );
    }

    /// A pocket enters by ramping round the innermost ring — on a round ring
    /// that is a helix — and never by driving the cutter's dead centre into the
    /// metal.
    #[test]
    fn a_pocket_ramps_in_and_never_plunges_by_default() {
        let tool = mill(4.0);
        let settings = cam(1.5);
        let (toolpath, report) = Pocket2D::circle(0.0, 0.0, 12.0, 4.5)
            .with_stepover(Stepover::fraction(0.45))
            .generate_reported(&tool, &settings)
            .unwrap();
        assert_eq!(report.plunge_entries, 0, "{report:?}");
        assert_eq!(
            report.ramp_entries, report.z_levels,
            "one entry per Z level"
        );

        // Every ramp descends at the angle asked for, along the ring.
        for m in moves(&toolpath) {
            if !m.tag.starts_with("ramp entry") || m.rapid {
                continue;
            }
            let run = (m.to[0] - m.from[0]).hypot(m.to[1] - m.from[1]);
            let drop = m.from[2] - m.to[2];
            if run < 1e-6 {
                continue;
            }
            let angle = (drop / run).atan().to_degrees();
            assert!(
                (angle - 3.0).abs() < 0.2,
                "a ramp leg falls at {angle:.2} deg, not 3"
            );
        }

        // Asked for, a plunge is a plunge — the fallback is never silent.
        let (_, plunged) = Pocket2D::circle(0.0, 0.0, 12.0, 4.5)
            .with_stepover(Stepover::fraction(0.45))
            .with_plunge_entry()
            .generate_reported(&tool, &settings)
            .unwrap();
        assert_eq!(plunged.ramp_entries, 0);
        assert_eq!(plunged.plunge_entries, plunged.z_levels);
    }

    /// A pocket that falls into two lobes is two pockets: both are cut, each
    /// entered on its own, and the neck the cutter cannot get into is reported
    /// rather than passed over.
    ///
    /// Reported and not refused, because "the cutter cannot reach all of it" is
    /// a continuum — the stator's slot corners are the same fact at a fraction
    /// of a mm² apiece — and an operation whose job is to clear what it can
    /// should say what it left, not decline the work. The caller has the
    /// number, and the verification oracle has it independently.
    #[test]
    fn a_neck_narrower_than_the_cutter_makes_two_pockets() {
        let tool = mill(2.0);
        let (toolpath, report) = Pocket2D::new(dumbbell(1.4), 1.0)
            .with_stepover(Stepover::fraction(0.45))
            .with_measure_grid(0.05)
            .generate_reported(&tool, &cam(0.5))
            .unwrap();

        assert_eq!(report.regions, 2, "two lobes: {report:?}");
        assert!(
            report.ramp_entries >= 2 * report.z_levels,
            "each lobe is entered on its own: {} entries over {} levels",
            report.ramp_entries,
            report.z_levels
        );

        // Both lobes really are cut, not one of them twice.
        let cuts = cuts_at(&toolpath, -1.0);
        let left = cuts.iter().filter(|(a, _)| a[0] < 20.0).count();
        let right = cuts.iter().filter(|(a, _)| a[0] > 30.0).count();
        assert!(left > 50 && right > 50, "left {left}, right {right}");

        // The neck is the leftover the report names: 10 mm long and 1.4 wide,
        // less what the cutter takes off each end where it meets a lobe.
        let neck = report
            .leftover
            .iter()
            .find(|p| (p.centroid[0] - 25.0).abs() < 1.5 && (p.centroid[1] - 10.0).abs() < 1.5)
            .unwrap_or_else(|| panic!("no leftover at the neck: {:?}", report.leftover));
        assert!(
            (8.0..16.0).contains(&neck.area),
            "the neck keeps {:.2} mm\u{b2}; 10 x 1.4 is 14",
            neck.area
        );
        // And nothing cuts into it: no pass comes within a radius of its middle.
        let mid = cuts
            .iter()
            .map(|(a, b)| geom2d::point_segment_distance([25.0, 10.0], *a, *b))
            .fold(f64::INFINITY, f64::min);
        assert!(mid > 1.0, "a pass reaches {mid:.3} mm from the neck centre");

        // A neck the cutter does fit through is one pocket, cut through.
        let (_, wide) = Pocket2D::new(dumbbell(3.0), 1.0)
            .with_stepover(Stepover::fraction(0.45))
            .with_measure_grid(0.1)
            .generate_reported(&tool, &cam(0.5))
            .unwrap();
        assert_eq!(wide.regions, 1);
    }

    /// An island is material to keep: the rings go round it, and no cutting
    /// move comes within a tool radius of it.
    #[test]
    fn an_island_is_kept_and_never_gouged() {
        let tool = mill(4.0);
        let radius = tool.radius();
        let island = [[18.0, 12.0], [32.0, 12.0], [32.0, 22.0], [18.0, 22.0]];
        let (toolpath, report) = Pocket2D::rectangle(0.0, 0.0, 50.0, 34.0, 2.0)
            .with_island(contour_of(&island))
            .with_stepover(Stepover::fraction(0.45))
            .with_measure_grid(0.05)
            .generate_reported(&tool, &cam(1.0))
            .unwrap();

        let island_poly = Poly::new(vec![island.to_vec()]).unwrap();
        let mut closest = f64::INFINITY;
        for (a, b) in cuts_at(&toolpath, -2.0) {
            closest = closest.min(island_poly.distance_to_segment(a, b));
        }
        assert!(
            closest >= radius - 0.02,
            "a pass comes {closest:.4} mm from the island; the cutter's radius is {radius}"
        );
        // And it does hug it: this is a pocket round an island, not a pocket
        // that stopped short of one.
        assert!(closest < radius + 0.05, "{closest:.4}");

        // The island's area is not part of the pocket, and what is left
        // standing is only the four corners of the wall — a square of side r
        // less a quarter disc, each. The island's own corners keep nothing: it
        // is convex, so the cutter rolls right round them, which is `fit`'s
        // answer for a convex shape from the outside.
        assert!(
            (report.pocket_area - (50.0 * 34.0 - 14.0 * 10.0)).abs() < 1e-6,
            "{}",
            report.pocket_area
        );
        let corner = radius * radius - std::f64::consts::PI * radius * radius / 4.0;
        assert_eq!(report.leftover.len(), 4, "{:?}", report.leftover);
        assert!(
            (report.leftover_area - 4.0 * corner).abs() < 0.08,
            "left {:.4} mm\u{b2}, four corners of {corner:.4} are {:.4}",
            report.leftover_area,
            4.0 * corner
        );
        for patch in &report.leftover {
            let at_corner = [[0.0, 0.0], [50.0, 0.0], [50.0, 34.0], [0.0, 34.0]]
                .iter()
                .any(|c: &[f64; 2]| geom2d::distance(patch.centroid, *c) < 1.0);
            assert!(at_corner, "leftover away from a wall corner: {patch:?}");
        }

        // An island outside the wall is a mistake with a silently wrong answer.
        let outside = Pocket2D::rectangle(0.0, 0.0, 50.0, 34.0, 2.0)
            .with_island(contour_of(&[
                [60.0, 12.0],
                [70.0, 12.0],
                [70.0, 22.0],
                [60.0, 22.0],
            ]))
            .generate(&tool, &cam(1.0));
        assert!(
            matches!(outside, Err(CamError::Operation(_))),
            "{outside:?}"
        );
    }

    /// Stock to leave is taken off by one full-depth lap on the true offset,
    /// linked out of the roughing rings rather than entered again.
    #[test]
    fn stock_to_leave_is_finished_on_the_true_offset() {
        let tool = mill(4.0);
        let radius = tool.radius();
        let wall = [[0.0, 0.0], [40.0, 0.0], [40.0, 28.0], [0.0, 28.0]];
        let (toolpath, report) = Pocket2D::new(contour_of(&wall), 3.0)
            .with_stock_to_leave(0.4)
            .with_stepover(Stepover::fraction(0.45))
            .with_finish_feed(300.0)
            .with_measure_grid(0.05)
            .generate_reported(&tool, &cam(1.0))
            .unwrap();

        assert_eq!(report.finish_passes, 1, "one lap, at the bottom");
        assert_eq!(report.z_levels, 3);

        let mut finish: Vec<Move> = Vec::new();
        let mut rough: Vec<Move> = Vec::new();
        for m in moves(&toolpath) {
            if m.rapid {
                continue;
            }
            if m.tag.starts_with("finish lap") {
                finish.push(m);
            } else if m.tag == "ring" {
                rough.push(m);
            }
        }
        assert!(!finish.is_empty());

        // The finish lap runs at exactly the tool radius from the wall, at full
        // depth, at the finishing feed.
        for m in &finish {
            let d = geom2d::distance_to_loop(m.b(), &wall);
            assert!(
                (d - radius).abs() < 0.02,
                "the finish lap is {d:.4} mm from the wall, not {radius}"
            );
            assert!((m.to[2] + 3.0).abs() < 1e-9, "at Z {:.3}", m.to[2]);
        }
        // The roughing rings all stay the stock-to-leave further out.
        for m in &rough {
            let d = geom2d::distance_to_loop(m.b(), &wall);
            assert!(
                d >= radius + 0.4 - 0.02,
                "a roughing ring comes {d:.4} mm from the wall, inside the 0.4 mm left on it"
            );
        }
        // Nothing is left standing but the four corners.
        let corner = radius * radius - std::f64::consts::PI * radius * radius / 4.0;
        assert!(
            (report.leftover_area - 4.0 * corner).abs() < 0.08,
            "left {:.4} mm\u{b2}, four corners are {:.4}",
            report.leftover_area,
            4.0 * corner
        );

        // More stock to leave than the pocket has room for is refused, not cut
        // as a single lonely finishing lap.
        // An 8 mm square has room for a lap 2 mm from its wall and none at all
        // for one 4.4 mm in.
        let fat = Pocket2D::new(
            contour_of(&[[0.0, 0.0], [8.0, 0.0], [8.0, 8.0], [0.0, 8.0]]),
            1.0,
        )
        .with_stock_to_leave(2.4)
        .with_stepover(Stepover::fraction(0.45))
        .generate(&tool, &cam(1.0));
        assert!(
            matches!(&fat, Err(CamError::Operation(m)) if m.contains("more than it has room for")),
            "{fat:?}"
        );
    }

    /// The whole point of the package: the stator's opening pocketed out with a
    /// Ø2 cutter frees nothing, with no tab and no skin. The inside contour
    /// that used to cut it let a Ø28 slug and twelve slot wedges go.
    #[test]
    fn the_stator_opening_pockets_out_with_nothing_left_loose() {
        let loops = stator_in_stock_frame();
        let tool = mill(2.0);
        let settings = CamSettings {
            feed_rate: 400.0,
            plunge_rate: 120.0,
            ..cam(0.5)
        };
        let (toolpath, report) = Pocket2D::new(contour_of(&loops[1]), 1.0)
            .with_stepover(Stepover::fraction(0.45))
            .with_bottom_allowance(-0.2)
            .with_spoilboard(3.0)
            .with_measure_grid(0.03)
            .generate_reported(&tool, &settings)
            .unwrap();

        println!(
            "stator pocket: {} ring offsets, {} laps, {} links, {} entries, {} region(s), \
             {} moves, {:.1} s, {:.3} mm² left of {:.1} mm²",
            report.ring_levels,
            report.rings,
            report.links,
            report.ramp_entries + report.plunge_entries,
            report.regions,
            toolpath.segments.len(),
            toolpath.estimated_time(),
            report.leftover_area,
            report.pocket_area,
        );

        let part = PartRegion::new(loops[0].clone(), loops[1..].to_vec()).unwrap();
        let b = part.bbox();
        let spec = JobSpec {
            bottom_allowance: -0.2,
            spoilboard: true,
            stock_bbox: Some([b[0] - 6.0, b[1] - 6.0, b[2] + 6.0, b[3] + 6.0]),
            ..JobSpec::new(part, 1.0, 2.0)
        };
        let rep = verify_toolpath(&toolpath, &spec, &VerifyOptions::default()).unwrap();

        assert!(
            rep.gouge.pass,
            "the cutter enters the part: {:?}",
            rep.gouge.examples
        );
        assert!(
            rep.loose.pieces.is_empty(),
            "the pocket frees {} piece(s): {:?}",
            rep.loose.pieces.len(),
            rep.loose.pieces
        );
        assert!(!rep.loose.skin_holds, "it does break through, on purpose");
        assert!(
            rep.rapids.pass,
            "a rapid crosses the work: {:?}",
            rep.rapids.examples
        );
        assert!(
            rep.material_left.check.pass,
            "wall left standing: {:?}",
            rep.material_left.check.examples
        );
        assert!(
            rep.plunges.pass || rep.plunges.severity == Severity::Warning,
            "{:?}",
            rep.plunges.examples
        );
        assert!((rep.depth.deepest_z + 1.2).abs() < 1e-6, "{:?}", rep.depth);

        // Nothing at all is left standing, and that is the fixture's own doing:
        // the part was drawn for a Ø2 cutter, with R1.05 fillets in every slot
        // corner. The Ø3.175 one that actually ran leaves 10.65 mm² in 24
        // corners it cannot enter (`fit`'s
        // `stator_inside_corners_a_d3175_cutter_cannot_reach`); the cutter the
        // drawing was made for leaves none, and the pocket reaches all of it.
        assert!(
            report.leftover.is_empty(),
            "left {:.4} mm\u{b2} in {} patch(es): {:?}",
            report.leftover_area,
            report.leftover.len(),
            report.leftover
        );
        assert!(
            (report.cleared_area - report.pocket_area).abs() < 1e-9,
            "cleared {:.3} of {:.3} mm\u{b2}",
            report.cleared_area,
            report.pocket_area
        );
        // The same cutter one thou wider does leave corners, so the reading
        // above is a measurement and not a blind spot.
        let (_, wider) = Pocket2D::new(contour_of(&loops[1]), 1.0)
            .with_stepover(Stepover::fraction(0.45))
            .with_measure_grid(0.03)
            .generate_reported(&mill(2.4), &settings)
            .unwrap();
        assert_eq!(wider.leftover.len(), 24, "one per slot corner pair");
    }

    // -----------------------------------------------------------------------
    // Refusals and settings
    // -----------------------------------------------------------------------

    /// A stepover wider than the cutter's radius cannot be linked: the strip
    /// between one ring's swept band and the next is metal nothing has cut.
    #[test]
    fn a_stepover_wider_than_the_radius_is_refused() {
        let tool = mill(4.0);
        let pocket = Pocket2D::rectangle(0.0, 0.0, 40.0, 30.0, 2.0);
        let err = pocket
            .clone()
            .with_stepover(Stepover::mm(2.5))
            .generate(&tool, &cam(1.0))
            .unwrap_err();
        assert!(err.to_string().contains("2.500 mm is more than"), "{err}");
        // Exactly the radius is the boundary, and it is allowed.
        assert!(pocket
            .clone()
            .with_stepover(Stepover::mm(2.0))
            .generate(&tool, &cam(1.0))
            .is_ok());
        // Wider than the cutter is the older, blunter refusal.
        assert!(matches!(
            pocket
                .with_stepover(Stepover::mm(5.0))
                .generate(&tool, &cam(1.0)),
            Err(CamError::InvalidStepover(_))
        ));
    }

    /// The bottom allowance is [`BottomAllowance`]'s, with its refusals.
    #[test]
    fn the_bottom_allowance_leaves_a_skin_or_needs_a_board() {
        let tool = mill(4.0);
        let pocket =
            Pocket2D::rectangle(0.0, 0.0, 40.0, 30.0, 3.0).with_stepover(Stepover::fraction(0.45));

        let (_, skinned) = pocket
            .clone()
            .with_bottom_allowance(0.2)
            .generate_reported(&tool, &cam(1.0))
            .unwrap();
        assert!((skinned.final_depth - 2.8).abs() < 1e-9);

        assert!(matches!(
            pocket
                .clone()
                .with_bottom_allowance(-0.3)
                .generate(&tool, &cam(1.0)),
            Err(CamError::BreakThroughWithoutSpoilboard { .. })
        ));
        let (_, through) = pocket
            .clone()
            .with_bottom_allowance(-0.3)
            .with_spoilboard(3.0)
            .generate_reported(&tool, &cam(1.0))
            .unwrap();
        assert!((through.final_depth - 3.3).abs() < 1e-9);

        assert!(matches!(
            pocket.with_bottom_allowance(3.0).generate(&tool, &cam(1.0)),
            Err(CamError::BottomAllowanceExceedsDepth { .. })
        ));
    }

    /// Climb runs counter-clockwise round the wall of an opening and clockwise
    /// round an island, whichever way the caller drew either.
    #[test]
    fn the_cut_direction_is_the_caller_s_and_not_the_drawing_s() {
        let tool = mill(4.0);
        let island = [[18.0, 12.0], [32.0, 12.0], [32.0, 22.0], [18.0, 22.0]];
        for (direction, wall_ccw) in [
            (CutDirection::Climb, true),
            (CutDirection::Conventional, false),
        ] {
            // Drawn clockwise, which must change nothing.
            let mut wall = vec![[0.0, 0.0], [50.0, 0.0], [50.0, 34.0], [0.0, 34.0]];
            wall.reverse();
            let pocket = Pocket2D::new(contour_of(&wall), 2.0)
                .with_island(contour_of(&island))
                .with_direction(direction)
                .with_stepover(Stepover::fraction(0.45));
            let region = pocket.region().unwrap();
            let rings = pocket.rings(&region, tool.radius(), 1.8).unwrap();
            // The outermost offset, where the wall's ring and the island's are
            // still two separate loops; further in they merge and "round which
            // of them" stops having an answer.
            let outer: Vec<&Ring> = rings.rings.iter().filter(|r| r.level == 0).collect();
            assert_eq!(
                outer.len(),
                2,
                "one loop round the wall, one round the island"
            );
            for ring in outer {
                let ccw = geom2d::signed_area(&ring.pts) > 0.0;
                let round_island =
                    geom2d::distance_to_loop(ring.pts[0], &island) < tool.radius() + 0.02;
                assert_eq!(
                    ccw,
                    if round_island { !wall_ccw } else { wall_ccw },
                    "{direction:?}: the ring round {} winds {}",
                    if round_island {
                        "the island"
                    } else {
                        "the wall"
                    },
                    if ccw { "ccw" } else { "cw" }
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // The shapes the operation has always taken
    // -----------------------------------------------------------------------

    #[test]
    fn test_pocket_rectangle() {
        let pocket = Pocket2D::rectangle(0.0, 0.0, 20.0, 15.0, 5.0);
        let tool = mill(6.0);
        let settings = CamSettings {
            stepover: 3.0,
            ..cam(2.0)
        };

        let toolpath = pocket.generate(&tool, &settings).unwrap();
        assert!(!toolpath.is_empty());
        let z_comments = toolpath
            .segments
            .iter()
            .filter(|s| matches!(s, ToolpathSegment::Comment { text } if text.contains("Z level")))
            .count();
        assert_eq!(z_comments, 3, "5 mm at a 2 mm stepdown");
    }

    #[test]
    fn test_pocket_circle() {
        // A Ø4 cutter's rings are 2 mm apart at most, which is what the default
        // 45% of the diameter gives; the job's 3 mm stepover is refused.
        let pocket = Pocket2D::circle(25.0, 25.0, 10.0, 3.0);
        let tool = mill(4.0);
        assert!(matches!(
            pocket.generate(&tool, &CamSettings::default()),
            Err(CamError::Operation(_))
        ));
        let toolpath = pocket
            .with_stepover(Stepover::fraction(0.45))
            .generate(&tool, &CamSettings::default())
            .unwrap();
        assert!(!toolpath.is_empty());
    }

    #[test]
    fn test_pocket_with_stock_to_leave() {
        let pocket = Pocket2D::rectangle(0.0, 0.0, 20.0, 15.0, 5.0).with_stock_to_leave(0.5);
        assert!((pocket.stock_to_leave - 0.5).abs() < 1e-6);
    }

    #[test]
    fn test_pocket_invalid_depth() {
        let pocket = Pocket2D::rectangle(0.0, 0.0, 20.0, 15.0, -1.0);
        let result = pocket.generate(&Tool::default_endmill(), &CamSettings::default());
        assert!(matches!(result, Err(CamError::InvalidDepth(_))));
    }

    /// The operation still loads from the shape it had before wave 2.
    #[test]
    fn an_old_pocket_still_deserialises() {
        let json = r#"{
            "contour": {"start": {"x": 0.0, "y": 0.0}, "segments": [
                {"type": "Line", "to": {"x": 10.0, "y": 0.0}},
                {"type": "Line", "to": {"x": 10.0, "y": 10.0}},
                {"type": "Line", "to": {"x": 0.0, "y": 10.0}},
                {"type": "Line", "to": {"x": 0.0, "y": 0.0}}
            ]},
            "depth": 2.0,
            "stock_to_leave": 0.0
        }"#;
        let pocket: Pocket2D = serde_json::from_str(json).unwrap();
        assert!(pocket.islands.is_empty());
        assert_eq!(pocket.stepover, None);
        assert_eq!(pocket.entry, EntryStyle::Ramp);
        assert!((pocket.ramp_angle - 3.0).abs() < 1e-9);
    }
}
