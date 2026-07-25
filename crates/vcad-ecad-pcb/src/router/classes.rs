//! Net-class inference and application — the constraint foundation for
//! signal-integrity routing.
//!
//! Production boards imported from traced references (the CM5 fixture) carry
//! a single `Default` net class: the electrical intent — which nets are
//! differential pairs, which belong to a matched bus — lives only in the net
//! *names*. This module recovers that intent:
//!
//! * [`classify_nets`] scans net names and produces a [`NetClassifier`]:
//!   pair membership (via [`super::pair::pair_partner`] plus the USB
//!   `DP`/`DM` convention) and length-match groups (LPDDR4 CA/DQ buses per
//!   channel, RGMII, and every pair intra-matched).
//! * [`apply_classes`] realizes the classification as IR design rules — a
//!   `diff-pair` class carrying the board's differential geometry — so the
//!   existing per-net width/clearance plumbing (session maps, DRC) simply
//!   picks it up.
//!
//! The classifier is deliberately name-driven and conservative: a net it
//! cannot place stays in the default class, which is always legal — just not
//! yet signal-integrity-aware.

use std::collections::BTreeMap;

use vcad_ir::ecad::{NetClassRules, Pcb};

use super::pair::pair_partner;

/// The class name applied to differential-pair nets.
pub const DIFF_PAIR_CLASS: &str = "diff-pair";

/// Recovered electrical intent for a board's nets.
#[derive(Debug, Clone, Default)]
pub struct NetClassifier {
    /// Differential pairs as `(positive, negative)` net names. Each net
    /// appears in at most one pair.
    pub pairs: Vec<(String, String)>,
    /// Length-match groups: group name → member nets. Pairs are implicitly
    /// intra-matched and do not need a group here unless they also belong to
    /// a bus (e.g. DDR DQS within a byte lane).
    pub match_groups: BTreeMap<String, Vec<String>>,
}

impl NetClassifier {
    /// True when `net` is one leg of a recognized differential pair.
    pub fn is_pair_member(&self, net: &str) -> bool {
        self.pairs.iter().any(|(p, n)| p == net || n == net)
    }
}

/// Scan `nets` and recover pairs and match groups from their names.
pub fn classify_nets(nets: &[String]) -> NetClassifier {
    let set: std::collections::BTreeSet<&str> = nets.iter().map(|s| s.as_str()).collect();
    let mut c = NetClassifier::default();
    let mut paired: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

    for net in nets {
        if paired.contains(net.as_str()) {
            continue;
        }
        // `pair_partner` is the single source of truth: what this classifier
        // declares as a pair is exactly what the pair router can route.
        let partner = pair_partner(net).filter(|p| set.contains(p.as_str()));
        if let Some(p) = partner {
            // Canonical order: positive leg first when identifiable.
            let (pos, neg) = if net.ends_with('N') || net.ends_with('M') || net.ends_with('-') {
                (p.clone(), net.clone())
            } else {
                (net.clone(), p.clone())
            };
            if !paired.contains(&pos) && !paired.contains(&neg) {
                paired.insert(pos.clone());
                paired.insert(neg.clone());
                c.pairs.push((pos, neg));
            }
        }
    }

    // Match groups. LPDDR4: CA bus per channel (the `_A`/`_B` suffix on the
    // CM5 is the die/channel, not a pair); DQ bus per byte lane; RGMII as one
    // source-synchronous group.
    for net in nets {
        let group = if let Some(rest) = net.strip_prefix("/LPDDR4 RAM/") {
            if rest.starts_with("CA") || rest.starts_with("CKE") || rest.starts_with("CS") {
                let chan = rest.chars().last().unwrap_or('A');
                Some(format!("lpddr4-ca-{chan}"))
            } else if let Some(dq) = rest.strip_prefix("DQ") {
                // Byte lane = DQ index / 8; DQS/DM riders join their lane.
                dq.chars()
                    .take_while(|ch| ch.is_ascii_digit())
                    .collect::<String>()
                    .parse::<usize>()
                    .ok()
                    .map(|i| format!("lpddr4-dq-lane{}", i / 8))
            } else {
                None
            }
        } else if net.starts_with("/RGMII.") {
            Some("rgmii".to_string())
        } else {
            None
        };
        if let Some(g) = group {
            c.match_groups.entry(g).or_default().push(net.clone());
        }
    }
    // A "group" of one constrains nothing.
    c.match_groups.retain(|_, v| v.len() > 1);
    c
}

/// Realize `classifier` on the board's design rules: create (or update) the
/// [`DIFF_PAIR_CLASS`] with the board's differential geometry and assign
/// every pair leg to it. The existing session/DRC per-net maps pick the
/// class up with no further wiring.
pub fn apply_classes(pcb: &mut Pcb, classifier: &NetClassifier) {
    if classifier.pairs.is_empty() {
        return;
    }
    let d = &pcb.rules.default_rules;
    // Boards imported without net-class geometry (calibrated .kicad_pcb —
    // classes live in the .kicad_pro that never ships with it) get the
    // CM5-class defaults: 0.2 mm legs, 0.25 mm gap — the values the CM5's
    // own project file declares, and a typical 90-100 ohm pair on this
    // stackup family. Without this, pair_geometry silently fell back to
    // single-ended width and 1.5x clearance for EVERY pair.
    let dp_width = d.diff_pair_width.unwrap_or(0.2);
    let dp_gap = d.diff_pair_gap.unwrap_or(0.25);
    let rule = NetClassRules {
        name: DIFF_PAIR_CLASS.to_string(),
        trace_width: dp_width,
        clearance: d.clearance,
        // Pairs transition on the smallest legal via: the microvia class the
        // board's rules carry (via_diameter on calibrated imports is already
        // the modal microvia).
        via_diameter: d.via_diameter,
        via_drill: d.via_drill,
        diff_pair_gap: Some(dp_gap),
        diff_pair_width: Some(dp_width),
        // Impedance targets are carried through from the board's default rules
        // when it declares them; they are never invented here. Absent a target,
        // the per-layer impedance solver stays inert and the declared geometry
        // routes on every layer exactly as before.
        target_impedance: d.target_impedance,
        target_diff_impedance: d.target_diff_impedance,
    };
    match pcb
        .rules
        .class_rules
        .iter_mut()
        .find(|r| r.name == DIFF_PAIR_CLASS)
    {
        Some(existing) => *existing = rule,
        None => pcb.rules.class_rules.push(rule),
    }
    let members: Vec<String> = classifier
        .pairs
        .iter()
        .flat_map(|(p, n)| [p.clone(), n.clone()])
        .collect();
    pcb.rules
        .net_class_assignments
        .insert(DIFF_PAIR_CLASS.to_string(), members);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn pairs_found_by_suffix_and_usb_convention() {
        let nets = names(&[
            "/PCIe.TX_P",
            "/PCIe.TX_N",
            "/USB3-0.DP",
            "/USB3-0.DM",
            "/MDIO.MDC",
            "/LPDDR4 RAM/CK0_T",
            "/LPDDR4 RAM/CK0_C",
        ]);
        let c = classify_nets(&nets);
        assert_eq!(c.pairs.len(), 3);
        assert!(c.is_pair_member("/PCIe.TX_N"));
        assert!(c.is_pair_member("/USB3-0.DM"));
        assert!(c.is_pair_member("/LPDDR4 RAM/CK0_C"));
        assert!(!c.is_pair_member("/MDIO.MDC"));
    }

    #[test]
    fn ddr_and_rgmii_match_groups() {
        let nets = names(&[
            "/LPDDR4 RAM/CA0_A",
            "/LPDDR4 RAM/CA1_A",
            "/LPDDR4 RAM/CA0_B",
            "/LPDDR4 RAM/CA1_B",
            "/LPDDR4 RAM/DQ0",
            "/LPDDR4 RAM/DQ7",
            "/LPDDR4 RAM/DQ8",
            "/RGMII.TXD0",
            "/RGMII.TXC",
        ]);
        let c = classify_nets(&nets);
        assert_eq!(c.match_groups["lpddr4-ca-A"].len(), 2);
        assert_eq!(c.match_groups["lpddr4-ca-B"].len(), 2);
        assert_eq!(c.match_groups["lpddr4-dq-lane0"].len(), 2);
        assert!(!c.match_groups.contains_key("lpddr4-dq-lane1"));
        assert_eq!(c.match_groups["rgmii"].len(), 2);
    }

    #[test]
    fn apply_creates_diff_pair_class_with_board_geometry() {
        let mut pcb: Pcb = serde_json::from_value(serde_json::json!({
            "outline": {"vertices": [], "cutouts": [], "thickness": 1.6},
            "stackup": {"layers": []},
            "nets": [],
            "rules": {
                "defaultRules": {"name": "Default", "traceWidth": 0.08, "clearance": 0.08,
                                  "viaDiameter": 0.21, "viaDrill": 0.12},
                "edgeClearance": 0.2, "holeToHole": 0.2, "minAnnularRing": 0.05, "minDrill": 0.1
            },
            "footprints": [], "traces": [], "traceArcs": [], "vias": [], "zones": []
        }))
        .expect("test board");
        pcb.rules.default_rules.trace_width = 0.08;
        pcb.rules.default_rules.diff_pair_width = Some(0.2);
        pcb.rules.default_rules.diff_pair_gap = Some(0.25);
        let c = classify_nets(&names(&["/PCIe.TX_P", "/PCIe.TX_N"]));
        apply_classes(&mut pcb, &c);
        let rule = pcb
            .rules
            .class_rules
            .iter()
            .find(|r| r.name == DIFF_PAIR_CLASS)
            .expect("class created");
        assert_eq!(rule.trace_width, 0.2);
        assert_eq!(rule.diff_pair_gap, Some(0.25));
        let members = &pcb.rules.net_class_assignments[DIFF_PAIR_CLASS];
        assert!(members.contains(&"/PCIe.TX_N".to_string()));
    }
}
