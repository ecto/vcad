//! Incremental routing session — the in-loop legality oracle.
//!
//! [`crate::drc`] answers "is the *finished* board legal?" by rebuilding a
//! spatial index from scratch every call. A router needs the opposite: to ask,
//! millions of times, "is *this* candidate segment legal, and how much
//! clearance does it have?" — while cheaply adding committed copper and ripping
//! it back out.
//!
//! [`RouteSession`] wraps the same R-tree broadphase and the same exact
//! [`CopperGeom::distance_to`] narrowphase the DRC clearance pass uses, plus the
//! same [`NetTieGroups`] exemptions, behind three operations:
//!
//! - [`RouteSession::probe`] — test a candidate geometry against existing copper
//!   without mutating anything (the avoidance constraint).
//! - [`RouteSession::commit`] — add a routed span, returning a stable [`SpanId`].
//! - [`RouteSession::remove`] — rip a span back out (tombstone + lazy compaction)
//!   so rip-up-and-reroute is O(drop) rather than a full index rebuild.
//!
//! Copper is not the whole legality story: a *drilled hole* obeys a rule that
//! has nothing to do with layers or nets. [`RouteSession::probe_drill`] /
//! [`RouteSession::commit_drill`] maintain a second index over via barrels and
//! through-hole pad drills, so two blind/buried vias on disjoint layer spans —
//! which share no copper element and therefore never meet in `probe` — still
//! have to keep `rules.hole_to_hole` apart. A committed barrel takes a
//! [`SpanId`] like any other span, so rip-up removes it with its copper.
//!
//! This is the structural inversion the router rests on: clearance becomes a
//! constraint consulted *during* the search, not a violation detected after it.

use std::collections::HashMap;

use rstar::{RTree, RTreeObject, AABB};

use vcad_ir::ecad::{Pcb, PcbLayer};
use vcad_ir::Vec2;

use crate::drc::{build_net_clearance_map, build_net_trace_width_map, NetTieGroups};
use crate::spatial::{copper_elements, CopperElement, CopperGeom};

/// Reserved net name for keepout boundary obstacles. Contains a control
/// character so no schematic net can collide with it (probes exempt same-net
/// copper — a colliding real net would route through keepouts freely).
const KEEPOUT_NET: &str = "\u{1}keepout";

/// Reserved net name for board-outline (and cutout) edge obstacles. Indexed
/// with a per-net clearance equal to the rules' `edge_clearance`, so every
/// probe-based path — maze raster, push-shove validation, via commits,
/// legalization shortcuts — enforces DRC's `EdgeClearance` by construction.
const EDGE_NET: &str = "\u{1}board-edge";

/// Stable handle to a committed copper span in a [`RouteSession`].
///
/// Remains valid across compaction — removing other spans never renumbers it.
pub type SpanId = usize;

/// A copper element carrying its session id, stored in the session's R-tree.
#[derive(Debug, Clone)]
struct SessionElement {
    id: SpanId,
    elem: CopperElement,
}

impl RTreeObject for SessionElement {
    type Envelope = AABB<[f64; 2]>;

    fn envelope(&self) -> Self::Envelope {
        AABB::from_corners(self.elem.min, self.elem.max)
    }
}

/// A drilled hole (via barrel or through-hole pad) in the session's drill
/// index. Holes are indexed apart from copper because the hole-to-hole rule is
/// mechanical: it ignores nets *and* layers, so two vias whose layer spans never
/// meet — a blind In1–In2 microvia and a buried In5–In6 one — are invisible to a
/// layer-scoped copper probe yet still collide in the drill file.
#[derive(Clone)]
struct DrillElement {
    center: Vec2,
    radius: f64,
    /// `Some` for a barrel the router committed — its liveness is tracked in
    /// `live` under this id, so rip-up takes the hole out with the copper.
    /// `None` for a hole that was on the board when the session was built.
    id: Option<SpanId>,
}

impl RTreeObject for DrillElement {
    type Envelope = AABB<[f64; 2]>;

    fn envelope(&self) -> Self::Envelope {
        AABB::from_corners(
            [self.center.x - self.radius, self.center.y - self.radius],
            [self.center.x + self.radius, self.center.y + self.radius],
        )
    }
}

/// Result of a [`RouteSession::probe_drill`] hole-to-hole test.
#[derive(Debug, Clone, PartialEq)]
pub struct DrillProbe {
    /// Whether the candidate hole satisfies the board's hole-to-hole rule.
    pub legal: bool,
    /// Closest edge-to-edge distance to an existing hole (mm); infinite when
    /// the board has no other holes.
    pub min_spacing: f64,
    /// Centre of the nearest hole, when one was found.
    pub nearest: Option<Vec2>,
}

/// A piece of existing copper found within the required clearance of a probed
/// candidate.
#[derive(Debug, Clone, PartialEq)]
pub struct Blocker {
    /// The blocking span (if it was committed; base board copper has an id too).
    pub span: SpanId,
    /// The blocker's net.
    pub net: String,
    /// The blocker's layer.
    pub layer: PcbLayer,
    /// True edge-to-edge distance to the candidate (mm).
    pub distance: f64,
}

/// Outcome of probing a candidate geometry against the session.
#[derive(Debug, Clone, PartialEq)]
pub struct ProbeResult {
    /// True when no other-net copper sits closer than the required clearance.
    pub legal: bool,
    /// Smallest edge-to-edge distance to any other-net copper (mm).
    /// `f64::INFINITY` when nothing relevant is nearby.
    pub min_clearance: f64,
    /// Every other-net element within the required clearance.
    pub blockers: Vec<Blocker>,
}

/// Cell size (mm) of the change-tracking grid — coarse on purpose: it
/// answers "did anything change near this corridor?", not "what changed".
const DIRTY_CELL: f64 = 4.0;

/// Coarse change-tracking grid over the board: every commit/remove stamps a
/// monotonically increasing epoch onto the cells its bbox overlaps, so a
/// caller can cheaply ask whether any copper changed inside a region since
/// it last looked (the router's failure cache).
#[derive(Clone)]
struct DirtyGrid {
    origin: [f64; 2],
    nx: usize,
    ny: usize,
    epoch: Vec<u64>,
}

impl DirtyGrid {
    fn new(lo: [f64; 2], hi: [f64; 2]) -> Self {
        let nx = (((hi[0] - lo[0]) / DIRTY_CELL).ceil() as usize + 1).max(1);
        let ny = (((hi[1] - lo[1]) / DIRTY_CELL).ceil() as usize + 1).max(1);
        Self {
            origin: lo,
            nx,
            ny,
            epoch: vec![0; nx * ny],
        }
    }

    fn cell_range(&self, lo: [f64; 2], hi: [f64; 2]) -> (usize, usize, usize, usize) {
        let cx = |x: f64| {
            (((x - self.origin[0]) / DIRTY_CELL).floor() as i64).clamp(0, self.nx as i64 - 1)
                as usize
        };
        let cy = |y: f64| {
            (((y - self.origin[1]) / DIRTY_CELL).floor() as i64).clamp(0, self.ny as i64 - 1)
                as usize
        };
        (cx(lo[0]), cy(lo[1]), cx(hi[0]), cy(hi[1]))
    }

    fn mark(&mut self, lo: [f64; 2], hi: [f64; 2], epoch: u64) {
        let (x0, y0, x1, y1) = self.cell_range(lo, hi);
        for y in y0..=y1 {
            for x in x0..=x1 {
                self.epoch[y * self.nx + x] = epoch;
            }
        }
    }

    fn max_epoch(&self, lo: [f64; 2], hi: [f64; 2]) -> u64 {
        let (x0, y0, x1, y1) = self.cell_range(lo, hi);
        let mut m = 0;
        for y in y0..=y1 {
            for x in x0..=x1 {
                m = m.max(self.epoch[y * self.nx + x]);
            }
        }
        m
    }
}

/// An incremental copper index for routing: probe, commit, rip-up.
#[derive(Clone)]
pub struct RouteSession {
    tree: RTree<SessionElement>,
    /// Liveness by id; a tombstoned (removed) span is `false`.
    live: Vec<bool>,
    /// Count of tombstoned-but-not-yet-compacted spans.
    dead: usize,
    net_ties: NetTieGroups,
    net_clearance: HashMap<String, f64>,
    /// net → (twin net, gap, leg width) for declared differential pairs: the
    /// probe enforces the pair GAP between the twins' leg-width copper, so no
    /// routing stage can emit an intra-pair pinch the DRC would flag.
    pair_rules: HashMap<String, (String, f64, f64)>,
    /// Pad positions of the nets in `pair_rules` — the breakout regions where
    /// the DRC relaxes the pair gap (see [`RouteSession::in_pair_breakout`]).
    pair_pads: HashMap<String, Vec<Vec2>>,
    /// Whether the intra-pair gap is scored the way the DRC scores it, including
    /// against copper that carries no width of its own — see
    /// [`RouteSession::set_strict_pair_coupling`].
    strict_pair_coupling: bool,
    default_clearance: f64,
    /// Largest clearance any net requires — the broadphase must reach this far
    /// so a wide net's clearance is never missed when it exceeds the candidate's.
    max_clearance: f64,
    net_width: HashMap<String, f64>,
    /// Every drilled hole on the board, for the layer- and net-agnostic
    /// hole-to-hole rule (see [`DrillElement`]).
    drills: RTree<DrillElement>,
    /// Largest indexed drill radius — the broadphase reach for a drill probe.
    max_drill_radius: f64,
    /// The board's minimum hole-to-hole edge spacing (mm).
    hole_to_hole: f64,
    /// True at ids that name a drilled barrel rather than a copper span
    /// (parallel to `live`), so `remove` knows which index to compact. A
    /// router-committed barrel takes a [`SpanId`] like any other span, so
    /// rip-up rips the hole out with the copper instead of leaking it.
    is_drill: Vec<bool>,
    /// Count of tombstoned-but-not-yet-compacted drills.
    drills_dead: usize,
    /// net → via drill diameter from its net class (mirrors how a routed via
    /// is sized when it is written back to the board).
    net_via_drill: HashMap<String, f64>,
    default_via_drill: f64,
    /// Per-span bboxes (parallel to `live`), so a remove can stamp the dirty
    /// grid without consulting the tree.
    bounds: Vec<[f64; 4]>,
    /// Coarse change-tracking grid; see [`RouteSession::region_epoch`].
    dirty: DirtyGrid,
    /// Monotonic change counter, bumped on every commit/remove.
    change_epoch: u64,
}

impl RouteSession {
    /// Build a session seeded with every copper element on `pcb`.
    pub fn from_pcb(pcb: &Pcb) -> Self {
        let mut elems = copper_elements(pcb);
        // No-tracks keepouts become session obstacles so the router's oracle
        // matches DRC's `check_keepout`: their boundary edges are indexed as
        // zero-width capsules under a reserved net name no real net can carry
        // (so they block every candidate; probes only exempt same-net copper).
        // Boundary-only is sufficient — any route entering the region must
        // cross an edge — and, unlike copper_elements, this stays session-local
        // so DRC's clearance pass never sees the phantom segments.
        for keepout in &pcb.keepouts {
            if !keepout.no_tracks || keepout.outline.len() < 3 {
                continue;
            }
            for layer in keepout.layers.iter().filter(|l| l.is_copper()) {
                for i in 0..keepout.outline.len() {
                    let a = keepout.outline[i];
                    let b = keepout.outline[(i + 1) % keepout.outline.len()];
                    elems.push(CopperElement {
                        min: [a.x.min(b.x), a.y.min(b.y)],
                        max: [a.x.max(b.x), a.y.max(b.y)],
                        net: KEEPOUT_NET.into(),
                        layer: *layer,
                        geom: CopperGeom::Segment { a, b, half_w: 0.0 },
                    });
                }
            }
        }
        // Board outline and cutout edges, on every copper layer, under the
        // reserved edge net (clearance = rules.edge_clearance, wired below).
        let stackup_copper: Vec<PcbLayer> = pcb
            .stackup
            .layers
            .iter()
            .map(|l| l.layer)
            .filter(|l| l.is_copper())
            .collect();
        let mut edge_ring = |poly: &[vcad_ir::Vec2]| {
            if poly.len() < 3 {
                return;
            }
            for layer in &stackup_copper {
                for i in 0..poly.len() {
                    let a = poly[i];
                    let b = poly[(i + 1) % poly.len()];
                    elems.push(CopperElement {
                        min: [a.x.min(b.x), a.y.min(b.y)],
                        max: [a.x.max(b.x), a.y.max(b.y)],
                        net: EDGE_NET.into(),
                        layer: *layer,
                        geom: CopperGeom::Segment { a, b, half_w: 0.0 },
                    });
                }
            }
        };
        edge_ring(&pcb.outline.vertices);
        for cutout in &pcb.outline.cutouts {
            edge_ring(cutout);
        }
        let elems = elems;
        let live = vec![true; elems.len()];
        let is_drill = vec![false; elems.len()];
        let bounds: Vec<[f64; 4]> = elems
            .iter()
            .map(|e| [e.min[0], e.min[1], e.max[0], e.max[1]])
            .collect();
        // Dirty grid over the board outline ∪ existing copper, with margin.
        let mut lo = [f64::INFINITY; 2];
        let mut hi = [f64::NEG_INFINITY; 2];
        for v in &pcb.outline.vertices {
            lo[0] = lo[0].min(v.x);
            lo[1] = lo[1].min(v.y);
            hi[0] = hi[0].max(v.x);
            hi[1] = hi[1].max(v.y);
        }
        for b in &bounds {
            lo[0] = lo[0].min(b[0]);
            lo[1] = lo[1].min(b[1]);
            hi[0] = hi[0].max(b[2]);
            hi[1] = hi[1].max(b[3]);
        }
        if !lo[0].is_finite() {
            (lo, hi) = ([0.0, 0.0], [100.0, 100.0]);
        }
        let dirty = DirtyGrid::new([lo[0] - 5.0, lo[1] - 5.0], [hi[0] + 5.0, hi[1] + 5.0]);
        let session_elems: Vec<SessionElement> = elems
            .into_iter()
            .enumerate()
            .map(|(id, elem)| SessionElement { id, elem })
            .collect();
        let default_clearance = pcb.rules.default_rules.clearance;
        let mut net_clearance = build_net_clearance_map(pcb);
        net_clearance.insert(EDGE_NET.into(), pcb.rules.edge_clearance);
        let max_clearance = net_clearance
            .values()
            .copied()
            .fold(default_clearance, f64::max);
        // net → via drill from its class, mirroring how the router's vias are
        // sized on write-back (see `router::legalize::via_geom_for`), so a
        // caller can probe a candidate via without plumbing drill sizes.
        let default_via_drill = pcb.rules.default_rules.via_drill;
        let mut net_via_drill: HashMap<String, f64> = HashMap::new();
        for (class, nets) in &pcb.rules.net_class_assignments {
            let Some(c) = pcb.rules.class_rules.iter().find(|c| c.name == *class) else {
                continue;
            };
            for n in nets {
                net_via_drill.insert(n.clone(), c.via_drill);
            }
        }
        let drills = drill_elements(pcb);
        // Broadphase reach: the biggest drill radius already on the board or
        // that any net class can still mint.
        let max_drill_radius = net_via_drill
            .values()
            .chain(std::iter::once(&default_via_drill))
            .map(|d| d / 2.0)
            .chain(drills.iter().map(|d| d.radius))
            .fold(0.0_f64, f64::max);        let pair_nets: std::collections::HashSet<String> = crate::drc::diff_pairs(pcb)
            .into_iter()
            .flat_map(|dp| [dp.net_p, dp.net_n])
            .collect();
        let mut pair_pads: HashMap<String, Vec<Vec2>> = HashMap::new();
        for fp in &pcb.footprints {
            for pad in &fp.pads {
                let Some(net) = pad.net.as_ref().filter(|n| pair_nets.contains(*n)) else {
                    continue;
                };
                pair_pads
                    .entry(net.clone())
                    .or_default()
                    .push(crate::geometry::pad_world_position(fp, pad));
            }
        }
        Self {
            tree: RTree::bulk_load(session_elems),
            live,
            dead: 0,
            net_ties: NetTieGroups::from_pcb(pcb),
            net_clearance,
            pair_rules: {
                let mut m = HashMap::new();
                for dp in crate::drc::diff_pairs(pcb) {
                    m.insert(dp.net_p.clone(), (dp.net_n.clone(), dp.gap, dp.width));
                    m.insert(dp.net_n.clone(), (dp.net_p.clone(), dp.gap, dp.width));
                }
                m
            },
            pair_pads,
            strict_pair_coupling: false,
            default_clearance,
            max_clearance,
            net_width: build_net_trace_width_map(pcb),
            drills: RTree::bulk_load(drills),
            max_drill_radius,
            hole_to_hole: pcb.rules.hole_to_hole,
            is_drill,
            drills_dead: 0,
            net_via_drill,
            default_via_drill,
            bounds,
            dirty,
            change_epoch: 0,
        }
    }

    /// The current change epoch: bumped on every commit and remove. Compare
    /// with [`RouteSession::region_epoch`] to detect regional change.
    pub fn epoch(&self) -> u64 {
        self.change_epoch
    }

    /// The largest change epoch stamped on any copper committed or removed
    /// inside the region `lo..hi` (mm). `0` means nothing has changed there
    /// since the session was built.
    pub fn region_epoch(&self, lo: [f64; 2], hi: [f64; 2]) -> u64 {
        self.dirty.max_epoch(lo, hi)
    }

    /// The required clearance for `net` from the design rules.
    pub fn clearance_for(&self, net: &str) -> f64 {
        self.net_clearance
            .get(net)
            .copied()
            .unwrap_or(self.default_clearance)
    }

    /// Test a candidate drilled hole against every hole already on the board.
    ///
    /// Nets and layers are deliberately ignored: hole-to-hole is a mechanical
    /// rule about the drill file, and it is the one legality question a copper
    /// probe structurally cannot answer — two vias on disjoint layer spans share
    /// no layer, so no layer-scoped probe ever compares them, and they land as
    /// drill collisions the DRC only finds after the fact.
    pub fn probe_drill(&self, center: Vec2, diameter: f64) -> DrillProbe {
        let r = diameter / 2.0;
        let reach = r + self.max_drill_radius + self.hole_to_hole;
        let mut min_spacing = f64::INFINITY;
        let mut nearest = None;
        for hole in self
            .drills
            .locate_in_envelope_intersecting(&AABB::from_corners(
                [center.x - reach, center.y - reach],
                [center.x + reach, center.y + reach],
            ))
        {
            // A router-committed barrel that has since been ripped up is not
            // an obstacle — otherwise rip-up-and-reroute would leak holes and
            // slowly wall the board off.
            if hole.id.is_some_and(|i| !self.live[i]) {
                continue;
            }
            let d = ((hole.center.x - center.x).powi(2) + (hole.center.y - center.y).powi(2))
                .sqrt()
                - hole.radius
                - r;
            if d < min_spacing {
                min_spacing = d;
                nearest = Some(hole.center);
            }
        }
        DrillProbe {
            legal: min_spacing >= self.hole_to_hole - 1e-6,
            min_spacing,
            nearest,
        }
    }

    /// Score the intra-pair gap exactly as the DRC does — including against a
    /// twin's pads and via annuli, which carry no width of their own and which
    /// `drc::pair_aware_clearance_w` therefore treats as coupled leg copper.
    ///
    /// Off by default, because it is *stricter* than the geometry the pair
    /// router currently realizes: its dog-bone offsets and jogs are sized to the
    /// base clearance, so turning this on globally would stop pairs from
    /// changing layers rather than making them legal (see the twin-clearance
    /// tests in `router::pair`). A caller that must not emit copper the board
    /// DRC will flag — the verdict driver, which commits fail-closed — turns it
    /// on and pays for it in routability instead.
    pub fn set_strict_pair_coupling(&mut self, strict: bool) {
        self.strict_pair_coupling = strict;
    }

    /// Whether a candidate/blocker pair sits in a twin's **pad breakout**, where
    /// the DRC relaxes the intra-pair gap to the base clearance.
    ///
    /// Mirrors `drc::check_clearance`: the gap binds the coupled run, not the
    /// escape region (1.5 mm) around either twin's own pads, where the legs must
    /// converge to reach their land patterns. The DRC judges a pair from the
    /// *trace* side, so copper with no endpoints of its own — a via annulus or a
    /// pad — imposes nothing from its side of the comparison.
    fn in_pair_breakout(
        &self,
        cand: &CopperGeom,
        blocker: &CopperGeom,
        net: &str,
        twin: &str,
    ) -> bool {
        const BREAKOUT_MM: f64 = 1.5;
        let near_pad =
            |p: Vec2| {
                [net, twin].iter().any(|n| {
                    self.pair_pads.get(*n).into_iter().flatten().any(|q| {
                        (q.x - p.x).powi(2) + (q.y - p.y).powi(2) <= BREAKOUT_MM * BREAKOUT_MM
                    })
                })
            };
        let side = |g: &CopperGeom| match g {
            CopperGeom::Segment { a, b, .. } => near_pad(*a) || near_pad(*b),
            _ => true,
        };
        side(cand) && side(blocker)
    }

    /// The board's minimum hole-to-hole edge spacing (mm).
    pub fn hole_to_hole(&self) -> f64 {
        self.hole_to_hole
    }

    /// Index a drilled hole so later [`RouteSession::probe_drill`] calls see it,
    /// returning its stable [`SpanId`]. Call this for every via a router
    /// commits — the copper commit alone leaves the barrel invisible to the
    /// hole-to-hole rule, so two router-placed vias on disjoint layer spans
    /// would still collide in the drill file.
    ///
    /// The id shares the span space with copper, so
    /// [`RouteSession::remove`] rips the barrel out exactly like a trace: a
    /// ripped-up via must stop blocking the ground it used to occupy.
    pub fn commit_drill(&mut self, center: Vec2, diameter: f64) -> SpanId {
        let id = self.live.len();
        let radius = diameter / 2.0;
        self.live.push(true);
        self.is_drill.push(true);
        let bbox = [
            center.x - radius,
            center.y - radius,
            center.x + radius,
            center.y + radius,
        ];
        self.bounds.push(bbox);
        self.change_epoch += 1;
        self.dirty
            .mark([bbox[0], bbox[1]], [bbox[2], bbox[3]], self.change_epoch);
        self.max_drill_radius = self.max_drill_radius.max(radius);
        self.drills.insert(DrillElement {
            center,
            radius,
            id: Some(id),
        });
        id
    }

    /// The declared intra-pair gap between `net` and `other` when the two are
    /// twins of a differential pair, else `None`. This is the separation the
    /// probe demands between their leg-width copper, so a router that must keep
    /// two candidate paths mutually legal has to space them by at least this
    /// much — not merely by the base clearance.
    pub fn pair_gap_between(&self, net: &str, other: &str) -> Option<f64> {
        self.pair_rules
            .get(net)
            .filter(|(twin, _, _)| twin == other)
            .map(|&(_, gap, _)| gap)
    }

    /// The trace width for `net` from its net class, or `fallback` if the net
    /// has no class width (so power/ground classes route wider than signals).
    pub fn width_for(&self, net: &str, fallback: f64) -> f64 {
        self.net_width.get(net).copied().unwrap_or(fallback)
    }

    /// Number of live (non-tombstoned) spans — copper elements and drilled
    /// holes alike.
    pub fn len(&self) -> usize {
        self.live.iter().filter(|&&l| l).count()
    }

    /// True when the session holds no live copper.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Commit a routed span, returning its stable [`SpanId`].
    pub fn commit(&mut self, elem: CopperElement) -> SpanId {
        let id = self.live.len();
        self.live.push(true);
        self.is_drill.push(false);
        let bbox = [elem.min[0], elem.min[1], elem.max[0], elem.max[1]];
        self.bounds.push(bbox);
        self.change_epoch += 1;
        self.dirty
            .mark([bbox[0], bbox[1]], [bbox[2], bbox[3]], self.change_epoch);
        self.tree.insert(SessionElement { id, elem });
        id
    }

    /// Rip a committed span back out. Returns `false` if the id was already
    /// removed or never existed. Tombstones the span; the R-tree is compacted
    /// once tombstones exceed half its contents, keeping ids stable throughout.
    pub fn remove(&mut self, id: SpanId) -> bool {
        if id >= self.live.len() || !self.live[id] {
            return false;
        }
        self.live[id] = false;
        let b = self.bounds[id];
        self.change_epoch += 1;
        self.dirty
            .mark([b[0], b[1]], [b[2], b[3]], self.change_epoch);
        if self.is_drill[id] {
            self.drills_dead += 1;
            if self.drills_dead * 2 > self.drills.size() {
                self.compact_drills();
            }
        } else {
            self.dead += 1;
            if self.dead * 2 > self.tree.size() {
                self.compact();
            }
        }
        true
    }

    /// Rebuild the R-tree without tombstoned spans. Ids are preserved.
    fn compact(&mut self) {
        let live = &self.live;
        let kept: Vec<SessionElement> = self.tree.iter().filter(|e| live[e.id]).cloned().collect();
        self.tree = RTree::bulk_load(kept);
        self.dead = 0;
    }

    /// Rebuild the drill index without tombstoned barrels. Ids are preserved.
    fn compact_drills(&mut self) {
        let live = &self.live;
        let kept: Vec<DrillElement> = self
            .drills
            .iter()
            .filter(|d| d.id.is_none_or(|i| live[i]))
            .cloned()
            .collect();
        self.drills = RTree::bulk_load(kept);
        self.drills_dead = 0;
    }

    /// The drill diameter (mm) a via on `net` gets from its net class — the
    /// same size the router writes back to the board (mirrors
    /// `router::legalize::via_geom_for`), so a caller can probe a candidate via
    /// without plumbing drill sizes through every call site.
    pub fn via_drill_for(&self, net: &str) -> f64 {
        self.net_via_drill
            .get(net)
            .copied()
            .unwrap_or(self.default_via_drill)
    }

    /// [`RouteSession::probe_drill`] for a via on `net`, sized from its net
    /// class — the shape every via-placement site in the router uses.
    pub fn probe_via_drill(&self, center: Vec2, net: &str) -> DrillProbe {
        self.probe_drill(center, self.via_drill_for(net))
    }

    /// Axis-aligned bounding boxes (`(min, max)` corners) of every live copper
    /// element on `layer` belonging to a net other than `net`, restricted to the
    /// query box `[lo, hi]`.
    ///
    /// Seeds the push-and-shove visibility router with the obstacles it must
    /// detour around. The boxes are coarse (they over-approximate the true
    /// copper), so any route built from them is re-validated with [`probe`]
    /// before it is trusted — this is purely a candidate-generation helper.
    ///
    /// [`probe`]: RouteSession::probe
    pub fn obstacles_in(
        &self,
        layer: PcbLayer,
        net: &str,
        lo: [f64; 2],
        hi: [f64; 2],
    ) -> Vec<(Vec2, Vec2)> {
        self.tree
            .locate_in_envelope_intersecting(&AABB::from_corners(lo, hi))
            .filter(|se| self.live[se.id])
            .filter(|se| se.elem.layer == layer && se.elem.net != net)
            .map(|se| {
                (
                    Vec2::new(se.elem.min[0], se.elem.min[1]),
                    Vec2::new(se.elem.max[0], se.elem.max[1]),
                )
            })
            .collect()
    }

    /// Probe a candidate geometry on `layer` belonging to `net` against all
    /// existing copper, requiring `clearance` mm to other nets.
    ///
    /// Mutates nothing. Same-net and net-tied copper is never a blocker — the
    /// exact rule the DRC clearance pass applies, so a span that probes legal
    /// here is legal in [`crate::check_drc`].
    /// Visit every live copper element on `layer` that `net` must clear,
    /// within the window `lo..hi` (mm): the element's geometry, its AABB, and
    /// the clearance required between it and `net` (the larger of the two
    /// nets' rules, matching [`RouteSession::probe`]).
    ///
    /// Net-tie exemptions are deliberately NOT applied — tied nets are
    /// visited as blockers. This visitor exists to build conservative
    /// occupancy rasters for the maze search: a region-scoped tie exemption
    /// has no single per-element answer, and over-blocking is safe (the
    /// exact probe remains the commit gate) while under-blocking never is.
    pub fn for_each_blocking(
        &self,
        layer: PcbLayer,
        net: &str,
        lo: [f64; 2],
        hi: [f64; 2],
        mut f: impl FnMut(&CopperGeom, [f64; 2], [f64; 2], f64),
    ) {
        for se in self
            .tree
            .locate_in_envelope_intersecting(&AABB::from_corners(lo, hi))
        {
            if !self.live[se.id] {
                continue;
            }
            let e = &se.elem;
            if e.layer != layer || e.net == net {
                continue;
            }
            let required = self.clearance_for(net).max(self.clearance_for(&e.net));
            f(&e.geom, e.min, e.max, required);
        }
    }

    /// Visit every live element OF `net` on any copper layer: geometry, AABB,
    /// and layer. Used to build the route-to-tree goal set (a connection may
    /// legally terminate on any copper already belonging to its net).
    pub fn for_each_of_net(
        &self,
        net: &str,
        mut f: impl FnMut(&CopperGeom, [f64; 2], [f64; 2], PcbLayer),
    ) {
        for se in self.tree.iter() {
            if !self.live[se.id] || se.elem.net != net {
                continue;
            }
            f(&se.elem.geom, se.elem.min, se.elem.max, se.elem.layer);
        }
    }

    /// Total live copper bbox area (mm², summed across layers) clipped to the
    /// window `lo..hi` — the capacity mesh's density estimate. Bboxes, not
    /// exact geometry: capacity is a budget, not a legality answer.
    pub fn copper_area_in(&self, lo: [f64; 2], hi: [f64; 2]) -> f64 {
        self.tree
            .locate_in_envelope_intersecting(&AABB::from_corners(lo, hi))
            .filter(|se| self.live[se.id])
            .map(|se| {
                let w = (se.elem.max[0].min(hi[0]) - se.elem.min[0].max(lo[0])).max(0.0);
                let h = (se.elem.max[1].min(hi[1]) - se.elem.min[1].max(lo[1])).max(0.0);
                w * h
            })
            .sum()
    }

    /// Probe a candidate geometry against every live other-net element on
    /// `layer`: legal iff nothing sits closer than the required clearance
    /// (the larger of the candidate's and each blocker's rule), with
    /// region-scoped net-tie exemptions honored.
    pub fn probe(
        &self,
        geom: &CopperGeom,
        layer: PcbLayer,
        net: &str,
        clearance: f64,
    ) -> ProbeResult {
        // Reach by the larger of the candidate's clearance and the biggest
        // clearance any net requires, so a wide net beyond the candidate's own
        // clearance is still found in the broadphase.
        let search = clearance.max(self.max_clearance);
        let (mut lo, mut hi) = geom_aabb(geom);
        lo[0] -= search;
        lo[1] -= search;
        hi[0] += search;
        hi[1] += search;
        let cand_center = geom_center(geom);

        let mut min_clearance = f64::INFINITY;
        let mut blockers = Vec::new();

        for se in self
            .tree
            .locate_in_envelope_intersecting(&AABB::from_corners(lo, hi))
        {
            if !self.live[se.id] {
                continue;
            }
            let e = &se.elem;
            if e.layer != layer || e.net == net {
                continue;
            }
            // Contact point: midway between the candidate and the blocker, for
            // region-scoped net-tie tests (mirrors the DRC clearance pass).
            let ec = Vec2::new((e.min[0] + e.max[0]) / 2.0, (e.min[1] + e.max[1]) / 2.0);
            let contact = Vec2::new((cand_center.x + ec.x) / 2.0, (cand_center.y + ec.y) / 2.0);
            if self.net_ties.exempt(net, &e.net, contact) {
                continue;
            }

            let d = geom.distance_to(&e.geom);
            if d < min_clearance {
                min_clearance = d;
            }
            // Two nets must clear by the larger of their two rules, matching the
            // DRC clearance pass (which flags the pair from whichever side has
            // the bigger requirement). A wide power net thus pushes thin signals
            // away by its own clearance, not theirs.
            let mut required = clearance.max(self.clearance_for(&e.net));
            // Intra-pair gap rule, mirrored from the DRC: both elements at
            // the pair's leg width must keep the declared gap (minus the 5um
            // exact-coupling tolerance). Necks and other thin copper keep the
            // base clearance — the uncoupled entry region.
            if let Some((twin, gap, leg_w)) = self.pair_rules.get(net) {
                if &e.net == twin {
                    // Which copper counts as a coupled leg. Segments count at
                    // (nearly) the leg width — a thinner neck is the uncoupled
                    // entry by definition. Copper with no width of its own (a
                    // pad, a via annulus) counts only in strict mode, which is
                    // how `drc::pair_aware_clearance_w` scores it.
                    let strict = self.strict_pair_coupling;
                    let leg = |g: &CopperGeom| match g {
                        CopperGeom::Segment { half_w, .. } => 2.0 * half_w >= leg_w - 0.01,
                        _ => strict,
                    };
                    if leg(geom) && leg(&e.geom) && !self.in_pair_breakout(geom, &e.geom, net, twin)
                    {
                        required = required.max(gap - 0.005);
                    }
                }
            }
            if d < required {
                blockers.push(Blocker {
                    span: se.id,
                    net: e.net.clone(),
                    layer: e.layer,
                    distance: d,
                });
            }
        }

        ProbeResult {
            legal: blockers.is_empty(),
            min_clearance,
            blockers,
        }
    }
}

/// Every drilled hole on `pcb`: via barrels and through-hole pads. Mirrors the
/// DRC's own hole census (`check_hole_to_hole`), so the router's answer and the
/// board's verdict cannot disagree.
fn drill_elements(pcb: &Pcb) -> Vec<DrillElement> {
    let mut out: Vec<DrillElement> = pcb
        .vias
        .iter()
        .map(|via| DrillElement {
            center: via.position,
            radius: via.drill / 2.0,
            id: None,
        })
        .collect();
    for fp in &pcb.footprints {
        for pad in &fp.pads {
            if let Some(drill) = &pad.drill {
                out.push(DrillElement {
                    center: crate::geometry::pad_world_position(fp, pad),
                    radius: drill.diameter / 2.0,
                    id: None,
                });
            }
        }
    }
    out
}

/// Axis-aligned bounding box (copper extent) of a [`CopperGeom`].
fn geom_aabb(g: &CopperGeom) -> ([f64; 2], [f64; 2]) {
    match g {
        CopperGeom::Segment { a, b, half_w } => (
            [a.x.min(b.x) - half_w, a.y.min(b.y) - half_w],
            [a.x.max(b.x) + half_w, a.y.max(b.y) + half_w],
        ),
        CopperGeom::Disc { center, r } => {
            ([center.x - r, center.y - r], [center.x + r, center.y + r])
        }
        CopperGeom::Rect {
            center,
            half_w,
            half_h,
            rot,
        } => {
            let (s, c) = rot.sin_cos();
            let ex = half_w * c.abs() + half_h * s.abs();
            let ey = half_w * s.abs() + half_h * c.abs();
            (
                [center.x - ex, center.y - ey],
                [center.x + ex, center.y + ey],
            )
        }
    }
}

/// Representative center of a [`CopperGeom`] (used for net-tie contact points).
fn geom_center(g: &CopperGeom) -> Vec2 {
    match g {
        CopperGeom::Segment { a, b, .. } => Vec2::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0),
        CopperGeom::Disc { center, .. } | CopperGeom::Rect { center, .. } => *center,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_ir::ecad::*;

    fn rules() -> DesignRules {
        DesignRules {
            default_rules: NetClassRules {
                name: "Default".into(),
                trace_width: 0.25,
                clearance: 0.2,
                via_diameter: 0.8,
                via_drill: 0.4,
                diff_pair_gap: None,
                diff_pair_width: None,
            },
            class_rules: vec![],
            net_class_assignments: std::collections::HashMap::new(),
            edge_clearance: 0.5,
            hole_to_hole: 0.5,
            min_annular_ring: 0.15,
            min_drill: 0.2,
        }
    }

    fn empty_pcb() -> Pcb {
        Pcb {
            outline: BoardOutline {
                vertices: vec![
                    Vec2::new(0.0, 0.0),
                    Vec2::new(100.0, 0.0),
                    Vec2::new(100.0, 100.0),
                    Vec2::new(0.0, 100.0),
                ],
                cutouts: vec![],
                thickness: 1.6,
            },
            stackup: LayerStackup {
                layers: vec![StackupLayer {
                    layer: PcbLayer::FCu,
                    copper_thickness: Some(0.035),
                    dielectric_thickness: None,
                    dielectric_er: None,
                    material: None,
                }],
            },
            nets: vec![],
            rules: rules(),
            footprints: vec![],
            traces: vec![],
            trace_arcs: vec![],
            vias: vec![],
            zones: vec![],
            keepouts: vec![],
            net_ties: vec![],
        }
    }

    fn h_trace(y: f64, net: &str) -> Trace {
        Trace {
            start: Vec2::new(10.0, y),
            end: Vec2::new(90.0, y),
            width: 0.25,
            layer: PcbLayer::FCu,
            net: net.into(),
            source: None,
        }
    }

    fn seg(y: f64, half_w: f64) -> CopperGeom {
        CopperGeom::Segment {
            a: Vec2::new(10.0, y),
            b: Vec2::new(90.0, y),
            half_w,
        }
    }

    #[test]
    fn probe_clean_board_is_legal() {
        let session = RouteSession::from_pcb(&empty_pcb());
        let r = session.probe(&seg(50.0, 0.125), PcbLayer::FCu, "SIG", 0.2);
        assert!(r.legal);
        assert!(r.blockers.is_empty());
        assert_eq!(r.min_clearance, f64::INFINITY);
    }

    #[test]
    fn probe_detects_other_net_blocker() {
        let mut pcb = empty_pcb();
        pcb.traces.push(h_trace(50.0, "GND"));
        let session = RouteSession::from_pcb(&pcb);
        // Candidate 0.3mm centerline away (edge-to-edge 0.3 - 0.125 - 0.125 =
        // 0.05) on a different net, required clearance 0.2 -> blocked.
        let r = session.probe(&seg(50.3, 0.125), PcbLayer::FCu, "SIG", 0.2);
        assert!(!r.legal, "0.05mm gap must violate 0.2mm clearance");
        assert_eq!(r.blockers.len(), 1);
        assert_eq!(r.blockers[0].net, "GND");
        assert!((r.min_clearance - 0.05).abs() < 1e-9);
    }

    #[test]
    fn probe_ignores_same_net() {
        let mut pcb = empty_pcb();
        pcb.traces.push(h_trace(50.0, "SIG"));
        let session = RouteSession::from_pcb(&pcb);
        // Overlapping, but same net — never a blocker.
        let r = session.probe(&seg(50.0, 0.125), PcbLayer::FCu, "SIG", 0.2);
        assert!(r.legal);
        assert!(r.blockers.is_empty());
    }

    #[test]
    fn probe_ignores_other_layer() {
        let mut pcb = empty_pcb();
        pcb.traces.push(h_trace(50.0, "GND"));
        let session = RouteSession::from_pcb(&pcb);
        let r = session.probe(&seg(50.3, 0.125), PcbLayer::BCu, "SIG", 0.2);
        assert!(r.legal, "blocker on FCu must not affect a BCu candidate");
    }

    #[test]
    fn remove_reopens_a_blocked_region() {
        // The keystone property: rip-up makes a previously-illegal route legal.
        let session_pcb = empty_pcb();
        let mut session = RouteSession::from_pcb(&session_pcb);
        let blocker = CopperElement {
            min: [10.0, 50.0 - 0.125],
            max: [90.0, 50.0 + 0.125],
            net: "GND".into(),
            layer: PcbLayer::FCu,
            geom: seg(50.0, 0.125),
        };
        let id = session.commit(blocker);

        let cand = seg(50.3, 0.125);
        assert!(!session.probe(&cand, PcbLayer::FCu, "SIG", 0.2).legal);

        assert!(session.remove(id));
        assert!(
            session.probe(&cand, PcbLayer::FCu, "SIG", 0.2).legal,
            "after rip-up the region must be routable"
        );
        assert!(!session.remove(id), "double-remove returns false");
    }

    #[test]
    fn probe_blocks_no_tracks_keepout() {
        // A no-tracks keepout square straddling the candidate path must make
        // the probe illegal — the oracle mirrors DRC's check_keepout, so the
        // router can never emit copper through a keepout.
        let mut pcb = empty_pcb();
        pcb.keepouts.push(Keepout {
            outline: vec![
                Vec2::new(45.0, 45.0),
                Vec2::new(55.0, 45.0),
                Vec2::new(55.0, 55.0),
                Vec2::new(45.0, 55.0),
            ],
            layers: vec![PcbLayer::FCu],
            no_tracks: true,
            no_vias: false,
            no_pour: false,
            no_components: false,
        });
        let session = RouteSession::from_pcb(&pcb);
        // Crosses the keepout boundary: illegal.
        let r = session.probe(&seg(50.0, 0.125), PcbLayer::FCu, "SIG", 0.2);
        assert!(
            !r.legal,
            "trace through a no-tracks keepout must be blocked"
        );
        // Well clear of the keepout: legal.
        let r = session.probe(&seg(20.0, 0.125), PcbLayer::FCu, "SIG", 0.2);
        assert!(r.legal);
    }

    #[test]
    fn keepout_without_no_tracks_does_not_block() {
        // A components-only keepout is not a routing obstacle.
        let mut pcb = empty_pcb();
        pcb.keepouts.push(Keepout {
            outline: vec![
                Vec2::new(45.0, 45.0),
                Vec2::new(55.0, 45.0),
                Vec2::new(55.0, 55.0),
                Vec2::new(45.0, 55.0),
            ],
            layers: vec![PcbLayer::FCu],
            no_tracks: false,
            no_vias: false,
            no_pour: false,
            no_components: true,
        });
        let session = RouteSession::from_pcb(&pcb);
        let r = session.probe(&seg(50.0, 0.125), PcbLayer::FCu, "SIG", 0.2);
        assert!(r.legal, "no-components keepout must not block traces");
    }

    #[test]
    fn probe_enforces_board_edge_clearance() {
        // rules().edge_clearance is 0.5: a trace whose copper comes within
        // 0.5mm of the outline must probe illegal, matching DRC EdgeClearance.
        let session = RouteSession::from_pcb(&empty_pcb());
        // Centerline 0.4mm from the y=0 edge, half width 0.125 → copper edge
        // at 0.275mm from the outline: violation.
        let r = session.probe(&seg(0.4, 0.125), PcbLayer::FCu, "SIG", 0.2);
        assert!(
            !r.legal,
            "copper 0.275mm from the board edge must be blocked"
        );
        // Centerline 0.7mm from the edge → copper edge at 0.575mm: legal.
        let r = session.probe(&seg(0.7, 0.125), PcbLayer::FCu, "SIG", 0.2);
        assert!(r.legal, "copper 0.575mm from the board edge is legal");
    }

    #[test]
    fn net_tie_exempts_blocker() {
        let mut pcb = empty_pcb();
        pcb.traces.push(h_trace(50.0, "GND"));
        pcb.net_ties.push(NetTie {
            nets: vec!["GND".into(), "AGND".into()],
            position: None,
            radius: None,
        });
        let session = RouteSession::from_pcb(&pcb);
        // AGND is tied to GND board-wide, so an overlap is not a violation.
        let r = session.probe(&seg(50.0, 0.125), PcbLayer::FCu, "AGND", 0.2);
        assert!(r.legal, "net-tied copper must be exempt");
    }

    /// Drills are compared across layer spans and nets, because that is what
    /// the fab file says: two vias whose copper never shares a layer still
    /// collide in the drill. This is the one legality question the copper probe
    /// structurally cannot answer.
    #[test]
    fn drill_probe_sees_holes_no_copper_probe_could() {
        let mut pcb = empty_pcb();
        pcb.vias.push(vcad_ir::ecad::Via {
            position: Vec2::new(50.0, 50.0),
            diameter: 0.8,
            drill: 0.4,
            start_layer: PcbLayer::In1Cu,
            end_layer: PcbLayer::In2Cu,
            net: "GND".into(),
            source: None,
        });
        let mut session = RouteSession::from_pcb(&pcb);
        // hole_to_hole is 0.5 here: edge distance must be at least that.
        let touching = session.probe_drill(Vec2::new(50.4, 50.0), 0.4);
        assert!(
            !touching.legal && touching.min_spacing < 0.5,
            "a hole 0.4mm away (edge 0.0mm) must be illegal, got {touching:?}"
        );
        assert_eq!(touching.nearest, Some(Vec2::new(50.0, 50.0)));
        let clear = session.probe_drill(Vec2::new(51.5, 50.0), 0.4);
        assert!(
            clear.legal,
            "a hole 1.5mm away must be legal, got {clear:?}"
        );
        // A copper probe on a layer the existing via does not span sees nothing
        // there at all — hence the separate index.
        assert!(
            session
                .probe(
                    &CopperGeom::Disc {
                        center: Vec2::new(50.4, 50.0),
                        r: 0.4,
                    },
                    PcbLayer::In5Cu,
                    "SIG",
                    0.2,
                )
                .legal,
            "the copper probe cannot see a hole on an unshared layer"
        );
        // Newly committed drills join the index.
        session.commit_drill(Vec2::new(60.0, 60.0), 0.4);
        assert!(!session.probe_drill(Vec2::new(60.3, 60.0), 0.4).legal);
    }

    #[test]
    fn ripped_up_barrel_stops_blocking() {
        // A committed barrel takes a SpanId, so rip-up-and-reroute takes the
        // hole out with the copper. Without this the router would wall itself
        // in: every via it ever tried and abandoned would keep its ground.
        let mut session = RouteSession::from_pcb(&empty_pcb());
        let a = Vec2::new(50.0, 50.0);
        let id = session.commit_drill(a, 0.4);

        // 0.7mm centers → 0.3mm edge gap against a 0.5mm rule.
        let near = Vec2::new(50.7, 50.0);
        let r = session.probe_drill(near, 0.4);
        assert!(!r.legal, "0.3mm hole gap must violate the 0.5mm rule");
        assert!(
            (r.min_spacing - 0.3).abs() < 1e-9,
            "spacing was {}",
            r.min_spacing
        );
        // Exactly at the rule (0.9mm centers → 0.5mm gap): legal.
        assert!(session.probe_drill(Vec2::new(50.9, 50.0), 0.4).legal);

        assert!(session.remove(id));
        assert!(
            session.probe_drill(near, 0.4).legal,
            "a ripped-up barrel must stop blocking the ground it held"
        );
        assert!(!session.remove(id), "double-remove returns false");
    }

    #[test]
    fn drill_index_includes_through_hole_pads() {
        // A drilled pad is a hole like any other: the DRC compares vias
        // against pad drills, so the router's oracle must too.
        let mut pcb = empty_pcb();
        pcb.footprints.push(Footprint {
            reference: "J1".into(),
            value: "conn".into(),
            footprint_name: "TH".into(),
            position: Vec2::new(50.0, 50.0),
            rotation: 0.0,
            front: true,
            pads: vec![Pad {
                number: "1".into(),
                pad_type: PadType::THT,
                shape: PadShape::Circle { diameter: 1.6 },
                position: Vec2::new(0.0, 0.0),
                rotation: 0.0,
                drill: Some(DrillSpec {
                    diameter: 1.0,
                    oval: false,
                    oval_height: None,
                }),
                net: Some("VCC".into()),
                layers: vec![PcbLayer::FCu],
            }],
            graphics: vec![],
            model_3d: None,
            properties: Default::default(),
        });
        let session = RouteSession::from_pcb(&pcb);
        // 1.0mm from the pad center: gap = 1.0 - 0.5 - 0.2 = 0.3 < 0.5.
        assert!(!session.probe_drill(Vec2::new(51.0, 50.0), 0.4).legal);
        // 1.3mm away: gap = 0.6 — clear.
        assert!(session.probe_drill(Vec2::new(51.3, 50.0), 0.4).legal);
        // The pad's own net is irrelevant: a drill bit is mechanical, and the
        // DRC's hole census counts same-net pairs too.
        assert_eq!(
            session.probe_drill(Vec2::new(51.0, 50.0), 0.4).nearest,
            Some(Vec2::new(50.0, 50.0))
        );
    }

    #[test]
    fn ids_stay_stable_across_compaction() {
        let mut session = RouteSession::from_pcb(&empty_pcb());
        // Board-edge obstacle elements are part of the base count.
        let base = session.len();
        // Commit a row of well-separated spans.
        let ids: Vec<SpanId> = (0..20)
            .map(|i| {
                let y = i as f64 * 2.0 + 1.0;
                session.commit(CopperElement {
                    min: [10.0, y - 0.125],
                    max: [90.0, y + 0.125],
                    net: format!("N{i}"),
                    layer: PcbLayer::FCu,
                    geom: seg(y, 0.125),
                })
            })
            .collect();
        // Remove enough to force at least one compaction.
        for &id in ids.iter().take(15) {
            assert!(session.remove(id));
        }
        // A surviving span must still block a coincident other-net candidate,
        // proving its id/geometry survived compaction intact.
        let survivor_y = 19.0 * 2.0 + 1.0; // index 19
        let r = session.probe(&seg(survivor_y, 0.125), PcbLayer::FCu, "X", 0.2);
        assert!(
            !r.legal,
            "surviving span must still be probed after compaction"
        );
        assert_eq!(session.len(), base + 5);
    }
}
