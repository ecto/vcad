//! Exact (0.000mm) cross-net trace overlaps on a routed board, found
//! geometrically rather than via the DRC message text, and cross-examined
//! against the router's own oracle.
//!
//! For each overlapping pair it asks `RouteSession::probe` whether the second
//! trace is legal on a board holding the first. `LEGAL` means the oracle
//! accepts physically overlapping copper (an oracle bug); `ILLEGAL` means some
//! commit path put the copper down without asking.
//!
//! ```bash
//! cargo run --release -p vcad-ecad-pcb --example overlap_report -- board.pcb.json
//! ```

use std::collections::BTreeMap;
use vcad_ecad_pcb::session::RouteSession;
use vcad_ecad_pcb::spatial::CopperGeom;
use vcad_ir::ecad::{Pcb, Trace};
use vcad_ir::Vec2;

/// Distance between two segment capsules' centerlines.
fn seg_seg_dist(a0: Vec2, a1: Vec2, b0: Vec2, b1: Vec2) -> f64 {
    let d = |p: Vec2, x: Vec2, y: Vec2| {
        let xy = y - x;
        let l2 = xy.x * xy.x + xy.y * xy.y;
        let t = if l2 <= 0.0 {
            0.0
        } else {
            (((p - x).x * xy.x + (p - x).y * xy.y) / l2).clamp(0.0, 1.0)
        };
        (p - (x + xy.scale(t))).length()
    };
    // Segment intersection ⇒ zero.
    let cross = |p: Vec2, q: Vec2, r: Vec2| (q.x - p.x) * (r.y - p.y) - (q.y - p.y) * (r.x - p.x);
    let (d1, d2, d3, d4) = (
        cross(a0, a1, b0),
        cross(a0, a1, b1),
        cross(b0, b1, a0),
        cross(b0, b1, a1),
    );
    if ((d1 > 0.0) != (d2 > 0.0)) && ((d3 > 0.0) != (d4 > 0.0)) {
        return 0.0;
    }
    d(b0, a0, a1)
        .min(d(b1, a0, a1))
        .min(d(a0, b0, b1))
        .min(d(a1, b0, b1))
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: overlap_report <board.pcb.json>");
    let pcb: Pcb =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("parse");

    // Bucket by layer so the O(n^2) sweep stays cheap.
    let mut by_layer: BTreeMap<String, Vec<&Trace>> = BTreeMap::new();
    for t in &pcb.traces {
        by_layer
            .entry(format!("{:?}", t.layer))
            .or_default()
            .push(t);
    }

    let session = RouteSession::from_pcb(&pcb);

    // What kind of copper does each overlapping trace actually hit? The DRC's
    // "trace net A to net B" covers traces, pads, vias and zone fill alike.
    {
        let is_via = |c: Vec2, r: f64| {
            pcb.vias
                .iter()
                .any(|v| (v.position - c).length() < 1e-6 && (v.diameter / 2.0 - r).abs() < 1e-6)
        };
        let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
        let mut total = 0usize;
        for t in &pcb.traces {
            let hw = t.width / 2.0;
            let g = CopperGeom::Segment {
                a: t.start,
                b: t.end,
                half_w: hw,
            };
            let (lo, hi) = (
                [
                    t.start.x.min(t.end.x) - hw - 1.0,
                    t.start.y.min(t.end.y) - hw - 1.0,
                ],
                [
                    t.start.x.max(t.end.x) + hw + 1.0,
                    t.start.y.max(t.end.y) + hw + 1.0,
                ],
            );
            session.for_each_blocking(t.layer, &t.net, lo, hi, |other, _, _, _| {
                if g.distance_to(other) > 1e-9 {
                    return;
                }
                total += 1;
                *kinds
                    .entry(match other {
                        CopperGeom::Segment { .. } => "trace",
                        CopperGeom::Disc { center, r } => {
                            if is_via(*center, *r) {
                                "via"
                            } else {
                                "round pad"
                            }
                        }
                        CopperGeom::Rect { .. } => "rect pad",
                    })
                    .or_default() += 1;
            });
        }
        // Which vias are the offenders?
        let mut via_nets: BTreeMap<String, usize> = BTreeMap::new();
        for t in &pcb.traces {
            let hw = t.width / 2.0;
            let g = CopperGeom::Segment {
                a: t.start,
                b: t.end,
                half_w: hw,
            };
            for v in &pcb.vias {
                if v.net == t.net || !layer_spanned(&pcb, v, t.layer) {
                    continue;
                }
                let d = g.distance_to(&CopperGeom::Disc {
                    center: v.position,
                    r: v.diameter / 2.0,
                });
                if d <= 1e-9 {
                    *via_nets.entry(v.net.clone()).or_default() += 1;
                }
            }
        }
        println!("vias whose barrel a foreign trace runs through:");
        for (nname, c) in &via_nets {
            println!("  {c:4}  {nname}");
        }
        println!();
        println!("exact overlaps by partner kind (each counted once per trace):");
        for (k, c) in &kinds {
            println!("  {c:5}  trace vs {k}");
        }
        println!("  {total:5}  total\n");
    }

    let mut pairs: Vec<(&Trace, &Trace)> = Vec::new();
    for ts in by_layer.values() {
        for i in 0..ts.len() {
            for j in (i + 1)..ts.len() {
                let (a, b) = (ts[i], ts[j]);
                if a.net == b.net {
                    continue;
                }
                let gap =
                    seg_seg_dist(a.start, a.end, b.start, b.end) - a.width / 2.0 - b.width / 2.0;
                if gap <= 1e-9 {
                    pairs.push((a, b));
                }
            }
        }
    }

    let mut classes: BTreeMap<String, usize> = BTreeMap::new();
    let mut probe_legal = 0usize;
    for (n, (a, b)) in pairs.iter().enumerate() {
        // Ask the oracle about `b` on a board that still holds `a`. Same-net
        // copper of `b` is excluded by the probe itself, so the full session
        // is the right thing to ask against.
        let pr = session.probe(
            &CopperGeom::Segment {
                a: b.start,
                b: b.end,
                half_w: b.width / 2.0,
            },
            b.layer,
            &b.net,
            session.clearance_for(&b.net),
        );
        let saw_a = pr.blockers.iter().any(|bl| bl.net == a.net);
        if !saw_a {
            probe_legal += 1;
        }
        *classes
            .entry(format!(
                "{} w={:.3}/{:.3}",
                if saw_a {
                    "oracle-refuses"
                } else {
                    "ORACLE-ACCEPTS"
                },
                a.width,
                b.width
            ))
            .or_default() += 1;
        if n < 30 {
            println!(
                "{:>15} {:?} ({:7.3},{:7.3})-({:7.3},{:7.3}) w{:.3}  X  {:>15} ({:7.3},{:7.3})-({:7.3},{:7.3}) w{:.3}   oracle: {}",
                a.net, a.layer, a.start.x, a.start.y, a.end.x, a.end.y, a.width,
                b.net, b.start.x, b.start.y, b.end.x, b.end.y, b.width,
                if saw_a { "refuses" } else { "ACCEPTS" },
            );
        }
    }
    println!(
        "\n{} exact cross-net trace overlaps; oracle accepts {probe_legal} of them",
        pairs.len()
    );
    for (k, c) in classes {
        println!("  {c:4}  {k}");
    }
}

/// Is `layer` inside the via's start..end span?
fn layer_spanned(pcb: &Pcb, v: &vcad_ir::ecad::Via, layer: vcad_ir::ecad::PcbLayer) -> bool {
    let copper: Vec<vcad_ir::ecad::PcbLayer> = pcb
        .stackup
        .layers
        .iter()
        .filter(|l| l.layer.is_copper())
        .map(|l| l.layer)
        .collect();
    let idx = |l: vcad_ir::ecad::PcbLayer| copper.iter().position(|&c| c == l);
    match (idx(v.start_layer), idx(v.end_layer), idx(layer)) {
        (Some(a), Some(b), Some(c)) => c >= a.min(b) && c <= a.max(b),
        _ => true,
    }
}
