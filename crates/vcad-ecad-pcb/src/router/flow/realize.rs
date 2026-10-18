//! Geometry of a set of per-layer wire words.
//!
//! A wire's crossing of a cut is an allowed interval on the cut (a free part
//! shrunk for its width, or a single pinned point). Wires crossing one cut
//! keep a given order along it (the embedding's, or the human's). Two wires
//! of different nets crossing a cut `Δ` apart along it, at angle `θ`, are
//! `Δ·sin θ` apart there, so neighbours need `Δ ≥ (w₁/2 + s + w₂/2) / sin θ`.
//!
//! Realization is a rubber-band relaxation: start from a feasible set of
//! crossing points (the human's, or the order spread out along each cut),
//! then repeatedly re-funnel each wire inside the channel its two
//! neighbours leave it on every cut. A wire only ever moves within its
//! current channel, so a feasible start stays feasible on every cut while
//! the wires shorten.

use std::collections::HashMap;

use super::cdt::CutGraph;

/// Safety margin (mm) on every wire-to-wire spacing: the funnel's bends and
/// the crossing angle are estimates, the DRC is exact.
pub const MARGIN: f64 = 0.012;
use super::funnel::{funnel, portal_params, Portal};

/// One crossing of a wire word.
#[derive(Debug, Clone, Copy)]
pub struct Wx {
    /// Cut crossed.
    pub cut: u32,
    /// Face the wire leaves.
    pub from: u32,
    /// Allowed centerline interval, mm from the cut's `a` vertex.
    pub lo: f64,
    /// Upper end of the interval.
    pub hi: f64,
}

/// One per-layer wire word.
#[derive(Debug, Clone)]
pub struct Wire {
    /// Graph layer.
    pub layer: usize,
    /// Track width (mm).
    pub width: f64,
    /// Clearance it needs to other nets' copper (mm).
    pub clearance: f64,
    /// Net id (wires of one net need no spacing).
    pub net: u32,
    /// Start point.
    pub start: [f64; 2],
    /// End point.
    pub end: [f64; 2],
    /// Crossings in order.
    pub xs: Vec<Wx>,
}

fn d2(a: [f64; 2], b: [f64; 2]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

fn seg_dist(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let ab = [b[0] - a[0], b[1] - a[1]];
    let l2 = ab[0] * ab[0] + ab[1] * ab[1];
    let t = if l2 < 1e-18 {
        0.0
    } else {
        (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1]) / l2).clamp(0.0, 1.0)
    };
    d2(p, [a[0] + t * ab[0], a[1] + t * ab[1]])
}

fn portal(cg: &CutGraph, w: &Wire, x: &Wx, lo: f64, hi: f64) -> Portal {
    let lg = &cg.layers[w.layer];
    let c = &lg.cuts[x.cut as usize];
    let t = lg.tris[x.from as usize];
    let kk = (0..3)
        .find(|&i| lg.tri_cut[x.from as usize][i] == x.cut)
        .unwrap_or(0);
    let (pa, pb) = (lg.vpos(c.a), lg.vpos(c.b));
    let at = |s: f64| {
        let u = (s / c.len.max(1e-12)).clamp(0.0, 1.0);
        [pa[0] + u * (pb[0] - pa[0]), pa[1] + u * (pb[1] - pa[1])]
    };
    // Leaving `from` through edge (v_k → v_k+1): v_k is on the right.
    if t[kk] == c.a {
        Portal {
            left: at(hi),
            right: at(lo),
        }
    } else {
        Portal {
            left: at(lo),
            right: at(hi),
        }
    }
}

/// Where `path` crosses each portal: (mm from the cut's `a`, sin of the
/// crossing angle).
fn crossings(cg: &CutGraph, w: &Wire, ps: &[Portal], path: &[[f64; 2]]) -> Vec<(f64, f64)> {
    let lg = &cg.layers[w.layer];
    let ts = portal_params(path, ps);
    w.xs.iter()
        .zip(ps)
        .zip(ts)
        .map(|((x, p), t)| {
            let c = &lg.cuts[x.cut as usize];
            let q = [
                p.left[0] + t * (p.right[0] - p.left[0]),
                p.left[1] + t * (p.right[1] - p.left[1]),
            ];
            let (a, b) = (lg.vpos(c.a), lg.vpos(c.b));
            let cd = [(b[0] - a[0]) / c.len, (b[1] - a[1]) / c.len];
            let sin = path
                .windows(2)
                .filter(|s| d2(s[0], s[1]) > 1e-9)
                .min_by(|u, v| seg_dist(q, u[0], u[1]).total_cmp(&seg_dist(q, v[0], v[1])))
                .map(|s| {
                    let l = d2(s[0], s[1]);
                    let d = [(s[1][0] - s[0][0]) / l, (s[1][1] - s[0][1]) / l];
                    (d[0] * cd[1] - d[1] * cd[0]).abs()
                })
                .unwrap_or(1.0);
            (d2(a, q), sin)
        })
        .collect()
}

/// Realize `wires`. `order[(layer, cut)]` lists the wires (indices) crossing
/// that cut in order from its `a` vertex; cuts without an entry order their
/// wires by position. `init`, when given, is a feasible starting crossing
/// position (mm from `a`) per wire per crossing — the human's; otherwise
/// the start is each cut's order spread out at the required spacing.
/// Returns one polyline per wire.
pub fn realize_wires(
    cg: &CutGraph,
    wires: &[Wire],
    order: &HashMap<(usize, u32), Vec<usize>>,
    init: Option<&[Vec<f64>]>,
    sweeps: usize,
) -> Vec<Vec<[f64; 2]>> {
    let need = |a: &Wire, b: &Wire, sin: f64| -> f64 {
        if a.net == b.net {
            0.0
        } else {
            (a.width / 2.0 + a.clearance.max(b.clearance) + b.width / 2.0 + MARGIN) / sin.max(0.2)
        }
    };
    // Current crossing positions and angles.
    let mut pos: Vec<Vec<(f64, f64)>> = Vec::with_capacity(wires.len());
    for (wi, w) in wires.iter().enumerate() {
        let free: Vec<Portal> = w.xs.iter().map(|x| portal(cg, w, x, x.lo, x.hi)).collect();
        let path = funnel(w.start, w.end, &free);
        let mut v = crossings(cg, w, &free, &path);
        if let Some(init) = init {
            for (p, s) in v.iter_mut().zip(&init[wi]) {
                p.0 = *s;
            }
        }
        pos.push(v);
    }
    // Who crosses each cut, in order.
    let mut members: HashMap<(usize, u32), Vec<(usize, usize)>> = HashMap::new();
    for (wi, w) in wires.iter().enumerate() {
        for (xi, x) in w.xs.iter().enumerate() {
            members.entry((w.layer, x.cut)).or_default().push((wi, xi));
        }
    }
    let mut keys: Vec<(usize, u32)> = members.keys().copied().collect();
    keys.sort_unstable();
    for key in &keys {
        let g = members.get_mut(key).unwrap();
        match order.get(key) {
            Some(ord) => {
                let rank: HashMap<usize, usize> =
                    ord.iter().enumerate().map(|(i, &w)| (w, i)).collect();
                g.sort_by(|a, b| {
                    let ra = rank.get(&a.0).copied().unwrap_or(usize::MAX);
                    let rb = rank.get(&b.0).copied().unwrap_or(usize::MAX);
                    ra.cmp(&rb)
                        .then(pos[a.0][a.1].0.total_cmp(&pos[b.0][b.1].0))
                        .then(a.0.cmp(&b.0))
                });
            }
            None => g.sort_by(|a, b| {
                pos[a.0][a.1]
                    .0
                    .total_cmp(&pos[b.0][b.1].0)
                    .then(a.0.cmp(&b.0))
            }),
        }
        if init.is_none() && g.len() > 1 {
            // Feasible start: the order spread at the required spacing.
            let n = g.len();
            let gp = |i: usize| {
                let (a, b) = (g[i], g[i + 1]);
                need(
                    &wires[a.0],
                    &wires[b.0],
                    pos[a.0][a.1].1.min(pos[b.0][b.1].1),
                )
            };
            let lo = |i: usize| wires[g[i].0].xs[g[i].1].lo;
            let hi = |i: usize| wires[g[i].0].xs[g[i].1].hi;
            let mut x: Vec<f64> = (0..n)
                .map(|i| pos[g[i].0][g[i].1].0.clamp(lo(i), hi(i)))
                .collect();
            for i in 1..n {
                let p = gp(i - 1);
                if hi(i - 1) + p > lo(i) {
                    x[i] = x[i].max(x[i - 1] + p);
                }
            }
            for i in (0..n - 1).rev() {
                let p = gp(i);
                if hi(i) + p > lo(i + 1) {
                    x[i] = x[i].min(x[i + 1] - p).max(lo(i));
                }
            }
            for (i, &(wi, xi)) in g.iter().enumerate() {
                pos[wi][xi].0 = x[i];
            }
        }
    }
    // Neighbours on each crossing (fixed by the order).
    type Side = Option<(usize, usize)>;
    let mut nbr: Vec<Vec<(Side, Side)>> = wires
        .iter()
        .map(|w| vec![(None, None); w.xs.len()])
        .collect();
    for key in &keys {
        let g = &members[key];
        for (i, &(wi, xi)) in g.iter().enumerate() {
            // nearest other-net neighbour on each side
            let left = g[..i]
                .iter()
                .rev()
                .find(|o| wires[o.0].net != wires[wi].net)
                .copied();
            let right = g[i + 1..]
                .iter()
                .find(|o| wires[o.0].net != wires[wi].net)
                .copied();
            nbr[wi][xi] = (left, right);
        }
    }
    let channel = |pos: &Vec<Vec<(f64, f64)>>, wi: usize| -> Vec<(f64, f64)> {
        let w = &wires[wi];
        w.xs.iter()
            .enumerate()
            .map(|(xi, x)| {
                let (l, r) = nbr[wi][xi];
                let mut lo = x.lo;
                let mut hi = x.hi;
                if let Some((lw, lx)) = l {
                    let p = pos[lw][lx];
                    lo = lo.max(p.0 + need(&wires[lw], w, p.1.min(pos[wi][xi].1)));
                }
                if let Some((rw, rx)) = r {
                    let p = pos[rw][rx];
                    hi = hi.min(p.0 - need(&wires[rw], w, p.1.min(pos[wi][xi].1)));
                }
                let cur = pos[wi][xi].0;
                if lo > hi {
                    (cur, cur) // squeezed: stay put
                } else {
                    (lo, hi)
                }
            })
            .collect()
    };
    let mut paths: Vec<Vec<[f64; 2]>> = vec![Vec::new(); wires.len()];
    for sweep in 0..sweeps.max(1) {
        for wi in 0..wires.len() {
            let w = &wires[wi];
            let ivs = channel(&pos, wi);
            let ps: Vec<Portal> =
                w.xs.iter()
                    .zip(&ivs)
                    .map(|(x, &(lo, hi))| portal(cg, w, x, lo, hi))
                    .collect();
            let path = funnel(w.start, w.end, &ps);
            let cr = crossings(cg, w, &ps, &path);
            for (p, c) in pos[wi].iter_mut().zip(cr) {
                *p = c;
            }
            if sweep + 1 == sweeps.max(1) {
                let mut p = vec![w.start];
                p.extend(path);
                p.dedup_by(|a, b| d2(*a, *b) < 1e-9);
                paths[wi] = p;
            }
        }
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::router::flow::cdt::{CopperFilter, CutGraph, CutParams};

    /// Two wires of different nets asked to run 0.2 mm apart through the
    /// same cuts come out at least `w + s` apart wherever they share one.
    #[test]
    fn neighbours_get_spaced_apart() {
        let mut pcb = crate::router::flow::test_board(20.0, 20.0, &["FCu"]);
        // a few pads so the triangulation has cuts across the corridor
        let pad = |num: &str, x: f64, y: f64| {
            serde_json::json!({"number": num, "padType": "SMD",
                "shape": {"type": "Circle", "diameter": 0.6},
                "position": {"x": x, "y": y}, "rotation": 0.0,
                "net": format!("P{num}"), "layers": ["FCu"]})
        };
        let fp: vcad_ir::ecad::Footprint = serde_json::from_value(serde_json::json!({
            "ref": "U1", "value": "", "footprintName": "T", "position": {"x": 0.0, "y": 0.0},
            "rotation": 0.0, "front": true, "graphics": [],
            "pads": [pad("1", 8.0, 12.0), pad("2", 12.0, 12.0), pad("3", 8.0, 8.0),
                     pad("4", 12.0, 8.0), pad("5", 10.0, 13.0), pad("6", 10.0, 7.0)]
        }))
        .unwrap();
        pcb.footprints.push(fp);
        let cp = CutParams::from_pcb(&pcb);
        let cg = CutGraph::build(&pcb, cp, CopperFilter::All);
        let lg = &cg.layers[0];
        let word = |a: [f64; 2], b: [f64; 2]| {
            let (xs, _) = lg.walk(a, b, None).expect("walk");
            let mut from = lg.locate(a).unwrap();
            xs.into_iter()
                .map(|x| {
                    let c = &lg.cuts[x.cut as usize];
                    let k = c.part_at(x.s).unwrap();
                    let q = c.parts[k];
                    let w = Wx {
                        cut: x.cut,
                        from,
                        lo: q[0],
                        hi: q[1],
                    };
                    from = x.into;
                    w
                })
                .collect::<Vec<_>>()
        };
        let mk = |y: f64, net: u32| Wire {
            layer: 0,
            width: 0.2,
            clearance: 0.2,
            net,
            start: [3.0, y],
            end: [17.0, y],
            xs: word([3.0, y], [17.0, y]),
        };
        let wires = vec![mk(9.9, 0), mk(10.1, 1)];
        let polys = realize_wires(&cg, &wires, &HashMap::new(), None, 3);
        // y of each wire where it passes x = 10 (between the pad pairs)
        let y_at = |p: &Vec<[f64; 2]>, x: f64| {
            p.windows(2)
                .find(|s| (s[0][0] - x) * (s[1][0] - x) <= 0.0 && s[0][0] != s[1][0])
                .map(|s| s[0][1] + (x - s[0][0]) / (s[1][0] - s[0][0]) * (s[1][1] - s[0][1]))
                .expect("wire passes x")
        };
        let gap = (y_at(&polys[1], 10.0) - y_at(&polys[0], 10.0)).abs();
        assert!(gap >= 0.4 - 1e-6, "gap at x=10: {gap}");
    }
}
