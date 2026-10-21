//! Human load check: map an existing routing onto the cut graph and count, per
//! cut, the wires crossing it against its capacity.
//!
//! This is the kill/adjust test for the capacity model. A DRC-clean human
//! layout satisfies the cut condition by construction, so any cut the human
//! "overflows" is a cut whose capacity the model under-estimates.
//!
//! Load is width-weighted (the exact Schrijver / Leiserson–Maley form): a free
//! part of length `ℓ` carries wires `i` iff `Σ (w_i + s) ≤ ℓ + s`. The count
//! form at the default width (`#wires ≤ cap`) is reported alongside, since
//! that is what the flow will price.

use std::collections::BTreeMap;

use vcad_ir::ecad::Pcb;

use super::cdt::{CutGraph, NONE};

/// One wire crossing a cut.
#[derive(Debug, Clone)]
pub struct WireCrossing {
    /// Net.
    pub net: String,
    /// Track width (mm).
    pub width: f64,
    /// Position along the cut (mm from vertex `a`).
    pub s: f64,
}

/// A cut the human routing overflows.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Overflow {
    /// Layer name.
    pub layer: String,
    /// Cut id within its layer graph.
    pub cut: u32,
    /// Cut midpoint (mm).
    pub at: [f64; 2],
    /// Cut length (mm).
    pub len: f64,
    /// Free length (mm).
    pub free: f64,
    /// Count capacity at the default width.
    pub cap: u32,
    /// Wires crossing.
    pub wires: usize,
    /// Width-weighted load minus (free + s) on the worst part (mm, > 0).
    pub excess: f64,
    /// Nets crossing.
    pub nets: Vec<String>,
}

/// Per-layer tallies.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct LayerLoad {
    /// Layer name.
    pub layer: String,
    /// Vertices / faces / cuts in the graph.
    pub verts: usize,
    /// Faces.
    pub faces: usize,
    /// Cuts.
    pub cuts: usize,
    /// Cuts with at least one human wire.
    pub loaded: usize,
    /// Cuts overflowed width-weighted (exact cut condition).
    pub overflow_weighted: usize,
    /// Cuts overflowed in the count form at the default width.
    pub overflow_count: usize,
    /// Crossings that land inside another net's inflated obstacle (the human
    /// is closer than the model's clearance to that copper).
    pub intrusions: usize,
    /// Cuts loaded exactly to capacity in the count form (the model is tight
    /// there: one more wire would overflow).
    pub saturated: usize,
    /// Trace segments whose walk failed.
    pub walk_failures: usize,
    /// Trace segments mapped.
    pub segments: usize,
}

/// Board-level result.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct HumanLoadReport {
    /// Per signal layer.
    pub layers: Vec<LayerLoad>,
    /// Human trace segments on plane-role layers (no graph there).
    pub plane_layer_segments: usize,
    /// Every overflowed cut (weighted or count form).
    pub overflows: Vec<Overflow>,
}

impl HumanLoadReport {
    /// Totals: (cuts, loaded, weighted overflow, count overflow, intrusions).
    pub fn totals(&self) -> (usize, usize, usize, usize, usize) {
        self.layers.iter().fold((0, 0, 0, 0, 0), |a, l| {
            (
                a.0 + l.cuts,
                a.1 + l.loaded,
                a.2 + l.overflow_weighted,
                a.3 + l.overflow_count,
                a.4 + l.intrusions,
            )
        })
    }
}

/// Map `pcb`'s traces (and arcs, as chords) onto `graph` and tally loads.
pub fn human_load(pcb: &Pcb, graph: &CutGraph) -> HumanLoadReport {
    let s = graph.params.clearance;
    let w0 = graph.params.width;
    let mut rep = HumanLoadReport::default();
    let planes = pcb.plane_layers();
    rep.plane_layer_segments = pcb
        .traces
        .iter()
        .filter(|t| planes.contains(&t.layer))
        .count();
    for g in &graph.layers {
        let mut ll = LayerLoad {
            layer: format!("{:?}", g.layer),
            verts: g.verts.len(),
            faces: g.tris.len(),
            cuts: g.cuts.len(),
            ..Default::default()
        };
        let mut per_cut: BTreeMap<u32, Vec<WireCrossing>> = BTreeMap::new();
        let mut segs: Vec<(vcad_ir::Vec2, vcad_ir::Vec2, f64, &str)> = pcb
            .traces
            .iter()
            .filter(|t| t.layer == g.layer)
            .map(|t| (t.start, t.end, t.width, t.net.as_str()))
            .collect();
        for a in pcb.trace_arcs.iter().filter(|a| a.layer == g.layer) {
            let pts = arc_points(a);
            for w in pts.windows(2) {
                segs.push((w[0], w[1], a.width, a.net.as_str()));
            }
        }
        for (a, b, w, net) in segs {
            ll.segments += 1;
            let Some((word, _)) = g.walk([a.x, a.y], [b.x, b.y], None) else {
                ll.walk_failures += 1;
                continue;
            };
            for c in word {
                per_cut.entry(c.cut).or_default().push(WireCrossing {
                    net: net.to_string(),
                    width: w,
                    s: c.s,
                });
            }
        }
        for (cid, mut xs) in per_cut {
            let cut = &g.cuts[cid as usize];
            // One wire of a net crossing at one place: a polyline vertex on
            // the cut shows up from both of its segments.
            xs.sort_by(|a, b| a.net.cmp(&b.net).then(a.s.total_cmp(&b.s)));
            xs.dedup_by(|b, a| a.net == b.net && (a.s - b.s).abs() < 0.02);
            ll.loaded += 1;
            let pa = g.vpos(cut.a);
            let pb = g.vpos(cut.b);
            let mut part_load = vec![0.0f64; cut.parts.len()];
            let mut counted = 0usize;
            for x in &xs {
                match cut.part_at(x.s) {
                    Some(k) => {
                        let credit = g.own_credit(cid, k, &x.net);
                        part_load[k] += (x.width + s - credit).max(0.0);
                        // A wire leaving its own pad does not consume the
                        // cut in the count form either.
                        if credit < x.width + s {
                            counted += 1;
                        }
                    }
                    None => {
                        let u = (x.s / cut.len.max(1e-12)).clamp(0.0, 1.0);
                        let at = [pa[0] + u * (pb[0] - pa[0]), pa[1] + u * (pb[1] - pa[1])];
                        let own = g.obstacles_at(at, 0.0).iter().all(|&o| {
                            let ob = &g.obstacles[o as usize];
                            ob.net == x.net
                        });
                        if !own {
                            ll.intrusions += 1;
                        }
                    }
                }
            }
            let excess = cut
                .parts
                .iter()
                .zip(&part_load)
                .map(|(p, l)| {
                    if *l > 0.0 {
                        l - (p[1] - p[0] + w0 + s)
                    } else {
                        f64::NEG_INFINITY
                    }
                })
                .fold(f64::NEG_INFINITY, f64::max);
            let weighted = excess > 1e-6;
            let count = counted as u32 > cut.cap_for(w0, w0, s);
            if weighted {
                ll.overflow_weighted += 1;
            }
            if counted > 0 && counted as u32 == cut.cap_for(w0, w0, s) {
                ll.saturated += 1;
            }
            if count {
                ll.overflow_count += 1;
            }
            if weighted || count {
                let mut nets: Vec<String> = xs.iter().map(|x| x.net.clone()).collect();
                nets.sort();
                nets.dedup();
                rep.overflows.push(Overflow {
                    layer: ll.layer.clone(),
                    cut: cid,
                    at: [(pa[0] + pb[0]) / 2.0, (pa[1] + pb[1]) / 2.0],
                    len: cut.len,
                    free: cut.free(),
                    cap: cut.cap,
                    wires: xs.len(),
                    excess,
                    nets,
                });
            }
        }
        let _ = NONE;
        rep.layers.push(ll);
    }
    rep
}

/// Chord points of a trace arc (16 chords; angles in degrees, start→end).
fn arc_points(a: &vcad_ir::ecad::TraceArc) -> Vec<vcad_ir::Vec2> {
    let (a0, a1) = (a.start_angle.to_radians(), a.end_angle.to_radians());
    (0..=16)
        .map(|i| {
            let t = a0 + (a1 - a0) * i as f64 / 16.0;
            vcad_ir::Vec2::new(
                a.center.x + a.radius * t.cos(),
                a.center.y + a.radius * t.sin(),
            )
        })
        .collect()
}
