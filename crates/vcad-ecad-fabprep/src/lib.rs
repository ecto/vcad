#![warn(missing_docs)]
//! Fab preparation as one reproducible command.
//!
//! Taking a routed board to a manufacturable Gerber package used to be a hand-run
//! sequence of example binaries: certify the remaining connections, calibrate the
//! rules, census the offending nets, strip them, re-route, repeat, prune the dead
//! copper, export. This crate is that sequence, with the thing that made it
//! trustworthy — the DRC delta — promoted from a note in a chat log to the
//! command's receipt.
//!
//! # The receipt is the point
//!
//! On an imported fixture, "zero DRC violations" is not an achievable claim and
//! anyone who reports it is reporting something else. The CM5 board with every
//! trace and via stripped out already scores 980 short/clearance violations
//! against its own land patterns, and the *human production board* scores 16,485
//! under the same rules. So [`FabPrepReport`] never reports one number. It
//! reports the pair — baseline (same board, no routing) and final — and the
//! difference between them, which is the only part the router is answerable for.
//! See [`delta`] for how the difference is taken, per rule.
//!
//! # Rule calibration is opt-in and logged
//!
//! Imported boards often declare global minima that forbid their own via
//! classes. Relaxing a DRC rule to make a board pass is a serious footgun, so
//! calibration never runs unless asked for, only ever relaxes a rule to the
//! point where the board's *own given* geometry stops being illegal, is floored
//! at the tightest process this codebase will name, and records its derivation
//! sentence in the receipt. See [`calibrate`].
//!
//! # Fail closed
//!
//! If the fix loop does not drive route-attributable violations to zero, the run
//! reports `converged: false` with the remaining offenders, and
//! [`package::write_fab_package`] refuses to write fabrication files. The report
//! and the board are still written, so the next run can pick up where this one
//! stopped.
//!
//! ```no_run
//! use vcad_ecad_fabprep::{run_fab_prep, FabPrepOptions};
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let mut pcb: vcad_ir::ecad::Pcb = serde_json::from_str(&std::fs::read_to_string("routed.pcb.json")?)?;
//! let out = run_fab_prep(&mut pcb, &FabPrepOptions { calibrate_rules: true, ..Default::default() });
//! // Refuses (and writes only the receipt) unless `out.report.converged`.
//! # #[cfg(feature = "package")]
//! vcad_ecad_fabprep::package::write_fab_package("out/".as_ref(), &pcb, &out, None)?;
//! # Ok(()) }
//! ```

pub mod calibrate;
pub mod delta;
#[cfg(feature = "package")]
pub mod package;
pub mod render;
pub mod verdict;

#[cfg(test)]
mod test_support;

use std::collections::BTreeSet;

use vcad_ecad_pcb::drc::check_drc;
use vcad_ir::ecad::Pcb;

pub use calibrate::{CalibrationReport, RefusedCalibration, RuleCalibration};
pub use delta::{DeltaMode, DrcDelta, Offender, RuleDelta, ROUTE_FIXABLE};
pub use verdict::{VerdictOptions, VerdictSummary};

/// Knobs for one fab-prep run.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FabPrepOptions {
    /// Derive and apply design-rule calibration from the board's own declared
    /// via classes and pre-existing holes. **Off by default** — a silent rule
    /// relaxation is how a board that cannot be built passes its own DRC.
    pub calibrate_rules: bool,
    /// Run the verdict ladder once before the fix loop, to route or certify
    /// connections the board arrived without.
    pub route_remaining: bool,
    /// Maximum strip-and-re-route rounds. The CM5 board converged in 5.
    pub max_rounds: usize,
    /// Search knobs handed to the verdict ladder.
    pub verdict: VerdictOptions,
    /// Remove copper islands that touch no pad or pour of their net before the
    /// final DRC.
    pub prune_dangling: bool,
    /// Rule names the caller explicitly waives (`MinTraceWidth`, …).
    ///
    /// A waived rule keeps its counts in the receipt and gains an `accepted`
    /// flag; it simply stops blocking the verdict. This exists because real fab
    /// packages ship with real, named exceptions — the CM5 campaign accepted 69
    /// fab-legal 0.08 mm neckdowns on its SI-class nets. The difference between
    /// that and a footgun is whether the exception is written down, so an
    /// unrecognised name here is an error rather than a silent no-op.
    #[serde(default)]
    pub accept_rules: Vec<String>,
}

impl Default for FabPrepOptions {
    fn default() -> Self {
        Self {
            calibrate_rules: false,
            route_remaining: true,
            max_rounds: 8,
            verdict: VerdictOptions::default(),
            prune_dangling: true,
            accept_rules: Vec::new(),
        }
    }
}

/// One iteration of the strip-and-re-route fix loop.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RoundSummary {
    /// 1-based round number.
    pub round: usize,
    /// Route-attributable violations in [`ROUTE_FIXABLE`] rules at the start of
    /// this round — the number the round is trying to reduce.
    pub attributable_before: usize,
    /// Nets stripped and handed back to the router.
    pub offending_nets: Vec<String>,
    /// Traces removed by the strip.
    pub stripped_traces: usize,
    /// Vias removed by the strip.
    pub stripped_vias: usize,
    /// What the re-route concluded.
    pub verdict: VerdictSummary,
}

/// Board-shape facts worth stating next to the verdict.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BoardStats {
    /// Copper layers in the stackup.
    pub copper_layers: usize,
    /// Distinct nets named by pads.
    pub nets: usize,
    /// Pads across all footprints.
    pub pads: usize,
    /// Placed footprints.
    pub footprints: usize,
    /// Trace segments.
    pub traces: usize,
    /// Vias.
    pub vias: usize,
    /// Filled zones.
    pub zones: usize,
}

impl BoardStats {
    /// Measure a board.
    pub fn of(pcb: &Pcb) -> Self {
        let nets: BTreeSet<&str> = pcb
            .footprints
            .iter()
            .flat_map(|f| f.pads.iter())
            .filter_map(|p| p.net.as_deref())
            .filter(|n| !n.is_empty())
            .collect();
        Self {
            copper_layers: pcb
                .stackup
                .layers
                .iter()
                .filter(|l| l.layer.is_copper())
                .count(),
            nets: nets.len(),
            pads: pcb.footprints.iter().map(|f| f.pads.len()).sum(),
            footprints: pcb.footprints.len(),
            traces: pcb.traces.len(),
            vias: pcb.vias.len(),
            zones: pcb.zones.len(),
        }
    }
}

/// Unconnected-net and net-island violations, counted before and after.
///
/// The guard against the one cheat this pipeline structurally has available to
/// it. Every geometric violation the fix loop chases can be made to disappear by
/// deleting the copper that causes it, and a board with no traces at all scores
/// perfectly on clearance, width, hole-to-hole and shorts. Watching connectivity
/// across the run is what turns "the violations went away" back into "the board
/// got better".
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Connectivity {
    /// Unconnected-net + net-island violations on the board as given.
    pub on_arrival: usize,
    /// The same count on the finished board.
    pub on_completion: usize,
}

impl Connectivity {
    /// How much worse the finished board is. Non-zero blocks convergence.
    pub fn regression(&self) -> usize {
        self.on_completion.saturating_sub(self.on_arrival)
    }
}

/// Unconnected-net and net-island violations in a DRC result — "the netlist is
/// not realized", as distinct from "the copper breaks a geometric rule".
fn connectivity_count(violations: &[vcad_ecad_pcb::drc::DrcViolation]) -> usize {
    use vcad_ecad_pcb::drc::DrcRuleType;
    violations
        .iter()
        .filter(|v| {
            matches!(
                v.rule,
                DrcRuleType::UnconnectedNet | DrcRuleType::NetIslands
            )
        })
        .count()
}

/// The receipt: everything a reader needs to audit the run without rerunning it.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FabPrepReport {
    /// True when route-attributable violations in [`ROUTE_FIXABLE`] rules
    /// reached zero. False means fail-closed: no fab package is written.
    pub converged: bool,
    /// Why the loop stopped short, when it did.
    pub blocker: Option<String>,
    /// Whether calibration was asked for at all.
    pub calibration_requested: bool,
    /// What calibration changed and what it refused.
    pub calibration: CalibrationReport,
    /// The up-front verdict pass, when it ran.
    pub initial_verdict: Option<VerdictSummary>,
    /// The fix loop, round by round.
    pub rounds: Vec<RoundSummary>,
    /// Dangling traces removed by the prune.
    pub pruned_traces: usize,
    /// Dangling vias removed by the prune.
    pub pruned_vias: usize,
    /// How much of the netlist was realized on arrival vs on completion.
    pub connectivity: Connectivity,
    /// Rules the caller waived, resolved to canonical names. Their violations
    /// are still counted and still listed — they just do not block the verdict.
    pub accepted_rules: Vec<String>,
    /// Baseline vs final, per rule.
    pub delta: DrcDelta,
    /// The finished board's shape.
    pub board: BoardStats,
}

impl FabPrepReport {
    /// The claim this run supports, in one sentence.
    pub fn headline(&self) -> String {
        if self.converged {
            let waived = match self.delta.route_attributable_accepted {
                0 => String::new(),
                n => format!(", {n} waived under {}", self.accepted_rules.join("+")),
            };
            format!(
                "zero route-attributable violations{waived} ({} on the finished board, {} on the \
                 same board stripped of all routing)",
                self.delta.final_total, self.delta.baseline_total
            )
        } else {
            format!(
                "NOT CONVERGED — {} route-attributable violation(s) remain ({})",
                self.delta.route_attributable_fixable,
                self.blocker.as_deref().unwrap_or("round limit reached")
            )
        }
    }
}

/// What a run produces: the receipt, and the DRC evidence it was derived from.
///
/// The violations travel with the report because a full board DRC is one of the
/// most expensive things this pipeline does — the renderers and the package
/// writer need them, and recomputing would be a silent tax on every caller.
#[derive(Debug, Clone)]
pub struct FabPrepOutcome {
    /// The receipt.
    pub report: FabPrepReport,
    /// Every violation on the finished board, under the rules the report names.
    pub violations: Vec<vcad_ecad_pcb::drc::DrcViolation>,
}

/// A run that refuses to start, expressed as an ordinary non-converging
/// outcome so a caller that already handles "did not converge" needs no second
/// error path — and so the refusal lands in the same receipt everything else
/// does.
fn refused(pcb: &Pcb, opts: &FabPrepOptions, why: String) -> FabPrepOutcome {
    FabPrepOutcome {
        report: FabPrepReport {
            converged: false,
            blocker: Some(why),
            calibration_requested: opts.calibrate_rules,
            calibration: CalibrationReport::default(),
            initial_verdict: None,
            rounds: Vec::new(),
            pruned_traces: 0,
            pruned_vias: 0,
            connectivity: Connectivity::default(),
            accepted_rules: Vec::new(),
            delta: DrcDelta::compute(&[], &[], &BTreeSet::new()),
            board: BoardStats::of(pcb),
        },
        violations: Vec::new(),
    }
}

/// What one call to [`FabPrepSession::round`] concluded — enough for a caller
/// to narrate progress between rounds without holding the receipt yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RoundStatus {
    /// The fix loop is finished (converged or blocked); call
    /// [`FabPrepSession::finish`] next.
    pub done: bool,
    /// 1-based number of the round that just ran, or of the check that ended
    /// the loop.
    pub round: usize,
    /// Route-attributable violations at the start of this round.
    pub attributable: usize,
    /// Total rounds the session may still run, including this one.
    pub max_rounds: usize,
}

/// Stepwise driver for the fab-prep pipeline.
///
/// [`FabPrepSession::begin`] runs waiver validation, calibration, the stripped
/// baseline and the up-front verdict pass; each [`FabPrepSession::round`] runs
/// one strip-and-re-route iteration; [`FabPrepSession::finish`] restores the
/// best board if the loop wandered, prunes, and builds the receipt.
///
/// [`run_fab_prep`] is implemented on top of this driver, so the chunked path
/// and the one-shot path are the same code — a caller that steps the session
/// (to surface progress between rounds) gets a bit-identical outcome.
pub struct FabPrepSession {
    opts: FabPrepOptions,
    accepted: BTreeSet<String>,
    calibration: CalibrationReport,
    baseline: Vec<vcad_ecad_pcb::drc::DrcViolation>,
    entry_connectivity: usize,
    initial_verdict: Option<VerdictSummary>,
    rounds: Vec<RoundSummary>,
    blocker: Option<String>,
    converged: bool,
    previous: Option<(BTreeSet<String>, usize)>,
    best: Option<(usize, Pcb)>,
    non_improving: usize,
    round_index: usize,
    done: bool,
}

/// Consecutive rounds allowed to not improve before the loop gives up. One
/// bad round can be a productive intermediate; two in a row is wandering.
const MAX_NON_IMPROVING: usize = 2;

impl FabPrepSession {
    /// Set up a run: validate waivers, calibrate (opt-in), measure the
    /// stripped-board baseline and arrival connectivity, and route or certify
    /// whatever the board arrived unrouted.
    ///
    /// A refusal (an unrecognised waiver name) comes back as `Err` carrying the
    /// same non-converging [`FabPrepOutcome`] the one-shot path reports.
    pub fn begin(pcb: &mut Pcb, opts: &FabPrepOptions) -> Result<Self, Box<FabPrepOutcome>> {
        // 0. Waivers. A name that matches no rule is refused outright: a typo in a
        //    safety valve that silently does nothing is worse than no valve.
        let known: BTreeSet<String> = delta::ALL_RULES.iter().map(delta::rule_name).collect();
        let (accepted, unknown): (BTreeSet<String>, Vec<String>) = {
            let mut ok = BTreeSet::new();
            let mut bad = Vec::new();
            for r in &opts.accept_rules {
                match known.iter().find(|k| k.eq_ignore_ascii_case(r)) {
                    Some(k) => {
                        ok.insert(k.clone());
                    }
                    None => bad.push(r.clone()),
                }
            }
            (ok, bad)
        };

        if !unknown.is_empty() {
            return Err(Box::new(refused(
                pcb,
                opts,
                format!(
                    "unrecognised rule name(s) in the waiver list: {}. Valid names: {}",
                    unknown.join(", "),
                    known.iter().cloned().collect::<Vec<_>>().join(", ")
                ),
            )));
        }

        // 1. Rule calibration, before anything is measured — the baseline and the
        //    final must be judged under identical rules or the delta is fiction.
        let calibration = if opts.calibrate_rules {
            calibrate::calibrate_rules(pcb)
        } else {
            CalibrationReport::default()
        };

        // 2. Baseline: the same board, same rules, with every trace and via
        //    removed. This is the floor the fixture arrives with.
        let baseline = {
            let mut stripped = pcb.clone();
            stripped.traces.clear();
            stripped.trace_arcs.clear();
            stripped.vias.clear();
            check_drc(&stripped)
        };
        log::info!(
            "fab-prep: fixture baseline = {} violations (stripped board)",
            baseline.len()
        );

        // 2b. Connectivity as the board arrived. The fix loop strips nets to clear
        //     violations, which means it has a trivially "successful" move available
        //     to it: delete the copper and the geometric violation goes with it. The
        //     resulting board is DRC-clean and electrically useless. Recording what
        //     the netlist realized on arrival lets the verdict refuse that trade.
        let entry_connectivity = connectivity_count(&check_drc(pcb));

        // 3. Route or certify whatever the board arrived unrouted.
        let initial_verdict = opts.route_remaining.then(|| {
            let v = verdict::route_remaining(pcb, opts.verdict);
            log::info!(
                "fab-prep: verdict ladder routed {} / proved-infeasible {} / unknown {}",
                v.routed,
                v.proved_infeasible,
                v.unknown
            );
            v
        });

        Ok(Self {
            opts: opts.clone(),
            accepted,
            calibration,
            baseline,
            entry_connectivity,
            initial_verdict,
            rounds: Vec::new(),
            blocker: None,
            converged: false,
            previous: None,
            // A round strips whole nets and hands them back to the router, so a round
            // whose re-route fails leaves the board sparser than it found it. Left
            // unguarded the loop can wander downhill for its whole round budget and
            // return a board materially worse than the one it was given. Keep the best
            // board seen and restore it if the loop never converges: fab-prep may fail
            // to improve a board, but it must never hand back a worse one.
            best: None,
            non_improving: 0,
            round_index: 0,
            done: false,
        })
    }

    /// One iteration of the fix loop: census the route-attributable offenders,
    /// strip their nets, hand them back to the (session-probed) ladder,
    /// re-check. Returns `done: true` once the loop has converged or blocked.
    pub fn round(&mut self, pcb: &mut Pcb) -> RoundStatus {
        let max_rounds = self.opts.max_rounds;
        if self.done || self.round_index > max_rounds {
            self.done = true;
            return RoundStatus {
                done: true,
                round: self.round_index,
                attributable: 0,
                max_rounds,
            };
        }
        let round = self.round_index;
        let status = |done: bool, attributable: usize| RoundStatus {
            done,
            round: round + 1,
            attributable,
            max_rounds,
        };

        let now = check_drc(pcb);
        let regression = connectivity_count(&now).saturating_sub(self.entry_connectivity);
        let delta = DrcDelta::compute(&self.baseline, &now, &self.accepted);
        let attributable = delta.route_attributable_fixable;
        if attributable == 0 && regression == 0 {
            self.converged = true;
            self.best = None;
            self.done = true;
            return status(true, attributable);
        }
        // Score a candidate board on both axes at once. Scoring on
        // `attributable` alone would rank "stripped the net, never re-routed
        // it" as the best board on offer.
        let score = attributable + regression;
        match &self.best {
            Some((n, _)) if *n <= score => self.non_improving += 1,
            _ => {
                self.non_improving = 0;
                self.best = Some((score, pcb.clone()));
            }
        }
        if attributable == 0 {
            // Geometrically clean, but the loop got there by removing copper.
            self.blocker = Some(format!(
                "the board is geometrically clean but {regression} more net(s) are unconnected or \
                 islanded than when it arrived — the loop cleared violations by removing copper it \
                 could not re-route, which is not a fix"
            ));
            self.done = true;
            return status(true, attributable);
        }
        if self.non_improving >= MAX_NON_IMPROVING {
            self.blocker = Some(format!(
                "the fix loop stopped improving — {MAX_NON_IMPROVING} consecutive rounds failed to \
                 beat the best board seen"
            ));
            self.done = true;
            return status(true, attributable);
        }
        if round == max_rounds {
            // Blockers say why the loop STOPPED and never restate the count —
            // `headline()` owns that number, and a blocker carrying a stale
            // count (the prune below can still change it) reads as a
            // contradiction in the receipt.
            self.blocker = Some(format!("the round limit ({max_rounds}) was reached"));
            self.done = true;
            return status(true, attributable);
        }

        // Only nets that actually own copper can be stripped and re-routed. An
        // offender naming no such net is one this loop structurally cannot fix
        // — say so rather than spinning.
        let with_copper = verdict::nets_with_copper(pcb);
        let offending: BTreeSet<String> = delta
            .offenders
            .iter()
            .flat_map(|o| {
                if o.nets.is_empty() {
                    // Hole-to-hole, annular ring and drill violations report a
                    // place, not a net. Read the nets off the copper standing
                    // there — otherwise the loop calls ordinary re-routing work
                    // unreachable.
                    verdict::nets_near(pcb, o.position, o.required.max(0.5))
                        .into_iter()
                        .collect::<Vec<_>>()
                } else {
                    o.nets.clone()
                }
            })
            .filter(|n| with_copper.contains(n))
            .collect();
        if offending.is_empty() {
            self.blocker = Some(
                "the remaining offenders name no net carrying board-level copper — the \
                 strip-and-re-route loop cannot reach them"
                    .to_string(),
            );
            self.done = true;
            return status(true, attributable);
        }
        // Oscillation guard: the same net set failing to reduce the count means
        // the ladder is returning the same copper. Stop and report rather than
        // burning the remaining rounds on it.
        if let Some((prev_nets, prev_count)) = &self.previous {
            if prev_nets == &offending && attributable >= *prev_count {
                self.blocker = Some(format!(
                    "the fix loop stalled: round {round} re-stripped the same {} net(s) without \
                     reducing the count",
                    offending.len()
                ));
                self.done = true;
                return status(true, attributable);
            }
        }
        self.previous = Some((offending.clone(), attributable));

        let (stripped_traces, _arcs, stripped_vias) = verdict::strip_nets(pcb, &offending);
        log::info!(
            "fab-prep round {}: {attributable} attributable, stripped {stripped_traces} traces / \
             {stripped_vias} vias across {} nets",
            round + 1,
            offending.len()
        );
        let verdict = verdict::route_remaining(pcb, self.opts.verdict);
        self.rounds.push(RoundSummary {
            round: round + 1,
            attributable_before: attributable,
            offending_nets: offending.into_iter().collect(),
            stripped_traces,
            stripped_vias,
            verdict,
        });
        self.round_index += 1;
        status(false, attributable)
    }

    /// Restore the best board if the loop wandered, prune dangling copper, run
    /// the final DRC, and build the receipt.
    pub fn finish(self, pcb: &mut Pcb) -> FabPrepOutcome {
        let Self {
            opts,
            accepted,
            calibration,
            baseline,
            entry_connectivity,
            initial_verdict,
            rounds,
            mut blocker,
            converged,
            best,
            ..
        } = self;

        // 4b. The loop ended without converging: hand back the best board it saw
        //     rather than whatever the last (failed) round happened to leave.
        let mut restored_best = false;
        if !converged {
            if let Some((best_score, board)) = best {
                let now = check_drc(pcb);
                let score = DrcDelta::compute(&baseline, &now, &accepted)
                    .route_attributable_fixable
                    + connectivity_count(&now).saturating_sub(entry_connectivity);
                if best_score < score {
                    log::info!(
                        "fab-prep: restoring the best board seen (score {best_score} vs {score})"
                    );
                    *pcb = board;
                    restored_best = true;
                }
            }
        }

        // 5. Prune copper that reaches no pad or pour of its net. Removing copper
        //    can only remove geometric violations, so this cannot invalidate a
        //    convergence reached above — but the final DRC below is what the
        //    receipt reports either way.
        let (pruned_traces, pruned_vias) = if opts.prune_dangling {
            vcad_ecad_pcb::drc::prune_dangling_copper(pcb)
        } else {
            (0, 0)
        };

        // 6. Final DRC and the delta that is the receipt.
        let violations = check_drc(pcb);
        let delta = DrcDelta::compute(&baseline, &violations, &accepted);
        let connectivity = Connectivity {
            on_arrival: entry_connectivity,
            on_completion: connectivity_count(&violations),
        };
        // The prune runs after the loop's last check, so re-derive convergence from
        // the numbers actually being reported rather than trusting the loop's flag.
        let converged =
            converged && delta.route_attributable_fixable == 0 && connectivity.regression() == 0;
        if converged {
            blocker = None;
        } else if blocker.is_none() {
            blocker = Some(if connectivity.regression() > 0 {
                format!(
                    "{} more net(s) are unconnected or islanded than when the board arrived",
                    connectivity.regression()
                )
            } else {
                "violations appeared in the post-prune check".to_string()
            });
        }
        if restored_best {
            if let Some(why) = &mut blocker {
                why.push_str(" (the best board the loop saw was restored)");
            }
        }

        FabPrepOutcome {
            report: FabPrepReport {
                converged,
                blocker,
                calibration_requested: opts.calibrate_rules,
                calibration,
                initial_verdict,
                rounds,
                pruned_traces,
                pruned_vias,
                connectivity,
                accepted_rules: accepted.iter().cloned().collect(),
                delta,
                board: BoardStats::of(pcb),
            },
            violations,
        }
    }
}

/// Run the whole fab-prep pipeline against `pcb`, mutating it in place.
///
/// Never panics on a difficult board: a run that cannot converge returns a
/// report with `converged: false` and the offenders named, and it is the
/// caller's job (see [`package::write_fab_package`]) not to ship it.
///
/// This is the one-shot form of [`FabPrepSession`]; callers that want to
/// observe progress between rounds drive the session directly.
pub fn run_fab_prep(pcb: &mut Pcb, opts: &FabPrepOptions) -> FabPrepOutcome {
    let mut session = match FabPrepSession::begin(pcb, opts) {
        Ok(s) => s,
        Err(outcome) => return *outcome,
    };
    while !session.round(pcb).done {}
    session.finish(pcb)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{test_board, with_smd_pad, with_trace};

    /// A board whose routing is legal: the loop has nothing to do and reports
    /// convergence on round zero.
    #[test]
    fn a_clean_board_converges_without_stripping_anything() {
        let mut pcb = test_board();
        with_smd_pad(&mut pcb, "R1", "1", 2.0, 2.0, "A");
        with_smd_pad(&mut pcb, "R1", "2", 8.0, 2.0, "A");
        with_trace(&mut pcb, "A", 2.0, 2.0, 8.0, 2.0);

        let report = run_fab_prep(
            &mut pcb,
            &FabPrepOptions {
                route_remaining: false,
                prune_dangling: false,
                ..Default::default()
            },
        )
        .report;
        assert!(report.converged, "{:?}", report.blocker);
        assert!(report.rounds.is_empty());
        assert_eq!(report.delta.route_attributable_fixable, 0);
        assert!(report.headline().contains("zero route-attributable"));
    }

    /// Two nets' traces laid across each other: a genuine, route-attributable
    /// short the fixture baseline does not contain. With re-routing disabled
    /// the loop cannot fix it, so it must fail closed — with the offender named
    /// — rather than declaring the board ready.
    #[test]
    fn an_unfixable_route_violation_fails_closed_and_names_the_offender() {
        let mut pcb = test_board();
        with_smd_pad(&mut pcb, "R1", "1", 2.0, 2.0, "A");
        with_smd_pad(&mut pcb, "R1", "2", 8.0, 2.0, "A");
        with_smd_pad(&mut pcb, "R2", "1", 5.0, 0.5, "B");
        with_smd_pad(&mut pcb, "R2", "2", 5.0, 4.0, "B");
        with_trace(&mut pcb, "A", 2.0, 2.0, 8.0, 2.0);
        with_trace(&mut pcb, "B", 5.0, 0.5, 5.0, 4.0);

        let report = run_fab_prep(
            &mut pcb,
            &FabPrepOptions {
                route_remaining: false,
                prune_dangling: false,
                // No rounds: with nothing allowed to strip the crossing pair,
                // the short survives and must be named. (Given a round, the
                // loop strips both nets and the fail-closed commit gate
                // refuses to re-route them into the same 0.3mm-drill
                // congestion — an honest unroutable, tested separately.)
                max_rounds: 0,
                ..Default::default()
            },
        )
        .report;
        assert!(!report.converged, "a crossing short must never pass");
        assert!(report.blocker.is_some());
        assert!(report.delta.route_attributable_fixable > 0);
        let nets: BTreeSet<&str> = report
            .delta
            .offenders
            .iter()
            .flat_map(|o| o.nets.iter().map(|s| s.as_str()))
            .collect();
        assert!(nets.contains("A") && nets.contains("B"), "{nets:?}");
        assert!(report.headline().contains("NOT CONVERGED"));
    }

    /// The loop must actually run — a strippable offender produces a round with
    /// the offending nets recorded.
    #[test]
    fn the_fix_loop_records_the_round_it_ran() {
        let mut pcb = test_board();
        with_smd_pad(&mut pcb, "R1", "1", 2.0, 2.0, "A");
        with_smd_pad(&mut pcb, "R1", "2", 8.0, 2.0, "A");
        with_smd_pad(&mut pcb, "R2", "1", 5.0, 0.5, "B");
        with_smd_pad(&mut pcb, "R2", "2", 5.0, 4.0, "B");
        with_trace(&mut pcb, "A", 2.0, 2.0, 8.0, 2.0);
        with_trace(&mut pcb, "B", 5.0, 0.5, 5.0, 4.0);

        let report = run_fab_prep(
            &mut pcb,
            &FabPrepOptions {
                route_remaining: false,
                prune_dangling: false,
                max_rounds: 2,
                ..Default::default()
            },
        )
        .report;
        assert!(!report.rounds.is_empty(), "the loop must have run a round");
        let r = &report.rounds[0];
        assert_eq!(r.round, 1);
        assert!(r.attributable_before > 0);
        assert!(r.stripped_traces > 0, "the offending nets must be stripped");
        assert!(r.offending_nets.iter().any(|n| n == "A" || n == "B"));
    }

    /// Baseline violations the board arrived with are never charged to the
    /// router, and the receipt shows both numbers so nobody mistakes one for
    /// the other.
    #[test]
    fn fixture_baseline_violations_are_reported_but_not_charged() {
        let mut pcb = test_board();
        // Two pads of different nets touching: a pad-artifact short that
        // survives stripping, exactly like the CM5 fixture's own floor.
        with_smd_pad(&mut pcb, "U1", "1", 3.0, 3.0, "A");
        with_smd_pad(&mut pcb, "U1", "2", 3.05, 3.0, "B");

        let report = run_fab_prep(
            &mut pcb,
            &FabPrepOptions {
                route_remaining: false,
                prune_dangling: false,
                ..Default::default()
            },
        )
        .report;
        assert!(
            report.delta.baseline_total > 0,
            "the fixture must score against itself"
        );
        assert_eq!(report.delta.route_attributable_fixable, 0);
        assert!(report.converged);
        assert!(
            report.headline().contains("stripped of all routing"),
            "the headline must state both numbers: {}",
            report.headline()
        );
    }

    /// A round strips whole nets and can fail to re-route them, which leaves the
    /// board sparser than it started. However badly the loop wanders, the board
    /// it hands back must never be worse than the best one it saw.
    #[test]
    fn a_wandering_loop_never_returns_a_worse_board_than_it_found() {
        let mut pcb = test_board();
        with_smd_pad(&mut pcb, "R1", "1", 2.0, 2.0, "A");
        with_smd_pad(&mut pcb, "R1", "2", 8.0, 2.0, "A");
        with_smd_pad(&mut pcb, "R2", "1", 5.0, 0.5, "B");
        with_smd_pad(&mut pcb, "R2", "2", 5.0, 4.0, "B");
        with_trace(&mut pcb, "A", 2.0, 2.0, 8.0, 2.0);
        with_trace(&mut pcb, "B", 5.0, 0.5, 5.0, 4.0);
        // Copper that is nowhere near the crossing short and has no business
        // being touched by the loop.
        with_smd_pad(&mut pcb, "R3", "1", 1.0, 9.0, "C");
        with_smd_pad(&mut pcb, "R3", "2", 9.0, 9.0, "C");
        with_trace(&mut pcb, "C", 1.0, 9.0, 9.0, 9.0);

        let report = run_fab_prep(
            &mut pcb,
            &FabPrepOptions {
                route_remaining: false,
                prune_dangling: false,
                max_rounds: 6,
                // A budget too small to re-route anything: every round strips
                // and fails, which is exactly the downhill case.
                verdict: VerdictOptions {
                    budget: 1,
                    max_cluster: 6,
                },
                ..Default::default()
            },
        )
        .report;

        assert!(!report.converged, "an unroutable short must fail closed");
        assert!(report.blocker.is_some(), "a failed run must name a blocker");
        // Two acceptable shapes, and the loop may reach either depending on how
        // far it wanders: it restored the best board it saw, so no copper was
        // lost at all (`regression() == 0`), or it did hand back a sparser board
        // and said so. What is never allowed is regressing silently — clearing
        // violations by deleting copper and reporting that as progress.
        assert!(
            report.connectivity.regression() == 0
                || report
                    .blocker
                    .as_deref()
                    .is_some_and(|b| b.contains("unconnected")),
            "clearing violations by deleting copper must be named as the reason, not passed: \
             {:?} / {:?}",
            report.connectivity,
            report.blocker
        );
        assert!(
            pcb.traces.iter().any(|t| t.net == "C"),
            "untouched copper must survive: {:?}",
            pcb.traces.iter().map(|t| &t.net).collect::<Vec<_>>()
        );
        assert!(
            report.delta.route_attributable_fixable
                <= report
                    .rounds
                    .iter()
                    .map(|r| r.attributable_before)
                    .max()
                    .unwrap_or(usize::MAX),
            "the returned board must be no worse than the best round saw"
        );
    }

    /// A waiver lets a run converge past a rule the caller has decided to
    /// accept — but it must never make the violations invisible.
    #[test]
    fn a_waiver_unblocks_the_verdict_without_hiding_the_count() {
        let mut pcb = test_board();
        with_smd_pad(&mut pcb, "R1", "1", 2.0, 2.0, "A");
        with_smd_pad(&mut pcb, "R1", "2", 8.0, 2.0, "A");
        // A trace under the class minimum: a MinTraceWidth violation the loop
        // cannot fix without re-routing.
        pcb.traces.push(vcad_ir::ecad::Trace {
            start: vcad_ir::Vec2::new(2.0, 2.0),
            end: vcad_ir::Vec2::new(8.0, 2.0),
            width: 0.05,
            layer: vcad_ir::ecad::PcbLayer::FCu,
            net: "A".into(),
            source: None,
        });

        let base = FabPrepOptions {
            route_remaining: false,
            prune_dangling: false,
            max_rounds: 0,
            ..Default::default()
        };
        let unwaived = run_fab_prep(&mut pcb.clone(), &base).report;
        assert!(
            !unwaived.converged,
            "the width violation must block by default"
        );

        let report = run_fab_prep(
            &mut pcb,
            &FabPrepOptions {
                accept_rules: vec!["MinTraceWidth".into()],
                ..base
            },
        )
        .report;
        assert!(report.converged, "{:?}", report.blocker);
        assert_eq!(report.accepted_rules, vec!["MinTraceWidth"]);
        assert!(
            report.delta.route_attributable_accepted > 0,
            "the waived violations must still be counted"
        );
        let row = report
            .delta
            .rules
            .iter()
            .find(|r| r.rule == "MinTraceWidth")
            .expect("still in the table");
        assert!(row.accepted && row.route_attributable > 0);
        let notes = render::fab_notes(&report);
        assert!(notes.contains("**With waivers.**"), "{notes}");
    }

    /// A typo in a waiver would silently accept nothing, which is the most
    /// dangerous way for a safety valve to fail. Refuse the run instead.
    #[test]
    fn an_unrecognised_waiver_name_refuses_the_run() {
        let mut pcb = test_board();
        let report = run_fab_prep(
            &mut pcb,
            &FabPrepOptions {
                accept_rules: vec!["MinTraceWidht".into()],
                ..Default::default()
            },
        )
        .report;
        assert!(!report.converged);
        assert!(
            report
                .blocker
                .as_deref()
                .is_some_and(|b| b.contains("MinTraceWidht") && b.contains("MinTraceWidth")),
            "the refusal must name the typo and the valid options: {:?}",
            report.blocker
        );
    }

    /// Driving the session round-by-round (the streamed-progress path) must
    /// produce exactly the outcome of the one-shot call on the same board.
    #[test]
    fn stepwise_session_matches_the_one_shot_run() {
        let mut make = || {
            let mut pcb = test_board();
            with_smd_pad(&mut pcb, "R1", "1", 2.0, 2.0, "A");
            with_smd_pad(&mut pcb, "R1", "2", 8.0, 2.0, "A");
            with_smd_pad(&mut pcb, "R2", "1", 5.0, 0.5, "B");
            with_smd_pad(&mut pcb, "R2", "2", 5.0, 4.0, "B");
            with_trace(&mut pcb, "A", 2.0, 2.0, 8.0, 2.0);
            with_trace(&mut pcb, "B", 5.0, 0.5, 5.0, 4.0);
            pcb
        };
        let opts = FabPrepOptions {
            route_remaining: false,
            prune_dangling: false,
            max_rounds: 2,
            ..Default::default()
        };

        let mut one_shot_pcb = make();
        let one_shot = run_fab_prep(&mut one_shot_pcb, &opts);

        let mut stepped_pcb = make();
        let mut session = FabPrepSession::begin(&mut stepped_pcb, &opts).expect("no refusal");
        let mut statuses = Vec::new();
        loop {
            let s = session.round(&mut stepped_pcb);
            statuses.push(s);
            if s.done {
                break;
            }
        }
        let stepped = session.finish(&mut stepped_pcb);

        assert_eq!(one_shot.report, stepped.report);
        assert_eq!(one_shot.violations, stepped.violations);
        assert_eq!(
            serde_json::to_string(&one_shot_pcb).unwrap(),
            serde_json::to_string(&stepped_pcb).unwrap()
        );
        assert!(statuses.len() >= 2, "the loop must have visibly stepped");
        assert!(statuses.last().unwrap().done);
    }

    #[test]
    fn calibration_is_off_unless_asked_for() {
        let mut pcb = test_board();
        pcb.rules.default_rules.via_diameter = 0.21;
        pcb.rules.default_rules.via_drill = 0.12;
        pcb.rules.min_drill = 0.2;

        let report = run_fab_prep(
            &mut pcb,
            &FabPrepOptions {
                route_remaining: false,
                prune_dangling: false,
                ..Default::default()
            },
        )
        .report;
        assert!(!report.calibration_requested);
        assert!(report.calibration.is_empty());
        assert_eq!(pcb.rules.min_drill, 0.2, "rules must be untouched");

        let mut pcb2 = test_board();
        pcb2.rules.default_rules.via_diameter = 0.21;
        pcb2.rules.default_rules.via_drill = 0.12;
        pcb2.rules.min_drill = 0.2;
        let report2 = run_fab_prep(
            &mut pcb2,
            &FabPrepOptions {
                calibrate_rules: true,
                route_remaining: false,
                prune_dangling: false,
                ..Default::default()
            },
        )
        .report;
        assert!(report2.calibration_requested);
        assert_eq!(report2.calibration.applied.len(), 2);
        assert_eq!(pcb2.rules.min_drill, 0.12);
    }
}
