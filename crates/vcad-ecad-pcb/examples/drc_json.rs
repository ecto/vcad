//! Full DRC over a .pcb.json board — the mutual-legality check for copper
//! committed outside a live RouteSession (e.g. cm5_verdict's joint paths).
//!
//! ```bash
//! cargo run --release -p vcad-ecad-pcb --example drc_json -- board.pcb.json [focus_net_substr]
//! ```

use std::collections::BTreeMap;
use vcad_ecad_pcb::drc::{check_drc, DrcRuleType};
use vcad_ir::ecad::Pcb;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: drc_json <board.pcb.json> [focus]");
    let focus = args.next();
    let pcb: Pcb =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("parse");
    let violations = check_drc(&pcb);
    let mut by_rule: BTreeMap<String, usize> = BTreeMap::new();
    let mut hard = 0usize;
    for v in &violations {
        *by_rule.entry(format!("{:?}", v.rule)).or_default() += 1;
        if matches!(v.rule, DrcRuleType::Short | DrcRuleType::Clearance) {
            hard += 1;
            let show = focus
                .as_deref()
                .map(|f| v.message.contains(f))
                .unwrap_or(true);
            if show && hard <= 40 {
                println!("HARD: {}", &v.message[..v.message.len().min(160)]);
            }
        }
    }
    println!("\nby rule: {by_rule:?}");
    println!(
        "== DRC: {} violations, {hard} short/clearance ==",
        violations.len()
    );
}
