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
use crate::session::{RouteSession, SpanId};
use crate::spatial::{CopperElement, CopperGeom};
use vcad_ir::Vec2;

/// Safety margin (mm) on every wire-to-wire spacing: the funnel's bends and
/// the crossing angle are estimates, the DRC is exact.
pub const MARGIN: f64 = 0.012;

/// Replace each bend `P` of `path` that sits on one of `corners` by an arc of radius `r` around `P` on
/// the bend's outer side (chords at most 30° apart, on a radius enlarged so
/// every chord stays `r` from `P`). The funnel bends exactly at the points
/// that bound it — obstacle corners in centerline space — so a wire wider
/// than the graph's design width, or any wire plus a margin, has to wrap
/// them rather than touch them.
pub fn round_bends(path: &[[f64; 2]], corners: &[([f64; 2], [f64; 2], f64)]) -> Vec<[f64; 2]> {
    if path.len() < 3 || corners.is_empty() {
        return path.to_vec();
    }
    let mut out = vec![path[0]];
    for i in 1..path.len() - 1 {
        let (a, p, b) = (path[i - 1], path[i], path[i + 1]);
        let (la, lb) = (d2(a, p), d2(p, b));
        let Some(&(_, center, r)) = corners.iter().find(|c| d2(c.0, p) < 1e-7) else {
            out.push(p);
            continue;
        };
        if la < 1e-9 || lb < 1e-9 {
            out.push(p);
            continue;
        }
        let u = [(p[0] - a[0]) / la, (p[1] - a[1]) / la];
        let w = [(b[0] - p[0]) / lb, (b[1] - p[1]) / lb];
        let turn = u[0] * w[1] - u[1] * w[0]; // > 0: left turn
        let ang = (u[0] * w[0] + u[1] * w[1]).clamp(-1.0, 1.0).acos();
        if ang < 1e-6 || turn.abs() < 1e-12 {
            out.push(p);
            continue;
        }
        // outer normal: right of travel for a left turn, left for a right
        let n = |d: [f64; 2]| {
            if turn > 0.0 {
                [d[1], -d[0]]
            } else {
                [-d[1], d[0]]
            }
        };
        let (n0, n1) = (n(u), n(w));
        let a0 = n0[1].atan2(n0[0]);
        let mut sweep = n1[1].atan2(n1[0]) - a0;
        while sweep > std::f64::consts::PI {
            sweep -= std::f64::consts::TAU;
        }
        while sweep < -std::f64::consts::PI {
            sweep += std::f64::consts::TAU;
        }
        let k = ((sweep.abs() / (std::f64::consts::PI / 6.0)).ceil() as usize).max(1);
        let rr = r / (sweep.abs() / k as f64 / 2.0).cos();
        for j in 0..=k {
            let t = a0 + sweep * j as f64 / k as f64;
            let rad = rr;
            out.push([center[0] + rad * t.cos(), center[1] + rad * t.sin()]);
        }
    }
    out.push(*path.last().unwrap());
    out
}
use super::funnel::{funnel, portal_params, Portal};

/// One crossing of a wire word.
#[derive(Debug, Clone, Copy)]
pub struct Wx {
    /// Cut crossed.
    pub cut: u32,
    /// Face the wire leaves.
    pub from: u32,
    /// Track width where the wire crosses this cut (its neck width near
    /// its own pads, else its class width).
    pub w: f64,
    /// Free extent on the cut (centerline space of the graph's design
    /// width), mm from the cut's `a` vertex; `lo == hi` pins the crossing.
    pub lo: f64,
    /// Upper end of the extent.
    pub hi: f64,
    /// Room the wire keeps from the extent's low end (its extra half-width
    /// over the design width plus margin; a via barrel's radius when the end
    /// is a used via site). The end is an obstacle corner, so a bend there
    /// wraps the corner at this radius.
    pub pad: f64,
    /// Room kept from the high end.
    pub pad_hi: f64,
}

impl Wire {
    /// Width of the segment `a`–`b`: the neck width when both ends lie in
    /// one pad's neck region, else the class width.
    pub fn seg_width(&self, a: [f64; 2], b: [f64; 2]) -> f64 {
        match &self.neck {
            Some((nw, r, cs)) if cs.iter().any(|c| d2(*c, a) <= *r && d2(*c, b) <= *r) => *nw,
            _ => self.width,
        }
    }
}

impl Wx {
    /// Usable interval: the extent less `pad` at both ends.
    pub fn span(&self) -> (f64, f64) {
        let (a, b) = (self.lo + self.pad, self.hi - self.pad_hi);
        if a <= b {
            (a, b)
        } else {
            let m = (self.lo + self.hi) / 2.0;
            (m, m)
        }
    }
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
    /// Net name (for the oracle).
    pub net_name: String,
    /// Fixed first / last interior point (a pad's interior face centroid):
    /// the wire runs start → lead → funnel → tail → end, and the gate checks
    /// the legs too.
    pub lead: Option<[f64; 2]>,
    /// See `lead`.
    pub tail: Option<[f64; 2]>,
    /// Neck: (width, radius, pad centres) — a segment with both ends within
    /// the radius of one of the centres is drawn at the neck width.
    pub neck: Option<(f64, f64, Vec<[f64; 2]>)>,
    /// A known-good starting polyline (the human's), used as the feasible
    /// start instead of the funnel through the starting crossing points.
    pub start_path: Option<Vec<[f64; 2]>>,
    /// Differential partner net and the pair gap (edge to edge) the two legs
    /// keep instead of the clearance.
    pub twin: Option<(u32, f64)>,
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

/// The point of copper `g` nearest to `p`.
fn nearest_point(g: &CopperGeom, p: [f64; 2]) -> [f64; 2] {
    let toward = |c: [f64; 2], r: f64| {
        let d = d2(c, p);
        if d < 1e-12 {
            c
        } else {
            [c[0] + (p[0] - c[0]) * r / d, c[1] + (p[1] - c[1]) * r / d]
        }
    };
    match *g {
        CopperGeom::Disc { center, r } => toward([center.x, center.y], r),
        CopperGeom::Segment { a, b, half_w } => {
            let ab = [b.x - a.x, b.y - a.y];
            let l2 = (ab[0] * ab[0] + ab[1] * ab[1]).max(1e-18);
            let t = (((p[0] - a.x) * ab[0] + (p[1] - a.y) * ab[1]) / l2).clamp(0.0, 1.0);
            toward([a.x + t * ab[0], a.y + t * ab[1]], half_w)
        }
        CopperGeom::Rect {
            center,
            half_w,
            half_h,
            rot,
        } => {
            let (sn, cs) = rot.sin_cos();
            let (dx, dy) = (p[0] - center.x, p[1] - center.y);
            let lx = (dx * cs + dy * sn).clamp(-half_w, half_w);
            let ly = (-dx * sn + dy * cs).clamp(-half_h, half_h);
            [center.x + lx * cs - ly * sn, center.y + lx * sn + ly * cs]
        }
    }
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
    oracle: Option<&RouteSession>,
) -> Vec<Vec<[f64; 2]>> {
    let need = |a: &Wire, wa: f64, b: &Wire, wb: f64, sin: f64| -> f64 {
        if a.net == b.net {
            0.0
        } else {
            let gap = match (a.twin, b.twin) {
                (Some((t, g)), _) if t == b.net => g,
                (_, Some((t, g))) if t == a.net => g,
                _ => a.clearance.max(b.clearance),
            };
            (wa / 2.0 + gap + wb / 2.0 + MARGIN) / sin.max(0.2)
        }
    };
    // Current crossing positions and angles.
    let mut pos: Vec<Vec<(f64, f64)>> = Vec::with_capacity(wires.len());
    for (wi, w) in wires.iter().enumerate() {
        let free: Vec<Portal> =
            w.xs.iter()
                .map(|x| {
                    let (a, b) = x.span();
                    portal(cg, w, x, a, b)
                })
                .collect();
        let path = funnel(w.lead.unwrap_or(w.start), w.tail.unwrap_or(w.end), &free);
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
                    wires[a.0].xs[a.1].w,
                    &wires[b.0],
                    wires[b.0].xs[b.1].w,
                    pos[a.0][a.1].1.min(pos[b.0][b.1].1),
                )
            };
            let lo = |i: usize| wires[g[i].0].xs[g[i].1].span().0;
            let hi = |i: usize| wires[g[i].0].xs[g[i].1].span().1;
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
                let (mut lo, mut hi) = x.span();
                if let Some((lw, lx)) = l {
                    let p = pos[lw][lx];
                    lo = lo.max(
                        p.0 + need(
                            &wires[lw],
                            wires[lw].xs[lx].w,
                            w,
                            x.w,
                            p.1.min(pos[wi][xi].1),
                        ),
                    );
                }
                if let Some((rw, rx)) = r {
                    let p = pos[rw][rx];
                    hi = hi.min(
                        p.0 - need(
                            &wires[rw],
                            wires[rw].xs[rx].w,
                            w,
                            x.w,
                            p.1.min(pos[wi][xi].1),
                        ),
                    );
                }
                if lo > hi {
                    // squeezed: stay where the (feasible) start put it
                    let cur = pos[wi][xi].0;
                    (cur, cur)
                } else {
                    (lo, hi)
                }
            })
            .collect()
    };
    // Exact gate: every wire's current polyline lives in a copy of the
    // oracle; a relaxation step is kept only if it adds no violation against
    // fixed copper and the other wires (the start is feasible, so the end is).
    let mut sess = oracle.cloned();
    let mut spans: Vec<Vec<SpanId>> = vec![Vec::new(); wires.len()];
    let layer_of = |w: &Wire| cg.layers[w.layer].layer;
    let seg = |a: [f64; 2], b: [f64; 2], w: &Wire| CopperGeom::Segment {
        a: Vec2::new(a[0], a[1]),
        b: Vec2::new(b[0], b[1]),
        half_w: w.seg_width(a, b) / 2.0,
    };
    // the nets a polyline violates clearance to, and how many segments do
    // per offending net, the closest approach of a polyline
    let bad =
        |s: &RouteSession, w: &Wire, p: &[[f64; 2]]| -> std::collections::BTreeMap<String, f64> {
            let mut nets: std::collections::BTreeMap<String, f64> =
                std::collections::BTreeMap::new();
            for q in p.windows(2).filter(|q| d2(q[0], q[1]) > 1e-9) {
                let r = s.probe(&seg(q[0], q[1], w), layer_of(w), &w.net_name, w.clearance);
                for b in r.blockers {
                    let e = nets.entry(b.net).or_insert(f64::INFINITY);
                    *e = e.min(b.distance);
                }
            }
            nets
        };
    let commit = |s: &mut RouteSession, w: &Wire, p: &[[f64; 2]]| -> Vec<SpanId> {
        p.windows(2)
            .filter(|q| d2(q[0], q[1]) > 1e-9)
            .map(|q| {
                let hw = w.seg_width(q[0], q[1]) / 2.0;
                s.commit(CopperElement {
                    min: [q[0][0].min(q[1][0]) - hw, q[0][1].min(q[1][1]) - hw],
                    max: [q[0][0].max(q[1][0]) + hw, q[0][1].max(q[1][1]) + hw],
                    net: w.net_name.clone(),
                    layer: layer_of(w),
                    geom: seg(q[0], q[1], w),
                })
            })
            .collect()
    };
    let mut paths: Vec<Vec<[f64; 2]>> = Vec::with_capacity(wires.len());
    for (wi, w) in wires.iter().enumerate() {
        let ps: Vec<Portal> =
            w.xs.iter()
                .zip(&pos[wi])
                .map(|(x, p)| portal(cg, w, x, p.0, p.0))
                .collect();
        let mut p = vec![w.start];
        p.extend(funnel(
            w.lead.unwrap_or(w.start),
            w.tail.unwrap_or(w.end),
            &ps,
        ));
        if w.tail.is_some() {
            p.push(w.end);
        }
        p.dedup_by(|a, b| d2(*a, *b) < 1e-9);
        if let Some(sp) = &w.start_path {
            p = sp.clone();
        }
        if let Some(s) = sess.as_mut() {
            spans[wi] = commit(s, w, &p);
        }
        paths.push(p);
    }
    // One candidate for wire `wi` inside intervals `ivs`: funnel, then wrap
    // the obstacle corners it bends at.
    let build = |wi: usize, ivs: &[(f64, f64)]| -> (Vec<[f64; 2]>, Vec<(f64, f64)>) {
        let w = &wires[wi];
        let ps: Vec<Portal> =
            w.xs.iter()
                .zip(ivs)
                .map(|(x, &(lo, hi))| portal(cg, w, x, lo, hi))
                .collect();
        let path = funnel(w.lead.unwrap_or(w.start), w.tail.unwrap_or(w.end), &ps);
        let cr = crossings(cg, w, &ps, &path);
        let mut p = vec![w.start];
        p.extend(path);
        if w.tail.is_some() {
            p.push(w.end);
        }
        p.dedup_by(|a, b| d2(*a, *b) < 1e-9);
        let mut corners: Vec<([f64; 2], [f64; 2], f64)> = Vec::new();
        for (x, &(lo, hi)) in w.xs.iter().zip(ivs) {
            if x.hi - x.lo < x.pad + x.pad_hi {
                continue;
            }
            let lg = &cg.layers[w.layer];
            let c = &lg.cuts[x.cut as usize];
            let at = |s: f64| {
                let (a, b) = (lg.vpos(c.a), lg.vpos(c.b));
                let u = (s / c.len.max(1e-12)).clamp(0.0, 1.0);
                [a[0] + u * (b[0] - a[0]), a[1] + u * (b[1] - a[1])]
            };
            let (el, eh) = x.span();
            if x.pad > 0.0 && (lo - el).abs() < 1e-9 {
                corners.push((at(lo), at(x.lo), x.pad));
            }
            if x.pad_hi > 0.0 && (hi - eh).abs() < 1e-9 {
                corners.push((at(hi), at(x.hi), x.pad_hi));
            }
        }
        (round_bends(&p, &corners), cr)
    };
    // Gate: swap in `cand` if it gains no offending net and gets no closer
    // to an old one (`strict`: also strictly fewer offending nets).
    let try_swap = |s: &mut RouteSession,
                    spans: &mut Vec<Vec<SpanId>>,
                    paths: &mut Vec<Vec<[f64; 2]>>,
                    wi: usize,
                    cand: &[[f64; 2]],
                    strict: bool|
     -> bool {
        let w = &wires[wi];
        for id in spans[wi].drain(..) {
            s.remove(id);
        }
        let new = bad(s, w, cand);
        let old = bad(s, w, &paths[wi]);
        let mut ok = new
            .iter()
            .all(|(n, d)| old.get(n).is_some_and(|o| *d >= o - 1e-6));
        if strict {
            let sum = |m: &std::collections::BTreeMap<String, f64>| {
                m.values().map(|d| (w.clearance - d).max(0.0)).sum::<f64>()
            };
            ok = ok && (new.len() < old.len() || sum(&new) < sum(&old) - 1e-6);
        }
        if ok {
            spans[wi] = commit(s, w, cand);
            paths[wi] = cand.to_vec();
        } else {
            let keep = paths[wi].clone();
            spans[wi] = commit(s, w, &keep);
        }
        ok
    };
    // Sequential thick-path placement against committed copper: wires go in
    // the embedding's left-to-right order; each one's corridor on every cut
    // is the part of its sleeve clear of fixed copper and of the wires
    // already placed, both inflated by its own half-width + clearance, and
    // inside the room its placed neighbours leave. The funnel runs on that
    // inflated geometry; the oracle keeps the placed path only if it offends
    // no more than its current path.
    if let (Some(s0), true) = (oracle, sess.is_some()) {
        let mut s2 = s0.clone();
        let mut rank = vec![(0.0f64, 0usize); wires.len()];
        for key in &keys {
            let g = &members[key];
            let n = g.len().max(2) - 1;
            for (i, &(wi, _)) in g.iter().enumerate() {
                rank[wi].0 += i as f64 / n as f64;
                rank[wi].1 += 1;
            }
        }
        let mut seq: Vec<usize> = (0..wires.len()).collect();
        let key_of = |wi: usize| rank[wi].0 / rank[wi].1.max(1) as f64;
        seq.sort_by(|&a, &b| key_of(a).total_cmp(&key_of(b)).then(a.cmp(&b)));
        let mut placed = vec![false; wires.len()];
        let (mut seq_won, mut seq_kept) = (0usize, 0usize);
        for wi in seq {
            let w = &wires[wi];
            let lg = &cg.layers[w.layer];
            let mut wraps: Vec<([f64; 2], [f64; 2], f64)> = Vec::new();
            let ivs: Vec<(f64, f64)> =
                w.xs.iter()
                    .enumerate()
                    .map(|(xi, x)| {
                        let (mut lo, mut hi) = x.span();
                        let (l, r) = nbr[wi][xi];
                        if let Some((lw, lx)) = l.filter(|o| placed[o.0]) {
                            let p = pos[lw][lx];
                            lo = lo.max(
                                p.0 + need(
                                    &wires[lw],
                                    wires[lw].xs[lx].w,
                                    w,
                                    x.w,
                                    p.1.min(pos[wi][xi].1),
                                ),
                            );
                        }
                        if let Some((rw, rx)) = r.filter(|o| placed[o.0]) {
                            let p = pos[rw][rx];
                            hi = hi.min(
                                p.0 - need(
                                    &wires[rw],
                                    wires[rw].xs[rx].w,
                                    w,
                                    x.w,
                                    p.1.min(pos[wi][xi].1),
                                ),
                            );
                        }
                        let cur = pos[wi][xi].0;
                        if lo > hi {
                            return (cur, cur);
                        }
                        let c = &lg.cuts[x.cut as usize];
                        let (a, b) = (lg.vpos(c.a), lg.vpos(c.b));
                        let at = |t: f64| {
                            let u = (t / c.len.max(1e-12)).clamp(0.0, 1.0);
                            [a[0] + u * (b[0] - a[0]), a[1] + u * (b[1] - a[1])]
                        };
                        let (pa, pb) = (at(lo), at(hi));
                        let m = x.w / 2.0 + w.clearance + 0.5;
                        let bl = [pa[0].min(pb[0]) - m, pa[1].min(pb[1]) - m];
                        let bh = [pa[0].max(pb[0]) + m, pa[1].max(pb[1]) + m];
                        let mut obs: Vec<(CopperGeom, f64)> = Vec::new();
                        s2.for_each_blocking(layer_of(w), &w.net_name, bl, bh, |og, _, _, req| {
                            obs.push((*og, req.max(w.clearance)));
                        });
                        let clear = |t: f64| {
                            let q = at(t);
                            let d = CopperGeom::Disc {
                                center: Vec2::new(q[0], q[1]),
                                r: x.w / 2.0,
                            };
                            obs.iter()
                                .all(|(o, req)| d.distance_to(o) >= req + MARGIN / 2.0)
                        };
                        let n = (((hi - lo) / 0.005).ceil() as usize).clamp(1, 4000);
                        let mut runs: Vec<(f64, f64)> = Vec::new();
                        let mut open: Option<f64> = None;
                        for k in 0..=n {
                            let t = lo + (hi - lo) * k as f64 / n as f64;
                            match (clear(t), open) {
                                (true, None) => open = Some(t),
                                (false, Some(t0)) => {
                                    runs.push((t0, lo + (hi - lo) * (k - 1) as f64 / n as f64));
                                    open = None;
                                }
                                _ => {}
                            }
                        }
                        if let Some(t0) = open {
                            runs.push((t0, hi));
                        }
                        let Some(run) = runs.into_iter().min_by(|p, q| {
                            let dp = (cur.max(p.0).min(p.1.max(p.0)) - cur).abs();
                            let dq = (cur.max(q.0).min(q.1.max(q.0)) - cur).abs();
                            dp.total_cmp(&dq)
                        }) else {
                            return (lo, hi);
                        };
                        // an obstacle-bounded end: wrap the obstacle's
                        // nearest copper at the distance the bound keeps
                        let step = (hi - lo) / n as f64;
                        for (t, inside) in [(run.0, run.0 - step), (run.1, run.1 + step)] {
                            if inside < lo - 1e-12 || inside > hi + 1e-12 {
                                continue;
                            }
                            let q = at(t);
                            let qi = at(inside);
                            let near = obs
                                .iter()
                                .map(|(o, req)| {
                                    let c = nearest_point(o, qi);
                                    (c, req + x.w / 2.0 + MARGIN / 2.0, d2(c, qi))
                                })
                                .min_by(|a, b| a.2.total_cmp(&b.2));
                            if let Some((c, r, _)) = near {
                                wraps.push((q, c, r.max(d2(q, c))));
                            }
                        }
                        run
                    })
                    .collect();
            let (cand, cr) = build(wi, &ivs);
            let cand = round_bends(&cand, &wraps);
            // exact gate on the full board (every other wire where it is now)
            let s = sess.as_mut().unwrap();
            if try_swap(s, &mut spans, &mut paths, wi, &cand, false) {
                for (q, c) in pos[wi].iter_mut().zip(cr) {
                    *q = c;
                }
                seq_won += 1;
            } else {
                seq_kept += 1;
            }
            let p = paths[wi].clone();
            commit(&mut s2, w, &p);
            placed[wi] = true;
        }
        log::info!(
            "realize: sequential placement took {seq_won} paths, kept {seq_kept} relaxed starts"
        );
    }
    for _ in 0..sweeps {
        for wi in 0..wires.len() {
            let ivs = channel(&pos, wi);
            let (cand, cr) = build(wi, &ivs);
            let accept = match sess.as_mut() {
                None => {
                    paths[wi] = cand;
                    true
                }
                Some(s) => try_swap(s, &mut spans, &mut paths, wi, &cand, false),
            };
            if accept {
                for (q, c) in pos[wi].iter_mut().zip(cr) {
                    *q = c;
                }
            }
        }
    }
    // Repair: a wire still offending something moves its crossings next to
    // the offence away from the offending copper by the shortfall, and keeps
    // the move only if it strictly helps.
    if let Some(s) = sess.as_mut() {
        for wi in 0..wires.len() {
            let w = &wires[wi];
            let lg = &cg.layers[w.layer];
            let mut ivs = channel(&pos, wi);
            for _ in 0..6 {
                let mut moved = false;
                for q in paths[wi].clone().windows(2) {
                    if d2(q[0], q[1]) < 1e-9 {
                        continue;
                    }
                    let g = seg(q[0], q[1], w);
                    let lo = [q[0][0].min(q[1][0]) - 0.5, q[0][1].min(q[1][1]) - 0.5];
                    let hi = [q[0][0].max(q[1][0]) + 0.5, q[0][1].max(q[1][1]) + 0.5];
                    // the closest offending copper and how short we fall
                    let mut worst: Option<([f64; 2], f64)> = None;
                    s.for_each_blocking(layer_of(w), &w.net_name, lo, hi, |og, _, _, req| {
                        let d = g.distance_to(og);
                        let need = req.max(w.clearance) + 0.004;
                        if d < need {
                            let c = match *og {
                                CopperGeom::Disc { center, .. } => [center.x, center.y],
                                CopperGeom::Rect { center, .. } => [center.x, center.y],
                                CopperGeom::Segment { a, b, .. } => {
                                    let m = [(q[0][0] + q[1][0]) / 2.0, (q[0][1] + q[1][1]) / 2.0];
                                    let ab = [b.x - a.x, b.y - a.y];
                                    let l2 = (ab[0] * ab[0] + ab[1] * ab[1]).max(1e-18);
                                    let t = (((m[0] - a.x) * ab[0] + (m[1] - a.y) * ab[1]) / l2)
                                        .clamp(0.0, 1.0);
                                    [a.x + t * ab[0], a.y + t * ab[1]]
                                }
                            };
                            if worst.is_none_or(|w0| need - d > w0.1) {
                                worst = Some((c, need - d));
                            }
                        }
                    });
                    let Some((c, short)) = worst else { continue };
                    for (xi, x) in w.xs.iter().enumerate() {
                        let cut = &lg.cuts[x.cut as usize];
                        let (a, b) = (lg.vpos(cut.a), lg.vpos(cut.b));
                        let sx = pos[wi][xi].0;
                        let at = |t: f64| {
                            let u = (t / cut.len.max(1e-12)).clamp(0.0, 1.0);
                            [a[0] + u * (b[0] - a[0]), a[1] + u * (b[1] - a[1])]
                        };
                        if seg_dist(at(sx), q[0], q[1]) > 1e-6 {
                            continue;
                        }
                        let step = short / pos[wi][xi].1.max(0.2);
                        let (lo_i, hi_i) = ivs[xi];
                        if d2(at(sx + 1e-3), c) > d2(at(sx - 1e-3), c) {
                            ivs[xi].0 = (sx + step).min(hi_i);
                        } else {
                            ivs[xi].1 = (sx - step).max(lo_i);
                        }
                        moved = true;
                    }
                }
                if !moved {
                    break;
                }
                let (cand, cr) = build(wi, &ivs);
                if try_swap(s, &mut spans, &mut paths, wi, &cand, true) {
                    for (q, c) in pos[wi].iter_mut().zip(cr) {
                        *q = c;
                    }
                } else {
                    break;
                }
            }
        }
    }
    if let Some(s) = sess.as_ref() {
        let dirty = (0..wires.len())
            .filter(|&wi| !bad(s, &wires[wi], &paths[wi]).is_empty())
            .count();
        log::info!(
            "realize: {dirty}/{} wires still offend after relax + repair",
            wires.len()
        );
        if log::log_enabled!(log::Level::Debug) {
            for wi in 0..wires.len() {
                let b = bad(s, &wires[wi], &paths[wi]);
                if !b.is_empty() {
                    log::debug!(
                        "realize dirty {} (w {}, {} pts): {:?}",
                        wires[wi].net_name,
                        wires[wi].width,
                        paths[wi].len(),
                        b
                    );
                }
            }
        }
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::router::flow::cdt::{CopperFilter, CutGraph, CutParams};

    /// A bend at a corner becomes an arc that keeps every chord at least the
    /// radius from the corner, on the outside of the turn.
    #[test]
    fn bends_wrap_corners() {
        let corner = [1.0, 0.0];
        // bend point 0.1 above the corner, path going over it
        let p = vec![[0.0, -0.5], [1.0, 0.1], [2.0, -0.5]];
        let out = round_bends(&p, &[([1.0, 0.1], corner, 0.1)]);
        assert!(out.len() > 3);
        for s in out.windows(2) {
            assert!(seg_dist(corner, s[0], s[1]) >= 0.1 - 1e-9, "{out:?}");
        }
        // a bend not on a corner is left alone
        assert_eq!(round_bends(&p, &[]), p);
    }

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
                        w: 0.2,
                        lo: q[0],
                        hi: q[1],
                        pad: 0.0,
                        pad_hi: 0.0,
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
            net_name: format!("N{net}"),
            start_path: None,
            lead: None,
            tail: None,
            neck: None,
            twin: None,
            start: [3.0, y],
            end: [17.0, y],
            xs: word([3.0, y], [17.0, y]),
        };
        let wires = vec![mk(9.9, 0), mk(10.1, 1)];
        let polys = realize_wires(&cg, &wires, &HashMap::new(), None, 3, None);
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
