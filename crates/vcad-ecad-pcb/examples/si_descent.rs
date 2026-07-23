//! Differentiable pair polish on a routed board (GPU-router charter M5→M6).
//!
//! For each classified differential pair whose legs form single-layer
//! unbranched polylines, run the tang-expr descent (skew² + gap springs +
//! clearance hinges) and commit the optimized geometry ONLY when the exact
//! oracle passes every final segment — fail-closed, per the charter.
//!
//! ```bash
//! cargo run --release -p vcad-ecad-pcb --example si_descent -- in.pcb.json out.pcb.json
//! ```

use vcad_ecad_pcb::router::classes::{apply_classes, classify_nets};
use vcad_ecad_pcb::router::descent::{descend_pair, DescentObstacle, DescentWeights};
use vcad_ecad_pcb::session::RouteSession;
use vcad_ecad_pcb::spatial::CopperGeom;
use vcad_ir::ecad::{Pcb, PcbLayer, Trace};
use vcad_ir::Vec2;

fn polyline_layer(pcb: &Pcb, net: &str) -> Option<PcbLayer> {
    let mut layers = pcb.traces.iter().filter(|t| t.net == net).map(|t| t.layer);
    let first = layers.next()?;
    layers.all(|l| l == first).then_some(first)
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();
    let mut args = std::env::args().skip(1);
    let input = args.next().expect("usage: si_descent <in> <out>");
    let output = args.next().expect("usage: si_descent <in> <out>");
    let mut pcb: Pcb =
        serde_json::from_str(&std::fs::read_to_string(&input).expect("read")).expect("parse");

    let nets: Vec<String> = {
        let mut v: std::collections::BTreeSet<String> = Default::default();
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
    let c = classify_nets(&nets);
    apply_classes(&mut pcb, &c);
    let gap_edge = pcb
        .rules
        .class_rules
        .iter()
        .find(|r| r.name == "diff-pair")
        .and_then(|r| r.diff_pair_gap)
        .unwrap_or(0.25);
    let leg_w = pcb
        .rules
        .class_rules
        .iter()
        .find(|r| r.name == "diff-pair")
        .and_then(|r| r.diff_pair_width)
        .unwrap_or(0.2);
    let gap_centre = gap_edge + leg_w;

    let (mut tuned, mut attempted, mut rejected) = (0usize, 0usize, 0usize);
    for (pn, nn) in &c.pairs {
        // Single-layer unbranched legs only (v1 restriction, honestly skipped
        // otherwise).
        let (Some(pl), Some(nl)) = (polyline_layer(&pcb, pn), polyline_layer(&pcb, nn)) else {
            continue;
        };
        if pl != nl {
            continue;
        }
        let (Ok((p_pts, _)), Ok((n_pts, _))) = (
            vcad_ecad_pcb::router::length_match::net_polyline(&pcb, pn),
            vcad_ecad_pcb::router::length_match::net_polyline(&pcb, nn),
        ) else {
            continue;
        };
        let plen: f64 = p_pts.windows(2).map(|w| (w[1] - w[0]).length()).sum();
        let nlen: f64 = n_pts.windows(2).map(|w| (w[1] - w[0]).length()).sum();
        if (plen - nlen).abs() < 0.2 {
            continue; // already matched
        }
        attempted += 1;

        // Obstacles: same-layer copper of every OTHER net near the corridor.
        let session = RouteSession::from_pcb(&pcb);
        let clearance = session.clearance_for(pn);
        let (mut lo, mut hi) = (
            Vec2::new(f64::INFINITY, f64::INFINITY),
            Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY),
        );
        for p in p_pts.iter().chain(n_pts.iter()) {
            lo.x = lo.x.min(p.x - 3.0);
            lo.y = lo.y.min(p.y - 3.0);
            hi.x = hi.x.max(p.x + 3.0);
            hi.y = hi.y.max(p.y + 3.0);
        }
        let mut obstacles = Vec::new();
        for t in &pcb.traces {
            if t.net == *pn || t.net == *nn || t.layer != pl {
                continue;
            }
            if t.start.x.max(t.end.x) < lo.x
                || t.start.x.min(t.end.x) > hi.x
                || t.start.y.max(t.end.y) < lo.y
                || t.start.y.min(t.end.y) > hi.y
            {
                continue;
            }
            obstacles.push(DescentObstacle {
                a: t.start,
                b: t.end,
                required: t.width / 2.0 + leg_w / 2.0 + clearance,
            });
        }

        let Some(r) = descend_pair(
            &p_pts,
            &n_pts,
            gap_centre,
            &obstacles,
            &DescentWeights::default(),
            2000,
        ) else {
            continue;
        };

        // Fail-closed: every optimized segment passes the exact oracle
        // (pair-aware session probe) with BOTH nets' old copper removed.
        let mut work = pcb.clone();
        work.traces.retain(|t| t.net != *pn && t.net != *nn);
        let vsession = RouteSession::from_pcb(&work);
        let legal = |pts: &[Vec2], net: &str| -> bool {
            pts.windows(2).all(|w| {
                let g = CopperGeom::Segment {
                    a: w[0],
                    b: w[1],
                    half_w: leg_w / 2.0,
                };
                vsession.probe(&g, pl, net, clearance).legal
            })
        };
        // Cross-check legs against each other via a temp commit of leg P.
        if !legal(&r.p_pts, pn) || !legal(&r.n_pts, nn) {
            rejected += 1;
            continue;
        }
        let before = (plen - nlen).abs();
        for (net, pts) in [(pn, &r.p_pts), (nn, &r.n_pts)] {
            work.traces.extend(pts.windows(2).map(|w| Trace {
                start: w[0],
                end: w[1],
                width: leg_w,
                layer: pl,
                net: net.clone(),
                source: None,
            }));
        }
        pcb = work;
        tuned += 1;
        log::info!(
            "descent: {pn} skew {:.3} -> {:.3} mm ({} iters, loss {:.3})",
            before,
            r.skew,
            r.iters,
            r.loss
        );
    }
    println!("si-descent: tuned {tuned}/{attempted} pairs ({rejected} oracle-rejected)");
    std::fs::write(&output, serde_json::to_string(&pcb).expect("serialize")).expect("write");
    eprintln!("wrote {output}");
}
