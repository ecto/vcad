//! Plane access as an exact sub-market.
//!
//! Every pad of a poured net that its pour does not already cover on the
//! pad's own layer needs one via to a layer where a pour of its net does
//! cover the via. Sites (via-in-pad, or a short dog-bone around the pad) are
//! unit-capacity resources thinned to a compatible set (one hole per hole
//! pitch), so the problem is a pure assignment — bipartite min-cost matching,
//! whose LP is integral (Birkhoff–von Neumann). The price loop is an
//! ascending auction on site prices: each pad takes its cheapest option at
//! current prices, over-demanded sites get dearer. The Lagrangian bound
//! `Σ_pads min(cost + p) − Σ_sites p` meets a feasible assignment's cost as
//! the prices converge, so the gap goes to zero (gross substitutes:
//! `docs/router-one-market-math.md` §(b), conjecture C3).

use std::collections::{BTreeMap, HashMap};

use vcad_ir::ecad::{Pcb, PcbLayer};
use vcad_ir::Vec2;

use crate::session::RouteSession;
use crate::spatial::{point_in_polygon, CopperGeom};

/// A legal via construction (top, bottom).
pub type Span = (PcbLayer, PcbLayer);

/// The board's via catalog: through, the micro pairs at the two outer
/// layers of each side, and the buried span between the second layers.
pub fn span_catalog(pcb: &Pcb) -> Vec<Span> {
    let cu: Vec<PcbLayer> = PcbLayer::COPPER_STACK
        .iter()
        .copied()
        .filter(|l| pcb.stackup.layers.iter().any(|s| s.layer == *l))
        .collect();
    let n = cu.len();
    if n < 2 {
        return Vec::new();
    }
    let mut v = vec![(cu[0], cu[n - 1])];
    if n >= 4 {
        v.push((cu[0], cu[1]));
        v.push((cu[n - 2], cu[n - 1]));
    }
    if n >= 6 {
        v.push((cu[1], cu[2]));
        v.push((cu[n - 3], cu[n - 2]));
        v.push((cu[1], cu[n - 2]));
    }
    v
}

/// A pad that needs plane access.
#[derive(Debug, Clone)]
pub struct AccessPad {
    /// Net.
    pub net: String,
    /// Pad center.
    pub center: [f64; 2],
    /// Pad copper layer.
    pub layer: PcbLayer,
}

/// One way to serve a pad.
#[derive(Debug, Clone, Copy)]
pub struct AccessOption {
    /// Site index.
    pub site: usize,
    /// Via span.
    pub span: Span,
    /// Stub length + via cost (mm-equivalent).
    pub cost: f64,
}

/// Per-iteration statistics.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AccessIter {
    /// Iteration.
    pub iter: usize,
    /// Sites wanted by more than one pad.
    pub conflicts: usize,
    /// Cost of the iterate (mm-eq).
    pub cost: f64,
    /// Lagrangian lower bound.
    pub lower_bound: f64,
    /// Cost of the greedy-repaired feasible assignment.
    pub upper_bound: f64,
    /// Relative gap (UB − LB) / UB.
    pub gap: f64,
}

/// The access sub-market.
#[derive(Debug, Clone, Default)]
pub struct AccessMarket {
    /// Pads needing access.
    pub pads: Vec<AccessPad>,
    /// Site positions.
    pub sites: Vec<[f64; 2]>,
    /// Options per pad (empty = no legal access).
    pub options: Vec<Vec<AccessOption>>,
    /// Pads already served by their own pour.
    pub served_by_pour: usize,
}

/// A chosen access via.
#[derive(Debug, Clone)]
pub struct AccessVia {
    /// Pad index.
    pub pad: usize,
    /// Via position.
    pub pos: [f64; 2],
    /// Span.
    pub span: Span,
}

/// Nets with a pour on a plane-role layer → those layers. These nets are
/// carried by their planes: their pads get access vias, not traces.
pub fn plane_poured(pcb: &Pcb) -> BTreeMap<&str, Vec<PcbLayer>> {
    let planes = pcb.plane_layers();
    let mut m: BTreeMap<&str, Vec<PcbLayer>> = BTreeMap::new();
    for z in pcb
        .zones
        .iter()
        .filter(|z| !z.net.is_empty() && z.outline.len() >= 3 && planes.contains(&z.layer))
    {
        let e = m.entry(z.net.as_str()).or_default();
        if !e.contains(&z.layer) {
            e.push(z.layer);
        }
    }
    m
}

fn zone_covers(pcb: &Pcb, net: &str, layer: PcbLayer, p: [f64; 2]) -> bool {
    let q = Vec2::new(p[0], p[1]);
    pcb.zones.iter().any(|z| {
        z.net == net
            && z.layer == layer
            && z.outline.len() >= 3
            && point_in_polygon(q, &z.outline)
            && !z
                .holes
                .iter()
                .any(|h| h.len() >= 3 && point_in_polygon(q, h))
    })
}

impl AccessMarket {
    /// Build the market for every poured net of `pcb`.
    pub fn build(pcb: &Pcb, micro_cost: f64, via_cost: f64) -> Self {
        let session = RouteSession::from_pcb(pcb);
        let sizing = session.via_sizing().clone();
        let spans = span_catalog(pcb);
        let planes = pcb.plane_layers();
        let poured = plane_poured(pcb);
        let _ = {
            let mut m: BTreeMap<&str, Vec<PcbLayer>> = BTreeMap::new();
            for z in pcb
                .zones
                .iter()
                .filter(|z| !z.net.is_empty() && z.outline.len() >= 3)
            {
                let e = m.entry(z.net.as_str()).or_default();
                if !e.contains(&z.layer) {
                    e.push(z.layer);
                }
            }
            m
        };
        let mut mk = Self::default();
        let mut site_ix: HashMap<(i64, i64), usize> = HashMap::new();
        let clr = pcb.rules.default_rules.clearance;
        let h2h = pcb.rules.hole_to_hole;
        for fp in &pcb.footprints {
            for pad in &fp.pads {
                let Some(net) = pad.net.as_deref().filter(|n| poured.contains_key(n)) else {
                    continue;
                };
                if pad.drill.is_some() {
                    continue; // a plated hole reaches every layer
                }
                let c = crate::geometry::pad_world_position(fp, pad);
                let center = [c.x, c.y];
                let Some(&layer) = pad.layers.iter().find(|l| l.is_copper()) else {
                    continue;
                };
                // Only a plane-role pour serves a pad on its own layer: fills on
                // signal layers are cut up by the routing.
                if planes.contains(&layer) && zone_covers(pcb, net, layer, center) {
                    mk.served_by_pour += 1;
                    continue;
                }
                let pi = mk.pads.len();
                mk.pads.push(AccessPad {
                    net: net.to_string(),
                    center,
                    layer,
                });
                // candidate points: in the pad, then a dog-bone ring
                let mut pts = vec![center];
                for &r in &[0.35, 0.5, 0.7] {
                    for k in 0..8 {
                        let a = std::f64::consts::TAU * k as f64 / 8.0;
                        pts.push([center[0] + r * a.cos(), center[1] + r * a.sin()]);
                    }
                }
                let mut opts = Vec::new();
                for p in pts {
                    let pv = Vec2::new(p[0], p[1]);
                    for &(a, b) in &spans {
                        if !layer.spanned_by(a, b) || (layer != a && layer != b) {
                            continue;
                        }
                        // lands in a pour of its net on some spanned layer
                        let reach = poured[net]
                            .iter()
                            .any(|&pl| pl.spanned_by(a, b) && zone_covers(pcb, net, pl, p));
                        if !reach {
                            continue;
                        }
                        let spec = sizing.spec(net, a, b);
                        if !session.probe_via_span(pv, net, a, b).legal {
                            continue;
                        }
                        let ok = pcb
                            .stackup
                            .layers
                            .iter()
                            .map(|l| l.layer)
                            .filter(|l| l.is_copper() && l.spanned_by(a, b))
                            .all(|l| {
                                session
                                    .probe(
                                        &CopperGeom::Disc {
                                            center: pv,
                                            r: spec.r(),
                                        },
                                        l,
                                        net,
                                        clr,
                                    )
                                    .legal
                            });
                        let stub = ((p[0] - center[0]).powi(2) + (p[1] - center[1]).powi(2)).sqrt();
                        let stub_ok = stub < 1e-9
                            || session
                                .probe(
                                    &CopperGeom::Segment {
                                        a: Vec2::new(center[0], center[1]),
                                        b: pv,
                                        half_w: session.width_for(net, 0.1) / 2.0,
                                    },
                                    layer,
                                    net,
                                    clr,
                                )
                                .legal;
                        if !ok || !stub_ok {
                            continue;
                        }
                        let key = ((p[0] * 1e4).round() as i64, (p[1] * 1e4).round() as i64);
                        let site = *site_ix.entry(key).or_insert_with(|| {
                            mk.sites.push(p);
                            mk.sites.len() - 1
                        });
                        let micro = sizing.is_micro_span(a, b);
                        opts.push(AccessOption {
                            site,
                            span: (a, b),
                            cost: stub + if micro { micro_cost } else { via_cost },
                        });
                    }
                }
                mk.options.push(opts);
                let _ = pi;
            }
        }
        // Sites closer than a hole pitch conflict; a used site blocks the
        // others near it. Exactness needs a compatible site set: drop, per
        // conflicting pair, the site with fewer pads that can use it
        // (deterministic), and keep every pad's options pointing at survivors.
        let pitch = {
            let s = sizing.max_spec("");
            (s.diameter + clr).max(s.drill + h2h)
        };
        let mut uses = vec![0usize; mk.sites.len()];
        for o in mk.options.iter().flatten() {
            uses[o.site] += 1;
        }
        let mut order: Vec<usize> = (0..mk.sites.len()).collect();
        // pad-center sites first (via in pad), then most useful
        let is_center: Vec<bool> = {
            let mut v = vec![false; mk.sites.len()];
            for (pad, opts) in mk.pads.iter().zip(&mk.options) {
                for o in opts {
                    let s = mk.sites[o.site];
                    if (s[0] - pad.center[0]).abs() < 1e-9 && (s[1] - pad.center[1]).abs() < 1e-9 {
                        v[o.site] = true;
                    }
                }
            }
            v
        };
        order.sort_by(|&a, &b| {
            is_center[b]
                .cmp(&is_center[a])
                .then(uses[b].cmp(&uses[a]))
                .then(a.cmp(&b))
        });
        let mut keep = vec![false; mk.sites.len()];
        let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        let cell = |p: [f64; 2]| ((p[0] / pitch).floor() as i64, (p[1] / pitch).floor() as i64);
        for s in order {
            let p = mk.sites[s];
            let k = cell(p);
            let clash = (-1..=1).any(|dx| {
                (-1..=1).any(|dy| {
                    grid.get(&(k.0 + dx, k.1 + dy)).is_some_and(|v| {
                        v.iter().any(|&o| {
                            let q = mk.sites[o];
                            ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2)).sqrt() < pitch
                        })
                    })
                })
            });
            if !clash {
                keep[s] = true;
                grid.entry(k).or_default().push(s);
            }
        }
        for opts in mk.options.iter_mut() {
            opts.retain(|o| keep[o.site]);
            opts.sort_by(|a, b| a.cost.total_cmp(&b.cost).then(a.site.cmp(&b.site)));
        }
        mk
    }

    /// Ascending auction. Returns the chosen access per pad (None = no
    /// option left) and the per-iteration statistics.
    pub fn solve(&self, iters: usize, alpha: f64) -> (Vec<Option<AccessOption>>, Vec<AccessIter>) {
        let mut price = vec![0.0f64; self.sites.len()];
        let mut stats = Vec::new();
        let mut best: Option<(f64, Vec<Option<AccessOption>>)> = None;
        let mut best_lb = f64::NEG_INFINITY;
        for it in 0..iters {
            let choice: Vec<Option<AccessOption>> = self
                .options
                .iter()
                .map(|opts| {
                    opts.iter()
                        .min_by(|a, b| {
                            (a.cost + price[a.site])
                                .total_cmp(&(b.cost + price[b.site]))
                                .then(a.site.cmp(&b.site))
                        })
                        .copied()
                })
                .collect();
            let mut demand = vec![0usize; self.sites.len()];
            for o in choice.iter().flatten() {
                demand[o.site] += 1;
            }
            let lb: f64 = choice
                .iter()
                .flatten()
                .map(|o| o.cost + price[o.site])
                .sum::<f64>()
                - price.iter().sum::<f64>();
            best_lb = best_lb.max(lb);
            let cost: f64 = choice.iter().flatten().map(|o| o.cost).sum();
            // greedy repair: each over-demanded site keeps its cheapest
            // user; the others take their best free option in pad order
            let mut taken = vec![false; self.sites.len()];
            let mut feas: Vec<Option<AccessOption>> = vec![None; self.pads.len()];
            let mut idx: Vec<usize> = (0..self.pads.len()).collect();
            idx.sort_by(|&a, &b| {
                let ca = choice[a].map(|o| o.cost).unwrap_or(f64::INFINITY);
                let cb = choice[b].map(|o| o.cost).unwrap_or(f64::INFINITY);
                // pads with the fewest options first
                self.options[a]
                    .len()
                    .cmp(&self.options[b].len())
                    .then(ca.total_cmp(&cb))
                    .then(a.cmp(&b))
            });
            for p in idx {
                let pick = std::iter::once(choice[p])
                    .flatten()
                    .chain(self.options[p].iter().copied())
                    .find(|o| !taken[o.site]);
                if let Some(o) = pick {
                    taken[o.site] = true;
                    feas[p] = Some(o);
                }
            }
            // A pad the repair could not serve counts at a fixed penalty so
            // the bound comparison stays on the same pad set.
            let ub: f64 = feas
                .iter()
                .zip(&self.options)
                .map(|(f, opts)| match f {
                    Some(o) => o.cost,
                    None if opts.is_empty() => 0.0,
                    None => 100.0,
                })
                .sum();
            let conflicts = demand.iter().filter(|&&d| d > 1).count();
            stats.push(AccessIter {
                iter: it,
                conflicts,
                cost,
                lower_bound: best_lb,
                upper_bound: ub,
                gap: ((ub - best_lb) / ub.max(1e-9)).max(0.0),
            });
            if best.as_ref().is_none_or(|b| ub < b.0) {
                best = Some((ub, feas));
            }
            if conflicts == 0 {
                break;
            }
            let a = alpha / ((it + 1) as f64).sqrt();
            for (p, &d) in price.iter_mut().zip(&demand) {
                *p = (*p + a * (d as f64 - 1.0)).max(0.0);
            }
        }
        (best.map(|b| b.1).unwrap_or_default(), stats)
    }

    /// The chosen vias.
    pub fn vias(&self, choice: &[Option<AccessOption>]) -> Vec<AccessVia> {
        choice
            .iter()
            .enumerate()
            .filter_map(|(i, o)| {
                o.map(|o| AccessVia {
                    pad: i,
                    pos: self.sites[o.site],
                    span: o.span,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 4 layers, In1 a GND plane: GND pads on F get a microvia each (in the
    /// pad), the auction clears with a zero gap, and a signal pad is left
    /// alone.
    #[test]
    fn gnd_pads_get_in_pad_microvias() {
        let mut pcb =
            crate::router::flow::test_board(20.0, 20.0, &["FCu", "In1Cu", "In2Cu", "BCu"]);
        pcb.stackup.layers[1].role = Some(vcad_ir::ecad::LayerRole::Plane { net: "GND".into() });
        pcb.rules.min_microvia_diameter = 0.2;
        pcb.rules.min_microvia_drill = 0.1;
        pcb.rules.default_rules.via_diameter = 0.3;
        pcb.rules.default_rules.via_drill = 0.15;
        pcb.rules.min_drill = 0.1;
        pcb.zones.push(
            serde_json::from_value(serde_json::json!({
                "outline": [{"x":0.5,"y":0.5},{"x":19.5,"y":0.5},{"x":19.5,"y":19.5},{"x":0.5,"y":19.5}],
                "net": "GND", "layer": "In1Cu", "clearance": 0.2, "minArea": 0.0,
                "fillType": "Solid", "thermalRelief": "Relief", "priority": 0
            }))
            .unwrap(),
        );
        let pad = |num: &str, x: f64, net: &str| {
            serde_json::json!({"number": num, "padType": "SMD",
                "shape": {"type": "Rect", "width": 1.0, "height": 1.0},
                "position": {"x": x, "y": 10.0}, "rotation": 0.0,
                "net": net, "layers": ["FCu"]})
        };
        let fp: vcad_ir::ecad::Footprint = serde_json::from_value(serde_json::json!({
            "ref": "U1", "value": "", "footprintName": "T", "position": {"x": 0.0, "y": 0.0},
            "rotation": 0.0, "front": true, "graphics": [],
            "pads": [pad("1", 5.0, "GND"), pad("2", 8.0, "GND"), pad("3", 11.0, "GND"),
                     pad("4", 14.0, "SIG")]
        }))
        .unwrap();
        pcb.footprints.push(fp);
        let mk = AccessMarket::build(&pcb, 1.0, 2.0);
        assert_eq!(mk.pads.len(), 3, "only the GND pads need plane access");
        let (choice, stats) = mk.solve(50, 2.0);
        assert!(stats.last().unwrap().gap < 1e-9, "{stats:?}");
        let vias = mk.vias(&choice);
        assert_eq!(vias.len(), 3);
        for v in &vias {
            assert_eq!(
                v.span,
                (PcbLayer::FCu, PcbLayer::In1Cu),
                "cheapest: the microvia"
            );
            let c = mk.pads[v.pad].center;
            assert!((c[0] - v.pos[0]).abs() < 1e-9 && (c[1] - v.pos[1]).abs() < 1e-9);
        }
    }
}
