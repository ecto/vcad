//! `vcad_cam_job`: a whole job, assembled, posted, and refused if it does not
//! verify.
//!
//! # Request
//!
//! ```json
//! {
//!   "name": "stator",
//!   "stock":   { "thickness": 1.0, "margin": 2.0, "bbox": [x0,y0,x1,y1], "spoilboard": 3.0 },
//!   "machine": { "name": "Anolex Ultra 2", "spindle": "dial", "class": "hobby",
//!                "max_feed": 4000, "max_accel": 400,
//!                "travel": { "min": [0,0,-80], "max": [400,300,0] },
//!                "work_offset": [10,10,-20] },
//!   "tools":   [ { "number": 1, "kind": "flat_end_mill", "diameter": 2.0, "flutes": 2,
//!                  "flute_length": 6, "stickout": 18, "shank_diameter": 3.175,
//!                  "centre_cutting": true, "holder": { "diameter": 30, "length": 40 } } ],
//!   "operations": [ { "name": "bore and slots", "tool": 1, "kind": "contour_inside",
//!                     "contour": [[x,y], …], "depth": 1.0, "stepdown": 0.17,
//!                     "feed": 250, "plunge": 40, "rpm": 13500,
//!                     "direction": "climb", "entry": "ramp", "ramp_angle": 3,
//!                     "lead_in": true, "stock_to_leave": 0.1, "finish_stepdowns": 1,
//!                     "spring_pass": false, "finish_feed": 200,
//!                     "tabs": 3, "tab_width": 4, "tab_height": 0.42,
//!                     "bottom_allowance": 0.15,
//!                     "thin_slot": { "strategy": "centre_line", "tolerance": 0.05 },
//!                     "order": 0, "role": "inside_feature", "phase": 0 } ],
//!   "options": { "arc_fit": { "tolerance": 0.01 },
//!                "tool_change": { "type": "manual_pause_reprobe", "probe_macro": null },
//!                "spin_up_seconds": 3.0, "park_z": 5.0, "safe_z": 5.0,
//!                "wcs": "G54", "end": "m2", "post": "grbl", "verify": true,
//!                "part": { "outer": [[x,y], …], "holes": [ [[x,y], …] ] } },
//!   "verify_policy": { "material_left": "warning" }
//! }
//! ```
//!
//! Operation `kind` is one of `face`, `pocket`, `contour_outside`,
//! `contour_inside`, `drill`, `helical_bore`. The geometry each one reads:
//! `rectangle: [x0,y0,x1,y1]` for `face` and a rectangular `pocket`;
//! `contour: [[x,y], …]` for a shaped `pocket` and both contours, with
//! `islands: [[[x,y], …], …]` on a pocket for material it clears around and
//! keeps;
//! `holes: [{ "x": , "y": , "depth": } | [x,y]]` plus `cycle`
//! (`spot` | `straight` | `peck` | `chip_break`), `peck_depth`, `retreat`,
//! `dwell`, `clearance`, `peck_clearance`, `through` for `drill`; and
//! `x`, `y`, `diameter`, `pitch`, `finish_pass`, `stock_to_leave`, `through`
//! for `helical_bore`.
//!
//! # Response
//!
//! ```json
//! { "name", "blocked": false, "gcode": "…",
//!   "moves": [ { "to": [x,y,z], "rapid": bool, "feed": 250 } ],
//!   "op_ranges": [ { "block": "operation", "name", "op_index", "tool",
//!                    "start", "end", "seconds" } ],
//!   "duration": { "naive_s": , "accel_aware_s": },
//!   "tool_checks": [ { "op", "op_index", "tool", "severity", "kind", "message" } ],
//!   "verification": { … JobVerification … },
//!   "policy": { "verified": true, "blocked_by": [], "warnings": ["material_left"] },
//!   "fit":    [ { "op", "op_index", "side", "report": { … FitReport … } } ],
//!   "report": [ { "op", "op_index", "report": { … ContourReport … } } ],
//!   "arc_fit": { "segments_in", "segments_out", "arcs_emitted", "max_deviation" },
//!   "island_clearance": [ { "op", "op_index", "tool_diameter", "tolerance_mm",
//!                           "islands": [ { "island", "centre_clearance_mm",
//!                                          "cut_into_mm", "kept" } ] } ],
//!   "tool_sequence": [1, 2],
//!   "notes": [ { "level": "caution", "text": "…" } ] }
//! ```
//!
//! **`gcode` is absent whenever `blocked` is true.** That is the whole point:
//! the app physically cannot export or send a job the oracle rejected, because
//! there is nothing to export.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use vcad_kernel_cam::materials::Note;
use vcad_kernel_cam::post::{GrblPost, LinuxCncPost, PostProcessor};
use vcad_kernel_cam::verify2d::{
    CheckReport, DeclaredTab, JobVerification, Severity, VerifyOptions,
};
use vcad_kernel_cam::{
    fit_arcs_reported, fit_contour, ArcDir, ArcFitOptions, ArcPlane, BlockRange, BottomAllowance,
    BreakThrough, CamOperation, CamSettings, Contour, Contour2D, ContourSegment, ContourSide,
    CutDirection, Drill, DrillCycle, Face, FitOptions, HelicalBore, Hole, Job, JobOp,
    MachineLimits, OpRole, PartRegion, Pocket2D, Program, ProgramBlock, ProgramEnd, Spoilboard,
    Stock, Tab, ThinSlotStrategy, ToolChangeStrategy, ToolEntry, ToolLibrary, ToolReach, Toolpath,
    ToolpathSegment, Wcs,
};

use crate::placement::{Placement, PlacementReq};
use crate::types::{
    contour_from, finite, non_negative, positive, settings as build_settings, MachineReq, PartReq,
    StockReq, ToolReq,
};

// ---------------------------------------------------------------------------
// Request
// ---------------------------------------------------------------------------

/// A hole centre, as `{ "x": , "y": , "depth": }` or the bare `[x, y]` the
/// outline tools hand back.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum HoleReq {
    Pair([f64; 2]),
    Full {
        x: f64,
        y: f64,
        #[serde(default)]
        depth: Option<f64>,
    },
}

impl HoleReq {
    /// The same hole, moved onto the stock.
    fn placed(&self, placement: &Placement) -> Self {
        match self {
            HoleReq::Pair(p) => {
                if p[0].is_finite() && p[1].is_finite() {
                    HoleReq::Pair(placement.apply(*p))
                } else {
                    // Leave a number that is not a number alone: `build` names
                    // it in the refusal, and arithmetic here would only turn
                    // one NaN into two.
                    self.clone()
                }
            }
            HoleReq::Full { x, y, depth } => {
                if x.is_finite() && y.is_finite() {
                    let moved = placement.apply([*x, *y]);
                    HoleReq::Full {
                        x: moved[0],
                        y: moved[1],
                        depth: *depth,
                    }
                } else {
                    self.clone()
                }
            }
        }
    }

    fn build(&self, what: &str) -> Result<Hole, String> {
        match self {
            HoleReq::Pair([x, y]) => Ok(Hole::at(
                finite(&format!("{what}.x"), *x)?,
                finite(&format!("{what}.y"), *y)?,
            )),
            HoleReq::Full { x, y, depth } => {
                let x = finite(&format!("{what}.x"), *x)?;
                let y = finite(&format!("{what}.y"), *y)?;
                match depth {
                    Some(d) => Ok(Hole::deep(x, y, positive(&format!("{what}.depth"), *d)?)),
                    None => Ok(Hole::at(x, y)),
                }
            }
        }
    }
}

/// `{ "strategy": "refuse" | "centre_line", "tolerance": 0.05 }`.
#[derive(Debug, Clone, Deserialize)]
struct ThinSlotReq {
    strategy: String,
    #[serde(default)]
    tolerance: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
struct ArcFitReq {
    tolerance: f64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ToolChangeReq {
    M6,
    ManualPauseReprobe {
        #[serde(default)]
        probe_macro: Option<String>,
    },
}

fn default_true() -> bool {
    true
}

// Not `derive(Default)`: serde's field defaults only run when the `options`
// map is present. A request without one took the derived default, and that
// derived `verify: false` returned unverified G-code with `blocked: false`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct OptionsReq {
    #[serde(default)]
    arc_fit: Option<ArcFitReq>,
    #[serde(default)]
    tool_change: Option<ToolChangeReq>,
    #[serde(default)]
    spin_up_seconds: Option<f64>,
    #[serde(default)]
    park_z: Option<f64>,
    #[serde(default)]
    safe_z: Option<f64>,
    #[serde(default)]
    wcs: Option<String>,
    #[serde(default)]
    end: Option<String>,
    #[serde(default)]
    post: Option<String>,
    #[serde(default = "default_true")]
    verify: bool,
    #[serde(default)]
    part: Option<PartReq>,
    /// Where the part sits on the stock: `{ dx, dy, rotation_deg }`, rotation
    /// first, about the stock-frame origin. Moves every operation that does
    /// not carry its own `placement`, **and** `options.part`, so a skewed job
    /// still verifies against the skewed part. See [`crate::placement`].
    #[serde(default)]
    placement: Option<PlacementReq>,
}

impl Default for OptionsReq {
    fn default() -> Self {
        Self {
            arc_fit: None,
            tool_change: None,
            spin_up_seconds: None,
            park_z: None,
            safe_z: None,
            wcs: None,
            end: None,
            post: None,
            verify: true,
            part: None,
            placement: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct OperationReq {
    #[serde(default)]
    name: Option<String>,
    tool: u32,
    kind: String,

    // geometry
    #[serde(default)]
    contour: Option<Vec<[f64; 2]>>,
    /// Material kept inside a `pocket`: closed polylines the cutter clears
    /// around and never enters. Only a pocket has them — an island in a
    /// contour operation would be a second wall, not a kept lump, and is
    /// refused rather than quietly dropped.
    ///
    /// The oracle's part region is an outer boundary and openings, which
    /// cannot express material *inside* an opening, so islands are checked
    /// separately: `island_clearance` in the response reports how close the
    /// cutter came to each one, and a cut into an island blocks the job.
    #[serde(default)]
    islands: Option<Vec<Vec<[f64; 2]>>>,
    #[serde(default)]
    holes: Option<Vec<HoleReq>>,
    #[serde(default)]
    rectangle: Option<[f64; 4]>,
    #[serde(default)]
    x: Option<f64>,
    #[serde(default)]
    y: Option<f64>,
    #[serde(default)]
    diameter: Option<f64>,
    #[serde(default)]
    pitch: Option<f64>,

    depth: f64,
    stepdown: f64,
    #[serde(default)]
    stepover: Option<f64>,
    feed: f64,
    plunge: f64,
    rpm: f64,

    #[serde(default)]
    direction: Option<String>,
    #[serde(default)]
    entry: Option<String>,
    #[serde(default)]
    ramp_angle: Option<f64>,
    #[serde(default)]
    lead_in: Option<bool>,
    #[serde(default)]
    lead_radius: Option<f64>,
    #[serde(default)]
    offset: Option<f64>,
    #[serde(default)]
    stock_to_leave: Option<f64>,
    #[serde(default)]
    finish_stepdowns: Option<usize>,
    #[serde(default)]
    spring_pass: Option<bool>,
    #[serde(default)]
    finish_feed: Option<f64>,
    #[serde(default)]
    finish_pass: Option<bool>,

    #[serde(default)]
    tabs: Option<usize>,
    /// Where the tabs go, as fractions (0..1) of the way round the cutter's
    /// own offset loop, in the direction of cut. When this is given it *is*
    /// the tab list, and `tabs` (the count) has to agree with it if both are
    /// present; leaving it out spaces `tabs` of them evenly, half a pitch in
    /// from the seam, as before.
    ///
    /// These are nominal: the kernel settles each tab onto the nearest stretch
    /// that runs straight, within half a tab pitch, so it never lands in a
    /// notch or wraps a tight corner. `tab_placement` in the response reports
    /// where each one ended up, measured off the toolpath.
    #[serde(default)]
    tab_positions: Option<Vec<f64>>,
    #[serde(default)]
    tab_width: Option<f64>,
    #[serde(default)]
    tab_height: Option<f64>,

    /// Where this operation's geometry sits on the stock. See
    /// [`crate::placement`]. Overrides `options.placement` for this operation
    /// only — and only its geometry, never the part the oracle checks against.
    #[serde(default)]
    placement: Option<PlacementReq>,

    #[serde(default)]
    bottom_allowance: Option<f64>,
    #[serde(default)]
    thin_slot: Option<ThinSlotReq>,

    #[serde(default)]
    cycle: Option<String>,
    #[serde(default)]
    peck_depth: Option<f64>,
    #[serde(default)]
    retreat: Option<f64>,
    #[serde(default)]
    dwell: Option<f64>,
    #[serde(default)]
    clearance: Option<f64>,
    #[serde(default)]
    peck_clearance: Option<f64>,
    #[serde(default)]
    through: Option<bool>,

    #[serde(default)]
    order: Option<i32>,
    #[serde(default)]
    role: Option<String>,
    /// When this operation runs, said outright: lower phases run first, and an
    /// operation with no phase keeps the one its role implies (facing 0,
    /// inside features 1, the profile that frees the part 2).
    ///
    /// This exists because the only way to run the outside profile early used
    /// to be to call it an `"inside_feature"` — a lie about what the cut *is*,
    /// told to move *when* it happens. Give a phase instead and the role stays
    /// true. Ties are broken by the order the operations were given in.
    ///
    /// A job that uses phases is ordered by them alone, so the "profile last"
    /// rule no longer decides anything it does not say: that is the point, and
    /// it is stated in the response's notes.
    #[serde(default)]
    phase: Option<i32>,
}

// A misspelt `options` key must not be a silent way to drop verification.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct JobRequest {
    #[serde(default)]
    name: Option<String>,
    stock: StockReq,
    #[serde(default)]
    machine: MachineReq,
    tools: Vec<ToolReq>,
    operations: Vec<OperationReq>,
    #[serde(default)]
    options: OptionsReq,
    /// Per-check severity override, e.g. `{"material_left": "warning"}`. The
    /// oracle's own severities are the default.
    #[serde(default)]
    verify_policy: std::collections::BTreeMap<String, String>,
}

// ---------------------------------------------------------------------------
// Response pieces
// ---------------------------------------------------------------------------

/// The preview polyline the Swift `CNCProgram` already decodes.
#[derive(Debug, Clone, Serialize)]
struct Move {
    to: [f64; 3],
    rapid: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    feed: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
struct RangeOut {
    /// `preamble`, `tool_start`, `tool_change`, `operation`, `postamble`.
    block: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    op_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool: Option<u32>,
    /// First index into `moves`.
    start: usize,
    /// One past the last index into `moves`. The ranges partition `moves`.
    end: usize,
    /// Acceleration-aware duration of this block, seconds.
    seconds: f64,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Run a job request.
pub fn run(input: &str) -> Result<Value, String> {
    let req: JobRequest = serde_json::from_str(input).map_err(|e| {
        format!("the job request could not be read: {e}. Expected an object with stock, tools and operations.")
    })?;
    build(req)
}

fn build(req: JobRequest) -> Result<Value, String> {
    let mut notes: Vec<Note> = Vec::new();
    let stock = req.stock.build()?;
    let park_z = match req.options.park_z {
        Some(z) => positive("options.park_z", z)?,
        None => 5.0,
    };
    let safe_z = match req.options.safe_z {
        Some(z) => positive("options.safe_z", z)?,
        None => park_z,
    };
    if req.tools.is_empty() {
        return Err("the job has no tools: every operation names a tool number, so the library cannot be empty.".into());
    }
    if req.operations.is_empty() {
        return Err("the job has no operations.".into());
    }

    let mut library = ToolLibrary::new();
    for t in &req.tools {
        let entry = t.build()?;
        if library.get_by_number(entry.number).is_some() {
            return Err(format!(
                "tool T{} is in the library twice: a tool number has to name one tool.",
                entry.number
            ));
        }
        library.add(entry);
    }

    // Job-level defaults; every operation overrides them with its own.
    let base = CamSettings {
        safe_z,
        retract_z: park_z,
        ..CamSettings::default()
    };
    let mut job = Job::new(
        req.name.clone().unwrap_or_else(|| "vcad job".to_string()),
        library.clone(),
        base,
    )
    .with_park_z(park_z)
    .with_spin_up(match req.options.spin_up_seconds {
        Some(s) => non_negative("options.spin_up_seconds", s)?,
        None => 3.0,
    })
    .with_wcs(parse_wcs(req.options.wcs.as_deref())?)
    .with_end(parse_end(req.options.end.as_deref())?)
    .with_strategy(match &req.options.tool_change {
        Some(ToolChangeReq::M6) => ToolChangeStrategy::M6,
        Some(ToolChangeReq::ManualPauseReprobe { probe_macro }) => {
            ToolChangeStrategy::ManualPauseReprobe {
                probe_macro: probe_macro.clone(),
            }
        }
        // No changer is the safe assumption: a machine that has one is told
        // so, a machine that has not must stop rather than crash a collet
        // into the work.
        None => ToolChangeStrategy::ManualPauseReprobe { probe_macro: None },
    });

    // Where the part sits on the stock. The job placement moves the part the
    // oracle checks against too; an operation that overrides it moves alone,
    // which is worth saying out loud because the oracle will then read it as
    // cutting somewhere the part is not.
    let job_placement = match &req.options.placement {
        Some(p) => p.build("options.placement")?,
        None => Placement::identity(),
    };
    if !job_placement.is_identity() {
        notes.push(Note::info(format!(
            "this job is placed {}: every operation and the part it is verified against were moved together.",
            job_placement.describe()
        )));
    }

    // Built operations, kept beside the job so the per-op reports can be
    // regenerated without guessing which settings the job used. The request
    // kept beside each one is the *placed* request, so everything downstream —
    // reports, the tab audit, the derived part — reads the geometry that is
    // really cut rather than the geometry that was drawn.
    let mut built: Vec<(CamSettings, OperationReq, ToolEntry)> = Vec::new();
    for (i, op) in req.operations.iter().enumerate() {
        let entry = library.get_by_number(op.tool).ok_or_else(|| {
            format!(
                "operation {} asks for T{}, which is not in the tool library.",
                op.name.clone().unwrap_or_else(|| format!("#{i}")),
                op.tool
            )
        })?;
        let name = op_name(i, op);
        let placement = match &op.placement {
            Some(p) => {
                let own = p.build(&format!("operation \"{name}\".placement"))?;
                if own != job_placement {
                    notes.push(Note::caution(format!(
                        "{name} carries its own placement ({}): its geometry moved, the part the job is verified against did not. Anything it cuts away from the part reads as a gouge.",
                        own.describe()
                    )));
                }
                own
            }
            None => job_placement,
        };
        let op = place(op, &placement, &name)?;
        let (job_op, op_settings) = build_op(i, &op, entry, &stock, safe_z, park_z, &mut notes)?;
        built.push((op_settings, op, entry.clone()));
        job.push(job_op);
    }

    // The run order, when the caller stated one outright.
    apply_phases(&mut job, &built, &mut notes)?;

    // Tool geometry, reported whatever happens. `Job::assemble` refuses on an
    // error-level finding, so the findings are collected before it runs — a
    // refusal with no list of what was wrong is the thing wave 1 set out to
    // stop.
    let tool_findings = job
        .checks()
        .map_err(|e| format!("the job could not be checked: {e}"))?;
    let mut tool_checks = Vec::new();
    let mut tool_errors: Vec<String> = Vec::new();
    for (index, findings) in &tool_findings {
        for c in findings {
            let severity = match c.severity {
                vcad_kernel_cam::CheckSeverity::Error => "error",
                vcad_kernel_cam::CheckSeverity::Warning => "warning",
            };
            if severity == "error" {
                tool_errors.push(c.message.clone());
            }
            tool_checks.push(json!({
                "op": job.ops[*index].name,
                "op_index": index,
                "tool": job.ops[*index].tool_number,
                "severity": severity,
                "kind": c.kind,
                "message": c.message,
            }));
        }
    }
    if !tool_errors.is_empty() {
        // Fail closed, but hand back everything that was learned: the app has
        // to be able to say which tool is wrong for which cut.
        return Ok(json!({
            "name": job.name,
            "blocked": true,
            "error": format!("the job cannot run as specified: {}", tool_errors.join("; ")),
            "tool_checks": tool_checks,
            "policy": { "verified": false, "blocked_by": ["tool_geometry"], "warnings": [] },
            "notes": notes,
        }));
    }

    let program = job
        .assemble()
        .map_err(|e| format!("the job could not be assembled: {e}"))?;

    // Arc fitting runs inside each block, never across one: a run of moves
    // that spans two operations is not one arc, and the block ranges have to
    // keep pointing at the same work afterwards.
    let (program, arc_fit) = match &req.options.arc_fit {
        Some(a) => {
            let tolerance = positive("options.arc_fit.tolerance", a.tolerance)?;
            let (p, report) = fit_program(&program, tolerance);
            (p, Some(report))
        }
        None => (program, None),
    };

    let post: Box<dyn PostProcessor> = match req
        .options
        .post
        .as_deref()
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        None | Some("grbl") => Box::new(GrblPost::default()),
        Some("linuxcnc") => Box::new(LinuxCncPost::default()),
        Some(other) => {
            return Err(format!(
                "options.post is \"{other}\": this build posts \"grbl\" or \"linuxcnc\"."
            ))
        }
    };
    // Posting can refuse: a changer strategy against a control with no `M6`
    // is a file that stops itself part-way through the job.
    let gcode = program
        .to_gcode(post.as_ref())
        .map_err(|e| format!("the job could not be posted: {e}"))?;

    // ---- preview polyline and block ranges ------------------------------
    let limits = req.machine.limits()?;
    let (moves, ranges) = flatten(&program, &limits);

    let duration = json!({
        "naive_s": program.toolpath.estimated_time(),
        "accel_aware_s": program.toolpath.estimated_time_with(&limits),
    });

    // ---- per-operation reports ------------------------------------------
    let mut contour_reports = Vec::new();
    let mut fit_reports = Vec::new();
    for (index, (op_settings, op, entry)) in built.iter().enumerate() {
        let kind = normalise_kind(&op.kind);
        if kind != "contour_outside" && kind != "contour_inside" {
            continue;
        }
        let CamOperation::Contour2D(c2d) = &job.ops[index].operation else {
            continue;
        };
        if let Ok((_, report)) = c2d.generate_reported(&entry.tool, op_settings) {
            contour_reports.push(json!({
                "op": job.ops[index].name,
                "op_index": index,
                "report": report,
            }));
        }
        let side = if c2d.inside {
            ContourSide::Inside
        } else {
            ContourSide::Outside
        };
        let points = loop_to_points(&c2d.contour);
        if let Ok(report) =
            fit_contour(&points, entry.tool.diameter(), side, &FitOptions::default())
        {
            if !report.fits {
                notes.push(Note::warning(format!(
                    "{}: a Ø{:.3} cutter does not fit this contour — {} unreachable corner(s), {:.4} mm² in all, worst stand-off {:.4} mm.",
                    job.ops[index].name,
                    entry.tool.diameter(),
                    report.unreachable.count,
                    report.unreachable.total_area,
                    report.unreachable.max_standoff
                )));
            }
            fit_reports.push(json!({
                "op": job.ops[index].name,
                "op_index": index,
                "side": if c2d.inside { "inside" } else { "outside" },
                "report": report,
            }));
        }
    }

    // ---- verification ----------------------------------------------------
    let mut response = json!({
        "name": job.name,
        "moves": moves,
        "op_ranges": ranges,
        "duration": duration,
        "tool_checks": tool_checks,
        "tool_sequence": program.tool_sequence(),
        "fit": fit_reports,
        "report": contour_reports,
        "stock": { "thickness": stock.thickness, "spoilboard": stock.spoilboard_thickness(),
                   "underside_z": stock.underside_z() },
    });
    if let Some(report) = arc_fit {
        response["arc_fit"] = report;
    }

    // Material a pocket keeps. The oracle cannot see it (see `island_audit`),
    // so this runs whatever `options.verify` says: cutting away a lump the
    // request asked to keep is a wrong job, not an opinion about one.
    let (island_reports, island_gouges) = island_audit(&built, &job, &moves, &ranges);
    if !island_reports.is_empty() {
        response["island_clearance"] = json!(island_reports);
    }
    if !island_gouges.is_empty() {
        notes.push(Note::danger(format!(
            "this job cuts into material it was told to keep: {}. No G-code is returned.",
            island_gouges.join("; ")
        )));
        response["blocked"] = json!(true);
        response["error"] = json!(format!(
            "this job cuts into material it was told to keep: {}.",
            island_gouges.join("; ")
        ));
        response["policy"] =
            json!({ "verified": false, "blocked_by": ["island_gouge"], "warnings": [] });
        response["notes"] = json!(notes);
        drop_machine_coordinates(&mut response);
        return Ok(response);
    }

    if !req.options.verify {
        notes.push(Note::warning(
            "verification is off: this G-code has not been replayed against the part it is meant to make. Nothing here says it will not cut into it.".to_string(),
        ));
        response["blocked"] = json!(false);
        response["policy"] = json!({ "verified": false, "blocked_by": [], "warnings": [] });
        response["gcode"] = json!(gcode);
        response["notes"] = json!(notes);
        return Ok(response);
    }

    let part = match &req.options.part {
        // A stated part is stated in the part's own frame, so it is placed
        // exactly as the operations were. A derived one is read back off
        // operations that have already moved, so it is not placed twice.
        Some(p) => p.placed(&job_placement)?,
        None => derive_part(&job, &library, &mut notes)?,
    };
    let allowance = BottomAllowance(
        req.operations
            .iter()
            .filter_map(|o| o.bottom_allowance)
            .fold(0.0f64, |a, b| if b.abs() > a.abs() { b } else { a }),
    );
    let (travel, work_offset) = req.machine.travel_limits()?;
    // A tab is one tab: an operation that asks for three declares three, so
    // the audit compares three against three rather than one against three.
    let declared_tabs: Vec<DeclaredTab> = built
        .iter()
        .enumerate()
        .map(|(i, (_, o, _))| {
            let count = tab_positions(o, &format!("operation \"{}\"", op_name(i, o)))?.len();
            Ok((0..count)
                .map(|_| DeclaredTab {
                    width: o.tab_width.unwrap_or(0.0),
                    height: o.tab_height.unwrap_or(0.0),
                })
                .collect::<Vec<_>>())
        })
        .collect::<Result<Vec<_>, String>>()?
        .concat();
    let spec_for = |tool: ToolReach| {
        let mut spec = stock.job_spec(part.clone(), tool, allowance);
        spec.travel = travel;
        spec.work_offset = work_offset;
        spec.declared_tabs = declared_tabs.clone();
        spec
    };

    let opts = VerifyOptions::default();
    let mut tools: Vec<u32> = program.tool_sequence();
    tools.sort_unstable();
    tools.dedup();

    // The artifact that runs is the G-code, so that is what is replayed. A
    // program carrying a machine-specific probe macro is text the oracle
    // cannot read; the toolpath it came from still can be, and saying which
    // was used is better than silently checking neither.
    let replay = |path: &Toolpath,
                  text: Option<&str>,
                  spec: &_|
     -> Result<(JobVerification, &'static str), String> {
        if let Some(text) = text {
            match vcad_kernel_cam::verify_gcode(text, spec, &opts) {
                Ok(v) => return Ok((v, "gcode")),
                Err(gcode_err) => {
                    let v = vcad_kernel_cam::verify_toolpath(path, spec, &opts).map_err(|e| {
                        format!("the job could not be verified: {e} (and the posted G-code did not replay either: {gcode_err})")
                    })?;
                    return Ok((v, "toolpath"));
                }
            }
        }
        let v = vcad_kernel_cam::verify_toolpath(path, spec, &opts)
            .map_err(|e| format!("the job could not be verified: {e}"))?;
        Ok((v, "toolpath"))
    };

    // Diameter and centre-cutting travel together, from the library entry, so
    // neither can be read off one tool while the other is assumed.
    let reach_of = |number: u32| {
        library
            .get_by_number(number)
            .map(|t| ToolReach::of(&t.tool, &t.geometry))
            .unwrap_or(ToolReach::declared(1.0, true))
    };
    let diameter_of = |number: u32| reach_of(number).diameter;

    // The spec comes back out alongside the verification: it is what the
    // claims are stated against, and what a later re-state has to hash.
    let (verification, replayed, whole_spec) = if tools.len() <= 1 {
        let number = tools.first().copied().unwrap_or(0);
        let spec = spec_for(reach_of(number));
        let (v, how) = replay(&program.toolpath, Some(&gcode), &spec)?;
        if how == "toolpath" {
            notes.push(Note::caution(
                "the posted G-code could not be replayed, so the toolpath it came from was verified instead. Anything the post adds — a probe macro, a machine-specific word — is unchecked.".to_string(),
            ));
        }
        (v, how, spec)
    } else {
        // One `JobSpec` carries one tool diameter, so a job with several tools
        // is replayed once per tool over that tool's own blocks — a Ø2 bore
        // swept as if it were the Ø3.175 profile cutter gouges everything it
        // touches, and a Ø3.175 profile swept as a Ø2 reads as leaving metal
        // it never could have reached.
        let mut per_tool = Vec::new();
        for &number in &tools {
            let spec = spec_for(reach_of(number));
            let path = toolpath_of(&program, number);
            let (v, _) = replay(&path, None, &spec)?;
            per_tool.push((number, v));
        }
        // The whole-part checks still need every move at once, and take the
        // narrowest tool: a narrow sweep is the cautious answer for
        // `loose_pieces`, which asks how much the job frees.
        let narrowest = tools
            .iter()
            .copied()
            .fold(f64::MAX, |a, t| a.min(diameter_of(t)));
        // The whole-part checks take the narrowest tool; a sweep that
        // narrow cannot say whose centre it is, so the plunge rule is left
        // permissive here and comes from each tool's own pass above.
        let spec = spec_for(ToolReach::declared(narrowest, true));
        let (whole, how) = replay(&program.toolpath, Some(&gcode), &spec)?;
        notes.push(Note::info(format!(
            "this job uses {} tools, so it was replayed once per tool: the cut checks come from each tool's own passes at its own diameter, and the whole-part checks from every move at Ø{narrowest:.3}.",
            tools.len()
        )));
        response["verification_by_tool"] = json!(per_tool
            .iter()
            .map(|(t, v)| json!({ "tool": t, "diameter": diameter_of(*t), "verification": v }))
            .collect::<Vec<_>>());
        let mut combined = combine(whole, per_tool);
        // …and `material_left` cannot be answered at all with one radius: a
        // wall the Ø3.175 profile cutter finished reads as unreached when it
        // is measured against what a Ø2 could have got into. It is reported,
        // and it warns, but on a multi-tool job it does not block on its own.
        // `verify_policy: {"material_left": "error"}` puts it back.
        combined.material_left.check.severity = Severity::Warning;
        combined.material_left.check.note.push_str(
            "; on a multi-tool job this is a warning, not a blocker: the oracle measures reach with one cutter radius and this job has several",
        );
        combined.pass = self::checks(&combined)
            .iter()
            .all(|c| c.pass || c.severity == Severity::Warning);
        (combined, how, spec)
    };

    // Where the tabs actually ended up, against where they were asked for.
    // The kernel settles every tab onto the nearest straight stretch, so
    // "three tabs at 0.1, 0.4, 0.7" is a request, not a result — and a tab
    // that moved 8 mm to find a straight run is something the operator has to
    // be able to see before the part comes loose somewhere unexpected.
    let tab_audit = tab_audit(&job, &built, &verification);
    if !tab_audit.is_empty() {
        response["tab_placement"] = json!(tab_audit);
    }

    let (blocked_by, warnings) = policy(&verification, &req.verify_policy)?;
    let blocked = !blocked_by.is_empty();
    response["blocked"] = json!(blocked);
    response["verification"] = serde_json::to_value(&verification)
        .map_err(|e| format!("the verification report could not be serialised: {e}"))?;
    response["policy"] = json!({
        "verified": true,
        "replayed": replayed,
        "blocked_by": blocked_by,
        "warnings": warnings,
    });
    if blocked {
        notes.push(Note::danger(format!(
            "this job is refused: {}. No G-code is returned, so it cannot be exported or sent by accident.",
            blocked_by.join(", ")
        )));
    } else {
        // The key is only ever written on the pass.
        response["gcode"] = json!(gcode);
    }

    // What this job claims about the part it makes, in the shape a document
    // stores and `build_receipt` reads. Deposited on a blocked job too: the
    // claims are *why* it was refused, and a receipt that only ever saw
    // passing jobs would be a record of nothing.
    //
    // `program` is the G-code even when the toolpath was what got replayed —
    // the program is the artifact that runs, so it is the thing whose change
    // has to invalidate the claims.
    response["claims"] = crate::claims::deposit(
        vcad_kernel_cam::receipt::job_claims(&verification, &whole_spec, &opts),
        crate::claims::Inputs::new()
            .with(vcad_kernel_cam::receipt::BASIS_PROGRAM, &gcode)
            .with(vcad_kernel_cam::receipt::BASIS_OUTLINE, &whole_spec.part)
            .with(
                vcad_kernel_cam::receipt::BASIS_TOOL,
                &whole_spec.tool_diameter,
            )
            .with(vcad_kernel_cam::receipt::BASIS_STOCK, &whole_spec),
        &["verify2d"],
        req.name.clone(),
    );

    response["notes"] = json!(notes);
    if blocked {
        drop_machine_coordinates(&mut response);
    }
    Ok(response)
}

/// Take the toolpath out of a refused answer.
///
/// Withholding `gcode` was only half the gate: `moves` is the same program in
/// machine coordinates, and a client that can read it can re-post it. The
/// answer keeps everything that says *why* the job was refused —
/// `verification`, `policy`, `notes`, `report`, `fit` — and loses the two keys
/// a machine could be driven from. `op_ranges` indexes `moves`, so it goes
/// with it.
fn drop_machine_coordinates(response: &mut Value) {
    if let Some(object) = response.as_object_mut() {
        object.remove("moves");
        object.remove("op_ranges");
        object.remove("gcode");
    }
}

// ---------------------------------------------------------------------------
// Operation construction
// ---------------------------------------------------------------------------

fn normalise_kind(kind: &str) -> String {
    kind.to_ascii_lowercase().replace([' ', '-'], "_")
}

/// What an operation is called in every message about it.
fn op_name(index: usize, op: &OperationReq) -> String {
    op.name
        .clone()
        .unwrap_or_else(|| format!("{} {}", op.kind, index + 1))
}

/// The tabs an operation asks for, as fractions of the way round the loop.
///
/// `tab_positions` is authoritative when it is given: the count is then a
/// cross-check rather than a spacing rule, so a request that says both and
/// means two different things is refused rather than silently believing one.
fn tab_positions(op: &OperationReq, what: &str) -> Result<Vec<f64>, String> {
    let count = op.tabs.unwrap_or(0);
    let Some(explicit) = &op.tab_positions else {
        // Half a pitch in, so no tab sits on the seam where each pass plunges
        // — the same rule the kernel's own even spacing follows.
        return Ok((0..count)
            .map(|i| (i as f64 + 0.5) / count as f64)
            .collect());
    };
    if explicit.is_empty() {
        return Err(format!(
            "{what}.tab_positions is empty: leave it out to space {count} tab(s) evenly, or say where they go."
        ));
    }
    if op.tabs.is_some() && count != explicit.len() {
        return Err(format!(
            "{what} asks for {count} tabs but lists {} position(s). Give one or the other, or make them agree.",
            explicit.len()
        ));
    }
    let mut out = Vec::with_capacity(explicit.len());
    for (i, position) in explicit.iter().enumerate() {
        let position = finite(&format!("{what}.tab_positions[{i}]"), *position)?;
        if !(0.0..=1.0).contains(&position) {
            return Err(format!(
                "{what}.tab_positions[{i}] is {position}: a position is a fraction of the way round the loop, from 0 to 1."
            ));
        }
        out.push(position);
    }
    Ok(out)
}

/// The same operation with its geometry moved onto the stock.
///
/// Only geometry moves. Depths, feeds and tab fractions are frames-independent
/// and stay exactly as they were.
fn place(op: &OperationReq, placement: &Placement, name: &str) -> Result<OperationReq, String> {
    if placement.is_identity() {
        return Ok(op.clone());
    }
    let mut out = op.clone();
    if let Some(points) = &op.contour {
        out.contour = Some(placement.apply_loop(points));
    }
    if let Some(islands) = &op.islands {
        // An island travels with the pocket it sits in, or the cutter would
        // clear around where it used to be.
        out.islands = Some(islands.iter().map(|i| placement.apply_loop(i)).collect());
    }
    if let Some(holes) = &op.holes {
        out.holes = Some(
            holes
                .iter()
                .map(|h| h.placed(placement))
                .collect::<Vec<_>>(),
        );
    }
    if let (Some(x), Some(y)) = (op.x, op.y) {
        let moved = placement.apply([finite("x", x)?, finite("y", y)?]);
        out.x = Some(moved[0]);
        out.y = Some(moved[1]);
    }
    if let Some(r) = op.rectangle {
        // A rectangle is axis-aligned by construction, so a turned one is no
        // longer a rectangle. Refusing is the only honest answer: silently
        // taking the bounding box of the turned rectangle would machine a
        // larger area than was asked for.
        if placement.rotates() {
            return Err(format!(
                "operation \"{name}\" is placed at {:.3}° but its geometry is a \"rectangle\", which cannot be turned and stay a rectangle. Give it as a \"contour\" of four points instead.",
                placement.rotation_deg
            ));
        }
        out.rectangle = Some([
            r[0] + placement.dx,
            r[1] + placement.dy,
            r[2] + placement.dx,
            r[3] + placement.dy,
        ]);
    }
    Ok(out)
}

/// Put the job in the order its phases ask for, when any were given.
///
/// The kernel orders a job by the *role* each operation plays — facing, then
/// inside features, then the profile that frees the part — and that rule is
/// right nearly always, which is why it stays the default. What it could not
/// express was "run this one earlier anyway", and the only way to say it was to
/// call the outside profile an `"inside_feature"`: a lie about what the cut is,
/// told to change when it happens.
///
/// A phase says it directly. Every operation gets one — its own, or the one its
/// role implies — and the whole job is ordered by phase, then by the order the
/// operations arrived in. The roles are then all one bucket on purpose: a role
/// decides nothing but the order, and the order has just been stated.
fn apply_phases(
    job: &mut Job,
    built: &[(CamSettings, OperationReq, ToolEntry)],
    notes: &mut Vec<Note>,
) -> Result<(), String> {
    if built.iter().all(|(_, o, _)| o.phase.is_none()) {
        return Ok(());
    }
    let phases: Vec<i32> = built
        .iter()
        .enumerate()
        .map(|(i, (_, o, _))| {
            o.phase.unwrap_or(match job.ops[i].role {
                OpRole::Facing => 0,
                OpRole::InsideFeature => 1,
                OpRole::OutsideProfile => 2,
            })
        })
        .collect();
    let mut wanted: Vec<usize> = (0..built.len()).collect();
    wanted.sort_by_key(|&i| (phases[i], i));
    for (rank, &i) in wanted.iter().enumerate() {
        job.ops[i].role = OpRole::InsideFeature;
        job.ops[i].order = rank as i32;
    }
    // Tools are still kept together, which on a multi-tool job can pull an
    // operation out of the order that was asked for. A job that runs in an
    // order it was not told to is exactly what this field exists to stop, so
    // it is refused rather than quietly re-sorted.
    let actual = job.order();
    if actual != wanted {
        let named = |list: &[usize]| {
            list.iter()
                .map(|&i| job.ops[i].name.clone())
                .collect::<Vec<_>>()
                .join(" → ")
        };
        return Err(format!(
            "the phases ask for {}, but this job would run {}: operations are grouped by tool, so a phase order that interleaves two tools cannot be honoured. Give each tool's operations consecutive phases, or drop the phase override.",
            named(&wanted),
            named(&actual)
        ));
    }
    notes.push(Note::info(format!(
        "this job runs in the phase order it was given ({}). The usual rule — facing, then inside features, then the profile that frees the part — did not decide it.",
        wanted
            .iter()
            .map(|&i| format!("{} (phase {})", job.ops[i].name, phases[i]))
            .collect::<Vec<_>>()
            .join(" → ")
    )));
    Ok(())
}

// ---------------------------------------------------------------------------
// Islands
// ---------------------------------------------------------------------------

/// How close the cutter came to the material each pocket was told to keep.
///
/// The oracle's part is an outer boundary and openings, and material *inside*
/// an opening cannot be written down that way — so a cut straight across an
/// island reads as perfectly clean to every check in `verify2d`. This is the
/// check that is missing, measured off the same moves the preview draws: for
/// every island, the closest the cutter's *centre* came, less its radius.
///
/// The preview samples arcs to [`PREVIEW_TOLERANCE`], and arc fitting is
/// allowed its own tolerance, so a clearance is only believed to about
/// `tolerance_mm`. Anything inside that is reported and not refused; anything
/// beyond it is a cut into a kept lump and blocks the job.
fn island_audit(
    built: &[(CamSettings, OperationReq, ToolEntry)],
    job: &Job,
    moves: &[Move],
    ranges: &[RangeOut],
) -> (Vec<Value>, Vec<String>) {
    let tolerance = PREVIEW_TOLERANCE + 0.01;
    let mut out = Vec::new();
    let mut gouges = Vec::new();
    for (index, (_, op, entry)) in built.iter().enumerate() {
        let islands: Vec<Vec<[f64; 2]>> = op
            .islands
            .iter()
            .flatten()
            .filter(|l| l.len() >= 3)
            .cloned()
            .collect();
        if islands.is_empty() {
            continue;
        }
        let radius = entry.tool.diameter() / 2.0;
        let cuts = cut_segments(moves, ranges, index);
        let mut entries = Vec::with_capacity(islands.len());
        for (i, island) in islands.iter().enumerate() {
            let clearance = cuts
                .iter()
                .map(|(a, b)| signed_clearance(*a, *b, island))
                .fold(f64::MAX, f64::min);
            let into = if clearance == f64::MAX {
                0.0
            } else {
                radius - clearance
            };
            if into > tolerance {
                gouges.push(format!(
                    "{} cuts {into:.3} mm into island {i}",
                    job.ops[index].name
                ));
            }
            entries.push(json!({
                "island": i,
                // Centre-line clearance: how far the tool centre stayed from
                // the island's wall. A path that entered the island reads
                // negative.
                "centre_clearance_mm": if clearance == f64::MAX { Value::Null } else { json!(clearance) },
                "cut_into_mm": into.max(0.0),
                "kept": into <= tolerance,
            }));
        }
        out.push(json!({
            "op": job.ops[index].name,
            "op_index": index,
            "tool_diameter": entry.tool.diameter(),
            "tolerance_mm": tolerance,
            "islands": entries,
            "note": "the part region the oracle checks against cannot hold material inside an opening, so islands are checked here instead: the closest the cutter centre came to each island wall, less its radius.",
        }));
    }
    (out, gouges)
}

/// Every cutting segment of one operation, in XY, at or below the stock top.
/// A move above Z0 cuts nothing, so it cannot gouge anything either.
fn cut_segments(moves: &[Move], ranges: &[RangeOut], op_index: usize) -> Vec<([f64; 2], [f64; 2])> {
    let mut out = Vec::new();
    for range in ranges.iter().filter(|r| r.op_index == Some(op_index)) {
        if range.start >= range.end || range.end > moves.len() {
            continue;
        }
        for pair in moves[range.start..range.end].windows(2) {
            let (a, b) = (&pair[0], &pair[1]);
            if b.rapid || a.to[2].min(b.to[2]) > 1e-9 {
                continue;
            }
            out.push(([a.to[0], a.to[1]], [b.to[0], b.to[1]]));
        }
    }
    out
}

/// Distance from a segment to a closed loop, negative when the segment is
/// inside it.
fn signed_clearance(a: [f64; 2], b: [f64; 2], loop_: &[[f64; 2]]) -> f64 {
    let mut best = f64::MAX;
    for i in 0..loop_.len() {
        let (c, d) = (loop_[i], loop_[(i + 1) % loop_.len()]);
        best = best.min(segment_distance(a, b, c, d));
    }
    if point_in_loop(a, loop_) || point_in_loop(b, loop_) {
        -best
    } else {
        best
    }
}

fn point_in_loop(p: [f64; 2], loop_: &[[f64; 2]]) -> bool {
    let mut inside = false;
    for i in 0..loop_.len() {
        let (a, b) = (loop_[i], loop_[(i + 1) % loop_.len()]);
        if (a[1] > p[1]) != (b[1] > p[1]) {
            let t = (p[1] - a[1]) / (b[1] - a[1]);
            if p[0] < a[0] + t * (b[0] - a[0]) {
                inside = !inside;
            }
        }
    }
    inside
}

fn segment_distance(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> f64 {
    if segments_cross(a, b, c, d) {
        return 0.0;
    }
    point_segment_distance(a, c, d)
        .min(point_segment_distance(b, c, d))
        .min(point_segment_distance(c, a, b))
        .min(point_segment_distance(d, a, b))
}

fn point_segment_distance(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let length2 = dx * dx + dy * dy;
    if length2 <= f64::EPSILON {
        return (p[0] - a[0]).hypot(p[1] - a[1]);
    }
    let t = (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / length2).clamp(0.0, 1.0);
    (a[0] + t * dx - p[0]).hypot(a[1] + t * dy - p[1])
}

fn segments_cross(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> bool {
    let side = |p: [f64; 2], q: [f64; 2], r: [f64; 2]| {
        (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0])
    };
    let (d1, d2) = (side(a, b, c), side(a, b, d));
    let (d3, d4) = (side(c, d, a), side(c, d, b));
    (d1 * d2 < 0.0) && (d3 * d4 < 0.0)
}

#[allow(clippy::too_many_arguments)]
fn build_op(
    index: usize,
    op: &OperationReq,
    entry: &ToolEntry,
    stock: &Stock,
    safe_z: f64,
    park_z: f64,
    notes: &mut Vec<Note>,
) -> Result<(JobOp, CamSettings), String> {
    let name = op_name(index, op);
    let what = format!("operation \"{name}\"");
    let diameter = entry.tool.diameter();
    let settings = build_settings(
        &what,
        op.feed,
        op.plunge,
        op.rpm,
        op.stepdown,
        op.stepover,
        diameter,
        safe_z,
        park_z,
    )?;
    let depth = positive(&format!("{what}.depth"), op.depth)?;
    let allowance = BottomAllowance(match op.bottom_allowance {
        Some(a) => finite(&format!("{what}.bottom_allowance"), a)?,
        None => 0.0,
    });
    let spoilboard = stock.spoilboard;
    // Every operation with a floor obeys the same rule, and it is checked
    // here so the refusal names the operation rather than arriving from
    // three different depths in the kernel.
    let final_depth = allowance.check(depth, spoilboard).map_err(|refusal| {
        use vcad_kernel_cam::AllowanceRefusal::*;
        match refusal {
            BreakThroughWithoutSpoilboard { overcut, spoilboard } =>
                format!("{what} cuts {overcut:.3} mm past the underside of the stock, and the declared spoilboard is {spoilboard:.3} mm thick. Declare a sacrificial board at least that thick, or reduce the break-through."),
            SpoilboardNotAThickness { declared } =>
                format!("{what} cuts past the underside of the stock into a spoilboard declared as {declared}, which is not a thickness. Say how much sacrificial material is under the stock, in mm."),
            ExceedsDepth { allowance, depth } =>
                format!("{what} asks for a {allowance:.3} mm bottom allowance on a {depth:.3} mm cut, which leaves nothing to machine."),
        }
    })?;

    let kind = normalise_kind(&op.kind);
    if op.islands.is_some() && kind != "pocket" {
        return Err(format!(
            "{what} is a \"{kind}\" and carries \"islands\": only a pocket keeps material inside itself. An island on a contour would be a second wall, not a kept lump."
        ));
    }
    let operation: CamOperation = match kind.as_str() {
        "face" => {
            let r = rectangle(op, stock, &what)?;
            CamOperation::Face(Face::new(r[0], r[1], r[2], r[3], final_depth))
        }
        "pocket" => {
            let contour = match &op.contour {
                Some(points) => contour_from(&format!("{what}.contour"), points)?,
                None => {
                    let r = rectangle(op, stock, &what)?;
                    Contour::rectangle(r[0], r[1], r[2] - r[0], r[3] - r[1])
                }
            };
            let mut pocket = Pocket2D::new(contour, final_depth);
            for (i, island) in op.islands.iter().flatten().enumerate() {
                pocket =
                    pocket.with_island(contour_from(&format!("{what}.islands[{i}]"), island)?);
            }
            if let Some(s) = op.stock_to_leave {
                pocket = pocket.with_stock_to_leave(non_negative(
                    &format!("{what}.stock_to_leave"),
                    s,
                )?);
            }
            CamOperation::Pocket2D(pocket)
        }
        "contour_outside" | "contour_inside" => {
            let points = op.contour.as_ref().ok_or_else(|| {
                format!("{what} is a contour but carries no \"contour\": a closed polyline of [x, y] points in the stock frame.")
            })?;
            let contour = contour_from(&format!("{what}.contour"), points)?;
            let inside = kind == "contour_inside";
            // The depth handed to the contour is the depth *asked for*: the
            // operation applies the allowance itself, in the kernel's own
            // words, so the onion skin and the break-through are one rule.
            let mut c = if inside {
                Contour2D::inside(contour, depth)
            } else {
                Contour2D::outside(contour, depth)
            }
            .with_bottom_allowance(allowance.0);
            if let Some(s) = spoilboard {
                c = c.with_spoilboard(s.thickness);
            }
            if let Some(o) = op.offset {
                c = c.with_offset(finite(&format!("{what}.offset"), o)?);
            }
            if let Some(s) = op.stock_to_leave {
                c = c.with_stock_to_leave(non_negative(&format!("{what}.stock_to_leave"), s)?);
            }
            if let Some(n) = op.finish_stepdowns {
                if n == 0 {
                    return Err(format!("{what}.finish_stepdowns is 0: use 1 for a single full-depth finish pass."));
                }
                c = c.with_finish_stepdowns(n);
            }
            if let Some(s) = op.spring_pass {
                c = c.with_spring_pass(s);
            }
            if let Some(f) = op.finish_feed {
                c = c.with_finish_feed(positive(&format!("{what}.finish_feed"), f)?);
            }
            c = c.with_direction(match op.direction.as_deref().map(str::to_ascii_lowercase).as_deref() {
                None | Some("climb") => CutDirection::Climb,
                Some("conventional") | Some("up") => CutDirection::Conventional,
                Some(other) => return Err(format!("{what}.direction is \"{other}\": it has to be \"climb\" or \"conventional\".")),
            });
            match op.entry.as_deref().map(str::to_ascii_lowercase).as_deref() {
                None | Some("ramp") => {}
                Some("plunge") => c = c.with_plunge_entry(),
                Some(other) => return Err(format!("{what}.entry is \"{other}\": it has to be \"ramp\" or \"plunge\".")),
            }
            if let Some(a) = op.ramp_angle {
                c = c.with_ramp_angle(positive(&format!("{what}.ramp_angle"), a)?);
            }
            if let Some(l) = op.lead_in {
                c = c.with_lead_in(l);
            }
            if let Some(r) = op.lead_radius {
                c = c.with_lead_radius(positive(&format!("{what}.lead_radius"), r)?);
            }
            if let Some(t) = &op.thin_slot {
                let strategy = normalise_kind(&t.strategy);
                c = match strategy.as_str() {
                    "refuse" => c.with_thin_slot(ThinSlotStrategy::Refuse),
                    "centre_line" | "center_line" | "centreline" | "centerline" => {
                        let tol = positive(
                            &format!("{what}.thin_slot.tolerance"),
                            t.tolerance.unwrap_or(0.05),
                        )?;
                        notes.push(Note::caution(format!(
                            "{name}: where the cutter is as wide as the opening the path follows the centre line, cutting up to {tol:.3} mm past the wall. The report says where."
                        )));
                        c.with_centre_line_fallback(tol)
                    }
                    other => return Err(format!("{what}.thin_slot.strategy is \"{other}\": it has to be \"refuse\" or \"centre_line\".")),
                };
            }
            let positions = tab_positions(op, &what)?;
            if !positions.is_empty() {
                if positions.len() > 64 {
                    return Err(format!(
                        "{what} asks for {} tabs: 64 is the most a loop can carry.",
                        positions.len()
                    ));
                }
                let width = positive(&format!("{what}.tab_width"), op.tab_width.unwrap_or(0.0))?;
                let height = positive(&format!("{what}.tab_height"), op.tab_height.unwrap_or(0.0))?;
                if height >= final_depth {
                    return Err(format!(
                        "{what} asks for {height:.3} mm tabs in a {final_depth:.3} mm cut: a tab as tall as the cut is not a tab."
                    ));
                }
                for position in positions {
                    c = c.with_tab(Tab::new(position, width, height));
                }
            }
            CamOperation::Contour2D(c)
        }
        "drill" => {
            let holes = op.holes.as_ref().ok_or_else(|| {
                format!("{what} is a drilling operation but carries no \"holes\".")
            })?;
            if holes.is_empty() {
                return Err(format!("{what} has an empty hole list."));
            }
            let centres: Result<Vec<Hole>, String> = holes
                .iter()
                .enumerate()
                .map(|(i, h)| h.build(&format!("{what}.holes[{i}]")))
                .collect();
            let mut drill = Drill::new(centres?, final_depth);
            drill.cycle = drill_cycle(op, &what)?;
            if let Some(c) = op.clearance {
                drill = drill.with_clearance(positive(&format!("{what}.clearance"), c)?);
            }
            if let Some(c) = op.peck_clearance {
                drill = drill.with_peck_clearance(positive(&format!("{what}.peck_clearance"), c)?);
            }
            if let Some(d) = op.dwell {
                drill = drill.with_dwell(non_negative(&format!("{what}.dwell"), d)?);
            }
            if op.through.unwrap_or(false) {
                drill = drill.with_break_through(break_through(stock, &allowance));
            }
            CamOperation::Drill(drill)
        }
        "helical_bore" => {
            let x = finite(&format!("{what}.x"), op.x.ok_or_else(|| format!("{what} needs an \"x\" centre."))?)?;
            let y = finite(&format!("{what}.y"), op.y.ok_or_else(|| format!("{what} needs a \"y\" centre."))?)?;
            let bore = positive(
                &format!("{what}.diameter"),
                op.diameter
                    .ok_or_else(|| format!("{what} needs a bore \"diameter\"."))?,
            )?;
            if bore <= diameter + 1e-9 {
                return Err(format!(
                    "{what} bores Ø{bore:.3} with a Ø{diameter:.3} cutter: a helical bore has to be larger than the cutter. Drill it, or use a smaller cutter."
                ));
            }
            let pitch = match op.pitch {
                Some(p) => positive(&format!("{what}.pitch"), p)?,
                None => settings.stepdown,
            };
            let mut b = HelicalBore::new(x, y, bore, final_depth, pitch);
            if let Some(c) = op.clearance {
                b = b.with_clearance(positive(&format!("{what}.clearance"), c)?);
            }
            if let Some(s) = op.stock_to_leave {
                b = b.with_stock_to_leave(non_negative(&format!("{what}.stock_to_leave"), s)?);
            }
            if op.finish_pass == Some(false) {
                b = b.without_finish_pass();
            }
            if op.through.unwrap_or(false) {
                b = b.with_break_through(break_through(stock, &allowance));
            }
            CamOperation::HelicalBore(b)
        }
        other => {
            return Err(format!(
                "{what} has kind \"{other}\": it has to be one of face, pocket, contour_outside, contour_inside, drill, helical_bore."
            ))
        }
    };

    if op.role.is_some() && op.phase.is_some() {
        return Err(format!(
            "{what} gives both a \"role\" and a \"phase\", and they are two ways of saying the same thing: a role is what the cut is, and the only thing it decides is when it runs. Give the phase alone."
        ));
    }
    let role = match op.role.as_deref().map(normalise_kind).as_deref() {
        None => operation.default_role(),
        Some("facing") => OpRole::Facing,
        Some("inside_feature") | Some("inside") => OpRole::InsideFeature,
        Some("outside_profile") | Some("outside") => OpRole::OutsideProfile,
        Some(other) => {
            return Err(format!(
                "{what}.role is \"{other}\": it has to be \"facing\", \"inside_feature\" or \"outside_profile\"."
            ))
        }
    };

    let mut job_op = JobOp::new(name, op.tool, operation)
        .with_role(role)
        .with_settings(settings.clone());
    if let Some(order) = op.order {
        job_op = job_op.with_order(order);
    }
    Ok((job_op, settings))
}

fn break_through(stock: &Stock, allowance: &BottomAllowance) -> BreakThrough {
    let mut through = BreakThrough::new(stock.thickness).with_allowance(allowance.overcut());
    if let Some(s) = stock.spoilboard {
        through = through.over(Spoilboard::new(s.thickness));
    }
    through
}

fn drill_cycle(op: &OperationReq, what: &str) -> Result<DrillCycle, String> {
    let named = op.cycle.as_deref().map(normalise_kind);
    match named.as_deref() {
        None | Some("straight") => Ok(DrillCycle::Straight),
        Some("spot") => Ok(DrillCycle::Spot),
        Some("peck") => Ok(DrillCycle::Peck {
            peck_depth: positive(
                &format!("{what}.peck_depth"),
                op.peck_depth.ok_or_else(|| {
                    format!("{what} asks for a peck cycle but gives no \"peck_depth\".")
                })?,
            )?,
        }),
        Some("chip_break") => Ok(DrillCycle::ChipBreak {
            peck_depth: positive(
                &format!("{what}.peck_depth"),
                op.peck_depth.ok_or_else(|| {
                    format!("{what} asks for a chip-break cycle but gives no \"peck_depth\".")
                })?,
            )?,
            retreat: positive(&format!("{what}.retreat"), op.retreat.unwrap_or(0.25))?,
        }),
        Some(other) => Err(format!(
            "{what}.cycle is \"{other}\": it has to be \"spot\", \"straight\", \"peck\" or \"chip_break\"."
        )),
    }
}

fn rectangle(op: &OperationReq, stock: &Stock, what: &str) -> Result<[f64; 4], String> {
    let r = match op.rectangle {
        Some(r) => r,
        None => stock.bbox.ok_or_else(|| {
            format!("{what} needs a \"rectangle\" [min_x, min_y, max_x, max_y], or a stock bbox to fall back on.")
        })?,
    };
    for (i, v) in r.iter().enumerate() {
        finite(&format!("{what}.rectangle[{i}]"), *v)?;
    }
    if r[2] <= r[0] || r[3] <= r[1] {
        return Err(format!(
            "{what}.rectangle is [{}, {}, {}, {}]: it has to run min_x, min_y, max_x, max_y with a positive size.",
            r[0], r[1], r[2], r[3]
        ));
    }
    Ok(r)
}

// ---------------------------------------------------------------------------
// Program → preview
// ---------------------------------------------------------------------------

fn parse_wcs(named: Option<&str>) -> Result<Wcs, String> {
    match named.map(str::to_ascii_uppercase).as_deref() {
        None | Some("G54") => Ok(Wcs::G54),
        Some("G55") => Ok(Wcs::G55),
        Some("G56") => Ok(Wcs::G56),
        Some("G57") => Ok(Wcs::G57),
        Some("G58") => Ok(Wcs::G58),
        Some("G59") => Ok(Wcs::G59),
        Some(other) => Err(format!(
            "options.wcs is \"{other}\": it has to be one of G54..G59."
        )),
    }
}

fn parse_end(named: Option<&str>) -> Result<ProgramEnd, String> {
    match named.map(str::to_ascii_lowercase).as_deref() {
        None | Some("m30") => Ok(ProgramEnd::M30),
        Some("m2") => Ok(ProgramEnd::M2),
        Some(other) => Err(format!(
            "options.end is \"{other}\": it has to be \"m2\" or \"m30\"."
        )),
    }
}

/// Fit arcs inside each block and rebuild the program with the ranges moved
/// to match.
fn fit_program(program: &Program, tolerance: f64) -> (Program, Value) {
    let options = ArcFitOptions::with_tolerance(tolerance);
    let mut toolpath = Toolpath::new();
    let mut blocks = Vec::with_capacity(program.blocks.len());
    let (mut segments_in, mut linear_in, mut arcs, mut linears) = (0usize, 0usize, 0usize, 0usize);
    let mut worst = 0.0f64;
    for block in &program.blocks {
        let mut slice = Toolpath::new();
        slice.extend(
            program.toolpath.segments[block.start..block.end]
                .iter()
                .cloned(),
        );
        let (fitted, report) = fit_arcs_reported(&slice, &options);
        segments_in += report.segments_in;
        linear_in += report.linear_in;
        arcs += report.arcs_emitted;
        linears += report.linears_emitted;
        worst = worst.max(report.max_deviation);
        let start = toolpath.len();
        toolpath.extend(fitted.segments);
        blocks.push(BlockRange {
            block: block.block.clone(),
            tool_number: block.tool_number,
            start,
            end: toolpath.len(),
        });
    }
    let segments_out = toolpath.len();
    let fitted = Program {
        name: program.name.clone(),
        toolpath,
        blocks,
        strategy: program.strategy.clone(),
        wcs: program.wcs,
        end: program.end,
        park_z: program.park_z,
    };
    (
        fitted,
        json!({
            "tolerance": tolerance,
            "segments_in": segments_in,
            "segments_out": segments_out,
            "linear_in": linear_in,
            "arcs_emitted": arcs,
            "linears_emitted": linears,
            "max_deviation": worst,
        }),
    )
}

/// Chord tolerance the preview polyline samples arcs at. Fine enough that a
/// Ø2.5 bore reads round on screen, coarse enough that fitting arcs is still
/// a large reduction in the number of points the app has to draw.
const PREVIEW_TOLERANCE: f64 = 0.02;

/// Walk the program once: the preview polyline, and the move range each block
/// occupies. The ranges partition `moves` — every move is in exactly one.
fn flatten(program: &Program, limits: &MachineLimits) -> (Vec<Move>, Vec<RangeOut>) {
    let mut moves: Vec<Move> = Vec::new();
    let mut ranges = Vec::with_capacity(program.blocks.len());
    let mut at = [0.0f64, 0.0, program.park_z];
    let mut started = false;
    for block in &program.blocks {
        let start = moves.len();
        let mut slice = Toolpath::new();
        slice.push(ToolpathSegment::rapid(at[0], at[1], at[2]));
        for seg in &program.toolpath.segments[block.start..block.end] {
            slice.push(seg.clone());
            match seg {
                ToolpathSegment::Rapid { to } => {
                    if started {
                        moves.push(Move {
                            to: *to,
                            rapid: true,
                            feed: None,
                        });
                    }
                    at = *to;
                    started = true;
                }
                ToolpathSegment::Linear { to, feed } => {
                    moves.push(Move {
                        to: *to,
                        rapid: false,
                        feed: Some(*feed),
                    });
                    at = *to;
                    started = true;
                }
                ToolpathSegment::Arc {
                    to,
                    center,
                    plane,
                    dir,
                    feed,
                } => {
                    if matches!(plane, ArcPlane::Xy) {
                        for p in sample_arc(at, *to, *center, matches!(dir, ArcDir::Ccw)) {
                            moves.push(Move {
                                to: p,
                                rapid: false,
                                feed: Some(*feed),
                            });
                        }
                    } else {
                        moves.push(Move {
                            to: *to,
                            rapid: false,
                            feed: Some(*feed),
                        });
                    }
                    at = *to;
                    started = true;
                }
                _ => {}
            }
        }
        let (block_name, name, op_index) = match &block.block {
            ProgramBlock::Preamble => ("preamble", None, None),
            ProgramBlock::ToolStart { tool } => {
                ("tool_start", Some(format!("start T{tool}")), None)
            }
            ProgramBlock::ToolChange { from, to } => {
                ("tool_change", Some(format!("T{from} → T{to}")), None)
            }
            ProgramBlock::Operation { name, op_index } => {
                ("operation", Some(name.clone()), Some(*op_index))
            }
            ProgramBlock::Postamble => ("postamble", None, None),
        };
        ranges.push(RangeOut {
            block: block_name,
            name,
            op_index,
            tool: block.tool_number,
            start,
            end: moves.len(),
            seconds: slice.estimated_time_with(limits),
        });
    }
    (moves, ranges)
}

/// Sample an arc into chords no further than [`PREVIEW_TOLERANCE`] from it.
/// `center` is the I/J offset from the arc's start, as the segment carries it.
fn sample_arc(from: [f64; 3], to: [f64; 3], center: [f64; 3], ccw: bool) -> Vec<[f64; 3]> {
    let cx = from[0] + center[0];
    let cy = from[1] + center[1];
    let r = (from[0] - cx).hypot(from[1] - cy);
    if !r.is_finite() || r <= 1e-9 {
        return vec![to];
    }
    let a0 = (from[1] - cy).atan2(from[0] - cx);
    let a1 = (to[1] - cy).atan2(to[0] - cx);
    let mut sweep = if ccw { a1 - a0 } else { a0 - a1 };
    while sweep <= 1e-12 {
        sweep += std::f64::consts::TAU;
    }
    let step = if PREVIEW_TOLERANCE >= r {
        std::f64::consts::FRAC_PI_2
    } else {
        2.0 * (1.0 - PREVIEW_TOLERANCE / r).clamp(-1.0, 1.0).acos()
    };
    let count = ((sweep / step.max(1e-6)).ceil() as usize).clamp(1, 4096);
    (1..=count)
        .map(|i| {
            let f = i as f64 / count as f64;
            let a = if ccw { a0 + sweep * f } else { a0 - sweep * f };
            if i == count {
                to
            } else {
                [
                    cx + r * a.cos(),
                    cy + r * a.sin(),
                    from[2] + (to[2] - from[2]) * f,
                ]
            }
        })
        .collect()
}

fn loop_to_points(contour: &Contour) -> Vec<[f64; 2]> {
    let mut pts = vec![[contour.start.x, contour.start.y]];
    for seg in &contour.segments {
        match seg {
            ContourSegment::Line { to } => pts.push([to.x, to.y]),
            ContourSegment::Arc { to, .. } => pts.push([to.x, to.y]),
        }
    }
    while pts.len() > 1 {
        let (a, b) = (pts[0], pts[pts.len() - 1]);
        if (a[0] - b[0]).hypot(a[1] - b[1]) <= 1e-9 {
            pts.pop();
        } else {
            break;
        }
    }
    pts
}

// ---------------------------------------------------------------------------
// Verification policy
// ---------------------------------------------------------------------------

/// Every segment one tool cuts, in program order, with a rapid seeded at the
/// park height so the replay knows where the tool starts.
fn toolpath_of(program: &Program, tool: u32) -> Toolpath {
    let mut path = Toolpath::new();
    path.push(ToolpathSegment::rapid(0.0, 0.0, program.park_z));
    for block in &program.blocks {
        if block.tool_number == Some(tool) {
            path.extend(
                program.toolpath.segments[block.start..block.end]
                    .iter()
                    .cloned(),
            );
        }
    }
    path
}

/// Fold the per-tool runs into the whole-program one: a cut check fails if it
/// failed for any tool, and the whole-part checks stay as the whole-program
/// run left them.
fn combine(mut whole: JobVerification, per_tool: Vec<(u32, JobVerification)>) -> JobVerification {
    let worst = |current: CheckReport, candidate: &CheckReport| {
        if candidate.pass {
            current
        } else if current.pass || candidate.worst > current.worst {
            candidate.clone()
        } else {
            current
        }
    };
    let mut gouge = per_tool[0].1.gouge.clone();
    let mut rapids = per_tool[0].1.rapids.clone();
    let mut depth = per_tool[0].1.depth.clone();
    let mut envelope = per_tool[0].1.envelope.clone();
    let mut plunges = per_tool[0].1.plunges.clone();
    // Tabs are cut by one tool; the run that saw them is the one that counts.
    let mut tabs = per_tool[0].1.tabs.clone();
    for (_, v) in per_tool.iter().skip(1) {
        gouge = worst(gouge, &v.gouge);
        rapids = worst(rapids, &v.rapids);
        plunges = worst(plunges, &v.plunges);
        if !v.depth.check.pass && depth.check.pass {
            depth = v.depth.clone();
        }
        if !v.envelope.check.pass && envelope.check.pass {
            envelope = v.envelope.clone();
        }
        if v.tabs.tab_count > tabs.tab_count {
            tabs = v.tabs.clone();
        }
    }
    whole.gouge = gouge;
    whole.rapids = rapids;
    whole.depth = depth;
    whole.envelope = envelope;
    whole.plunges = plunges;
    whole.tabs = tabs;
    whole.pass = checks(&whole)
        .iter()
        .all(|c| c.pass || c.severity == Severity::Warning);
    whole
}

fn checks(v: &JobVerification) -> Vec<&CheckReport> {
    vec![
        &v.gouge,
        &v.material_left.check,
        &v.rapids,
        &v.depth.check,
        &v.tabs.check,
        &v.envelope.check,
        &v.loose.check,
        &v.plunges,
    ]
}

/// Which failed checks block and which only warn, after the caller's
/// overrides. Returns `(blocked_by, warnings)`, both named by check.
fn policy(
    v: &JobVerification,
    overrides: &std::collections::BTreeMap<String, String>,
) -> Result<(Vec<String>, Vec<String>), String> {
    for (name, severity) in overrides {
        if !matches!(severity.to_ascii_lowercase().as_str(), "error" | "warning") {
            return Err(format!(
                "verify_policy.{name} is \"{severity}\": a severity is \"error\" or \"warning\"."
            ));
        }
        if !checks(v).iter().any(|c| c.name == *name) {
            return Err(format!(
                "verify_policy names \"{name}\", which is not a check. The checks are: {}.",
                checks(v)
                    .iter()
                    .map(|c| c.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    let mut blocked_by = Vec::new();
    let mut warnings = Vec::new();
    for c in checks(v) {
        if c.pass {
            continue;
        }
        let severity = match overrides.get(&c.name).map(|s| s.to_ascii_lowercase()) {
            Some(ref s) if s == "error" => Severity::Error,
            Some(_) => Severity::Warning,
            None => c.severity,
        };
        match severity {
            Severity::Error => blocked_by.push(c.name.clone()),
            Severity::Warning => warnings.push(c.name.clone()),
        }
    }
    Ok((blocked_by, warnings))
}

/// The part the job is meant to make, read off its own operations when the
/// caller did not state one: the outside profile is the boundary, every
/// inside feature is an opening.
fn derive_part(
    job: &Job,
    library: &ToolLibrary,
    notes: &mut Vec<Note>,
) -> Result<PartRegion, String> {
    let mut outer: Option<Vec<[f64; 2]>> = None;
    let mut holes: Vec<Vec<[f64; 2]>> = Vec::new();
    for op in &job.ops {
        match &op.operation {
            CamOperation::Contour2D(c) if !c.inside => {
                let pts = loop_to_points(&c.contour);
                if outer.as_ref().is_none_or(|o| area(&pts) > area(o)) {
                    outer = Some(pts);
                }
            }
            CamOperation::Contour2D(c) => holes.push(loop_to_points(&c.contour)),
            CamOperation::Pocket2D(p) => holes.push(loop_to_points(&p.contour)),
            CamOperation::HelicalBore(b) => holes.push(circle(b.x, b.y, b.diameter / 2.0)),
            CamOperation::Drill(d) => {
                let r = library
                    .get_by_number(op.tool_number)
                    .map(|t| t.tool.diameter() / 2.0)
                    .unwrap_or(0.5);
                for h in &d.holes {
                    holes.push(circle(h.x, h.y, r));
                }
            }
            _ => {}
        }
    }
    let outer = outer.ok_or_else(|| {
        "this job has nothing that says where the part ends, so there is nothing to verify it against. Give options.part, or add an outside contour.".to_string()
    })?;
    notes.push(Note::info(format!(
        "the part was read off the job's own operations: the outside profile as the boundary and {} inside feature(s) as openings. Pass options.part to state it instead.",
        holes.len()
    )));
    PartRegion::new(outer, holes)
        .map_err(|e| format!("the part read off the operations is unusable: {e}"))
}

// ---------------------------------------------------------------------------
// Tab audit
// ---------------------------------------------------------------------------

/// Where the tabs of every tabbed operation actually ended up.
///
/// The oracle sees lifted stretches, not tabs: one tab is several
/// observations, one per pass that had to ride over it. This folds them back
/// into one entry per tab, on the operation that asked for it.
///
/// # What is reported, and what is not
///
/// Every number here is **measured off the toolpath**: where the cutter was
/// when it lifted, how much metal that leaves, how tall it stands, and how far
/// along the part's outline the next tab is. What is deliberately *not*
/// reported is "this tab is 3 mm from where you asked for it", because that
/// subtraction cannot be done honestly: a requested position is a fraction of
/// the **cutter's own offset loop**, which runs in the direction of cut and is
/// a different length from the contour the caller drew, while a landed
/// position can only be measured on the contour the caller drew. The two
/// frames differ by a reversal and by arc-length drift around every corner,
/// and a difference taken across them would be a number that looks like
/// millimetres of settling and is not.
///
/// So: the count and the spacing are the checkable claims. Six tabs asked for
/// in one third of the loop that come back 20 mm apart went where they were
/// asked; three tabs that come back 70 mm apart did not.
fn tab_audit(
    job: &Job,
    built: &[(CamSettings, OperationReq, ToolEntry)],
    verification: &JobVerification,
) -> Vec<Value> {
    // Contours that asked for tabs, with their arc-length parameterisation.
    let mut tabbed: Vec<(usize, Vec<[f64; 2]>, Vec<f64>)> = Vec::new();
    for (index, (_, op, _)) in built.iter().enumerate() {
        let Ok(requested) = tab_positions(op, "") else {
            continue;
        };
        if requested.is_empty() {
            continue;
        }
        let CamOperation::Contour2D(c2d) = &job.ops[index].operation else {
            continue;
        };
        tabbed.push((index, loop_to_points(&c2d.contour), requested));
    }
    if tabbed.is_empty() {
        return Vec::new();
    }

    // Each observation belongs to whichever tabbed contour it sits closest to:
    // a job may tab two separate profiles, and a tab on one is not a tab on
    // the other.
    let mut assigned: Vec<Vec<(f64, &vcad_kernel_cam::verify2d::TabObservation)>> =
        vec![Vec::new(); tabbed.len()];
    for observation in &verification.tabs.observations {
        let mut best: Option<(usize, f64, f64)> = None;
        for (slot, (_, points, _)) in tabbed.iter().enumerate() {
            let (fraction, distance) = project(points, observation.xy);
            if best.is_none_or(|(_, _, d)| distance < d) {
                best = Some((slot, fraction, distance));
            }
        }
        if let Some((slot, fraction, _)) = best {
            assigned[slot].push((fraction, observation));
        }
    }

    let mut out = Vec::with_capacity(tabbed.len());
    for (slot, (index, points, requested)) in tabbed.iter().enumerate() {
        let observations = &assigned[slot];
        let total = perimeter(points);
        // Cluster by position round the loop: every pass that rode over the
        // same tab lands at the same place, within the tab's own width.
        let span = if total > 0.0 {
            (observations
                .iter()
                .map(|(_, o)| o.lifted_run)
                .fold(0.0f64, f64::max)
                / total)
                .max(1e-4)
        } else {
            1e-4
        };
        let mut clusters: Vec<Vec<usize>> = Vec::new();
        for (i, (fraction, _)) in observations.iter().enumerate() {
            match clusters
                .iter_mut()
                .find(|c| cyclic_gap(*fraction, observations[c[0]].0) <= span)
            {
                Some(c) => c.push(i),
                None => clusters.push(vec![i]),
            }
        }
        // In order round the part, so `gap_to_next_mm` reads as a walk.
        clusters.sort_by(|a, b| observations[a[0]].0.total_cmp(&observations[b[0]].0));

        let at: Vec<f64> = clusters.iter().map(|c| observations[c[0]].0).collect();
        let tabs: Vec<Value> = clusters
            .iter()
            .enumerate()
            .map(|(i, members)| {
                let n = members.len() as f64;
                let centre = members.iter().fold([0.0f64; 2], |a, i| {
                    let xy = observations[*i].1.xy;
                    [a[0] + xy[0] / n, a[1] + xy[1] / n]
                });
                let mean = |f: fn(&vcad_kernel_cam::verify2d::TabObservation) -> f64| {
                    members.iter().map(|i| f(observations[*i].1)).sum::<f64>() / n
                };
                let next = at[(i + 1) % at.len()];
                json!({
                    "at": centre,
                    // Fraction of the way round the contour *as the caller
                    // drew it* — not the frame `tab_positions` is stated in.
                    "along_contour": at[i],
                    "gap_to_next_mm": if at.len() > 1 {
                        (next - at[i]).rem_euclid(1.0) * total
                    } else {
                        total
                    },
                    "metal_width": mean(|o| o.metal_width),
                    "height": mean(|o| o.height),
                    "straight": members.iter().all(|i| observations[*i].1.straight),
                    "passes": members.len(),
                })
            })
            .collect();

        out.push(json!({
            "op": job.ops[*index].name,
            "op_index": index,
            "tool": job.ops[*index].tool_number,
            "requested": requested.len(),
            "requested_positions": requested,
            "found": tabs.len(),
            "perimeter_mm": total,
            "tabs": tabs,
            "note": "positions in `tab_positions` are fractions of the cutter's own offset loop; `at` and `gap_to_next_mm` are measured on the contour as drawn. Compare counts and spacing, not the two fractions.",
        }));
    }
    out
}

/// Where `p` falls along a closed polyline: the fraction of the way round, and
/// how far off the polyline it is.
fn project(points: &[[f64; 2]], p: [f64; 2]) -> (f64, f64) {
    let total = perimeter(points);
    if points.len() < 2 || total <= 0.0 {
        return (0.0, f64::MAX);
    }
    let (mut run, mut best) = (0.0f64, (0.0f64, f64::MAX));
    for i in 0..points.len() {
        let (a, b) = (points[i], points[(i + 1) % points.len()]);
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let length = dx.hypot(dy);
        if length > 0.0 {
            let t = (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / (length * length)).clamp(0.0, 1.0);
            let distance = (a[0] + t * dx - p[0]).hypot(a[1] + t * dy - p[1]);
            if distance < best.1 {
                best = ((run + t * length) / total, distance);
            }
        }
        run += length;
    }
    best
}

fn perimeter(points: &[[f64; 2]]) -> f64 {
    (0..points.len())
        .map(|i| {
            let (a, b) = (points[i], points[(i + 1) % points.len()]);
            (a[0] - b[0]).hypot(a[1] - b[1])
        })
        .sum()
}

/// Distance between two fractions of a closed loop, the short way round.
fn cyclic_gap(a: f64, b: f64) -> f64 {
    let d = (a - b).abs().rem_euclid(1.0);
    d.min(1.0 - d)
}

fn area(points: &[[f64; 2]]) -> f64 {
    let mut a = 0.0;
    for i in 0..points.len() {
        let p = points[i];
        let q = points[(i + 1) % points.len()];
        a += p[0] * q[1] - q[0] * p[1];
    }
    (a / 2.0).abs()
}

fn circle(cx: f64, cy: f64, r: f64) -> Vec<[f64; 2]> {
    (0..64)
        .map(|i| {
            let a = std::f64::consts::TAU * i as f64 / 64.0;
            [cx + r * a.cos(), cy + r * a.sin()]
        })
        .collect()
}
