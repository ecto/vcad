//! `vcad.cam-claims/1` — what a CAM job claims about the part it will make.
//!
//! Wave 1 gave CAM real oracles: [`crate::verify2d`] replays a job against the
//! part, [`crate::fit`] says whether the cutter can reach the shape,
//! [`crate::outline`] compares a contour against the solid it came from,
//! [`crate::gear`] prices a tooth space and its measurement over pins, and
//! [`crate::materials`] grades the feeds. None of them produced a receipt.
//! This module is that receipt, in the shape the rest of the repo already
//! uses (`vcad.particle-claims/1`, `vcad.tolerance-claims/1`, …): a claim
//! family with an open domain vocabulary, translated into the unified
//! [`vcad_receipt::DesignReceipt`] schema by [`design_claims`].
//!
//! # The ladder
//!
//! CAM is unusual among the claim families in that some of its claims are
//! *not* predictions. "This program never brings the cutter inside the part"
//! is arithmetic on the program: it is true or false, and no measurement can
//! make it truer. So the ladder has two rungs, and which one a claim sits on
//! is a property of the claim, not of how hard the oracle worked:
//!
//! * **Computed** ([`Basis::Computed`]) — a property of the program, the
//!   contour and the tool. Reaches [`ClaimStatus::Holds`] or
//!   [`ClaimStatus::Violated`] on its own.
//! * **Predicted** ([`Basis::Predicted`]) — a statement about the *physical
//!   part*: a dimension it will have, a tab that will hold. It can never be
//!   better than [`ClaimStatus::Provisional`] until a measurement binds it
//!   ([`bind`]), at which point it becomes `Holds` or `Violated` on
//!   [`Basis::Measured`]. A predicted claim that fails its own arithmetic is
//!   `Violated` straight away — the fast path saying "no" is actionable.
//!
//! Two statuses are neither: [`ClaimStatus::Unverified`] (the oracle could
//! not run — no work offset, no travel limits, no reachability verdict) and
//! [`ClaimStatus::Stale`] (the inputs moved under a stored claim). Neither
//! ever reads as a pass, on either rung.
//!
//! # Basis, and why a claim goes stale
//!
//! Every claim names the inputs it rests on ([`Claim::depends_on`]) by key
//! into a [`Fingerprint`] — the G-code or toolpath, the outline, the tool,
//! the stock, the gear, the material. Re-hash the inputs and call
//! [`restate`]: a claim whose basis moved comes back `Stale`, exactly as the
//! mechanical clearance assertions do when the document changes under a
//! stored receipt. A receipt that cannot say *which* job it certifies
//! certifies nothing.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::fit::FitReport;
use crate::gear::{Compensation, GearError, GearReport, SpurGear};
use crate::materials::{Note, NoteLevel};
use crate::outline::{OutlineDiff, PrismaticReport};
use crate::verify2d::{JobSpec, JobVerification, VerifyOptions};

/// Schema tag for this claim family.
pub const CLAIM_SCHEMA: &str = "vcad.cam-claims/1";

/// Domain tag for CAM claims in the unified [`vcad_receipt`] schema.
pub const RECEIPT_DOMAIN: &str = "cam";

// ---------------------------------------------------------------------------
// Fingerprints: what a claim rests on
// ---------------------------------------------------------------------------

/// Basis key: the G-code program or toolpath the claim was checked against.
pub const BASIS_PROGRAM: &str = "program";
/// Basis key: the part contour (outline loops) in the stock frame.
pub const BASIS_OUTLINE: &str = "outline";
/// Basis key: the cutter.
pub const BASIS_TOOL: &str = "tool";
/// Basis key: the stock and the job's declared frame (thickness, allowance,
/// spoilboard, work offset, travel limits, declared tabs).
pub const BASIS_STOCK: &str = "stock";
/// Basis key: the gear definition.
pub const BASIS_GEAR: &str = "gear";
/// Basis key: the material, machine and spindle the feeds were graded against.
pub const BASIS_MATERIAL: &str = "material";
/// Basis key: the target solid an outline was compared against.
pub const BASIS_SOLID: &str = "solid";

/// FNV-1a over bytes. A change detector, not a cryptographic hash: two
/// different jobs colliding is a one-in-2^64 bookkeeping accident, not an
/// attack surface. Chosen so this module stays dependency-free, the way the
/// other claim families are.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Hex digest of a byte slice, as stored in a [`Fingerprint`].
pub fn digest(bytes: &[u8]) -> String {
    format!("{:016x}", fnv1a(bytes))
}

/// The hashes a claim set's claims point at.
///
/// Keys are open (the `BASIS_*` constants are the ones this module produces);
/// values are hex digests. A key that is *absent* when a claim depends on it
/// is treated as a change, not as "unchanged" — fail-closed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fingerprint {
    /// Digest per input key.
    pub entries: BTreeMap<String, String>,
}

impl Fingerprint {
    /// An empty fingerprint.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a pre-computed digest.
    pub fn with(mut self, key: impl Into<String>, digest: impl Into<String>) -> Self {
        self.entries.insert(key.into(), digest.into());
        self
    }

    /// Hash raw text (a G-code program, a DXF).
    pub fn with_text(self, key: impl Into<String>, text: &str) -> Self {
        let d = digest(text.as_bytes());
        self.with(key, d)
    }

    /// Hash anything serializable — a [`crate::Toolpath`], a [`crate::Tool`],
    /// a [`JobSpec`], a [`SpurGear`].
    ///
    /// Serialization is via `serde_json`; the wire shapes in this crate are
    /// structs and enums, whose field order is fixed, so the digest is stable
    /// for a given value. A value that will not serialize records the empty
    /// digest, which never matches and so reads as changed.
    pub fn with_json<T: Serialize>(self, key: impl Into<String>, value: &T) -> Self {
        match serde_json::to_vec(value) {
            Ok(bytes) => {
                let d = digest(&bytes);
                self.with(key, d)
            }
            Err(_) => self.with(key, String::new()),
        }
    }

    /// Hash a set of closed loops (a part contour in the stock frame),
    /// rounded to the nanometre so that re-reading the same DXF gives the
    /// same digest.
    pub fn with_loops(self, key: impl Into<String>, loops: &[Vec<[f64; 2]>]) -> Self {
        let mut buf = Vec::with_capacity(loops.iter().map(|l| l.len() * 18).sum());
        for l in loops {
            buf.extend_from_slice(b"|");
            for p in l {
                buf.extend_from_slice(&((p[0] * 1e6).round() as i64).to_le_bytes());
                buf.extend_from_slice(&((p[1] * 1e6).round() as i64).to_le_bytes());
            }
        }
        let d = digest(&buf);
        self.with(key, d)
    }

    /// Look a key up.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries.get(key).map(String::as_str)
    }

    /// Keys of `depends_on` whose digest differs from `self`, or that `self`
    /// does not carry at all. Empty means the basis is intact.
    pub fn drifted(&self, depends_on: &[String], stored: &Fingerprint) -> Vec<String> {
        depends_on
            .iter()
            .filter(|k| self.get(k) != stored.get(k) || self.get(k).is_none())
            .cloned()
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Claims
// ---------------------------------------------------------------------------

/// Which rung of the ladder a claim sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Basis {
    /// Arithmetic on the program, the contour and the tool. No measurement
    /// can improve it and none is needed.
    Computed,
    /// A statement about the physical part. Never better than
    /// [`ClaimStatus::Provisional`] until a measurement binds it.
    Predicted,
    /// Closed by a measurement of the real part.
    Measured,
}

/// Where a claim stands. Fail-closed: only `Holds` is clean, and a
/// `Predicted` claim can never reach it without a measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClaimStatus {
    /// The claim is true, on computed or measured evidence.
    Holds,
    /// The claim's arithmetic passes, but it is about a part nobody has
    /// measured. A promissory note, not a pass.
    Provisional,
    /// The claim is false.
    Violated,
    /// The oracle could not check it — a missing input, named in
    /// [`Claim::detail`]. Never a pass.
    Unverified,
    /// The inputs moved since the claim was made. Never a pass.
    Stale,
}

impl ClaimStatus {
    /// Fail-closed severity order, worst first: `Violated` > `Stale` >
    /// `Unverified` > `Provisional` > `Holds`.
    fn rank(self) -> u8 {
        match self {
            ClaimStatus::Holds => 0,
            ClaimStatus::Provisional => 1,
            ClaimStatus::Unverified => 2,
            ClaimStatus::Stale => 3,
            ClaimStatus::Violated => 4,
        }
    }
}

/// One CAM claim.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Claim {
    /// Dotted claim name, e.g. `"job.no_gouge"`, `"gear.over_pins"`.
    pub name: String,
    /// What the claim is about when narrower than the whole job:
    /// `"gear:planet-20T"`, `"hole:3"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// Where the claim stands.
    pub status: ClaimStatus,
    /// Which rung of the ladder it sits on.
    pub basis: Basis,
    /// What the oracle computed (or, once bound, what was measured).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    /// The bound the value is held to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<f64>,
    /// The measured value, once a measurement has been bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measured: Option<f64>,
    /// Unit of `value`/`limit`/`measured`. `"1"` for counts and ratios.
    pub unit: String,
    /// The other numbers behind the claim, so a reader never has to re-run
    /// the oracle to see why it said what it said.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metrics: BTreeMap<String, f64>,
    /// [`Fingerprint`] keys this claim rests on.
    pub depends_on: Vec<String>,
    /// What the claim means, in a machinist's words.
    pub note: String,
    /// Why it is `Unverified`, `Stale` or `Violated`. Always present for
    /// those three by construction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// The measurement that closed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measurement: Option<Measurement>,
    /// Name of the claim this one replaces — a re-cut after compensation
    /// supersedes the claim the measurement violated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<String>,
}

impl Claim {
    fn new(name: &str, unit: &str, note: &str, basis: Basis, depends_on: &[&str]) -> Self {
        Self {
            name: name.to_string(),
            subject: None,
            status: ClaimStatus::Holds,
            basis,
            value: None,
            limit: None,
            measured: None,
            unit: unit.to_string(),
            metrics: BTreeMap::new(),
            depends_on: depends_on.iter().map(|s| (*s).to_string()).collect(),
            note: note.to_string(),
            detail: None,
            measurement: None,
            supersedes: None,
        }
    }

    fn at(mut self, value: f64, limit: f64) -> Self {
        self.value = Some(value);
        self.limit = Some(limit);
        self
    }

    fn metric(mut self, key: &str, value: f64) -> Self {
        self.metrics.insert(key.to_string(), value);
        self
    }

    fn subject(mut self, s: &str) -> Self {
        self.subject = Some(s.to_string());
        self
    }

    /// Settle a claim that passed its own arithmetic onto the right rung:
    /// a computed claim `Holds`, a predicted one is only `Provisional`.
    fn settled(mut self) -> Self {
        self.status = match self.basis {
            Basis::Computed | Basis::Measured => ClaimStatus::Holds,
            Basis::Predicted => ClaimStatus::Provisional,
        };
        self
    }

    fn violated(mut self, why: impl Into<String>) -> Self {
        self.status = ClaimStatus::Violated;
        self.detail = Some(why.into());
        self
    }

    fn unverified(mut self, why: impl Into<String>) -> Self {
        self.status = ClaimStatus::Unverified;
        self.detail = Some(why.into());
        self
    }

    /// Settle on the arithmetic: `ok` decides between the settled rung and
    /// `Violated`.
    fn decide(self, ok: bool, why: impl Into<String>) -> Self {
        if ok {
            self.settled()
        } else {
            self.violated(why)
        }
    }
}

/// How the claim set was produced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Provenance {
    /// Oracles that contributed, e.g. `["verify2d", "fit", "gear"]`.
    pub oracles: Vec<String>,
    /// Crate version.
    pub version: String,
    /// Free-form context — the job name, the machine, the operator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

impl Default for Provenance {
    fn default() -> Self {
        Self {
            oracles: Vec::new(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            context: None,
        }
    }
}

/// The full `vcad.cam-claims/1` claim set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClaimSet {
    /// Schema tag ([`CLAIM_SCHEMA`]).
    pub schema: String,
    /// How it was produced.
    pub provenance: Provenance,
    /// The hashes the claims rest on.
    pub fingerprint: Fingerprint,
    /// The claims.
    pub claims: Vec<Claim>,
}

impl ClaimSet {
    /// Assemble a claim set.
    pub fn new(fingerprint: Fingerprint, provenance: Provenance, claims: Vec<Claim>) -> Self {
        Self {
            schema: CLAIM_SCHEMA.to_string(),
            provenance,
            fingerprint,
            claims,
        }
    }

    /// Fail-closed rollup. An empty set is `Unverified` — no claims is no
    /// evidence, never a clean job.
    pub fn status(&self) -> ClaimStatus {
        self.claims
            .iter()
            .map(|c| c.status)
            .max_by_key(|s| s.rank())
            .unwrap_or(ClaimStatus::Unverified)
    }

    /// True only when every claim `Holds`. A `Provisional` set has not
    /// earned a pass.
    pub fn all_hold(&self) -> bool {
        !self.claims.is_empty() && self.claims.iter().all(|c| c.status == ClaimStatus::Holds)
    }

    /// Find a claim by name and subject.
    pub fn find(&self, name: &str, subject: Option<&str>) -> Option<&Claim> {
        self.claims
            .iter()
            .find(|c| c.name == name && c.subject.as_deref() == subject)
    }
}

/// Things this module refuses to guess at.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ClaimError {
    /// A measurement names a claim that is not in the set.
    #[error("measurement of {0:?} matches no claim in the set")]
    NoSuchClaim(String),
    /// A measurement names a claim on the computed rung.
    #[error(
        "claim {0:?} is arithmetic on the job, not a dimension of the part; \
         a measurement cannot close it"
    )]
    NotPredicted(String),
    /// A measurement's own numbers are unusable.
    #[error("measurement of {0:?} has an unusable value, uncertainty or tolerance")]
    BadMeasurement(String),
    /// The claim a measurement is meant to close has no predicted value.
    #[error("claim {0:?} carries no predicted value to compare a measurement against")]
    NothingToCompare(String),
}

// ---------------------------------------------------------------------------
// Builders: wave-1 reports -> claims
// ---------------------------------------------------------------------------

/// Claims from a 2D job replay ([`crate::verify2d::verify_gcode`] /
/// [`crate::verify2d::verify_toolpath`]).
///
/// Nine claims, on both rungs. The geometric checks are arithmetic on the
/// program; `job.tabs_hold` — that the tabs will actually carry the part
/// through the last pass — is a prediction about metal, and stays
/// `Provisional` until someone cuts one and it holds.
///
/// Two of them are places where the 2D oracle passes *vacuously*, and the
/// claim refuses to:
///
/// * `job.envelope_in_travel` is `Unverified` without a work offset and
///   travel limits. [`crate::verify2d`] reports the swept extents either way
///   and its check passes when there is nothing to compare them to; a receipt
///   that read that as "the job fits the machine" would be lying.
/// * `job.tabs*` is `Unverified` when the job declares no tabs and the moves
///   show none — there is nothing to audit, which is not the same as tabs
///   that hold.
pub fn job_claims(v: &JobVerification, spec: &JobSpec, opts: &VerifyOptions) -> Vec<Claim> {
    let deps: &[&str] = &[BASIS_PROGRAM, BASIS_OUTLINE, BASIS_TOOL, BASIS_STOCK];
    let mut out = Vec::with_capacity(9);

    // 1. Gouge — the inside contour that cut outward (friction log item 32).
    out.push(
        Claim::new(
            "job.no_gouge",
            "mm",
            "no cutting move brings the swept cutter inside the part beyond \
             the stated tolerance",
            Basis::Computed,
            deps,
        )
        .at(v.gouge.worst, opts.tolerance)
        .metric("violations", v.gouge.violation_count as f64)
        .decide(
            v.gouge.pass,
            format!(
                "{} cutting move(s) enter the part, worst {:.4} mm past the {:.4} mm tolerance",
                v.gouge.violation_count, v.gouge.worst, opts.tolerance
            ),
        ),
    );

    // 2. Material left on the walls the passes were meant to reach.
    let ml = &v.material_left;
    out.push(
        Claim::new(
            "job.material_left",
            "mm",
            "every wall the full-depth passes reach is cut to size",
            Basis::Computed,
            deps,
        )
        .at(ml.max_standoff, opts.tolerance)
        .metric("unswept_area_mm2", ml.unswept_area)
        .metric("reachable_band_area_mm2", ml.reachable_band_area)
        .metric("walls_machined", (ml.walls - ml.untouched_walls) as f64)
        .metric("walls", ml.walls as f64)
        .decide(
            ml.check.pass,
            format!(
                "{:.3} mm² of wall band was never swept; worst leftover stands {:.4} mm \
                 off its wall ({} of {} walls machined)",
                ml.unswept_area,
                ml.max_standoff,
                ml.walls - ml.untouched_walls,
                ml.walls
            ),
        ),
    );

    // 3. Depth against the stock and the declared allowance.
    let d = &v.depth;
    let depth = Claim::new(
        "job.depth",
        "mm",
        "the job reaches its floor, and goes no deeper than the declared \
         bottom allowance allows",
        Basis::Computed,
        deps,
    )
    .at(d.remaining_under_part, spec.bottom_allowance)
    .metric("deepest_z", d.deepest_z)
    .metric("floor_z", d.floor_z)
    .metric("stock_thickness", spec.stock_thickness)
    .metric("features", d.features as f64)
    .metric("spoilboard_declared", f64::from(u8::from(spec.spoilboard)));
    out.push(if !d.check.pass {
        depth.violated(format!(
            "deepest Z {:.3} against a floor of {:.3}; {:.3} mm left under the part",
            d.deepest_z, d.floor_z, d.remaining_under_part
        ))
    } else if d.remaining_under_part < 0.0 && !spec.spoilboard {
        // The oracle allows a negative allowance; the claim will not certify
        // cutting into a bed nobody declared.
        depth.violated(format!(
            "the job cuts {:.3} mm past the stock underside with no spoilboard declared",
            -d.remaining_under_part
        ))
    } else {
        depth.settled()
    });

    // 4. Rapids.
    out.push(
        Claim::new(
            "job.rapids_safe",
            "mm",
            "no rapid travels in XY below the safe height, and none dives \
             into material",
            Basis::Computed,
            deps,
        )
        .at(v.rapids.worst, opts.safe_rapid_z)
        .metric("violations", v.rapids.violation_count as f64)
        .decide(
            v.rapids.pass,
            format!(
                "{} rapid(s) travel unsafely, worst {:.3} mm",
                v.rapids.violation_count, v.rapids.worst
            ),
        ),
    );

    // 5/6. Tabs: the audit is arithmetic, that they hold is a prediction.
    let t = &v.tabs;
    let min_metal = t
        .observations
        .iter()
        .map(|o| o.metal_width)
        .fold(f64::INFINITY, f64::min);
    let declared = spec.declared_tabs.len();
    if t.observations.is_empty() && declared == 0 {
        for name in ["job.tabs", "job.tabs_hold"] {
            out.push(
                Claim::new(
                    name,
                    "mm",
                    "tabs hold the part until the job is done",
                    if name == "job.tabs" {
                        Basis::Computed
                    } else {
                        Basis::Predicted
                    },
                    deps,
                )
                .unverified(
                    "the job declares no tabs and the moves show none — there is nothing \
                     to audit, which is not the same as tabs that hold",
                ),
            );
        }
    } else {
        let audit = Claim::new(
            "job.tabs",
            "mm",
            "every tab leaves at least the stated metal, and every pass that \
             goes below a tab steps over it",
            Basis::Computed,
            deps,
        )
        .at(min_metal, opts.min_tab_metal)
        .metric("tab_count", t.tab_count as f64)
        .metric("declared_tabs", declared as f64)
        .metric("observations", t.observations.len() as f64)
        .metric("passes_below_tabs", t.passes_below_tabs as f64)
        .decide(
            t.check.pass,
            format!(
                "{} tab problem(s): thinnest tab leaves {:.3} mm against a {:.3} mm minimum, \
                 over {} pass(es) that cut below the tab tops",
                t.check.violation_count, min_metal, opts.min_tab_metal, t.passes_below_tabs
            ),
        );
        let holds = Claim::new(
            "job.tabs_hold",
            "mm",
            "the tabs carry the part through the last pass without it moving",
            Basis::Predicted,
            deps,
        )
        .at(min_metal, opts.min_tab_metal)
        .metric("tab_count", t.tab_count as f64)
        .decide(
            t.check.pass,
            "the tab audit already fails, so nothing is predicted to hold".to_string(),
        );
        out.push(audit);
        out.push(holds);
    }

    // 7. Loose pieces.
    let l = &v.loose;
    let largest = l.pieces.first().map(|p| p.area).unwrap_or(0.0);
    out.push(
        Claim::new(
            "job.loose_pieces",
            "1",
            "the job frees no stock that can move under the cutter",
            Basis::Computed,
            deps,
        )
        .at(l.pieces.len() as f64, 0.0)
        .metric("largest_area_mm2", largest)
        .metric("skin_holds", f64::from(u8::from(l.skin_holds)))
        .decide(
            l.check.pass,
            format!(
                "{} piece(s) come free, largest {:.2} mm²{}",
                l.pieces.len(),
                largest,
                if l.skin_holds {
                    ""
                } else {
                    "; the job breaks through, so nothing holds them"
                }
            ),
        ),
    );

    // 8. Envelope — the vacuous-pass case.
    let e = &v.envelope;
    let env = Claim::new(
        "job.envelope_in_travel",
        "mm",
        "the whole tool sweep sits inside the machine's travel at the stated \
         work offset",
        Basis::Computed,
        deps,
    )
    .metric("work_min_x", e.work_min[0])
    .metric("work_min_y", e.work_min[1])
    .metric("work_min_z", e.work_min[2])
    .metric("work_max_x", e.work_max[0])
    .metric("work_max_y", e.work_max[1])
    .metric("work_max_z", e.work_max[2]);
    out.push(match (spec.work_offset, spec.travel) {
        (None, _) => env.unverified(
            "no work offset: the sweep is known in work coordinates only, so it cannot \
             be placed inside the machine's travel",
        ),
        (_, None) => env
            .unverified("no machine travel limits: there is nothing to compare the sweep against"),
        (Some(_), Some(_)) => env.at(e.check.worst, 0.0).decide(
            e.check.pass,
            format!(
                "the sweep reaches {:.3} mm past a travel limit ({} axis violation(s))",
                e.check.worst, e.check.violation_count
            ),
        ),
    });

    // 9. Plunges.
    out.push(
        Claim::new(
            "job.plunges",
            "mm/min",
            "no straight vertical entry into uncut material above the stated \
             plunge feed",
            Basis::Computed,
            deps,
        )
        .at(v.plunges.worst, opts.max_plunge_feed)
        .metric("violations", v.plunges.violation_count as f64)
        .metric("centre_cutting", f64::from(u8::from(spec.centre_cutting)))
        .decide(
            v.plunges.pass,
            format!(
                "{} plunge(s) enter uncut material, worst {:.0} mm/min",
                v.plunges.violation_count, v.plunges.worst
            ),
        ),
    );

    out
}

/// Claims from a cutter-fit report ([`crate::fit::fit_contour`]).
///
/// `max_standoff` is the number the claim is held to: the 2026-09-17 part
/// kept up to 0.29 mm in 24 inside corners and nothing said so.
pub fn fit_claims(report: &FitReport, subject: &str, max_standoff: f64) -> Vec<Claim> {
    let deps: &[&str] = &[BASIS_OUTLINE, BASIS_TOOL];
    let u = &report.unreachable;
    let mut reachable = Claim::new(
        "fit.reachable",
        "mm",
        "the cutter reaches every corner of the contour to within the stated \
         stand-off",
        Basis::Computed,
        deps,
    )
    .subject(subject)
    .at(u.max_standoff, max_standoff)
    .metric("tool_diameter", report.tool_diameter)
    .metric("corners", u.count as f64)
    .metric("unreachable_area_mm2", u.total_area)
    .metric("largest_corner_area_mm2", u.largest_area)
    .metric("centre_pieces", report.centre_pieces as f64);
    if let Some(n) = report.min_neck {
        reachable = reachable.metric("min_neck_width", n.width);
    }
    if let Some(c) = report.slot_clearance_per_side {
        reachable = reachable.metric("slot_clearance_per_side", c);
    }
    if let Some(d) = report.largest_tool_diameter {
        reachable = reachable.metric("largest_tool_diameter", d);
    }

    let passes = Claim::new(
        "fit.cutter_passes",
        "1",
        "the tool centre region is one whole piece, so the cutter can follow \
         the contour without the path falling apart",
        Basis::Computed,
        deps,
    )
    .subject(subject)
    .at(report.centre_pieces as f64, 1.0)
    .metric("tool_diameter", report.tool_diameter)
    .decide(
        report.fits && report.centre_pieces == 1,
        match report.min_neck {
            Some(n) => format!(
                "a Ø{:.3} cutter does not pass the {:.3} mm neck at ({:.2}, {:.2}); \
                 the centre region falls into {} piece(s){}",
                report.tool_diameter,
                n.width,
                n.at[0],
                n.at[1],
                report.centre_pieces,
                match report.largest_tool_diameter {
                    Some(d) => format!(" — the largest cutter that passes is Ø{d:.3}"),
                    None => String::new(),
                }
            ),
            None => format!(
                "a Ø{:.3} cutter does not fit this contour at all",
                report.tool_diameter
            ),
        },
    );

    let reachable = reachable.decide(
        u.max_standoff <= max_standoff,
        format!(
            "{} corner(s) keep {:.3} mm² of metal; the worst stands {:.4} mm off its wall, \
             past the {:.4} mm allowed{}",
            u.count,
            u.total_area,
            u.max_standoff,
            max_standoff,
            match report.largest_tool_diameter {
                Some(d) => format!(" (a Ø{d:.3} cutter would pass every neck)"),
                None => String::new(),
            }
        ),
    );

    vec![passes, reachable]
}

/// Claim that a contour describes the solid it was taken from
/// ([`crate::outline::compare_outlines`]).
pub fn outline_claims(diff: &OutlineDiff, subject: &str, tolerance: f64) -> Vec<Claim> {
    let deps: &[&str] = &[BASIS_OUTLINE, BASIS_SOLID];
    let unmatched = diff.unmatched_a.len() + diff.unmatched_b.len();
    let c = Claim::new(
        "outline.matches_solid",
        "mm",
        "the contour the job cuts is the same shape as the solid it came from",
        Basis::Computed,
        deps,
    )
    .subject(subject)
    .at(diff.max_boundary_distance, tolerance)
    .metric(
        "symmetric_difference_area_mm2",
        diff.symmetric_difference_area,
    )
    .metric("intersection_area_mm2", diff.intersection_area)
    .metric("holes_matched", diff.holes.len() as f64)
    .metric("holes_solid", diff.hole_count_a as f64)
    .metric("holes_contour", diff.hole_count_b as f64)
    .metric("holes_unmatched", unmatched as f64);
    vec![c.decide(
        diff.max_boundary_distance <= tolerance && unmatched == 0,
        format!(
            "the contour differs from the solid by {:.4} mm at worst ({:.3} mm² of \
             symmetric difference) and {} hole(s) have no partner",
            diff.max_boundary_distance, diff.symmetric_difference_area, unmatched
        ),
    )]
}

/// Claim that the part is prismatic, so a contour job describes it at all
/// ([`crate::outline::is_prismatic`]).
pub fn prismatic_claims(report: &PrismaticReport, subject: &str) -> Vec<Claim> {
    let c = Claim::new(
        "outline.prismatic",
        "mm",
        "the part's section does not change with depth, so a 2.5D contour job \
         is a faithful description of it",
        Basis::Computed,
        &[BASIS_SOLID],
    )
    .subject(subject)
    .at(report.max_boundary_distance, report.tolerance)
    .metric(
        "symmetric_difference_area_mm2",
        report.symmetric_difference_area,
    )
    .metric("worst_z", report.worst_z)
    .metric("reference_z", report.reference_z)
    .metric("levels", report.levels.len() as f64);
    vec![c.decide(
        report.prismatic,
        format!(
            "the section at z = {:.3} differs from the reference by {:.4} mm \
             ({:.3} mm² of symmetric difference), past the {:.4} mm tolerance",
            report.worst_z,
            report.max_boundary_distance,
            report.symmetric_difference_area,
            report.tolerance
        ),
    )]
}

/// Claims from a gear report ([`crate::gear::GearReport`]).
///
/// Three claims, and they sit on different rungs on purpose:
///
/// * `gear.contact_ratio` and `gear.reachable` are geometry — the design
///   either meshes and the cutter either leaves an involute flank, or not.
/// * `gear.over_pins` is the **dimension the part will have**. It is
///   `Provisional` until a pin measurement closes it ([`bind`],
///   [`close_over_pins`]), and it carries its own sensitivity so the
///   measurement can be turned back into a cutter offset.
pub fn gear_claims(report: &GearReport, subject: &str) -> Vec<Claim> {
    let deps: &[&str] = &[BASIS_GEAR, BASIS_TOOL];
    let mut out = Vec::with_capacity(3);

    for (i, mesh) in report.meshes.iter().enumerate() {
        let name = if report.meshes.len() == 1 {
            subject.to_string()
        } else {
            format!("{subject}/mesh{i}")
        };
        out.push(
            Claim::new(
                "gear.contact_ratio",
                "1",
                "the mesh keeps at least one tooth pair in contact at all times",
                Basis::Computed,
                deps,
            )
            .subject(&name)
            .at(mesh.contact_ratio, 1.0)
            .metric("path_of_contact", mesh.path_of_contact)
            .metric("centre_distance", mesh.centre_distance)
            .metric("normal_backlash", mesh.normal_backlash)
            .decide(
                mesh.contact_ratio > 1.0,
                format!(
                    "transverse contact ratio {:.4} is not above 1: the mesh loses contact \
                     between tooth pairs",
                    mesh.contact_ratio
                ),
            ),
        );
    }

    let reach = Claim::new(
        "gear.reachable",
        "mm",
        "the cutter's fillet leaves the active flank involute where contact \
         happens, within the stated flank deviation",
        Basis::Computed,
        deps,
    )
    .subject(subject)
    .metric("cutter_diameter", report.cutter_diameter)
    .metric("form_radius", report.form_radius)
    .metric("effective_root_radius", report.effective_root_radius);
    out.push(match report.reachability {
        None => reach.unverified(
            "no contact limit was given, so there is nothing for the fillet to stay clear of",
        ),
        Some(r) => {
            // The strict verdict allows no deviation at all; the graded one
            // states its own tolerance. Either way the claim is held to the
            // metal, not to the radius crossed.
            let tol = r.flank_deviation_tolerance.unwrap_or(0.0);
            reach
                .at(r.flank_deviation_at_contact_limit, tol)
                .metric("radial_overlap", r.radial_overlap)
                .metric("margin", r.margin)
                .metric("contact_limit", r.contact_limit)
                .decide(
                    r.ok,
                    format!(
                        "the fillet leaves {:.6} mm of metal proud of the involute at the \
                         contact limit r = {:.4}, past the {:.6} mm allowed (radial overlap \
                         {:.4} mm)",
                        r.flank_deviation_at_contact_limit, r.contact_limit, tol, r.radial_overlap
                    ),
                )
        }
    });

    let pins = Claim::new(
        "gear.over_pins",
        "mm",
        "the cut gear measures this over pins",
        Basis::Predicted,
        deps,
    )
    .subject(subject);
    out.push(match report.over_pins {
        None => pins.unverified("no pin diameter was chosen, so no measurement is predicted"),
        Some(p) => {
            let mut c = pins
                .metric("pin_diameter", p.pin_diameter)
                .metric("sensitivity", p.sensitivity)
                .metric("contact_radius", p.contact_radius)
                .metric("pitch_tooth_thickness", report.pitch_tooth_thickness)
                .metric("even_teeth", f64::from(u8::from(p.even_teeth)));
            c.value = Some(p.dimension);
            c.settled()
        }
    });

    out
}

/// Claim that the feeds a job will run at are the ones the material asks for
/// ([`crate::materials::check`]).
///
/// `block_at` is the note level that makes the claim fail — [`NoteLevel::Danger`]
/// is the usual choice (a broken cutter or a hazard), [`NoteLevel::Warning`]
/// for a finish pass where a poor cut is a scrapped part.
pub fn feeds_claims(notes: &[Note], block_at: NoteLevel) -> Vec<Claim> {
    let level_of = |l: NoteLevel| match l {
        NoteLevel::Info => 0.0,
        NoteLevel::Caution => 1.0,
        NoteLevel::Warning => 2.0,
        NoteLevel::Danger => 3.0,
    };
    let worst = notes.iter().map(|n| n.level).max();
    let count = |l: NoteLevel| notes.iter().filter(|n| n.level == l).count() as f64;
    let c = Claim::new(
        "feeds.within_recommendation",
        "1",
        "the job's feeds, speed and stepdown are inside what this material, \
         cutter and machine can be run at",
        Basis::Computed,
        &[BASIS_MATERIAL, BASIS_TOOL, BASIS_STOCK],
    )
    .at(worst.map(level_of).unwrap_or(0.0), level_of(block_at))
    .metric("notes", notes.len() as f64)
    .metric("caution", count(NoteLevel::Caution))
    .metric("warning", count(NoteLevel::Warning))
    .metric("danger", count(NoteLevel::Danger));
    let blocked: Vec<&Note> = notes.iter().filter(|n| n.level >= block_at).collect();
    vec![c.decide(
        blocked.is_empty(),
        blocked
            .iter()
            .map(|n| n.text.as_str())
            .collect::<Vec<_>>()
            .join(" | "),
    )]
}

// ---------------------------------------------------------------------------
// Staleness
// ---------------------------------------------------------------------------

/// Re-state a stored claim set against the inputs as they are now.
///
/// Every claim whose basis moved comes back [`ClaimStatus::Stale`], naming
/// the keys that changed. Claims whose basis is intact are returned
/// untouched — re-stating does not re-run the oracles, it only says whether
/// their answers still describe this job.
///
/// A key a claim depends on that the current fingerprint does not carry is a
/// change, not a match: a receipt cannot certify a job it cannot identify.
pub fn restate(set: &ClaimSet, current: &Fingerprint) -> ClaimSet {
    let claims = set
        .claims
        .iter()
        .map(|c| {
            let drifted = current.drifted(&c.depends_on, &set.fingerprint);
            if drifted.is_empty() {
                c.clone()
            } else {
                let mut c = c.clone();
                c.status = ClaimStatus::Stale;
                c.detail = Some(format!(
                    "the claim rests on [{}]; {} changed since it was made",
                    c.depends_on.join(", "),
                    drifted.join(", ")
                ));
                c
            }
        })
        .collect();
    ClaimSet {
        schema: set.schema.clone(),
        provenance: set.provenance.clone(),
        fingerprint: current.clone(),
        claims,
    }
}

// ---------------------------------------------------------------------------
// Measurement binding
// ---------------------------------------------------------------------------

/// What was put on the part.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MeasurementKind {
    /// Calipers or a micrometer over a named feature.
    Caliper {
        /// What was measured, e.g. `"across the flats"`.
        feature: String,
    },
    /// Over (external) or between (internal) pins of the stated diameter.
    OverPins {
        /// Pin or ball diameter, mm.
        pin_diameter: f64,
    },
    /// Base tangent over `teeth` teeth.
    Span {
        /// Teeth spanned.
        teeth: u32,
    },
    /// A bore or hole diameter.
    HoleDiameter,
}

/// A measurement of the real part, bound to the claim it closes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Measurement {
    /// Name of the claim this closes.
    pub claim: String,
    /// Subject of that claim, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// What was measured and how.
    pub kind: MeasurementKind,
    /// The reading, in the claim's unit.
    pub value: f64,
    /// One-sigma uncertainty of the reading, same unit.
    pub uncertainty: f64,
    /// Acceptance half-width on `|measured − predicted|`, same unit. The
    /// claim holds when the difference is inside `tolerance + uncertainty`.
    /// Additive rather than multiplicative because these are dimensions with
    /// a print tolerance, not order-of-magnitude predictions.
    pub tolerance: f64,
    /// Instrument provenance: `"Mitutoyo 293-340 s/n …"`, `"Ø1.5 gauge pins"`.
    pub instrument: String,
}

impl Measurement {
    /// A measurement over pins.
    pub fn over_pins(
        claim: impl Into<String>,
        subject: Option<&str>,
        pin_diameter: f64,
        value: f64,
        tolerance: f64,
        instrument: impl Into<String>,
    ) -> Self {
        Self {
            claim: claim.into(),
            subject: subject.map(str::to_string),
            kind: MeasurementKind::OverPins { pin_diameter },
            value,
            uncertainty: 0.0,
            tolerance,
            instrument: instrument.into(),
        }
    }

    fn valid(&self) -> bool {
        self.value.is_finite()
            && self.uncertainty.is_finite()
            && self.uncertainty >= 0.0
            && self.tolerance.is_finite()
            && self.tolerance >= 0.0
    }
}

/// Bind measurements to a claim set, fail-closed.
///
/// Each measurement must name exactly one claim, and that claim must be on
/// the [`Basis::Predicted`] rung — a measurement of an arithmetic check is a
/// bookkeeping error, not evidence, and is refused rather than quietly
/// ignored. A bound claim comes back on [`Basis::Measured`], `Holds` or
/// `Violated` by whether the reading is inside `tolerance + uncertainty` of
/// the prediction. Claims nobody measured keep their `Provisional` status:
/// the set as a whole cannot read clean until every prediction is closed.
pub fn bind(set: &ClaimSet, measurements: &[Measurement]) -> Result<ClaimSet, ClaimError> {
    for m in measurements {
        let c = set
            .claims
            .iter()
            .find(|c| c.name == m.claim && c.subject.as_deref() == m.subject.as_deref())
            .ok_or_else(|| ClaimError::NoSuchClaim(m.claim.clone()))?;
        if c.basis == Basis::Computed {
            return Err(ClaimError::NotPredicted(m.claim.clone()));
        }
        if !m.valid() {
            return Err(ClaimError::BadMeasurement(m.claim.clone()));
        }
        if c.value.is_none() {
            return Err(ClaimError::NothingToCompare(m.claim.clone()));
        }
    }

    let claims = set
        .claims
        .iter()
        .map(|c| {
            let Some(m) = measurements
                .iter()
                .find(|m| m.claim == c.name && m.subject.as_deref() == c.subject.as_deref())
            else {
                return c.clone();
            };
            let predicted = c.value.expect("checked above");
            let band = m.tolerance + m.uncertainty;
            let delta = m.value - predicted;
            let mut out = c.clone();
            out.basis = Basis::Measured;
            out.measured = Some(m.value);
            out.limit = Some(band);
            out.measurement = Some(m.clone());
            out.metrics.insert("predicted".to_string(), predicted);
            out.metrics.insert("delta".to_string(), delta);
            if delta.abs() <= band {
                out.status = ClaimStatus::Holds;
                out.detail = None;
            } else {
                out.status = ClaimStatus::Violated;
                out.detail = Some(format!(
                    "measured {:.5} against a predicted {:.5}: off by {:+.5}, past the \
                     {:.5} the measurement allows ({})",
                    m.value, predicted, delta, band, m.instrument
                ));
            }
            out
        })
        .collect();

    Ok(ClaimSet {
        schema: set.schema.clone(),
        provenance: set.provenance.clone(),
        fingerprint: set.fingerprint.clone(),
        claims,
    })
}

/// A gear's over-pins claim, closed by a measurement and re-opened by the
/// compensated cut that supersedes it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GearClosure {
    /// What to change on the next cut.
    pub compensation: Compensation,
    /// `gear.over_pins`, now on [`Basis::Measured`].
    pub closed: Claim,
    /// `gear.over_pins.compensated`: the same dimension claimed again, for a
    /// job that carries the compensation. Predicted, so `Provisional` — a
    /// compensation is a plan, and the second part has to be measured too.
    pub corrected: Claim,
}

/// Turn a measured over-pins dimension into a tooth-thickness error, the
/// cutter offset that corrects it, and a claim that supersedes the one the
/// measurement violated.
///
/// This is the measurement loop the first milestone rests on: cut a planet,
/// put pins in it, and get back the number to change rather than a verdict.
/// The arithmetic is [`SpurGear::compensation`]'s — exact algebra on the
/// involute, not a table.
pub fn close_over_pins(
    gear: &SpurGear,
    set: &ClaimSet,
    subject: Option<&str>,
    measurement: &Measurement,
) -> Result<GearClosure, ClaimError> {
    let claim = set
        .find("gear.over_pins", subject)
        .ok_or_else(|| ClaimError::NoSuchClaim("gear.over_pins".to_string()))?;
    let nominal = claim
        .value
        .ok_or_else(|| ClaimError::NothingToCompare("gear.over_pins".to_string()))?;
    let MeasurementKind::OverPins { pin_diameter } = measurement.kind else {
        return Err(ClaimError::BadMeasurement(measurement.claim.clone()));
    };
    // The pins the prediction was made with have to be the pins on the bench.
    if let Some(p) = claim.metrics.get("pin_diameter") {
        if (p - pin_diameter).abs() > 1e-9 {
            return Err(ClaimError::BadMeasurement(format!(
                "{}: predicted over Ø{p} pins, measured over Ø{pin_diameter}",
                measurement.claim
            )));
        }
    }
    let bound = bind(set, std::slice::from_ref(measurement))?;
    let closed = bound
        .find("gear.over_pins", subject)
        .expect("bind preserves claim identity")
        .clone();
    let compensation = gear
        .compensation(pin_diameter, measurement.value, nominal)
        .map_err(|e: GearError| ClaimError::BadMeasurement(e.to_string()))?;

    let mut corrected = Claim::new(
        "gear.over_pins.compensated",
        "mm",
        "the next gear, cut with the compensation applied, measures nominal \
         over the same pins",
        Basis::Predicted,
        &[BASIS_GEAR, BASIS_TOOL, BASIS_PROGRAM],
    )
    .metric("pin_diameter", pin_diameter)
    .metric("thickness_error", compensation.thickness_error)
    .metric("profile_shift_error", compensation.profile_shift_error)
    .metric("tool_normal_offset", compensation.tool_normal_offset)
    .metric("tool_radial_offset", compensation.tool_radial_offset)
    .metric("measured", measurement.value);
    if let Some(s) = subject {
        corrected = corrected.subject(s);
    }
    corrected.value = Some(nominal);
    corrected.supersedes = Some("gear.over_pins".to_string());
    let corrected = corrected.settled();

    Ok(GearClosure {
        compensation,
        closed,
        corrected,
    })
}

// ---------------------------------------------------------------------------
// The unified receipt
// ---------------------------------------------------------------------------

/// The oracle reference for this crate's CAM oracles.
pub fn oracle() -> vcad_receipt::OracleRef {
    vcad_receipt::OracleRef::new("vcad-kernel-cam/verify", env!("CARGO_PKG_VERSION"))
}

fn quantity(value: f64, unit: &str) -> vcad_receipt::ClaimQuantity {
    if unit == "1" {
        vcad_receipt::ClaimQuantity::bare(value)
    } else {
        vcad_receipt::ClaimQuantity::new(value, unit)
    }
}

/// Translate a [`ClaimSet`] into unified-receipt claims.
///
/// The ladder maps onto `vcad.receipt/1` without losing anything:
///
/// | CAM status | receipt verdict | receipt basis | rolls up as |
/// |---|---|---|---|
/// | `Holds`, computed | `Pass` | `Verified` | Pass |
/// | `Holds`, measured | `Pass` | `Measured` | Pass |
/// | `Provisional` | `Pass` | `Predicted` | **Provisional** |
/// | `Violated` | `Fail` | as recorded | Fail |
/// | `Unverified` | `Unverifiable` | — | Unverifiable |
/// | `Stale` | `Unverifiable` | — | Unverifiable |
///
/// The whole [`Claim`] rides in `details` as JSON, so a stored receipt can be
/// re-stated against a changed job without external context — the same trick
/// `mech.clearance.*` uses.
pub fn design_claims(set: &ClaimSet) -> Vec<vcad_receipt::ReceiptClaim> {
    let oracle = oracle();
    set.claims
        .iter()
        .map(|c| {
            let id = format!("cam.{}", c.name);
            let basis = match c.basis {
                Basis::Computed => vcad_receipt::ClaimBasis::Verified,
                Basis::Predicted => vcad_receipt::ClaimBasis::Predicted,
                Basis::Measured => vcad_receipt::ClaimBasis::Measured,
            };
            let mut out = match c.status {
                ClaimStatus::Holds | ClaimStatus::Provisional => {
                    vcad_receipt::ReceiptClaim::pass(id, RECEIPT_DOMAIN, &c.note, oracle.clone())
                        .with_basis(basis)
                }
                ClaimStatus::Violated => {
                    vcad_receipt::ReceiptClaim::fail(id, RECEIPT_DOMAIN, &c.note, oracle.clone())
                        .with_basis(basis)
                }
                ClaimStatus::Unverified | ClaimStatus::Stale => {
                    vcad_receipt::ReceiptClaim::unverifiable(
                        id,
                        RECEIPT_DOMAIN,
                        &c.note,
                        oracle.clone(),
                        c.detail
                            .clone()
                            .unwrap_or_else(|| "the oracle could not check this claim".to_string()),
                    )
                }
            };
            if let Some(s) = &c.subject {
                out = out.with_subject(s);
            }
            if let Some(limit) = c.limit {
                out = out.with_predicted(quantity(limit, &c.unit));
            }
            if let Some(v) = c.measured.or(c.value) {
                out = out.with_measured(quantity(v, &c.unit));
            }
            // Unverified/Stale already carry their reason; for the rest the
            // typed claim is what makes the receipt re-verifiable.
            if !matches!(c.status, ClaimStatus::Unverified | ClaimStatus::Stale) {
                if let Ok(json) = serde_json::to_string(c) {
                    out = out.with_details(json);
                }
            }
            out
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gear::{PlanetaryTrain, SpurGear};
    use crate::verify2d::{verify_gcode, PartRegion, TravelLimits};
    use vcad_receipt::{ClaimBasis, ClaimVerdict, DesignReceipt, ReceiptVerdict};

    // -- fixtures ----------------------------------------------------------

    /// A 20 mm square part at (10,10)–(30,30) in a 40 × 40 × 3 mm blank.
    fn square_part() -> PartRegion {
        PartRegion::new(
            vec![[10.0, 10.0], [30.0, 10.0], [30.0, 30.0], [10.0, 30.0]],
            vec![],
        )
        .unwrap()
    }

    /// An outside contour around that square with a Ø4 cutter: the tool
    /// centre runs 2 mm clear of every wall, three levels down to an onion
    /// skin 0.3 mm above the blank's underside.
    fn square_job_gcode() -> String {
        ring_job_gcode(2.0)
    }

    /// The same job with the tool centre `offset` mm outside the wall. At
    /// 2.0 it just clears; below that the cutter bites into the part.
    fn ring_job_gcode(offset: f64) -> String {
        let (lo, hi) = (10.0 - offset, 30.0 + offset);
        let mut g = String::from("G21\nG90\n");
        for z in [-0.9, -1.8, -2.7] {
            // Retract and re-approach between levels: a continuous helix
            // would read as one pass lifting over the level above it.
            g.push_str("G0 Z5.000\n");
            g.push_str(&format!("G0 X{lo:.3} Y{lo:.3}\n"));
            g.push_str(&format!("G1 Z{z:.3} F100.000\n"));
            for (x, y) in [(hi, lo), (hi, hi), (lo, hi), (lo, lo)] {
                g.push_str(&format!("G1 X{x:.3} Y{y:.3} F500.000\n"));
            }
        }
        g.push_str("G0 Z5.000\n");
        g
    }

    fn square_spec() -> JobSpec {
        let mut s = JobSpec::new(square_part(), 3.0, 4.0);
        s.bottom_allowance = 0.3;
        s.stock_bbox = Some([0.0, 0.0, 40.0, 40.0]);
        s
    }

    fn square_claims(gcode: &str, spec: &JobSpec) -> ClaimSet {
        let opts = VerifyOptions::default();
        let v = verify_gcode(gcode, spec, &opts).unwrap();
        let fp = Fingerprint::new()
            .with_text(BASIS_PROGRAM, gcode)
            .with_loops(BASIS_OUTLINE, std::slice::from_ref(&spec.part.outer))
            .with_json(BASIS_TOOL, &spec.tool_diameter)
            .with_json(BASIS_STOCK, spec);
        ClaimSet::new(
            fp,
            Provenance {
                oracles: vec!["verify2d".into()],
                ..Default::default()
            },
            job_claims(&v, spec, &opts),
        )
    }

    fn get<'a>(set: &'a ClaimSet, name: &str) -> &'a Claim {
        set.find(name, None)
            .unwrap_or_else(|| panic!("missing claim {name}"))
    }

    /// Module 1.0, 20 teeth, no shift, tip 10.89 — the planet of
    /// `docs/cam-fixtures/gears-60-cnc.json`.
    fn planet() -> SpurGear {
        SpurGear::external(1.0, 20)
            .with_tip_radius(10.89)
            .with_face_width(5.0)
    }

    /// The planet against a Ø1 cutter, graded at 5 µm of flank metal against
    /// the sun-side contact limit from `gears-60-cnc.json`
    /// (`r_contact_p_sun`), with the Ø1.5 pin the milestone measures over.
    fn planet_report() -> GearReport {
        GearReport::new(&planet(), 1.0)
            .unwrap()
            .with_contact_tolerance(&planet(), 9.577932062278455, 0.005)
            .unwrap()
            .with_pin(&planet(), 1.5)
            .unwrap()
    }

    // -- job claims --------------------------------------------------------

    /// A job that does what it says: the geometric checks hold, and the two
    /// that have nothing to check say so rather than passing vacuously.
    #[test]
    fn a_clean_job_holds_and_the_unknowable_parts_do_not() {
        let spec = square_spec();
        let set = square_claims(&square_job_gcode(), &spec);

        for name in [
            "job.no_gouge",
            "job.material_left",
            "job.depth",
            "job.rapids_safe",
            "job.loose_pieces",
            "job.plunges",
        ] {
            let c = get(&set, name);
            assert_eq!(c.status, ClaimStatus::Holds, "{name}: {c:?}");
            assert_eq!(c.basis, Basis::Computed);
        }
        // The cutter never entered the part, and the walls were all swept.
        assert!(get(&set, "job.no_gouge").value.unwrap() <= 0.0);
        // One closed wall (the square's outer loop), and the passes reached it.
        assert_eq!(get(&set, "job.material_left").metrics["walls"], 1.0);
        assert_eq!(
            get(&set, "job.material_left").metrics["walls_machined"],
            1.0
        );
        assert_eq!(
            get(&set, "job.material_left").metrics["unswept_area_mm2"],
            0.0
        );
        // 0.3 mm of skin under the part, exactly as declared.
        let d = get(&set, "job.depth");
        assert!((d.value.unwrap() - 0.3).abs() < 1e-6, "{:?}", d.value);
        assert!((d.metrics["deepest_z"] + 2.7).abs() < 1e-6);
        // …so nothing can come free.
        assert_eq!(get(&set, "job.loose_pieces").metrics["skin_holds"], 1.0);

        // No machine, no tabs: unverified, and that poisons the rollup.
        assert_eq!(
            get(&set, "job.envelope_in_travel").status,
            ClaimStatus::Unverified
        );
        assert_eq!(get(&set, "job.tabs").status, ClaimStatus::Unverified);
        assert_eq!(get(&set, "job.tabs_hold").status, ClaimStatus::Unverified);
        assert_eq!(set.status(), ClaimStatus::Unverified);
        assert!(!set.all_hold());
    }

    /// Friction-log item 32, in miniature: the contour ran on the wrong side
    /// and ate 2 mm of wall. The claim is `Violated` and says how much.
    #[test]
    fn a_gouging_job_violates_the_gouge_claim_with_the_depth() {
        let spec = square_spec();
        let set = square_claims(&ring_job_gcode(0.0), &spec);
        let g = get(&set, "job.no_gouge");
        assert_eq!(g.status, ClaimStatus::Violated);
        assert!(g.metrics["violations"] > 0.0);
        // Tool centre on the wall with a Ø4 cutter: 2 mm of metal gone.
        assert!(
            (g.value.unwrap() - 2.0).abs() < 0.05,
            "worst intrusion {:?}",
            g.value
        );
        assert!(g.detail.as_deref().unwrap().contains("enter the part"));
        assert_eq!(set.status(), ClaimStatus::Violated);

        // …and it fails the unified receipt, not merely "does not pass".
        let receipt = DesignReceipt::with_claims(design_claims(&set));
        assert_eq!(receipt.verdict(), ReceiptVerdict::Fail);
    }

    /// The vacuous pass the 2D oracle cannot avoid: with no work offset and
    /// no travel limits its envelope check passes because there is nothing
    /// to compare against. The claim refuses to call that a fit.
    #[test]
    fn envelope_without_a_machine_is_unverified_not_passed() {
        let spec = square_spec();
        let opts = VerifyOptions::default();
        let v = verify_gcode(&square_job_gcode(), &spec, &opts).unwrap();
        assert!(v.envelope.check.pass, "the 2D oracle passes it vacuously");

        let set = square_claims(&square_job_gcode(), &spec);
        let e = get(&set, "job.envelope_in_travel");
        assert_eq!(e.status, ClaimStatus::Unverified);
        assert!(e.detail.as_deref().unwrap().contains("work offset"));
        // The sweep is still on the record, it is just not a verdict.
        assert!((e.metrics["work_max_x"] - 34.0).abs() < 1e-9);

        // Given a machine, it becomes a real verdict — both ways.
        let mut fits = square_spec();
        fits.work_offset = Some([100.0, 100.0, -20.0]);
        fits.travel = Some(TravelLimits {
            min: [0.0, 0.0, -100.0],
            max: [300.0, 300.0, 0.0],
        });
        let set = square_claims(&square_job_gcode(), &fits);
        assert_eq!(
            get(&set, "job.envelope_in_travel").status,
            ClaimStatus::Holds
        );

        let mut tiny = fits.clone();
        tiny.travel = Some(TravelLimits {
            min: [0.0, 0.0, -100.0],
            max: [120.0, 300.0, 0.0],
        });
        let set = square_claims(&square_job_gcode(), &tiny);
        let e = get(&set, "job.envelope_in_travel");
        assert_eq!(e.status, ClaimStatus::Violated);
        assert!(e.value.unwrap() > 0.0);
    }

    /// A job that breaks through into a bed nobody declared is `Violated`
    /// even though the 2D depth check allows a negative allowance.
    #[test]
    fn breaking_through_with_no_spoilboard_is_violated() {
        let mut spec = square_spec();
        spec.bottom_allowance = -0.2;
        spec.spoilboard = false;
        let gcode = square_job_gcode().replace("Z-2.700", "Z-3.200");
        let set = square_claims(&gcode, &spec);
        let d = get(&set, "job.depth");
        assert_eq!(d.status, ClaimStatus::Violated);
        assert!(d.detail.as_deref().unwrap().contains("spoilboard"));

        spec.spoilboard = true;
        let set = square_claims(&gcode, &spec);
        assert_eq!(get(&set, "job.depth").status, ClaimStatus::Holds);
    }

    // -- staleness ---------------------------------------------------------

    /// Change one line of the program and every claim that rests on it goes
    /// `Stale` — the claims about the contour and the tool do not.
    #[test]
    fn a_changed_program_turns_its_claims_stale() {
        let spec = square_spec();
        let gcode = square_job_gcode();
        let mut set = square_claims(&gcode, &spec);
        // Bolt a contour-only claim onto the set so the blast radius is
        // visible: it must survive a program edit untouched.
        let fit = crate::fit::fit_contour(
            &[[10.0, 10.0], [30.0, 10.0], [30.0, 30.0], [10.0, 30.0]],
            4.0,
            crate::fit::ContourSide::Outside,
            &crate::fit::FitOptions::default(),
        )
        .unwrap();
        set.claims.extend(fit_claims(&fit, "part", 0.05));

        let edited = gcode.replace("F500.000", "F900.000");
        assert_ne!(edited, gcode);
        let now = Fingerprint::new()
            .with_text(BASIS_PROGRAM, &edited)
            .with_loops(BASIS_OUTLINE, std::slice::from_ref(&spec.part.outer))
            .with_json(BASIS_TOOL, &spec.tool_diameter)
            .with_json(BASIS_STOCK, &spec);

        let restated = restate(&set, &now);
        for c in &restated.claims {
            if c.depends_on.iter().any(|k| k == BASIS_PROGRAM) {
                assert_eq!(c.status, ClaimStatus::Stale, "{}", c.name);
                assert!(c.detail.as_deref().unwrap().contains(BASIS_PROGRAM));
            } else {
                assert_ne!(c.status, ClaimStatus::Stale, "{}", c.name);
            }
        }
        assert_eq!(restated.status(), ClaimStatus::Stale);
        // Stale is unverifiable on the unified receipt, never a pass.
        let receipt = DesignReceipt::with_claims(design_claims(&restated));
        assert_eq!(receipt.overall(), ClaimVerdict::Unverifiable);

        // Re-hashing the unchanged job leaves everything alone.
        let same = restate(&set, &set.fingerprint.clone());
        assert_eq!(same.claims, set.claims);
    }

    /// A fingerprint that does not carry a key a claim depends on is a
    /// change, not a match: a receipt cannot certify what it cannot name.
    #[test]
    fn a_missing_basis_key_is_stale_not_intact() {
        let spec = square_spec();
        let set = square_claims(&square_job_gcode(), &spec);
        let blank = Fingerprint::new();
        let restated = restate(&set, &blank);
        assert!(restated
            .claims
            .iter()
            .all(|c| c.status == ClaimStatus::Stale));
    }

    // -- the predicted rung ------------------------------------------------

    /// The rule the whole family turns on: a claim about a dimension of the
    /// physical part is `Provisional` on its own, and the unified receipt
    /// built from it rolls up `Provisional`, never `Pass`.
    #[test]
    fn a_predicted_dimension_is_never_a_pass_without_a_measurement() {
        let report = planet_report();
        let set = ClaimSet::new(
            Fingerprint::new().with_json(BASIS_GEAR, &planet()),
            Provenance::default(),
            gear_claims(&report, "planet-20T"),
        );
        let pins = set.find("gear.over_pins", Some("planet-20T")).unwrap();
        assert_eq!(pins.basis, Basis::Predicted);
        assert_eq!(pins.status, ClaimStatus::Provisional);
        assert!(pins.value.unwrap() > 0.0);
        assert!(pins.measured.is_none());

        // Every claim in the set passes its own arithmetic…
        assert!(set
            .claims
            .iter()
            .all(|c| matches!(c.status, ClaimStatus::Holds | ClaimStatus::Provisional)));
        // …and the receipt still refuses to read verified.
        let receipt = DesignReceipt::with_claims(design_claims(&set));
        assert_eq!(receipt.overall(), ClaimVerdict::Pass);
        assert_eq!(
            receipt.verdict(),
            ReceiptVerdict::Provisional,
            "a predicted dimension must never certify a part"
        );
        assert_eq!(receipt.summary().predicted_basis, 1);
        assert!(!set.all_hold());
    }

    /// Geometry claims on the same gear are computed, not predicted: they
    /// hold or they do not, and no measurement is owed.
    #[test]
    fn gear_geometry_claims_are_computed_and_graded_by_flank_metal() {
        let train = PlanetaryTrain::new(
            SpurGear::external(1.0, 10)
                .with_profile_shift(0.47)
                .with_tip_radius(6.45)
                .with_face_width(5.6),
            planet(),
            SpurGear::internal(1.0, 50)
                .with_profile_shift(0.47)
                .with_face_width(6.0),
            3,
        );
        // Strict: the ring's fillet crosses the contact limit.
        let strict = train.reports(1.0).unwrap();
        let ring = gear_claims(&strict[2], "ring-50T");
        let reach = ring.iter().find(|c| c.name == "gear.reachable").unwrap();
        assert_eq!(reach.basis, Basis::Computed);
        assert_eq!(reach.status, ClaimStatus::Violated);
        // Graded at 5 µm of flank metal, the same geometry passes.
        let graded = train.reports_within(1.0, 0.005).unwrap();
        let ring = gear_claims(&graded[2], "ring-50T");
        let reach = ring.iter().find(|c| c.name == "gear.reachable").unwrap();
        assert_eq!(reach.status, ClaimStatus::Holds);
        assert!(
            (reach.value.unwrap() - 0.0013557).abs() < 1e-6,
            "flank deviation {:?}",
            reach.value
        );
        assert_eq!(reach.limit, Some(0.005));
        // Both meshes the planet takes part in get their own contact ratio.
        let planet_claims = gear_claims(&graded[1], "planet-20T");
        let ratios: Vec<&Claim> = planet_claims
            .iter()
            .filter(|c| c.name == "gear.contact_ratio")
            .collect();
        assert_eq!(ratios.len(), 2);
        assert!(ratios.iter().all(|c| c.status == ClaimStatus::Holds));
        assert!((ratios[0].value.unwrap() - 1.2365192525876858).abs() < 1e-9);
    }

    /// Without a contact limit there is no reachability verdict, and the
    /// claim says so instead of inventing one.
    #[test]
    fn a_gear_with_no_contact_limit_is_unverified() {
        let bare = GearReport::new(&planet(), 1.0).unwrap();
        let set = gear_claims(&bare, "planet-20T");
        let reach = set.iter().find(|c| c.name == "gear.reachable").unwrap();
        assert_eq!(reach.status, ClaimStatus::Unverified);
        let pins = set.iter().find(|c| c.name == "gear.over_pins").unwrap();
        assert_eq!(pins.status, ClaimStatus::Unverified);
    }

    // -- measurement binding -----------------------------------------------

    fn gear_set() -> ClaimSet {
        ClaimSet::new(
            Fingerprint::new().with_json(BASIS_GEAR, &planet()),
            Provenance::default(),
            gear_claims(&planet_report(), "planet-20T"),
        )
    }

    /// A measurement inside its band closes the claim; outside it, the claim
    /// is `Violated` and the receipt fails.
    #[test]
    fn a_measurement_closes_a_prediction_or_breaks_it() {
        let set = gear_set();
        let nominal = set
            .find("gear.over_pins", Some("planet-20T"))
            .unwrap()
            .value
            .unwrap();

        let good = Measurement::over_pins(
            "gear.over_pins",
            Some("planet-20T"),
            1.5,
            nominal + 0.004,
            0.01,
            "Ø1.5 gauge pins, Mitutoyo 293-340",
        );
        let bound = bind(&set, std::slice::from_ref(&good)).unwrap();
        let c = bound.find("gear.over_pins", Some("planet-20T")).unwrap();
        assert_eq!(c.status, ClaimStatus::Holds);
        assert_eq!(c.basis, Basis::Measured);
        assert_eq!(c.measured, Some(nominal + 0.004));
        assert!((c.metrics["delta"] - 0.004).abs() < 1e-12);
        assert!(c.measurement.is_some());
        // Measured evidence certifies: the receipt reads Pass, not Provisional.
        let receipt = DesignReceipt::with_claims(design_claims(&bound));
        assert_eq!(receipt.verdict(), ReceiptVerdict::Pass);
        assert!(bound.all_hold());
        let bound_claim = receipt
            .claims
            .iter()
            .find(|c| c.id == "cam.gear.over_pins")
            .unwrap();
        assert_eq!(bound_claim.basis, Some(ClaimBasis::Measured));

        let bad = Measurement::over_pins(
            "gear.over_pins",
            Some("planet-20T"),
            1.5,
            nominal + 0.08,
            0.01,
            "Ø1.5 gauge pins",
        );
        let bound = bind(&set, std::slice::from_ref(&bad)).unwrap();
        let c = bound.find("gear.over_pins", Some("planet-20T")).unwrap();
        assert_eq!(c.status, ClaimStatus::Violated);
        assert!(c.detail.as_deref().unwrap().contains("past the"));
        assert_eq!(
            DesignReceipt::with_claims(design_claims(&bound)).verdict(),
            ReceiptVerdict::Fail
        );
    }

    /// Fail-closed bookkeeping: a measurement of nothing, of arithmetic, or
    /// with unusable numbers is an error, never a quiet pass.
    #[test]
    fn binding_refuses_what_it_cannot_honestly_close() {
        let set = gear_set();
        let nowhere =
            Measurement::over_pins("gear.backlash", Some("planet-20T"), 1.5, 10.0, 0.01, "pins");
        assert_eq!(
            bind(&set, &[nowhere]),
            Err(ClaimError::NoSuchClaim("gear.backlash".into()))
        );

        let job = square_claims(&square_job_gcode(), &square_spec());
        let arithmetic = Measurement {
            claim: "job.no_gouge".into(),
            subject: None,
            kind: MeasurementKind::Caliper {
                feature: "wall".into(),
            },
            value: 0.0,
            uncertainty: 0.0,
            tolerance: 0.01,
            instrument: "calipers".into(),
        };
        assert_eq!(
            bind(&job, &[arithmetic]),
            Err(ClaimError::NotPredicted("job.no_gouge".into()))
        );

        let mut nan = Measurement::over_pins(
            "gear.over_pins",
            Some("planet-20T"),
            1.5,
            f64::NAN,
            0.01,
            "pins",
        );
        assert!(bind(&set, std::slice::from_ref(&nan)).is_err());
        nan.value = 10.0;
        nan.tolerance = -1.0;
        assert!(bind(&set, &[nan]).is_err());
    }

    /// The milestone loop: measure a planet over Ø1.5 pins, get back the
    /// thickness error and the cutter offset that corrects it, and a claim
    /// for the compensated job that supersedes the one the measurement broke.
    ///
    /// The numbers come out of `SpurGear::compensation`, which is exact
    /// algebra on the involute — the test states them so a regression in
    /// that algebra is caught here too.
    #[test]
    fn a_measured_gear_becomes_a_compensation_and_a_superseding_claim() {
        let gear = planet();
        let set = gear_set();
        let nominal = set
            .find("gear.over_pins", Some("planet-20T"))
            .unwrap()
            .value
            .unwrap();
        // Sanity: the claim's nominal is the gear's own prediction.
        assert!((nominal - gear.over_pins(1.5).unwrap().dimension).abs() < 1e-12);

        let m = Measurement::over_pins(
            "gear.over_pins",
            Some("planet-20T"),
            1.5,
            nominal + 0.02,
            0.005,
            "Ø1.5 gauge pins",
        );
        let closure = close_over_pins(&gear, &set, Some("planet-20T"), &m).unwrap();

        // 0.02 mm over Ø1.5 pins is 7.49 µm of tooth thickness at the pitch
        // circle. (The linearised reading — ΔM over the claim's own
        // sensitivity, 2.6789 — is 7.47 µm; `compensation` inverts the
        // involute algebra exactly instead, and the two differ by 0.4 %.)
        assert!(
            (closure.compensation.thickness_error - 0.00749228).abs() < 1e-7,
            "thickness error {}",
            closure.compensation.thickness_error
        );
        let linearised = 0.02 / planet_report().over_pins.unwrap().sensitivity;
        assert!(
            (closure.compensation.thickness_error - linearised).abs() < 0.05 * linearised,
            "the exact and linearised readings agree to a few percent"
        );
        // …and 3.52 µm of cutter offset, into the material: a flank moved n
        // along its normal changes the circumferential thickness by 2n/cos α.
        assert!(
            (closure.compensation.tool_normal_offset + 0.00352022).abs() < 1e-7,
            "tool normal offset {}",
            closure.compensation.tool_normal_offset
        );
        assert!(
            (closure.compensation.tool_normal_offset
                + closure.compensation.thickness_error * planet().pressure_angle.cos() / 2.0)
                .abs()
                < 1e-15
        );
        assert!(
            closure.compensation.tool_normal_offset < 0.0,
            "teeth too thick: cut deeper"
        );

        // The measurement broke the claim (0.02 > the 0.005 band)…
        assert_eq!(closure.closed.status, ClaimStatus::Violated);
        assert_eq!(closure.closed.basis, Basis::Measured);
        assert_eq!(closure.closed.measured, Some(nominal + 0.02));
        // …and the compensated job claims the nominal again, Provisionally.
        assert_eq!(closure.corrected.status, ClaimStatus::Provisional);
        assert_eq!(closure.corrected.basis, Basis::Predicted);
        assert_eq!(
            closure.corrected.supersedes.as_deref(),
            Some("gear.over_pins")
        );
        assert!((closure.corrected.value.unwrap() - nominal).abs() < 1e-12);
        assert!(
            (closure.corrected.metrics["tool_normal_offset"]
                - closure.compensation.tool_normal_offset)
                .abs()
                < 1e-15
        );

        // Applying the compensation is a real fix: a gear thinned by the
        // measured error measures nominal over the same pins again.
        let thick = gear
            .tooth_thickness_from_over_pins(1.5, nominal + 0.02)
            .unwrap();
        let want = gear.pitch_tooth_thickness();
        assert!(
            (thick - want - closure.compensation.thickness_error).abs() < 1e-12,
            "the compensation is exactly the thickness error"
        );

        // Wrong pins are a bookkeeping error, not a silent conversion.
        let wrong = Measurement::over_pins(
            "gear.over_pins",
            Some("planet-20T"),
            2.0,
            nominal + 0.02,
            0.005,
            "Ø2 pins",
        );
        assert!(close_over_pins(&gear, &set, Some("planet-20T"), &wrong).is_err());
    }

    // -- the other oracles -------------------------------------------------

    /// The stator, against the cutter that actually ran and against the one
    /// it was drawn for (`docs/native-app-friction-log.md` item 38).
    #[test]
    fn fit_claims_grade_the_stator_against_the_cutter_that_ran() {
        let dxf = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/cam-fixtures/stator-outline.dxf"
        ));
        let outline = crate::outline::read_dxf(dxf).unwrap();
        // The bore-and-slots loop: the biggest hole of the one region.
        let slots = outline.regions[0]
            .holes
            .iter()
            .max_by(|a, b| a.area().total_cmp(&b.area()))
            .unwrap();
        let loop_: Vec<[f64; 2]> = slots.points.iter().map(|p| [p.x, p.y]).collect();
        let opts = crate::fit::FitOptions::default();

        // The cutter it was drawn for reaches everything.
        let drawn_for =
            crate::fit::fit_contour(&loop_, 2.0, crate::fit::ContourSide::Inside, &opts).unwrap();
        for c in &fit_claims(&drawn_for, "bore+slots", 0.05) {
            assert_eq!(c.status, ClaimStatus::Holds, "{c:?}");
            assert_eq!(c.subject.as_deref(), Some("bore+slots"));
        }

        // The Ø3.175 that actually ran fits *through* the 3.87 mm slot mouths
        // — so a "does the tool fit" question answers yes — and still leaves
        // 24 corners of metal, up to 0.29 mm proud. Two claims, because they
        // are two different facts, and only one of them reached the machine.
        let ran =
            crate::fit::fit_contour(&loop_, 3.175, crate::fit::ContourSide::Inside, &opts).unwrap();
        let claims = fit_claims(&ran, "bore+slots", 0.05);
        let passes = claims
            .iter()
            .find(|c| c.name == "fit.cutter_passes")
            .unwrap();
        assert_eq!(passes.status, ClaimStatus::Holds);
        let reach = claims.iter().find(|c| c.name == "fit.reachable").unwrap();
        assert_eq!(reach.status, ClaimStatus::Violated);
        assert_eq!(reach.metrics["corners"], 24.0);
        assert!(
            (reach.value.unwrap() - 0.29237).abs() < 1e-4,
            "stand-off {:?}",
            reach.value
        );
        assert!((reach.metrics["unreachable_area_mm2"] - 10.571).abs() < 1e-2);
        assert!((reach.metrics["min_neck_width"] - 3.8736).abs() < 1e-3);
        assert!(reach.detail.as_deref().unwrap().contains("24 corner"));

        // A cutter wider than the slot mouth cannot follow the contour at
        // all, and the claim names the largest one that can.
        let too_big =
            crate::fit::fit_contour(&loop_, 4.0, crate::fit::ContourSide::Inside, &opts).unwrap();
        let claims = fit_claims(&too_big, "bore+slots", 0.05);
        let passes = claims
            .iter()
            .find(|c| c.name == "fit.cutter_passes")
            .unwrap();
        assert_eq!(passes.status, ClaimStatus::Violated);
        assert!(passes.value.unwrap() > 1.0, "the centre region falls apart");
        assert!(
            passes.detail.as_deref().unwrap().contains("Ø3.874"),
            "{:?}",
            passes.detail
        );
    }

    /// Feeds: a note the material calls dangerous blocks the job; the same
    /// notes below the threshold do not.
    #[test]
    fn feeds_claims_block_at_the_stated_note_level() {
        let notes = vec![
            Note::info("chipload 0.02 mm x 2 flutes x 10000 rpm = 400 mm/min"),
            Note::warning("Ø1 mm cutter: keep the depth of cut under one diameter"),
        ];
        let c = &feeds_claims(&notes, NoteLevel::Danger)[0];
        assert_eq!(c.status, ClaimStatus::Holds);
        assert_eq!(c.metrics["warning"], 1.0);

        let c = &feeds_claims(&notes, NoteLevel::Warning)[0];
        assert_eq!(c.status, ClaimStatus::Violated);
        assert!(c.detail.as_deref().unwrap().contains("Ø1 mm cutter"));

        let danger = vec![Note::danger(
            "copper without lubricant will weld to the flutes",
        )];
        let c = &feeds_claims(&danger, NoteLevel::Danger)[0];
        assert_eq!(c.status, ClaimStatus::Violated);
        assert_eq!(c.value, Some(3.0));
    }

    /// An outline that came from a solid is checked against it, and an
    /// unmatched hole is a violation even when the boundaries agree.
    #[test]
    fn outline_claims_catch_a_missing_hole() {
        let mut diff = OutlineDiff {
            max_boundary_distance: 0.002,
            ..Default::default()
        };
        let c = &outline_claims(&diff, "plate", 0.01)[0];
        assert_eq!(c.status, ClaimStatus::Holds);

        diff.unmatched_a = vec![2];
        diff.hole_count_a = 3;
        diff.hole_count_b = 2;
        let c = &outline_claims(&diff, "plate", 0.01)[0];
        assert_eq!(c.status, ClaimStatus::Violated);
        assert_eq!(c.metrics["holes_unmatched"], 1.0);

        diff.unmatched_a.clear();
        diff.hole_count_a = 2;
        diff.max_boundary_distance = 0.5;
        diff.symmetric_difference_area = 3.1;
        let c = &outline_claims(&diff, "plate", 0.01)[0];
        assert_eq!(c.status, ClaimStatus::Violated);
        assert_eq!(c.value, Some(0.5));
    }

    /// A part that is not prismatic cannot be described by a contour job.
    #[test]
    fn prismatic_claim_follows_the_report() {
        let ok = PrismaticReport {
            prismatic: true,
            tolerance: 0.01,
            reference_z: 1.5,
            levels: vec![0.3, 0.9, 1.5, 2.1, 2.7],
            worst_z: 2.7,
            max_boundary_distance: 0.0008,
            symmetric_difference_area: 0.002,
        };
        assert_eq!(prismatic_claims(&ok, "part")[0].status, ClaimStatus::Holds);
        let tapered = PrismaticReport {
            prismatic: false,
            max_boundary_distance: 0.84,
            symmetric_difference_area: 41.0,
            ..ok
        };
        let c = &prismatic_claims(&tapered, "part")[0];
        assert_eq!(c.status, ClaimStatus::Violated);
        assert!(c.detail.as_deref().unwrap().contains("2.700"));
    }

    // -- the wire ----------------------------------------------------------

    #[test]
    fn claim_set_round_trips_through_json() {
        let mut set = square_claims(&square_job_gcode(), &square_spec());
        set.claims
            .extend(gear_claims(&planet_report(), "planet-20T"));
        set.fingerprint = set.fingerprint.clone().with_json(BASIS_GEAR, &planet());

        let json = serde_json::to_string_pretty(&set).unwrap();
        assert!(json.contains("vcad.cam-claims/1"));
        assert!(json.contains("job.no_gouge"));
        assert!(json.contains("\"Provisional\""));
        assert!(json.contains("\"Unverified\""));
        let back: ClaimSet = serde_json::from_str(&json).unwrap();
        assert_eq!(back.schema, set.schema);
        assert_eq!(back.fingerprint, set.fingerprint);
        assert_eq!(back.claims.len(), set.claims.len());
        for (a, b) in back.claims.iter().zip(&set.claims) {
            assert_eq!(a.name, b.name);
            assert_eq!(a.subject, b.subject);
            assert_eq!(a.status, b.status);
            assert_eq!(a.basis, b.basis);
            assert_eq!(a.depends_on, b.depends_on);
            assert_eq!(a.detail, b.detail);
            // serde_json parses floats on a fast path that is not bit-exact
            // without its `float_roundtrip` feature; the values agree far
            // below any manufacturing tolerance.
            let close = |x: Option<f64>, y: Option<f64>| match (x, y) {
                (Some(x), Some(y)) => (x - y).abs() <= 1e-12 * y.abs().max(1.0),
                (n, m) => n.is_none() && m.is_none(),
            };
            assert!(close(a.value, b.value), "{}: {:?}", a.name, a.value);
            assert!(close(a.limit, b.limit), "{}", a.name);
            assert_eq!(
                a.metrics.keys().collect::<Vec<_>>(),
                b.metrics.keys().collect::<Vec<_>>()
            );
            for (k, v) in &a.metrics {
                let w = b.metrics[k];
                assert!((v - w).abs() <= 1e-12 * w.abs().max(1.0), "{}.{k}", a.name);
            }
        }

        // The typed claim rides in the unified receipt's details, so a stored
        // receipt can be re-stated without the original claim set.
        let receipt = design_claims(&set);
        let gouge = receipt.iter().find(|c| c.id == "cam.job.no_gouge").unwrap();
        let stored: Claim = serde_json::from_str(gouge.details.as_deref().unwrap()).unwrap();
        assert_eq!(stored.name, "job.no_gouge");
        assert_eq!(
            stored.depends_on,
            vec![BASIS_PROGRAM, BASIS_OUTLINE, BASIS_TOOL, BASIS_STOCK]
        );
        assert_eq!(gouge.domain, RECEIPT_DOMAIN);
    }

    /// An empty claim set is no evidence, not a clean job.
    #[test]
    fn an_empty_set_is_unverified_not_clean() {
        let set = ClaimSet::new(Fingerprint::new(), Provenance::default(), vec![]);
        assert_eq!(set.status(), ClaimStatus::Unverified);
        assert!(!set.all_hold());
        assert_eq!(
            DesignReceipt::with_claims(design_claims(&set)).verdict(),
            ReceiptVerdict::Unverifiable
        );
    }

    /// Two different programs hash differently and the same program hashes
    /// the same — the whole staleness story rests on this.
    #[test]
    fn fingerprints_detect_change_and_only_change() {
        let a = Fingerprint::new().with_text(BASIS_PROGRAM, "G0 X1\n");
        let b = Fingerprint::new().with_text(BASIS_PROGRAM, "G0 X1\n");
        let c = Fingerprint::new().with_text(BASIS_PROGRAM, "G0 X1.0\n");
        assert_eq!(a, b);
        assert_ne!(a, c);
        // Loops hash by geometry, not by float noise below a nanometre.
        let l1 = vec![vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]]];
        let l2 = vec![vec![[0.0, 0.0], [1.0 + 1e-11, 0.0], [1.0, 1.0]]];
        let l3 = vec![vec![[0.0, 0.0], [1.001, 0.0], [1.0, 1.0]]];
        assert_eq!(
            Fingerprint::new().with_loops(BASIS_OUTLINE, &l1),
            Fingerprint::new().with_loops(BASIS_OUTLINE, &l2)
        );
        assert_ne!(
            Fingerprint::new().with_loops(BASIS_OUTLINE, &l1),
            Fingerprint::new().with_loops(BASIS_OUTLINE, &l3)
        );
    }
}
