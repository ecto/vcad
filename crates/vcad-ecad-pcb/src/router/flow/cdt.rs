//! Per-layer cut graph: a Delaunay triangulation of clearance-inflated
//! obstacles whose edges are the *cuts* of the one-market router.
//!
//! Every obstacle on a signal layer — pad, existing trace, via, keepout edge,
//! board edge — is inflated by the clearance it demands and sampled into
//! boundary vertices. Those vertices are triangulated (Delaunay, via
//! `spade`, already a workspace dependency through `geo`). A triangle edge
//! `pq` is a cut: any wire that passes between the features at `p` and `q`
//! crosses it. Its capacity is computed *exactly against the obstacle
//! geometry*, not from the vertex positions: the segment is clipped against
//! every inflated obstacle it touches, leaving free sub-intervals ("parts"),
//! and each part of length `ℓ` holds `floor((ℓ + s) / (w + s))` wires of width
//! `w` and spacing `s` (k wires need `k·w + (k−1)·s ≤ ℓ` once the obstacles
//! are grown by their clearance). Edges inside one obstacle come out with no
//! parts and capacity 0, so the graph needs no constrained edges and is robust
//! to overlapping pads.
//!
//! This is the Leiserson–Maley / Schrijver cut condition made concrete: a
//! single-layer homotopic routing is feasible iff every cut carries at most its
//! capacity (see `docs/router-one-market-math.md` §3). Plane-role layers get
//! no graph at all.

use rstar::primitives::{GeomWithData, Rectangle};
use rstar::{RTree, AABB};
use spade::{DelaunayTriangulation, Point2, Triangulation};

use vcad_ir::ecad::{Pcb, PcbLayer};
use vcad_ir::Vec2;

use crate::spatial::{copper_elements, point_in_polygon, CopperGeom};

/// No obstacle / no face sentinel.
pub const NONE: u32 = u32::MAX;

/// What an obstacle is (for reporting and terminal attachment).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObsKind {
    /// Footprint pad.
    Pad,
    /// Existing via.
    Via,
    /// Existing trace.
    Trace,
    /// Board outline / cutout edge.
    Edge,
    /// No-tracks keepout boundary.
    Keepout,
}

/// One obstacle on one layer, inflated by `grow` (its clearance).
#[derive(Debug, Clone)]
pub struct Obstacle {
    /// Net of the copper ("" for edges / keepouts / unassigned).
    pub net: String,
    /// Raw copper geometry.
    pub geom: CopperGeom,
    /// Inflation (mm): the clearance this obstacle demands.
    pub grow: f64,
    /// Obstacle kind.
    pub kind: ObsKind,
}

/// Width/spacing model the capacities are computed for.
#[derive(Debug, Clone, Copy)]
pub struct CutParams {
    /// Track width (mm).
    pub width: f64,
    /// Track-to-track / track-to-copper spacing (mm).
    pub clearance: f64,
    /// Copper-to-board-edge clearance (mm).
    pub edge_clearance: f64,
    /// Max spacing of boundary samples along long obstacle sides (mm).
    pub sample: f64,
}

impl CutParams {
    /// The board's default rules.
    pub fn from_pcb(pcb: &Pcb) -> Self {
        Self {
            width: pcb.rules.default_rules.trace_width,
            clearance: pcb.rules.default_rules.clearance,
            edge_clearance: pcb.rules.edge_clearance,
            sample: 1.0,
        }
    }

    /// Wires of this width/spacing that fit in a free part of length `l`.
    /// (`l` is a centerline-space length: obstacles are grown by clearance
    /// plus half the default width, so `l + width` is the copper room.)
    pub fn fit(&self, l: f64) -> u32 {
        fit(l + self.width, self.width, self.clearance)
    }
}

/// Wires of width `w`, spacing `s` that fit in a free interval of length `l`.
pub fn fit(l: f64, w: f64, s: f64) -> u32 {
    if l < w - 1e-9 {
        return 0;
    }
    (((l + s) / (w + s)) + 1e-9).floor().max(0.0) as u32
}

/// A triangulation edge = a cut.
#[derive(Debug, Clone)]
pub struct Cut {
    /// Endpoint vertex ids (`a < b`).
    pub a: u32,
    /// Endpoint vertex id.
    pub b: u32,
    /// The (up to two) live faces on either side; [`NONE`] on the hull or
    /// when the face is off-board.
    pub faces: [u32; 2],
    /// Euclidean length (mm).
    pub len: f64,
    /// Free sub-intervals `[s0, s1]` in mm from `a`, after clipping against
    /// every inflated obstacle.
    pub parts: Vec<[f64; 2]>,
    /// Covered intervals `[s0, s1]` (mm from `a`) between/around the parts,
    /// each with the obstacle that covers it when every obstacle covering it
    /// is copper of one net (else [`NONE`]): a wire of that net may use it.
    pub blocks: Vec<([f64; 2], u32)>,
    /// Wire capacity at the default [`CutParams`].
    pub cap: u32,
}

impl Cut {
    /// Capacity for a width/spacing other than the default.
    /// `w0` is the width the graph was inflated for.
    pub fn cap_for(&self, w0: f64, w: f64, s: f64) -> u32 {
        self.parts.iter().map(|p| fit(p[1] - p[0] + w0, w, s)).sum()
    }

    /// Total free length (mm).
    pub fn free(&self) -> f64 {
        self.parts.iter().map(|p| p[1] - p[0]).sum()
    }

    /// Index of the free part containing position `s` (mm from `a`).
    pub fn part_at(&self, s: f64) -> Option<usize> {
        self.parts
            .iter()
            .position(|p| s >= p[0] - 1e-6 && s <= p[1] + 1e-6)
    }
}

/// The cut graph of one signal layer.
pub struct LayerCdt {
    /// Copper layer.
    pub layer: PcbLayer,
    /// Vertex positions.
    pub verts: Vec<[f64; 2]>,
    /// Owning obstacle of each vertex (index into [`LayerCdt::obstacles`]).
    pub vobs: Vec<u32>,
    /// Triangles, counter-clockwise vertex ids.
    pub tris: Vec<[u32; 3]>,
    /// Cut id of triangle edge `k` = `(v[k], v[(k+1)%3])`.
    pub tri_cut: Vec<[u32; 3]>,
    /// Neighbor face across edge `k` ([`NONE`] at the hull).
    pub tri_nbr: Vec<[u32; 3]>,
    /// Face is inside the board outline (outside cutouts).
    pub tri_live: Vec<bool>,
    /// Obstacle whose inflated shape contains the face centroid ([`NONE`] in
    /// free space).
    pub tri_obs: Vec<u32>,
    /// Face is unusable by every net: its centroid is inside the inflation of
    /// the board edge / a keepout, or of copper of two different nets.
    pub tri_block: Vec<bool>,
    /// Cuts.
    pub cuts: Vec<Cut>,
    /// Obstacles on this layer.
    pub obstacles: Vec<Obstacle>,
    /// Vertex id of each Steiner point passed to [`LayerCdt::build`].
    pub steiner: Vec<u32>,
    tri_index: RTree<GeomWithData<Rectangle<[f64; 2]>, u32>>,
    obs_index: RTree<GeomWithData<Rectangle<[f64; 2]>, u32>>,
}

/// One crossing of a segment walk.
#[derive(Debug, Clone, Copy)]
pub struct Crossing {
    /// Cut crossed.
    pub cut: u32,
    /// Position along the cut, mm from its `a` vertex.
    pub s: f64,
    /// Face entered.
    pub into: u32,
}

fn sub(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}
fn cross(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}
/// Twice the signed area of `abc` (> 0: counter-clockwise).
pub fn orient(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    cross(sub(b, a), sub(c, a))
}
fn v2(p: Vec2) -> [f64; 2] {
    [p.x, p.y]
}

/// Bounding box of an obstacle's inflated shape.
fn obs_aabb(o: &Obstacle) -> ([f64; 2], [f64; 2]) {
    let g = o.grow;
    match o.geom {
        CopperGeom::Disc { center, r } => (
            [center.x - r - g, center.y - r - g],
            [center.x + r + g, center.y + r + g],
        ),
        CopperGeom::Segment { a, b, half_w } => {
            let r = half_w + g;
            (
                [a.x.min(b.x) - r, a.y.min(b.y) - r],
                [a.x.max(b.x) + r, a.y.max(b.y) + r],
            )
        }
        CopperGeom::Rect {
            center,
            half_w,
            half_h,
            rot,
        } => {
            let (s, c) = rot.sin_cos();
            let hw = half_w + g;
            let hh = half_h + g;
            let ex = (hw * c).abs() + (hh * s).abs();
            let ey = (hw * s).abs() + (hh * c).abs();
            (
                [center.x - ex, center.y - ey],
                [center.x + ex, center.y + ey],
            )
        }
    }
}

/// Parameter interval `[t0, t1] ⊂ [0, 1]` of segment `p→q` inside a disc.
fn clip_disc(p: [f64; 2], q: [f64; 2], c: [f64; 2], r: f64, out: &mut Vec<[f64; 2]>) {
    let d = sub(q, p);
    let f = sub(p, c);
    let a = d[0] * d[0] + d[1] * d[1];
    if a < 1e-18 {
        if f[0] * f[0] + f[1] * f[1] <= r * r {
            out.push([0.0, 1.0]);
        }
        return;
    }
    let b = 2.0 * (f[0] * d[0] + f[1] * d[1]);
    let cc = f[0] * f[0] + f[1] * f[1] - r * r;
    let disc = b * b - 4.0 * a * cc;
    if disc <= 0.0 {
        return;
    }
    let sq = disc.sqrt();
    let t0 = ((-b - sq) / (2.0 * a)).max(0.0);
    let t1 = ((-b + sq) / (2.0 * a)).min(1.0);
    if t1 > t0 {
        out.push([t0, t1]);
    }
}

/// Liang–Barsky clip of `p→q` against an oriented box.
fn clip_box(
    p: [f64; 2],
    q: [f64; 2],
    c: [f64; 2],
    hw: f64,
    hh: f64,
    rot: f64,
    out: &mut Vec<[f64; 2]>,
) {
    let (s, co) = rot.sin_cos();
    let loc = |x: [f64; 2]| {
        let d = sub(x, c);
        [d[0] * co + d[1] * s, -d[0] * s + d[1] * co]
    };
    let lp = loc(p);
    let lq = loc(q);
    let d = sub(lq, lp);
    let mut t0: f64 = 0.0;
    let mut t1: f64 = 1.0;
    for (pp, qq) in [
        (-d[0], lp[0] + hw),
        (d[0], hw - lp[0]),
        (-d[1], lp[1] + hh),
        (d[1], hh - lp[1]),
    ] {
        if pp.abs() < 1e-15 {
            if qq < 0.0 {
                return;
            }
        } else {
            let r = qq / pp;
            if pp < 0.0 {
                t0 = t0.max(r);
            } else {
                t1 = t1.min(r);
            }
        }
    }
    if t1 > t0 {
        out.push([t0, t1]);
    }
}

/// Covered parameter intervals of `p→q` inside obstacle `o` (inflated).
pub fn clip_obstacle(p: [f64; 2], q: [f64; 2], o: &Obstacle, out: &mut Vec<[f64; 2]>) {
    let g = o.grow;
    match o.geom {
        CopperGeom::Disc { center, r } => clip_disc(p, q, v2(center), r + g, out),
        CopperGeom::Rect {
            center,
            half_w,
            half_h,
            rot,
        } => clip_box(p, q, v2(center), half_w + g, half_h + g, rot, out),
        CopperGeom::Segment { a, b, half_w } => {
            let r = half_w + g;
            clip_disc(p, q, v2(a), r, out);
            clip_disc(p, q, v2(b), r, out);
            let len = ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt();
            if len > 1e-9 {
                let mid = [(a.x + b.x) / 2.0, (a.y + b.y) / 2.0];
                let rot = (b.y - a.y).atan2(b.x - a.x);
                clip_box(p, q, mid, len / 2.0, r, rot, out);
            }
        }
    }
}

/// Whether point `x` lies inside obstacle `o` grown by `extra` beyond its own
/// inflation (`extra = -grow` tests the raw copper).
pub fn obstacle_contains(o: &Obstacle, x: [f64; 2], extra: f64) -> bool {
    let g = o.grow + extra;
    match o.geom {
        CopperGeom::Disc { center, r } => {
            let d = sub(x, v2(center));
            d[0] * d[0] + d[1] * d[1] <= (r + g) * (r + g)
        }
        CopperGeom::Rect {
            center,
            half_w,
            half_h,
            rot,
        } => {
            let (s, c) = rot.sin_cos();
            let d = sub(x, v2(center));
            let lx = d[0] * c + d[1] * s;
            let ly = -d[0] * s + d[1] * c;
            lx.abs() <= half_w + g && ly.abs() <= half_h + g
        }
        CopperGeom::Segment { a, b, half_w } => {
            let ab = sub(v2(b), v2(a));
            let ax = sub(x, v2(a));
            let l2 = ab[0] * ab[0] + ab[1] * ab[1];
            let t = if l2 < 1e-18 {
                0.0
            } else {
                ((ax[0] * ab[0] + ax[1] * ab[1]) / l2).clamp(0.0, 1.0)
            };
            let cpt = [a.x + t * ab[0], a.y + t * ab[1]];
            let d = sub(x, cpt);
            d[0] * d[0] + d[1] * d[1] <= (half_w + g) * (half_w + g)
        }
    }
}

/// Boundary samples of an inflated obstacle (slightly outside it, so a
/// sample never sits strictly inside its own obstacle's clip region).
fn obstacle_samples(o: &Obstacle, sample: f64, out: &mut Vec<[f64; 2]>) {
    let g = o.grow;
    let ring = |c: [f64; 2], r: f64, out: &mut Vec<[f64; 2]>| {
        let n = 8usize;
        let rr = r / (std::f64::consts::PI / n as f64).cos() + 1e-6;
        for i in 0..n {
            let a = std::f64::consts::TAU * (i as f64 + 0.5) / n as f64;
            out.push([c[0] + rr * a.cos(), c[1] + rr * a.sin()]);
        }
    };
    let side = |p: [f64; 2], q: [f64; 2], out: &mut Vec<[f64; 2]>| {
        let l = ((q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2)).sqrt();
        let n = (l / sample).ceil().max(1.0) as usize;
        for i in 0..n {
            let t = i as f64 / n as f64;
            out.push([p[0] + t * (q[0] - p[0]), p[1] + t * (q[1] - p[1])]);
        }
    };
    match o.geom {
        CopperGeom::Disc { center, r } => ring(v2(center), r + g, out),
        CopperGeom::Rect {
            center,
            half_w,
            half_h,
            rot,
        } => {
            let (s, c) = rot.sin_cos();
            let hw = half_w + g + 1e-6;
            let hh = half_h + g + 1e-6;
            let w = |lx: f64, ly: f64| [center.x + lx * c - ly * s, center.y + lx * s + ly * c];
            let corners = [w(-hw, -hh), w(hw, -hh), w(hw, hh), w(-hw, hh)];
            for k in 0..4 {
                side(corners[k], corners[(k + 1) % 4], out);
            }
        }
        CopperGeom::Segment { a, b, half_w } => {
            let r = half_w + g;
            ring(v2(a), r, out);
            ring(v2(b), r, out);
            let ab = sub(v2(b), v2(a));
            let l = (ab[0] * ab[0] + ab[1] * ab[1]).sqrt();
            if l > sample {
                let n = [-ab[1] / l * (r + 1e-6), ab[0] / l * (r + 1e-6)];
                side([a.x + n[0], a.y + n[1]], [b.x + n[0], b.y + n[1]], out);
                side([a.x - n[0], a.y - n[1]], [b.x - n[0], b.y - n[1]], out);
            }
        }
    }
}

/// Which copper to keep as obstacles when building a layer graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopperFilter {
    /// Everything on the board (pads, vias, traces).
    All,
    /// Pads only: the board as the router sees it before routing.
    PadsOnly,
}

/// Collect the obstacles of `layer`.
pub fn layer_obstacles(
    pcb: &Pcb,
    layer: PcbLayer,
    p: &CutParams,
    filter: CopperFilter,
) -> Vec<Obstacle> {
    let cl = crate::drc::build_net_clearance_map(pcb);
    let c_of = |net: &str| cl.get(net).copied().unwrap_or(p.clearance).max(p.clearance);
    let mut out = Vec::new();
    let mut probe = pcb.clone();
    if filter == CopperFilter::PadsOnly {
        probe.traces.clear();
        probe.trace_arcs.clear();
        probe.vias.clear();
    }
    for e in copper_elements(&probe) {
        if e.layer != layer {
            continue;
        }
        let kind = match e.geom {
            CopperGeom::Segment { .. } => ObsKind::Trace,
            _ => ObsKind::Pad,
        };
        out.push(Obstacle {
            grow: c_of(&e.net) + p.width / 2.0,
            net: e.net,
            geom: e.geom,
            kind,
        });
    }
    // Plated through-holes occupy every copper layer, whatever layers the
    // importer listed (KiCad's `*.Cu` arrives as F/B only): keep their pad
    // shape as an obstacle on the inner layers too.
    for fp in &pcb.footprints {
        for pad in &fp.pads {
            if pad.drill.is_none() || pad.layers.contains(&layer) {
                continue;
            }
            let center = crate::geometry::pad_world_position(fp, pad);
            let rot = (fp.rotation + pad.rotation).to_radians();
            let net = pad.net.clone().unwrap_or_default();
            out.push(Obstacle {
                grow: c_of(&net) + p.width / 2.0,
                net,
                geom: crate::spatial::pad_geom(pad, center, rot),
                kind: ObsKind::Pad,
            });
        }
    }
    // Vias are discs too; re-tag them so reports can tell them apart.
    if filter == CopperFilter::All {
        for o in out.iter_mut() {
            if let CopperGeom::Disc { center, .. } = o.geom {
                if pcb.vias.iter().any(|v| {
                    (v.position.x - center.x).abs() < 1e-9 && (v.position.y - center.y).abs() < 1e-9
                }) {
                    o.kind = ObsKind::Via;
                }
            }
        }
    }
    // Board outline and cutouts: zero-width capsules grown by the edge rule.
    let mut ring = |poly: &[Vec2], kind: ObsKind, grow: f64| {
        for i in 0..poly.len() {
            let a = poly[i];
            let b = poly[(i + 1) % poly.len()];
            out.push(Obstacle {
                net: String::new(),
                geom: CopperGeom::Segment { a, b, half_w: 0.0 },
                grow,
                kind,
            });
        }
    };
    let half = p.width / 2.0;
    ring(
        &pcb.outline.vertices,
        ObsKind::Edge,
        p.edge_clearance + half,
    );
    for c in &pcb.outline.cutouts {
        ring(c, ObsKind::Edge, p.edge_clearance + half);
    }
    for k in &pcb.keepouts {
        if k.no_tracks && k.layers.contains(&layer) {
            ring(&k.outline, ObsKind::Keepout, p.clearance + half);
        }
    }
    out
}

impl LayerCdt {
    /// Build the cut graph of `layer` from explicit obstacles, plus Steiner
    /// vertices `steiner` (via sites: owned by no obstacle, `vobs` = [`NONE`]).
    /// A Steiner vertex is a point obstacle of centerline radius
    /// `(w0 + s) / 2`, so the two cuts it splits a channel into never hold more
    /// than the channel did.
    pub fn build(
        pcb: &Pcb,
        layer: PcbLayer,
        obstacles: Vec<Obstacle>,
        steiner: &[[f64; 2]],
        p: &CutParams,
    ) -> Self {
        // --- vertices: boundary samples of every inflated obstacle, deduped.
        let mut verts: Vec<[f64; 2]> = Vec::new();
        let mut vobs: Vec<u32> = Vec::new();
        let mut seen = std::collections::HashMap::new();
        let mut buf = Vec::new();
        for (i, o) in obstacles.iter().enumerate() {
            buf.clear();
            obstacle_samples(o, p.sample, &mut buf);
            for &v in &buf {
                let key = ((v[0] * 1e5).round() as i64, (v[1] * 1e5).round() as i64);
                if let std::collections::hash_map::Entry::Vacant(e) = seen.entry(key) {
                    e.insert(verts.len());
                    verts.push(v);
                    vobs.push(i as u32);
                }
            }
        }
        let mut steiner_vid = Vec::with_capacity(steiner.len());
        for &v in steiner {
            let key = ((v[0] * 1e5).round() as i64, (v[1] * 1e5).round() as i64);
            match seen.entry(key) {
                std::collections::hash_map::Entry::Vacant(e) => {
                    e.insert(verts.len());
                    steiner_vid.push(verts.len() as u32);
                    verts.push(v);
                    vobs.push(NONE);
                }
                std::collections::hash_map::Entry::Occupied(e) => {
                    steiner_vid.push(*e.get() as u32);
                }
            }
        }
        // --- Delaunay triangulation (spade, bulk load = deterministic).
        let pts: Vec<Point2<f64>> = verts.iter().map(|v| Point2::new(v[0], v[1])).collect();
        let dt: DelaunayTriangulation<Point2<f64>> =
            DelaunayTriangulation::bulk_load_stable(pts).expect("cdt: finite vertices");
        // Vertex handles -> our ids (bulk_load_stable keeps insertion order,
        // but map by position to be safe against dedup inside spade).
        let mut vid = vec![NONE; dt.num_vertices()];
        for vh in dt.vertices() {
            let pos = vh.position();
            let key = ((pos.x * 1e5).round() as i64, (pos.y * 1e5).round() as i64);
            vid[vh.fix().index()] = seen.get(&key).map(|&i| i as u32).unwrap_or(NONE);
        }
        let nf = dt.num_inner_faces();
        let mut face_id = vec![NONE; dt.num_all_faces()];
        let mut tris = Vec::with_capacity(nf);
        for (i, f) in dt.inner_faces().enumerate() {
            face_id[f.fix().index()] = i as u32;
            // Vertex k = origin of adjacent edge k, so edge k runs v[k]→v[k+1].
            let es = f.adjacent_edges();
            tris.push([
                vid[es[0].from().fix().index()],
                vid[es[1].from().fix().index()],
                vid[es[2].from().fix().index()],
            ]);
        }
        let mut edge_id = vec![NONE; dt.num_undirected_edges()];
        let mut cuts: Vec<Cut> = Vec::new();
        let mut tri_cut = vec![[NONE; 3]; nf];
        let mut tri_nbr = vec![[NONE; 3]; nf];
        for (i, f) in dt.inner_faces().enumerate() {
            let es = f.adjacent_edges();
            for k in 0..3 {
                let e = es[k];
                let ue = e.as_undirected().fix().index();
                if edge_id[ue] == NONE {
                    let a = vid[e.from().fix().index()];
                    let b = vid[e.to().fix().index()];
                    edge_id[ue] = cuts.len() as u32;
                    cuts.push(Cut {
                        a: a.min(b),
                        b: a.max(b),
                        faces: [NONE, NONE],
                        len: 0.0,
                        parts: Vec::new(),
                        blocks: Vec::new(),
                        cap: 0,
                    });
                }
                tri_cut[i][k] = edge_id[ue];
                let other = e.rev().face();
                tri_nbr[i][k] = other
                    .as_inner()
                    .map(|o| face_id[o.fix().index()])
                    .unwrap_or(NONE);
            }
        }
        drop(dt);

        // --- liveness and containing obstacle per face.
        let outline: Vec<Vec2> = pcb.outline.vertices.clone();
        let obs_index = RTree::bulk_load(
            obstacles
                .iter()
                .enumerate()
                .map(|(i, o)| {
                    let (lo, hi) = obs_aabb(o);
                    GeomWithData::new(Rectangle::from_corners(lo, hi), i as u32)
                })
                .collect(),
        );
        let mut tri_live = Vec::with_capacity(nf);
        let mut tri_obs = Vec::with_capacity(nf);
        let mut tri_block = Vec::with_capacity(nf);
        for t in &tris {
            let c = centroid(&verts, t);
            let cp = Vec2::new(c[0], c[1]);
            let live = (outline.len() < 3 || point_in_polygon(cp, &outline))
                && !pcb
                    .outline
                    .cutouts
                    .iter()
                    .any(|h| h.len() >= 3 && point_in_polygon(cp, h));
            tri_live.push(live);
            let mut best = NONE;
            let mut block = false;
            for g in obs_index.locate_all_at_point(&c) {
                let o = &obstacles[g.data as usize];
                if !obstacle_contains(o, c, 0.0) {
                    continue;
                }
                if matches!(o.kind, ObsKind::Edge | ObsKind::Keepout) {
                    block = true;
                    continue;
                }
                if best == NONE || g.data < best {
                    best = g.data;
                }
            }
            if best != NONE && !block {
                let net = &obstacles[best as usize].net;
                block = obs_index.locate_all_at_point(&c).any(|g| {
                    let o = &obstacles[g.data as usize];
                    !matches!(o.kind, ObsKind::Edge | ObsKind::Keepout)
                        && o.net != *net
                        && obstacle_contains(o, c, 0.0)
                });
            }
            tri_block.push(block);
            tri_obs.push(best);
        }

        // --- cut faces, lengths, free parts, capacities.
        for (i, tc) in tri_cut.iter().enumerate() {
            if !tri_live[i] {
                continue;
            }
            for &c in tc {
                let cut = &mut cuts[c as usize];
                if cut.faces[0] == NONE {
                    cut.faces[0] = i as u32;
                } else {
                    cut.faces[1] = i as u32;
                }
            }
        }
        let mut iv: Vec<[f64; 2]> = Vec::new();
        let mut ivo: Vec<u32> = Vec::new();
        for cut in cuts.iter_mut() {
            let pa = verts[cut.a as usize];
            let pb = verts[cut.b as usize];
            let len = ((pb[0] - pa[0]).powi(2) + (pb[1] - pa[1]).powi(2)).sqrt();
            cut.len = len;
            iv.clear();
            ivo.clear();
            let lo = [pa[0].min(pb[0]), pa[1].min(pb[1])];
            let hi = [pa[0].max(pb[0]), pa[1].max(pb[1])];
            for g in obs_index.locate_in_envelope_intersecting(&AABB::from_corners(lo, hi)) {
                let n0 = iv.len();
                clip_obstacle(pa, pb, &obstacles[g.data as usize], &mut iv);
                ivo.extend(std::iter::repeat_n(g.data, iv.len() - n0));
            }
            let rs = (p.width + p.clearance) / 2.0;
            for (end, pt) in [(cut.a, pa), (cut.b, pb)] {
                if vobs[end as usize] == NONE {
                    let n0 = iv.len();
                    clip_disc(pa, pb, pt, rs, &mut iv);
                    ivo.extend(std::iter::repeat_n(NONE, iv.len() - n0));
                }
            }
            let (parts, blocks) = free_parts(&iv, &ivo, len, &obstacles);
            cut.parts = parts;
            cut.blocks = blocks;
            cut.cap = cut.parts.iter().map(|q| p.fit(q[1] - q[0])).sum();
        }

        let tri_index = RTree::bulk_load(
            tris.iter()
                .enumerate()
                .map(|(i, t)| {
                    let a = verts[t[0] as usize];
                    let b = verts[t[1] as usize];
                    let c = verts[t[2] as usize];
                    let lo = [a[0].min(b[0]).min(c[0]), a[1].min(b[1]).min(c[1])];
                    let hi = [a[0].max(b[0]).max(c[0]), a[1].max(b[1]).max(c[1])];
                    GeomWithData::new(Rectangle::from_corners(lo, hi), i as u32)
                })
                .collect(),
        );
        Self {
            layer,
            verts,
            vobs,
            tris,
            tri_cut,
            tri_nbr,
            tri_live,
            tri_obs,
            tri_block,
            cuts,
            obstacles,
            steiner: steiner_vid,
            tri_index,
            obs_index,
        }
    }

    /// Capacity (default width) of cut `c` if a disc of centerline radius
    /// `r` stood at `center` (a via placed at a site).
    pub fn cap_with_disc(&self, c: u32, center: [f64; 2], r: f64, p: &CutParams) -> u32 {
        let cut = &self.cuts[c as usize];
        let (pa, pb) = (self.vpos(cut.a), self.vpos(cut.b));
        let mut iv = Vec::new();
        clip_disc(pa, pb, center, r, &mut iv);
        let Some(d) = iv.first().map(|x| [x[0] * cut.len, x[1] * cut.len]) else {
            return cut.cap;
        };
        let mut cap = 0;
        for q in &cut.parts {
            for piece in [[q[0], q[1].min(d[0])], [q[0].max(d[1]), q[1]]] {
                if piece[1] - piece[0] > 1e-3 {
                    cap += p.fit(piece[1] - piece[0]);
                }
            }
        }
        cap
    }

    /// Extra room (mm) a wire of `net` gets in free part `k` of cut `c` by
    /// overlapping its own net's copper (a wire leaving or entering its own
    /// pad): the length of own-net blocks adjacent to the part, and — when the
    /// part ends at a cut vertex sampled from an own-net obstacle — that
    /// obstacle's inflation zone, i.e. unlimited (`f64::INFINITY`).
    pub fn own_credit(&self, c: u32, k: usize, net: &str) -> f64 {
        if net.is_empty() {
            return 0.0;
        }
        let cut = &self.cuts[c as usize];
        let p = cut.parts[k];
        let own = |o: u32| o != NONE && self.obstacles[o as usize].net == net;
        if (p[0] < 1e-6 && own(self.vobs[cut.a as usize]))
            || (p[1] > cut.len - 1e-6 && own(self.vobs[cut.b as usize]))
        {
            return f64::INFINITY;
        }
        cut.blocks
            .iter()
            .filter(|(iv, o)| {
                own(*o) && ((iv[1] - p[0]).abs() < 1e-6 || (iv[0] - p[1]).abs() < 1e-6)
            })
            .map(|(iv, _)| iv[1] - iv[0])
            .sum()
    }

    /// Vertex position.
    pub fn vpos(&self, v: u32) -> [f64; 2] {
        self.verts[v as usize]
    }

    /// Centroid of face `t`.
    pub fn tri_centroid(&self, t: u32) -> [f64; 2] {
        centroid(&self.verts, &self.tris[t as usize])
    }

    /// The face containing `x` (live or not), if any.
    pub fn locate(&self, x: [f64; 2]) -> Option<u32> {
        let mut best: Option<u32> = None;
        for g in self.tri_index.locate_all_at_point(&x) {
            let t = &self.tris[g.data as usize];
            let (a, b, c) = (self.vpos(t[0]), self.vpos(t[1]), self.vpos(t[2]));
            let e = -1e-12;
            if orient(a, b, x) >= e && orient(b, c, x) >= e && orient(c, a, x) >= e {
                // deterministic: lowest face id among touching faces
                if best.is_none_or(|b| g.data < b) {
                    best = Some(g.data);
                }
            }
        }
        best
    }

    /// Obstacles whose inflated shape (grown further by `extra`) contains `x`.
    pub fn obstacles_at(&self, x: [f64; 2], extra: f64) -> Vec<u32> {
        let mut v: Vec<u32> = self
            .obs_index
            .locate_in_envelope_intersecting(&AABB::from_corners(
                [x[0] - extra.max(0.0), x[1] - extra.max(0.0)],
                [x[0] + extra.max(0.0), x[1] + extra.max(0.0)],
            ))
            .map(|g| g.data)
            .filter(|&i| obstacle_contains(&self.obstacles[i as usize], x, extra))
            .collect();
        v.sort_unstable();
        v
    }

    /// Walk the segment `p→q` through the triangulation, returning every cut
    /// crossed in order (the segment's homotopy word on this layer). Starts in
    /// `start` when given (a face known to contain `p`), else locates `p`.
    /// `None` when `p` is off the triangulation or the walk does not converge.
    pub fn walk(
        &self,
        p: [f64; 2],
        q: [f64; 2],
        start: Option<u32>,
    ) -> Option<(Vec<Crossing>, u32)> {
        let mut f = match start {
            Some(f) => f,
            None => self.locate(p)?,
        };
        let mut out = Vec::new();
        let mut came: u32 = NONE;
        for _ in 0..(self.tris.len() + 8) {
            let t = self.tris[f as usize];
            let pos = [self.vpos(t[0]), self.vpos(t[1]), self.vpos(t[2])];
            if orient(pos[0], pos[1], q) >= 0.0
                && orient(pos[1], pos[2], q) >= 0.0
                && orient(pos[2], pos[0], q) >= 0.0
            {
                return Some((out, f));
            }
            // Exit edge: q strictly right of it and the segment straddles it.
            let mut exit = None;
            let mut best_score = f64::NEG_INFINITY;
            for k in 0..3 {
                let c = self.tri_cut[f as usize][k];
                if c == came {
                    continue;
                }
                let a = pos[k];
                let b = pos[(k + 1) % 3];
                if orient(a, b, q) >= 0.0 {
                    continue;
                }
                let oa = orient(p, q, a);
                let ob = orient(p, q, b);
                // Leaving a ccw triangle through a→b, `a` is on the right
                // of p→q and `b` on the left.
                let score = if oa <= 0.0 && ob >= 0.0 {
                    1.0
                } else {
                    -(oa.abs().min(ob.abs()))
                };
                if score > best_score {
                    best_score = score;
                    exit = Some(k);
                }
            }
            let k = exit?;
            let c = self.tri_cut[f as usize][k];
            let nb = self.tri_nbr[f as usize][k];
            if nb == NONE {
                return None;
            }
            let a = pos[k];
            let b = pos[(k + 1) % 3];
            // intersection parameter along a->b
            let d1 = sub(b, a);
            let d2 = sub(q, p);
            let den = cross(d1, d2);
            let u = if den.abs() < 1e-18 {
                0.5
            } else {
                (cross(sub(p, a), d2) / den).clamp(0.0, 1.0)
            };
            let cut = &self.cuts[c as usize];
            let len = cut.len;
            let s = if t[k] == cut.a {
                u * len
            } else {
                (1.0 - u) * len
            };
            out.push(Crossing {
                cut: c,
                s,
                into: nb,
            });
            came = c;
            f = nb;
        }
        None
    }
}

fn centroid(verts: &[[f64; 2]], t: &[u32; 3]) -> [f64; 2] {
    let a = verts[t[0] as usize];
    let b = verts[t[1] as usize];
    let c = verts[t[2] as usize];
    [(a[0] + b[0] + c[0]) / 3.0, (a[1] + b[1] + c[1]) / 3.0]
}

/// Free parts (mm) = complement of the union of the covered parameter
/// intervals `iv` (owner obstacles `ivo`), plus the merged covered blocks
/// tagged with their single owning net's obstacle (or [`NONE`]). Parts under
/// 1 µm are covered.
#[allow(clippy::type_complexity)]
fn free_parts(
    iv: &[[f64; 2]],
    ivo: &[u32],
    len: f64,
    obstacles: &[Obstacle],
) -> (Vec<[f64; 2]>, Vec<([f64; 2], u32)>) {
    let mut ord: Vec<usize> = (0..iv.len()).collect();
    ord.sort_by(|&a, &b| iv[a][0].total_cmp(&iv[b][0]).then(ivo[a].cmp(&ivo[b])));
    let mut blocks: Vec<([f64; 2], u32)> = Vec::new();
    for &k in &ord {
        let x = [iv[k][0] * len, iv[k][1] * len];
        let o = ivo[k];
        let single = |o: u32| {
            let ob = &obstacles[o as usize];
            matches!(ob.kind, ObsKind::Pad | ObsKind::Via | ObsKind::Trace) && !ob.net.is_empty()
        };
        let tag = if o != NONE && single(o) { o } else { NONE };
        match blocks.last_mut() {
            Some((b, t)) if x[0] <= b[1] + 1e-6 => {
                b[1] = b[1].max(x[1]);
                if *t != tag
                    && (*t == NONE
                        || tag == NONE
                        || obstacles[*t as usize].net != obstacles[tag as usize].net)
                {
                    *t = NONE;
                }
            }
            _ => blocks.push((x, tag)),
        }
    }
    let mut parts = Vec::new();
    let mut cur = 0.0f64;
    for (b, _) in &blocks {
        if b[0] > cur {
            parts.push([cur, b[0]]);
        }
        cur = cur.max(b[1]);
    }
    if cur < len {
        parts.push([cur, len]);
    }
    // A sliver part is a sampling artifact (vertices sit 1 µm outside their
    // obstacle): fold it into the cover.
    parts.retain(|p| p[1] - p[0] > 1e-3);
    (parts, blocks)
}

/// The cut graphs of every signal-role layer of `pcb`.
pub struct CutGraph {
    /// Width/spacing model.
    pub params: CutParams,
    /// One graph per signal layer, in stack order.
    pub layers: Vec<LayerCdt>,
}

impl CutGraph {
    /// Build every signal layer's graph (plane / split-plane layers get none).
    pub fn build(pcb: &Pcb, params: CutParams, filter: CopperFilter) -> Self {
        Self::build_with(pcb, params, filter, &[])
    }

    /// [`CutGraph::build`] with Steiner vertices (via sites) on every layer.
    pub fn build_with(
        pcb: &Pcb,
        params: CutParams,
        filter: CopperFilter,
        steiner: &[[f64; 2]],
    ) -> Self {
        let planes = pcb.plane_layers();
        let copper: Vec<PcbLayer> = PcbLayer::COPPER_STACK
            .iter()
            .copied()
            .filter(|l| pcb.stackup.layers.iter().any(|s| s.layer == *l))
            .filter(|l| !planes.contains(l))
            .collect();
        use rayon::prelude::*;
        let layers = copper
            .par_iter()
            .map(|&l| {
                LayerCdt::build(
                    pcb,
                    l,
                    layer_obstacles(pcb, l, &params, filter),
                    steiner,
                    &params,
                )
            })
            .collect();
        Self { params, layers }
    }

    /// The graph of `layer`, if it is a signal layer.
    pub fn layer(&self, layer: PcbLayer) -> Option<&LayerCdt> {
        self.layers.iter().find(|l| l.layer == layer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs_disc(x: f64, y: f64, r: f64, g: f64) -> Obstacle {
        Obstacle {
            net: "N".into(),
            geom: CopperGeom::Disc {
                center: Vec2::new(x, y),
                r,
            },
            grow: g,
            kind: ObsKind::Pad,
        }
    }

    #[test]
    fn fit_counts_wires() {
        // 3 wires of 0.1 with 0.1 spacing need 0.5 mm.
        assert_eq!(fit(0.5, 0.1, 0.1), 3);
        assert_eq!(fit(0.49, 0.1, 0.1), 2);
        assert_eq!(fit(0.09, 0.1, 0.1), 0);
        assert_eq!(fit(0.1, 0.1, 0.1), 1);
    }

    #[test]
    fn clip_disc_chord() {
        let mut iv = Vec::new();
        clip_disc([-2.0, 0.0], [2.0, 0.0], [0.0, 0.0], 1.0, &mut iv);
        assert_eq!(iv.len(), 1);
        assert!((iv[0][0] - 0.25).abs() < 1e-12 && (iv[0][1] - 0.75).abs() < 1e-12);
    }

    #[test]
    fn two_pads_gap_capacity() {
        // Two discs r=0.5 at x=10 and x=13, grown by s + w/2 = 0.15: the
        // centerline gap is 3 - 2*0.65 = 1.7 mm, copper room 1.8 mm; w = 0.1,
        // s = 0.1 gives floor((1.8+0.1)/0.2) = 9 wires on the cut joining them.
        let pcb = crate::router::flow::test_board(20.0, 20.0, &["FCu"]);
        let p = CutParams {
            width: 0.1,
            clearance: 0.1,
            edge_clearance: 0.2,
            sample: 1.0,
        };
        let mut obs = layer_obstacles(&pcb, PcbLayer::FCu, &p, CopperFilter::PadsOnly);
        obs.push(obs_disc(10.0, 10.0, 0.5, 0.15));
        obs.push(obs_disc(13.0, 10.0, 0.5, 0.15));
        let g = LayerCdt::build(&pcb, PcbLayer::FCu, obs, &[], &p);
        // Walking the perpendicular bisector crosses the cut between them.
        let (pa, pb) = ([11.5, 8.0], [11.5, 12.0]);
        let (word, _) = g.walk(pa, pb, None).expect("walk");
        assert!(!word.is_empty());
        // Every crossing lies on the walked segment.
        for x in &word {
            let c = &g.cuts[x.cut as usize];
            let (a, b) = (g.vpos(c.a), g.vpos(c.b));
            let u = x.s / c.len;
            let pt = [a[0] + u * (b[0] - a[0]), a[1] + u * (b[1] - a[1])];
            assert!((pt[0] - 11.5).abs() < 1e-9, "crossing off segment: {pt:?}");
        }
        let caps: Vec<u32> = word.iter().map(|c| g.cuts[c.cut as usize].cap).collect();
        // The tightest cut crossed sits between the two pads.
        assert_eq!(*caps.iter().min().unwrap(), 9, "caps {caps:?}");
        // Edges inside a pad have no free length.
        assert!(g.cuts.iter().any(|c| c.cap == 0 && c.len > 0.0));
    }
}
