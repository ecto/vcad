//! fab_prep-style DRC delta for a routed `.pcb.json`: violations on the board
//! vs the same board stripped of every trace/arc/via (zones kept), per rule,
//! plus the connectivity counts (UnconnectedNet + NetIslands) on both.
//!
//! ```bash
//! cargo run --release -p vcad-ecad-fabprep --example drc_delta -- routed.pcb.json [offenders.txt]
//! ```

use std::collections::BTreeSet;

use vcad_ecad_fabprep::delta::DrcDelta;
use vcad_ecad_pcb::drc::{check_drc, DrcRuleType};
use vcad_ir::ecad::Pcb;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .expect("usage: drc_delta <board.pcb.json> [offenders.txt]");
    let offenders_out = args.next();
    let pcb: Pcb =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("parse");
    let mut stripped = pcb.clone();
    stripped.traces.clear();
    stripped.trace_arcs.clear();
    stripped.vias.clear();
    let base = check_drc(&stripped);
    let fin = check_drc(&pcb);
    let d = DrcDelta::compute(&base, &fin, &BTreeSet::new());
    let conn = |v: &[vcad_ecad_pcb::drc::DrcViolation], r: DrcRuleType| {
        v.iter().filter(|x| x.rule == r).count()
    };
    println!(
        "{:<22} {:>8} {:>8} {:>8} fixable",
        "rule", "stripped", "final", "attrib"
    );
    for r in &d.rules {
        println!(
            "{:<22} {:>8} {:>8} {:>8} {}",
            r.rule, r.baseline, r.final_count, r.route_attributable, r.route_fixable
        );
    }
    println!(
        "TOTAL stripped {} final {} route-attributable {} (fixable {})",
        d.baseline_total, d.final_total, d.route_attributable_total, d.route_attributable_fixable
    );
    println!(
        "connectivity: UnconnectedNet {} -> {}, NetIslands {} -> {}",
        conn(&base, DrcRuleType::UnconnectedNet),
        conn(&fin, DrcRuleType::UnconnectedNet),
        conn(&base, DrcRuleType::NetIslands),
        conn(&fin, DrcRuleType::NetIslands),
    );
    if let Some(p) = offenders_out {
        let s: String = d
            .offenders
            .iter()
            .map(|o| {
                format!(
                    "{} @({:.3},{:.3}) {}\n",
                    o.rule, o.position[0], o.position[1], o.message
                )
            })
            .collect();
        std::fs::write(p, s).expect("write offenders");
    }
}
