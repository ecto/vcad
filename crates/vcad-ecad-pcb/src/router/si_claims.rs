//! Signal-integrity claims for the design receipt — `vcad.si-claims/1`.
//!
//! Measures the routed copper itself (no simulation): per-group length skew,
//! intra-pair skew, differential coupled-length fraction, and SI-net via
//! counts, each judged against explicit bounds. These are `basis: "verified"`
//! claims in the [`vcad-receipt`] sense — the oracle (geometric measurement
//! over the actual board copper) ran for real. What they do NOT claim is
//! spelled out per-claim: no impedance verification (analytic tables are
//! `predicted`, emitted separately), no crosstalk, no eye simulation.
//!
//! The bound defaults come from the traced Raspberry Pi CM5 production board
//! — hardware that demonstrably boots — making a passing claim set an
//! *envelope* argument: the copper stays within the measured discipline of a
//! known-working reference.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use vcad_ir::ecad::Pcb;
use vcad_ir::Vec2;

use super::classes::NetClassifier;
use super::length_match::net_routed_length;

/// Schema tag for this claim family.
pub const CLAIM_SCHEMA: &str = "vcad.si-claims/1";

/// Domain tag in the unified `vcad.receipt/1` schema.
pub const RECEIPT_DOMAIN: &str = "ecad-si";

/// Bounds a claim set is judged against. Defaults are the human CM5
/// envelope measured by `si_report` (raw trace-length metric).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SiBounds {
    /// Max allowed length skew within a match group (mm).
    pub group_skew_mm: f64,
    /// Max allowed intra-pair skew (mm).
    pub pair_skew_mm: f64,
    /// Min required coupled-length fraction per pair (0..1).
    pub min_coupled_fraction: f64,
    /// Max vias per SI net, averaged.
    pub max_vias_per_si_net: f64,
    /// Max differential pairs allowed to carry copper without being routed
    /// pad-to-pad. Guards the other pair claims: they are worst-case over
    /// *measured* pairs, so without this a board could pass them by leaving the
    /// hard pairs as stubs.
    pub max_incomplete_pairs: f64,
}

impl Default for SiBounds {
    fn default() -> Self {
        Self {
            // Human board: DDR groups reach ~10mm raw skew (multi-drop);
            // RGMII 3.2mm. 10mm is the reference envelope, not a target.
            group_skew_mm: 10.0,
            // Human worst intra-pair: 1.074mm.
            pair_skew_mm: 1.1,
            // Aspirational floor; the human couples essentially everywhere
            // outside escape zones.
            min_coupled_fraction: 0.5,
            // Human: 222 vias / 98 SI nets ≈ 2.3.
            max_vias_per_si_net: 3.0,
            // A pair that is not routed pad-to-pad is a routing failure, not an
            // SI margin, so the only defensible bound is zero.
            max_incomplete_pairs: 0.0,
        }
    }
}

/// One measured claim.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Claim {
    /// Claim name, snake_case.
    pub name: String,
    /// Measured value.
    pub value: f64,
    /// Unit ("1" for dimensionless).
    pub unit: String,
    /// Bound the value was judged against.
    pub bound: f64,
    /// Whether the value is within the bound.
    pub holds: bool,
    /// `"verified"` — the geometric oracle ran over the actual copper.
    pub basis: String,
    /// What was measured and what is NOT claimed.
    pub note: String,
}

/// The full claim set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SiClaimSet {
    /// Schema tag ([`CLAIM_SCHEMA`]).
    pub schema: String,
    /// Bounds used.
    pub bounds: SiBounds,
    /// The claims.
    pub claims: Vec<Claim>,
    /// True when every claim holds.
    pub all_hold: bool,
}

fn seg_len(a: Vec2, b: Vec2) -> f64 {
    ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt()
}

/// Fraction of its own pad span a leg's copper must reach before the receipt is
/// willing to measure skew or coupling against it.
///
/// Calibrated, like every bound in this module, to the human CM5: its thinnest
/// leg is `/LPDDR4 RAM/DQS1_C_A` at 0.522 (6.81mm of copper across a 13.04mm
/// span), so 0.5 is the largest round threshold the anchor clears — a ~4%
/// margin, which is thin and is the reason this is a screen for *absent* copper
/// rather than a length check.
///
/// It cannot be 1.0 even though a connected path must span the pads: copper runs
/// pad-edge to pad-edge, not centre to centre, so plenty of correctly routed
/// legs measure a hair under their centre-to-centre span (`/LPDDR4 RAM/DQS0_T_B`
/// at 0.999, `/HDMI0.TX0_P` at 0.994).
const MIN_LEG_SPAN_FRACTION: f64 = 0.5;

/// True when a net holds far less copper than it would take to join its own pads
/// — so it cannot be routed, whatever else is true of it.
///
/// Any connected copper joining a set of pads is at least as long as the
/// farthest pair of those pads, so a large shortfall is evidence of
/// non-connection rather than of a tight route. That structure matters here
/// because the CM5 fixture is reverse-engineered: its imported copper has
/// sub-tolerance gaps that make DRC continuity report 25 of the *human* board's
/// 49 pairs as discontinuous, which would disqualify the calibration anchor.
/// This test reads only copper length and pad positions, so it is immune to
/// that. Measured at [`MIN_LEG_SPAN_FRACTION`]: 0 human legs fail, 7 of ours do,
/// the worst being `/USB3-0.TX_P` with 0.23mm of copper against an 18.36mm span.
///
/// It is a *necessary* condition, not a sufficient one: a leg with ample copper
/// can still be open. `UnconnectedNet` in the DRC remains the authoritative
/// completeness check; this exists so the receipt refuses to *measure* legs that
/// are plainly not routes.
fn leg_copper_is_starved(pcb: &Pcb, net: &str) -> bool {
    let mut pads: Vec<Vec2> = Vec::new();
    for fp in &pcb.footprints {
        for pad in &fp.pads {
            if pad.net.as_deref() == Some(net) {
                pads.push(crate::geometry::pad_world_position(fp, pad));
            }
        }
    }
    let mut span = 0.0f64;
    for (i, a) in pads.iter().enumerate() {
        for b in &pads[i + 1..] {
            span = span.max((*a - *b).length());
        }
    }
    // Sub-millimetre spans are escape-scale: the metric has no resolution there
    // and the `/HS.*` pairs live at ~1.5mm, so only judge spans worth judging.
    if span <= 1.0 {
        return false;
    }
    net_routed_length(pcb, net) < span * MIN_LEG_SPAN_FRACTION
}

/// Fraction of `p_net`'s routed length that runs coupled to `n_net`: sampled
/// at each P segment midpoint, coupled means some same-layer N segment passes
/// within `max_sep` (center-to-center).
///
/// Note the asymmetry, which matters when reading a high value: the denominator
/// is P's own length, so a short P beside a long N scores near 1.0. Callers that
/// need "these two legs run together" must also check the legs are comparable in
/// length — [`si_claims`] gates on [`leg_copper_is_starved`] first for exactly
/// this reason.
///
/// Exposed as `router::pair_coupled_fraction` so reports can name the pairs
/// that break `min_pair_coupled_fraction` — the claim is a minimum over every
/// routed pair, so the aggregate alone never says which one to fix.
pub fn coupled_fraction(pcb: &Pcb, p_net: &str, n_net: &str, max_sep: f64) -> f64 {
    let p_segs: Vec<_> = pcb.traces.iter().filter(|t| t.net == p_net).collect();
    let n_segs: Vec<_> = pcb.traces.iter().filter(|t| t.net == n_net).collect();
    if p_segs.is_empty() || n_segs.is_empty() {
        return 0.0;
    }
    let point_seg_dist = |p: Vec2, a: Vec2, b: Vec2| -> f64 {
        let ab = Vec2::new(b.x - a.x, b.y - a.y);
        let l2 = ab.x * ab.x + ab.y * ab.y;
        if l2 < 1e-18 {
            return seg_len(p, a);
        }
        let t = (((p.x - a.x) * ab.x + (p.y - a.y) * ab.y) / l2).clamp(0.0, 1.0);
        seg_len(p, Vec2::new(a.x + ab.x * t, a.y + ab.y * t))
    };
    let (mut coupled, mut total) = (0.0f64, 0.0f64);
    for ps in &p_segs {
        let l = seg_len(ps.start, ps.end);
        total += l;
        let mid = Vec2::new((ps.start.x + ps.end.x) / 2.0, (ps.start.y + ps.end.y) / 2.0);
        let near = n_segs
            .iter()
            .filter(|ns| ns.layer == ps.layer)
            .any(|ns| point_seg_dist(mid, ns.start, ns.end) <= max_sep);
        if near {
            coupled += l;
        }
    }
    if total > 0.0 {
        coupled / total
    } else {
        0.0
    }
}

/// Measure the board against `bounds` and emit the claim set.
pub fn si_claims(pcb: &Pcb, classifier: &NetClassifier, bounds: &SiBounds) -> SiClaimSet {
    let mut claims = Vec::new();

    // Worst group skew across all match groups (routed members only).
    let mut worst_group = (0.0f64, String::new());
    for (gname, members) in &classifier.match_groups {
        let lens: Vec<f64> = members
            .iter()
            .map(|n| net_routed_length(pcb, n))
            .filter(|l| *l > 0.0)
            .collect();
        if lens.len() > 1 {
            let skew = lens.iter().cloned().fold(f64::MIN, f64::max)
                - lens.iter().cloned().fold(f64::MAX, f64::min);
            if skew > worst_group.0 {
                worst_group = (skew, gname.clone());
            }
        }
    }
    claims.push(Claim {
        name: "worst_group_skew".into(),
        value: worst_group.0,
        unit: "mm".into(),
        bound: bounds.group_skew_mm,
        holds: worst_group.0 <= bounds.group_skew_mm,
        basis: "verified".into(),
        note: format!(
            "raw trace-length skew over match groups, worst = {} — multi-drop \
             branch structure and package delays NOT modeled",
            if worst_group.1.is_empty() {
                "none"
            } else {
                &worst_group.1
            }
        ),
    });

    // Intra-pair skew + coupled fraction, worst over measured pairs.
    let (mut worst_pair_skew, mut worst_coupled) = (0.0f64, 1.0f64);
    let mut measured_pairs = 0usize;
    let gap = pcb.rules.default_rules.diff_pair_gap.unwrap_or(0.25);
    let w = pcb
        .rules
        .default_rules
        .diff_pair_width
        .unwrap_or(pcb.rules.default_rules.trace_width);
    let max_sep = (w + gap) * 1.75;
    let mut incomplete_pairs = 0usize;
    for (p, n) in &classifier.pairs {
        let (lp, ln) = (net_routed_length(pcb, p), net_routed_length(pcb, n));
        if lp > 0.0 && ln > 0.0 {
            // Fail closed: a leg holding less copper than its own pad span is
            // provably not a route, so neither skew nor coupled fraction means
            // anything against it. Count it — silently dropping such a pair
            // would let a board pass this receipt by leaving the hard pairs as
            // stubs, and `coupled_fraction` normalizes by P alone, so a stub
            // lying beside a full twin scores 1.000.
            if leg_copper_is_starved(pcb, p) || leg_copper_is_starved(pcb, n) {
                incomplete_pairs += 1;
                continue;
            }
            measured_pairs += 1;
            worst_pair_skew = worst_pair_skew.max((lp - ln).abs());
            worst_coupled = worst_coupled.min(coupled_fraction(pcb, p, n, max_sep));
        }
    }
    claims.push(Claim {
        name: "worst_intra_pair_skew".into(),
        value: worst_pair_skew,
        unit: "mm".into(),
        bound: bounds.pair_skew_mm,
        holds: worst_pair_skew <= bounds.pair_skew_mm,
        basis: "verified".into(),
        note: format!(
            "over {measured_pairs} fully-routed pairs; {incomplete_pairs} pair(s) \
             excluded as not routed pad-to-pad (see si_pairs_incomplete); \
             time-of-flight NOT converted"
        ),
    });
    claims.push(Claim {
        name: "si_pairs_incomplete".into(),
        value: incomplete_pairs as f64,
        unit: "1".into(),
        bound: bounds.max_incomplete_pairs,
        holds: (incomplete_pairs as f64) <= bounds.max_incomplete_pairs,
        basis: "verified".into(),
        note: "differential pairs holding copper but less of it than their own pad \
               span requires on at least one leg, so provably not routed and \
               excluded from the skew and coupling claims"
            .into(),
    });
    claims.push(Claim {
        name: "min_pair_coupled_fraction".into(),
        value: worst_coupled,
        unit: "1".into(),
        bound: bounds.min_coupled_fraction,
        holds: worst_coupled >= bounds.min_coupled_fraction,
        basis: "verified".into(),
        note: "fraction of P length with same-layer N copper within 1.75x pitch; \
               impedance NOT verified by this claim"
            .into(),
    });

    // Vias per SI net.
    let si_nets: Vec<&String> = classifier.pairs.iter().flat_map(|(p, n)| [p, n]).collect();
    let mut via_count = 0usize;
    let mut routed_si = 0usize;
    for net in &si_nets {
        let vias = pcb.vias.iter().filter(|v| &v.net == *net).count();
        if net_routed_length(pcb, net) > 0.0 {
            routed_si += 1;
            via_count += vias;
        }
    }
    let vias_per_net = if routed_si > 0 {
        via_count as f64 / routed_si as f64
    } else {
        0.0
    };
    claims.push(Claim {
        name: "vias_per_si_net".into(),
        value: vias_per_net,
        unit: "1".into(),
        bound: bounds.max_vias_per_si_net,
        holds: vias_per_net <= bounds.max_vias_per_si_net,
        basis: "verified".into(),
        note: format!(
            "{via_count} vias over {routed_si} routed SI nets; each via is a reference \
             change — return-path continuity NOT verified"
        ),
    });

    let all_hold = claims.iter().all(|c| c.holds);
    SiClaimSet {
        schema: CLAIM_SCHEMA.into(),
        bounds: bounds.clone(),
        claims,
        all_hold,
    }
}

/// Per-group skew table (name → (net_count, skew_mm)), for reports.
pub fn group_skews(pcb: &Pcb, classifier: &NetClassifier) -> BTreeMap<String, (usize, f64)> {
    let mut out = BTreeMap::new();
    for (gname, members) in &classifier.match_groups {
        let lens: Vec<f64> = members
            .iter()
            .map(|n| net_routed_length(pcb, n))
            .filter(|l| *l > 0.0)
            .collect();
        if lens.len() > 1 {
            let skew = lens.iter().cloned().fold(f64::MIN, f64::max)
                - lens.iter().cloned().fold(f64::MAX, f64::min);
            out.insert(gname.clone(), (lens.len(), skew));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::router::classes::classify_nets;
    use vcad_ir::ecad::{PcbLayer, Trace};

    fn board_with(traces: Vec<Trace>) -> Pcb {
        let mut pcb: Pcb = serde_json::from_value(serde_json::json!({
            "outline": {"vertices": [], "cutouts": [], "thickness": 1.6},
            "stackup": {"layers": []},
            "nets": [],
            "rules": {
                "defaultRules": {"name": "Default", "traceWidth": 0.08, "clearance": 0.08,
                                  "viaDiameter": 0.21, "viaDrill": 0.12,
                                  "diffPairWidth": 0.2, "diffPairGap": 0.25},
                "edgeClearance": 0.2, "holeToHole": 0.2, "minAnnularRing": 0.05, "minDrill": 0.1
            },
            "footprints": [], "traces": [], "traceArcs": [], "vias": [], "zones": []
        }))
        .expect("test board");
        pcb.traces = traces;
        pcb
    }

    fn seg(net: &str, y: f64, x0: f64, x1: f64) -> Trace {
        Trace {
            start: Vec2::new(x0, y),
            end: Vec2::new(x1, y),
            width: 0.2,
            layer: PcbLayer::FCu,
            net: net.into(),
            source: None,
        }
    }

    #[test]
    fn coupled_pair_passes_uncoupled_fails() {
        // Tight pair: legs 0.45mm apart over the full run.
        let tight = board_with(vec![
            seg("/X.TX_P", 0.0, 0.0, 20.0),
            seg("/X.TX_N", 0.45, 0.0, 20.0),
        ]);
        let c = classify_nets(&["/X.TX_P".into(), "/X.TX_N".into()]);
        let set = si_claims(&tight, &c, &SiBounds::default());
        assert!(set.all_hold, "{:?}", set.claims);

        // Split pair: legs 5mm apart — coupled fraction collapses.
        let split = board_with(vec![
            seg("/X.TX_P", 0.0, 0.0, 20.0),
            seg("/X.TX_N", 5.0, 0.0, 20.0),
        ]);
        let set = si_claims(&split, &c, &SiBounds::default());
        let cf = set
            .claims
            .iter()
            .find(|cl| cl.name == "min_pair_coupled_fraction")
            .unwrap();
        assert!(!cf.holds);
        assert!(!set.all_hold);
    }

    #[test]
    fn pair_skew_judged_against_bound() {
        let b = board_with(vec![
            seg("/X.TX_P", 0.0, 0.0, 20.0),
            seg("/X.TX_N", 0.45, 0.0, 25.0),
        ]);
        let c = classify_nets(&["/X.TX_P".into(), "/X.TX_N".into()]);
        let set = si_claims(&b, &c, &SiBounds::default());
        let skew = set
            .claims
            .iter()
            .find(|cl| cl.name == "worst_intra_pair_skew")
            .unwrap();
        assert!((skew.value - 5.0).abs() < 1e-9);
        assert!(!skew.holds, "5mm > 1.1mm bound");
    }
}

/// Bridge into the unified `vcad.receipt/1` schema: every SI claim becomes a
/// [`ReceiptClaim`] with `basis: Measured` — the geometric oracle ran over
/// the actual board copper — under the [`RECEIPT_DOMAIN`] domain. Broken
/// bounds are `Fail` claims, not omissions: the receipt is fail-closed.
pub fn to_receipt_claims(set: &SiClaimSet) -> Vec<vcad_receipt::ReceiptClaim> {
    use vcad_receipt::{ClaimBasis, ClaimQuantity, OracleRef, ReceiptClaim};
    let oracle = OracleRef::new("vcad-ecad-pcb::si_claims", env!("CARGO_PKG_VERSION"));
    set.claims
        .iter()
        .map(|c| {
            let desc = format!("{} <= {} {} — {}", c.name, c.bound, c.unit, c.note);
            let id = format!("si.{}", c.name);
            let base = if c.holds {
                ReceiptClaim::pass(id, RECEIPT_DOMAIN, desc, oracle.clone())
            } else {
                ReceiptClaim::fail(id, RECEIPT_DOMAIN, desc, oracle.clone())
            };
            base.with_basis(ClaimBasis::Measured)
                .with_measured(ClaimQuantity::new(c.value, c.unit.clone()))
        })
        .collect()
}
