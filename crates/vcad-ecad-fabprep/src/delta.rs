//! The DRC delta: what the *routing* is responsible for, separated from what
//! the board arrived with.
//!
//! Absolute zero is not a reachable claim on an imported fixture. The stripped
//! CM5 board — no traces, no vias, nothing but its own land patterns — already
//! scores 980 short/clearance violations, and the human production board
//! scores 16,485 under the same rules. A receipt that reported "1,150
//! violations" would be indistinguishable from a receipt that reported "1,150
//! violations, 0 of which we caused", and only the second one is a claim about
//! the router.
//!
//! So every number here is a *pair*: the baseline (same board, all traces and
//! vias stripped, same rules) and the final. Route-attributable is the
//! difference — and how the difference is taken is stated per rule rather than
//! assumed, because the two kinds of rule behave differently:
//!
//! * **Geometric** rules (clearance, widths, drills, rings, edge, hole-to-hole,
//!   keepouts) name a fixed pair of objects at a fixed place. Their violations
//!   survive stripping unchanged, so the honest difference is a **set**
//!   difference: a final violation counts against the router only if it is not
//!   literally the same violation the stripped board already had.
//! * **Connectivity** rules (unconnected nets, net islands, unstitched pads)
//!   are *supposed* to change when copper is added — stripping the board makes
//!   every net unconnected by construction. A set difference would charge the
//!   router for the entire final population. These use a **count** difference,
//!   saturating at zero, which is the weaker but non-fictional claim: the
//!   router left fewer of these than it found.

use std::collections::{BTreeMap, BTreeSet};

use vcad_ecad_pcb::drc::{DrcRuleType, DrcSeverity, DrcViolation};

/// How a rule's route-attributable count is derived.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeltaMode {
    /// Final violations that are not literally present in the baseline.
    /// Used where a violation's identity survives stripping.
    SetDifference,
    /// `max(0, final − baseline)`. Used where adding copper legitimately
    /// rewrites the violation's identity (connectivity rules).
    CountDifference,
}

/// Rules a strip-and-re-route round can actually fix, and therefore the rules
/// the fix loop drives to zero route-attributable. Connectivity rules are not
/// in this set: stripping a net *creates* `UnconnectedNet`, so including them
/// would make the loop chase its own tail, and the verdict ladder already
/// reports unrouted connections as explicit proved-infeasible or unknown
/// certificates rather than hiding them in a DRC count.
pub const ROUTE_FIXABLE: &[DrcRuleType] = &[
    DrcRuleType::Clearance,
    DrcRuleType::Short,
    DrcRuleType::MinTraceWidth,
    DrcRuleType::EdgeClearance,
    DrcRuleType::HoleToHole,
    DrcRuleType::AnnularRing,
    DrcRuleType::MinDrill,
    DrcRuleType::Keepout,
    DrcRuleType::SameNetBypass,
];

/// Every rule name this crate knows, for validating a caller's waiver list. A
/// waiver naming a rule that does not exist is a typo that would silently do
/// nothing, which is the worst possible outcome for a safety valve.
pub const ALL_RULES: &[DrcRuleType] = &[
    DrcRuleType::Clearance,
    DrcRuleType::MinTraceWidth,
    DrcRuleType::MinDrill,
    DrcRuleType::AnnularRing,
    DrcRuleType::EdgeClearance,
    DrcRuleType::HoleToHole,
    DrcRuleType::UnconnectedNet,
    DrcRuleType::SilkscreenClearance,
    DrcRuleType::CourtyardOverlap,
    DrcRuleType::AcidTrap,
    DrcRuleType::Keepout,
    DrcRuleType::Short,
    DrcRuleType::NetIslands,
    DrcRuleType::UnstitchedPad,
    DrcRuleType::SameNetBypass,
];

/// Stable rule label for reports and JSON keys.
///
/// `DrcRuleType`'s variants are all unit variants, so `Debug` is exactly the
/// variant name — the same spelling the kernel's serde output uses.
pub fn rule_name(rule: &DrcRuleType) -> String {
    format!("{rule:?}")
}

/// How this rule's identity behaves across a strip.
fn delta_mode(rule: &DrcRuleType) -> DeltaMode {
    match rule {
        DrcRuleType::UnconnectedNet | DrcRuleType::NetIslands | DrcRuleType::UnstitchedPad => {
            DeltaMode::CountDifference
        }
        _ => DeltaMode::SetDifference,
    }
}

/// Net names quoted in a DRC message.
///
/// The kernel's DRC deliberately spells net attribution into its messages as a
/// quoted token — the same channel the MCP summary buckets by. Two spellings
/// occur and both are read here:
///
/// * `... net 'A' ... net 'B' ...` — clearance, width, keepout, islands, …
/// * `Short: nets 'A' and 'B' are connected by copper` — the plural form, which
///   a `net '` scan silently misses (there is an `s` between `net` and the
///   quote). Missing it would leave every short unattributed.
///
/// Returned in first-seen order, deduplicated.
pub fn nets_in_message(message: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let bytes = message.as_bytes();

    let mut push = |name: &str| {
        if !name.is_empty() && !out.iter().any(|n| n == name) {
            out.push(name.to_string());
        }
    };

    // Plural form: `nets 'A' and 'B'`.
    if let Some(rel) = message.find("nets '") {
        let mut i = rel + "nets '".len();
        // Two quoted names separated by ` and `.
        for _ in 0..2 {
            let Some(end_rel) = message[i..].find('\'') else {
                break;
            };
            let end = i + end_rel;
            push(&message[i..end]);
            let Some(next_rel) = message[end..].find(" and '") else {
                break;
            };
            i = end + next_rel + " and '".len();
        }
    }

    // Singular form: every `net '<name>'`. `nets '` cannot match this pattern,
    // so the two passes never double-read the same token.
    let mut i = 0;
    while let Some(rel) = message[i..].find("net '") {
        let start = i + rel + "net '".len();
        // The token must start a word — `Net-(U1-PAD)` inside a name must not
        // be mistaken for an attribution token.
        let is_word_start = i + rel == 0 || !bytes[i + rel - 1].is_ascii_alphanumeric();
        match message[start..].find('\'') {
            Some(end_rel) => {
                let end = start + end_rel;
                if is_word_start {
                    push(&message[start..end]);
                }
                i = end + 1;
            }
            None => break,
        }
    }
    out
}

/// Union-find over net names, used to reason about the fixture's own shorts.
#[derive(Default)]
struct NetGroups {
    parent: BTreeMap<String, String>,
}

impl NetGroups {
    fn find(&mut self, n: &str) -> String {
        let Some(p) = self.parent.get(n).cloned() else {
            return n.to_string();
        };
        if p == n {
            return p;
        }
        let root = self.find(&p);
        self.parent.insert(n.to_string(), root.clone());
        root
    }

    fn union(&mut self, a: &str, b: &str) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.parent.insert(ra, rb);
        }
    }

    /// The net groups the *fixture* already merged, before any routing.
    fn from_shorts(violations: &[DrcViolation]) -> Self {
        let mut g = Self::default();
        for v in violations.iter().filter(|v| v.rule == DrcRuleType::Short) {
            let nets = nets_in_message(&v.message);
            for pair in nets.windows(2) {
                g.union(&pair[0], &pair[1]);
            }
        }
        g
    }
}

/// Identity of a violation for set-difference purposes.
///
/// Shorts do not use this: their identity is the net *group* they belong to, not
/// a place on the board. See [`short_is_route_attributable`].
fn violation_key(v: &DrcViolation) -> String {
    format!(
        "{}|{:?}|{:.3},{:.3}|{}",
        rule_name(&v.rule),
        v.provenance,
        v.position.x,
        v.position.y,
        v.message
    )
}

/// Whether a short on the finished board is the routing's doing.
///
/// Shorts propagate, and that makes naive differencing badly wrong on an
/// imported fixture. Suppose the fixture's own overlapping pads already short
/// net X to net A, and elsewhere short net Y to net A. Route net A between its
/// own two pads — a perfectly legal connection — and the two copper blobs merge,
/// so the DRC now also reports X shorted to Y. That pair is new, but the router
/// did nothing wrong: the fault is entirely the fixture's overlapping pads, and
/// the routing was the innocent wire that happened to join them.
///
/// The sound test is therefore not "is this exact pair new?" but "did the
/// routing merge two groups the fixture had kept apart?". Routing only ever
/// joins copper of a single net, so a merge can only bridge two groups that
/// *already contained that net* — which means the fixture had already shorted
/// into both. A pair whose two nets sit in the same baseline group is
/// fixture-implied; a pair spanning two baseline groups is a real new bridge and
/// is charged.
fn short_is_route_attributable(v: &DrcViolation, baseline_groups: &mut NetGroups) -> bool {
    let nets = nets_in_message(&v.message);
    if nets.len() < 2 {
        // No attribution channel — charge it, because the safe direction for a
        // violation we cannot explain is "ours".
        return true;
    }
    let root = baseline_groups.find(&nets[0]);
    nets[1..].iter().any(|n| baseline_groups.find(n) != root)
}

/// Per-rule accounting: what the stripped board had, what the routed board has,
/// and what the difference charges to the routing.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RuleDelta {
    /// Rule name (`Clearance`, `NetIslands`, …).
    pub rule: String,
    /// Violations on the stripped board (same rules, no traces or vias).
    pub baseline: usize,
    /// Violations on the finished board.
    pub final_count: usize,
    /// Violations charged to the routing.
    pub route_attributable: usize,
    /// How `route_attributable` was derived.
    pub mode: DeltaMode,
    /// True when the rule is one the fix loop drives to zero.
    pub route_fixable: bool,
    /// True when the caller explicitly waived this rule. An accepted rule is
    /// still counted and still reported — it simply does not block the verdict.
    pub accepted: bool,
}

/// A route-attributable violation, carried into the receipt so a
/// non-converging run can name exactly what it could not fix.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Offender {
    /// Rule name.
    pub rule: String,
    /// Severity as reported by the kernel.
    pub severity: String,
    /// Board position (mm).
    pub position: [f64; 2],
    /// Kernel message, verbatim.
    pub message: String,
    /// The rule's required value (mm). Doubles as the search radius when the
    /// message names no net and the offender has to be attributed geometrically.
    pub required: f64,
    /// Nets named in the message, if any. Empty for the rules whose messages
    /// carry no net at all (hole-to-hole, annular ring, drill) — those are
    /// attributed by position instead, see [`crate::verdict::nets_near`].
    pub nets: Vec<String>,
}

impl Offender {
    fn from(v: &DrcViolation) -> Self {
        Self {
            rule: rule_name(&v.rule),
            severity: match v.severity {
                DrcSeverity::Error => "error".into(),
                DrcSeverity::Warning => "warning".into(),
            },
            position: [v.position.x, v.position.y],
            message: v.message.clone(),
            required: v.required,
            nets: nets_in_message(&v.message),
        }
    }
}

/// The full delta between a stripped-board baseline and a finished board.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DrcDelta {
    /// Per-rule rows, rule-name ordered.
    pub rules: Vec<RuleDelta>,
    /// Total violations on the stripped board.
    pub baseline_total: usize,
    /// Total violations on the finished board.
    pub final_total: usize,
    /// Total charged to the routing, across every rule.
    pub route_attributable_total: usize,
    /// Charged to the routing, restricted to [`ROUTE_FIXABLE`] minus any rule
    /// the caller waived — the number the fix loop is trying to zero and the one
    /// the convergence verdict reads.
    pub route_attributable_fixable: usize,
    /// Charged to the routing but explicitly waived by the caller. Never zero
    /// silently: a waiver that hides violations has to show how many.
    pub route_attributable_accepted: usize,
    /// The route-attributable violations the loop must clear — [`ROUTE_FIXABLE`]
    /// minus anything the caller waived.
    pub offenders: Vec<Offender>,
    /// Route-attributable violations in waived rules. Kept separate from
    /// `offenders` so the loop does not chase them, and kept at all so the
    /// report can label them as *waived* rather than silently filing them under
    /// the fixture's baseline — which would be the exact misattribution this
    /// whole delta exists to prevent.
    pub waived: Vec<Offender>,
}

impl DrcDelta {
    /// Compute the delta between `baseline` (stripped board) and `final_`.
    ///
    /// `accepted` names rules the caller has explicitly waived — they keep their
    /// counts in the table and gain an `accepted` flag, but stop blocking the
    /// verdict. The CM5 campaign did exactly this in prose for the 69 fab-legal
    /// 0.08 mm neckdowns on its SI-class nets; making it a parameter with a
    /// receipt entry is the same decision, only auditable.
    pub fn compute(
        baseline: &[DrcViolation],
        final_: &[DrcViolation],
        accepted: &BTreeSet<String>,
    ) -> Self {
        let fixable: BTreeSet<String> = ROUTE_FIXABLE
            .iter()
            .map(rule_name)
            .filter(|r| !accepted.contains(r))
            .collect();

        // (baseline count, final count, mode) per rule name.
        let mut counts: BTreeMap<String, (usize, usize, DeltaMode)> = BTreeMap::new();
        for v in baseline {
            counts
                .entry(rule_name(&v.rule))
                .or_insert((0, 0, delta_mode(&v.rule)))
                .0 += 1;
        }
        for v in final_ {
            counts
                .entry(rule_name(&v.rule))
                .or_insert((0, 0, delta_mode(&v.rule)))
                .1 += 1;
        }

        // A baseline key can legitimately occur more than once (two identical
        // messages at the same rounded position); consume one baseline
        // occurrence per matching final violation so a genuine duplicate the
        // router *added* still shows up.
        let mut remaining: BTreeMap<String, usize> = BTreeMap::new();
        for v in baseline {
            *remaining.entry(violation_key(v)).or_default() += 1;
        }

        let mut baseline_groups = NetGroups::from_shorts(baseline);
        let mut attributable: Vec<&DrcViolation> = Vec::new();
        for v in final_ {
            if delta_mode(&v.rule) != DeltaMode::SetDifference {
                continue;
            }
            if v.rule == DrcRuleType::Short {
                if short_is_route_attributable(v, &mut baseline_groups) {
                    attributable.push(v);
                }
                continue;
            }
            match remaining.get_mut(&violation_key(v)) {
                Some(n) if *n > 0 => *n -= 1,
                _ => attributable.push(v),
            }
        }

        let mut set_attr: BTreeMap<String, usize> = BTreeMap::new();
        for v in &attributable {
            *set_attr.entry(rule_name(&v.rule)).or_default() += 1;
        }

        let rules: Vec<RuleDelta> = counts
            .iter()
            .map(|(name, (b, f, mode))| {
                let route_attributable = match mode {
                    DeltaMode::SetDifference => set_attr.get(name).copied().unwrap_or(0),
                    DeltaMode::CountDifference => f.saturating_sub(*b),
                };
                RuleDelta {
                    rule: name.clone(),
                    baseline: *b,
                    final_count: *f,
                    route_attributable,
                    mode: *mode,
                    route_fixable: fixable.contains(name),
                    accepted: accepted.contains(name),
                }
            })
            .collect();

        let offenders: Vec<Offender> = attributable
            .iter()
            .copied()
            .filter(|v| fixable.contains(&rule_name(&v.rule)))
            .map(Offender::from)
            .collect();
        let waived: Vec<Offender> = attributable
            .iter()
            .copied()
            .filter(|v| accepted.contains(&rule_name(&v.rule)))
            .map(Offender::from)
            .collect();

        Self {
            baseline_total: baseline.len(),
            final_total: final_.len(),
            route_attributable_total: rules.iter().map(|r| r.route_attributable).sum(),
            route_attributable_fixable: rules
                .iter()
                .filter(|r| r.route_fixable)
                .map(|r| r.route_attributable)
                .sum(),
            route_attributable_accepted: rules
                .iter()
                .filter(|r| r.accepted)
                .map(|r| r.route_attributable)
                .sum(),
            offenders,
            waived,
            rules,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_ecad_pcb::drc::DrcProvenance;
    use vcad_ir::Vec2;

    fn v(rule: DrcRuleType, x: f64, msg: &str) -> DrcViolation {
        DrcViolation {
            rule,
            severity: DrcSeverity::Error,
            position: Vec2::new(x, 0.0),
            message: msg.to_string(),
            actual: 0.0,
            required: 0.1,
            provenance: DrcProvenance::Routing,
            generated: false,
        }
    }

    #[test]
    fn nets_parse_out_of_kernel_messages() {
        assert_eq!(
            nets_in_message("Clearance violation: trace net 'GND' to net '+5V': 0.01mm < 0.08mm"),
            vec!["GND", "+5V"],
            "both sides of a clearance pair are attributable"
        );
        assert_eq!(
            nets_in_message(
                "Clearance violation: pad C1.1 net 'VCC' to pad J1.2 net 'Net-(U1-PAD)': 0.0mm"
            ),
            vec!["VCC", "Net-(U1-PAD)"]
        );
        assert!(nets_in_message("Via annular ring 0.030mm < 0.045mm").is_empty());
        assert_eq!(
            nets_in_message("Short: nets 'GND' and '+3V3' are connected by copper"),
            vec!["GND", "+3V3"],
            "the plural `nets 'A' and 'B'` spelling is the only attribution a short carries"
        );
    }

    #[test]
    fn an_identical_geometric_violation_is_not_charged_to_the_router() {
        let base = vec![v(DrcRuleType::Clearance, 1.0, "pad overlap")];
        let fin = vec![
            v(DrcRuleType::Clearance, 1.0, "pad overlap"),
            v(DrcRuleType::Clearance, 2.0, "trace net 'A' to net 'B'"),
        ];
        let d = DrcDelta::compute(&base, &fin, &BTreeSet::new());
        assert_eq!(d.baseline_total, 1);
        assert_eq!(d.final_total, 2);
        assert_eq!(d.route_attributable_fixable, 1);
        assert_eq!(d.offenders.len(), 1);
        assert_eq!(d.offenders[0].nets, vec!["A", "B"]);
    }

    #[test]
    fn a_duplicated_baseline_violation_still_counts_once_against_the_router() {
        let base = vec![v(DrcRuleType::Clearance, 1.0, "pad overlap")];
        let fin = vec![
            v(DrcRuleType::Clearance, 1.0, "pad overlap"),
            v(DrcRuleType::Clearance, 1.0, "pad overlap"),
        ];
        let d = DrcDelta::compute(&base, &fin, &BTreeSet::new());
        assert_eq!(d.route_attributable_fixable, 1);
    }

    #[test]
    fn connectivity_rules_use_a_count_difference_and_saturate() {
        let base = vec![
            v(DrcRuleType::NetIslands, 1.0, "Disjoint net 'A': 3 islands"),
            v(DrcRuleType::NetIslands, 2.0, "Disjoint net 'B': 2 islands"),
        ];
        let fin = vec![v(
            DrcRuleType::NetIslands,
            9.0,
            "Disjoint net 'C': 2 islands",
        )];
        let d = DrcDelta::compute(&base, &fin, &BTreeSet::new());
        let row = d.rules.iter().find(|r| r.rule == "NetIslands").unwrap();
        assert_eq!(row.mode, DeltaMode::CountDifference);
        assert_eq!(row.baseline, 2);
        assert_eq!(row.final_count, 1);
        assert_eq!(
            row.route_attributable, 0,
            "the router repaired one island; that is never negative credit"
        );
    }

    #[test]
    fn a_short_keeps_its_identity_when_the_contact_point_moves() {
        let base = vec![v(
            DrcRuleType::Short,
            1.0,
            "Short: nets 'A' and 'B' are connected by copper",
        )];
        let fin = vec![v(
            DrcRuleType::Short,
            7.5,
            "Short: nets 'B' and 'A' are connected by copper",
        )];
        let d = DrcDelta::compute(&base, &fin, &BTreeSet::new());
        assert_eq!(
            d.route_attributable_fixable, 0,
            "same net pair, different contact point — the fixture's short, not ours"
        );
    }

    /// The fixture shorts X–A and Y–A through its own overlapping pads. Routing
    /// net A between its own pads merges the two blobs, so X–Y is now reported
    /// too. The router is not answerable for a pair the fixture's pad overlaps
    /// had already implied.
    #[test]
    fn a_short_implied_by_two_fixture_shorts_is_not_charged_to_the_router() {
        let base = vec![
            v(
                DrcRuleType::Short,
                1.0,
                "Short: nets 'X' and 'A' are connected by copper",
            ),
            v(
                DrcRuleType::Short,
                2.0,
                "Short: nets 'Y' and 'A' are connected by copper",
            ),
        ];
        let mut fin = base.clone();
        fin.push(v(
            DrcRuleType::Short,
            3.0,
            "Short: nets 'X' and 'Y' are connected by copper",
        ));
        let d = DrcDelta::compute(&base, &fin, &BTreeSet::new());
        assert_eq!(
            d.route_attributable_fixable, 0,
            "X and Y sit in the same baseline short group — the fixture merged them"
        );
    }

    /// A genuinely new bridge between two nets the fixture kept apart is
    /// charged, and named.
    #[test]
    fn a_short_bridging_two_baseline_groups_is_charged() {
        let base = vec![v(
            DrcRuleType::Short,
            1.0,
            "Short: nets 'X' and 'A' are connected by copper",
        )];
        let fin = vec![
            base[0].clone(),
            v(
                DrcRuleType::Short,
                9.0,
                "Short: nets 'A' and 'Z' are connected by copper",
            ),
        ];
        let d = DrcDelta::compute(&base, &fin, &BTreeSet::new());
        assert_eq!(d.route_attributable_fixable, 1);
        assert_eq!(d.offenders[0].nets, vec!["A", "Z"]);
    }

    /// A waived violation is not the fixture's fault and must never be filed as
    /// such — that misattribution is the exact thing this delta exists to stop.
    #[test]
    fn a_waived_violation_is_listed_as_waived_not_as_fixture_baseline() {
        let base: Vec<DrcViolation> = vec![];
        let fin = vec![v(
            DrcRuleType::MinTraceWidth,
            1.0,
            "Trace width 0.050mm below minimum 0.200mm for net 'A'",
        )];
        let accepted: BTreeSet<String> = ["MinTraceWidth".to_string()].into_iter().collect();
        let d = DrcDelta::compute(&base, &fin, &accepted);
        assert_eq!(d.route_attributable_fixable, 0, "waived: does not block");
        assert_eq!(d.route_attributable_accepted, 1);
        assert!(
            d.offenders.is_empty(),
            "the loop must not chase a waived rule"
        );
        assert_eq!(d.waived.len(), 1, "but it must still be attributed to us");
        assert_eq!(d.waived[0].nets, vec!["A"]);
    }

    #[test]
    fn non_fixable_rules_stay_out_of_the_convergence_number() {
        let base: Vec<DrcViolation> = vec![];
        let fin = vec![v(
            DrcRuleType::UnstitchedPad,
            1.0,
            "Unstitched pad U1.1 on plane net 'GND'",
        )];
        let d = DrcDelta::compute(&base, &fin, &BTreeSet::new());
        assert_eq!(d.route_attributable_total, 1);
        assert_eq!(d.route_attributable_fixable, 0);
        assert!(d.offenders.is_empty());
    }
}
