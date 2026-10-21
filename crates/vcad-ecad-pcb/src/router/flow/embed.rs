//! Planar embedding: route each commodity, in order, through the *gaps*
//! between the wires already embedded, at the market's prices.
//!
//! Cut capacities alone do not keep wires of different nets from crossing:
//! two shortest paths that share a bundle of cuts can enter it in one order
//! and leave it in the other. The embedding makes the order explicit. Every
//! free part of every cut keeps the ordered list of wires through it; a face
//! keeps the chords its wires draw between its edges. A new wire enters a
//! face at a gap and may leave through any gap (or via site, or terminal) that
//! no other-net chord separates it from — exactly the planar (rubber-band)
//! routing graph. Capacity is hard (a part never holds more wires than fit);
//! the market's λ steer the search toward the plan's corridors.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

use super::cdt::{orient, NONE};
use super::flow::{Commodity, Cross, FlowGraph, FlowParams};

/// One crossing of an embedded path.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EStep {
    /// Crossed `cut` on `layer` leaving face `from`, in free part `part`
    /// (`None` = own-copper crossing) as wire `wire`.
    Cross {
        /// Graph layer.
        layer: u8,
        /// Cut.
        cut: u32,
        /// Face left.
        from: u32,
        /// Free part used.
        part: Option<u16>,
    },
    /// Changed layers through a via.
    Via {
        /// Site.
        site: u32,
        /// Span index.
        span: u16,
        /// Layer left.
        from: u8,
        /// Layer entered.
        to: u8,
        /// Face entered on the new layer.
        face: u32,
    },
}

/// An embedded route.
#[derive(Debug, Clone)]
pub struct EPath {
    /// Graph node (face) the route starts in.
    pub start: u32,
    /// Graph node (face) it ends in.
    pub end: u32,
    /// Steps.
    pub steps: Vec<EStep>,
}

#[derive(Debug, Clone, Copy)]
enum End {
    /// An embedded wire's slot on a cut part.
    Slot {
        layer: u8,
        cut: u32,
        part: u16,
        wire: u32,
    },
    /// A gap between the wires of a cut part (a candidate crossing).
    Gap {
        layer: u8,
        cut: u32,
        part: u16,
        gap: u16,
    },
    /// A fixed point (pad center, via site vertex, own-copper end).
    Point([f64; 2]),
}

#[derive(Debug, Clone, Copy)]
struct Chord {
    a: End,
    b: End,
    net: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum St {
    /// In a terminal face.
    Start(u32),
    /// Just crossed `cut` part `part` at gap `gap` into face `face`.
    Cut {
        layer: u8,
        cut: u32,
        part: u16,
        gap: u16,
        face: u32,
    },
    /// Just crossed `cut` inside own copper into `face`.
    Free { layer: u8, cut: u32, face: u32 },
    /// At via site `site`, in face `face` of `layer` (arrived by via).
    Vert { layer: u8, site: u32, face: u32 },
}

#[derive(Copy, Clone, PartialEq)]
struct Item {
    f: f64,
    g: f64,
    id: u32,
}
impl Eq for Item {}
impl Ord for Item {
    fn cmp(&self, o: &Self) -> Ordering {
        o.f.total_cmp(&self.f).then_with(|| o.id.cmp(&self.id))
    }
}
impl PartialOrd for Item {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

fn proper_cross(p: [f64; 2], q: [f64; 2], a: [f64; 2], b: [f64; 2]) -> bool {
    let e = 1e-12;
    let d1 = orient(p, q, a);
    let d2 = orient(p, q, b);
    let d3 = orient(a, b, p);
    let d4 = orient(a, b, q);
    ((d1 > e && d2 < -e) || (d1 < -e && d2 > e)) && ((d3 > e && d4 < -e) || (d3 < -e && d4 > e))
}

/// The embedding state.
pub struct Embedding<'a, 'g> {
    g: &'a FlowGraph<'g>,
    /// Ordered wires (commodity index) through each free part.
    parts: HashMap<(u8, u32, u16), Vec<u32>>,
    chords: HashMap<(u8, u32), Vec<Chord>>,
    /// Net and span of the via at each used site.
    via_at: HashMap<u32, (u32, u16)>,
    /// Per layer: Steiner vertex → site.
    vsite: Vec<HashMap<u32, u32>>,
    /// Capacity taken from a part by vias at its ends.
    taken: HashMap<(u8, u32, u16), u32>,
    /// Net of each embedded wire (by commodity index).
    wire_net: HashMap<u32, u32>,
    /// Searches that hit their work bound / exhausted the gap graph.
    pub stats: [usize; 2],
    /// Room on each cut (layer, cut) the market plan holds for commodities
    /// not embedded yet: a wire may take a cut's last places only if they
    /// are not promised to someone else.
    pub reserved: HashMap<(u8, u32), u32>,
}

impl<'a, 'g> Embedding<'a, 'g> {
    /// An empty embedding over `g`.
    pub fn new(g: &'a FlowGraph<'g>) -> Self {
        Self {
            g,
            parts: HashMap::new(),
            chords: HashMap::new(),
            via_at: HashMap::new(),
            vsite: g
                .cg
                .layers
                .iter()
                .map(|lg| {
                    lg.steiner
                        .iter()
                        .enumerate()
                        .map(|(s, &v)| (v, s as u32))
                        .collect()
                })
                .collect(),
            taken: HashMap::new(),
            wire_net: HashMap::new(),
            stats: [0; 2],
            reserved: HashMap::new(),
        }
    }

    /// Wires through a part, in order from the part's start (toward cut.b).
    pub fn part_wires(&self, layer: u8, cut: u32, part: u16) -> &[u32] {
        self.parts
            .get(&(layer, cut, part))
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    fn part_cap(&self, layer: u8, cut: u32, part: u16) -> u32 {
        let lg = &self.g.cg.layers[layer as usize];
        let c = &lg.cuts[cut as usize];
        let q = c.parts[part as usize];
        let cap = self.g.cg.params.fit(q[1] - q[0]);
        cap.saturating_sub(self.taken.get(&(layer, cut, part)).copied().unwrap_or(0))
    }

    fn cut_point(&self, layer: u8, cut: u32, s: f64) -> [f64; 2] {
        let lg = &self.g.cg.layers[layer as usize];
        let c = &lg.cuts[cut as usize];
        let (a, b) = (lg.vpos(c.a), lg.vpos(c.b));
        let u = s / c.len.max(1e-12);
        [a[0] + u * (b[0] - a[0]), a[1] + u * (b[1] - a[1])]
    }

    /// A free part's extent (mm from cut.a) less the barrels of vias placed
    /// at its end vertices.
    pub fn bounds(&self, layer: u8, cut: u32, part: u16) -> [f64; 2] {
        let lg = &self.g.cg.layers[layer as usize];
        let c = &lg.cuts[cut as usize];
        let mut q = c.parts[part as usize];
        let r_at = |v: u32| {
            let s = *self.vsite[layer as usize].get(&v)?;
            let (_, sp) = self.via_at.get(&s)?;
            let span = &self.g.sites[s as usize].spans[*sp as usize];
            // only on the layers the barrel actually spans
            span.layers.contains(&layer).then_some(span.r_center)
        };
        if part == 0 {
            if let Some(r) = r_at(c.a) {
                q[0] = q[0].max(r);
            }
        }
        if part as usize == c.parts.len() - 1 {
            if let Some(r) = r_at(c.b) {
                q[1] = q[1].min(c.len - r);
            }
        }
        if q[1] < q[0] {
            let m = (q[0] + q[1]) / 2.0;
            q = [m, m];
        }
        q
    }

    /// Position along its cut (mm from cut.a) of a slot or gap.
    fn cut_s(&self, e: &End) -> Option<(u8, u32, f64)> {
        let (layer, cut, part, k, n) = match *e {
            End::Point(_) => return None,
            End::Slot {
                layer,
                cut,
                part,
                wire,
            } => {
                let ws = self.part_wires(layer, cut, part);
                let k = ws.iter().position(|&w| w == wire).unwrap_or(0) as f64;
                (layer, cut, part, k, ws.len())
            }
            End::Gap {
                layer,
                cut,
                part,
                gap,
            } => (
                layer,
                cut,
                part,
                gap as f64 - 0.5,
                self.part_wires(layer, cut, part).len(),
            ),
        };
        let q = self.bounds(layer, cut, part);
        Some((
            layer,
            cut,
            q[0] + (k + 1.0) / (n as f64 + 1.0) * (q[1] - q[0]),
        ))
    }

    fn end_point(&self, e: &End) -> [f64; 2] {
        match *e {
            End::Point(p) => p,
            _ => {
                let (layer, cut, s) = self.cut_s(e).unwrap();
                self.cut_point(layer, cut, s)
            }
        }
    }

    /// Cyclic boundary coordinate of `e` in face `face` (edge `k` from
    /// vertex `k` to `k+1` spans `[k, k+1)`), or `None` for an interior point.
    fn coord(&self, layer: u8, face: u32, e: &End) -> Option<f64> {
        let lg = &self.g.cg.layers[layer as usize];
        let t = lg.tris[face as usize];
        match *e {
            End::Point(p) => (0..3)
                .find(|&k| dist(p, lg.vpos(t[k])) < 1e-7)
                .map(|k| k as f64),
            _ => {
                let (_, cut, s) = self.cut_s(e)?;
                let k = (0..3).find(|&k| lg.tri_cut[face as usize][k] == cut)?;
                let c = &lg.cuts[cut as usize];
                let u = (s / c.len.max(1e-12)).clamp(0.0, 1.0);
                Some(k as f64 + if t[k] == c.a { u } else { 1.0 - u })
            }
        }
    }

    /// Whether the connection `p–q` in `face` crosses the chord `a–b`:
    /// interleaving on the face boundary when all four ends lie on it, a
    /// proper segment intersection otherwise.
    fn crosses(&self, layer: u8, face: u32, p: &End, q: &End, a: &End, b: &End) -> bool {
        let c = |e: &End| self.coord(layer, face, e);
        if let (Some(cp), Some(cq), Some(ca), Some(cb)) = (c(p), c(q), c(a), c(b)) {
            let eq = |x: f64, y: f64| (x - y).abs() < 1e-9;
            if eq(cp, ca) || eq(cp, cb) || eq(cq, ca) || eq(cq, cb) {
                return false;
            }
            let inside = |z: f64| {
                if ca < cb {
                    ca < z && z < cb
                } else {
                    z > ca || z < cb
                }
            };
            return inside(cp) != inside(cq);
        }
        proper_cross(
            self.end_point(p),
            self.end_point(q),
            self.end_point(a),
            self.end_point(b),
        )
    }

    /// Own-copper end of a cut (where a wire leaving its own pad sits).
    fn own_end(&self, layer: u8, cut: u32, net: u32) -> [f64; 2] {
        let lg = &self.g.cg.layers[layer as usize];
        let c = &lg.cuts[cut as usize];
        let own = |v: u32| {
            let o = lg.vobs[v as usize];
            o != NONE && self.g.net_id.get(&lg.obstacles[o as usize].net) == Some(&net)
        };
        if own(c.a) {
            lg.vpos(c.a)
        } else if own(c.b) {
            lg.vpos(c.b)
        } else {
            let (a, b) = (lg.vpos(c.a), lg.vpos(c.b));
            [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]
        }
    }

    fn blocked(&self, layer: u8, face: u32, p: &End, q: &End, net: u32) -> bool {
        let Some(cs) = self.chords.get(&(layer, face)) else {
            return false;
        };
        cs.iter()
            .any(|c| c.net != net && self.crosses(layer, face, p, q, &c.a, &c.b))
    }

    fn st_face(&self, s: &St) -> (u8, u32) {
        match *s {
            St::Start(n) => {
                let (l, f) = self.g.split(n);
                (l as u8, f)
            }
            St::Cut { layer, face, .. }
            | St::Free { layer, face, .. }
            | St::Vert { layer, face, .. } => (layer, face),
        }
    }

    fn st_end(&self, s: &St, net: u32) -> End {
        match *s {
            St::Start(n) => {
                let (l, f) = self.g.split(n);
                End::Point(self.g.cg.layers[l].tri_centroid(f))
            }
            St::Cut {
                layer,
                cut,
                part,
                gap,
                ..
            } => End::Gap {
                layer,
                cut,
                part,
                gap,
            },
            St::Free { layer, cut, .. } => End::Point(self.own_end(layer, cut, net)),
            St::Vert { site, .. } => End::Point(self.g.sites[site as usize].pos),
        }
    }

    /// Embed commodity `ki` at the current prices; `None` when no planar
    /// path exists through the remaining gaps.
    ///
    /// `corridor` (face nodes) restricts the search to the market plan's
    /// neighbourhood; `max_pops` bounds the work.
    pub fn route(
        &mut self,
        ki: u32,
        k: &Commodity,
        params: &FlowParams,
        corridor: Option<&std::collections::HashSet<u32>>,
        max_pops: usize,
    ) -> Option<EPath> {
        let inside = |l: usize, f: u32| corridor.is_none_or(|c| c.contains(&self.g.node(l, f)));
        let g = self.g;
        let nl = g.cg.layers.len();
        let net = k.net_id;
        let dst: std::collections::HashSet<u32> = k.dst.nodes.iter().copied().collect();
        let tgt = k.dst.point;
        let slack = k
            .dst
            .nodes
            .iter()
            .map(|&nd| {
                let (l, f) = g.split(nd);
                dist(g.cg.layers[l].tri_centroid(f), tgt)
            })
            .fold(0.0f64, f64::max);
        let mut ids: HashMap<St, u32> = HashMap::new();
        let mut states: Vec<St> = Vec::new();
        let mut gs: Vec<f64> = Vec::new();
        let mut pred: Vec<(u32, Option<EStep>)> = Vec::new();
        let mut heap = BinaryHeap::new();
        let intern = |s: St,
                      ids: &mut HashMap<St, u32>,
                      states: &mut Vec<St>,
                      gs: &mut Vec<f64>,
                      pred: &mut Vec<(u32, Option<EStep>)>|
         -> u32 {
            *ids.entry(s).or_insert_with(|| {
                states.push(s);
                gs.push(f64::INFINITY);
                pred.push((NONE, None));
                (states.len() - 1) as u32
            })
        };
        let hpt = |p: [f64; 2]| (dist(p, tgt) - slack).max(0.0);
        for &n in &k.src.nodes {
            let id = intern(St::Start(n), &mut ids, &mut states, &mut gs, &mut pred);
            gs[id as usize] = 0.0;
            let (l, f) = g.split(n);
            heap.push(Item {
                f: hpt(g.cg.layers[l].tri_centroid(f)),
                g: 0.0,
                id,
            });
        }
        let mut found = NONE;
        let mut pops = 0usize;
        while let Some(Item { g: gu, id, .. }) = heap.pop() {
            if gu > gs[id as usize] {
                continue;
            }
            pops += 1;
            if pops > max_pops {
                self.stats[0] += 1;
                return None;
            }
            let s = states[id as usize];
            let (layer, face) = self.st_face(&s);
            let l = layer as usize;
            if dst.contains(&g.node(l, face)) {
                found = id;
                break;
            }
            let p = self.st_end(&s, net);
            let entry_cut = match s {
                St::Cut { cut, .. } | St::Free { cut, .. } => cut,
                _ => NONE,
            };
            let lg = &g.cg.layers[l];
            let mut push = |ns: St, w: f64, st: EStep| {
                let nid = intern(ns, &mut ids, &mut states, &mut gs, &mut pred);
                let nv = gu + w;
                if nv < gs[nid as usize] {
                    gs[nid as usize] = nv;
                    pred[nid as usize] = (id, Some(st));
                    let q = match ns {
                        St::Vert { site, .. } => g.sites[site as usize].pos,
                        _ => {
                            let (nl_, nf) = match ns {
                                St::Cut { layer, face, .. }
                                | St::Free { layer, face, .. }
                                | St::Vert { layer, face, .. } => (layer as usize, face),
                                St::Start(n) => g.split(n),
                            };
                            g.cg.layers[nl_].tri_centroid(nf)
                        }
                    };
                    heap.push(Item {
                        f: nv + hpt(q),
                        g: nv,
                        id: nid,
                    });
                }
            };
            for kk in 0..3 {
                let c = lg.tri_cut[face as usize][kk];
                let nb = lg.tri_nbr[face as usize][kk];
                if c == entry_cut
                    || nb == NONE
                    || (!g.face_ok(l, nb, net) && !dst.contains(&g.node(l, nb)))
                    || !inside(l, nb)
                {
                    continue;
                }
                let len = g.arc_len[l][c as usize];
                if !len.is_finite() {
                    continue;
                }
                let st = |part: Option<u16>| EStep::Cross {
                    layer,
                    cut: c,
                    from: face,
                    part,
                };
                match g.cross(l, c, net) {
                    Cross::Blocked => {}
                    Cross::Free => {
                        let q = End::Point(self.own_end(layer, c, net));
                        if !self.blocked(layer, face, &p, &q, net) {
                            push(
                                St::Free {
                                    layer,
                                    cut: c,
                                    face: nb,
                                },
                                len,
                                st(None),
                            );
                        }
                    }
                    Cross::Consumes => {
                        let w = len + k.units_at(g.cut_mid(l, c)) * g.lambda[l][c as usize];
                        let np = lg.cuts[c as usize].parts.len();
                        let in_cut: u32 = (0..np as u16)
                            .map(|pt| self.part_wires(layer, c, pt).len() as u32)
                            .sum();
                        let promised = self.reserved.get(&(layer, c)).copied().unwrap_or(0);
                        if in_cut + promised >= lg.cuts[c as usize].cap {
                            continue;
                        }
                        for part in 0..np as u16 {
                            let n = self.part_wires(layer, c, part).len();
                            if n as u32 >= self.part_cap(layer, c, part) {
                                continue;
                            }
                            for gap in 0..=n as u16 {
                                let q = End::Gap {
                                    layer,
                                    cut: c,
                                    part,
                                    gap,
                                };
                                if self.blocked(layer, face, &p, &q, net) {
                                    continue;
                                }
                                push(
                                    St::Cut {
                                        layer,
                                        cut: c,
                                        part,
                                        gap,
                                        face: nb,
                                    },
                                    w,
                                    st(Some(part)),
                                );
                            }
                        }
                    }
                }
            }
            // Vias at this face's corners (not right after arriving by one).
            if !matches!(s, St::Vert { .. }) {
                let fnode = g.node(l, face) as usize;
                for &sl in &g.face_sites[fnode] {
                    let site = sl / nl as u32;
                    if self.via_at.get(&site).is_some_and(|&(o, _)| o != net) {
                        continue;
                    }
                    let q = g.sites[site as usize].pos;
                    if self.blocked(layer, face, &p, &End::Point(q), net) {
                        continue;
                    }
                    let to_vertex = dist(self.end_point(&p), q);
                    for (si, sp) in g.sites[site as usize].spans.iter().enumerate() {
                        if !sp.layers.contains(&layer) {
                            continue;
                        }
                        // The barrel must fit beside the wires already there.
                        let fits = self.via_at.contains_key(&site)
                            || sp.cuts.iter().all(|&(cl, cc, units)| {
                                let part = self.site_part(cl, cc, site);
                                part.is_none_or(|pt| {
                                    self.part_wires(cl, cc, pt).len() as u32 + units as u32
                                        <= self.part_cap(cl, cc, pt)
                                })
                            });
                        if !fits {
                            continue;
                        }
                        let mut w = to_vertex + sp.cost;
                        for &(cl, cc, units) in &sp.cuts {
                            w += units * g.lambda[cl as usize][cc as usize];
                        }
                        let _ = params;
                        for &to in &sp.layers {
                            if to == layer {
                                continue;
                            }
                            for &f2 in &g.site_faces[site as usize * nl + to as usize] {
                                if !g.face_ok(to as usize, f2, net) || !inside(to as usize, f2) {
                                    continue;
                                }
                                push(
                                    St::Vert {
                                        layer: to,
                                        site,
                                        face: f2,
                                    },
                                    w,
                                    EStep::Via {
                                        site,
                                        span: si as u16,
                                        from: layer,
                                        to,
                                        face: f2,
                                    },
                                );
                            }
                        }
                    }
                }
            }
        }
        if found == NONE {
            self.stats[1] += 1;
            return None;
        }
        // Reconstruct.
        let mut chain: Vec<u32> = vec![found];
        let mut steps = Vec::new();
        let mut cur = found;
        while let (p, Some(st)) = pred[cur as usize] {
            steps.push(st);
            chain.push(p);
            cur = p;
        }
        steps.reverse();
        chain.reverse();
        let start = match states[chain[0] as usize] {
            St::Start(n) => n,
            _ => unreachable!("chains start at a terminal"),
        };
        let (fl, ff) = self.st_face(&states[found as usize]);
        let end = g.node(fl as usize, ff);
        // Commit: insert slots, then chords (positions are recomputed from
        // the lists, so insert all slots first).
        let ends: Vec<End> = chain
            .iter()
            .map(|&id| match states[id as usize] {
                St::Cut {
                    layer, cut, part, ..
                } => End::Slot {
                    layer,
                    cut,
                    part,
                    wire: ki,
                },
                s => self.st_end(&s, net),
            })
            .collect();
        for &id in &chain {
            if let St::Cut {
                layer,
                cut,
                part,
                gap,
                ..
            } = states[id as usize]
            {
                let v = self.parts.entry((layer, cut, part)).or_default();
                let at = (gap as usize).min(v.len());
                v.insert(at, ki);
            }
        }
        self.wire_net.insert(ki, net);
        for (i, st) in steps.iter().enumerate() {
            let (l0, f0) = self.st_face(&states[chain[i] as usize]);
            let a = ends[i];
            let b = match *st {
                EStep::Via { site, .. } => End::Point(g.sites[site as usize].pos),
                _ => ends[i + 1],
            };
            self.chords
                .entry((l0, f0))
                .or_default()
                .push(Chord { a, b, net });
            if let EStep::Via { site, span, .. } = *st {
                if self.via_at.insert(site, (net, span)).is_none() {
                    let sp = &g.sites[site as usize].spans[span as usize];
                    for &(cl, cc, units) in &sp.cuts {
                        if let Some(pt) = self.site_part(cl, cc, site) {
                            *self.taken.entry((cl, cc, pt)).or_default() += units as u32;
                        }
                    }
                }
            }
        }
        // Chord from the last crossing to the terminal inside the end face.
        let (le, fe) = self.st_face(&states[found as usize]);
        self.chords.entry((le, fe)).or_default().push(Chord {
            a: *ends.last().unwrap(),
            b: End::Point(tgt),
            net,
        });
        Some(EPath { start, end, steps })
    }

    /// The free part of cut `c` touching site `site`'s vertex on layer `l`.
    fn site_part(&self, l: u8, c: u32, site: u32) -> Option<u16> {
        let lg = &self.g.cg.layers[l as usize];
        let cut = &lg.cuts[c as usize];
        let v = lg.steiner[site as usize];
        if cut.parts.is_empty() {
            return None;
        }
        if cut.a == v {
            Some(0)
        } else if cut.b == v {
            Some((cut.parts.len() - 1) as u16)
        } else {
            None
        }
    }
}
