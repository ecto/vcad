//! CM5 ground-truth benchmark: import a routed KiCad board (the reference
//! fixture is schlae/cm5-reveng's 10-layer Raspberry Pi CM5 reverse
//! engineering), strip its human-routed copper, autoroute, and score against
//! the human layout — completion, via count, and total copper length.
//!
//! ```bash
//! cargo run --release -p vcad-ecad-pcb --example cm5_bench -- \
//!     CM5RevEng.kicad_pcb [effort] [max_nets] [out.pcb.json]
//! ```
//!
//! `max_nets` (default all) routes only the N highest-pad-count nets — handy
//! for a quick smoke run on the full board. `out.pcb.json` saves the board
//! with the routed copper applied, so rendering (vcad-render's `cm5_render`)
//! and styling iteration never pay for another routing run.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use vcad_ecad_pcb::router::classes::{apply_classes, classify_nets};
use vcad_ecad_pcb::router::length_match::net_routed_length;
use vcad_ecad_pcb::router::{route_all_with_opts, RouteOptions};
use vcad_ecad_symbols::parse_kicad_pcb;
use vcad_ir::ecad::{Trace, Via};

fn seg_len(a: vcad_ir::Vec2, b: vcad_ir::Vec2) -> f64 {
    ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt()
}

fn main() {
    // RUST_LOG=info for round/batch progress, debug for per-batch and rip-up
    // detail, trace for every search. Timestamped to correlate with `top`.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| {
        eprintln!("usage: cm5_bench <board.kicad_pcb> [effort] [max_nets]");
        std::process::exit(2);
    });
    let effort: f64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(1.0);
    let max_nets: usize = args
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(usize::MAX);
    let out_json = args.next();
    let priority: Vec<String> = args
        .next()
        .map(|p| {
            serde_json::from_str(&std::fs::read_to_string(p).expect("read priority"))
                .expect("parse priority")
        })
        .unwrap_or_default();

    let text = std::fs::read_to_string(&path).expect("read board file");
    // Resume mode: a .pcb.json saved by a previous run loads with its routed
    // copper intact — the session seeds from it, the ratsnest re-lists only
    // the still-unrouted nets, and the run costs minutes instead of hours.
    let resume = path.ends_with(".json");
    let mut pcb = if resume {
        serde_json::from_str(&text).expect("parse pcb json")
    } else if path.ends_with(".brd") {
        vcad_ecad_symbols::parse_eagle_brd(&text).expect("parse eagle brd")
    } else {
        parse_kicad_pcb(&text).expect("parse kicad_pcb")
    };

    // Ground truth: the human routing we are about to strip.
    let human_traces = pcb.traces.len();
    let human_vias = pcb.vias.len();
    let human_len: f64 = pcb.traces.iter().map(|t| seg_len(t.start, t.end)).sum();
    let human_nets: BTreeSet<&str> = pcb.traces.iter().map(|t| t.net.as_str()).collect();

    println!(
        "board: {} copper layers, {} nets, {} footprints, {} pads",
        pcb.stackup
            .layers
            .iter()
            .filter(|l| l.layer.is_copper())
            .count(),
        pcb.nets.len(),
        pcb.footprints.len(),
        pcb.footprints.iter().map(|f| f.pads.len()).sum::<usize>(),
    );
    println!(
        "human: {human_traces} segments, {human_vias} vias, {:.0} mm copper, {} routed nets",
        human_len,
        human_nets.len()
    );

    // Strip everything the autorouter is expected to produce (fresh runs
    // only — resume keeps its own previous routing). Zones stay: a poured
    // plane is design intent (the router stitches to it, not through it).
    if !resume {
        pcb.traces.clear();
        pcb.trace_arcs.clear();
        pcb.vias.clear();
    }

    // Optional subset: the N nets with the most pads (the hard ones first).
    // Keyed by the pad net strings — the exact names the router's pad-derived
    // netlist and ratsnest match on (`pcb.nets` ids can differ from pad net
    // names on imported boards, which made the filter silently select nothing).
    let filter: Vec<String> = if max_nets == usize::MAX {
        Vec::new()
    } else {
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for p in pcb.footprints.iter().flat_map(|f| f.pads.iter()) {
            if let Some(net) = p.net.as_deref() {
                if !net.is_empty() {
                    *counts.entry(net).or_default() += 1;
                }
            }
        }
        let mut counts: Vec<(&str, usize)> = counts.into_iter().filter(|(_, c)| *c >= 2).collect();
        counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        counts
            .into_iter()
            .take(max_nets)
            .map(|(n, _)| n.to_string())
            .collect()
    };

    // Recover electrical intent from net names (diff pairs, match groups)
    // and realize it as class rules — the SI constraint foundation.
    let all_nets: Vec<String> = {
        let mut v: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for f in &pcb.footprints {
            for pad in &f.pads {
                if let Some(n) = &pad.net {
                    if !n.is_empty() {
                        v.insert(n.clone());
                    }
                }
            }
        }
        v.into_iter().collect()
    };
    let classifier = classify_nets(&all_nets);
    apply_classes(&mut pcb, &classifier);
    println!(
        "classes: {} diff pairs, {} match groups",
        classifier.pairs.len(),
        classifier.match_groups.len()
    );

    let width = pcb.rules.default_rules.trace_width;
    // VCAD_POUR_SYNTH=0 routes the board exactly as authored — the A/B control
    // for what copper-pour synthesis is worth on a given fixture.
    let mut pour_policy = vcad_ecad_pcb::pour_synth::PourPolicy::default();
    if std::env::var("VCAD_POUR_SYNTH").is_ok_and(|v| v == "0") {
        pour_policy.enabled = false;
        println!("pours: synthesis DISABLED (VCAD_POUR_SYNTH=0)");
    }
    let t0 = Instant::now();
    let r = route_all_with_opts(
        &pcb,
        width,
        &filter,
        &RouteOptions {
            effort,
            priority_nets: priority.clone(),
            pour_policy,
            ..Default::default()
        },
    );
    let elapsed = t0.elapsed();

    let routed_len: f64 = r.traces.iter().map(|t| seg_len(t.start, t.end)).sum();

    println!(
        "vcad:  {} segments, {} vias, {:.0} mm copper, {} routed / {} unrouted nets",
        r.traces.len(),
        r.vias.len(),
        routed_len,
        r.routed_nets.len(),
        r.unrouted_nets.len(),
    );
    if !r.zones.is_empty() {
        let mut by_net: BTreeMap<&str, usize> = BTreeMap::new();
        for z in &r.zones {
            *by_net.entry(z.net.as_str()).or_default() += 1;
        }
        println!(
            "pours: {} synthesized zone(s) over {} net(s): {}",
            r.zones.len(),
            by_net.len(),
            by_net
                .iter()
                .map(|(n, c)| format!("{n}x{c}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    println!(
        "score: routability {:.3}, via ratio {:.2}x human, length ratio {:.2}x human, {:.1}s",
        r.routability,
        if human_vias > 0 {
            r.vias.len() as f64 / human_vias as f64
        } else {
            f64::NAN
        },
        if human_len > 0.0 {
            routed_len / human_len
        } else {
            f64::NAN
        },
        elapsed.as_secs_f64(),
    );
    for d in r.diagnostics.iter().take(10) {
        eprintln!("unrouted {}: {}", d.net, d.reason);
    }

    // Apply the routed copper to the board, then run the SI finishing pass
    // (reroute-then-descend). Both of its stages are non-regressive and
    // oracle-gated, so this can only improve the pair claims — and it runs
    // here, inside the route, so a *freshly routed* board is the one the
    // receipt is measured on.
    //
    // Synthesized pours go on first: the routing above assumes them (a poured
    // net is carried by its plane, so its pads were stitched rather than traced
    // to each other), and the SI pass reroutes against this board, so it has to
    // see the planes too.
    pcb.zones.extend(r.zones.iter().cloned());
    for t in &r.traces {
        pcb.traces.push(Trace {
            start: t.start,
            end: t.end,
            width: t.width,
            layer: t.layer,
            net: t.net.clone(),
            source: None,
        });
    }
    for v in &r.vias {
        pcb.vias.push(Via {
            position: v.position,
            diameter: pcb.rules.default_rules.via_diameter,
            drill: pcb.rules.default_rules.via_drill,
            start_layer: v.start_layer,
            end_layer: v.end_layer,
            net: v.net.clone(),
            source: None,
        });
    }
    if std::env::var("VCAD_SI_FINISH").as_deref() != Ok("0") {
        let t1 = Instant::now();
        let fin = vcad_ecad_pcb::router::si_finish(&mut pcb, 2_000_000, 2000);
        println!(
            "si-finish: {}/{} pairs re-coupled, {}/{} descended ({} rejected), {:.1}s",
            fin.polished,
            fin.polish_attempted,
            fin.descent.tuned,
            fin.descent.attempted,
            fin.descent.rejected,
            t1.elapsed().as_secs_f64(),
        );
    }

    // SI scoreboard: skew per length-match group and per differential pair,
    // measured on the routed copper. This is the gap the meander tuner must
    // close — and the number the human board is matched to within microns.
    {
        let with_routes = pcb.clone();
        for (gname, members) in &classifier.match_groups {
            let lens: Vec<(f64, &str)> = members
                .iter()
                .map(|n| (net_routed_length(&with_routes, n), n.as_str()))
                .filter(|(l, _)| *l > 0.0)
                .collect();
            if lens.len() > 1 {
                let max = lens.iter().map(|(l, _)| *l).fold(f64::MIN, f64::max);
                let min = lens.iter().map(|(l, _)| *l).fold(f64::MAX, f64::min);
                println!(
                    "si-skew group {gname}: {} nets, {:.2}..{:.2} mm, skew {:.2} mm",
                    lens.len(),
                    min,
                    max,
                    max - min
                );
            }
        }
        let mut pair_worst: f64 = 0.0;
        let mut pair_measured = 0usize;
        for (p, n) in &classifier.pairs {
            let (lp, ln) = (
                net_routed_length(&with_routes, p),
                net_routed_length(&with_routes, n),
            );
            if lp > 0.0 && ln > 0.0 {
                pair_measured += 1;
                pair_worst = pair_worst.max((lp - ln).abs());
            }
        }
        println!("si-skew pairs: {pair_measured} measured, worst intra-pair {pair_worst:.3} mm");
    }

    // Save the routed board for rendering / inspection without re-routing.
    // The copper is already applied to `pcb` above (the SI finishing pass
    // rewrites some of it, so it has to be applied before that runs).
    if let Some(out) = out_json {
        std::fs::write(&out, serde_json::to_string(&pcb).expect("serialize board"))
            .expect("write routed board json");
        eprintln!("wrote {out}");
    }
}
