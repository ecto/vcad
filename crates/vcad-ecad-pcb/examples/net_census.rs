//! What fraction of a board's nets are actually connected — the electrical
//! scoreboard, as the `UnconnectedNet` DRC rule judges it.
//!
//! ```bash
//! cargo run --release -p vcad-ecad-pcb --example net_census -- board.pcb.json
//! ```
//!
//! Use this rather than the router's `routability`, which measures the fraction
//! of attempted *connections* that closed and is computed before `si_finish`
//! rips and prunes. The two answer different questions and can disagree wildly:
//! the full CM5 read 0.988 routability on a board where 254 of 408 nets were
//! unconnected.
//!
//! The `missing connections` histogram is the actionable part — `pad_groups - 1`
//! is how many connections a net still needs, so it separates "one hop short"
//! from "never started".

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: net_census <board.pcb.json>");
        std::process::exit(2);
    });
    let pcb: vcad_ir::ecad::Pcb =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read board"))
            .expect("parse board json");
    let planes: std::collections::BTreeSet<&str> = pcb
        .zones
        .iter()
        .filter(|z| !z.net.is_empty())
        .map(|z| z.net.as_str())
        .collect();
    let census = vcad_ecad_pcb::drc::net_pad_groups(&pcb);

    for (label, want_plane) in [("signal", false), ("plane ", true)] {
        let group: Vec<_> = census
            .iter()
            .filter(|n| planes.contains(n.net.as_str()) == want_plane)
            .collect();
        let total = group.len();
        if total == 0 {
            continue;
        }
        let connected = group.iter().filter(|n| n.pad_groups <= 1).count();
        let bare = group
            .iter()
            .filter(|n| n.pad_groups > 1 && n.copper == 0)
            .count();
        println!(
            "{label}: {connected}/{total} connected ({:.3}), {} partial, {bare} with no copper",
            connected as f64 / total as f64,
            total - connected - bare,
        );
        let mut hist: std::collections::BTreeMap<usize, usize> = Default::default();
        for n in group.iter().filter(|n| n.pad_groups > 1) {
            *hist.entry(n.pad_groups - 1).or_default() += 1;
        }
        println!(
            "         missing connections: {}",
            hist.iter()
                .map(|(m, c)| format!("{m}x{c}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
}
