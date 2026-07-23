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
use vcad_ecad_pcb::router::descent::{descend_pair_runs, DescentObstacle, DescentWeights};
use vcad_ecad_pcb::router::length_match::longest_chain;
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

    // Longest single-layer run of a net plus the length of everything else.
    let net_run = |pcb: &Pcb, net: &str| -> Option<(PcbLayer, Vec<Vec2>, f64, f64)> {
        let mut layers: Vec<PcbLayer> = pcb
            .traces
            .iter()
            .filter(|t| t.net == net)
            .map(|t| t.layer)
            .collect();
        layers.sort();
        layers.dedup();
        let total: f64 = pcb
            .traces
            .iter()
            .filter(|t| t.net == net)
            .map(|t| (t.end - t.start).length())
            .sum();
        let mut best: Option<(f64, Vec<Vec2>, PcbLayer)> = None;
        for layer in layers {
            let segs: Vec<&Trace> = pcb
                .traces
                .iter()
                .filter(|t| t.net == net && t.layer == layer)
                .collect();
            if let Some(points) = longest_chain(&segs) {
                let len: f64 = points.windows(2).map(|w| (w[1] - w[0]).length()).sum();
                if best.as_ref().map(|(l, _, _)| len > *l).unwrap_or(true) {
                    best = Some((len, points, layer));
                }
            }
        }
        best.map(|(len, pts, layer)| {
            let width = pcb
                .traces
                .iter()
                .find(|t| t.net == net && t.layer == layer)
                .map(|t| t.width)
                .unwrap_or(0.08);
            (layer, pts, total - len, width)
        })
    };

    let (mut tuned, mut attempted, mut rejected) = (0usize, 0usize, 0usize);
    for (pn, nn) in &c.pairs {
        let (Some((pl, p_raw, extra_p, p_w)), Some((nlayer, n_raw, extra_n, n_w))) =
            (net_run(&pcb, pn), net_run(&pcb, nn))
        else {
            continue;
        };
        // Densify: point hinges only constrain POINTS — a long segment
        // between two legal points sweeps across the board unconstrained.
        // Resampling at ~1mm makes point coverage ≈ segment coverage.
        let densify = |pts: &[Vec2]| -> Vec<Vec2> {
            let mut out = vec![pts[0]];
            for w in pts.windows(2) {
                let len = (w[1] - w[0]).length();
                let n = (len / 0.4).ceil().max(1.0) as usize;
                for i in 1..=n {
                    let t = i as f64 / n as f64;
                    out.push(w[0] + (w[1] - w[0]).scale(t));
                }
            }
            out
        };
        let p_pts = densify(&p_raw);
        let n_pts = densify(&n_raw);
        let run_w = p_w.max(n_w);
        let plen: f64 = p_pts
            .windows(2)
            .map(|w| (w[1] - w[0]).length())
            .sum::<f64>()
            + extra_p;
        let nlen: f64 = n_pts
            .windows(2)
            .map(|w| (w[1] - w[0]).length())
            .sum::<f64>()
            + extra_n;
        if (plen - nlen).abs() < 0.2 || p_pts.len() < 3 || n_pts.len() < 3 {
            continue; // matched already, or no interior freedom
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
        // Obstacles: EVERY copper element near the corridor on either run
        // layer, from a session with both nets removed — pads and vias
        // included (the first run's 17/17 oracle rejections were descended
        // runs swinging across pad fields the trace-only set couldn't see).
        let mut ob_board = pcb.clone();
        ob_board.traces.retain(|t| t.net != *pn && t.net != *nn);
        ob_board.vias.retain(|v| v.net != *pn && v.net != *nn);
        let ob_session = RouteSession::from_pcb(&ob_board);
        let mut obstacles = Vec::new();
        let mut ob_layers = vec![pl];
        if nlayer != pl {
            ob_layers.push(nlayer);
        }
        for &layer in &ob_layers {
            ob_session.for_each_blocking(
                layer,
                "",
                [lo.x, lo.y],
                [hi.x, hi.y],
                |geom, emin, emax, req| {
                    // +0.05mm safety: the hinge is a soft penalty traded
                    // against skew, so its equilibrium must land OUTSIDE the
                    // hard rule the oracle enforces.
                    let required = req + run_w / 2.0 + 0.05;
                    match geom {
                        CopperGeom::Segment { a, b, half_w } => obstacles.push(DescentObstacle {
                            a: *a,
                            b: *b,
                            required: required + half_w,
                        }),
                        CopperGeom::Disc { center, r } => obstacles.push(DescentObstacle {
                            a: *center,
                            b: *center,
                            required: required + r,
                        }),
                        _ => {
                            // Rect etc.: conservative disc over the bbox.
                            let c = Vec2::new((emin[0] + emax[0]) / 2.0, (emin[1] + emax[1]) / 2.0);
                            let hd = ((emax[0] - emin[0]).powi(2) + (emax[1] - emin[1]).powi(2))
                                .sqrt()
                                / 2.0;
                            obstacles.push(DescentObstacle {
                                a: c,
                                b: c,
                                required: required + hd,
                            });
                        }
                    }
                },
            );
        }

        let Some(r) = descend_pair_runs(
            &p_pts,
            &n_pts,
            extra_p,
            extra_n,
            pl == nlayer,
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
        let legal = |pts: &[Vec2], net: &str, layer: PcbLayer, w: f64| -> bool {
            pts.windows(2).all(|seg| {
                let g = CopperGeom::Segment {
                    a: seg[0],
                    b: seg[1],
                    half_w: w / 2.0,
                };
                vsession.probe(&g, layer, net, clearance).legal
            })
        };
        let diag = |pts: &[Vec2], net: &str, layer: PcbLayer, w: f64| {
            for seg in pts.windows(2) {
                let g = CopperGeom::Segment {
                    a: seg[0],
                    b: seg[1],
                    half_w: w / 2.0,
                };
                let pr = vsession.probe(&g, layer, net, clearance);
                if !pr.legal {
                    log::info!(
                        "reject {net}: seg ({:.2},{:.2})->({:.2},{:.2}) {layer:?} w={w} blocker {} at {:.3} (skew would be {:.3}, loss {:.2}, iters {})",
                        seg[0].x, seg[0].y, seg[1].x, seg[1].y,
                        pr.blockers.first().map(|b| b.net.clone()).unwrap_or_default(),
                        pr.min_clearance, r.skew, r.loss, r.iters
                    );
                    return false;
                }
            }
            true
        };
        if !diag(&r.p_pts, pn, pl, p_w) || !diag(&r.n_pts, nn, nlayer, n_w) {
            rejected += 1;
            continue;
        }
        let before = (plen - nlen).abs();
        // Whole-net accounting: r.skew is run-only; judge the committed
        // outcome on the full nets and accept only real improvements.
        let run_p_after: f64 = r.p_pts.windows(2).map(|w| (w[1] - w[0]).length()).sum();
        let run_n_after: f64 = r.n_pts.windows(2).map(|w| (w[1] - w[0]).length()).sum();
        let after = ((run_p_after + extra_p) - (run_n_after + extra_n)).abs();
        if after >= before {
            rejected += 1;
            log::info!("descent: {pn} no improvement ({before:.3} -> {after:.3}) — skipped");
            continue;
        }
        // Replace only the optimized RUN's segments; the rest of each net's
        // copper (other layers, stubs, vias) is untouched.
        let mut work2 = pcb.clone();
        let replace_run = |work2: &mut Pcb,
                           net: &str,
                           layer: PcbLayer,
                           old: &[Vec2],
                           new: &[Vec2],
                           width: f64| {
            let on_old = |t: &Trace| {
                t.net == net
                    && t.layer == layer
                    && old.windows(2).any(|w| {
                        (t.start - w[0]).length() < 1e-6 && (t.end - w[1]).length() < 1e-6
                            || (t.start - w[1]).length() < 1e-6 && (t.end - w[0]).length() < 1e-6
                    })
            };
            work2.traces.retain(|t| !on_old(t));
            work2.traces.extend(new.windows(2).map(|w| Trace {
                start: w[0],
                end: w[1],
                width,
                layer,
                net: net.to_string(),
                source: None,
            }));
        };
        replace_run(&mut work2, pn, pl, &p_raw, &r.p_pts, p_w);
        replace_run(&mut work2, nn, nlayer, &n_raw, &r.n_pts, n_w);
        pcb = work2;
        tuned += 1;
        log::info!(
            "descent: {pn} whole-net skew {:.3} -> {:.3} mm ({} iters, loss {:.3})",
            before,
            after,
            r.iters,
            r.loss
        );
    }
    println!("si-descent: tuned {tuned}/{attempted} pairs ({rejected} oracle-rejected)");
    std::fs::write(&output, serde_json::to_string(&pcb).expect("serialize")).expect("write");
    eprintln!("wrote {output}");
}
