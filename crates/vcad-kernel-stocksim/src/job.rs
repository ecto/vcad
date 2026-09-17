//! Replay a whole job into a simulated stock and grade what is left.
//!
//! [`verify_stock_against_mesh`](crate::verify_stock_against_mesh) grades a
//! stock that somebody else already cut. This module is the call in front of
//! it: hand it the ops and the part, and it builds the stock, subtracts every
//! toolpath, checks the shank and the holder, checks the rapids, and reports
//! what the octree could and could not have seen.
//!
//! # The resolution is part of the answer
//!
//! A voxel oracle cannot see a defect thinner than its own cells. The 2D
//! oracle in `vcad_kernel_cam::verify2d` is exact — it works on the polygons
//! — so it can say "0.0000 mm of gouge" and mean it. This one cannot, and the
//! failure mode is not a wrong number but a *reassuring* one. So
//! [`ResolutionReport`] states the floor, and a gouge tolerance below it does
//! not come back "no gouge": it comes back [`Verdict::Unresolved`], with the
//! resolution that would have answered the question.
//!
//! # Leftover material, and what the target mesh cannot say on its own
//!
//! Grading "material left" against a target part only works where the job was
//! *meant* to clear material. A contour job leaves the waste frame around the
//! part standing, and drops a slug out of every closed pocket it cuts around;
//! both read as megatonnes of "excess" against a target that is just the part,
//! and neither is a defect. [`MaterialLeft`] therefore brackets the leftover
//! by how far it stands off the part: the oracle is run twice, at two
//! allowances, and the difference is the material inside the band — the
//! direct analogue of the 2D oracle's wall bands. Everything beyond the band
//! is reported as stock, not as a violation.
//!
//! # What "certain" means for a holder crash or a rapid through metal
//!
//! Material only ever decreases, so anything the **finished** stock still
//! collides with was certainly there at the time: those collisions are
//! reported as certain. Anything the **starting** stock collides with might
//! have been cut away first: those are reported as possible. Neither number
//! is a guess, and the gap between them is the resolution of the question,
//! not an error bar.
//!
//! Frame: mm, Z up, stock top at Z0, tool-tip coordinates.

use serde::{Deserialize, Serialize};
use thiserror::Error;
use vcad_kernel_cam::{Tool, ToolHolder, Toolpath, ToolpathSegment};

use crate::{verify_stock_against_mesh, CollisionType, Stock, StockVerification, VerifyOptions};

/// One operation of a job: a cutter, the path it follows, and what is above it.
#[derive(Debug, Clone, Copy)]
pub struct SimOp<'a> {
    /// Operation name, for the report.
    pub name: &'a str,
    /// The cutter.
    pub tool: &'a Tool,
    /// The path, in tool-tip coordinates.
    pub toolpath: &'a Toolpath,
    /// The holder above the cutter, when one is known.
    pub holder: Option<&'a ToolHolder>,
}

impl<'a> SimOp<'a> {
    /// An operation with no declared holder.
    pub fn new(name: &'a str, tool: &'a Tool, toolpath: &'a Toolpath) -> Self {
        Self {
            name,
            tool,
            toolpath,
            holder: None,
        }
    }

    /// The same operation with a holder above the cutter.
    pub fn with_holder(mut self, holder: &'a ToolHolder) -> Self {
        self.holder = Some(holder);
        self
    }
}

/// Things this oracle refuses to guess at.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum JobError {
    /// The stock box is empty or inside-out.
    #[error("stock bounds {0:?} are empty or inside-out")]
    BadStock([f64; 6]),
    /// The resolution is not a usable length.
    #[error("stock resolution {0} must be a positive length")]
    BadResolution(f64),
    /// The target mesh is missing, malformed, or not made of triangles.
    #[error("target mesh is unusable: {0}")]
    BadTarget(String),
    /// There is nothing to replay.
    #[error("the job has no motion to replay")]
    NothingToReplay,
    /// A threshold makes no sense.
    #[error("{0}")]
    BadInput(String),
}

/// Thresholds and sampling settings for [`verify_toolpath_against_mesh`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobOptions {
    /// Stock block `[min_x, min_y, min_z, max_x, max_y, max_z]` (mm).
    pub stock: [f64; 6],
    /// Requested octree cell size (mm). The octree caps its own depth at 10,
    /// so the cell it actually uses can be coarser; [`ResolutionReport`] says
    /// which.
    pub resolution: f64,
    /// How far the cutter may enter the part before it is a gouge (mm).
    /// Below the simulation's own margin this is not a question the octree
    /// can answer, and the verdict comes back [`Verdict::Unresolved`].
    pub tolerance: f64,
    /// Leftover thickness outside the part that is still acceptable (mm) —
    /// the machining allowance a later finish pass will take.
    pub allowance: f64,
    /// How far outside the part leftover stock still counts as material left
    /// on the job's own walls (mm). One tool diameter is the usual choice:
    /// beyond that the cutter was never going to touch it.
    pub excess_band: f64,
    /// Height above the stock top a rapid must clear before it moves in XY.
    pub safe_rapid_z: f64,
    /// Cap on example violations kept per check.
    pub max_examples: usize,
}

impl JobOptions {
    /// Options for a stock block, with the usual thresholds.
    pub fn new(stock: [f64; 6], resolution: f64) -> Self {
        Self {
            stock,
            resolution,
            tolerance: 0.1,
            allowance: 0.1,
            excess_band: 2.0,
            safe_rapid_z: 0.5,
            max_examples: 8,
        }
    }
}

/// Where a check stands. `Unresolved` is the one that matters: it is not a
/// pass, not a failure, and not an error — it is the simulation saying the
/// question is finer than its own cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Verdict {
    /// The check ran and holds.
    Pass,
    /// The check ran and does not hold.
    Fail,
    /// The simulation cannot see a defect this small. Never a pass.
    Unresolved,
}

/// What the octree could and could not have seen.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolutionReport {
    /// Cell size asked for (mm).
    pub requested_resolution: f64,
    /// Octree depth actually used (the octree caps itself at 10).
    pub octree_depth: u8,
    /// Leaf cell size per axis (mm). The octree splits the *box*, so a thin
    /// stock gets thin cells in Z and coarse ones in XY.
    pub leaf_cell: [f64; 3],
    /// Spacing of the samples the grading actually visited (mm). The grader
    /// strides the leaf grid when sampling every leaf would blow its budget,
    /// so this is coarser than `leaf_cell` on a deep octree — and it, not the
    /// cell size, is what bounds the *width* of a defect that can hide.
    pub sample_pitch: [f64; 3],
    /// Margin the grader adds to both thresholds to absorb discretization
    /// (mm).
    pub sim_margin: f64,
    /// Shallowest gouge this run could report (mm): `tolerance + sim_margin`.
    pub min_detectable_depth: f64,
    /// Narrowest defect this run could have hit at all (mm): the coarsest
    /// sample pitch. A gouge narrower than this can fall between samples.
    pub min_detectable_extent: f64,
    /// Nodes in the octree after the job was subtracted.
    pub octree_nodes: usize,
    /// Approximate octree footprint (bytes).
    pub octree_bytes: usize,
    /// Grid samples the grading visited, per pass.
    pub grid_samples: usize,
}

/// Leftover material, bracketed by how far it stands off the part.
///
/// `in_band` is the analogue of the 2D oracle's `material_left`: stock still
/// standing within `band` of a wall the job was working on. `beyond_band` is
/// the rest of the blank — the waste frame, the slugs a contour job drops —
/// which is stock, not a defect, and is reported so a reader can see the
/// oracle did not simply miss it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaterialLeft {
    /// The band width the bracket was taken at (mm).
    pub band: f64,
    /// Samples of leftover material within the band.
    pub in_band_samples: usize,
    /// Volume that represents (mm³), at the sampling resolution.
    pub in_band_volume: f64,
    /// Samples of leftover material further out than the band.
    pub beyond_band_samples: usize,
    /// Volume that represents (mm³).
    pub beyond_band_volume: f64,
    /// Volume one grid sample stands for (mm³).
    pub sample_volume: f64,
    /// Farthest any leftover stands off the part (mm) — over the whole
    /// blank, so it is dominated by the waste when there is any.
    pub worst_standoff: f64,
    /// Pass when nothing is left inside the band.
    pub verdict: Verdict,
}

/// One place the shank or the holder met material.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HolderHit {
    /// Index of the op it happened in.
    pub op: usize,
    /// Motion segment within that op.
    pub segment: usize,
    /// Tool-tip position there (mm).
    pub at: [f64; 3],
    /// Shank or holder.
    pub what: String,
    /// Clearance, negative for a collision (mm).
    pub clearance: f64,
    /// True when the *finished* stock still collides here, so the crash is
    /// certain rather than merely possible.
    pub certain: bool,
}

/// Shank and holder against the stock.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HolderReport {
    /// Motion segments checked.
    pub checked: usize,
    /// Collisions with the finished stock: material that was certainly still
    /// there.
    pub certain: usize,
    /// Collisions with the starting blank: material that may have been cut
    /// away before the tool got there.
    pub possible: usize,
    /// Least clearance against the finished stock (mm).
    pub min_clearance: f64,
    /// Worst examples, certain ones first.
    pub examples: Vec<HolderHit>,
    /// Pass when nothing certainly collides. Unresolved when no op declared
    /// a holder — an undeclared holder is not a holder that clears.
    pub verdict: Verdict,
}

/// One rapid that should not have happened.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RapidHit {
    /// Index of the op.
    pub op: usize,
    /// Motion segment within that op.
    pub segment: usize,
    /// Where (mm).
    pub at: [f64; 3],
    /// How far into material, or below the safe height (mm).
    pub depth: f64,
    /// What is wrong, in a machinist's words.
    pub what: String,
    /// True when the finished stock is still solid there.
    pub certain: bool,
}

/// Rapids against the stock.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RapidsReport {
    /// Rapids with XY motion checked.
    pub checked: usize,
    /// Rapids that pass through material the finished stock still has:
    /// certain crashes.
    pub through_material: usize,
    /// Rapids that travel in XY below the safe height above the stock top.
    pub below_safe_height: usize,
    /// Deepest of either (mm).
    pub worst_depth: f64,
    /// Worst examples.
    pub examples: Vec<RapidHit>,
    /// Pass when neither count is non-zero.
    pub verdict: Verdict,
}

/// One job, replayed in three dimensions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StockJobVerification {
    /// True when every check passes. An `Unresolved` check is not a pass.
    pub pass: bool,
    /// Did the job cut into the part?
    pub gouge: Verdict,
    /// Worst intrusion found (mm); 0 when clean, and meaningless below
    /// [`ResolutionReport::min_detectable_depth`].
    pub worst_gouge: f64,
    /// The raw oracle result behind `gouge`, over the whole blank.
    pub stock: StockVerification,
    /// Leftover material, bracketed by stand-off.
    pub material_left: MaterialLeft,
    /// What the simulation could see.
    pub resolution: ResolutionReport,
    /// Shank and holder.
    pub holder: HolderReport,
    /// Rapids.
    pub rapids: RapidsReport,
    /// Operations replayed.
    pub ops: usize,
    /// Motion segments replayed (arcs count once).
    pub motion_segments: usize,
}

/// Replay a job into a simulated stock and grade it against the target part.
///
/// Builds the stock from `opts.stock`, subtracts every op's toolpath with its
/// own cutter envelope, and reports:
///
/// * **gouge** — material the part needed that the job removed, from
///   [`verify_stock_against_mesh`]. Refused (as [`Verdict::Unresolved`]) when
///   the requested tolerance is finer than the simulation's own margin.
/// * **material left** — leftover stock, bracketed into "on the job's walls"
///   and "the rest of the blank" (see [`MaterialLeft`]).
/// * **holder** — shank and holder against the finished stock (certain) and
///   against the starting blank (possible).
/// * **rapids** — through material the finished stock still has (certain),
///   and travelling in XY below the safe height.
///
/// `target_vertices` is a flat `xyz` f32 list and `target_indices` triangle
/// index triples, the tessellation layout. The target must be closed:
/// containment is decided by ray casting.
pub fn verify_toolpath_against_mesh(
    ops: &[SimOp<'_>],
    target_vertices: &[f32],
    target_indices: &[u32],
    opts: &JobOptions,
) -> Result<StockJobVerification, JobError> {
    let b = opts.stock;
    if !b.iter().all(|v| v.is_finite()) || b[3] <= b[0] || b[4] <= b[1] || b[5] <= b[2] {
        return Err(JobError::BadStock(b));
    }
    if !opts.resolution.is_finite() || opts.resolution <= 0.0 {
        return Err(JobError::BadResolution(opts.resolution));
    }
    for (name, v) in [
        ("tolerance", opts.tolerance),
        ("allowance", opts.allowance),
        ("excess_band", opts.excess_band),
        ("safe_rapid_z", opts.safe_rapid_z),
    ] {
        if !v.is_finite() || v < 0.0 {
            return Err(JobError::BadInput(format!(
                "{name} must be a finite, non-negative length, got {v}"
            )));
        }
    }
    if opts.excess_band <= 0.0 {
        return Err(JobError::BadInput(
            "excess_band must be positive: with no band there is no way to tell material \
             left on a wall from the blank around the part"
                .to_string(),
        ));
    }
    if target_vertices.is_empty() || target_indices.is_empty() {
        return Err(JobError::BadTarget(
            "no triangles: containment is decided by ray casting, which needs a closed mesh"
                .to_string(),
        ));
    }
    if !target_indices.len().is_multiple_of(3) {
        return Err(JobError::BadTarget(format!(
            "{} indices is not a whole number of triangles",
            target_indices.len()
        )));
    }
    if !target_vertices.len().is_multiple_of(3) {
        return Err(JobError::BadTarget(format!(
            "{} floats is not a whole number of xyz vertices",
            target_vertices.len()
        )));
    }
    let n_verts = target_vertices.len() / 3;
    if let Some(bad) = target_indices.iter().find(|i| **i as usize >= n_verts) {
        return Err(JobError::BadTarget(format!(
            "index {bad} points past the {n_verts} vertices given"
        )));
    }
    let motion_segments: usize = ops
        .iter()
        .map(|o| {
            o.toolpath
                .segments
                .iter()
                .filter(|s| s.target().is_some())
                .count()
        })
        .sum();
    if motion_segments == 0 {
        return Err(JobError::NothingToReplay);
    }

    // The blank, kept for the "possible collision" pass.
    let blank = Stock::from_box(b, opts.resolution);
    let mut stock = blank.clone();
    for op in ops {
        stock.subtract_toolpath(op.tool, op.toolpath);
    }

    // --- grading ----------------------------------------------------------
    let vo = |allowance: f64| VerifyOptions {
        tolerance: opts.tolerance,
        allowance,
        max_examples: opts.max_examples,
    };
    let near =
        verify_stock_against_mesh(&stock, target_vertices, target_indices, &vo(opts.allowance));
    let far = verify_stock_against_mesh(
        &stock,
        target_vertices,
        target_indices,
        &vo(opts.allowance + opts.excess_band),
    );

    let depth = stock.max_depth();
    let cell = near.cell_size;
    let n = 1usize << depth;
    // The grader strides its grid to stay inside a sample budget; recover the
    // pitch it used from the sample count it reports rather than guessing at
    // its private constant.
    let per_axis = (near.grid_samples as f64).cbrt().round().max(1.0);
    let stride = (n as f64 / per_axis).max(1.0);
    let sample_pitch = [cell[0] * stride, cell[1] * stride, cell[2] * stride];
    let stock_volume = (b[3] - b[0]) * (b[4] - b[1]) * (b[5] - b[2]);
    let sample_volume = stock_volume / near.grid_samples.max(1) as f64;

    let resolution = ResolutionReport {
        requested_resolution: opts.resolution,
        octree_depth: depth,
        leaf_cell: cell,
        sample_pitch,
        sim_margin: near.sim_margin,
        min_detectable_depth: opts.tolerance + near.sim_margin,
        min_detectable_extent: sample_pitch[0].max(sample_pitch[1]).max(sample_pitch[2]),
        octree_nodes: stock.node_count(),
        octree_bytes: stock.node_count() * std::mem::size_of::<crate::OctreeNode>(),
        grid_samples: near.grid_samples,
    };

    // A tolerance finer than the simulation's own margin is not a question
    // this octree can answer. Saying "no gouge" there would be the most
    // expensive kind of wrong.
    let gouge = if opts.tolerance < near.sim_margin {
        Verdict::Unresolved
    } else if near.gouge.pass {
        Verdict::Pass
    } else {
        Verdict::Fail
    };

    let in_band = near
        .excess
        .violation_count
        .saturating_sub(far.excess.violation_count);
    let material_left = MaterialLeft {
        band: opts.excess_band,
        in_band_samples: in_band,
        in_band_volume: in_band as f64 * sample_volume,
        beyond_band_samples: far.excess.violation_count,
        beyond_band_volume: far.excess.violation_count as f64 * sample_volume,
        sample_volume,
        worst_standoff: near.excess.worst_depth,
        verdict: if in_band == 0 {
            Verdict::Pass
        } else {
            Verdict::Fail
        },
    };

    let holder = check_holders(&stock, &blank, ops, opts);
    let rapids = check_rapids(&stock, ops, opts);

    let pass = gouge == Verdict::Pass
        && material_left.verdict == Verdict::Pass
        && holder.verdict == Verdict::Pass
        && rapids.verdict == Verdict::Pass;

    Ok(StockJobVerification {
        pass,
        gouge,
        worst_gouge: near.gouge.worst_depth,
        stock: near,
        material_left,
        resolution,
        holder,
        rapids,
        ops: ops.len(),
        motion_segments,
    })
}

/// Walk an op's motion segments as straight tip-to-tip moves.
///
/// Arcs are taken chord-wise: their endpoints bound the shank and holder
/// envelope for these two checks, and a rapid is never an arc.
///
/// The **first** motion segment only establishes where the tool is. Where the
/// spindle sat before the program's first move is not in the program, and
/// grading a move whose start we invented would be grading our own guess —
/// the same reason `verify2d::parse_gcode` refuses a cutting move until a
/// rapid has said where the tool is.
fn moves_of(op: &SimOp<'_>) -> Vec<(usize, bool, [f64; 3], [f64; 3])> {
    let mut out = Vec::new();
    let mut current: Option<[f64; 3]> = None;
    for (i, seg) in op.toolpath.segments.iter().enumerate() {
        let Some(to) = seg.target() else { continue };
        if let Some(from) = current {
            out.push((i, matches!(seg, ToolpathSegment::Rapid { .. }), from, to));
        }
        current = Some(to);
    }
    out
}

fn check_holders(
    stock: &Stock,
    blank: &Stock,
    ops: &[SimOp<'_>],
    opts: &JobOptions,
) -> HolderReport {
    let declared = ops.iter().any(|o| o.holder.is_some());
    let mut report = HolderReport {
        checked: 0,
        certain: 0,
        possible: 0,
        min_clearance: f64::INFINITY,
        examples: Vec::new(),
        verdict: Verdict::Pass,
    };
    for (oi, op) in ops.iter().enumerate() {
        for (si, _, from, to) in moves_of(op) {
            report.checked += 1;
            let now = stock.check_collision(op.tool, op.holder, from, to);
            let ever = blank.check_collision(op.tool, op.holder, from, to);
            report.min_clearance = report.min_clearance.min(now.clearance);
            if ever.collides {
                report.possible += 1;
            }
            if now.collides {
                report.certain += 1;
            }
            if now.collides || ever.collides {
                let src = if now.collides { &now } else { &ever };
                report.examples.push(HolderHit {
                    op: oi,
                    segment: si,
                    at: src.point.unwrap_or(from),
                    what: match src.collision_type {
                        Some(CollisionType::Holder) => "holder".to_string(),
                        _ => "shank".to_string(),
                    },
                    clearance: src.clearance,
                    certain: now.collides,
                });
            }
        }
    }
    if !report.min_clearance.is_finite() {
        report.min_clearance = 0.0;
    }
    report.examples.sort_by(|a, b| {
        (b.certain, -a.clearance)
            .partial_cmp(&(a.certain, -b.clearance))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    report.examples.truncate(opts.max_examples);
    report.verdict = if !declared {
        // Nothing was said about what is above the cutter. That is not the
        // same as a holder that clears.
        Verdict::Unresolved
    } else if report.certain > 0 {
        Verdict::Fail
    } else {
        Verdict::Pass
    };
    report
}

fn check_rapids(stock: &Stock, ops: &[SimOp<'_>], opts: &JobOptions) -> RapidsReport {
    let top = opts.stock[5];
    let safe = top + opts.safe_rapid_z;
    let mut report = RapidsReport {
        checked: 0,
        through_material: 0,
        below_safe_height: 0,
        worst_depth: 0.0,
        examples: Vec::new(),
        verdict: Verdict::Pass,
    };
    for (oi, op) in ops.iter().enumerate() {
        let radius = op.tool.radius();
        for (si, rapid, from, to) in moves_of(op) {
            if !rapid {
                continue;
            }
            let xy = (to[0] - from[0]).hypot(to[1] - from[1]);
            if xy <= 1e-9 {
                continue;
            }
            report.checked += 1;

            let lowest = from[2].min(to[2]);
            if lowest < safe {
                report.below_safe_height += 1;
                let d = safe - lowest;
                report.worst_depth = report.worst_depth.max(d);
                report.examples.push(RapidHit {
                    op: oi,
                    segment: si,
                    at: if from[2] <= to[2] { from } else { to },
                    depth: d,
                    what: format!(
                        "rapid travels {xy:.2} mm in XY at Z{lowest:.3}, {d:.3} mm below the \
                         safe height"
                    ),
                    certain: false,
                });
            }

            // Sample the tip path against the *finished* stock: material only
            // ever decreases, so anything still solid here was solid then.
            let steps = ((xy / (opts.resolution.max(1e-3))).ceil() as usize).clamp(2, 512);
            let mut worst = 0.0f64;
            let mut worst_at = from;
            for k in 0..=steps {
                let t = k as f64 / steps as f64;
                let p = [
                    from[0] + t * (to[0] - from[0]),
                    from[1] + t * (to[1] - from[1]),
                    from[2] + t * (to[2] - from[2]),
                ];
                if p[2] > top {
                    continue;
                }
                // The cutter is a disc of `radius` about the tip; probe the
                // centre and the rim so a rapid grazing a wall is caught.
                for (dx, dy) in [
                    (0.0, 0.0),
                    (radius, 0.0),
                    (-radius, 0.0),
                    (0.0, radius),
                    (0.0, -radius),
                ] {
                    let d = -stock.sdf_at(p[0] + dx, p[1] + dy, p[2]);
                    if d > worst {
                        worst = d;
                        worst_at = p;
                    }
                }
            }
            if worst > 0.0 {
                report.through_material += 1;
                report.worst_depth = report.worst_depth.max(worst);
                report.examples.push(RapidHit {
                    op: oi,
                    segment: si,
                    at: worst_at,
                    depth: worst,
                    what: format!(
                        "rapid passes {worst:.3} mm inside material the finished stock \
                         still has: it was certainly there at the time"
                    ),
                    certain: true,
                });
            }
        }
    }
    report.examples.sort_by(|a, b| {
        (b.certain, b.depth)
            .partial_cmp(&(a.certain, a.depth))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    report.examples.truncate(opts.max_examples);
    report.verdict = if report.through_material > 0 || report.below_safe_height > 0 {
        Verdict::Fail
    } else {
        Verdict::Pass
    };
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    use vcad_kernel_cam::fit::{offset_loop, OffsetOptions};
    use vcad_kernel_cam::outline::read_dxf;
    use vcad_kernel_cam::verify2d::{
        parse_gcode, verify_gcode, JobSpec, PartRegion, VerifyOptions as V2,
    };

    // -- target meshes -----------------------------------------------------

    /// Extrude polygon regions (outer plus holes) between two heights into a
    /// closed triangle mesh, in the tessellation layout.
    ///
    /// Ear-clipping via `earcutr` for the caps — a target mesh has to be
    /// closed for the ray-cast containment test, and this is the shortest
    /// correct route from a DXF loop to one.
    /// An outer loop and the loops cut out of it.
    type PlanarRegion = (Vec<[f64; 2]>, Vec<Vec<[f64; 2]>>);

    fn extrude(regions: &[PlanarRegion], z0: f64, z1: f64) -> (Vec<f32>, Vec<u32>) {
        let mut verts: Vec<f32> = Vec::new();
        let mut idx: Vec<u32> = Vec::new();
        for (outer, holes) in regions {
            let mut data: Vec<f64> = Vec::new();
            let mut hole_starts: Vec<usize> = Vec::new();
            for p in outer {
                data.extend_from_slice(&[p[0], p[1]]);
            }
            let mut loops = vec![outer.clone()];
            for h in holes {
                hole_starts.push(data.len() / 2);
                for p in h {
                    data.extend_from_slice(&[p[0], p[1]]);
                }
                loops.push(h.clone());
            }
            let tris = earcutr::earcut(&data, &hole_starts, 2).expect("cap triangulation");
            let base = (verts.len() / 3) as u32;
            let n = (data.len() / 2) as u32;
            for z in [z0, z1] {
                for i in 0..n as usize {
                    verts.extend_from_slice(&[
                        data[2 * i] as f32,
                        data[2 * i + 1] as f32,
                        z as f32,
                    ]);
                }
            }
            for t in tris.chunks(3) {
                // Bottom cap wound the other way so both normals point out.
                idx.extend_from_slice(&[
                    base + t[0] as u32,
                    base + t[2] as u32,
                    base + t[1] as u32,
                ]);
                idx.extend_from_slice(&[
                    base + n + t[0] as u32,
                    base + n + t[1] as u32,
                    base + n + t[2] as u32,
                ]);
            }
            let mut off = 0u32;
            for l in &loops {
                let m = l.len() as u32;
                for i in 0..m {
                    let a = base + off + i;
                    let b = base + off + (i + 1) % m;
                    idx.extend_from_slice(&[a, b, b + n, a, b + n, a + n]);
                }
                off += m;
            }
        }
        (verts, idx)
    }

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<[f64; 2]> {
        vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
    }

    fn circle(cx: f64, cy: f64, r: f64, n: usize, ccw: bool) -> Vec<[f64; 2]> {
        (0..n)
            .map(|i| {
                let a = std::f64::consts::TAU * i as f64 / n as f64;
                let a = if ccw { a } else { -a };
                [cx + r * a.cos(), cy + r * a.sin()]
            })
            .collect()
    }

    // -- fixtures ----------------------------------------------------------

    const D2: Tool = Tool::FlatEndMill {
        diameter: 2.0,
        flute_length: 6.0,
        flutes: 2,
    };

    /// `docs/cam-fixtures/stator-outline.dxf`, shifted so its lower-left
    /// corner is the stock origin — the frame the fixture's G-code is in.
    fn stator_loops() -> Vec<Vec<[f64; 2]>> {
        let dxf = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/cam-fixtures/stator-outline.dxf"
        ));
        let o = read_dxf(dxf).expect("stator outline");
        let b = o.bounds();
        let mut out = Vec::new();
        for r in &o.regions {
            out.push(
                r.outer
                    .points
                    .iter()
                    .map(|p| [p.x - b[0], p.y - b[1]])
                    .collect::<Vec<_>>(),
            );
            for h in &r.holes {
                out.push(h.points.iter().map(|p| [p.x - b[0], p.y - b[1]]).collect());
            }
        }
        assert_eq!(out.len(), 5, "1 outer + 4 holes");
        out
    }

    fn stator_gcode() -> &'static str {
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/cam-fixtures/stator-copper-d2.nc"
        ))
    }

    /// The fixture's G-code as a toolpath. `parse_gcode` has already sampled
    /// the arcs, so every segment is a straight move.
    fn toolpath_from_gcode(text: &str) -> Toolpath {
        let mut tp = Toolpath::new();
        for m in parse_gcode(text, &V2::default()).expect("the fixture parses") {
            tp.push(if m.rapid {
                ToolpathSegment::rapid(m.to[0], m.to[1], m.to[2])
            } else {
                ToolpathSegment::linear(m.to[0], m.to[1], m.to[2], m.feed)
            });
        }
        tp
    }

    // -- the agreement test ------------------------------------------------

    /// The real job, in both oracles.
    ///
    /// `docs/cam-fixtures/stator-copper-d2.nc` is four inside contours and an
    /// outside one, cut with a Ø2 end mill through 0.8 mm of copper. The 2D
    /// oracle works on the polygons and is exact; this one works on voxels.
    /// They have to agree about the thing that matters — the cutter never
    /// went inside the part — and where they differ, the difference has to be
    /// the resolution, not the answer.
    ///
    /// The target is the part **plus the waste frame**: the job cuts a 2 mm
    /// kerf around the part and leaves the rest of the blank standing, so a
    /// target that is only the part reads the whole blank as "excess". The
    /// frame's inner edge is the part grown by one tool diameter, which is
    /// where the outside contour puts it.
    #[test]
    fn the_stator_job_agrees_with_the_2d_oracle() {
        let loops = stator_loops();
        let gcode = stator_gcode();

        // --- 2D: the ground truth ---------------------------------------
        let part = PartRegion::new(loops[0].clone(), loops[1..].to_vec()).unwrap();
        let mut spec = JobSpec::new(part, 0.8, 2.0);
        spec.bottom_allowance = -0.05; // the job breaks through into the bed
        spec.spoilboard = true;
        spec.stock_bbox = Some([-2.0, -2.0, 66.0, 66.0]);
        let v2 = verify_gcode(gcode, &spec, &V2::default()).unwrap();
        assert!(v2.gouge.pass, "the 2D oracle finds no gouge");
        assert_eq!(v2.gouge.worst, 0.0);
        // It does find leftover metal on the walls — the slot wedges the
        // Ø2 cutter cannot clean out.
        assert!(!v2.material_left.check.pass);
        let left_2d_mm3 = v2.material_left.unswept_area * 0.8;
        assert!(
            (v2.material_left.unswept_area - 26.29).abs() < 0.5,
            "2D unswept {}",
            v2.material_left.unswept_area
        );

        // --- 3D ---------------------------------------------------------
        let tp = toolpath_from_gcode(gcode);
        let grown = offset_loop(&loops[0], 2.0, &OffsetOptions::default());
        assert_eq!(grown.len(), 1, "the part grows to one piece");
        let target = extrude(
            &[
                (loops[0].clone(), loops[1..].to_vec()),
                (rect(-2.0, -2.0, 66.0, 66.0), vec![grown[0].clone()]),
            ],
            -0.8,
            0.0,
        );

        let mut opts = JobOptions::new([-2.0, -2.0, -0.8, 66.0, 66.0, 0.0], 1.0);
        // 0.53 mm cells cannot certify 0.1 mm; state what they can.
        opts.tolerance = 0.2;
        opts.allowance = 0.05;
        opts.excess_band = 2.0;
        opts.safe_rapid_z = 0.5;

        let t0 = Instant::now();
        let ops = [SimOp::new("stator", &D2, &tp)];
        let v3 = verify_toolpath_against_mesh(&ops, &target.0, &target.1, &opts).unwrap();
        let elapsed = t0.elapsed();
        println!(
            "stator 3D: {:?}, {} nodes ({:.1} MB), depth {}, leaf {:?}, pitch {:?}, \
             margin {:.4}, floor {:.4}/{:.4}, gouge {:?} worst {:.4}, \
             left in band {} ({:.2} mm3) beyond {} ({:.1} mm3), rapids {:?}",
            elapsed,
            v3.resolution.octree_nodes,
            v3.resolution.octree_bytes as f64 / 1e6,
            v3.resolution.octree_depth,
            v3.resolution.leaf_cell,
            v3.resolution.sample_pitch,
            v3.resolution.sim_margin,
            v3.resolution.min_detectable_depth,
            v3.resolution.min_detectable_extent,
            v3.gouge,
            v3.worst_gouge,
            v3.material_left.in_band_samples,
            v3.material_left.in_band_volume,
            v3.material_left.beyond_band_samples,
            v3.material_left.beyond_band_volume,
            v3.rapids.verdict,
        );

        // 1. No gouge, and the claim is resolvable at the stated tolerance.
        assert_eq!(v3.gouge, Verdict::Pass, "3D vs 2D disagree about gouging");
        assert_eq!(v3.worst_gouge, 0.0);
        assert!(v3.resolution.min_detectable_depth < 0.35);
        assert_eq!(v3.motion_segments, 7580);

        // 2. Leftover material inside the band agrees with the 2D oracle in
        //    magnitude. Voxels cannot match an exact polygon area, but they
        //    must land in the same order of magnitude or one of them is
        //    measuring something else.
        let left_3d = v3.material_left.in_band_volume;
        assert!(
            left_3d > 0.25 * left_2d_mm3 && left_3d < 4.0 * left_2d_mm3,
            "3D leftover {left_3d:.2} mm³ vs 2D {left_2d_mm3:.2} mm³"
        );

        // 3. The waste frame and the freed slugs are stock, not a defect —
        //    and the report says how much of it there is rather than
        //    silently dropping it.
        assert!(
            v3.material_left.beyond_band_volume > 5.0 * left_3d,
            "the waste frame and the freed slugs dwarf what is left on the walls"
        );
        assert!(v3.material_left.worst_standoff > 10.0);

        // 4. Rapids: the 2D oracle finds none unsafe, and neither does this.
        assert!(v2.rapids.pass);
        assert_eq!(v3.rapids.through_material, 0);
        assert_eq!(v3.rapids.below_safe_height, 0);

        // 5. Nothing said what is above the cutter, so the holder check is
        //    unresolved — which keeps the whole job from passing.
        assert_eq!(v3.holder.verdict, Verdict::Unresolved);
        assert!(!v3.pass);
    }

    // -- the deliberately wrong job ----------------------------------------

    /// Friction-log item 32: an inside contour that ran on the outside of the
    /// line. The tool centre is one tool diameter further out than it should
    /// be, so the cutter eats one tool diameter of wall — and that is the
    /// number the oracle has to come back with.
    #[test]
    fn an_inside_contour_offset_outward_gouges_by_one_tool_diameter() {
        // A 30 × 30 × 6 plate with a Ø10 hole. The plate is thicker than the
        // gouge is wide on purpose: gouge depth is distance to the nearest
        // target surface, so a thin sheet caps what can be read at half its
        // thickness (on the 0.8 mm stator the same mistake reads 0.4 mm).
        let hole = circle(15.0, 15.0, 5.0, 64, false);
        let target = extrude(
            &[(rect(0.0, 0.0, 30.0, 30.0), vec![hole.clone()])],
            -6.0,
            0.0,
        );
        let opts = JobOptions {
            tolerance: 0.2,
            allowance: 0.2,
            // A contour job around a bore drops a slug, and this one sits
            // 2 mm off the wall. The band is the question "how far out do I
            // still care?", and 1 mm puts the slug where it belongs: stock,
            // not material left on a wall.
            excess_band: 1.0,
            ..JobOptions::new([0.0, 0.0, -6.0, 30.0, 30.0, 0.0], 0.5)
        };

        let path = |radius: f64| {
            let ring = circle(15.0, 15.0, radius, 64, true);
            let mut tp = Toolpath::new();
            tp.push(ToolpathSegment::rapid(ring[0][0], ring[0][1], 5.0));
            for z in [-2.0, -4.0, -6.0] {
                tp.push(ToolpathSegment::linear(ring[0][0], ring[0][1], z, 100.0));
                for p in ring.iter().skip(1).chain(std::iter::once(&ring[0])) {
                    tp.push(ToolpathSegment::linear(p[0], p[1], z, 400.0));
                }
            }
            tp.push(ToolpathSegment::rapid(ring[0][0], ring[0][1], 5.0));
            tp
        };

        // Right: the Ø2 cutter's centre runs 1 mm inside the hole wall.
        let right = path(4.0);
        let v = verify_toolpath_against_mesh(
            &[SimOp::new("bore", &D2, &right)],
            &target.0,
            &target.1,
            &opts,
        )
        .unwrap();
        assert_eq!(v.gouge, Verdict::Pass, "worst {:.4}", v.worst_gouge);
        assert_eq!(v.material_left.verdict, Verdict::Pass);
        // The slug is still in there, counted as stock rather than dropped.
        assert!(v.material_left.beyond_band_samples > 0);

        // Wrong: the same path pushed one tool diameter outward.
        let wrong = path(4.0 + 2.0);
        let v = verify_toolpath_against_mesh(
            &[SimOp::new("bore", &D2, &wrong)],
            &target.0,
            &target.1,
            &opts,
        )
        .unwrap();
        assert_eq!(v.gouge, Verdict::Fail);
        let voxel = v.resolution.sample_pitch[0].max(v.resolution.sample_pitch[1]);
        println!(
            "wrong job: worst {:.4} mm, voxel {:.4} mm, samples {}",
            v.worst_gouge, voxel, v.stock.gouge.violation_count
        );
        assert!(
            (v.worst_gouge - 2.0).abs() <= voxel,
            "worst gouge {:.4} should be one tool diameter (2.0) within a voxel ({voxel:.4})",
            v.worst_gouge
        );
        assert!(v.stock.gouge.violation_count > 100);
        assert!(!v.pass);
    }

    // -- resolution honesty -------------------------------------------------

    /// A gouge tolerance finer than the simulation's own margin is not a
    /// question voxels can answer, and the answer is not "clean".
    #[test]
    fn a_tolerance_below_the_voxel_margin_is_unresolved_not_clean() {
        let target = extrude(&[(rect(0.0, 0.0, 30.0, 30.0), vec![])], -6.0, 0.0);
        let mut tp = Toolpath::new();
        tp.push(ToolpathSegment::rapid(-5.0, -5.0, 5.0));
        tp.push(ToolpathSegment::rapid(-5.0, -5.0, -3.0));
        tp.push(ToolpathSegment::linear(-5.0, 35.0, -3.0, 400.0));

        let coarse = JobOptions {
            tolerance: 0.01,
            ..JobOptions::new([-8.0, -8.0, -6.0, 38.0, 38.0, 0.0], 2.0)
        };
        let v = verify_toolpath_against_mesh(
            &[SimOp::new("air", &D2, &tp)],
            &target.0,
            &target.1,
            &coarse,
        )
        .unwrap();
        assert!(v.resolution.sim_margin > 0.01);
        assert_eq!(
            v.gouge,
            Verdict::Unresolved,
            "0.01 mm is finer than a {:.4} mm margin",
            v.resolution.sim_margin
        );
        assert!(!v.pass, "unresolved is never a pass");
        assert!(v.resolution.min_detectable_depth > v.resolution.sim_margin);
        assert!(v.resolution.min_detectable_extent >= v.resolution.leaf_cell[0]);

        // Ask a question the same run *can* answer and it answers it.
        let stated = JobOptions {
            tolerance: 0.5,
            ..coarse
        };
        let v = verify_toolpath_against_mesh(
            &[SimOp::new("air", &D2, &tp)],
            &target.0,
            &target.1,
            &stated,
        )
        .unwrap();
        assert_eq!(v.gouge, Verdict::Pass);
    }

    /// Finer cells lower the floor. The point of the report is that the floor
    /// is a number a caller can act on, not a footnote.
    #[test]
    fn the_resolution_floor_follows_the_cell_size() {
        let target = extrude(&[(rect(0.0, 0.0, 30.0, 30.0), vec![])], -6.0, 0.0);
        let mut tp = Toolpath::new();
        tp.push(ToolpathSegment::rapid(-5.0, -5.0, 5.0));
        tp.push(ToolpathSegment::linear(-5.0, 35.0, 5.0, 400.0));
        let mut last = f64::INFINITY;
        for res in [4.0, 2.0, 1.0] {
            let v = verify_toolpath_against_mesh(
                &[SimOp::new("air", &D2, &tp)],
                &target.0,
                &target.1,
                &JobOptions::new([-8.0, -8.0, -6.0, 38.0, 38.0, 0.0], res),
            )
            .unwrap();
            assert!(
                v.resolution.sim_margin < last,
                "margin {} did not fall below {last}",
                v.resolution.sim_margin
            );
            last = v.resolution.sim_margin;
        }
    }

    // -- rapids and holders -------------------------------------------------

    /// A rapid dragged through the blank at cutting depth is a crash, and the
    /// finished stock still being solid there makes it a certain one.
    #[test]
    fn a_rapid_through_material_is_certain_and_one_above_the_stock_is_not() {
        let target = extrude(&[(rect(0.0, 0.0, 30.0, 30.0), vec![])], -6.0, 0.0);
        let opts = JobOptions::new([0.0, 0.0, -6.0, 30.0, 30.0, 0.0], 1.0);

        let mut bad = Toolpath::new();
        bad.push(ToolpathSegment::rapid(2.0, 15.0, 5.0));
        bad.push(ToolpathSegment::rapid(2.0, 15.0, -3.0));
        bad.push(ToolpathSegment::rapid(28.0, 15.0, -3.0)); // straight through
        let v = verify_toolpath_against_mesh(
            &[SimOp::new("crash", &D2, &bad)],
            &target.0,
            &target.1,
            &opts,
        )
        .unwrap();
        // One graded rapid: the plunge has no XY motion, and the program's
        // very first move only says where the tool is.
        assert_eq!(v.rapids.checked, 1);
        assert_eq!(v.rapids.through_material, 1);
        assert_eq!(v.rapids.below_safe_height, 1);
        assert_eq!(v.rapids.verdict, Verdict::Fail);
        assert!(v.rapids.examples.iter().any(|e| e.certain));
        assert!(v.rapids.worst_depth > 0.0);

        let mut good = Toolpath::new();
        good.push(ToolpathSegment::rapid(2.0, 15.0, 5.0));
        good.push(ToolpathSegment::rapid(28.0, 15.0, 5.0));
        let v = verify_toolpath_against_mesh(
            &[SimOp::new("clear", &D2, &good)],
            &target.0,
            &target.1,
            &opts,
        )
        .unwrap();
        assert_eq!(v.rapids.checked, 1);
        assert_eq!(v.rapids.through_material, 0);
        assert_eq!(v.rapids.below_safe_height, 0);
        assert_eq!(v.rapids.verdict, Verdict::Pass);
    }

    /// An undeclared holder is not a holder that clears; a declared one that
    /// buries itself in the blank is a failure.
    #[test]
    fn an_undeclared_holder_is_unresolved_and_a_short_one_fails() {
        let target = extrude(&[(rect(0.0, 0.0, 30.0, 30.0), vec![])], -30.0, 0.0);
        let opts = JobOptions::new([0.0, 0.0, -30.0, 30.0, 30.0, 0.0], 2.0);
        // A deep slot with a cutter whose flutes are far shorter than it.
        let short = Tool::FlatEndMill {
            diameter: 2.0,
            flute_length: 3.0,
            flutes: 2,
        };
        let mut tp = Toolpath::new();
        tp.push(ToolpathSegment::rapid(15.0, 2.0, 5.0));
        tp.push(ToolpathSegment::linear(15.0, 2.0, -20.0, 100.0));
        tp.push(ToolpathSegment::linear(15.0, 28.0, -20.0, 200.0));

        let v = verify_toolpath_against_mesh(
            &[SimOp::new("slot", &short, &tp)],
            &target.0,
            &target.1,
            &opts,
        )
        .unwrap();
        assert_eq!(v.holder.verdict, Verdict::Unresolved);

        let holder = ToolHolder {
            diameter: 20.0,
            length: 40.0,
            taper_angle: 0.0,
        };
        let v = verify_toolpath_against_mesh(
            &[SimOp::new("slot", &short, &tp).with_holder(&holder)],
            &target.0,
            &target.1,
            &opts,
        )
        .unwrap();
        assert_eq!(v.holder.verdict, Verdict::Fail);
        assert!(v.holder.certain > 0);
        assert!(v.holder.possible >= v.holder.certain);
        assert!(v.holder.min_clearance < 0.0);
        assert!(!v.holder.examples.is_empty());
        assert!(v.holder.examples[0].certain);
    }

    // -- fail-closed inputs -------------------------------------------------

    #[test]
    fn unusable_inputs_are_errors_not_empty_passes() {
        let target = extrude(&[(rect(0.0, 0.0, 10.0, 10.0), vec![])], -2.0, 0.0);
        let mut tp = Toolpath::new();
        tp.push(ToolpathSegment::rapid(0.0, 0.0, 5.0));
        let ops = [SimOp::new("op", &D2, &tp)];
        let base = JobOptions::new([0.0, 0.0, -2.0, 10.0, 10.0, 0.0], 0.5);

        let inside_out = JobOptions {
            stock: [10.0, 0.0, -2.0, 0.0, 10.0, 0.0],
            ..base.clone()
        };
        assert!(matches!(
            verify_toolpath_against_mesh(&ops, &target.0, &target.1, &inside_out),
            Err(JobError::BadStock(_))
        ));

        let no_res = JobOptions {
            resolution: 0.0,
            ..base.clone()
        };
        assert!(matches!(
            verify_toolpath_against_mesh(&ops, &target.0, &target.1, &no_res),
            Err(JobError::BadResolution(_))
        ));

        let no_band = JobOptions {
            excess_band: 0.0,
            ..base.clone()
        };
        assert!(matches!(
            verify_toolpath_against_mesh(&ops, &target.0, &target.1, &no_band),
            Err(JobError::BadInput(_))
        ));

        assert!(matches!(
            verify_toolpath_against_mesh(&ops, &[], &[], &base),
            Err(JobError::BadTarget(_))
        ));
        assert!(matches!(
            verify_toolpath_against_mesh(&ops, &target.0, &[0, 1], &base),
            Err(JobError::BadTarget(_))
        ));
        assert!(matches!(
            verify_toolpath_against_mesh(&ops, &target.0, &[0, 1, 99_999], &base),
            Err(JobError::BadTarget(_))
        ));

        let empty = Toolpath::new();
        assert!(matches!(
            verify_toolpath_against_mesh(
                &[SimOp::new("nothing", &D2, &empty)],
                &target.0,
                &target.1,
                &base
            ),
            Err(JobError::NothingToReplay)
        ));
    }

    /// The report is plain data all the way down.
    #[test]
    fn the_report_round_trips_through_json() {
        let target = extrude(&[(rect(0.0, 0.0, 10.0, 10.0), vec![])], -2.0, 0.0);
        let mut tp = Toolpath::new();
        tp.push(ToolpathSegment::rapid(-2.0, -2.0, 5.0));
        tp.push(ToolpathSegment::linear(-2.0, 12.0, 5.0, 400.0));
        let v = verify_toolpath_against_mesh(
            &[SimOp::new("air", &D2, &tp)],
            &target.0,
            &target.1,
            &JobOptions::new([-4.0, -4.0, -2.0, 14.0, 14.0, 0.0], 0.5),
        )
        .unwrap();
        let json = serde_json::to_string(&v).unwrap();
        let back: StockJobVerification = serde_json::from_str(&json).unwrap();
        assert_eq!(back.gouge, v.gouge);
        assert_eq!(back.resolution.octree_depth, v.resolution.octree_depth);
        assert_eq!(back.material_left.band, v.material_left.band);
        assert!(json.contains("min_detectable_depth"));
    }
}
