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
//! This is the structural inversion the router rests on: clearance becomes a
//! constraint consulted *during* the search, not a violation detected after it.

use std::collections::HashMap;

use rstar::{RTree, RTreeObject, AABB};

use vcad_ir::ecad::{Pcb, PcbLayer};
use vcad_ir::Vec2;

use crate::drc::{build_net_clearance_map, build_net_trace_width_map, NetTieGroups};
use crate::spatial::{copper_elements, CopperElement, CopperGeom};

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

/// An incremental copper index for routing: probe, commit, rip-up.
pub struct RouteSession {
    tree: RTree<SessionElement>,
    /// Liveness by id; a tombstoned (removed) span is `false`.
    live: Vec<bool>,
    /// Count of tombstoned-but-not-yet-compacted spans.
    dead: usize,
    net_ties: NetTieGroups,
    net_clearance: HashMap<String, f64>,
    default_clearance: f64,
    /// Largest clearance any net requires — the broadphase must reach this far
    /// so a wide net's clearance is never missed when it exceeds the candidate's.
    max_clearance: f64,
    net_width: HashMap<String, f64>,
}

impl RouteSession {
    /// Build a session seeded with every copper element on `pcb`.
    pub fn from_pcb(pcb: &Pcb) -> Self {
        let elems = copper_elements(pcb);
        let live = vec![true; elems.len()];
        let session_elems: Vec<SessionElement> = elems
            .into_iter()
            .enumerate()
            .map(|(id, elem)| SessionElement { id, elem })
            .collect();
        let default_clearance = pcb.rules.default_rules.clearance;
        let net_clearance = build_net_clearance_map(pcb);
        let max_clearance = net_clearance
            .values()
            .copied()
            .fold(default_clearance, f64::max);
        Self {
            tree: RTree::bulk_load(session_elems),
            live,
            dead: 0,
            net_ties: NetTieGroups::from_pcb(pcb),
            net_clearance,
            default_clearance,
            max_clearance,
            net_width: build_net_trace_width_map(pcb),
        }
    }

    /// The required clearance for `net` from the design rules.
    pub fn clearance_for(&self, net: &str) -> f64 {
        self.net_clearance
            .get(net)
            .copied()
            .unwrap_or(self.default_clearance)
    }

    /// The trace width for `net` from its net class, or `fallback` if the net
    /// has no class width (so power/ground classes route wider than signals).
    pub fn width_for(&self, net: &str, fallback: f64) -> f64 {
        self.net_width.get(net).copied().unwrap_or(fallback)
    }

    /// Number of live (non-tombstoned) copper spans.
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
        self.dead += 1;
        if self.dead * 2 > self.tree.size() {
            self.compact();
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

    /// Probe a candidate geometry on `layer` belonging to `net` against all
    /// existing copper, requiring `clearance` mm to other nets.
    ///
    /// Mutates nothing. Same-net and net-tied copper is never a blocker — the
    /// exact rule the DRC clearance pass applies, so a span that probes legal
    /// here is legal in [`crate::check_drc`].
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
            let required = clearance.max(self.clearance_for(&e.net));
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

    #[test]
    fn ids_stay_stable_across_compaction() {
        let mut session = RouteSession::from_pcb(&empty_pcb());
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
        assert_eq!(session.len(), 5);
    }
}
