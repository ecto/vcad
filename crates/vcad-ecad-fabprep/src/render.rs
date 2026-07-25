//! Human-readable renderings of the receipt.
//!
//! Two documents, deliberately different in shape:
//!
//! * [`drc_report`] — every violation on the finished board, each line tagged
//!   with whether the routing is answerable for it. A reader scanning it can
//!   tell a fixture artifact from a routing fault without cross-referencing
//!   anything.
//! * [`fab_notes`] — the summary a fab package ships with: the claim, the two
//!   numbers behind it, the calibration log, and the honest gaps.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use vcad_ecad_pcb::drc::{DrcSeverity, DrcViolation};

use crate::delta::{rule_name, DeltaMode};
use crate::FabPrepReport;

/// Thousands separators — a receipt that says "1867" reads worse than "1,867",
/// and these numbers are meant to be quoted.
fn commas(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The per-rule delta table, shared by both documents.
fn delta_table(report: &FabPrepReport) -> String {
    let mut s = String::new();
    let _ = writeln!(
        s,
        "{:<22} {:>10} {:>10} {:>18}  mode",
        "rule", "baseline", "final", "route-attributable"
    );
    let _ = writeln!(s, "{}", "-".repeat(78));
    for r in &report.delta.rules {
        let mode = match r.mode {
            DeltaMode::SetDifference => "set difference",
            DeltaMode::CountDifference => "count difference",
        };
        let _ = writeln!(
            s,
            "{:<22} {:>10} {:>10} {:>18}  {mode}{}",
            r.rule,
            commas(r.baseline),
            commas(r.final_count),
            commas(r.route_attributable),
            match (r.accepted, r.route_fixable) {
                (true, _) => " *** ACCEPTED BY WAIVER ***",
                (false, false) => " (not loop-fixable)",
                (false, true) => "",
            },
        );
    }
    let _ = writeln!(s, "{}", "-".repeat(78));
    let _ = writeln!(
        s,
        "{:<22} {:>10} {:>10} {:>18}",
        "TOTAL",
        commas(report.delta.baseline_total),
        commas(report.delta.final_total),
        commas(report.delta.route_attributable_total),
    );
    s
}

/// The verdict, the delta table, the calibration log, and the remaining
/// offenders — everything except the full violation dump.
///
/// This is what a terminal run prints: on a dense board the dump runs to
/// thousands of lines, and burying the verdict under it would defeat the point
/// of having a verdict.
pub fn summary(report: &FabPrepReport) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "vcad fab-prep — DRC delta report");
    let _ = writeln!(s, "================================\n");
    let _ = writeln!(s, "VERDICT: {}\n", report.headline());
    let _ = writeln!(
        s,
        "Baseline = this same board with every trace and via stripped, checked under the same\n\
         rules. It is not zero and is not supposed to be: an imported fixture's own land\n\
         patterns violate its own rules. Route-attributable is the difference, and it is the\n\
         only column the router is answerable for.\n"
    );
    let _ = write!(s, "{}", delta_table(report));
    if !report.accepted_rules.is_empty() {
        let _ = writeln!(
            s,
            "\nWAIVED: {} — {} route-attributable violation(s) in these rules were accepted by an\n\
             explicit caller waiver and do NOT block the verdict. They are counted above and\n\
             listed below; a reader who disagrees with the waiver can see exactly what it covers.",
            report.accepted_rules.join(", "),
            commas(report.delta.route_attributable_accepted),
        );
    }
    let _ = writeln!(
        s,
        "\nConnectivity: {} unconnected/islanded net violation(s) on arrival, {} on completion{}.\n\
         Every geometric violation above can be made to vanish by deleting the copper that causes\n\
         it, so this pair is the guard: a run that cleared violations by removing routing it could\n\
         not replace does not converge, however clean the table looks.",
        commas(report.connectivity.on_arrival),
        commas(report.connectivity.on_completion),
        match report.connectivity.regression() {
            0 => String::new(),
            n => format!(" — a regression of {}", commas(n)),
        }
    );

    if !report.calibration.applied.is_empty() || !report.calibration.refused.is_empty() {
        let _ = writeln!(
            s,
            "\nRule calibration (opt-in, applied to BOTH baseline and final):"
        );
        for c in &report.calibration.applied {
            let _ = writeln!(
                s,
                "  {} {:.3} -> {:.3}\n      {}",
                c.rule, c.declared, c.calibrated, c.justification
            );
        }
        for r in &report.calibration.refused {
            let _ = writeln!(
                s,
                "  {} REFUSED (requested {:.3}, floor {:.3})\n      {}",
                r.rule, r.requested, r.floor, r.reason
            );
        }
    } else if report.calibration_requested {
        let _ = writeln!(
            s,
            "\nRule calibration: requested, nothing to calibrate — the board's rules already\n\
             admit its own declared via classes and pre-existing holes."
        );
    } else {
        let _ = writeln!(
            s,
            "\nRule calibration: NOT REQUESTED — the board was judged against the rules exactly\n\
             as it declared them."
        );
    }

    // Offenders first: if the run failed closed, this is what the reader came
    // for and it must not be buried under thousands of baseline lines.
    if !report.delta.offenders.is_empty() {
        let _ = writeln!(
            s,
            "\nREMAINING ROUTE-ATTRIBUTABLE OFFENDERS ({}):",
            commas(report.delta.offenders.len())
        );
        for o in &report.delta.offenders {
            let _ = writeln!(
                s,
                "  {} [{}] at ({:.3}, {:.3}): {}",
                o.rule, o.severity, o.position[0], o.position[1], o.message
            );
        }
    }
    s
}

/// [`summary`] followed by every violation on the finished board, each line
/// tagged `ROUTE` or `FIXTURE`.
///
/// `violations` must be the DRC of the board the report describes, under the
/// same (possibly calibrated) rules — [`crate::FabPrepOutcome::violations`].
pub fn drc_report(report: &FabPrepReport, violations: &[DrcViolation]) -> String {
    let mut s = summary(report);

    let key_of = |o: &crate::Offender| {
        format!(
            "{}|{:.3},{:.3}|{}",
            o.rule, o.position[0], o.position[1], o.message
        )
    };
    let attributable: BTreeSet<String> = report.delta.offenders.iter().map(key_of).collect();
    let waived: BTreeSet<String> = report.delta.waived.iter().map(key_of).collect();

    let _ = writeln!(s, "\nFULL VIOLATION LIST ({}):", commas(violations.len()));
    for v in violations {
        let key = format!(
            "{}|{:.3},{:.3}|{}",
            rule_name(&v.rule),
            v.position.x,
            v.position.y,
            v.message
        );
        let tag = if attributable.contains(&key) {
            "ROUTE"
        } else if waived.contains(&key) {
            "WAIVED"
        } else {
            "FIXTURE"
        };
        let sev = match v.severity {
            DrcSeverity::Error => "HARD",
            DrcSeverity::Warning => "WARN",
        };
        let _ = writeln!(s, "{tag:<8} {sev}: {}", v.message);
    }
    s
}

/// The `FAB_NOTES.md` that ships inside the package.
pub fn fab_notes(report: &FabPrepReport) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "# vcad autorouted fab package\n");
    let _ = writeln!(
        s,
        "Prepared by `vcad fab-prep`. {} copper layers, {} nets, {} pads across {} footprints; \
         {} trace segments and {} vias realized.\n",
        report.board.copper_layers,
        commas(report.board.nets),
        commas(report.board.pads),
        commas(report.board.footprints),
        commas(report.board.traces),
        commas(report.board.vias),
    );

    let _ = writeln!(s, "## DRC status (vs the stripped-board baseline)\n");
    if report.converged {
        let _ = writeln!(
            s,
            "Route-attributable violations: **ZERO** in every loop-fixable rule.\n"
        );
        if !report.accepted_rules.is_empty() {
            let _ = writeln!(
                s,
                "**With waivers.** {} route-attributable violation(s) in `{}` were accepted by an \
                 explicit caller waiver rather than fixed. That is a deliberate, named exception — \
                 read it before treating this board as clean.\n",
                commas(report.delta.route_attributable_accepted),
                report.accepted_rules.join("`, `"),
            );
        }
    } else {
        let _ = writeln!(
            s,
            "**NOT FAB-READY.** {} route-attributable violation(s) remain — {}.\n",
            commas(report.delta.route_attributable_fixable),
            report.blocker.as_deref().unwrap_or("loop did not converge"),
        );
    }
    let _ = writeln!(
        s,
        "Absolute zero is not achievable on an imported fixture, and claiming it would be a \
         different claim than the one this run supports. The same board stripped of all routing \
         scores **{}** violations against its own land patterns; the finished board scores \
         **{}**. Both numbers are below. The router is answerable for the difference.\n",
        commas(report.delta.baseline_total),
        commas(report.delta.final_total),
    );
    let _ = writeln!(s, "```");
    let _ = write!(s, "{}", delta_table(report));
    let _ = writeln!(s, "```\n");
    let _ = writeln!(
        s,
        "Connectivity: **{}** unconnected/islanded net violation(s) on arrival, **{}** on \
         completion. A geometric violation can always be made to vanish by deleting the copper \
         that causes it, so this pair is checked too — a run that cleared violations by removing \
         routing it could not replace does not converge.\n",
        commas(report.connectivity.on_arrival),
        commas(report.connectivity.on_completion),
    );
    let _ = writeln!(
        s,
        "Connectivity rules (unconnected nets, net islands, unstitched pads) use a count \
         difference rather than a set difference: adding copper legitimately rewrites those \
         violations, so the claim there is the weaker, non-fictional one — the router left fewer \
         than it found.\n"
    );

    let _ = writeln!(s, "## Rule calibration\n");
    if !report.calibration_requested {
        let _ = writeln!(
            s,
            "Not requested. The board was judged against the rules exactly as it declared them.\n"
        );
    } else if report.calibration.is_empty() {
        let _ = writeln!(
            s,
            "Requested; nothing to calibrate. The board's rules already admit its own declared \
             via classes and pre-existing holes.\n"
        );
    } else {
        let _ = writeln!(
            s,
            "Requested and applied to **both** the baseline and the final check — a delta taken \
             under two different rule sets would be meaningless.\n"
        );
        for c in &report.calibration.applied {
            let _ = writeln!(
                s,
                "- `{}` {:.3} → {:.3} — {}",
                c.rule, c.declared, c.calibrated, c.justification
            );
        }
        for r in &report.calibration.refused {
            let _ = writeln!(
                s,
                "- `{}` **refused** (asked for {:.3}, floor {:.3}) — {}",
                r.rule, r.requested, r.floor, r.reason
            );
        }
        let _ = writeln!(s);
    }

    let _ = writeln!(s, "## Routing verdict\n");
    if let Some(v) = &report.initial_verdict {
        let _ = writeln!(
            s,
            "Opening pass over {} unrouted connection(s) in {} search window(s): **{} routed**, \
             {} proved infeasible (bottleneck-cut certificates), {} honest unknown (search budget \
             exhausted, or a path the legality oracle rejected).\n",
            commas(v.connections),
            commas(v.clusters),
            commas(v.routed),
            commas(v.proved_infeasible),
            commas(v.unknown),
        );
        if !v.certificates.is_empty() {
            let _ = writeln!(
                s,
                "Infeasibility certificates (first {}):\n",
                v.certificates.len()
            );
            for c in &v.certificates {
                let _ = writeln!(s, "- {c}");
            }
            let _ = writeln!(s);
        }
    } else {
        let _ = writeln!(
            s,
            "Skipped — the board was taken as routed, and only the fix loop ran.\n"
        );
    }

    let _ = writeln!(s, "## Fix loop\n");
    if report.rounds.is_empty() {
        let _ = writeln!(
            s,
            "No rounds needed: the board had no route-attributable violations to strip.\n"
        );
    } else {
        let _ = writeln!(
            s,
            "| round | attributable before | nets stripped | traces | vias | re-routed | proved | unknown |"
        );
        let _ = writeln!(s, "|---|---|---|---|---|---|---|---|");
        for r in &report.rounds {
            let _ = writeln!(
                s,
                "| {} | {} | {} | {} | {} | {} | {} | {} |",
                r.round,
                commas(r.attributable_before),
                commas(r.offending_nets.len()),
                commas(r.stripped_traces),
                commas(r.stripped_vias),
                commas(r.verdict.routed),
                commas(r.verdict.proved_infeasible),
                commas(r.verdict.unknown),
            );
        }
        let _ = writeln!(s);
    }
    let _ = writeln!(
        s,
        "Dangling-copper prune: {} trace(s) and {} via(s) removed (copper reaching no pad or \
         pour of its net).\n",
        commas(report.pruned_traces),
        commas(report.pruned_vias),
    );

    let _ = writeln!(s, "## Known gaps (honest)\n");
    let unrouted: usize = report
        .initial_verdict
        .iter()
        .map(|v| v.proved_infeasible + v.unknown)
        .sum::<usize>()
        + report
            .rounds
            .iter()
            .map(|r| r.verdict.proved_infeasible + r.verdict.unknown)
            .sum::<usize>();
    if unrouted > 0 {
        let _ = writeln!(
            s,
            "- {} connection(s) end unrouted: proved-infeasible certificates plus honest unknowns \
             from the verdict ladder. They are accounted for, not hidden.",
            commas(unrouted)
        );
    }
    for r in &report.delta.rules {
        if r.route_attributable == 0 || r.route_fixable {
            continue;
        }
        if r.accepted {
            let _ = writeln!(
                s,
                "- `{}` is {} above baseline and was **waived**, not fixed. The violations are \
                 real and still on the board; someone decided to accept them.",
                r.rule, r.route_attributable
            );
        } else {
            let _ = writeln!(
                s,
                "- `{}` is {} above baseline. The strip-and-re-route loop does not target this \
                 rule — stripping a net *creates* connectivity violations, so chasing them would \
                 make the loop chase its own tail. Reported, not claimed clean.",
                r.rule, r.route_attributable
            );
        }
    }
    let _ = writeln!(
        s,
        "- Impedance geometry is class-width, not field-solved per layer.\n"
    );
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{test_board, with_smd_pad, with_trace};
    use crate::{run_fab_prep, FabPrepOptions};

    fn short_board_report() -> FabPrepReport {
        let mut pcb = test_board();
        with_smd_pad(&mut pcb, "R1", "1", 2.0, 2.0, "A");
        with_smd_pad(&mut pcb, "R1", "2", 8.0, 2.0, "A");
        with_smd_pad(&mut pcb, "R2", "1", 5.0, 0.5, "B");
        with_smd_pad(&mut pcb, "R2", "2", 5.0, 4.0, "B");
        with_trace(&mut pcb, "A", 2.0, 2.0, 8.0, 2.0);
        with_trace(&mut pcb, "B", 5.0, 0.5, 5.0, 4.0);
        run_fab_prep(
            &mut pcb,
            &FabPrepOptions {
                route_remaining: false,
                prune_dangling: false,
                max_rounds: 1,
                ..Default::default()
            },
        )
        .report
    }

    #[test]
    fn commas_group_thousands() {
        assert_eq!(commas(0), "0");
        assert_eq!(commas(980), "980");
        assert_eq!(commas(1867), "1,867");
        assert_eq!(commas(16485), "16,485");
    }

    #[test]
    fn the_report_states_both_numbers_and_the_mode_per_rule() {
        let report = short_board_report();
        let text = drc_report(&report, &[]);
        assert!(text.contains("baseline"), "{text}");
        assert!(text.contains("route-attributable"));
        assert!(text.contains("set difference"));
        assert!(
            text.contains("NOT REQUESTED"),
            "an uncalibrated run must say so: {text}"
        );
    }

    #[test]
    fn a_failed_run_lists_its_offenders_up_front() {
        let report = short_board_report();
        assert!(!report.converged);
        let text = drc_report(&report, &[]);
        assert!(
            text.contains("REMAINING ROUTE-ATTRIBUTABLE OFFENDERS"),
            "{text}"
        );
        let notes = fab_notes(&report);
        assert!(notes.contains("NOT FAB-READY"), "{notes}");
    }

    #[test]
    fn notes_log_every_calibration_with_its_justification() {
        let mut pcb = test_board();
        pcb.rules.default_rules.via_diameter = 0.21;
        pcb.rules.default_rules.via_drill = 0.12;
        pcb.rules.min_drill = 0.2;
        let report = run_fab_prep(
            &mut pcb,
            &FabPrepOptions {
                calibrate_rules: true,
                route_remaining: false,
                prune_dangling: false,
                ..Default::default()
            },
        )
        .report;
        let notes = fab_notes(&report);
        assert!(notes.contains("`minDrill` 0.200 → 0.120"), "{notes}");
        assert!(notes.contains("via class 'Default'"), "{notes}");
        assert!(
            notes.contains("applied to **both** the baseline and the final check"),
            "{notes}"
        );
    }
}
