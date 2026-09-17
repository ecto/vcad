//! Assembling several operations and several tools into one program.
//!
//! What this module exists to fix (friction log items 19, 42, 49): the app
//! could only post one operation with one tool, `M3` was followed straight
//! away by motion, and the spindle stopped and restarted between every
//! operation. Here a tool is started once, given time to come up to speed,
//! runs everything it has to run, and only then is changed — and on a machine
//! without a changer the program stops, says which tool goes in, and waits
//! for Z to be re-established.
//!
//! Ordering is not the order the operations were typed in: features inside
//! the part are cut before the outside profile that frees it from the stock,
//! and the operations of one tool are kept together.

use crate::operation::CamOperation;
use crate::post::{PostProcessor, ProgramOptions};
use crate::{
    check_tool_for_cut, CamSettings, CheckSeverity, CutContext, SpindleDir, ToolCheck, ToolEntry,
    ToolLibrary, Toolpath, ToolpathSegment,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Errors from assembling a job.
#[derive(Debug, Clone, Error, PartialEq)]
pub enum JobError {
    /// The job has no operations.
    #[error("the job has no operations")]
    NoOperations,

    /// An operation names a tool the library does not hold.
    #[error("operation \"{op}\" asks for T{tool_number}, which is not in the tool library")]
    UnknownTool {
        /// Operation name.
        op: String,
        /// The tool number asked for.
        tool_number: u32,
    },

    /// An operation could not produce a toolpath.
    #[error("operation \"{op}\" could not be generated: {message}")]
    OpFailed {
        /// Operation name.
        op: String,
        /// The operation's own message.
        message: String,
    },

    /// The tool is not up to the cut the operation asks of it.
    #[error("operation \"{op}\" cannot run on T{tool_number}: {}", findings.join("; "))]
    ToolCheckFailed {
        /// Operation name.
        op: String,
        /// The tool asked to do it.
        tool_number: u32,
        /// The error-level findings.
        findings: Vec<String>,
    },

    /// An operation's toolpath carries spindle or tool-change commands of its
    /// own, which would fight the ones the job emits.
    #[error(
        "operation \"{op}\" drives the spindle or changes tools inside its own toolpath, which the job already does"
    )]
    OpControlsSpindle {
        /// Operation name.
        op: String,
    },

    /// Two operations on one tool want different spindle speeds.
    #[error(
        "operation \"{op}\" wants {wanted:.0} rpm but T{tool_number} is already running at {running:.0} rpm: one spindle start per tool means one speed, so split the tool or match the speeds"
    )]
    SpindleSpeedConflict {
        /// The tool that is already running.
        tool_number: u32,
        /// Operation name.
        op: String,
        /// Speed the spindle was started at.
        running: f64,
        /// Speed this operation asks for.
        wanted: f64,
    },

    /// A 3D roughing operation needs a height field the job does not carry.
    #[error(
        "operation \"{op}\" is 3D roughing, which needs a height field: generate it separately"
    )]
    NeedsHeightField {
        /// Operation name.
        op: String,
    },

    /// The spin-up dwell is not a usable number.
    #[error("invalid spindle spin-up dwell: {0} (must be finite and >= 0)")]
    InvalidSpinUp(f64),

    /// The park height is not a usable number.
    #[error("invalid park Z: {0} (must be finite and > 0, above the stock top)")]
    InvalidParkZ(f64),
}

/// Work coordinate system the program runs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Wcs {
    /// G54.
    #[default]
    G54,
    /// G55.
    G55,
    /// G56.
    G56,
    /// G57.
    G57,
    /// G58.
    G58,
    /// G59.
    G59,
}

impl Wcs {
    /// The G-code word for this work offset.
    pub fn code(&self) -> &'static str {
        match self {
            Wcs::G54 => "G54",
            Wcs::G55 => "G55",
            Wcs::G56 => "G56",
            Wcs::G57 => "G57",
            Wcs::G58 => "G58",
            Wcs::G59 => "G59",
        }
    }
}

/// How the program ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ProgramEnd {
    /// `M2` — end of program, the FFI's current choice for stock Grbl.
    #[default]
    M2,
    /// `M30` — end of program and rewind.
    M30,
}

impl ProgramEnd {
    /// The G-code word.
    pub fn code(&self) -> &'static str {
        match self {
            ProgramEnd::M2 => "M2",
            ProgramEnd::M30 => "M30",
        }
    }
}

/// How the machine gets from one tool to the next.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ToolChangeStrategy {
    /// The machine has a changer: `T<n> M6` and carry on.
    M6,
    /// The machine has no changer. The program retracts, stops the spindle,
    /// names the tool and pauses with `M0`. Z has to be re-established before
    /// the operator resumes, because the new tool is a different length:
    /// either by running `probe_macro` on resume, or by touching off by hand
    /// as the program's own comment instructs.
    ManualPauseReprobe {
        /// G-code run on resume to re-establish Z, if the machine has a probe.
        probe_macro: Option<String>,
    },
}

/// Where an operation sits in the order of things.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum OpRole {
    /// Facing the stock: before anything is cut to size.
    Facing,
    /// A feature inside the part outline: a hole, a bore, a pocket, an inside
    /// contour.
    InsideFeature,
    /// The profile that cuts the part free from the stock. Once this has run
    /// the part is held by tabs at best, so nothing else may follow it.
    OutsideProfile,
}

impl OpRole {
    /// Phase this role runs in. Roles are cut in phase order, whatever order
    /// the operations were given in.
    fn phase(self) -> u8 {
        match self {
            OpRole::Facing => 0,
            OpRole::InsideFeature => 1,
            OpRole::OutsideProfile => 2,
        }
    }
}

/// An operation a job can run.
///
/// Wave 1 had a parallel enum here — `Cam` / `Drill` / `Bore` — because the
/// hole operations refuse in [`DrillError`](crate::DrillError)'s words and
/// [`CamError`](crate::CamError) had nowhere to put them. It has somewhere
/// now ([`CamError::Operation`](crate::CamError::Operation), which carries
/// the text verbatim), so there is one enum, and this name is kept only so
/// callers that spelled it out still compile.
pub type JobOperation = CamOperation;

/// The cut an operation asks of its tool, with the job's name on any refusal.
fn cut_context(
    operation: &CamOperation,
    entry: &ToolEntry,
    name: &str,
) -> Result<CutContext, JobError> {
    if operation.requires_height_field() {
        return Err(JobError::NeedsHeightField { op: name.into() });
    }
    operation
        .cut_context(&entry.tool)
        .map_err(|e| JobError::OpFailed {
            op: name.to_string(),
            message: e.to_string(),
        })
}

/// Generate an operation's toolpath, with the job's name on any refusal.
fn generate(
    operation: &CamOperation,
    entry: &ToolEntry,
    settings: &CamSettings,
    name: &str,
) -> Result<Toolpath, JobError> {
    if operation.requires_height_field() {
        return Err(JobError::NeedsHeightField { op: name.into() });
    }
    operation
        .generate_with_geometry(&entry.tool, &entry.geometry, settings)
        .map_err(|e| JobError::OpFailed {
            op: name.to_string(),
            message: e.to_string(),
        })
}

/// One operation in a job: what to cut, with which tool, and when.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobOp {
    /// Name shown in the operation list and posted as a comment. Real names
    /// matter: four operations all called "Inside contour" is how a bore and
    /// a pilot hole became indistinguishable.
    pub name: String,
    /// Number of the tool in the job's library.
    pub tool_number: u32,
    /// What to cut.
    pub operation: JobOperation,
    /// Where it sits in the order.
    pub role: OpRole,
    /// Explicit order within the role, lowest first. Operations that share an
    /// order key keep the order they were added in.
    pub order: i32,
    /// Settings for this operation; the job's settings are used when absent.
    pub settings: Option<CamSettings>,
}

impl JobOp {
    /// An operation with the role its kind implies.
    pub fn new(
        name: impl Into<String>,
        tool_number: u32,
        operation: impl Into<JobOperation>,
    ) -> Self {
        let operation = operation.into();
        Self {
            name: name.into(),
            tool_number,
            role: operation.default_role(),
            operation,
            order: 0,
            settings: None,
        }
    }

    /// Override the role.
    pub fn with_role(mut self, role: OpRole) -> Self {
        self.role = role;
        self
    }

    /// Set the explicit order key within the role.
    pub fn with_order(mut self, order: i32) -> Self {
        self.order = order;
        self
    }

    /// Give this operation its own settings.
    pub fn with_settings(mut self, settings: CamSettings) -> Self {
        self.settings = Some(settings);
        self
    }
}

/// A job: a tool library, a pile of operations, and the machine's habits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    /// Program name, posted in the header.
    pub name: String,
    /// The tools this job may use.
    pub tools: ToolLibrary,
    /// The operations, in any order.
    pub ops: Vec<JobOp>,
    /// Settings for operations that do not carry their own.
    pub settings: CamSettings,
    /// How the machine changes tools.
    pub strategy: ToolChangeStrategy,
    /// Seconds to wait after starting the spindle, before the first cut of
    /// that tool. A relay-switched router needs seconds, not the length of a
    /// plunge.
    pub spin_up: f64,
    /// Height the tool retracts to for a tool change and at the end, in mm.
    pub park_z: f64,
    /// Work coordinate system.
    pub wcs: Wcs,
    /// How the program ends.
    pub end: ProgramEnd,
    /// Spindle direction.
    pub spindle_dir: SpindleDir,
}

impl Job {
    /// A job with no operations yet, parking at the settings' retract height.
    pub fn new(name: impl Into<String>, tools: ToolLibrary, settings: CamSettings) -> Self {
        let park_z = settings.retract_z.max(settings.safe_z);
        Self {
            name: name.into(),
            tools,
            ops: Vec::new(),
            settings,
            strategy: ToolChangeStrategy::ManualPauseReprobe { probe_macro: None },
            spin_up: 3.0,
            park_z,
            wcs: Wcs::G54,
            end: ProgramEnd::M2,
            spindle_dir: SpindleDir::Cw,
        }
    }

    /// Set how the machine changes tools.
    pub fn with_strategy(mut self, strategy: ToolChangeStrategy) -> Self {
        self.strategy = strategy;
        self
    }

    /// Set the spindle spin-up dwell, in seconds.
    pub fn with_spin_up(mut self, seconds: f64) -> Self {
        self.spin_up = seconds;
        self
    }

    /// Set the park height for tool changes and the end of the program.
    pub fn with_park_z(mut self, park_z: f64) -> Self {
        self.park_z = park_z;
        self
    }

    /// Set the work coordinate system.
    pub fn with_wcs(mut self, wcs: Wcs) -> Self {
        self.wcs = wcs;
        self
    }

    /// Set the program end word.
    pub fn with_end(mut self, end: ProgramEnd) -> Self {
        self.end = end;
        self
    }

    /// Add an operation.
    pub fn push(&mut self, op: JobOp) -> &mut Self {
        self.ops.push(op);
        self
    }

    /// Add an operation, by value.
    pub fn with_op(mut self, op: JobOp) -> Self {
        self.ops.push(op);
        self
    }

    /// The order the operations run in, as indices into `ops`.
    ///
    /// Phases first (facing, inside features, the profile that frees the
    /// part), then tools kept together within a phase — a tool group runs
    /// where its earliest operation asked to run — then the explicit order
    /// key, then the order the operations were added in.
    pub fn order(&self) -> Vec<usize> {
        let key = |i: usize| (self.ops[i].order, i);
        let mut group_first: Vec<((u8, u32), (i32, usize))> = Vec::new();
        for (i, op) in self.ops.iter().enumerate() {
            let group = (op.role.phase(), op.tool_number);
            match group_first.iter_mut().find(|(g, _)| *g == group) {
                Some((_, first)) => *first = (*first).min(key(i)),
                None => group_first.push((group, key(i))),
            }
        }
        let group_key = |i: usize| {
            let group = (self.ops[i].role.phase(), self.ops[i].tool_number);
            group_first
                .iter()
                .find(|(g, _)| *g == group)
                .map(|(_, first)| *first)
                .unwrap_or((0, 0))
        };
        let mut order: Vec<usize> = (0..self.ops.len()).collect();
        order.sort_by_key(|&i| (self.ops[i].role.phase(), group_key(i), self.ops[i].order, i));
        order
    }

    /// Every tool-geometry finding for the job, operation by operation, in run
    /// order. Warnings included: [`Job::assemble`] refuses on errors only.
    pub fn checks(&self) -> Result<Vec<(usize, Vec<ToolCheck>)>, JobError> {
        let mut out = Vec::new();
        for i in self.order() {
            let op = &self.ops[i];
            let entry = self.entry(op)?;
            let cut = cut_context(&op.operation, entry, &op.name)?;
            out.push((i, check_tool_for_cut(&entry.tool, &entry.geometry, &cut)));
        }
        Ok(out)
    }

    fn entry(&self, op: &JobOp) -> Result<&ToolEntry, JobError> {
        self.tools
            .get_by_number(op.tool_number)
            .ok_or_else(|| JobError::UnknownTool {
                op: op.name.clone(),
                tool_number: op.tool_number,
            })
    }

    /// Assemble the program: one toolpath, and the ranges of it each block
    /// owns.
    pub fn assemble(&self) -> Result<Program, JobError> {
        if self.ops.is_empty() {
            return Err(JobError::NoOperations);
        }
        if !self.spin_up.is_finite() || self.spin_up < 0.0 {
            return Err(JobError::InvalidSpinUp(self.spin_up));
        }
        if !self.park_z.is_finite() || self.park_z <= 0.0 {
            return Err(JobError::InvalidParkZ(self.park_z));
        }

        let mut toolpath = Toolpath::new();
        let mut blocks: Vec<BlockRange> = Vec::new();
        let order = self.order();

        let start = toolpath.len();
        toolpath.push(ToolpathSegment::comment(format!(
            "job: {} — {} operation(s)",
            self.name,
            self.ops.len()
        )));
        blocks.push(BlockRange {
            block: ProgramBlock::Preamble,
            tool_number: None,
            start,
            end: toolpath.len(),
        });

        let mut running: Option<(u32, f64)> = None;
        let mut at = [0.0, 0.0, self.park_z];

        for index in order {
            let op = &self.ops[index];
            let entry = self.entry(op)?;
            let settings = op.settings.as_ref().unwrap_or(&self.settings);

            let cut = cut_context(&op.operation, entry, &op.name)?;
            let findings = check_tool_for_cut(&entry.tool, &entry.geometry, &cut);
            let errors: Vec<String> = findings
                .iter()
                .filter(|c| c.severity == CheckSeverity::Error)
                .map(|c| c.message.clone())
                .collect();
            if !errors.is_empty() {
                return Err(JobError::ToolCheckFailed {
                    op: op.name.clone(),
                    tool_number: op.tool_number,
                    findings: errors,
                });
            }

            let op_path = generate(&op.operation, entry, settings, &op.name)?;
            if op_path.segments.iter().any(|s| {
                matches!(
                    s,
                    ToolpathSegment::Spindle { .. } | ToolpathSegment::ToolChange { .. }
                )
            }) {
                return Err(JobError::OpControlsSpindle {
                    op: op.name.clone(),
                });
            }

            match running {
                Some((tool_number, rpm)) if tool_number == op.tool_number => {
                    // One spindle start per tool means one speed for the whole
                    // block: a second speed would be silently ignored.
                    if (rpm - settings.spindle_rpm).abs() > 1e-9 {
                        return Err(JobError::SpindleSpeedConflict {
                            tool_number,
                            op: op.name.clone(),
                            running: rpm,
                            wanted: settings.spindle_rpm,
                        });
                    }
                }
                _ => {
                    if let Some((previous, _)) = running {
                        let start = toolpath.len();
                        // Out of the cut first, then stop the spindle, then
                        // say what goes in — and nothing moves after that
                        // until the operator has resumed.
                        toolpath.push(ToolpathSegment::rapid(at[0], at[1], self.park_z));
                        toolpath.push(ToolpathSegment::spindle_off());
                        toolpath.push(ToolpathSegment::comment(format!(
                            "next tool: T{} {} (Ø{:.2})",
                            entry.number,
                            entry.name,
                            entry.tool.diameter()
                        )));
                        match &self.strategy {
                            ToolChangeStrategy::M6 => {
                                toolpath.push(ToolpathSegment::tool_change(entry.number))
                            }
                            ToolChangeStrategy::ManualPauseReprobe { probe_macro } => {
                                // A real stop, not a comment about one: the
                                // post writes the word, so every post writes
                                // the word it means.
                                toolpath.push(ToolpathSegment::pause(format!(
                                    "fit T{}, then re-establish Z before resuming",
                                    entry.number
                                )));
                                match probe_macro {
                                    Some(text) => toolpath
                                        .push(ToolpathSegment::raw(text.trim_end().to_string())),
                                    None => toolpath.push(ToolpathSegment::comment(format!(
                                        "re-establish Z for T{}: touch off and set {} Z0 before \
                                         resuming",
                                        entry.number,
                                        self.wcs.code()
                                    ))),
                                }
                            }
                        }
                        at = [at[0], at[1], self.park_z];
                        blocks.push(BlockRange {
                            block: ProgramBlock::ToolChange {
                                from: previous,
                                to: entry.number,
                            },
                            tool_number: Some(entry.number),
                            start,
                            end: toolpath.len(),
                        });
                    }

                    let start = toolpath.len();
                    toolpath.push(ToolpathSegment::comment(format!(
                        "T{} {} (Ø{:.2}) at {:.0} rpm",
                        entry.number,
                        entry.name,
                        entry.tool.diameter(),
                        settings.spindle_rpm
                    )));
                    toolpath.push(ToolpathSegment::spindle_on(
                        settings.spindle_rpm,
                        self.spindle_dir,
                    ));
                    if self.spin_up > 0.0 {
                        // The spindle has to be at speed before the first
                        // plunge, not on its way there.
                        toolpath.push(ToolpathSegment::dwell(self.spin_up));
                    }
                    blocks.push(BlockRange {
                        block: ProgramBlock::ToolStart { tool: entry.number },
                        tool_number: Some(entry.number),
                        start,
                        end: toolpath.len(),
                    });
                    running = Some((entry.number, settings.spindle_rpm));
                }
            }

            let start = toolpath.len();
            toolpath.push(ToolpathSegment::comment(format!("op: {}", op.name)));
            for seg in &op_path.segments {
                if let Some(to) = seg.target() {
                    at = to;
                }
                toolpath.push(seg.clone());
            }
            blocks.push(BlockRange {
                block: ProgramBlock::Operation {
                    name: op.name.clone(),
                    op_index: index,
                },
                tool_number: Some(entry.number),
                start,
                end: toolpath.len(),
            });
        }

        let start = toolpath.len();
        toolpath.push(ToolpathSegment::rapid(at[0], at[1], self.park_z));
        toolpath.push(ToolpathSegment::spindle_off());
        blocks.push(BlockRange {
            block: ProgramBlock::Postamble,
            tool_number: running.map(|(tool, _)| tool),
            start,
            end: toolpath.len(),
        });

        Ok(Program {
            name: self.name.clone(),
            toolpath,
            blocks,
            strategy: self.strategy.clone(),
            wcs: self.wcs,
            end: self.end,
            park_z: self.park_z,
        })
    }
}

/// What a stretch of the assembled toolpath is for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ProgramBlock {
    /// Program start.
    Preamble,
    /// Starting a tool: the spindle comes on and comes up to speed.
    ToolStart {
        /// The tool starting.
        tool: u32,
    },
    /// Getting from one tool to the next.
    ToolChange {
        /// The tool coming out.
        from: u32,
        /// The tool going in.
        to: u32,
    },
    /// One operation's own toolpath.
    Operation {
        /// The operation's name.
        name: String,
        /// Its index in the job's `ops`.
        op_index: usize,
    },
    /// Program end.
    Postamble,
}

/// A half-open range of the assembled toolpath, and what it is for. The
/// ranges of a program partition its toolpath: every segment is in exactly
/// one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlockRange {
    /// What this stretch is for.
    pub block: ProgramBlock,
    /// The tool in the spindle over this stretch, where there is one.
    pub tool_number: Option<u32>,
    /// First segment index.
    pub start: usize,
    /// One past the last segment index.
    pub end: usize,
}

impl BlockRange {
    /// Number of segments in this block.
    pub fn len(&self) -> usize {
        self.end - self.start
    }

    /// Whether the block has no segments.
    pub fn is_empty(&self) -> bool {
        self.end == self.start
    }
}

/// An assembled program: one toolpath the oracle and the arc fitter can read
/// whole, plus the ranges that say which operation and tool each stretch
/// belongs to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Program {
    /// Program name.
    pub name: String,
    /// The whole job as one toolpath.
    pub toolpath: Toolpath,
    /// The blocks, in order, partitioning `toolpath`.
    pub blocks: Vec<BlockRange>,
    /// How tools are changed (the pause lives here, not in the toolpath).
    pub strategy: ToolChangeStrategy,
    /// Work coordinate system.
    pub wcs: Wcs,
    /// How the program ends.
    pub end: ProgramEnd,
    /// Park height in mm.
    pub park_z: f64,
}

impl Program {
    /// The blocks that are operations, in run order.
    pub fn op_ranges(&self) -> impl Iterator<Item = &BlockRange> {
        self.blocks
            .iter()
            .filter(|b| matches!(b.block, ProgramBlock::Operation { .. }))
    }

    /// The tool numbers in the order they are used, one entry per block of
    /// work (a tool used twice, far apart, appears twice).
    pub fn tool_sequence(&self) -> Vec<u32> {
        self.blocks
            .iter()
            .filter_map(|b| match b.block {
                ProgramBlock::ToolStart { tool } => Some(tool),
                _ => None,
            })
            .collect()
    }

    /// What the post needs to know about this program that is not in the
    /// toolpath: its name, its work offset, where it parks and how it ends.
    pub fn options(&self) -> ProgramOptions {
        ProgramOptions {
            name: self.name.clone(),
            wcs: Some(self.wcs),
            park_z: Some(self.park_z),
            end: self.end,
            spindle: None,
        }
    }

    /// Post the whole program.
    ///
    /// Thin on purpose. Until wave 2 the header, the `M0` pause and the end
    /// word were written *here*, in one dialect, because a post's own header
    /// assumed a single-tool program and started a spindle of its own that
    /// fought the one the job had already started — and because
    /// `ToolpathSegment` had no way to say "stop and wait". Both holes are
    /// closed: the pause is a [`ToolpathSegment::Pause`] like any other
    /// segment, and the preamble and postamble are the post's.
    pub fn to_gcode<P: PostProcessor + ?Sized>(&self, post: &P) -> String {
        post.program(&self.options(), &self.toolpath)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operation::{Contour, Contour2D, Face, Pocket2D};
    use crate::post::GrblPost;
    use crate::{Drill, HelicalBore, Tool, ToolGeometry};

    fn library() -> ToolLibrary {
        let mut lib = ToolLibrary::new();
        lib.add(
            ToolEntry::new(
                1,
                "Ø6 flat",
                Tool::FlatEndMill {
                    diameter: 6.0,
                    flute_length: 20.0,
                    flutes: 2,
                },
            )
            .with_geometry(ToolGeometry::new().with_stickout(25.0)),
        );
        lib.add(
            ToolEntry::new(
                2,
                "Ø3 drill",
                Tool::Drill {
                    diameter: 3.0,
                    point_angle: 118.0,
                },
            )
            .with_geometry(
                ToolGeometry::new()
                    .with_flute_length(30.0)
                    .with_stickout(35.0),
            ),
        );
        lib.add(
            ToolEntry::new(
                3,
                "Ø2 flat",
                Tool::FlatEndMill {
                    diameter: 2.0,
                    flute_length: 10.0,
                    flutes: 2,
                },
            )
            .with_geometry(
                ToolGeometry::new()
                    .with_stickout(15.0)
                    .with_centre_cutting(true),
            ),
        );
        lib
    }

    fn settings() -> CamSettings {
        CamSettings {
            stepdown: 2.0,
            safe_z: 5.0,
            retract_z: 25.0,
            ..CamSettings::default()
        }
    }

    fn outside_profile() -> JobOp {
        JobOp::new(
            "Outside profile",
            1,
            CamOperation::Contour2D(Contour2D::outside(
                Contour::rectangle(0.0, 0.0, 50.0, 40.0),
                4.0,
            )),
        )
    }

    fn inside_pocket() -> JobOp {
        JobOp::new(
            "Bore pocket",
            1,
            CamOperation::Pocket2D(Pocket2D::rectangle(10.0, 10.0, 20.0, 15.0, 3.0)),
        )
    }

    fn pilot_holes() -> JobOp {
        JobOp::new(
            "Pilot holes",
            2,
            Drill::peck([(12.0, 12.0), (38.0, 28.0)], 4.0, 1.5),
        )
    }

    fn job() -> Job {
        Job::new("test", library(), settings())
    }

    /// One `M3` per tool, and a dwell between it and the first cut of that
    /// tool — the spindle is at speed before the cutter is in the metal.
    #[test]
    fn test_one_spindle_start_and_dwell_per_tool() {
        let program = job()
            .with_op(outside_profile())
            .with_op(pilot_holes())
            .with_spin_up(3.0)
            .assemble()
            .unwrap();

        let starts: Vec<usize> = program
            .toolpath
            .segments
            .iter()
            .enumerate()
            .filter(|(_, s)| matches!(s, ToolpathSegment::Spindle { rpm, .. } if *rpm > 0.0))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(starts.len(), 2, "one start per tool");
        assert_eq!(program.tool_sequence(), vec![2, 1]);

        for start in starts {
            let ToolpathSegment::Dwell { seconds } = &program.toolpath.segments[start + 1] else {
                panic!("no dwell after the spindle start");
            };
            assert!((seconds - 3.0).abs() < 1e-9);
            // Nothing cuts between the start and the dwell, and the next cut
            // comes after both.
            let next_cut = program.toolpath.segments[start..]
                .iter()
                .position(|s| s.is_cutting())
                .unwrap();
            assert!(next_cut > 1, "cut {next_cut} segments after M3");
        }

        // And the spindle is not stopped between operations of one tool.
        let stops = program
            .toolpath
            .segments
            .iter()
            .filter(|s| matches!(s, ToolpathSegment::Spindle { rpm, .. } if *rpm <= 0.0))
            .count();
        assert_eq!(stops, 2, "one stop per tool change plus the end");
    }

    /// Between tools: up to park, spindle off, and then nothing moves until
    /// the pause.
    #[test]
    fn test_nothing_moves_between_the_spindle_stop_and_the_pause() {
        let program = job()
            .with_op(outside_profile())
            .with_op(pilot_holes())
            .assemble()
            .unwrap();

        let change = program
            .blocks
            .iter()
            .find(|b| matches!(b.block, ProgramBlock::ToolChange { .. }))
            .expect("a tool change");
        let segments = &program.toolpath.segments[change.start..change.end];
        let stop = segments
            .iter()
            .position(|s| matches!(s, ToolpathSegment::Spindle { rpm, .. } if *rpm <= 0.0))
            .expect("M5 in the tool change");

        // The last move before the stop lifts to the park height.
        let last_move = segments[..stop]
            .iter()
            .rev()
            .find_map(|s| s.target())
            .expect("a retract before M5");
        assert!(
            (last_move[2] - 25.0).abs() < 1e-9,
            "retracted to {last_move:?}, not the park height"
        );
        assert!(last_move[2] >= settings().safe_z);
        // After the stop the block is words only.
        for seg in &segments[stop + 1..] {
            assert!(seg.target().is_none(), "moved after M5: {seg:?}");
        }
    }

    /// `M0` between tools under the manual strategy, `M6` and no pause under
    /// a changer.
    #[test]
    fn test_pause_only_under_the_manual_strategy() {
        let post = GrblPost::default();

        let manual = job()
            .with_op(outside_profile())
            .with_op(pilot_holes())
            .with_strategy(ToolChangeStrategy::ManualPauseReprobe { probe_macro: None })
            .assemble()
            .unwrap()
            .to_gcode(&post);
        assert_eq!(manual.matches("\nM0\n").count(), 1);
        assert!(!manual.contains("M6"));
        assert!(manual.contains("re-establish Z for T1"), "{manual}");

        let probed = job()
            .with_op(outside_profile())
            .with_op(pilot_holes())
            .with_strategy(ToolChangeStrategy::ManualPauseReprobe {
                probe_macro: Some("G38.2 Z-30 F100\nG10 L20 P1 Z19.05".into()),
            })
            .assemble()
            .unwrap()
            .to_gcode(&post);
        let pause = probed.find("\nM0\n").expect("a pause");
        let probe = probed.find("G38.2").expect("the probe macro");
        assert!(probe > pause, "the probe has to run on resume");

        let changer = job()
            .with_op(outside_profile())
            .with_op(pilot_holes())
            .with_strategy(ToolChangeStrategy::M6)
            .assemble()
            .unwrap()
            .to_gcode(&post);
        assert!(!changer.contains("M0\n"), "{changer}");
        assert_eq!(changer.matches("M6").count(), 1);
    }

    /// The outside profile runs last however the operations are given, and a
    /// tool's operations stay together.
    #[test]
    fn test_inside_features_come_before_the_profile_that_frees_the_part() {
        let backwards = job()
            .with_op(outside_profile()) // given first
            .with_op(pilot_holes())
            .with_op(inside_pocket());
        assert_eq!(backwards.order(), vec![1, 2, 0]);

        let program = backwards.assemble().unwrap();
        let names: Vec<&str> = program
            .op_ranges()
            .map(|b| match &b.block {
                ProgramBlock::Operation { name, .. } => name.as_str(),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(names, vec!["Pilot holes", "Bore pocket", "Outside profile"]);

        // T1 does the pocket and the profile back to back: one tool change in
        // the whole program.
        assert_eq!(program.tool_sequence(), vec![2, 1]);
        let changes = program
            .blocks
            .iter()
            .filter(|b| matches!(b.block, ProgramBlock::ToolChange { .. }))
            .count();
        assert_eq!(changes, 1);

        // The explicit order key sorts within a phase.
        let ordered = job()
            .with_op(pilot_holes().with_order(10))
            .with_op(inside_pocket().with_order(-5))
            .with_op(outside_profile().with_order(-100));
        assert_eq!(
            ordered.order(),
            vec![1, 0, 2],
            "the profile still runs last"
        );
    }

    /// One tool at a time: operations that share a tool are run together even
    /// when another tool's operation was given between them.
    #[test]
    fn test_a_tool_is_loaded_once_for_all_its_operations() {
        let second_pocket = JobOp::new(
            "Second pocket",
            1,
            CamOperation::Pocket2D(Pocket2D::rectangle(32.0, 10.0, 8.0, 8.0, 3.0)),
        );
        let interleaved = job()
            .with_op(inside_pocket()) // T1
            .with_op(pilot_holes()) // T2
            .with_op(second_pocket); // T1 again
        assert_eq!(interleaved.order(), vec![0, 2, 1]);

        let program = interleaved.assemble().unwrap();
        assert_eq!(program.tool_sequence(), vec![1, 2]);
        let changes = program
            .blocks
            .iter()
            .filter(|b| matches!(b.block, ProgramBlock::ToolChange { .. }))
            .count();
        assert_eq!(changes, 1, "T1 was loaded twice");
    }

    /// The blocks partition the toolpath: no gaps, no overlaps, nothing left
    /// over.
    #[test]
    fn test_blocks_partition_the_toolpath() {
        let program = job()
            .with_op(outside_profile())
            .with_op(pilot_holes())
            .with_op(inside_pocket())
            .assemble()
            .unwrap();

        let mut at = 0;
        for block in &program.blocks {
            assert_eq!(block.start, at, "gap or overlap at {block:?}");
            assert!(block.end >= block.start);
            at = block.end;
        }
        assert_eq!(at, program.toolpath.len());

        // Each operation's range holds that operation's own segments.
        for range in program.op_ranges() {
            let ProgramBlock::Operation { name, op_index } = &range.block else {
                unreachable!()
            };
            assert!(range.len() > 1, "{name} is empty");
            let first = &program.toolpath.segments[range.start];
            assert!(
                matches!(first, ToolpathSegment::Comment { text } if text == &format!("op: {name}")),
                "{first:?}"
            );
            assert!(*op_index < program.blocks.len());
        }
    }

    /// The posted text carries the header, the work offset, and exactly one
    /// program end.
    #[test]
    fn test_posted_program_header_and_end() {
        let program = job()
            .with_op(pilot_holes())
            .with_op(outside_profile())
            .with_wcs(Wcs::G55)
            .with_end(ProgramEnd::M30)
            .assemble()
            .unwrap();
        let gcode = program.to_gcode(&GrblPost::default());

        let header: Vec<&str> = gcode.lines().take(9).collect();
        assert_eq!(
            header[2..9],
            ["G21", "G90", "G94", "G17", "G40", "G49", "G55"]
        );
        assert!(gcode.lines().nth(9).unwrap().starts_with("G0 Z25"));
        assert_eq!(gcode.matches("G54").count(), 0);

        assert_eq!(gcode.matches("\nM30\n").count(), 1);
        assert!(gcode.trim_end().ends_with("M30"));
        assert_eq!(gcode.matches("M2\n").count(), 0);

        // One M3 per tool, each followed by its spin-up dwell, in the text.
        assert_eq!(gcode.matches("M3 S").count(), 2);
        for (i, line) in gcode.lines().enumerate() {
            if line.starts_with("M3 S") {
                assert_eq!(gcode.lines().nth(i + 1).unwrap(), "G4 P3000");
            }
        }
    }

    /// A tool that cannot make the cut stops the job, with the reason.
    #[test]
    fn test_assembly_refuses_a_tool_that_cannot_make_the_cut() {
        // 30 mm deep with 20 mm flutes and 25 mm of stickout.
        let deep = JobOp::new(
            "Deep profile",
            1,
            CamOperation::Contour2D(Contour2D::outside(
                Contour::rectangle(0.0, 0.0, 50.0, 40.0),
                30.0,
            )),
        );
        let err = job().with_op(deep).assemble().unwrap_err();
        let JobError::ToolCheckFailed {
            op,
            tool_number,
            findings,
        } = err
        else {
            panic!("{err:?}");
        };
        assert_eq!(op, "Deep profile");
        assert_eq!(tool_number, 1);
        assert!(
            findings.iter().any(|f| f.contains("flutes")),
            "{findings:?}"
        );
        assert!(
            findings.iter().any(|f| f.contains("stickout")),
            "{findings:?}"
        );
    }

    /// Operation-level refusals carry the operation's own message out.
    #[test]
    fn test_assembly_carries_operation_refusals_out() {
        // A Ø1.5 hole asked of the Ø2 cutter: the tool geometry is fine, the
        // operation is not.
        let op = JobOp::new(
            "Undersize bore",
            3,
            HelicalBore::new(10.0, 10.0, 1.5, 2.0, 0.3),
        );
        let err = job().with_op(op).assemble().unwrap_err();
        let JobError::OpFailed { op, message } = err else {
            panic!("{err:?}");
        };
        assert_eq!(op, "Undersize bore");
        assert!(
            message.contains("not larger than the Ø2.000 cutter"),
            "{message}"
        );

        let unknown = JobOp::new("Nowhere", 9, Drill::new([(1.0, 1.0)], 1.0));
        assert_eq!(
            job().with_op(unknown).assemble().unwrap_err(),
            JobError::UnknownTool {
                op: "Nowhere".into(),
                tool_number: 9
            }
        );

        assert_eq!(job().assemble().unwrap_err(), JobError::NoOperations);
    }

    /// One spindle start per tool means one speed: a second speed on the same
    /// tool is refused rather than dropped.
    #[test]
    fn test_two_speeds_on_one_tool_are_refused() {
        let faster = CamSettings {
            spindle_rpm: 18000.0,
            ..settings()
        };
        let err = job()
            .with_op(inside_pocket())
            .with_op(
                JobOp::new(
                    "Second pocket",
                    1,
                    CamOperation::Pocket2D(Pocket2D::rectangle(5.0, 5.0, 10.0, 10.0, 2.0)),
                )
                .with_settings(faster),
            )
            .assemble()
            .unwrap_err();
        assert!(
            matches!(err, JobError::SpindleSpeedConflict { tool_number: 1, .. }),
            "{err:?}"
        );
    }

    /// Facing runs before the features, whatever order it was added in.
    #[test]
    fn test_facing_runs_first() {
        let facing = JobOp::new(
            "Face the stock",
            1,
            CamOperation::Face(Face::from_size(50.0, 40.0, 0.5)),
        );
        assert_eq!(facing.role, OpRole::Facing);
        let job = job()
            .with_op(outside_profile())
            .with_op(facing)
            .with_op(pilot_holes());
        assert_eq!(job.order(), vec![1, 2, 0]);
    }

    #[test]
    fn test_job_serialization() {
        let job = job().with_op(pilot_holes()).with_op(outside_profile());
        let json = serde_json::to_string(&job).unwrap();
        let parsed: Job = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.ops.len(), 2);
        let program = parsed.assemble().unwrap();
        let json = serde_json::to_string(&program).unwrap();
        let parsed: Program = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.blocks.len(), program.blocks.len());
    }
}
