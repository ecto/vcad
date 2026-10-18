//! Flow router M2: take each human wire's homotopy word on the cut graph and
//! realize it with the funnel; compare length and clearance with the human.
//!
//! ```bash
//! cargo run --release -p vcad-ecad-pcb --example flow_m2 -- board.pcb.json [out.pcb.json]
//! ```
//!
//! The graph keeps the human's pads and vias (its layer changes are fixed);
//! the human's traces are removed, re-mapped to cut sequences, and funneled.
//! Wires through one free part keep the human's order along the cut.

use std::collections::{BTreeMap, HashMap};
use std::time::Instant;

use vcad_ecad_pcb::drc::check_drc;
use vcad_ecad_pcb::router::flow::cdt::{CopperFilter, CutGraph, CutParams, NONE};
use vcad_ecad_pcb::router::flow::realize::{realize_wires, Wire, Wx};
use vcad_ir::ecad::{Pcb, Trace};
use vcad_ir::Vec2;

fn d2(a: [f64; 2], b: [f64; 2]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

struct Chain {
    layer: usize,
    net: String,
    width: f64,
    pts: Vec<[f64; 2]>,
}

struct X {
    cut: u32,
    from: u32,
    s: f64,
    sin: f64,
}

fn rules(pcb: &Pcb) -> BTreeMap<String, usize> {
    let mut m = BTreeMap::new();
    for v in check_drc(pcb) {
        *m.entry(format!("{:?}", v.rule)).or_default() += 1;
    }
    m
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: flow_m2 <board.pcb.json> [out]");
    let out = args.next();
    let mut pcb: Pcb =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
    if pcb.stackup.layers.iter().all(|l| l.role.is_none()) {
        pcb.infer_layer_roles();
    }
    let mut params = CutParams::from_pcb(&pcb);
    // The graph is inflated for the narrowest track a chain uses (w0); wider
    // chains shrink their portals by the extra half-width.
    if let Some(w) = std::env::var("VCAD_FLOW_WIDTH")
        .ok()
        .and_then(|v| v.parse().ok())
    {
        params.width = w;
    }
    let t0 = Instant::now();
    let cg = CutGraph::build(&pcb, params, CopperFilter::PadsAndVias);
    println!(
        "graph (pads + human vias) {:.0} ms; model w0 {} s {}",
        t0.elapsed().as_secs_f64() * 1e3,
        params.width,
        params.clearance
    );

    // --- chains: maximal runs of one net's segments through degree-2 points.
    let key = |p: Vec2| ((p.x * 1e4).round() as i64, (p.y * 1e4).round() as i64);
    let mut chains: Vec<Chain> = Vec::new();
    let mut kept: Vec<Trace> = Vec::new(); // traces we do not re-realize
    for (li, lg) in cg.layers.iter().enumerate() {
        let mut segs: Vec<(Vec2, Vec2, f64, String)> = pcb
            .traces
            .iter()
            .filter(|t| t.layer == lg.layer)
            .map(|t| (t.start, t.end, t.width, t.net.clone()))
            .collect();
        for a in pcb.trace_arcs.iter().filter(|a| a.layer == lg.layer) {
            let (a0, a1) = (a.start_angle.to_radians(), a.end_angle.to_radians());
            let pts: Vec<Vec2> = (0..=8)
                .map(|i| {
                    let t = a0 + (a1 - a0) * i as f64 / 8.0;
                    Vec2::new(
                        a.center.x + a.radius * t.cos(),
                        a.center.y + a.radius * t.sin(),
                    )
                })
                .collect();
            for w in pts.windows(2) {
                segs.push((w[0], w[1], a.width, a.net.clone()));
            }
        }
        let mut by_net: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (i, s) in segs.iter().enumerate() {
            by_net.entry(s.3.clone()).or_default().push(i);
        }
        for (net, ids) in by_net {
            let mut adj: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
            for &i in &ids {
                adj.entry(key(segs[i].0)).or_default().push(i);
                adj.entry(key(segs[i].1)).or_default().push(i);
            }
            let mut used = vec![false; segs.len()];
            let mut starts: Vec<usize> = ids
                .iter()
                .copied()
                .filter(|&i| adj[&key(segs[i].0)].len() != 2 || adj[&key(segs[i].1)].len() != 2)
                .collect();
            starts.extend(ids.iter().copied()); // cycles
            for s0 in starts {
                if used[s0] {
                    continue;
                }
                // orient so we start at a non-degree-2 end when possible
                let (mut a, mut b) = (segs[s0].0, segs[s0].1);
                if adj[&key(a)].len() == 2 && adj[&key(b)].len() != 2 {
                    std::mem::swap(&mut a, &mut b);
                }
                used[s0] = true;
                let mut pts = vec![[a.x, a.y], [b.x, b.y]];
                let mut width = segs[s0].2;
                let mut cur = b;
                loop {
                    let nb = &adj[&key(cur)];
                    if nb.len() != 2 {
                        break;
                    }
                    let Some(&nx) = nb.iter().find(|&&j| !used[j]) else {
                        break;
                    };
                    used[nx] = true;
                    let (p, q) = (segs[nx].0, segs[nx].1);
                    let next = if key(p) == key(cur) { q } else { p };
                    pts.push([next.x, next.y]);
                    width = width.max(segs[nx].2);
                    cur = next;
                }
                chains.push(Chain {
                    layer: li,
                    net: net.clone(),
                    width,
                    pts,
                });
            }
        }
    }
    let planes: Vec<_> = pcb.plane_layers();
    for t in &pcb.traces {
        if !cg.layers.iter().any(|l| l.layer == t.layer) {
            kept.push(t.clone());
        }
    }
    let _ = planes;

    // --- words.
    let mut words: Vec<Vec<X>> = Vec::with_capacity(chains.len());
    let mut walk_fail = 0usize;
    for ch in &chains {
        let lg = &cg.layers[ch.layer];
        let mut w: Vec<X> = Vec::new();
        let mut face = lg.locate(ch.pts[0]);
        for seg in ch.pts.windows(2) {
            let Some(f0) = face else { break };
            let Some((xs, end)) = lg.walk(seg[0], seg[1], Some(f0)) else {
                walk_fail += 1;
                face = lg.locate(seg[1]);
                continue;
            };
            let len = d2(seg[0], seg[1]).max(1e-12);
            let dir = [(seg[1][0] - seg[0][0]) / len, (seg[1][1] - seg[0][1]) / len];
            let mut from = f0;
            for x in xs {
                let c = &lg.cuts[x.cut as usize];
                let (a, b) = (lg.vpos(c.a), lg.vpos(c.b));
                let cd = [(b[0] - a[0]) / c.len, (b[1] - a[1]) / c.len];
                let sin = (dir[0] * cd[1] - dir[1] * cd[0]).abs();
                // back-and-forth over one cut cancels (reduced word)
                if w.last().is_some_and(|l: &X| l.cut == x.cut) {
                    w.pop();
                } else {
                    w.push(X {
                        cut: x.cut,
                        from,
                        s: x.s,
                        sin,
                    });
                }
                from = x.into;
            }
            face = Some(end);
        }
        words.push(w);
    }

    // --- wires: each crossing may use the free part holding the human
    // crossing (shrunk for the chain's width), else it is pinned at the
    // human's point; wires through one cut keep the human's order.
    let eps = 0.012;
    let w0 = params.width;
    let mut off_part = 0usize;
    let mut net_ids: HashMap<String, u32> = HashMap::new();
    let mut wires: Vec<Wire> = Vec::with_capacity(chains.len());
    let mut by_cut: HashMap<(usize, u32), Vec<(f64, usize)>> = HashMap::new();
    for (ci, (ch, w)) in chains.iter().zip(&words).enumerate() {
        let lg = &cg.layers[ch.layer];
        let shrink = eps + ((ch.width - w0) / 2.0).max(0.0);
        let nid = {
            let n = net_ids.len() as u32;
            *net_ids.entry(ch.net.clone()).or_insert(n)
        };
        let xs = w
            .iter()
            .map(|x| {
                let c = &lg.cuts[x.cut as usize];
                by_cut.entry((ch.layer, x.cut)).or_default().push((x.s, ci));
                let (lo, hi) = match c.part_at(x.s) {
                    Some(k) => {
                        let q = c.parts[k];
                        let e = shrink.min((q[1] - q[0]) / 2.0);
                        (q[0] + e, q[1] - e)
                    }
                    None => {
                        off_part += 1;
                        (x.s, x.s)
                    }
                };
                let _ = x.sin;
                Wx {
                    cut: x.cut,
                    from: x.from,
                    lo,
                    hi,
                }
            })
            .collect();
        wires.push(Wire {
            layer: ch.layer,
            width: ch.width,
            clearance: params.clearance,
            net: nid,
            start: ch.pts[0],
            end: *ch.pts.last().unwrap(),
            xs,
        });
    }
    let order: HashMap<(usize, u32), Vec<usize>> = by_cut
        .into_iter()
        .map(|(k, mut v)| {
            v.sort_by(|a, b| a.0.total_cmp(&b.0));
            (k, v.into_iter().map(|x| x.1).collect())
        })
        .collect();
    let init: Vec<Vec<f64>> = words
        .iter()
        .map(|w| w.iter().map(|x| x.s).collect())
        .collect();
    let sweeps = std::env::var("VCAD_FLOW_SWEEPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    let polys = realize_wires(&cg, &wires, &order, Some(&init), sweeps);

    // --- funnel, measure, rebuild.
    let mut human_len = 0.0;
    let mut flow_len = 0.0;
    let mut longer = 0usize;
    let mut board = pcb.clone();
    board.traces = kept;
    board.trace_arcs.clear();
    for (ci, (ch, path)) in chains.iter().zip(&polys).enumerate() {
        let hl: f64 = ch.pts.windows(2).map(|w| d2(w[0], w[1])).sum();
        let path = path.clone();
        if let Ok(t) = std::env::var("VCAD_FLOW_TRACE") {
            let near = |p: &[f64; 2]| {
                let f: Vec<f64> = t.split(',').map(|x| x.parse().unwrap()).collect();
                (p[0] - f[0]).abs() < 0.6 && (p[1] - f[1]).abs() < 0.6
            };
            if path.iter().any(near) || ch.pts.iter().any(near) {
                println!(
                    "TRACE {} {:?} w {} human {:?}",
                    ch.net, cg.layers[ch.layer].layer, ch.width, ch.pts
                );
                println!("   path {path:?}");
                for (x, hx) in wires[ci].xs.iter().zip(&words[ci]) {
                    let c = &cg.layers[ch.layer].cuts[x.cut as usize];
                    println!(
                        "   x cut {} len {:.3} human s {:.3} lo {:.3} hi {:.3} parts {:?}",
                        x.cut, c.len, hx.s, x.lo, x.hi, c.parts
                    );
                }
            }
        }
        let fl: f64 = path.windows(2).map(|w| d2(w[0], w[1])).sum();
        human_len += hl;
        flow_len += fl;
        if fl > hl * 1.10 + 0.05 {
            longer += 1;
        }
        for w in path.windows(2) {
            if d2(w[0], w[1]) < 1e-6 {
                continue;
            }
            board.traces.push(Trace {
                start: Vec2::new(w[0][0], w[0][1]),
                end: Vec2::new(w[1][0], w[1][1]),
                width: ch.width,
                layer: cg.layers[ch.layer].layer,
                net: ch.net.clone(),
                source: None,
            });
        }
    }
    let _ = NONE;
    println!(
        "chains {} (walk failures {walk_fail}, crossings outside a free part {off_part}); \
         length human {human_len:.1} mm -> funneled {flow_len:.1} mm ({:+.2}%), \
         {longer} chains >10% longer",
        chains.len(),
        100.0 * (flow_len / human_len - 1.0)
    );
    let t1 = Instant::now();
    let before = rules(&pcb);
    let after = rules(&board);
    let mut delta: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for k in before.keys().chain(after.keys()) {
        delta.insert(
            k.clone(),
            (*before.get(k).unwrap_or(&0), *after.get(k).unwrap_or(&0)),
        );
    }
    println!(
        "DRC by rule (human -> funneled), {:.1} s:",
        t1.elapsed().as_secs_f64()
    );
    for (k, (b, a)) in delta {
        println!("  {k:<22} {b:>6} -> {a:>6}");
    }
    // Net pairs in clearance/short violation: funneled-only vs human.
    {
        let pairs = |b: &Pcb| -> std::collections::BTreeSet<(String, String)> {
            check_drc(b)
                .into_iter()
                .filter(|v| matches!(format!("{:?}", v.rule).as_str(), "Clearance" | "Short"))
                .filter_map(|v| {
                    let q: Vec<&str> = v.message.split('\'').collect();
                    (q.len() >= 4).then(|| {
                        let (a, b) = (q[1].to_string(), q[3].to_string());
                        if a < b {
                            (a, b)
                        } else {
                            (b, a)
                        }
                    })
                })
                .collect()
        };
        let (hp, fp) = (pairs(&pcb), pairs(&board));
        // Pours are re-filled around routing; stored fills are stale for
        // moved wires, so pairs with a pour net are listed apart.
        let poured: std::collections::BTreeSet<&str> =
            pcb.zones.iter().map(|z| z.net.as_str()).collect();
        let (new, new_pour): (Vec<_>, Vec<_>) = fp
            .difference(&hp)
            .partition(|(a, b)| !poured.contains(a.as_str()) && !poured.contains(b.as_str()));
        println!("  ({} more new pairs involve a poured net)", new_pour.len());
        println!(
            "net pairs in clearance/short violation: human {}, funneled {}, new {} (fixed {})",
            hp.len(),
            fp.len(),
            new.len(),
            hp.difference(&fp).count()
        );
        for p in new.iter().take(12) {
            println!("  new pair {p:?}");
        }
    }
    if std::env::var("VCAD_FLOW_SHOW").is_ok() {
        let hv: Vec<Vec2> = check_drc(&pcb)
            .into_iter()
            .filter(|v| format!("{:?}", v.rule) == "Clearance")
            .map(|v| v.position)
            .collect();
        for v in check_drc(&board)
            .into_iter()
            .filter(|v| format!("{:?}", v.rule) == "Clearance")
            .filter(|v| {
                !hv.iter()
                    .any(|h| (h.x - v.position.x).abs() + (h.y - v.position.y).abs() < 0.05)
            })
            .take(
                std::env::var("VCAD_FLOW_SHOW")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(25),
            )
        {
            println!(
                "  new {:?} {:.3}/{:.3}: {}",
                v.position, v.actual, v.required, v.message
            );
        }
    }
    if let Some(o) = out {
        std::fs::write(o, serde_json::to_string(&board).unwrap()).unwrap();
    }
}
