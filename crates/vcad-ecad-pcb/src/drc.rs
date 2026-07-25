//! Design Rule Checking (DRC) engine.
//!
//! Validates a PCB layout against its design rules and reports violations.
//! Uses the spatial index from [`crate::spatial`] for efficient proximity queries.

use std::collections::HashMap;

use vcad_ir::ecad::{Footprint, FootprintGraphic, Pad, PadShape, PadType, Pcb, PcbLayer};
use vcad_ir::Vec2;

#[cfg(feature = "gpu")]
use crate::spatial::CopperElement;
use crate::spatial::{
    pad_geom, point_in_polygon, segment_polygon_intersects, CopperGeom, SpatialIndex,
};

/// DRC rule type.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub enum DrcRuleType {
    /// Copper-to-copper clearance violation.
    Clearance,
    /// Trace width below minimum.
    MinTraceWidth,
    /// Drill diameter below minimum.
    MinDrill,
    /// Annular ring too narrow.
    AnnularRing,
    /// Copper too close to board edge.
    EdgeClearance,
    /// Hole-to-hole distance too small.
    HoleToHole,
    /// Net has unconnected terminals.
    UnconnectedNet,
    /// Silkscreen overlapping pads.
    SilkscreenClearance,
    /// Component courtyards overlapping.
    CourtyardOverlap,
    /// Acute angle copper creating acid trap.
    AcidTrap,
    /// Copper/via/component violates a keepout region.
    Keepout,
    /// Two distinct nets are electrically connected (a short).
    Short,
    /// A single net's realized copper forms more than one galvanically-isolated
    /// group (the schematic says one node; the board built several islands).
    NetIslands,
    /// An SMD pad names a copper-plane net but has no galvanic path to that
    /// plane — it needs a stitching via (or a dog-bone escape via on fine pitch).
    UnstitchedPad,
    /// Same-net copper contact far from any intended junction — a trace body
    /// touching copper of its own net that is many hops away along the
    /// conductor (e.g. a star trace overlapping a spiral coil's inner via).
    /// Invisible to every net-based rule, but it short-circuits the structure
    /// between the two points.
    SameNetBypass,
}

/// DRC violation severity.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub enum DrcSeverity {
    /// Must be fixed before fabrication.
    Error,
    /// Should be reviewed but may be acceptable.
    Warning,
}

/// Where a DRC violation originates — lets a caller separate synthesized
/// land-pattern artifacts from genuine layout faults instead of hand-triaging
/// the raw count. Serializes snake_case (`intra_footprint` / `inter_component`
/// / `routing`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DrcProvenance {
    /// Both elements lie within one footprint's land pattern (pad↔pad of a
    /// single footprint, or a pad's own drill / annular ring). On a generated
    /// footprint these are land-pattern artifacts, not board faults.
    IntraFootprint,
    /// The conflict is between two distinct placed components (cross-footprint
    /// pad clearance / hole-to-hole, or a courtyard overlap) — a placement
    /// fault.
    InterComponent,
    /// A trace, via, zone, board edge, keepout, or net connectivity is involved
    /// — board-level routing, not footprint geometry.
    Routing,
}

/// A DRC violation found during checking.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct DrcViolation {
    /// Which rule was violated.
    pub rule: DrcRuleType,
    /// Severity of the violation.
    pub severity: DrcSeverity,
    /// Location of the violation on the board (mm).
    pub position: Vec2,
    /// Human-readable description.
    pub message: String,
    /// Actual measured value (mm).
    pub actual: f64,
    /// Required value from design rules (mm).
    pub required: f64,
    /// Origin class — footprint-internal, between components, or routing.
    pub provenance: DrcProvenance,
    /// True when a generated (synthesized) footprint land pattern is involved.
    /// Lets callers discount footprint artifacts from the headline count.
    pub generated: bool,
}

/// True when a footprint's land pattern was synthesized by the parametric
/// engine (marked `padSource = "generated"` at placement) rather than authored
/// inline or imported. Drives the `generated` flag on violations it touches.
fn fp_is_generated(fp: &Footprint) -> bool {
    fp.properties
        .get("padSource")
        .map(|s| s == "generated")
        .unwrap_or(false)
}

/// Classify a footprint-involved conflict: the same ref on both sides is
/// internal to one land pattern; two different refs is a placement collision.
fn fp_pair_provenance(a_ref: &str, b_ref: &str) -> DrcProvenance {
    if a_ref == b_ref {
        DrcProvenance::IntraFootprint
    } else {
        DrcProvenance::InterComponent
    }
}

/// Axis-aligned region scoping an incremental DRC run.
///
/// A scoped run keeps only the elements whose copper bounds intersect this
/// region as *subjects* of the per-element and pairwise checks (clearance,
/// widths, drills, edge, hole-to-hole, keepouts, courtyards). Pairwise checks
/// fire when **either** party is in scope, so a subject in the region is still
/// judged against everything around it. Connectivity (shorts through copper,
/// unrouted nets, net islands, unstitched pads) is always board-global — a
/// local copper edit changes the electrical graph everywhere.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrcRegion {
    /// Minimum corner `[x, y]` (mm).
    pub min: [f64; 2],
    /// Maximum corner `[x, y]` (mm).
    pub max: [f64; 2],
}

impl DrcRegion {
    /// True when the AABB `(min, max)` intersects this region.
    fn hits(&self, min: [f64; 2], max: [f64; 2]) -> bool {
        min[0] <= self.max[0]
            && max[0] >= self.min[0]
            && min[1] <= self.max[1]
            && max[1] >= self.min[1]
    }

    /// True when a disc of radius `r` at `p` intersects this region.
    fn hits_point(&self, p: Vec2, r: f64) -> bool {
        self.hits([p.x - r, p.y - r], [p.x + r, p.y + r])
    }

    /// True when a stroked segment (half-width `r`) intersects this region.
    fn hits_segment(&self, a: Vec2, b: Vec2, r: f64) -> bool {
        self.hits(
            [a.x.min(b.x) - r, a.y.min(b.y) - r],
            [a.x.max(b.x) + r, a.y.max(b.y) + r],
        )
    }

    /// True when a copper geometry's bounds intersect this region.
    fn hits_geom(&self, geom: &CopperGeom) -> bool {
        let (min, max) = geom.bounds();
        self.hits(min, max)
    }
}

/// True when `region` is unset (full-board run) or the predicate matches it.
/// The subject filter every scoped check runs through.
fn in_scope(region: Option<&DrcRegion>, pred: impl Fn(&DrcRegion) -> bool) -> bool {
    region.is_none_or(pred)
}

/// Run all DRC checks on a PCB and return violations.
///
/// Checks performed:
/// - Copper clearance between different-net elements
/// - Minimum trace width
/// - Minimum drill diameter
/// - Edge clearance
/// - Hole-to-hole spacing
/// - Annular ring width
/// - Connectivity (shorts, unrouted nets, net islands, unstitched pads, and
///   same-net bypass contacts)
pub fn check_drc(pcb: &Pcb) -> Vec<DrcViolation> {
    check_drc_scoped(pcb, None)
}

/// Run DRC with the geometric checks scoped to `region` (see [`DrcRegion`]).
///
/// The incremental entry point for verify-on-write: a mutator that touched
/// copper only inside `region` gets a full-fidelity verdict for that copper
/// (every rule, judged against the whole board via the spatial index) without
/// paying for a board-wide clearance sweep. Connectivity checks still run
/// board-global — they are the only rules a local edit can violate remotely.
///
/// Two scoped runs over the same region on the same board are element-wise
/// comparable, so a before/after diff of this function isolates exactly what a
/// mutation inside the region changed.
pub fn check_drc_in_region(pcb: &Pcb, min: Vec2, max: Vec2) -> Vec<DrcViolation> {
    check_drc_scoped(
        pcb,
        Some(DrcRegion {
            min: [min.x.min(max.x), min.y.min(max.y)],
            max: [min.x.max(max.x), min.y.max(max.y)],
        }),
    )
}

/// Shared body of [`check_drc`] / [`check_drc_in_region`] — one rule set, with
/// an optional subject scope. Never duplicates rule logic per scope.
fn check_drc_scoped(pcb: &Pcb, region: Option<DrcRegion>) -> Vec<DrcViolation> {
    let mut violations = Vec::new();
    let index = SpatialIndex::from_pcb(pcb);
    let net_ties = NetTieGroups::from_pcb(pcb);
    let dp_map = build_diff_pair_gap_map(pcb);
    let region = region.as_ref();

    check_clearance(pcb, &index, &net_ties, &dp_map, region, &mut violations);
    check_pad_clearance(pcb, &net_ties, &dp_map, region, &mut violations);
    check_min_trace_width(pcb, region, &mut violations);
    check_min_drill(pcb, region, &mut violations);
    check_edge_clearance(pcb, region, &mut violations);
    check_hole_to_hole(pcb, region, &mut violations);
    check_annular_ring(pcb, region, &mut violations);
    check_keepout(pcb, region, &mut violations);
    check_courtyard_overlap(pcb, region, &mut violations);
    // Board-global by construction (a union-find over ALL copper): never scoped.
    check_connectivity(pcb, &net_ties, &mut violations);

    violations
}

/// World-space axis-aligned courtyard bounds for a footprint.
///
/// Prefers an explicit courtyard graphic (`FCrtYd`/`BCrtYd` rectangle) rotated
/// and translated into board coordinates; falls back to the pad bounding box
/// when a footprint carries no courtyard layer (e.g. legacy chip templates).
fn courtyard_bounds(fp: &Footprint) -> (Vec2, Vec2) {
    let rot = fp.rotation.to_radians();
    let (cos_r, sin_r) = (rot.cos(), rot.sin());
    let to_world = |p: Vec2| {
        Vec2::new(
            fp.position.x + p.x * cos_r - p.y * sin_r,
            fp.position.y + p.x * sin_r + p.y * cos_r,
        )
    };

    let mut min = Vec2::new(f64::MAX, f64::MAX);
    let mut max = Vec2::new(f64::MIN, f64::MIN);
    let mut found = false;
    for g in &fp.graphics {
        if let FootprintGraphic::Rect {
            start, end, layer, ..
        } = g
        {
            if !matches!(layer, PcbLayer::FCrtYd | PcbLayer::BCrtYd) {
                continue;
            }
            found = true;
            // All four (rotated) corners — the rect may be rotated off-axis.
            for corner in [
                Vec2::new(start.x, start.y),
                Vec2::new(end.x, start.y),
                Vec2::new(end.x, end.y),
                Vec2::new(start.x, end.y),
            ] {
                let w = to_world(corner);
                min.x = min.x.min(w.x);
                min.y = min.y.min(w.y);
                max.x = max.x.max(w.x);
                max.y = max.y.max(w.y);
            }
        }
    }
    if found {
        (min, max)
    } else {
        crate::geometry::footprint_bounds(fp)
    }
}

/// Flag components whose courtyards overlap on the same board side — a
/// placement collision. Courtyards are an assembly keep-clear envelope, so any
/// overlap is an error. Components on opposite sides never collide.
fn check_courtyard_overlap(
    pcb: &Pcb,
    region: Option<&DrcRegion>,
    violations: &mut Vec<DrcViolation>,
) {
    let boxes: Vec<(usize, (Vec2, Vec2))> = pcb
        .footprints
        .iter()
        .enumerate()
        .map(|(i, fp)| (i, courtyard_bounds(fp)))
        .collect();

    // Scoped runs: a footprint pair is a subject when either courtyard is in
    // the region.
    let in_region: Vec<bool> = boxes
        .iter()
        .map(|(_, (min, max))| in_scope(region, |r| r.hits([min.x, min.y], [max.x, max.y])))
        .collect();

    for a in 0..boxes.len() {
        for b in (a + 1)..boxes.len() {
            if !in_region[a] && !in_region[b] {
                continue;
            }
            let (ia, (amin, amax)) = &boxes[a];
            let (ib, (bmin, bmax)) = &boxes[b];
            let fa = &pcb.footprints[*ia];
            let fb = &pcb.footprints[*ib];
            if fa.front != fb.front {
                continue;
            }
            // AABB overlap = positive penetration on both axes.
            let pen_x = amax.x.min(bmax.x) - amin.x.max(bmin.x);
            let pen_y = amax.y.min(bmax.y) - amin.y.max(bmin.y);
            if pen_x > 1e-6 && pen_y > 1e-6 {
                let ox = pen_x.min(pen_y);
                let cx = (amin.x.max(bmin.x) + amax.x.min(bmax.x)) / 2.0;
                let cy = (amin.y.max(bmin.y) + amax.y.min(bmax.y)) / 2.0;
                violations.push(DrcViolation {
                    rule: DrcRuleType::CourtyardOverlap,
                    severity: DrcSeverity::Error,
                    position: Vec2::new(cx, cy),
                    message: format!(
                        "courtyards of {} and {} overlap",
                        fa.reference, fb.reference
                    ),
                    actual: ox.max(0.0),
                    required: 0.0,
                    provenance: DrcProvenance::InterComponent,
                    generated: fp_is_generated(fa) || fp_is_generated(fb),
                });
            }
        }
    }
}

// ============================================================================
// Net-tie grouping (intentional net junctions)
// ============================================================================

/// A single net-tie group with an optional spatial restriction.
#[derive(Clone)]
struct TieGroup {
    /// Nets joined by this tie.
    nets: Vec<String>,
    /// Optional `(center, radius²)` restricting where the exemption applies.
    region: Option<(Vec2, f64)>,
}

/// Resolved net-tie groups, used to treat intentionally-joined nets as a
/// single electrical node for short/clearance detection.
///
/// Built from [`vcad_ir::ecad::NetTie`]. Two nets are exempt from
/// short/clearance reporting when they appear together in any tie group; if a
/// group carries a `position`+`radius`, the exemption only holds inside that
/// region.
#[derive(Clone)]
pub(crate) struct NetTieGroups {
    groups: Vec<TieGroup>,
}

impl NetTieGroups {
    /// Build groups from a PCB's `net_ties`.
    pub(crate) fn from_pcb(pcb: &Pcb) -> Self {
        let groups = pcb
            .net_ties
            .iter()
            .filter(|t| t.nets.len() >= 2)
            .map(|t| {
                let region = match (t.position, t.radius) {
                    (Some(p), Some(r)) => Some((p, r * r)),
                    _ => None,
                };
                TieGroup {
                    nets: t.nets.clone(),
                    region,
                }
            })
            .collect();
        Self { groups }
    }

    /// Returns true if nets `a` and `b` are tied together (and, for
    /// region-scoped ties, the contact point lies within the region).
    pub(crate) fn exempt(&self, a: &str, b: &str, at: Vec2) -> bool {
        a == b || !self.covering_group_ids(a, b, at).is_empty()
    }

    /// Indices of tie groups that join `a` and `b` and whose region (if any)
    /// contains `at`. A board-wide group (no region) always covers.
    pub(crate) fn covering_group_ids(&self, a: &str, b: &str, at: Vec2) -> Vec<usize> {
        let mut ids = Vec::new();
        for (idx, g) in self.groups.iter().enumerate() {
            let joins = g.nets.iter().any(|n| n == a) && g.nets.iter().any(|n| n == b);
            if !joins {
                continue;
            }
            match g.region {
                None => ids.push(idx),
                Some((c, r2)) => {
                    let dx = at.x - c.x;
                    let dy = at.y - c.y;
                    if dx * dx + dy * dy <= r2 {
                        ids.push(idx);
                    }
                }
            }
        }
        ids
    }

    /// True if `net` belongs to any tie group whose region (if any) contains
    /// `at` — the point lies in a declared junction area involving the net.
    /// Used by same-net bypass detection: copper meeting inside a tie region
    /// (a star point, a neutral bar) is joined there by design.
    pub(crate) fn covers_net_at(&self, net: &str, at: Vec2) -> bool {
        self.groups.iter().any(|g| {
            g.nets.iter().any(|n| n == net)
                && match g.region {
                    None => true,
                    Some((c, r2)) => {
                        let dx = at.x - c.x;
                        let dy = at.y - c.y;
                        dx * dx + dy * dy <= r2
                    }
                }
        })
    }

    /// Returns true if nets `a` and `b` are tied board-wide (no region scope).
    /// Used by connectivity detection, where there is no single contact point.
    pub(crate) fn tied_board_wide(&self, a: &str, b: &str) -> bool {
        if a == b {
            return true;
        }
        self.groups.iter().any(|g| {
            g.region.is_none() && g.nets.iter().any(|n| n == a) && g.nets.iter().any(|n| n == b)
        })
    }
}

/// Pick a contact point between a trace and a candidate copper element: the
/// trace midpoint clamped toward the element's bbox center.
fn midpoint_of(start: Vec2, end: Vec2, elem: &crate::spatial::CopperElement) -> Vec2 {
    let ec = Vec2::new(
        (elem.min[0] + elem.max[0]) / 2.0,
        (elem.min[1] + elem.max[1]) / 2.0,
    );
    let tm = Vec2::new((start.x + end.x) / 2.0, (start.y + end.y) / 2.0);
    Vec2::new((tm.x + ec.x) / 2.0, (tm.y + ec.y) / 2.0)
}

/// Check copper-to-copper clearance between different-net elements.
fn check_clearance(
    pcb: &Pcb,
    index: &SpatialIndex,
    net_ties: &NetTieGroups,
    dp_map: &HashMap<(String, String), (f64, f64)>,
    region: Option<&DrcRegion>,
    violations: &mut Vec<DrcViolation>,
) {
    // GPU narrowphase prefilter (charter M2, `--features gpu`): evaluate all
    // broadphase pairs in one dispatch; pairs that provably clear (with an
    // f32-drift margin) skip the exact narrowphase below. Verdicts other
    // than "clears" always fall through to the exact oracle, so the filter
    // can only reduce work. Rect pads are not encoded — they take the exact
    // path unconditionally.
    #[cfg(feature = "gpu")]
    let gpu_skip: std::collections::HashSet<(usize, usize)> = {
        use vcad_kernel_gpu::narrowphase::{clears_batch, NarrowGeom, NarrowPair, DEFAULT_MARGIN};
        let net_clearance = build_net_clearance_map(pcb);
        let default_clearance = pcb.rules.default_rules.clearance;
        let mut keys: Vec<(usize, usize)> = Vec::new();
        let mut pairs: Vec<NarrowPair> = Vec::new();
        let encode = |g: &CopperGeom| -> Option<NarrowGeom> {
            match g {
                CopperGeom::Segment { a, b, half_w } => Some(NarrowGeom::Capsule {
                    a: [a.x as f32, a.y as f32],
                    b: [b.x as f32, b.y as f32],
                    r: *half_w as f32,
                }),
                CopperGeom::Disc { center, r } => Some(NarrowGeom::Disc {
                    c: [center.x as f32, center.y as f32],
                    r: *r as f32,
                }),
                _ => None,
            }
        };
        for (ti, trace) in pcb.traces.iter().enumerate() {
            let clearance = net_clearance
                .get(&trace.net)
                .copied()
                .unwrap_or(default_clearance);
            let half_w = trace.width / 2.0;
            let search_margin = clearance + half_w + 1.0;
            let nearby = index.query_region(
                [
                    trace.start.x.min(trace.end.x) - search_margin,
                    trace.start.y.min(trace.end.y) - search_margin,
                ],
                [
                    trace.start.x.max(trace.end.x) + search_margin,
                    trace.start.y.max(trace.end.y) + search_margin,
                ],
            );
            let tg = CopperGeom::Segment {
                a: trace.start,
                b: trace.end,
                half_w,
            };
            for elem in nearby {
                if elem.layer != trace.layer || elem.net == trace.net {
                    continue;
                }
                let (Some(a), Some(b)) = (encode(&tg), encode(&elem.geom)) else {
                    continue;
                };
                let required = pair_aware_clearance_w(
                    dp_map,
                    &trace.net,
                    &elem.net,
                    Some(trace.width),
                    match &elem.geom {
                        CopperGeom::Segment { half_w, .. } => Some(2.0 * half_w),
                        _ => None,
                    },
                    clearance.max(
                        net_clearance
                            .get(&elem.net)
                            .copied()
                            .unwrap_or(default_clearance),
                    ),
                );
                keys.push((ti, elem as *const CopperElement as usize));
                pairs.push(NarrowPair {
                    a,
                    b,
                    required: required as f32,
                });
            }
        }
        match clears_batch(&pairs, DEFAULT_MARGIN) {
            Ok(flags) => keys
                .into_iter()
                .zip(flags)
                .filter_map(|(k, clear)| clear.then_some(k))
                .collect(),
            Err(_) => Default::default(),
        }
    };

    let default_clearance = pcb.rules.default_rules.clearance;

    // Build net class clearance lookup
    let net_clearance = build_net_clearance_map(pcb);

    // Check each trace against nearby elements on the same layer
    for (trace_index, trace) in pcb.traces.iter().enumerate() {
        #[cfg(not(feature = "gpu"))]
        let _ = trace_index;
        if !in_scope(region, |r| {
            r.hits_segment(trace.start, trace.end, trace.width / 2.0)
        }) {
            continue;
        }
        let clearance = net_clearance
            .get(&trace.net)
            .copied()
            .unwrap_or(default_clearance);

        let half_w = trace.width / 2.0;
        let search_margin = clearance + half_w + 1.0; // extra margin for search
        let min_x = trace.start.x.min(trace.end.x) - search_margin;
        let min_y = trace.start.y.min(trace.end.y) - search_margin;
        let max_x = trace.start.x.max(trace.end.x) + search_margin;
        let max_y = trace.start.y.max(trace.end.y) + search_margin;

        let nearby = index.query_region([min_x, min_y], [max_x, max_y]);

        let trace_geom = CopperGeom::Segment {
            a: trace.start,
            b: trace.end,
            half_w,
        };

        for elem in nearby {
            if elem.layer != trace.layer {
                continue;
            }
            // Same net never shorts. Net-tied nets are treated as same-net
            // (subject to the tie's optional region).
            if elem.net == trace.net {
                continue;
            }

            // GPU-cleared pairs skip the exact narrowphase (prefilter).
            #[cfg(feature = "gpu")]
            if gpu_skip.contains(&(trace_index, elem as *const CopperElement as usize)) {
                continue;
            }
            // True copper-to-copper distance (narrowphase). The R-tree query
            // above is only a broadphase candidate filter.
            let dist = trace_geom.distance_to(&elem.geom);

            // Net-tie exemption: a deliberate junction between tied nets is
            // not a short. Use the contact point (midpoint of the two
            // elements) to test region-scoped ties.
            let contact = midpoint_of(trace.start, trace.end, elem);
            if net_ties.exempt(&trace.net, &elem.net, contact) {
                continue;
            }

            // A declared diff-pair partner only needs to clear by its gap, so
            // the intentional close coupling isn't flagged as a short.
            let elem_w = match &elem.geom {
                CopperGeom::Segment { half_w, .. } => Some(2.0 * half_w),
                _ => None,
            };
            let mut required = pair_aware_clearance_w(
                dp_map,
                &trace.net,
                &elem.net,
                Some(trace.width),
                elem_w,
                clearance,
            );
            // Pad-breakout exemption: within the escape region of either
            // net's own pads the legs MUST converge below the gap to reach
            // the land pattern — every ECAD DRC exempts this. The base
            // clearance still applies (a true short stays a short).
            if required > clearance && dist < required - 1e-6 {
                const BREAKOUT_MM: f64 = 1.5;
                let near_pad = pcb.footprints.iter().any(|fp| {
                    fp.pads.iter().any(|pad| {
                        matches!(&pad.net, Some(n) if n == &trace.net || n == &elem.net) && {
                            let pp = crate::geometry::pad_world_position(fp, pad);
                            let d0 = ((pp.x - trace.start.x).powi(2)
                                + (pp.y - trace.start.y).powi(2))
                            .sqrt();
                            let d1 = ((pp.x - trace.end.x).powi(2) + (pp.y - trace.end.y).powi(2))
                                .sqrt();
                            d0 <= BREAKOUT_MM || d1 <= BREAKOUT_MM
                        }
                    })
                });
                if near_pad {
                    required = clearance;
                }
            }
            if dist < required - 1e-6 {
                violations.push(DrcViolation {
                    rule: DrcRuleType::Clearance,
                    severity: DrcSeverity::Error,
                    position: contact,
                    message: format!(
                        "Clearance violation: trace net '{}' to net '{}': {:.3}mm < {:.3}mm",
                        trace.net, elem.net, dist, required
                    ),
                    actual: dist,
                    required,
                    provenance: DrcProvenance::Routing,
                    generated: false,
                });
            }
        }
    }
}

/// Check copper clearance between pads of different nets.
///
/// The trace pass covers trace↔copper pairs; this covers pad↔pad shorts
/// (overlapping footprints or stacked pads), which that pass never sees.
fn check_pad_clearance(
    pcb: &Pcb,
    net_ties: &NetTieGroups,
    dp_map: &HashMap<(String, String), (f64, f64)>,
    region: Option<&DrcRegion>,
    violations: &mut Vec<DrcViolation>,
) {
    let default_clearance = pcb.rules.default_rules.clearance;
    let net_clearance = build_net_clearance_map(pcb);

    struct PadBox<'a> {
        center: Vec2,
        geom: CopperGeom,
        net: &'a str,
        layers: &'a [vcad_ir::ecad::PcbLayer],
        fp_ref: &'a str,
        number: &'a str,
        generated: bool,
    }

    let mut boxes: Vec<PadBox> = Vec::new();
    for fp in &pcb.footprints {
        let generated = fp_is_generated(fp);
        for pad in &fp.pads {
            // Pads without a net can't short two nets together.
            let Some(net) = pad.net.as_deref() else {
                continue;
            };
            let center = crate::geometry::pad_world_position(fp, pad);
            let rot = (fp.rotation + pad.rotation).to_radians();
            boxes.push(PadBox {
                center,
                geom: pad_geom(pad, center, rot),
                net,
                layers: &pad.layers,
                fp_ref: &fp.reference,
                number: &pad.number,
                generated,
            });
        }
    }

    // Scoped runs: a pad pair is a subject when either pad is in the region.
    let in_region: Vec<bool> = boxes
        .iter()
        .map(|b| in_scope(region, |r| r.hits_geom(&b.geom)))
        .collect();

    for i in 0..boxes.len() {
        for j in (i + 1)..boxes.len() {
            if !in_region[i] && !in_region[j] {
                continue;
            }
            let (a, b) = (&boxes[i], &boxes[j]);
            if a.net == b.net {
                continue;
            }
            let share_copper = a
                .layers
                .iter()
                .any(|la| la.is_copper() && b.layers.contains(la));
            if !share_copper {
                continue;
            }

            let base = net_clearance
                .get(a.net)
                .copied()
                .unwrap_or(default_clearance)
                .max(
                    net_clearance
                        .get(b.net)
                        .copied()
                        .unwrap_or(default_clearance),
                );
            // Pads are the breakout region by definition: intra-pair copper
            // converges onto the land pattern, so pads need only the base
            // clearance from their partner net (never the full gap).
            let _ = dp_map;
            let clearance = base;

            // True copper-to-copper distance, respecting pad rotation.
            let dist = a.geom.distance_to(&b.geom);
            let pos = Vec2::new(
                (a.center.x + b.center.x) / 2.0,
                (a.center.y + b.center.y) / 2.0,
            );
            if net_ties.exempt(a.net, b.net, pos) {
                continue;
            }
            // Pads of one footprint are a qualified land pattern: their
            // copper-to-copper *spacing* is the footprint's concern, not board
            // DRC (KiCad exempts intra-footprint pad pairs likewise). This
            // removes the phantom adjacent-pin clearance violations that
            // dominate fine-pitch parts. A genuine *overlap* (dist ≈ 0, a
            // broken land pattern) still falls through and is flagged.
            if a.fp_ref == b.fp_ref && dist > 1e-6 {
                continue;
            }
            if dist < clearance - 1e-6 {
                violations.push(DrcViolation {
                    rule: DrcRuleType::Clearance,
                    severity: DrcSeverity::Error,
                    position: pos,
                    message: format!(
                        "Clearance violation: pad {}.{} net '{}' to pad {}.{} net '{}': {:.3}mm < {:.3}mm",
                        a.fp_ref, a.number, a.net, b.fp_ref, b.number, b.net, dist, clearance
                    ),
                    actual: dist,
                    required: clearance,
                    provenance: fp_pair_provenance(a.fp_ref, b.fp_ref),
                    generated: a.generated || b.generated,
                });
            }
        }
    }
}

/// Check that all traces meet the minimum trace width.
fn check_min_trace_width(
    pcb: &Pcb,
    region: Option<&DrcRegion>,
    violations: &mut Vec<DrcViolation>,
) {
    let net_width = build_net_trace_width_map(pcb);
    let default_width = pcb.rules.default_rules.trace_width;

    for trace in &pcb.traces {
        if !in_scope(region, |r| {
            r.hits_segment(trace.start, trace.end, trace.width / 2.0)
        }) {
            continue;
        }
        let min_width = net_width.get(&trace.net).copied().unwrap_or(default_width);

        if trace.width < min_width - 1e-6 {
            violations.push(DrcViolation {
                rule: DrcRuleType::MinTraceWidth,
                severity: DrcSeverity::Error,
                position: Vec2::new(
                    (trace.start.x + trace.end.x) / 2.0,
                    (trace.start.y + trace.end.y) / 2.0,
                ),
                message: format!(
                    "Trace width {:.3}mm below minimum {:.3}mm for net '{}'",
                    trace.width, min_width, trace.net
                ),
                actual: trace.width,
                required: min_width,
                provenance: DrcProvenance::Routing,
                generated: false,
            });
        }
    }
}

/// Check that all drills meet the minimum drill diameter.
fn check_min_drill(pcb: &Pcb, region: Option<&DrcRegion>, violations: &mut Vec<DrcViolation>) {
    let min_drill = pcb.rules.min_drill;

    // Check via drills
    for via in &pcb.vias {
        if !in_scope(region, |r| r.hits_point(via.position, via.diameter / 2.0)) {
            continue;
        }
        if via.drill < min_drill - 1e-6 {
            violations.push(DrcViolation {
                rule: DrcRuleType::MinDrill,
                severity: DrcSeverity::Error,
                position: via.position,
                message: format!(
                    "Via drill {:.3}mm below minimum {:.3}mm",
                    via.drill, min_drill
                ),
                actual: via.drill,
                required: min_drill,
                provenance: DrcProvenance::Routing,
                generated: false,
            });
        }
    }

    // Check pad drills
    for footprint in &pcb.footprints {
        let generated = fp_is_generated(footprint);
        for pad in &footprint.pads {
            if let Some(drill) = &pad.drill {
                if drill.diameter < min_drill - 1e-6 {
                    let abs_pos = crate::geometry::pad_world_position(footprint, pad);
                    if !in_scope(region, |r| r.hits_point(abs_pos, drill.diameter / 2.0)) {
                        continue;
                    }
                    violations.push(DrcViolation {
                        rule: DrcRuleType::MinDrill,
                        severity: DrcSeverity::Error,
                        position: abs_pos,
                        message: format!(
                            "Pad {} drill {:.3}mm below minimum {:.3}mm on {}",
                            pad.number, drill.diameter, min_drill, footprint.reference
                        ),
                        actual: drill.diameter,
                        required: min_drill,
                        provenance: DrcProvenance::IntraFootprint,
                        generated,
                    });
                }
            }
        }
    }
}

/// Check that all copper elements maintain edge clearance.
fn check_edge_clearance(pcb: &Pcb, region: Option<&DrcRegion>, violations: &mut Vec<DrcViolation>) {
    let edge_clearance = pcb.rules.edge_clearance;
    let outline = &pcb.outline.vertices;

    if outline.is_empty() {
        return;
    }

    // Check traces against board edges
    for trace in &pcb.traces {
        if !in_scope(region, |r| {
            r.hits_segment(trace.start, trace.end, trace.width / 2.0)
        }) {
            continue;
        }
        let mid = Vec2::new(
            (trace.start.x + trace.end.x) / 2.0,
            (trace.start.y + trace.end.y) / 2.0,
        );
        // Check start and end points
        for point in [&trace.start, &trace.end] {
            let dist = min_distance_to_polygon(point, outline);
            let effective_dist = dist - trace.width / 2.0;
            if effective_dist < edge_clearance - 1e-6 {
                violations.push(DrcViolation {
                    rule: DrcRuleType::EdgeClearance,
                    severity: DrcSeverity::Error,
                    position: mid,
                    message: format!(
                        "Trace net '{}' edge clearance {:.3}mm < {:.3}mm",
                        trace.net, effective_dist, edge_clearance
                    ),
                    actual: effective_dist,
                    required: edge_clearance,
                    provenance: DrcProvenance::Routing,
                    generated: false,
                });
                break; // one violation per trace
            }
        }
    }

    // Check vias against board edges
    for via in &pcb.vias {
        if !in_scope(region, |r| r.hits_point(via.position, via.diameter / 2.0)) {
            continue;
        }
        let dist = min_distance_to_polygon(&via.position, outline);
        let effective_dist = dist - via.diameter / 2.0;
        if effective_dist < edge_clearance - 1e-6 {
            violations.push(DrcViolation {
                rule: DrcRuleType::EdgeClearance,
                severity: DrcSeverity::Error,
                position: via.position,
                message: format!(
                    "Via net '{}' edge clearance {:.3}mm < {:.3}mm",
                    via.net, effective_dist, edge_clearance
                ),
                actual: effective_dist,
                required: edge_clearance,
                provenance: DrcProvenance::Routing,
                generated: false,
            });
        }
    }
}

/// Check hole-to-hole spacing.
fn check_hole_to_hole(pcb: &Pcb, region: Option<&DrcRegion>, violations: &mut Vec<DrcViolation>) {
    let min_spacing = pcb.rules.hole_to_hole;

    /// Where a drilled hole came from — drives the violation's provenance.
    struct Hole {
        pos: Vec2,
        radius: f64,
        /// `Some(footprint)` for a pad hole (with its generated flag); `None`
        /// for a via (routing).
        fp: Option<(usize, bool)>,
    }

    let mut holes: Vec<Hole> = Vec::new();

    for via in &pcb.vias {
        holes.push(Hole {
            pos: via.position,
            radius: via.drill / 2.0,
            fp: None,
        });
    }

    for (fi, footprint) in pcb.footprints.iter().enumerate() {
        let generated = fp_is_generated(footprint);
        for pad in &footprint.pads {
            if let Some(drill) = &pad.drill {
                let abs_pos = crate::geometry::pad_world_position(footprint, pad);
                holes.push(Hole {
                    pos: abs_pos,
                    radius: drill.diameter / 2.0,
                    fp: Some((fi, generated)),
                });
            }
        }
    }

    // Scoped runs: a hole pair is a subject when either hole is in the region.
    let in_region: Vec<bool> = holes
        .iter()
        .map(|h| in_scope(region, |r| r.hits_point(h.pos, h.radius)))
        .collect();

    // O(n^2) check — fine for typical PCB sizes; use spatial index for large boards
    for i in 0..holes.len() {
        for j in (i + 1)..holes.len() {
            if !in_region[i] && !in_region[j] {
                continue;
            }
            let dx = holes[i].pos.x - holes[j].pos.x;
            let dy = holes[i].pos.y - holes[j].pos.y;
            let center_dist = (dx * dx + dy * dy).sqrt();
            let edge_dist = center_dist - holes[i].radius - holes[j].radius;

            if edge_dist < min_spacing - 1e-6 {
                let mid = Vec2::new(
                    (holes[i].pos.x + holes[j].pos.x) / 2.0,
                    (holes[i].pos.y + holes[j].pos.y) / 2.0,
                );
                // A via on either side makes it a routing conflict; two pad
                // holes of one footprint are intra, of two footprints inter.
                let (provenance, generated) = match (holes[i].fp, holes[j].fp) {
                    (Some((a, ga)), Some((b, gb))) => (
                        if a == b {
                            DrcProvenance::IntraFootprint
                        } else {
                            DrcProvenance::InterComponent
                        },
                        ga || gb,
                    ),
                    _ => (DrcProvenance::Routing, false),
                };
                violations.push(DrcViolation {
                    rule: DrcRuleType::HoleToHole,
                    severity: DrcSeverity::Error,
                    position: mid,
                    message: format!(
                        "Hole-to-hole spacing {:.3}mm < {:.3}mm",
                        edge_dist, min_spacing
                    ),
                    actual: edge_dist,
                    required: min_spacing,
                    provenance,
                    generated,
                });
            }
        }
    }
}

/// Check annular ring width on through-hole pads and vias.
fn check_annular_ring(pcb: &Pcb, region: Option<&DrcRegion>, violations: &mut Vec<DrcViolation>) {
    let min_ring = pcb.rules.min_annular_ring;

    // Check vias
    for via in &pcb.vias {
        if !in_scope(region, |r| r.hits_point(via.position, via.diameter / 2.0)) {
            continue;
        }
        let ring = (via.diameter - via.drill) / 2.0;
        if ring < min_ring - 1e-6 {
            violations.push(DrcViolation {
                rule: DrcRuleType::AnnularRing,
                severity: DrcSeverity::Error,
                position: via.position,
                message: format!("Via annular ring {:.3}mm < {:.3}mm", ring, min_ring),
                actual: ring,
                required: min_ring,
                provenance: DrcProvenance::Routing,
                generated: false,
            });
        }
    }

    // Check THT pads
    for footprint in &pcb.footprints {
        let generated = fp_is_generated(footprint);
        for pad in &footprint.pads {
            if pad.pad_type != PadType::THT {
                continue;
            }
            if let Some(drill) = &pad.drill {
                let pad_min_dim = pad_min_dimension(pad);
                let ring = (pad_min_dim - drill.diameter) / 2.0;
                if ring < min_ring - 1e-6 {
                    let abs_pos = crate::geometry::pad_world_position(footprint, pad);
                    if !in_scope(region, |r| r.hits_point(abs_pos, pad_min_dim / 2.0)) {
                        continue;
                    }
                    violations.push(DrcViolation {
                        rule: DrcRuleType::AnnularRing,
                        severity: DrcSeverity::Error,
                        position: abs_pos,
                        message: format!(
                            "Pad {} on {} annular ring {:.3}mm < {:.3}mm",
                            pad.number, footprint.reference, ring, min_ring
                        ),
                        actual: ring,
                        required: min_ring,
                        provenance: DrcProvenance::IntraFootprint,
                        generated,
                    });
                }
            }
        }
    }
}

/// Enforce keepout regions: no-tracks / no-vias / no-components.
fn check_keepout(pcb: &Pcb, region: Option<&DrcRegion>, violations: &mut Vec<DrcViolation>) {
    for keepout in &pcb.keepouts {
        if keepout.outline.len() < 3 {
            continue;
        }
        let centroid = polygon_centroid(&keepout.outline);

        // no_tracks: any trace on a matching layer that touches the region.
        if keepout.no_tracks {
            for trace in &pcb.traces {
                if !keepout.layers.contains(&trace.layer) {
                    continue;
                }
                if !in_scope(region, |r| {
                    r.hits_segment(trace.start, trace.end, trace.width / 2.0)
                }) {
                    continue;
                }
                if segment_polygon_intersects(trace.start, trace.end, &keepout.outline) {
                    let pos = Vec2::new(
                        (trace.start.x + trace.end.x) / 2.0,
                        (trace.start.y + trace.end.y) / 2.0,
                    );
                    violations.push(DrcViolation {
                        rule: DrcRuleType::Keepout,
                        severity: DrcSeverity::Error,
                        position: pos,
                        message: format!(
                            "Keepout violation: trace net '{}' enters a no-tracks keepout",
                            trace.net
                        ),
                        actual: 0.0,
                        required: 0.0,
                        provenance: DrcProvenance::Routing,
                        generated: false,
                    });
                }
            }
        }

        // no_vias: any via whose layer span overlaps a keepout layer and whose
        // center lies inside the region.
        if keepout.no_vias {
            for via in &pcb.vias {
                let layer_match = keepout.layers.contains(&via.start_layer)
                    || keepout.layers.contains(&via.end_layer);
                if !layer_match {
                    continue;
                }
                if !in_scope(region, |r| r.hits_point(via.position, via.diameter / 2.0)) {
                    continue;
                }
                if point_in_polygon(via.position, &keepout.outline) {
                    violations.push(DrcViolation {
                        rule: DrcRuleType::Keepout,
                        severity: DrcSeverity::Error,
                        position: via.position,
                        message: format!(
                            "Keepout violation: via net '{}' inside a no-vias keepout",
                            via.net
                        ),
                        actual: 0.0,
                        required: 0.0,
                        provenance: DrcProvenance::Routing,
                        generated: false,
                    });
                }
            }
        }

        // no_components: any footprint whose pads or courtyard fall inside.
        if keepout.no_components {
            for fp in &pcb.footprints {
                if !in_scope(region, |r| {
                    let (min, max) = courtyard_bounds(fp);
                    r.hits([min.x, min.y], [max.x, max.y])
                }) {
                    continue;
                }
                let mut hit = false;
                let mut hit_pos = fp.position;
                for pad in &fp.pads {
                    let pad_pos = crate::geometry::pad_world_position(fp, pad);
                    if point_in_polygon(pad_pos, &keepout.outline) {
                        hit = true;
                        hit_pos = pad_pos;
                        break;
                    }
                }
                if !hit && point_in_polygon(fp.position, &keepout.outline) {
                    hit = true;
                    hit_pos = fp.position;
                }
                if hit {
                    violations.push(DrcViolation {
                        rule: DrcRuleType::Keepout,
                        severity: DrcSeverity::Error,
                        position: hit_pos,
                        message: format!(
                            "Keepout violation: component '{}' inside a no-components keepout",
                            fp.reference
                        ),
                        actual: 0.0,
                        required: 0.0,
                        // A real placement fault against a board region — not a
                        // land-pattern artifact, so never flagged `generated`.
                        provenance: DrcProvenance::Routing,
                        generated: false,
                    });
                }
            }
        }

        // Silence unused-variable warning when no sub-flags are set.
        let _ = centroid;
    }
}

/// Centroid of a polygon's vertices (simple average, used for messaging only).
fn polygon_centroid(poly: &[Vec2]) -> Vec2 {
    if poly.is_empty() {
        return Vec2::new(0.0, 0.0);
    }
    let n = poly.len() as f64;
    let sx: f64 = poly.iter().map(|p| p.x).sum();
    let sy: f64 = poly.iter().map(|p| p.y).sum();
    Vec2::new(sx / n, sy / n)
}

/// Get the minimum dimension of a pad (for annular ring calculation).
fn pad_min_dimension(pad: &Pad) -> f64 {
    match &pad.shape {
        PadShape::Circle { diameter } => *diameter,
        PadShape::Rect { width, height }
        | PadShape::Oval { width, height }
        | PadShape::RoundRect { width, height, .. } => width.min(*height),
        PadShape::Custom { vertices } => {
            // Approximate as bounding box minimum dimension
            if vertices.is_empty() {
                return 0.0;
            }
            let mut min_x = f64::MAX;
            let mut max_x = f64::MIN;
            let mut min_y = f64::MAX;
            let mut max_y = f64::MIN;
            for v in vertices {
                min_x = min_x.min(v.x);
                max_x = max_x.max(v.x);
                min_y = min_y.min(v.y);
                max_y = max_y.max(v.y);
            }
            (max_x - min_x).min(max_y - min_y)
        }
    }
}

// ============================================================================
// Connectivity flood-fill (shorts + unrouted nets)
// ============================================================================

/// All copper layers, ordered top → bottom, for via span enumeration.
const COPPER_STACK: [PcbLayer; 8] = [
    PcbLayer::FCu,
    PcbLayer::In1Cu,
    PcbLayer::In2Cu,
    PcbLayer::In3Cu,
    PcbLayer::In4Cu,
    PcbLayer::In5Cu,
    PcbLayer::In6Cu,
    PcbLayer::BCu,
];

/// Bitmask of copper layers between `start` and `end` (inclusive) in the
/// physical stack. Used so a via bridges every layer it spans.
fn copper_layer_span(start: PcbLayer, end: PcbLayer) -> u16 {
    let idx = |l: PcbLayer| COPPER_STACK.iter().position(|&c| c == l);
    match (idx(start), idx(end)) {
        (Some(a), Some(b)) => {
            let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
            let mut mask = 0u16;
            for bit in lo..=hi {
                mask |= 1 << bit;
            }
            mask
        }
        // Unknown layer — fall back to just the two endpoints.
        _ => single_layer_mask(start) | single_layer_mask(end),
    }
}

/// Bitmask for a single copper layer (0 for non-copper).
fn single_layer_mask(layer: PcbLayer) -> u16 {
    COPPER_STACK
        .iter()
        .position(|&c| c == layer)
        .map(|i| 1u16 << i)
        .unwrap_or(0)
}

/// Geometry of a connectivity node.
enum NodeGeom {
    /// Copper segment / disc / rect (trace, via, pad).
    Copper(CopperGeom),
    /// A zone copper pour as its *filled* rings — the poured outline minus the
    /// clearance voids around other-net copper (CCW outer + CW holes). A point
    /// is in the pour by the even-odd rule over the rings, so the plane connects
    /// to same-net copper it floods over and not to the cleared other-net copper
    /// sitting in its voids.
    Pour(PourRings),
}

/// A pour's filled rings with cached per-ring bounding boxes, so the even-odd
/// and distance narrowphases can skip the thousands of small void rings a
/// dense plane carries instead of scanning every vertex per query.
struct PourRings {
    rings: Vec<Vec<Vec2>>,
    /// Per-ring axis-aligned bbox `(min, max)`.
    bboxes: Vec<(Vec2, Vec2)>,
}

impl PourRings {
    fn new(rings: Vec<Vec<Vec2>>) -> Self {
        let bboxes = rings
            .iter()
            .map(|r| {
                let mut min = Vec2::new(f64::MAX, f64::MAX);
                let mut max = Vec2::new(f64::MIN, f64::MIN);
                for p in r {
                    min.x = min.x.min(p.x);
                    min.y = min.y.min(p.y);
                    max.x = max.x.max(p.x);
                    max.y = max.y.max(p.y);
                }
                (min, max)
            })
            .collect();
        Self { rings, bboxes }
    }

    /// Distance from a point to a ring's bbox (0 inside).
    fn bbox_dist_point(bb: &(Vec2, Vec2), p: Vec2) -> f64 {
        let dx = (bb.0.x - p.x).max(0.0).max(p.x - bb.1.x);
        let dy = (bb.0.y - p.y).max(0.0).max(p.y - bb.1.y);
        (dx * dx + dy * dy).sqrt()
    }

    /// Distance from a segment's bbox to a ring's bbox (0 on overlap).
    fn bbox_dist_segment(bb: &(Vec2, Vec2), a: Vec2, b: Vec2) -> f64 {
        let dx = (bb.0.x - a.x.max(b.x)).max(0.0).max(a.x.min(b.x) - bb.1.x);
        let dy = (bb.0.y - a.y.max(b.y)).max(0.0).max(a.y.min(b.y) - bb.1.y);
        (dx * dx + dy * dy).sqrt()
    }
}

/// Axis-aligned bounding box of a connectivity node's geometry (mm).
fn node_bounds(geom: &NodeGeom) -> ([f64; 2], [f64; 2]) {
    match geom {
        NodeGeom::Copper(g) => g.bounds(),
        NodeGeom::Pour(pour) => {
            let mut min = [f64::MAX, f64::MAX];
            let mut max = [f64::MIN, f64::MIN];
            for ring in &pour.rings {
                for p in ring {
                    min[0] = min[0].min(p.x);
                    min[1] = min[1].min(p.y);
                    max[0] = max[0].max(p.x);
                    max[1] = max[1].max(p.y);
                }
            }
            if min[0] > max[0] {
                (min, min)
            } else {
                (min, max)
            }
        }
    }
}

/// A piece of copper participating in connectivity analysis.
struct ConnNode {
    geom: NodeGeom,
    /// Copper layers this node occupies (bitmask over [`COPPER_STACK`]).
    layers: u16,
    /// Declared net label (empty string ⇒ no declared net).
    net: String,
    /// Whether this node is a pad (used for the unrouted netlist).
    pad: Option<(String, String)>, // (footprint ref, pad number)
    /// Representative position for violation messaging.
    pos: Vec2,
}

impl ConnNode {
    /// True if this node's copper physically touches/overlaps `other` and they
    /// share a copper layer.
    fn touches(&self, other: &ConnNode) -> bool {
        if self.layers & other.layers == 0 {
            return false;
        }
        // A copper pour is poured for a single net, and fabrication always
        // carves an anti-pad around every *other*-net via / pad / trace that
        // crosses it — a plane never galvanically connects to foreign-net
        // copper. Model that directly: a pour connects only to same-net copper.
        // Relying solely on the polygonized clearance hole is fragile — a
        // sub-clearance anti-pad around a small via can vanish in the boolean,
        // making a through-plane via read as inside the plane and cascade into
        // spurious N² net-pair shorts. (Genuine proximity to the plane is still
        // caught by the Clearance rule.)
        let a_pour = matches!(self.geom, NodeGeom::Pour(_));
        let b_pour = matches!(other.geom, NodeGeom::Pour(_));
        if (a_pour ^ b_pour) && self.net != other.net {
            return false;
        }
        node_geoms_touch(&self.geom, &other.geom)
    }
}

/// Touch threshold (mm). Copper within this gap is treated as connected, to
/// absorb floating-point seams where routed segments meet at endpoints.
const TOUCH_EPS: f64 = 1e-6;

/// True if two node geometries touch or overlap.
fn node_geoms_touch(a: &NodeGeom, b: &NodeGeom) -> bool {
    match (a, b) {
        (NodeGeom::Copper(ga), NodeGeom::Copper(gb)) => ga.distance_to(gb) <= TOUCH_EPS,
        (NodeGeom::Pour(rings), NodeGeom::Copper(g))
        | (NodeGeom::Copper(g), NodeGeom::Pour(rings)) => copper_touches_pour(g, rings),
        (NodeGeom::Pour(ra), NodeGeom::Pour(rb)) => pours_touch(ra, rb),
    }
}

/// Even-odd point-in-pour test over the filled rings. Rings whose bbox
/// excludes the point contribute an even (zero) crossing count, so they are
/// skipped outright.
fn point_in_pour(pour: &PourRings, p: Vec2) -> bool {
    pour.rings
        .iter()
        .zip(&pour.bboxes)
        .filter(|(r, bb)| {
            p.x >= bb.0.x
                && p.x <= bb.1.x
                && p.y >= bb.0.y
                && p.y <= bb.1.y
                && point_in_polygon(p, r)
        })
        .count()
        % 2
        == 1
}

/// Minimum distance from a point to the nearest ring edge, ignoring rings
/// whose bbox is already farther than `cutoff` (the caller's touch
/// threshold) — the result is only ever compared against `cutoff`.
fn min_dist_point_to_pour(p: Vec2, pour: &PourRings, cutoff: f64) -> f64 {
    let mut best = f64::MAX;
    for (r, bb) in pour.rings.iter().zip(&pour.bboxes) {
        if PourRings::bbox_dist_point(bb, p) > cutoff.min(best) {
            continue;
        }
        best = best.min(min_distance_to_polygon(&p, r));
    }
    best
}

/// True if a copper geom touches/overlaps a filled pour (even-odd over rings).
///
/// Copper that the plane floods over reads as inside (odd ring count); copper
/// sitting in a clearance void reads as outside (its hole adds an even count),
/// and is also a full `clearance` from the nearest void edge, so the proximity
/// check never false-connects it.
fn copper_touches_pour(g: &CopperGeom, pour: &PourRings) -> bool {
    match g {
        CopperGeom::Disc { center, r } => {
            point_in_pour(pour, *center)
                || min_dist_point_to_pour(*center, pour, *r + TOUCH_EPS) <= *r + TOUCH_EPS
        }
        CopperGeom::Segment { a, b, half_w } => {
            let mid = Vec2::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0);
            if point_in_pour(pour, *a) || point_in_pour(pour, *b) || point_in_pour(pour, mid) {
                return true;
            }
            min_dist_segment_to_pour(*a, *b, pour, *half_w + TOUCH_EPS) <= *half_w + TOUCH_EPS
        }
        CopperGeom::Rect { center, .. } => {
            if point_in_pour(pour, *center) {
                return true;
            }
            rect_corners(g).iter().any(|c| point_in_pour(pour, *c))
        }
    }
}

/// Minimum distance from a segment to the nearest ring edge, ignoring rings
/// whose bbox is already farther than `cutoff`.
fn min_dist_segment_to_pour(a: Vec2, b: Vec2, pour: &PourRings, cutoff: f64) -> f64 {
    let mut best = f64::MAX;
    for (r, bb) in pour.rings.iter().zip(&pour.bboxes) {
        if PourRings::bbox_dist_segment(bb, a, b) > cutoff.min(best) {
            continue;
        }
        best = best.min(min_dist_segment_to_polygon_edges(a, b, r));
    }
    best
}

/// Corners of a [`CopperGeom::Rect`] (empty otherwise).
fn rect_corners(g: &CopperGeom) -> [Vec2; 4] {
    match g {
        CopperGeom::Rect {
            center,
            half_w,
            half_h,
            rot,
        } => {
            let (s, c) = rot.sin_cos();
            let local = [
                (-half_w, -half_h),
                (*half_w, -half_h),
                (*half_w, *half_h),
                (-half_w, *half_h),
            ];
            let mut out = [Vec2::new(0.0, 0.0); 4];
            for (i, (lx, ly)) in local.iter().enumerate() {
                out[i] = Vec2::new(center.x + lx * c - ly * s, center.y + lx * s + ly * c);
            }
            out
        }
        _ => [Vec2::new(0.0, 0.0); 4],
    }
}

/// Min distance from a segment to any edge of a polygon.
fn min_dist_segment_to_polygon_edges(a: Vec2, b: Vec2, poly: &[Vec2]) -> f64 {
    let n = poly.len();
    if n < 2 {
        return f64::MAX;
    }
    let seg = CopperGeom::Segment { a, b, half_w: 0.0 };
    let mut min_d = f64::MAX;
    for i in 0..n {
        let e = CopperGeom::Segment {
            a: poly[i],
            b: poly[(i + 1) % n],
            half_w: 0.0,
        };
        let d = seg.distance_to(&e);
        if d < min_d {
            min_d = d;
        }
    }
    min_d
}

/// True if two filled pours touch/overlap (even-odd over each ring set).
fn pours_touch(a: &PourRings, b: &PourRings) -> bool {
    if a.rings.iter().flatten().any(|p| point_in_pour(b, *p))
        || b.rings.iter().flatten().any(|p| point_in_pour(a, *p))
    {
        return true;
    }
    // Any ring edge of one crossing a ring of the other (bbox-pruned).
    for ra in &a.rings {
        for i in 0..ra.len() {
            let (s, e) = (ra[i], ra[(i + 1) % ra.len()]);
            if b.rings.iter().zip(&b.bboxes).any(|(rb, bb)| {
                PourRings::bbox_dist_segment(bb, s, e) <= 0.0
                    && segment_polygon_intersects(s, e, rb)
            }) {
                return true;
            }
        }
    }
    false
}

/// Connectivity flood-fill: detects shorts (one component, ≥2 distinct nets),
/// unrouted nets (one net split across ≥2 components), stranded copper
/// islands, unstitched plane pads, and same-net bypass contacts.
fn check_connectivity(pcb: &Pcb, net_ties: &NetTieGroups, violations: &mut Vec<DrcViolation>) {
    // Union-find over geometric touch (same-layer, overlapping/touching) — the
    // same graph `analyze_net_continuity` reads, so DRC and the realized-plane
    // gates agree on what is connected.
    let (nodes, mut dsu, contacts) = build_connectivity_with_contacts(pcb);
    if nodes.is_empty() {
        return;
    }

    detect_shorts(&nodes, &mut dsu, net_ties, &contacts, violations);
    detect_unrouted(pcb, &nodes, &mut dsu, net_ties, violations);
    detect_net_islands(pcb, &nodes, &mut dsu, violations);
    detect_unstitched_pads(pcb, &nodes, &mut dsu, violations);
    detect_same_net_bypass(&nodes, net_ties, &contacts, violations);
}

/// Build connectivity nodes from all copper on the board.
fn build_conn_nodes(pcb: &Pcb) -> Vec<ConnNode> {
    let mut nodes = Vec::new();

    for trace in &pcb.traces {
        nodes.push(ConnNode {
            geom: NodeGeom::Copper(CopperGeom::Segment {
                a: trace.start,
                b: trace.end,
                half_w: trace.width / 2.0,
            }),
            layers: single_layer_mask(trace.layer),
            net: trace.net.clone(),
            pad: None,
            pos: Vec2::new(
                (trace.start.x + trace.end.x) / 2.0,
                (trace.start.y + trace.end.y) / 2.0,
            ),
        });
    }

    for via in &pcb.vias {
        nodes.push(ConnNode {
            geom: NodeGeom::Copper(CopperGeom::Disc {
                center: via.position,
                r: via.diameter / 2.0,
            }),
            layers: copper_layer_span(via.start_layer, via.end_layer),
            net: via.net.clone(),
            pad: None,
            pos: via.position,
        });
    }

    for fp in &pcb.footprints {
        for pad in &fp.pads {
            let center = crate::geometry::pad_world_position(fp, pad);
            let rot = (fp.rotation + pad.rotation).to_radians();
            let mut layers = 0u16;
            for &l in &pad.layers {
                layers |= single_layer_mask(l);
            }
            if layers == 0 {
                continue;
            }
            nodes.push(ConnNode {
                geom: NodeGeom::Copper(pad_geom(pad, center, rot)),
                layers,
                net: pad.net.clone().unwrap_or_default(),
                pad: Some((fp.reference.clone(), pad.number.clone())),
                pos: center,
            });
        }
    }

    // Pour each zone so connectivity sees the real filled copper (outline minus
    // clearance voids), not the raw rectangle — otherwise a ground plane reads
    // as touching every net it overlaps. `fill_zones` returns one result per
    // zone in order, so the index lines up with `pcb.zones`.
    let filled = crate::copper_pour::fill_zones(pcb);
    for (i, zone) in pcb.zones.iter().enumerate() {
        if zone.outline.len() < 3 {
            continue;
        }
        let rings = filled
            .get(i)
            .map(|f| f.polygons.clone())
            .unwrap_or_else(|| vec![zone.outline.clone()]);
        if rings.iter().all(|r| r.len() < 3) {
            continue;
        }
        // A pour can flood into several physically-disjoint pieces (clearance
        // voids carve it up). `fill_zones` emits each piece as a CCW outer ring
        // immediately followed by its CW holes, so each piece becomes its own
        // connectivity node — otherwise a plane that fractured into N islands
        // would read as a single connected node and hide that N-1 of them stitch
        // to nothing.
        let layer = single_layer_mask(zone.layer);
        for island in split_pour_islands(&rings) {
            let pos = island
                .first()
                .map(|outer| polygon_centroid(outer))
                .unwrap_or_else(|| polygon_centroid(&zone.outline));
            nodes.push(ConnNode {
                geom: NodeGeom::Pour(PourRings::new(island)),
                layers: layer,
                net: zone.net.clone(),
                pad: None,
                pos,
            });
        }
    }

    nodes
}

/// Signed area of a closed polygon (positive for counter-clockwise winding).
fn polygon_signed_area(ring: &[Vec2]) -> f64 {
    let n = ring.len();
    if n < 3 {
        return 0.0;
    }
    let mut a = 0.0;
    for i in 0..n {
        let p = ring[i];
        let q = ring[(i + 1) % n];
        a += p.x * q.y - q.x * p.y;
    }
    a / 2.0
}

/// Split a zone's filled rings into physically-disjoint copper islands.
///
/// [`crate::copper_pour::fill_zones`] emits copper as CCW outer rings (positive
/// signed area) each immediately followed by its CW clearance-void holes
/// (negative area). Each CCW outer therefore begins a new island, and the holes
/// that follow it belong to that island — so the returned ring sets keep the
/// even-odd fill semantics [`copper_touches_pour`] relies on. If no CCW outer is
/// found (an unexpected winding), the whole ring set is returned as one island
/// so a pour is never silently dropped from connectivity.
fn split_pour_islands(rings: &[Vec<Vec2>]) -> Vec<Vec<Vec<Vec2>>> {
    let mut islands: Vec<Vec<Vec<Vec2>>> = Vec::new();
    for ring in rings {
        if ring.len() < 3 {
            continue;
        }
        if polygon_signed_area(ring) >= 0.0 {
            islands.push(vec![ring.clone()]);
        } else if let Some(last) = islands.last_mut() {
            last.push(ring.clone());
        }
    }
    if islands.is_empty() {
        let all: Vec<Vec<Vec2>> = rings.iter().filter(|r| r.len() >= 3).cloned().collect();
        if !all.is_empty() {
            islands.push(all);
        }
    }
    islands
}

/// Emit a `Short` violation for unintentional cross-net copper contact.
///
/// Two passes:
///
/// 1. **Direct contacts.** Every geometric touch between differently-netted
///    copper is judged at its contact point: covered by a tie group (board-wide
///    or a scoped region containing the point) it is an intentional junction;
///    otherwise it is a short, reported where the copper actually meets. This
///    is what lets a region-scoped [`vcad_ir::ecad::NetTie`] do its job — a
///    star/neutral junction is exempt exactly there, while a stray crossing of
///    the same two nets elsewhere on the board still fires.
///
/// 2. **Indirect joins.** A connected component carrying two nets whose copper
///    never directly touches (they're joined through other copper) is still a
///    short — unless both nets are *anchored* into a common tie group within
///    that component, i.e. each has a tie-covered direct contact with a member
///    of the group. That is precisely a wye junction: PHA and PHB never touch,
///    but both touch the neutral inside the tie region.
fn detect_shorts(
    nodes: &[ConnNode],
    dsu: &mut Dsu,
    net_ties: &NetTieGroups,
    contacts: &[NodeContact],
    violations: &mut Vec<DrcViolation>,
) {
    let pair_key = |a: &str, b: &str| -> (String, String) {
        if a <= b {
            (a.to_string(), b.to_string())
        } else {
            (b.to_string(), a.to_string())
        }
    };

    let mut seen_pairs: std::collections::HashSet<(String, String)> = Default::default();
    // Pairs with ≥1 direct contact in a component — judged in pass 1, so the
    // component-level pass must not re-judge (and mis-report) them.
    let mut direct: std::collections::HashSet<(usize, (String, String))> = Default::default();
    // (component, net) → tie groups the net has a covered contact with.
    let mut anchored: HashMap<(usize, String), std::collections::HashSet<usize>> = HashMap::new();

    for c in contacts {
        let na = &nodes[c.i].net;
        let nb = &nodes[c.j].net;
        // Only cross-net contacts between declared nets are candidate shorts.
        if na.is_empty() || nb.is_empty() || na == nb {
            continue;
        }
        let root = dsu.find(c.i);
        direct.insert((root, pair_key(na, nb)));
        let covering = net_ties.covering_group_ids(na, nb, c.at);
        if covering.is_empty() {
            if seen_pairs.insert(pair_key(na, nb)) {
                violations.push(DrcViolation {
                    rule: DrcRuleType::Short,
                    severity: DrcSeverity::Error,
                    position: c.at,
                    message: format!("Short: nets '{}' and '{}' are connected by copper", na, nb),
                    actual: 0.0,
                    required: 0.0,
                    provenance: DrcProvenance::Routing,
                    generated: false,
                });
            }
        } else {
            for g in covering {
                anchored.entry((root, na.clone())).or_default().insert(g);
                anchored.entry((root, nb.clone())).or_default().insert(g);
            }
        }
    }

    // Gather declared nets per component, with a representative position.
    let mut comp_nets: HashMap<usize, Vec<(String, Vec2)>> = HashMap::new();
    for (i, node) in nodes.iter().enumerate() {
        if node.net.is_empty() {
            continue;
        }
        let root = dsu.find(i);
        let entry = comp_nets.entry(root).or_default();
        if !entry.iter().any(|(n, _)| n == &node.net) {
            entry.push((node.net.clone(), node.pos));
        }
    }

    for (root, nets) in &comp_nets {
        if nets.len() < 2 {
            continue;
        }
        for a in 0..nets.len() {
            for b in (a + 1)..nets.len() {
                let (na, pa) = &nets[a];
                let (nb, _pb) = &nets[b];
                if net_ties.tied_board_wide(na, nb) {
                    continue;
                }
                let key = pair_key(na, nb);
                if direct.contains(&(*root, key.clone())) {
                    continue; // judged at its contact points in pass 1
                }
                // Indirectly joined. Intentional iff some tie group joining
                // both nets anchors each of them in this component.
                let shared_anchor = match (
                    anchored.get(&(*root, na.clone())),
                    anchored.get(&(*root, nb.clone())),
                ) {
                    (Some(ga), Some(gb)) => ga.iter().any(|g| gb.contains(g)),
                    _ => false,
                };
                if shared_anchor {
                    continue;
                }
                if !seen_pairs.insert(key) {
                    continue;
                }
                violations.push(DrcViolation {
                    rule: DrcRuleType::Short,
                    severity: DrcSeverity::Error,
                    position: *pa,
                    message: format!("Short: nets '{}' and '{}' are connected by copper", na, nb),
                    actual: 0.0,
                    required: 0.0,
                    provenance: DrcProvenance::Routing,
                    generated: false,
                });
            }
        }
    }
}

/// Emit an `UnconnectedNet` violation for any declared net whose pads land in
/// ≥2 disjoint components.
fn detect_unrouted(
    pcb: &Pcb,
    nodes: &[ConnNode],
    dsu: &mut Dsu,
    net_ties: &NetTieGroups,
    violations: &mut Vec<DrcViolation>,
) {
    // net → set of component roots that contain a pad of that net.
    let mut net_comps: HashMap<String, Vec<(usize, Vec2)>> = HashMap::new();
    for (i, node) in nodes.iter().enumerate() {
        if node.pad.is_none() || node.net.is_empty() {
            continue;
        }
        let root = dsu.find(i);
        let entry = net_comps.entry(node.net.clone()).or_default();
        if !entry.iter().any(|(r, _)| *r == root) {
            entry.push((root, node.pos));
        }
    }

    // Stable iteration for deterministic output.
    let mut nets: Vec<&String> = net_comps.keys().collect();
    nets.sort();

    for net in nets {
        let comps = &net_comps[net];
        if comps.len() < 2 {
            continue;
        }
        // Net-tied nets merge their pads' components: if every component for
        // this net shares a tie group with another net's component that bridges
        // them, we still report based on this net's own pad components. (A tie
        // is an intentional junction, not a router substitute, so disjoint pads
        // of the *same* net remain unrouted.)
        let _ = net_ties;
        let pos = comps[0].1;
        violations.push(DrcViolation {
            rule: DrcRuleType::UnconnectedNet,
            severity: DrcSeverity::Error,
            position: pos,
            message: format!(
                "Unconnected net '{}': pads split across {} disjoint copper groups",
                net,
                comps.len()
            ),
            actual: comps.len() as f64,
            required: 1.0,
            provenance: DrcProvenance::Routing,
            generated: false,
        });
    }

    let _ = pcb;
}

/// Reconcile each net's intended single node against the copper actually built.
///
/// Walks every piece of copper carrying a declared net — traces, pads, vias and
/// each disjoint same-net zone island — and groups them by the connectivity
/// union-find (vias bridge layers, copper that touches merges). A net whose
/// realized copper includes a group that reaches **none of its pads** — stranded
/// copper, the canonical case being a poured power plane that stitched to
/// nothing and fractured into islands — is reported as `NetIslands` with the
/// total island count and the pads per island.
///
/// This is the check the flight-controller session lacked. [`detect_unrouted`]
/// (`UnconnectedNet`) already flags pads that aren't mutually routed — a routing
/// to-do — but it only counts pad-bearing groups, so a pad-less floating plane
/// is invisible to it. `NetIslands` is the complement: it fires only when a net
/// has more total copper islands than pad groups, i.e. some copper connects to
/// nothing. Firing only on stranded copper keeps it strictly additive to
/// `UnconnectedNet` (a plain unrouted net is not double-reported) so the real
/// defect isn't buried under one-per-net noise on a freshly-placed board.
fn detect_net_islands(
    pcb: &Pcb,
    nodes: &[ConnNode],
    dsu: &mut Dsu,
    violations: &mut Vec<DrcViolation>,
) {
    /// One galvanically-connected group of a net's copper.
    struct Island {
        root: usize,
        pads: Vec<String>,
        has_pad: bool,
        pos: Vec2,
    }

    // net id → its islands, in first-seen order for deterministic numbering.
    let mut by_net: HashMap<String, Vec<Island>> = HashMap::new();
    for (i, node) in nodes.iter().enumerate() {
        if node.net.is_empty() {
            continue;
        }
        let root = dsu.find(i);
        let islands = by_net.entry(node.net.clone()).or_default();
        let island = match islands.iter_mut().find(|isl| isl.root == root) {
            Some(isl) => isl,
            None => {
                islands.push(Island {
                    root,
                    pads: Vec::new(),
                    has_pad: false,
                    pos: node.pos,
                });
                islands.last_mut().expect("just pushed")
            }
        };
        if let Some((fp, pad)) = &node.pad {
            island.pads.push(format!("{fp}.{pad}"));
            island.has_pad = true;
        }
    }

    // Prefer the human-readable net name in the message, fall back to the id.
    let net_name: HashMap<&str, &str> = pcb
        .nets
        .iter()
        .map(|n| (n.id.as_str(), n.name.as_str()))
        .collect();

    // Deterministic output: nets sorted by id.
    let mut net_ids: Vec<&String> = by_net.keys().collect();
    net_ids.sort();

    for net_id in net_ids {
        let islands = &by_net[net_id];
        if islands.len() < 2 {
            continue;
        }
        // Only fire when some group reaches none of the net's pads — stranded
        // copper. A net whose every island carries a pad is "pads not all
        // routed together", which is `UnconnectedNet`'s job; flagging it here too
        // would double-report every unrouted net.
        let pad_groups = islands.iter().filter(|isl| isl.has_pad).count();
        if islands.len() <= pad_groups {
            continue;
        }
        let label = net_name
            .get(net_id.as_str())
            .copied()
            .filter(|n| !n.is_empty())
            .unwrap_or(net_id.as_str());
        let detail = islands
            .iter()
            .enumerate()
            .map(|(idx, isl)| describe_island(idx + 1, isl.has_pad, &isl.pads))
            .collect::<Vec<_>>()
            .join("; ");
        violations.push(DrcViolation {
            rule: DrcRuleType::NetIslands,
            severity: DrcSeverity::Error,
            position: islands[0].pos,
            // The lowercase `net '<label>'` token lets the MCP summary attribute
            // this violation to the right net when it buckets by net-pair.
            message: format!(
                "Disjoint net '{label}': realized as {} disjoint copper islands — {detail}",
                islands.len()
            ),
            actual: islands.len() as f64,
            required: 1.0,
            provenance: DrcProvenance::Routing,
            generated: false,
        });
    }
}

/// Compact, deterministic description of one net island for the violation
/// message (sorted pad list, capped, or a "copper only" note when pad-less).
fn describe_island(n: usize, has_pad: bool, pads: &[String]) -> String {
    if !has_pad {
        return format!("#{n} (copper only, no pads)");
    }
    let mut pads = pads.to_vec();
    pads.sort();
    pads.dedup();
    const CAP: usize = 8;
    if pads.len() > CAP {
        format!(
            "#{n} [{}, +{} more]",
            pads[..CAP].join(", "),
            pads.len() - CAP
        )
    } else {
        format!("#{n} [{}]", pads.join(", "))
    }
}

/// Emit an `UnstitchedPad` violation for any SMD pad that names a copper-plane
/// net but has no galvanic path to that plane.
///
/// An inner power/ground plane (a poured zone) only reaches an SMD pad on an
/// outer layer through a stitching via — pour two of them and an SMD `+3V3` pad
/// connects to *nothing* until a via bridges the layers. This is the first-class
/// signal for that: it names the exact pad and suggests where to drop the via,
/// rather than rolling the whole net up into one opaque `UnconnectedNet`.
///
/// Scope: SMD pads only (a THT pad's plated barrel already bridges to an inner
/// plane), whose net owns a plane on a layer the pad is *not* on (a pad already
/// on its plane layer floods straight in), and whose connectivity component does
/// not contain any same-net plane. Galvanic path is read straight off the same
/// union-find the short/unrouted checks use, so a stitching via that reaches the
/// plane clears the violation automatically.
fn detect_unstitched_pads(
    pcb: &Pcb,
    nodes: &[ConnNode],
    dsu: &mut Dsu,
    violations: &mut Vec<DrcViolation>,
) {
    // Plane layers each net owns (a declared zone with a real outline).
    let mut plane_layers: HashMap<String, u16> = HashMap::new();
    for zone in &pcb.zones {
        if zone.net.is_empty() || zone.outline.len() < 3 {
            continue;
        }
        *plane_layers.entry(zone.net.clone()).or_default() |= single_layer_mask(zone.layer);
    }
    if plane_layers.is_empty() {
        return;
    }

    // Component roots that hold a plane (pour) of each net.
    let mut plane_roots: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, node) in nodes.iter().enumerate() {
        if matches!(node.geom, NodeGeom::Pour(_)) && !node.net.is_empty() {
            let root = dsu.find(i);
            plane_roots.entry(node.net.clone()).or_default().push(root);
        }
    }

    // Map each pad node to its component root, keyed by (footprint ref, pad num).
    let mut pad_root: HashMap<(String, String), usize> = HashMap::new();
    for (i, node) in nodes.iter().enumerate() {
        if let Some(id) = &node.pad {
            pad_root.insert(id.clone(), dsu.find(i));
        }
    }

    let via_r = pcb.rules.default_rules.via_diameter / 2.0;
    let suggest_offset =
        via_r + pcb.rules.default_rules.clearance + pcb.rules.default_rules.trace_width / 2.0;

    for fp in &pcb.footprints {
        let (sin_r, cos_r) = fp.rotation.to_radians().sin_cos();
        for pad in &fp.pads {
            if pad.pad_type != PadType::SMD {
                continue;
            }
            let Some(net) = pad.net.as_ref().filter(|n| !n.is_empty()) else {
                continue;
            };
            let Some(&pl_mask) = plane_layers.get(net) else {
                continue;
            };
            // A pad already on one of its plane layers floods straight into the
            // plane — it needs no stitching via (a void/thermal gap there is a
            // different defect, caught by the pour fill / clearance checks).
            let pad_mask = pad
                .layers
                .iter()
                .fold(0u16, |m, &l| m | single_layer_mask(l));
            if pad_mask & pl_mask != 0 {
                continue;
            }
            // Galvanic path: is the pad's component one that holds this net's
            // plane? A stitching via makes it so; without one it is not.
            let Some(&root) = pad_root.get(&(fp.reference.clone(), pad.number.clone())) else {
                continue;
            };
            let stitched = plane_roots
                .get(net)
                .is_some_and(|roots| roots.contains(&root));
            if stitched {
                continue;
            }

            // Suggested escape: radially outward from the footprint origin (the
            // open side for a perimeter QFP/QFN pad). Falls back to +X for a pad
            // sitting on the origin.
            let off_x = pad.position.x * cos_r - pad.position.y * sin_r;
            let off_y = pad.position.x * sin_r + pad.position.y * cos_r;
            let mag = (off_x * off_x + off_y * off_y).sqrt();
            let (ex, ey) = if mag > 1e-9 {
                (off_x / mag, off_y / mag)
            } else {
                (1.0, 0.0)
            };
            let pad_pt = Vec2::new(fp.position.x + off_x, fp.position.y + off_y);

            violations.push(DrcViolation {
                rule: DrcRuleType::UnstitchedPad,
                severity: DrcSeverity::Error,
                position: pad_pt,
                message: format!(
                    "Unstitched pad {}.{} on plane net '{}': no via reaches the \
                     plane — drop a stitching via near the pad, escaping ~{:.2}mm \
                     along ({:.2}, {:.2}) if the pitch is too fine for an at-pad via",
                    fp.reference, pad.number, net, suggest_offset, ex, ey
                ),
                actual: 0.0,
                required: 1.0,
                provenance: DrcProvenance::Routing,
                generated: false,
            });
        }
    }
}

// ============================================================================
// Same-net bypass detection
// ============================================================================

/// Maximum intended-adjacency hops between two touching same-net elements
/// before the contact reads as a bypass. At or under the limit the touch is a
/// local artifact (a stitching via reached through its pour, a tight polyline
/// corner); over it the touch short-circuits real conductor length.
const BYPASS_HOP_LIMIT: usize = 3;

/// Classify a same-net copper touch as an *intended* junction (an adjacency
/// edge of the conductor graph) or a suspect body contact.
///
/// Intended junctions are the ways copper is deliberately joined:
/// - traces chained end-to-end (their capsule end caps overlap),
/// - a trace terminating on a via or pad (an endpoint lands on the copper),
/// - via↔via, via↔pad and pad↔pad overlaps (land-pattern geometry), and
/// - anything a same-net pour floods over (zone fills and stitching-via
///   arrays are the pour doing its job).
///
/// What's left — a trace *body* crossing same-net copper it doesn't terminate
/// on — is exactly the geometry that silently destroys a two-terminal
/// structure (a spiral coil, a shunt, a sense trace), and is judged by
/// conductor-graph distance in [`detect_same_net_bypass`].
fn is_intended_junction(a: &NodeGeom, b: &NodeGeom) -> bool {
    let (ga, gb) = match (a, b) {
        (NodeGeom::Pour(_), _) | (_, NodeGeom::Pour(_)) => return true,
        (NodeGeom::Copper(ga), NodeGeom::Copper(gb)) => (ga, gb),
    };
    let close = |p: Vec2, q: Vec2, tol: f64| {
        let dx = p.x - q.x;
        let dy = p.y - q.y;
        (dx * dx + dy * dy).sqrt() <= tol
    };
    match (ga, gb) {
        (
            CopperGeom::Segment {
                a: a1,
                b: b1,
                half_w: h1,
            },
            CopperGeom::Segment {
                a: a2,
                b: b2,
                half_w: h2,
            },
        ) => {
            let tol = h1 + h2 + TOUCH_EPS;
            close(*a1, *a2, tol)
                || close(*a1, *b2, tol)
                || close(*b1, *a2, tol)
                || close(*b1, *b2, tol)
        }
        (CopperGeom::Segment { a, b, half_w }, CopperGeom::Disc { center, r })
        | (CopperGeom::Disc { center, r }, CopperGeom::Segment { a, b, half_w }) => {
            let tol = half_w + r + TOUCH_EPS;
            close(*a, *center, tol) || close(*b, *center, tol)
        }
        (CopperGeom::Segment { a, b, half_w }, rect @ CopperGeom::Rect { .. })
        | (rect @ CopperGeom::Rect { .. }, CopperGeom::Segment { a, b, half_w }) => {
            let tol = half_w + TOUCH_EPS;
            rect.point_distance(*a) <= tol || rect.point_distance(*b) <= tol
        }
        // Overlapping vias/pads (via-in-pad, stacked land patterns) are
        // deliberate — only a trace body can wander somewhere it shouldn't.
        _ => true,
    }
}

/// Shortest hop count between two nodes over the intended-adjacency graph
/// (breadth-first), or `None` when no path exists.
fn adjacency_hops(adjacency: &[Vec<usize>], from: usize, to: usize) -> Option<usize> {
    if from == to {
        return Some(0);
    }
    let mut depth = vec![usize::MAX; adjacency.len()];
    depth[from] = 0;
    let mut queue = std::collections::VecDeque::from([from]);
    while let Some(n) = queue.pop_front() {
        for &m in &adjacency[n] {
            if depth[m] != usize::MAX {
                continue;
            }
            depth[m] = depth[n] + 1;
            if m == to {
                return Some(depth[m]);
            }
            queue.push_back(m);
        }
    }
    None
}

/// Emit a `SameNetBypass` warning for copper that touches its own net far from
/// any intended junction.
///
/// The field failure this encodes: `add_motor_winding`'s old star traces ran
/// exactly along a coil's terminal ray and overlapped the coil's inner via —
/// same net, so no clearance/short rule fired, but the contact electrically
/// shorted out the spiral (a third of the winding dead). Any two-terminal
/// copper structure is silently destroyed by same-net contact across its body.
///
/// Method: split each net's contact-graph edges into *intended junctions*
/// ([`is_intended_junction`] — end-to-end chaining, terminations, pour floods)
/// and *suspect body contacts*. A suspect whose two elements are more than
/// [`BYPASS_HOP_LIMIT`] hops apart along the intended-adjacency graph bridges
/// two distant regions of the conductor — a bypass. Warning severity: the
/// geometry is same-net, so some touches are legitimate; exemptions:
/// - pours and anything touching them (zone fills, stitching-via arrays on
///   pour nets reach each other through the pour within the hop limit),
/// - contacts inside a net-tie region involving the net (declared junctions),
/// - contacts joining otherwise-disconnected groups (a T-junction is the
///   net's link, not a bypass of conductor between the points).
fn detect_same_net_bypass(
    nodes: &[ConnNode],
    net_ties: &NetTieGroups,
    contacts: &[NodeContact],
    violations: &mut Vec<DrcViolation>,
) {
    let mut adjacency: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    let mut suspects: Vec<&NodeContact> = Vec::new();
    for c in contacts {
        let net = &nodes[c.i].net;
        if net.is_empty() || net != &nodes[c.j].net {
            continue;
        }
        if is_intended_junction(&nodes[c.i].geom, &nodes[c.j].geom) {
            adjacency[c.i].push(c.j);
            adjacency[c.j].push(c.i);
        } else {
            suspects.push(c);
        }
    }

    for c in suspects {
        let net = &nodes[c.i].net;
        // A net-tie region is a declared junction area (a winding's star
        // point, a neutral bar) — same-net contact there is by design.
        if net_ties.covers_net_at(net, c.at) {
            continue;
        }
        // Disconnected under intended adjacency: this contact is the piece
        // that joins the two groups (a T-junction or the net's only link),
        // not a bypass of the conductor between them.
        let Some(hops) = adjacency_hops(&adjacency, c.i, c.j) else {
            continue;
        };
        if hops <= BYPASS_HOP_LIMIT {
            continue;
        }
        let (pa, pb) = (nodes[c.i].pos, nodes[c.j].pos);
        violations.push(DrcViolation {
            rule: DrcRuleType::SameNetBypass,
            severity: DrcSeverity::Warning,
            position: c.at,
            message: format!(
                "Same-net bypass on net '{net}': copper at ({:.2}, {:.2}) touches copper at \
                 ({:.2}, {:.2}) that is {hops} conductor hops away — the contact \
                 short-circuits everything between them (fatal to a two-terminal \
                 structure like a spiral coil, shunt, or sense trace)",
                pa.x, pa.y, pb.x, pb.y
            ),
            actual: hops as f64,
            required: BYPASS_HOP_LIMIT as f64,
            provenance: DrcProvenance::Routing,
            generated: false,
        });
    }
}

/// A simple disjoint-set (union-find) with path compression + union by size.
struct Dsu {
    parent: Vec<usize>,
    size: Vec<usize>,
}

impl Dsu {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
            size: vec![1; n],
        }
    }

    fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]];
            x = self.parent[x];
        }
        x
    }

    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra == rb {
            return;
        }
        let (big, small) = if self.size[ra] >= self.size[rb] {
            (ra, rb)
        } else {
            (rb, ra)
        };
        self.parent[small] = big;
        self.size[big] += self.size[small];
    }
}

// ===========================================================================
// Net galvanic-continuity analysis (realized-copper verification)
// ===========================================================================

/// One galvanic island of a net's copper — a maximal set of the net's copper
/// that is electrically continuous (a single connected blob).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NetIsland {
    /// Pads of the net that land in this island.
    pub pad_count: usize,
    /// Total copper nodes (traces, vias, pads, pour fragments) in this island.
    pub node_count: usize,
    /// Representative position (mm) for locating the island on the board.
    pub position: Vec2,
}

/// Galvanic-continuity analysis for a single net's *realized* copper.
///
/// Built from the same union-find over geometric copper touch that DRC uses to
/// flag shorts and unrouted nets, so a verdict here is the same physical
/// connectivity DRC sees. This is the check that gates a power/PDN or impedance
/// PASS: a closed-form number is only meaningful if the copper it describes is
/// actually a single continuous conductor.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NetContinuity {
    /// The analyzed net.
    pub net: String,
    /// Number of disjoint galvanic islands the net's copper forms. `0` = the
    /// net has no realized copper; `1` = fully continuous; `≥2` = a split
    /// (electrically open) plane/trace.
    pub islands: usize,
    /// Total pads assigned to this net on the board.
    pub total_pads: usize,
    /// Pads landing in the largest island (the "main" plane/conductor).
    pub connected_pads: usize,
    /// `connected_pads / total_pads`, in `[0, 1]`. `1.0` when the net has no
    /// pads (nothing to strand).
    pub coverage: f64,
    /// Vias on this net — the stitching vias that bridge layers/islands.
    pub vias: usize,
    /// True when the net has at least one piece of realized copper.
    pub realized: bool,
    /// True when the net's copper forms exactly one galvanic island.
    pub continuous: bool,
    /// The largest island that is NOT the main plane — the biggest stranded
    /// chunk — when the net is split (`islands ≥ 2`); `None` otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worst_island: Option<NetIsland>,
}

/// Run the board-wide union-find over geometric copper touch (the same graph
/// [`check_connectivity`] builds) and return the nodes alongside it, so callers
/// can analyze one or many nets without rebuilding the connectivity.
fn build_connectivity(pcb: &Pcb) -> (Vec<ConnNode>, Dsu) {
    let (nodes, dsu, _) = build_connectivity_with_contacts(pcb);
    (nodes, dsu)
}

/// Per-trace and per-via keep flags for [`prune_dangling_copper`]: `false`
/// marks copper whose galvanic island holds no pad and no pour fragment —
/// electrically dead. Indices line up with `pcb.traces` and `pcb.vias`.
///
/// Split out from the pruner so a caller that owns only *part* of a board's
/// copper (the autorouter, judging the candidate board it is about to return)
/// can drop its own dead pieces without touching copper it did not place.
pub(crate) fn dangling_copper_mask(pcb: &Pcb) -> (Vec<bool>, Vec<bool>) {
    let (nodes, mut dsu) = build_connectivity(pcb);
    // Node order in build_conn_nodes: traces, then vias, then pads/pours.
    let n_traces = pcb.traces.len();
    let n_vias = pcb.vias.len();

    // Component roots that are anchored: hold a pad or a pour fragment.
    let mut anchored: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for (i, node) in nodes.iter().enumerate() {
        let is_anchor = node.pad.is_some() || matches!(node.geom, NodeGeom::Pour(_));
        if is_anchor {
            let root = dsu.find(i);
            anchored.insert(root);
        }
    }

    let keep_trace: Vec<bool> = (0..n_traces)
        .map(|i| anchored.contains(&dsu.find(i)))
        .collect();
    let keep_via: Vec<bool> = (0..n_vias)
        .map(|i| anchored.contains(&dsu.find(n_traces + i)))
        .collect();
    (keep_trace, keep_via)
}

/// Remove dangling copper: board-level traces and vias whose galvanic island
/// touches no pad and no pour fragment of their net — copper connected to
/// nothing, left behind by rip-up/restore cycles. Uses the same connectivity
/// model as DRC's island detection, so exactly the islands DRC reports as
/// "copper only, no pads" are removed. Returns `(traces_removed,
/// vias_removed)`.
///
/// Galvanic islands are disjoint by construction, so dropping a whole unanchored
/// island can never disconnect copper that stays — one pass reaches the fixpoint.
pub fn prune_dangling_copper(pcb: &mut Pcb) -> (usize, usize) {
    let (keep_trace, keep_via) = dangling_copper_mask(pcb);

    let mut ti = 0;
    pcb.traces.retain(|_| {
        let k = keep_trace[ti];
        ti += 1;
        k
    });
    let mut vi = 0;
    pcb.vias.retain(|_| {
        let k = keep_via[vi];
        vi += 1;
        k
    });
    (
        keep_trace.iter().filter(|k| !**k).count(),
        keep_via.iter().filter(|k| !**k).count(),
    )
}

/// A direct geometric touch between two connectivity nodes — one edge of the
/// contact graph — with the approximate location where the copper meets.
/// Cross-net edges are the candidate shorts (each judged against the net-tie
/// regions); same-net edges feed [`detect_same_net_bypass`], which needs the
/// contact graph rather than just the union.
struct NodeContact {
    /// Index of the first node.
    i: usize,
    /// Index of the second node.
    j: usize,
    /// Approximate contact location.
    at: Vec2,
}

/// [`build_connectivity`], additionally recording every copper contact so
/// short detection can judge each cross-net junction at the point where it
/// happens and bypass detection can walk the same-net contact graph.
fn build_connectivity_with_contacts(pcb: &Pcb) -> (Vec<ConnNode>, Dsu, Vec<NodeContact>) {
    let nodes = build_conn_nodes(pcb);
    let mut dsu = Dsu::new(nodes.len());
    let mut contacts = Vec::new();

    // Broadphase: two nodes whose (slightly inflated) bounding boxes are
    // disjoint can't touch, so bucket nodes into a uniform grid and only run
    // the narrowphase on pairs sharing a cell. On a dense imported board
    // (~12k copper nodes, pours with 10k-vertex rings) the old all-pairs loop
    // was ~80M narrowphase calls — most against pour polygons.
    let bboxes: Vec<([f64; 2], [f64; 2])> = nodes
        .iter()
        .map(|n| {
            let (mut min, mut max) = node_bounds(&n.geom);
            min[0] -= TOUCH_EPS;
            min[1] -= TOUCH_EPS;
            max[0] += TOUCH_EPS;
            max[1] += TOUCH_EPS;
            (min, max)
        })
        .collect();
    let mut gmin = [f64::MAX, f64::MAX];
    let mut gmax = [f64::MIN, f64::MIN];
    for (min, max) in &bboxes {
        gmin[0] = gmin[0].min(min[0]);
        gmin[1] = gmin[1].min(min[1]);
        gmax[0] = gmax[0].max(max[0]);
        gmax[1] = gmax[1].max(max[1]);
    }
    if nodes.is_empty() {
        return (nodes, dsu, contacts);
    }
    const GRID_RES: f64 = 128.0;
    let cell = ((gmax[0] - gmin[0]).max(gmax[1] - gmin[1]) / GRID_RES).max(1e-3);
    let cols = (((gmax[0] - gmin[0]) / cell) as usize).max(1) + 1;
    let rows = (((gmax[1] - gmin[1]) / cell) as usize).max(1) + 1;
    let cell_of = |x: f64, y: f64| -> (usize, usize) {
        (
            (((x - gmin[0]) / cell) as usize).min(cols - 1),
            (((y - gmin[1]) / cell) as usize).min(rows - 1),
        )
    };
    let mut buckets: Vec<Vec<u32>> = vec![Vec::new(); cols * rows];
    for (i, (min, max)) in bboxes.iter().enumerate() {
        let (c0, r0) = cell_of(min[0], min[1]);
        let (c1, r1) = cell_of(max[0], max[1]);
        for r in r0..=r1 {
            for c in c0..=c1 {
                buckets[r * cols + c].push(i as u32);
            }
        }
    }
    let mut seen = vec![u32::MAX; nodes.len()];
    for (i, (min, max)) in bboxes.iter().enumerate() {
        let (c0, r0) = cell_of(min[0], min[1]);
        let (c1, r1) = cell_of(max[0], max[1]);
        for r in r0..=r1 {
            for c in c0..=c1 {
                for &j32 in &buckets[r * cols + c] {
                    let j = j32 as usize;
                    if j <= i || seen[j] == i as u32 {
                        continue;
                    }
                    seen[j] = i as u32;
                    let (jmin, jmax) = &bboxes[j];
                    if min[0] > jmax[0] || jmin[0] > max[0] || min[1] > jmax[1] || jmin[1] > max[1]
                    {
                        continue;
                    }
                    if nodes[i].touches(&nodes[j]) {
                        dsu.union(i, j);
                        contacts.push(NodeContact {
                            i,
                            j,
                            at: contact_point(&nodes[i].geom, &nodes[j].geom),
                        });
                    }
                }
            }
        }
    }
    (nodes, dsu, contacts)
}

/// Closest point on segment `ab` to point `p`.
fn closest_point_on_segment(p: Vec2, a: Vec2, b: Vec2) -> Vec2 {
    let abx = b.x - a.x;
    let aby = b.y - a.y;
    let len2 = abx * abx + aby * aby;
    if len2 <= f64::EPSILON {
        return a;
    }
    let t = (((p.x - a.x) * abx + (p.y - a.y) * aby) / len2).clamp(0.0, 1.0);
    Vec2::new(a.x + t * abx, a.y + t * aby)
}

/// Closest pair of points between segments `p1p2` and `p3p4` (centerlines).
fn segment_closest_points(p1: Vec2, p2: Vec2, p3: Vec2, p4: Vec2) -> (Vec2, Vec2) {
    // Candidate pairs from each endpoint projected onto the other segment;
    // for touching/overlapping copper this lands inside the overlap, which is
    // all the tie-region check needs.
    let candidates = [
        (closest_point_on_segment(p3, p1, p2), p3),
        (closest_point_on_segment(p4, p1, p2), p4),
        (p1, closest_point_on_segment(p1, p3, p4)),
        (p2, closest_point_on_segment(p2, p3, p4)),
    ];
    let mut best = candidates[0];
    let mut best_d2 = f64::MAX;
    for (a, b) in candidates {
        let dx = a.x - b.x;
        let dy = a.y - b.y;
        let d2 = dx * dx + dy * dy;
        if d2 < best_d2 {
            best_d2 = d2;
            best = (a, b);
        }
    }
    best
}

/// Representative point of a copper geometry.
fn copper_rep_point(g: &CopperGeom) -> Vec2 {
    match g {
        CopperGeom::Segment { a, b, .. } => Vec2::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0),
        CopperGeom::Disc { center, .. } | CopperGeom::Rect { center, .. } => *center,
    }
}

/// Approximate location where two touching node geometries meet.
///
/// Used to judge region-scoped net ties. Tie radii are sized generously by the
/// tools that write them (millimetres of slack around the junction), so a
/// nearby representative point is sufficient — exact overlap geometry is not.
fn contact_point(a: &NodeGeom, b: &NodeGeom) -> Vec2 {
    match (a, b) {
        (NodeGeom::Copper(ga), NodeGeom::Copper(gb)) => match (ga, gb) {
            (
                CopperGeom::Segment { a: a1, b: b1, .. },
                CopperGeom::Segment { a: a2, b: b2, .. },
            ) => {
                let (p, q) = segment_closest_points(*a1, *b1, *a2, *b2);
                Vec2::new((p.x + q.x) / 2.0, (p.y + q.y) / 2.0)
            }
            (CopperGeom::Segment { a, b, .. }, CopperGeom::Disc { center, .. })
            | (CopperGeom::Disc { center, .. }, CopperGeom::Segment { a, b, .. })
            | (CopperGeom::Segment { a, b, .. }, CopperGeom::Rect { center, .. })
            | (CopperGeom::Rect { center, .. }, CopperGeom::Segment { a, b, .. }) => {
                closest_point_on_segment(*center, *a, *b)
            }
            _ => {
                let p = copper_rep_point(ga);
                let q = copper_rep_point(gb);
                Vec2::new((p.x + q.x) / 2.0, (p.y + q.y) / 2.0)
            }
        },
        (NodeGeom::Pour(_), NodeGeom::Copper(g)) | (NodeGeom::Copper(g), NodeGeom::Pour(_)) => {
            copper_rep_point(g)
        }
        (NodeGeom::Pour(ra), NodeGeom::Pour(rb)) => {
            let p = ra
                .rings
                .first()
                .map(|r| polygon_centroid(r))
                .unwrap_or(Vec2::new(0.0, 0.0));
            let q = rb
                .rings
                .first()
                .map(|r| polygon_centroid(r))
                .unwrap_or(Vec2::new(0.0, 0.0));
            Vec2::new((p.x + q.x) / 2.0, (p.y + q.y) / 2.0)
        }
    }
}

/// Compute one net's continuity from a pre-built connectivity graph.
fn continuity_of(pcb: &Pcb, nodes: &[ConnNode], dsu: &mut Dsu, net: &str) -> NetContinuity {
    // Group this net's copper nodes by union-find root: (pads, nodes, pos).
    let mut groups: HashMap<usize, (usize, usize, Vec2)> = HashMap::new();
    for (i, node) in nodes.iter().enumerate() {
        if node.net != net {
            continue;
        }
        let root = dsu.find(i);
        let entry = groups.entry(root).or_insert((0, 0, node.pos));
        entry.1 += 1;
        if node.pad.is_some() {
            entry.0 += 1;
        }
    }

    // Stitching vias come straight from the net's via list — `build_conn_nodes`
    // doesn't tag a node as a via, and a via is exactly a `pcb.vias` entry.
    let vias = pcb.vias.iter().filter(|v| v.net == net).count();
    let total_pads = pcb
        .footprints
        .iter()
        .flat_map(|f| &f.pads)
        .filter(|p| p.net.as_deref() == Some(net))
        .count();

    let islands = groups.len();
    // Rank islands by pad count (then node count): the main plane is the one
    // most loads connect to; the worst island is the largest stranded chunk.
    let mut ranked: Vec<(usize, usize, Vec2)> = groups.into_values().collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));

    let connected_pads = ranked.first().map(|g| g.0).unwrap_or(0);
    let coverage = if total_pads == 0 {
        1.0
    } else {
        connected_pads as f64 / total_pads as f64
    };
    let worst_island = if islands >= 2 {
        ranked
            .get(1)
            .map(|&(pad_count, node_count, position)| NetIsland {
                pad_count,
                node_count,
                position,
            })
    } else {
        None
    };

    NetContinuity {
        net: net.to_string(),
        islands,
        total_pads,
        connected_pads,
        coverage,
        vias,
        realized: islands >= 1,
        continuous: islands == 1,
        worst_island,
    }
}

/// Analyze the galvanic continuity of one net's realized copper.
///
/// The verification leaf behind the power/PDN and impedance PASS-gates: returns
/// how many disjoint islands the net's copper forms, what fraction of its pads
/// reach the main plane, its stitching-via count, and the worst stranded
/// island. `continuous` is the single bit those gates key off.
pub fn analyze_net_continuity(pcb: &Pcb, net: &str) -> NetContinuity {
    let (nodes, mut dsu) = build_connectivity(pcb);
    continuity_of(pcb, &nodes, &mut dsu, net)
}

/// True if a net name looks like a power/ground rail — the nets whose copper is
/// expected to be a continuous plane, so a [`build_receipt`](crate) verdict
/// should check their realized continuity. Conservative + case-insensitive:
/// well-known rail names, `V…`/`…V…` voltage tags (`+3V3`, `5V0`, `1V8`), and
/// `GND`-family grounds. Word separators are folded away first, so the same
/// rail spelled `V_SUPPLY`, `V-SUPPLY` or `VSUPPLY` reads the same — a motor
/// controller's battery input is often the highest-current net on the board and
/// was being missed purely on punctuation.
pub fn is_power_net(name: &str) -> bool {
    let n = name.trim().to_ascii_uppercase();
    let core: String = n
        .trim_start_matches(['+', '-'])
        .chars()
        .filter(|c| !matches!(c, '_' | '-' | '.' | ' '))
        .collect();
    let core = core.as_str();
    const RAILS: &[&str] = &[
        "GND", "GROUND", "EARTH", "VSS", "VEE", "AGND", "DGND", "PGND", "SGND", "VCC", "VDD",
        "VBAT", "VBUS", "VIN", "VOUT", "VREF", "VPP", "AVDD", "DVDD", "AVCC", "DVCC", "VDDA",
        "VSSA", "VDDIO", "PWR", "POWER", "VSYS", "VRAW", "VSUPPLY", "VMOT", "VMOTOR", "B+",
    ];
    if RAILS.iter().any(|r| core == *r || core.starts_with(r)) {
        return true;
    }
    // Voltage tags: a digit adjacent to a 'V' (3V3, 5V, 1V8, 12V, 3.3V).
    let bytes = core.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'V' {
            let before = i > 0 && bytes[i - 1].is_ascii_digit();
            let after = i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit();
            if before || after {
                return true;
            }
        }
    }
    false
}

/// Continuity for every power-integrity-relevant net on the board: any net that
/// is poured as a plane (has a zone) or whose name reads as a power/ground rail.
/// Builds the connectivity graph once and analyzes each relevant net against it.
pub fn analyze_power_integrity(pcb: &Pcb) -> Vec<NetContinuity> {
    use std::collections::BTreeSet;

    // Net names are stored verbatim on copper (id == name in vcad), so a
    // declared `Net`'s name is also its copper key.
    let mut relevant: BTreeSet<String> = BTreeSet::new();
    for z in &pcb.zones {
        if !z.net.is_empty() {
            relevant.insert(z.net.clone());
        }
    }
    for net in &pcb.nets {
        if is_power_net(&net.name) {
            relevant.insert(net.id.clone());
        }
    }
    if relevant.is_empty() {
        return Vec::new();
    }

    let (nodes, mut dsu) = build_connectivity(pcb);
    relevant
        .into_iter()
        .map(|net| continuity_of(pcb, &nodes, &mut dsu, &net))
        .collect()
}

/// Build a map of net ID to clearance from design rules.
pub(crate) fn build_net_clearance_map(pcb: &Pcb) -> HashMap<String, f64> {
    let mut map = HashMap::new();
    for rule in &pcb.rules.class_rules {
        if let Some(nets) = pcb.rules.net_class_assignments.get(&rule.name) {
            for net_id in nets {
                map.insert(net_id.clone(), rule.clearance);
            }
        }
    }
    map
}

/// Build a map of net ID to minimum trace width from design rules.
pub(crate) fn build_net_trace_width_map(pcb: &Pcb) -> HashMap<String, f64> {
    let mut map = HashMap::new();
    for rule in &pcb.rules.class_rules {
        if let Some(nets) = pcb.rules.net_class_assignments.get(&rule.name) {
            for net_id in nets {
                map.insert(net_id.clone(), rule.trace_width);
            }
        }
    }
    map
}

/// A declared differential pair: two nets in a diff-pair net class matched by
/// base name (`FOO_P`/`FOO_N`, `FOO+`/`FOO-`).
pub(crate) struct DiffPair {
    /// Positive-polarity net.
    pub net_p: String,
    /// Negative-polarity net.
    pub net_n: String,
    /// Target gap between the two traces (mm).
    pub gap: f64,
    /// Trace width for each leg (mm).
    pub width: f64,
}

/// Strip a polarity suffix, returning `(base, is_positive)`.
fn split_polarity(net: &str) -> Option<(String, bool)> {
    for (suf, pos) in [
        ("_P", true),
        ("_N", false),
        ("_p", true),
        ("_n", false),
        ("+", true),
        ("-", false),
    ] {
        if let Some(base) = net.strip_suffix(suf) {
            if !base.is_empty() {
                return Some((base.to_string(), pos));
            }
        }
    }
    None
}

/// Find every differential pair declared on the board: nets assigned to a class
/// that carries `diff_pair_gap`, matched into +/- pairs by base name.
pub(crate) fn diff_pairs(pcb: &Pcb) -> Vec<DiffPair> {
    let mut pairs = Vec::new();
    for class in &pcb.rules.class_rules {
        let Some(gap) = class.diff_pair_gap else {
            continue;
        };
        let width = class.diff_pair_width.unwrap_or(class.trace_width);
        let Some(nets) = pcb.rules.net_class_assignments.get(&class.name) else {
            continue;
        };
        let mut bases: std::collections::BTreeMap<String, (Option<String>, Option<String>)> =
            Default::default();
        for net in nets {
            if let Some((base, pos)) = split_polarity(net) {
                let e = bases.entry(base).or_default();
                if pos {
                    e.0 = Some(net.clone());
                } else {
                    e.1 = Some(net.clone());
                }
            }
        }
        for (_, (p, n)) in bases {
            if let (Some(net_p), Some(net_n)) = (p, n) {
                pairs.push(DiffPair {
                    net_p,
                    net_n,
                    gap,
                    width,
                });
            }
        }
    }
    pairs
}

/// Map an unordered net pair to its differential-pair gap (its required
/// clearance), so the clearance passes let a pair couple to its declared gap
/// instead of false-flagging the intentional close spacing as a short.
fn build_diff_pair_gap_map(pcb: &Pcb) -> HashMap<(String, String), (f64, f64)> {
    let mut map = HashMap::new();
    for dp in diff_pairs(pcb) {
        let key = if dp.net_p <= dp.net_n {
            (dp.net_p.clone(), dp.net_n.clone())
        } else {
            (dp.net_n.clone(), dp.net_p.clone())
        };
        map.insert(key, (dp.gap, dp.width));
    }
    map
}

/// The required clearance between two nets: their diff-pair gap if they are a
/// declared pair, else `fallback`.
/// Width-aware variant: the pair-gap requirement binds the COUPLED section —
/// both elements at the pair's leg width. A neck/breakout connector (thinner
/// than the leg) is the uncoupled entry by definition and needs only the
/// base clearance, exactly as commercial DRC treats uncoupled length.
fn pair_aware_clearance_w(
    dp_map: &HashMap<(String, String), (f64, f64)>,
    a: &str,
    b: &str,
    w_a: Option<f64>,
    w_b: Option<f64>,
    fallback: f64,
) -> f64 {
    let key = if a <= b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    };
    let Some(&(gap, leg_w)) = dp_map.get(&key) else {
        return fallback;
    };
    let is_leg = |w: Option<f64>| w.map(|w| w >= leg_w - 0.01).unwrap_or(true);
    if !is_leg(w_a) || !is_leg(w_b) {
        return fallback;
    }
    // 5um under the declared gap: legs coupled at EXACTLY the gap are the
    // design intent, and offset/grid arithmetic sits within float noise of
    // it. Anything meaningfully tighter is still a pinch.
    (gap - 0.005).max(0.0)
}

/// Compute the minimum distance between two axis-aligned bounding boxes.
/// Each box is represented as `[min_x, min_y, max_x, max_y]`.
/// Returns 0.0 if they overlap.
///
/// Retained as a broadphase utility; the clearance passes use true-geometry
/// distance (see [`CopperGeom::distance_to`]) for the narrowphase.
#[cfg(test)]
fn bbox_distance(a: [f64; 4], b: [f64; 4]) -> f64 {
    let dx = (a[0] - b[2]).max(b[0] - a[2]).max(0.0);
    let dy = (a[1] - b[3]).max(b[1] - a[3]).max(0.0);
    (dx * dx + dy * dy).sqrt()
}

/// Compute minimum distance from a point to a closed polygon (edge segments).
fn min_distance_to_polygon(point: &Vec2, polygon: &[Vec2]) -> f64 {
    if polygon.is_empty() {
        return f64::MAX;
    }

    let mut min_dist = f64::MAX;
    let n = polygon.len();
    for i in 0..n {
        let a = &polygon[i];
        let b = &polygon[(i + 1) % n];
        let dist = point_to_segment_distance(point, a, b);
        if dist < min_dist {
            min_dist = dist;
        }
    }
    min_dist
}

/// Distance from a point to a line segment.
fn point_to_segment_distance(p: &Vec2, a: &Vec2, b: &Vec2) -> f64 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let len_sq = dx * dx + dy * dy;

    if len_sq < 1e-12 {
        // Degenerate segment (point)
        let ex = p.x - a.x;
        let ey = p.y - a.y;
        return (ex * ex + ey * ey).sqrt();
    }

    let t = ((p.x - a.x) * dx + (p.y - a.y) * dy) / len_sq;
    let t = t.clamp(0.0, 1.0);

    let proj_x = a.x + t * dx;
    let proj_y = a.y + t * dy;
    let ex = p.x - proj_x;
    let ey = p.y - proj_y;
    (ex * ex + ey * ey).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_ir::ecad::*;

    /// Create a minimal clean PCB (no violations expected).
    fn clean_pcb() -> Pcb {
        Pcb {
            outline: BoardOutline {
                vertices: vec![
                    Vec2::new(0.0, 0.0),
                    Vec2::new(100.0, 0.0),
                    Vec2::new(100.0, 80.0),
                    Vec2::new(0.0, 80.0),
                ],
                cutouts: vec![],
                thickness: 1.6,
            },
            stackup: LayerStackup {
                layers: vec![
                    StackupLayer {
                        layer: PcbLayer::FCu,
                        copper_thickness: Some(0.035),
                        dielectric_thickness: Some(1.5),
                        dielectric_er: Some(4.5),
                        material: Some("FR4".to_string()),
                    },
                    StackupLayer {
                        layer: PcbLayer::BCu,
                        copper_thickness: Some(0.035),
                        dielectric_thickness: None,
                        dielectric_er: None,
                        material: None,
                    },
                ],
            },
            nets: vec![
                Net {
                    id: "1".to_string(),
                    name: "VCC".to_string(),
                },
                Net {
                    id: "2".to_string(),
                    name: "GND".to_string(),
                },
            ],
            rules: DesignRules {
                default_rules: NetClassRules {
                    name: "Default".to_string(),
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
            },
            footprints: vec![],
            traces: vec![
                Trace {
                    start: Vec2::new(20.0, 40.0),
                    end: Vec2::new(50.0, 40.0),
                    width: 0.25,
                    layer: PcbLayer::FCu,
                    net: "1".to_string(),
                    source: None,
                },
                Trace {
                    start: Vec2::new(20.0, 50.0),
                    end: Vec2::new(50.0, 50.0),
                    width: 0.25,
                    layer: PcbLayer::FCu,
                    net: "2".to_string(),
                    source: None,
                },
            ],
            trace_arcs: vec![],
            vias: vec![Via {
                position: Vec2::new(50.0, 40.0),
                diameter: 0.8,
                drill: 0.4,
                start_layer: PcbLayer::FCu,
                end_layer: PcbLayer::BCu,
                net: "1".to_string(),
                source: None,
            }],
            zones: vec![],
            keepouts: vec![],
            net_ties: vec![],
        }
    }

    /// A foreign-net through-via passing through an inner-layer plane must NOT
    /// be flagged as a short at ANY clearance — including the degenerate
    /// near-zero anti-pad where the polygonized clearance hole would vanish.
    /// Real 4-layer fabrication anti-pads the via, so the plane never connects
    /// to it. Before the net-aware pour fix, small/zero clearance produced a
    /// cascade of spurious shorts (every via false-joined through the plane).
    #[test]
    fn via_through_foreign_plane_is_not_short() {
        for clr in [0.0_f64, 0.001, 0.05, 0.2] {
            let mut pcb = clean_pcb();
            pcb.nets.push(Net {
                id: "3".to_string(),
                name: "3V3".to_string(),
            });
            // 3V3 plane flooding the whole board on inner layer In2Cu.
            pcb.zones.push(Zone {
                outline: pcb.outline.vertices.clone(),
                holes: vec![],
                net: "3".to_string(),
                layer: PcbLayer::In2Cu,
                clearance: clr,
                min_area: 0.0,
                fill_type: ZoneFillType::Solid,
                thermal_relief: ThermalReliefStyle::Relief,
                thermal_gap: Some(0.5),
                thermal_spoke_width: Some(0.5),
                priority: 0,
            });
            // A GND (net "2") through-via FCu->BCu that physically passes
            // through the In2Cu 3V3 plane.
            pcb.vias.push(Via {
                position: Vec2::new(70.0, 40.0),
                diameter: 0.8,
                drill: 0.4,
                start_layer: PcbLayer::FCu,
                end_layer: PcbLayer::BCu,
                net: "2".to_string(),
                source: None,
            });

            let shorts: Vec<_> = check_drc(&pcb)
                .into_iter()
                .filter(|v| matches!(v.rule, DrcRuleType::Short))
                .collect();
            assert!(
                shorts.is_empty(),
                "via through a foreign plane must not short (clearance={clr}): {:?}",
                shorts
            );
        }
    }

    /// Guard against over-suppression: a genuine cross-net copper contact on the
    /// plane layer is still reported as a short.
    #[test]
    fn real_short_still_detected_with_plane() {
        let mut pcb = clean_pcb();
        pcb.nets.push(Net {
            id: "3".to_string(),
            name: "3V3".to_string(),
        });
        pcb.zones.push(Zone {
            outline: pcb.outline.vertices.clone(),
            holes: vec![],
            net: "3".to_string(),
            layer: PcbLayer::In2Cu,
            clearance: 0.2,
            min_area: 0.0,
            fill_type: ZoneFillType::Solid,
            thermal_relief: ThermalReliefStyle::Relief,
            thermal_gap: Some(0.5),
            thermal_spoke_width: Some(0.5),
            priority: 0,
        });
        // A 3V3 trace on FCu crossing the existing GND trace ((20,50)-(50,50)).
        pcb.traces.push(Trace {
            start: Vec2::new(35.0, 45.0),
            end: Vec2::new(35.0, 55.0),
            width: 0.25,
            layer: PcbLayer::FCu,
            net: "3".to_string(),
            source: None,
        });
        let shorts: Vec<_> = check_drc(&pcb)
            .into_iter()
            .filter(|v| matches!(v.rule, DrcRuleType::Short))
            .collect();
        assert!(
            shorts
                .iter()
                .any(|v| v.message.contains("'2'") && v.message.contains("'3'")),
            "a real GND/3V3 contact must still be flagged: {:?}",
            shorts
        );
    }

    /// A foreign-net trace lying over a pour on the SAME layer is NOT a short:
    /// the pour is generated with a clearance void around all foreign copper, so
    /// it never galvanically connects to another net. A signal trace crossing a
    /// ground pour is the everyday case, not a defect (inadequate spacing would
    /// be a Clearance concern). This guards the pour-only-same-net rule against
    /// a regression that re-introduces the false short.
    #[test]
    fn same_layer_foreign_trace_over_pour_is_not_short() {
        let mut pcb = clean_pcb();
        pcb.nets.push(Net {
            id: "3".to_string(),
            name: "3V3".to_string(),
        });
        // GND (net "2") pour flooding the board on FCu.
        pcb.zones.push(Zone {
            outline: pcb.outline.vertices.clone(),
            holes: vec![],
            net: "2".to_string(),
            layer: PcbLayer::FCu,
            clearance: 0.2,
            min_area: 0.0,
            fill_type: ZoneFillType::Solid,
            thermal_relief: ThermalReliefStyle::Relief,
            thermal_gap: Some(0.5),
            thermal_spoke_width: Some(0.5),
            priority: 0,
        });
        // A 3V3 (net "3") signal trace on FCu, inside the GND pour area.
        pcb.traces.push(Trace {
            start: Vec2::new(20.0, 65.0),
            end: Vec2::new(60.0, 65.0),
            width: 0.25,
            layer: PcbLayer::FCu,
            net: "3".to_string(),
            source: None,
        });
        let shorts: Vec<_> = check_drc(&pcb)
            .into_iter()
            .filter(|v| matches!(v.rule, DrcRuleType::Short))
            .collect();
        assert!(
            shorts.is_empty(),
            "a signal trace over a same-layer foreign pour must not short (the pour voids around it): {:?}",
            shorts
        );
    }

    #[test]
    fn clean_pcb_no_violations() {
        let pcb = clean_pcb();
        let violations = check_drc(&pcb);
        // The clean PCB should have no violations — traces are 10mm apart
        assert!(
            violations.is_empty(),
            "expected no violations, got: {:?}",
            violations
        );
    }

    /// One-pad SMD footprint for the rotated-placement regression tests.
    fn one_pad_footprint(
        reference: &str,
        position: Vec2,
        rotation: f64,
        pad_local: Vec2,
        net: &str,
    ) -> Footprint {
        Footprint {
            reference: reference.to_string(),
            value: "X".to_string(),
            footprint_name: "TEST_1PAD".to_string(),
            position,
            rotation,
            front: true,
            pads: vec![Pad {
                number: "1".to_string(),
                pad_type: PadType::SMD,
                shape: PadShape::Rect {
                    width: 1.2,
                    height: 1.2,
                },
                position: pad_local,
                rotation: 0.0,
                drill: None,
                net: Some(net.to_string()),
                layers: vec![PcbLayer::FCu],
            }],
            graphics: vec![],
            model_3d: None,
            properties: std::collections::HashMap::new(),
        }
    }

    /// DRC must judge pad copper at the ROTATED world position — the position
    /// `get_pad_positions` reports and the Gerber writer exports. Regression
    /// for the pad transform dropping the footprint rotation: J1's pad sits at
    /// local (+5, 0) on a 180°-rotated footprint, so its real copper is at
    /// (65, 60); the unrotated phantom position (75, 60) would sit dead on
    /// R1's foreign-net pad and produced a bogus clearance/short.
    #[test]
    fn rotated_footprint_pads_are_judged_at_rotated_positions() {
        let mut pcb = clean_pcb();
        pcb.footprints.push(one_pad_footprint(
            "J1",
            Vec2::new(70.0, 60.0),
            180.0,
            Vec2::new(5.0, 0.0),
            "1",
        ));
        pcb.footprints.push(one_pad_footprint(
            "R1",
            Vec2::new(75.0, 60.0),
            0.0,
            Vec2::new(0.0, 0.0),
            "2",
        ));
        let conflicts: Vec<_> = check_drc(&pcb)
            .into_iter()
            .filter(|v| matches!(v.rule, DrcRuleType::Clearance | DrcRuleType::Short))
            .collect();
        assert!(
            conflicts.is_empty(),
            "rotated pad copper is 10mm from R1 — a violation means DRC placed \
             it at the unrotated phantom position: {:?}",
            conflicts
        );
    }

    /// Positive control for the test above: when the ROTATED position really
    /// does land on a foreign-net pad, DRC must still flag it.
    #[test]
    fn rotated_footprint_real_pad_overlap_is_still_flagged() {
        let mut pcb = clean_pcb();
        // 180° rotation puts J1's pad at (80 − 5, 60) = (75, 60) — on R1.
        pcb.footprints.push(one_pad_footprint(
            "J1",
            Vec2::new(80.0, 60.0),
            180.0,
            Vec2::new(5.0, 0.0),
            "1",
        ));
        pcb.footprints.push(one_pad_footprint(
            "R1",
            Vec2::new(75.0, 60.0),
            0.0,
            Vec2::new(0.0, 0.0),
            "2",
        ));
        let conflicts: Vec<_> = check_drc(&pcb)
            .into_iter()
            .filter(|v| matches!(v.rule, DrcRuleType::Clearance | DrcRuleType::Short))
            .collect();
        assert!(
            !conflicts.is_empty(),
            "overlapping foreign-net pads at the rotated position must be flagged"
        );
    }

    #[test]
    fn detect_min_trace_width_violation() {
        let mut pcb = clean_pcb();
        pcb.traces.push(Trace {
            start: Vec2::new(20.0, 60.0),
            end: Vec2::new(50.0, 60.0),
            width: 0.1, // below 0.25 minimum
            layer: PcbLayer::FCu,
            net: "1".to_string(),
            source: None,
        });

        let violations = check_drc(&pcb);
        let trace_violations: Vec<_> = violations
            .iter()
            .filter(|v| v.rule == DrcRuleType::MinTraceWidth)
            .collect();
        assert!(
            !trace_violations.is_empty(),
            "should detect min trace width violation"
        );
        assert!((trace_violations[0].actual - 0.1).abs() < 1e-6);
        assert!((trace_violations[0].required - 0.25).abs() < 1e-6);
    }

    #[test]
    fn detect_pad_to_pad_short() {
        let mut pcb = clean_pcb();
        let pad = |num: &str, x: f64, net: &str| Pad {
            number: num.to_string(),
            pad_type: PadType::SMD,
            shape: PadShape::Rect {
                width: 1.0,
                height: 1.2,
            },
            position: Vec2::new(x, 0.0),
            rotation: 0.0,
            drill: None,
            net: Some(net.to_string()),
            layers: vec![PcbLayer::FCu],
        };
        // Two stacked pads on different nets — a hard short.
        pcb.footprints.push(Footprint {
            reference: "U1".to_string(),
            value: "IC".to_string(),
            footprint_name: "broken".to_string(),
            position: Vec2::new(60.0, 60.0),
            rotation: 0.0,
            front: true,
            pads: vec![pad("1", 0.0, "1"), pad("2", 0.0, "2")],
            graphics: vec![],
            model_3d: None,
            properties: std::collections::HashMap::new(),
        });

        let violations = check_drc(&pcb);
        let pad_violations: Vec<_> = violations
            .iter()
            .filter(|v| v.rule == DrcRuleType::Clearance && v.message.contains("pad"))
            .collect();
        assert!(
            !pad_violations.is_empty(),
            "should detect pad-to-pad short, got: {:?}",
            violations
        );
    }

    #[test]
    fn detect_min_drill_violation() {
        let mut pcb = clean_pcb();
        pcb.vias.push(Via {
            position: Vec2::new(30.0, 60.0),
            diameter: 0.6,
            drill: 0.15, // below 0.2 minimum
            start_layer: PcbLayer::FCu,
            end_layer: PcbLayer::BCu,
            net: "2".to_string(),
            source: None,
        });

        let violations = check_drc(&pcb);
        let drill_violations: Vec<_> = violations
            .iter()
            .filter(|v| v.rule == DrcRuleType::MinDrill)
            .collect();
        assert!(
            !drill_violations.is_empty(),
            "should detect min drill violation"
        );
        assert!((drill_violations[0].actual - 0.15).abs() < 1e-6);
    }

    #[test]
    fn detect_edge_clearance_violation() {
        let mut pcb = clean_pcb();
        // Place a trace very close to the board edge
        pcb.traces.push(Trace {
            start: Vec2::new(0.1, 40.0),
            end: Vec2::new(0.1, 60.0),
            width: 0.25,
            layer: PcbLayer::FCu,
            net: "1".to_string(),
            source: None,
        });

        let violations = check_drc(&pcb);
        let edge_violations: Vec<_> = violations
            .iter()
            .filter(|v| v.rule == DrcRuleType::EdgeClearance)
            .collect();
        assert!(
            !edge_violations.is_empty(),
            "should detect edge clearance violation"
        );
    }

    #[test]
    fn detect_hole_to_hole_violation() {
        let mut pcb = clean_pcb();
        // Place two vias very close together
        pcb.vias.push(Via {
            position: Vec2::new(50.5, 40.0),
            diameter: 0.8,
            drill: 0.4,
            start_layer: PcbLayer::FCu,
            end_layer: PcbLayer::BCu,
            net: "2".to_string(),
            source: None,
        });

        let violations = check_drc(&pcb);
        let hole_violations: Vec<_> = violations
            .iter()
            .filter(|v| v.rule == DrcRuleType::HoleToHole)
            .collect();
        assert!(
            !hole_violations.is_empty(),
            "should detect hole-to-hole violation"
        );
    }

    #[test]
    fn detect_annular_ring_violation() {
        let mut pcb = clean_pcb();
        // Via with very thin annular ring
        pcb.vias.push(Via {
            position: Vec2::new(70.0, 40.0),
            diameter: 0.5,
            drill: 0.4, // ring = 0.05mm < 0.15mm
            start_layer: PcbLayer::FCu,
            end_layer: PcbLayer::BCu,
            net: "1".to_string(),
            source: None,
        });

        let violations = check_drc(&pcb);
        let ring_violations: Vec<_> = violations
            .iter()
            .filter(|v| v.rule == DrcRuleType::AnnularRing)
            .collect();
        assert!(
            !ring_violations.is_empty(),
            "should detect annular ring violation"
        );
        assert!((ring_violations[0].actual - 0.05).abs() < 1e-6);
    }

    #[test]
    fn detect_clearance_violation() {
        let mut pcb = clean_pcb();
        // Remove existing well-spaced traces
        pcb.traces.clear();
        // Add two traces on the same layer, same Y, different nets, very close
        pcb.traces.push(Trace {
            start: Vec2::new(20.0, 40.0),
            end: Vec2::new(80.0, 40.0),
            width: 0.25,
            layer: PcbLayer::FCu,
            net: "1".to_string(),
            source: None,
        });
        pcb.traces.push(Trace {
            start: Vec2::new(20.0, 40.3),
            end: Vec2::new(80.0, 40.3),
            width: 0.25,
            layer: PcbLayer::FCu,
            net: "2".to_string(),
            source: None,
        });

        let violations = check_drc(&pcb);
        let clearance_violations: Vec<_> = violations
            .iter()
            .filter(|v| v.rule == DrcRuleType::Clearance)
            .collect();
        assert!(
            !clearance_violations.is_empty(),
            "should detect clearance violation between close traces"
        );
    }

    #[test]
    fn ground_pour_does_not_short_to_cleared_nets() {
        let mut pcb = clean_pcb();
        // A net "2" (GND) pour flooding the whole board on FCu. The net "1"
        // trace and via sit inside its outline but are cleared by clearance
        // voids — connectivity must read the FILLED copper, not the raw
        // rectangle, so this is NOT a short.
        pcb.zones.push(Zone {
            outline: pcb.outline.vertices.clone(),
            holes: vec![],
            net: "2".to_string(),
            layer: PcbLayer::FCu,
            clearance: 0.3,
            min_area: 0.0,
            fill_type: ZoneFillType::Solid,
            thermal_relief: ThermalReliefStyle::Relief,
            thermal_gap: Some(0.4),
            thermal_spoke_width: Some(0.5),
            priority: 0,
        });
        let viols = check_drc(&pcb);
        let shorts: Vec<_> = viols
            .iter()
            .filter(|v| v.rule == DrcRuleType::Short)
            .map(|v| &v.message)
            .collect();
        assert!(
            shorts.is_empty(),
            "a poured plane must not short to the copper it clears, got: {shorts:?}"
        );
    }

    #[test]
    fn diff_pair_couples_at_its_gap_not_full_clearance() {
        let mut pcb = clean_pcb();
        pcb.traces.clear();
        pcb.vias.clear();
        // A diff-pair class (0.1mm gap, 0.2mm legs) with USB_P / USB_N.
        pcb.rules.class_rules.push(NetClassRules {
            name: "DP".into(),
            trace_width: 0.2,
            clearance: 0.2,
            via_diameter: 0.8,
            via_drill: 0.4,
            diff_pair_gap: Some(0.1),
            diff_pair_width: Some(0.2),
        });
        pcb.rules
            .net_class_assignments
            .insert("DP".into(), vec!["USB_P".into(), "USB_N".into()]);
        // Parallel legs 0.3mm centre-to-centre, width 0.2 -> 0.1mm gap = exactly
        // the declared gap. The normal 0.2mm clearance would flag this.
        let leg = |y: f64, net: &str| Trace {
            start: Vec2::new(20.0, y),
            end: Vec2::new(60.0, y),
            width: 0.2,
            layer: PcbLayer::FCu,
            net: net.into(),
            source: None,
        };
        pcb.traces.push(leg(40.0, "USB_P"));
        pcb.traces.push(leg(40.3, "USB_N"));
        let clr = |p: &Pcb| {
            check_drc(p)
                .into_iter()
                .filter(|v| v.rule == DrcRuleType::Clearance)
                .count()
        };
        assert_eq!(clr(&pcb), 0, "a diff pair at its gap is not a violation");

        // Squeeze them below the gap (0.02mm edge-to-edge) -> flagged.
        pcb.traces[1].start.y = 40.22;
        pcb.traces[1].end.y = 40.22;
        assert!(
            clr(&pcb) > 0,
            "a diff pair closer than its gap must violate"
        );
    }

    #[test]
    fn point_to_segment() {
        let p = Vec2::new(1.0, 1.0);
        let a = Vec2::new(0.0, 0.0);
        let b = Vec2::new(2.0, 0.0);
        let dist = point_to_segment_distance(&p, &a, &b);
        assert!((dist - 1.0).abs() < 1e-10);
    }

    #[test]
    fn bbox_distance_separated() {
        let dist = bbox_distance([0.0, 0.0, 1.0, 1.0], [3.0, 0.0, 4.0, 1.0]);
        assert!((dist - 2.0).abs() < 1e-10);
    }

    #[test]
    fn bbox_distance_overlapping() {
        let dist = bbox_distance([0.0, 0.0, 2.0, 2.0], [1.0, 1.0, 3.0, 3.0]);
        assert!((dist - 0.0).abs() < 1e-10);
    }

    // ------------------------------------------------------------------
    // Test helpers
    // ------------------------------------------------------------------

    fn trace(start: (f64, f64), end: (f64, f64), net: &str) -> Trace {
        Trace {
            start: Vec2::new(start.0, start.1),
            end: Vec2::new(end.0, end.1),
            width: 0.25,
            layer: PcbLayer::FCu,
            net: net.to_string(),
            source: None,
        }
    }

    fn smd_pad(num: &str, pos: (f64, f64), net: &str) -> Pad {
        Pad {
            number: num.to_string(),
            pad_type: PadType::SMD,
            shape: PadShape::Rect {
                width: 1.0,
                height: 1.0,
            },
            position: Vec2::new(pos.0, pos.1),
            rotation: 0.0,
            drill: None,
            net: Some(net.to_string()),
            layers: vec![PcbLayer::FCu],
        }
    }

    fn footprint(reference: &str, pos: (f64, f64), rotation: f64, pads: Vec<Pad>) -> Footprint {
        Footprint {
            reference: reference.to_string(),
            value: "X".to_string(),
            footprint_name: "test".to_string(),
            position: Vec2::new(pos.0, pos.1),
            rotation,
            front: true,
            pads,
            graphics: vec![],
            model_3d: None,
            properties: std::collections::HashMap::new(),
        }
    }

    /// Two pads of the SAME footprint on different nets, closer than the rule
    /// clearance, must NOT be flagged — a footprint is a qualified land pattern,
    /// so intra-footprint pad spacing is the footprint's concern, not board DRC.
    /// (This is what kills the phantom adjacent-pin clearance violations on
    /// fine-pitch parts.)
    #[test]
    fn intra_footprint_pads_exempt_from_clearance() {
        let mut pcb = clean_pcb();
        pcb.traces.clear();
        pcb.vias.clear();
        // Pad edges 0.1mm apart (centers 1.1mm, width 1.0) — well under 0.2mm.
        pcb.footprints = vec![footprint(
            "U1",
            (60.0, 60.0),
            0.0,
            vec![
                smd_pad("1", (-0.55, 0.0), "A"),
                smd_pad("2", (0.55, 0.0), "B"),
            ],
        )];
        let violations = check_drc(&pcb);
        let bad: Vec<_> = violations
            .iter()
            .filter(|v| matches!(v.rule, DrcRuleType::Clearance | DrcRuleType::Short))
            .collect();
        assert!(
            bad.is_empty(),
            "same-footprint pads must be exempt, got: {bad:?}"
        );
    }

    /// Two components whose courtyards (here, pad-bound fallback) overlap on
    /// the same side are a placement collision → CourtyardOverlap.
    #[test]
    fn courtyard_overlap_flags_colliding_components() {
        let mut pcb = clean_pcb();
        pcb.traces.clear();
        pcb.vias.clear();
        // 1×1mm pads → ±0.5mm bounds; centers 0.5mm apart overlap.
        pcb.footprints = vec![
            footprint("U1", (60.0, 60.0), 0.0, vec![smd_pad("1", (0.0, 0.0), "A")]),
            footprint("U2", (60.5, 60.0), 0.0, vec![smd_pad("1", (0.0, 0.0), "B")]),
        ];
        let v = check_drc(&pcb);
        assert!(
            v.iter().any(|x| x.rule == DrcRuleType::CourtyardOverlap),
            "overlapping components must flag CourtyardOverlap: {v:?}"
        );
    }

    /// Separated components, and components on opposite sides, do not collide.
    #[test]
    fn courtyard_overlap_clear_when_separated_or_opposite_side() {
        let mut pcb = clean_pcb();
        pcb.traces.clear();
        pcb.vias.clear();
        // Far apart on the same side.
        pcb.footprints = vec![
            footprint("U1", (50.0, 60.0), 0.0, vec![smd_pad("1", (0.0, 0.0), "A")]),
            footprint("U2", (70.0, 60.0), 0.0, vec![smd_pad("1", (0.0, 0.0), "B")]),
        ];
        assert!(
            !check_drc(&pcb)
                .iter()
                .any(|x| x.rule == DrcRuleType::CourtyardOverlap),
            "separated components must not collide"
        );
        // Overlapping XY but opposite sides — no collision.
        pcb.footprints = vec![
            footprint("U1", (60.0, 60.0), 0.0, vec![smd_pad("1", (0.0, 0.0), "A")]),
            footprint("U2", (60.0, 60.0), 0.0, vec![smd_pad("1", (0.0, 0.0), "B")]),
        ];
        pcb.footprints[1].front = false;
        assert!(
            !check_drc(&pcb)
                .iter()
                .any(|x| x.rule == DrcRuleType::CourtyardOverlap),
            "opposite-side components must not collide"
        );
    }

    /// The exemption is footprint-scoped: two pads of DIFFERENT footprints at
    /// the same spacing MUST still be flagged.
    #[test]
    fn inter_footprint_pads_still_flagged() {
        let mut pcb = clean_pcb();
        pcb.traces.clear();
        pcb.vias.clear();
        pcb.footprints = vec![
            footprint(
                "U1",
                (59.45, 60.0),
                0.0,
                vec![smd_pad("1", (0.0, 0.0), "A")],
            ),
            footprint(
                "U2",
                (60.55, 60.0),
                0.0,
                vec![smd_pad("1", (0.0, 0.0), "B")],
            ),
        ];
        let violations = check_drc(&pcb);
        let clearance: Vec<_> = violations
            .iter()
            .filter(|v| v.rule == DrcRuleType::Clearance)
            .collect();
        assert!(
            !clearance.is_empty(),
            "different-footprint pads 0.1mm apart must still violate"
        );
    }

    // ------------------------------------------------------------------
    // 1. True-geometry clearance narrowphase
    // ------------------------------------------------------------------

    /// Two perpendicular diagonal traces whose closest approach is ~1mm must
    /// NOT be flagged. The old bbox-based check false-positived here because
    /// the diagonal bounding boxes overlap heavily.
    #[test]
    fn diagonal_traces_far_apart_pass() {
        let mut pcb = clean_pcb();
        pcb.traces.clear();
        // Trace A: lower-left to upper-right diagonal (centerline y = x).
        pcb.traces.push(trace((10.0, 10.0), (20.0, 20.0), "1"));
        // Trace B: a perpendicular diagonal (direction (1,-1)) sitting wholly
        // on the y > x side, so it does NOT cross A. Closest approach is its
        // (14,16) endpoint at |14-16|/sqrt(2) ≈ 1.41mm centerline (~1.16mm
        // edge-to-edge). Its AABB still overlaps A's AABB completely, so the
        // old bbox check false-positived here.
        pcb.traces.push(trace((12.0, 18.0), (14.0, 16.0), "2"));

        let violations = check_drc(&pcb);
        let clearance: Vec<_> = violations
            .iter()
            .filter(|v| v.rule == DrcRuleType::Clearance)
            .collect();
        assert!(
            clearance.is_empty(),
            "diagonal traces ~1mm apart should not violate, got: {:?}",
            clearance
        );
    }

    /// Two parallel traces 0.1mm apart (edge-to-edge) MUST still be flagged.
    #[test]
    fn parallel_traces_too_close_fail() {
        let mut pcb = clean_pcb();
        pcb.traces.clear();
        // width 0.25 → half_w 0.125 each. Centerline gap 0.35 → edge gap 0.1.
        pcb.traces.push(trace((10.0, 40.0), (40.0, 40.0), "1"));
        pcb.traces.push(trace((10.0, 40.35), (40.0, 40.35), "2"));

        let violations = check_drc(&pcb);
        let clearance: Vec<_> = violations
            .iter()
            .filter(|v| v.rule == DrcRuleType::Clearance)
            .collect();
        assert!(
            !clearance.is_empty(),
            "parallel traces 0.1mm apart should violate"
        );
        // Edge-to-edge distance should be ~0.1mm, not the centerline 0.35.
        assert!(
            (clearance[0].actual - 0.1).abs() < 1e-6,
            "expected edge distance ~0.1mm, got {}",
            clearance[0].actual
        );
    }

    /// A rotated pad's true footprint is respected: a 45°-rotated rectangular
    /// pad clears a nearby trace its AABB would otherwise overlap.
    #[test]
    fn rotated_pad_clearance_uses_true_geometry() {
        let mut pcb = clean_pcb();
        pcb.traces.clear();
        // A long thin pad rotated 45°, far enough that its real corners clear
        // the trace, even though its AABB is large.
        let mut pad = Pad {
            number: "1".to_string(),
            pad_type: PadType::SMD,
            shape: PadShape::Rect {
                width: 4.0,
                height: 0.4,
            },
            position: Vec2::new(0.0, 0.0),
            rotation: 45.0,
            drill: None,
            net: Some("1".to_string()),
            layers: vec![PcbLayer::FCu],
        };
        pad.net = Some("1".to_string());
        pcb.footprints
            .push(footprint("U1", (50.0, 50.0), 0.0, vec![pad]));
        // Trace on net 2, several mm from the rotated pad's true body.
        pcb.traces.push(trace((40.0, 56.0), (60.0, 56.0), "2"));

        let violations = check_drc(&pcb);
        let clearance: Vec<_> = violations
            .iter()
            .filter(|v| v.rule == DrcRuleType::Clearance)
            .collect();
        assert!(
            clearance.is_empty(),
            "rotated pad should clear distant trace, got: {:?}",
            clearance
        );
    }

    // ------------------------------------------------------------------
    // 2. Keepout enforcement
    // ------------------------------------------------------------------

    #[test]
    fn keepout_no_tracks_flags_trace() {
        let mut pcb = clean_pcb();
        pcb.traces.clear();
        // A no-tracks keepout square around (50,50).
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
        // Trace passing straight through the keepout.
        pcb.traces.push(trace((40.0, 50.0), (60.0, 50.0), "1"));

        let violations = check_drc(&pcb);
        let ko: Vec<_> = violations
            .iter()
            .filter(|v| v.rule == DrcRuleType::Keepout)
            .collect();
        assert!(
            !ko.is_empty(),
            "trace through no-tracks keepout should flag"
        );
    }

    #[test]
    fn keepout_clear_board_passes() {
        let mut pcb = clean_pcb();
        pcb.traces.clear();
        pcb.keepouts.push(Keepout {
            outline: vec![
                Vec2::new(45.0, 45.0),
                Vec2::new(55.0, 45.0),
                Vec2::new(55.0, 55.0),
                Vec2::new(45.0, 55.0),
            ],
            layers: vec![PcbLayer::FCu],
            no_tracks: true,
            no_vias: true,
            no_components: true,
            no_pour: false,
        });
        // Trace well away from the keepout.
        pcb.traces.push(trace((10.0, 20.0), (30.0, 20.0), "1"));

        let violations = check_drc(&pcb);
        let ko: Vec<_> = violations
            .iter()
            .filter(|v| v.rule == DrcRuleType::Keepout)
            .collect();
        assert!(
            ko.is_empty(),
            "clear board should have no keepout violations"
        );
    }

    #[test]
    fn keepout_no_vias_flags_via_inside() {
        let mut pcb = clean_pcb();
        pcb.traces.clear();
        pcb.vias.clear();
        pcb.keepouts.push(Keepout {
            outline: vec![
                Vec2::new(45.0, 45.0),
                Vec2::new(55.0, 45.0),
                Vec2::new(55.0, 55.0),
                Vec2::new(45.0, 55.0),
            ],
            layers: vec![PcbLayer::FCu],
            no_tracks: false,
            no_vias: true,
            no_pour: false,
            no_components: false,
        });
        pcb.vias.push(Via {
            position: Vec2::new(50.0, 50.0),
            diameter: 0.8,
            drill: 0.4,
            start_layer: PcbLayer::FCu,
            end_layer: PcbLayer::BCu,
            net: "1".to_string(),
            source: None,
        });

        let violations = check_drc(&pcb);
        let ko: Vec<_> = violations
            .iter()
            .filter(|v| v.rule == DrcRuleType::Keepout && v.message.contains("via"))
            .collect();
        assert!(!ko.is_empty(), "via inside no-vias keepout should flag");
    }

    // ------------------------------------------------------------------
    // 3. Net-tie exemption
    // ------------------------------------------------------------------

    #[test]
    fn overlapping_nets_short_without_tie() {
        let mut pcb = clean_pcb();
        pcb.traces.clear();
        // Two traces of different nets that physically touch at (50,50).
        pcb.traces.push(trace((40.0, 50.0), (50.0, 50.0), "1"));
        pcb.traces.push(trace((50.0, 50.0), (60.0, 50.0), "2"));

        let violations = check_drc(&pcb);
        let shorts: Vec<_> = violations
            .iter()
            .filter(|v| v.rule == DrcRuleType::Short || v.rule == DrcRuleType::Clearance)
            .collect();
        assert!(
            !shorts.is_empty(),
            "touching different-net traces should short without a tie"
        );
    }

    #[test]
    fn net_tie_exempts_short() {
        let mut pcb = clean_pcb();
        pcb.traces.clear();
        pcb.traces.push(trace((40.0, 50.0), (50.0, 50.0), "1"));
        pcb.traces.push(trace((50.0, 50.0), (60.0, 50.0), "2"));
        // Join nets 1 and 2 board-wide.
        pcb.net_ties.push(NetTie {
            nets: vec!["1".to_string(), "2".to_string()],
            position: None,
            radius: None,
        });

        let violations = check_drc(&pcb);
        let shorts: Vec<_> = violations
            .iter()
            .filter(|v| v.rule == DrcRuleType::Short || v.rule == DrcRuleType::Clearance)
            .collect();
        assert!(
            shorts.is_empty(),
            "net-tied junction should not short, got: {:?}",
            shorts
        );
    }

    #[test]
    fn net_tie_region_scoped() {
        let mut pcb = clean_pcb();
        pcb.traces.clear();
        // Junction at (50,50).
        pcb.traces.push(trace((40.0, 50.0), (50.0, 50.0), "1"));
        pcb.traces.push(trace((50.0, 50.0), (60.0, 50.0), "2"));
        // Tie region centered far away — the junction is OUTSIDE it, so the
        // exemption must NOT apply.
        pcb.net_ties.push(NetTie {
            nets: vec!["1".to_string(), "2".to_string()],
            position: Some(Vec2::new(10.0, 10.0)),
            radius: Some(2.0),
        });

        let violations = check_drc(&pcb);
        let clearance: Vec<_> = violations
            .iter()
            .filter(|v| v.rule == DrcRuleType::Clearance)
            .collect();
        assert!(
            !clearance.is_empty(),
            "junction outside tie region should still violate clearance"
        );
    }

    /// A region-scoped tie covering the junction exempts the galvanic Short
    /// there — the whole point of a scoped tie (e.g. a winding's star point).
    #[test]
    fn scoped_net_tie_exempts_short_at_junction() {
        let mut pcb = clean_pcb();
        pcb.traces.clear();
        pcb.traces.push(trace((40.0, 50.0), (50.0, 50.0), "1"));
        pcb.traces.push(trace((50.0, 50.0), (60.0, 50.0), "2"));
        pcb.net_ties.push(NetTie {
            nets: vec!["1".to_string(), "2".to_string()],
            position: Some(Vec2::new(50.0, 50.0)),
            radius: Some(2.0),
        });

        let violations = check_drc(&pcb);
        let bad: Vec<_> = violations
            .iter()
            .filter(|v| v.rule == DrcRuleType::Short || v.rule == DrcRuleType::Clearance)
            .collect();
        assert!(
            bad.is_empty(),
            "junction inside scoped tie region must be exempt, got: {:?}",
            bad
        );
    }

    /// The scoped exemption is local: the same two nets touching AGAIN outside
    /// the tie region is still a short, reported at the stray contact.
    #[test]
    fn scoped_net_tie_does_not_exempt_stray_contact() {
        let mut pcb = clean_pcb();
        pcb.traces.clear();
        // Intentional junction at (50,50), tied there.
        pcb.traces.push(trace((40.0, 50.0), (50.0, 50.0), "1"));
        pcb.traces.push(trace((50.0, 50.0), (60.0, 50.0), "2"));
        // Stray second contact between the same nets at (50,20).
        pcb.traces.push(trace((40.0, 20.0), (50.0, 20.0), "1"));
        pcb.traces.push(trace((50.0, 20.0), (60.0, 20.0), "2"));
        pcb.net_ties.push(NetTie {
            nets: vec!["1".to_string(), "2".to_string()],
            position: Some(Vec2::new(50.0, 50.0)),
            radius: Some(2.0),
        });

        let shorts: Vec<_> = check_drc(&pcb)
            .into_iter()
            .filter(|v| v.rule == DrcRuleType::Short)
            .collect();
        assert!(
            !shorts.is_empty(),
            "same nets touching outside the tie region must still short"
        );
        let p = shorts[0].position;
        assert!(
            (p.y - 20.0).abs() < 1.0,
            "short must be reported at the stray contact, got {:?}",
            p
        );
    }

    /// Wye/star junction: three phase nets each touch the neutral inside one
    /// scoped tie region and are thereby joined pairwise *indirectly*. No pair
    /// touches directly, and every join is intentional — no shorts.
    #[test]
    fn scoped_net_tie_exempts_wye_star_junction() {
        let mut pcb = clean_pcb();
        pcb.traces.clear();
        // Neutral bar at y=50, x in [48,52].
        pcb.traces.push(trace((48.0, 50.0), (52.0, 50.0), "WIND_N"));
        // Three phases arriving at the bar.
        pcb.traces.push(trace((48.0, 50.0), (40.0, 58.0), "PHA"));
        pcb.traces.push(trace((50.0, 50.0), (50.0, 60.0), "PHB"));
        pcb.traces.push(trace((52.0, 50.0), (60.0, 58.0), "PHC"));
        pcb.net_ties.push(NetTie {
            nets: vec![
                "PHA".to_string(),
                "PHB".to_string(),
                "PHC".to_string(),
                "WIND_N".to_string(),
            ],
            position: Some(Vec2::new(50.0, 50.0)),
            radius: Some(3.0),
        });

        let shorts: Vec<_> = check_drc(&pcb)
            .into_iter()
            .filter(|v| v.rule == DrcRuleType::Short)
            .collect();
        assert!(
            shorts.is_empty(),
            "star junction under a scoped tie must not short, got: {:?}",
            shorts
        );

        // A genuine PHA/PHB crossing far from the junction still fires.
        pcb.traces.push(trace((10.0, 10.0), (30.0, 10.0), "PHA"));
        pcb.traces.push(trace((20.0, 5.0), (20.0, 15.0), "PHB"));
        let shorts: Vec<_> = check_drc(&pcb)
            .into_iter()
            .filter(|v| v.rule == DrcRuleType::Short)
            .collect();
        assert!(
            shorts
                .iter()
                .any(|v| v.message.contains("PHA") && v.message.contains("PHB")),
            "crossing outside the tie region must still short, got: {:?}",
            shorts
        );
    }

    // ------------------------------------------------------------------
    // 4. Connectivity flood-fill
    // ------------------------------------------------------------------

    /// A stray copper bridge between two otherwise-separate nets reports a Short.
    #[test]
    fn connectivity_detects_short_bridge() {
        let mut pcb = clean_pcb();
        pcb.traces.clear();
        pcb.vias.clear();
        // Net 1 island.
        pcb.traces.push(trace((10.0, 30.0), (30.0, 30.0), "1"));
        // Net 2 island.
        pcb.traces.push(trace((10.0, 50.0), (30.0, 50.0), "2"));
        // Stray bridge on net 1 touching both islands (an accidental short).
        pcb.traces.push(trace((20.0, 30.0), (20.0, 50.0), "1"));

        let violations = check_drc(&pcb);
        let shorts: Vec<_> = violations
            .iter()
            .filter(|v| v.rule == DrcRuleType::Short)
            .collect();
        assert!(
            !shorts.is_empty(),
            "copper bridge between nets should report a Short, got: {:?}",
            violations
        );
        assert!(shorts[0].message.contains('1') && shorts[0].message.contains('2'));
    }

    /// A net whose two pads are not connected by copper reports UnconnectedNet.
    #[test]
    fn connectivity_detects_unrouted() {
        let mut pcb = clean_pcb();
        pcb.traces.clear();
        pcb.vias.clear();
        // Two pads on net "1", far apart, with NO trace between them.
        pcb.footprints.push(footprint(
            "J1",
            (10.0, 10.0),
            0.0,
            vec![smd_pad("1", (0.0, 0.0), "1")],
        ));
        pcb.footprints.push(footprint(
            "J2",
            (60.0, 60.0),
            0.0,
            vec![smd_pad("1", (0.0, 0.0), "1")],
        ));

        let violations = check_drc(&pcb);
        let unrouted: Vec<_> = violations
            .iter()
            .filter(|v| v.rule == DrcRuleType::UnconnectedNet)
            .collect();
        assert!(
            !unrouted.is_empty(),
            "two disjoint pads on the same net should report UnconnectedNet, got: {:?}",
            violations
        );
    }

    /// A correctly-routed net (pads joined by a trace) reports neither a short
    /// nor an unrouted-net violation.
    #[test]
    fn connectivity_clean_routed_net() {
        let mut pcb = clean_pcb();
        pcb.traces.clear();
        pcb.vias.clear();
        // Two pads on net "1" joined by a trace.
        pcb.footprints.push(footprint(
            "J1",
            (10.0, 40.0),
            0.0,
            vec![smd_pad("1", (0.0, 0.0), "1")],
        ));
        pcb.footprints.push(footprint(
            "J2",
            (30.0, 40.0),
            0.0,
            vec![smd_pad("1", (0.0, 0.0), "1")],
        ));
        pcb.traces.push(trace((10.0, 40.0), (30.0, 40.0), "1"));

        let violations = check_drc(&pcb);
        let bad: Vec<_> = violations
            .iter()
            .filter(|v| v.rule == DrcRuleType::UnconnectedNet || v.rule == DrcRuleType::Short)
            .collect();
        assert!(
            bad.is_empty(),
            "correctly routed net should be clean, got: {:?}",
            bad
        );
    }

    /// Via bridges its layer span: a trace on FCu and a trace on BCu joined by
    /// a via on the same net form one component (no spurious unrouted).
    #[test]
    fn connectivity_via_bridges_layers() {
        let mut pcb = clean_pcb();
        pcb.traces.clear();
        pcb.vias.clear();
        // FCu pad + trace.
        pcb.footprints.push(footprint(
            "J1",
            (10.0, 40.0),
            0.0,
            vec![smd_pad("1", (0.0, 0.0), "1")],
        ));
        pcb.traces.push(trace((10.0, 40.0), (40.0, 40.0), "1"));
        // BCu pad + trace.
        let mut bpad = smd_pad("1", (0.0, 0.0), "1");
        bpad.layers = vec![PcbLayer::BCu];
        pcb.footprints
            .push(footprint("J2", (40.0, 40.0), 0.0, vec![bpad]));
        let mut btrace = trace((40.0, 40.0), (40.0, 40.0), "1");
        btrace.layer = PcbLayer::BCu;
        // Via at (40,40) bridging FCu→BCu on net 1.
        pcb.vias.push(Via {
            position: Vec2::new(40.0, 40.0),
            diameter: 0.8,
            drill: 0.4,
            start_layer: PcbLayer::FCu,
            end_layer: PcbLayer::BCu,
            net: "1".to_string(),
            source: None,
        });

        let violations = check_drc(&pcb);
        let unrouted: Vec<_> = violations
            .iter()
            .filter(|v| v.rule == DrcRuleType::UnconnectedNet)
            .collect();
        assert!(
            unrouted.is_empty(),
            "via should bridge FCu/BCu pads on same net, got: {:?}",
            unrouted
        );
    }

    /// Issue #378 regression: connectivity must place a rotated footprint's
    /// pads at their TRUE (rotation-applied) world positions — the same
    /// positions get_pad_positions, the ratsnest, and the routers report.
    /// Before the fix the pad nodes sat at the unrotated (phantom) offsets, so
    /// copper laid between the true positions (a hand route) never touched
    /// them and UnconnectedNet could not clear.
    #[test]
    fn connectivity_credits_copper_at_rotated_pad_positions() {
        let mut pcb = clean_pcb();
        pcb.traces.clear();
        pcb.vias.clear();
        // Both footprints rotated 90°, pads 2mm off the footprint origin along
        // local +X. True pad centers land 2mm along +Y instead:
        //   J1 pad: (20, 42)   [phantom would be (22, 40)]
        //   J2 pad: (50, 42)   [phantom would be (52, 40)]
        pcb.footprints.push(footprint(
            "J1",
            (20.0, 40.0),
            90.0,
            vec![smd_pad("1", (2.0, 0.0), "1")],
        ));
        pcb.footprints.push(footprint(
            "J2",
            (50.0, 40.0),
            90.0,
            vec![smd_pad("1", (2.0, 0.0), "1")],
        ));

        // Sanity: with no copper the net is open.
        let open: Vec<_> = check_drc(&pcb)
            .into_iter()
            .filter(|v| v.rule == DrcRuleType::UnconnectedNet)
            .collect();
        assert!(!open.is_empty(), "unrouted rotated pads should report open");

        // Hand-route between the TRUE rotated pad centers.
        pcb.traces.push(trace((20.0, 42.0), (50.0, 42.0), "1"));

        let violations = check_drc(&pcb);
        let bad: Vec<_> = violations
            .iter()
            .filter(|v| v.rule == DrcRuleType::UnconnectedNet || v.rule == DrcRuleType::Short)
            .collect();
        assert!(
            bad.is_empty(),
            "copper at the true rotated pad positions must clear UnconnectedNet, got: {:?}",
            bad
        );
    }

    /// Issue #378 counterpart: copper laid at the phantom (unrotated) offsets
    /// must NOT be credited — those locations hold no pad copper on a rotated
    /// footprint.
    #[test]
    fn connectivity_ignores_copper_at_phantom_unrotated_positions() {
        let mut pcb = clean_pcb();
        pcb.traces.clear();
        pcb.vias.clear();
        pcb.footprints.push(footprint(
            "J1",
            (20.0, 40.0),
            90.0,
            vec![smd_pad("1", (2.0, 0.0), "1")],
        ));
        pcb.footprints.push(footprint(
            "J2",
            (50.0, 40.0),
            90.0,
            vec![smd_pad("1", (2.0, 0.0), "1")],
        ));
        // Trace between the unrotated offsets — thin air on this board.
        pcb.traces.push(trace((22.0, 40.0), (52.0, 40.0), "1"));

        let unrouted: Vec<_> = check_drc(&pcb)
            .into_iter()
            .filter(|v| v.rule == DrcRuleType::UnconnectedNet)
            .collect();
        assert!(
            !unrouted.is_empty(),
            "copper at phantom unrotated positions must not credit connectivity"
        );
    }

    // ------------------------------------------------------------------------
    // Provenance + generated tagging
    // ------------------------------------------------------------------------

    /// A THT footprint with one drilled, netted pad. `generated` marks it as a
    /// synthesized land pattern via the `padSource` property the placer sets.
    fn tht_fp(reference: &str, at: Vec2, drill: f64, net: &str, generated: bool) -> Footprint {
        let mut properties = std::collections::HashMap::new();
        if generated {
            properties.insert("padSource".to_string(), "generated".to_string());
        }
        Footprint {
            reference: reference.to_string(),
            value: "X".to_string(),
            footprint_name: "fp".to_string(),
            position: at,
            rotation: 0.0,
            front: true,
            pads: vec![Pad {
                number: "1".to_string(),
                pad_type: PadType::THT,
                shape: PadShape::Circle { diameter: 1.6 },
                position: Vec2::new(0.0, 0.0),
                rotation: 0.0,
                drill: Some(vcad_ir::ecad::DrillSpec {
                    diameter: drill,
                    oval: false,
                    oval_height: None,
                }),
                net: Some(net.to_string()),
                layers: vec![
                    PcbLayer::FCu,
                    PcbLayer::BCu,
                    PcbLayer::FMask,
                    PcbLayer::BMask,
                ],
            }],
            graphics: vec![],
            model_3d: None,
            properties,
        }
    }

    /// Every violation carries a provenance, and a via drill fault is routing.
    #[test]
    fn via_drill_fault_is_routing_provenance() {
        let mut pcb = clean_pcb();
        pcb.vias.push(Via {
            position: Vec2::new(30.0, 60.0),
            diameter: 0.6,
            drill: 0.15,
            start_layer: PcbLayer::FCu,
            end_layer: PcbLayer::BCu,
            net: "2".to_string(),
            source: None,
        });
        let v = check_drc(&pcb);
        let d = v.iter().find(|v| v.rule == DrcRuleType::MinDrill).unwrap();
        assert_eq!(d.provenance, DrcProvenance::Routing);
        assert!(
            !d.generated,
            "a via is never a generated footprint artifact"
        );
    }

    /// Two drilled holes of the *same* footprint placed too close are an
    /// intra-footprint hole-to-hole — and inherit the footprint's generated flag.
    #[test]
    fn intra_footprint_hole_to_hole_is_tagged_generated() {
        let mut pcb = clean_pcb();
        let mut fp = tht_fp("J1", Vec2::new(40.0, 40.0), 0.9, "1", true);
        // Add a second hole 1.0mm away — edge = 1.0 - 0.9 = 0.1 < 0.5.
        let mut p2 = fp.pads[0].clone();
        p2.number = "2".to_string();
        p2.position = Vec2::new(1.0, 0.0);
        p2.net = Some("2".to_string());
        fp.pads.push(p2);
        pcb.footprints.push(fp);

        let v = check_drc(&pcb);
        let h = v
            .iter()
            .find(|v| v.rule == DrcRuleType::HoleToHole)
            .unwrap();
        assert_eq!(h.provenance, DrcProvenance::IntraFootprint);
        assert!(
            h.generated,
            "generated footprint's own land pattern artifact"
        );
    }

    /// Holes from two different footprints that crowd report inter_component,
    /// and the generated flag is the OR of the two footprints' flags.
    #[test]
    fn inter_component_hole_to_hole_distinguishes_generated() {
        // One generated, one author-placed; nearest holes 1.0mm apart (edge 0.1).
        let mut pcb = clean_pcb();
        pcb.footprints
            .push(tht_fp("J1", Vec2::new(40.0, 40.0), 0.9, "1", true));
        pcb.footprints
            .push(tht_fp("J2", Vec2::new(41.0, 40.0), 0.9, "2", false));
        let v = check_drc(&pcb);
        let h = v
            .iter()
            .find(|v| v.rule == DrcRuleType::HoleToHole)
            .unwrap();
        assert_eq!(h.provenance, DrcProvenance::InterComponent);
        assert!(h.generated, "one side is generated → flagged generated");

        // Both author-placed → same conflict, but NOT a generated artifact.
        let mut pcb2 = clean_pcb();
        pcb2.footprints
            .push(tht_fp("J1", Vec2::new(40.0, 40.0), 0.9, "1", false));
        pcb2.footprints
            .push(tht_fp("J2", Vec2::new(41.0, 40.0), 0.9, "2", false));
        let v2 = check_drc(&pcb2);
        let h2 = v2
            .iter()
            .find(|v| v.rule == DrcRuleType::HoleToHole)
            .unwrap();
        assert_eq!(h2.provenance, DrcProvenance::InterComponent);
        assert!(
            !h2.generated,
            "neither side generated → real fault, not artifact"
        );
    }

    /// Provenance serializes snake_case so the MCP layer can group by it.
    #[test]
    fn provenance_serializes_snake_case() {
        let json = serde_json::to_string(&DrcProvenance::IntraFootprint).unwrap();
        assert_eq!(json, "\"intra_footprint\"");
        assert_eq!(
            serde_json::to_string(&DrcProvenance::InterComponent).unwrap(),
            "\"inter_component\""
        );
        assert_eq!(
            serde_json::to_string(&DrcProvenance::Routing).unwrap(),
            "\"routing\""
        );
    }

    // ----- Net galvanic-continuity analysis -----

    /// A net whose pads are all tied together by trace copper is a single
    /// galvanic island: continuous, full pad coverage. A PASS may stand.
    #[test]
    fn continuity_single_plane_is_continuous() {
        let mut pcb = clean_pcb();
        pcb.traces.clear();
        pcb.vias.clear();
        pcb.footprints = vec![
            footprint(
                "U1",
                (20.0, 40.0),
                0.0,
                vec![smd_pad("1", (0.0, 0.0), "3V3")],
            ),
            footprint(
                "U2",
                (35.0, 40.0),
                0.0,
                vec![smd_pad("1", (0.0, 0.0), "3V3")],
            ),
            footprint(
                "U3",
                (50.0, 40.0),
                0.0,
                vec![smd_pad("1", (0.0, 0.0), "3V3")],
            ),
        ];
        pcb.traces = vec![
            Trace {
                start: Vec2::new(20.0, 40.0),
                end: Vec2::new(35.0, 40.0),
                width: 0.25,
                layer: PcbLayer::FCu,
                net: "3V3".to_string(),
                source: None,
            },
            Trace {
                start: Vec2::new(35.0, 40.0),
                end: Vec2::new(50.0, 40.0),
                width: 0.25,
                layer: PcbLayer::FCu,
                net: "3V3".to_string(),
                source: None,
            },
        ];

        let c = analyze_net_continuity(&pcb, "3V3");
        assert!(c.realized, "net has copper");
        assert!(c.continuous, "one connected blob");
        assert_eq!(c.islands, 1);
        assert_eq!(c.total_pads, 3);
        assert_eq!(c.connected_pads, 3);
        assert!((c.coverage - 1.0).abs() < 1e-9);
        assert!(c.worst_island.is_none());
    }

    /// A net split into disjoint copper groups (the +3V3-into-N-islands failure)
    /// is NOT continuous, and the analysis surfaces partial pad coverage plus
    /// the worst stranded island — the stats a PASS-gate refuses on.
    #[test]
    fn continuity_split_plane_reports_coverage_and_worst_island() {
        let mut pcb = clean_pcb();
        pcb.traces.clear();
        pcb.vias.clear();
        // Three 3V3 pads; a trace ties two together, the third is stranded.
        pcb.footprints = vec![
            footprint(
                "U1",
                (20.0, 40.0),
                0.0,
                vec![smd_pad("1", (0.0, 0.0), "3V3")],
            ),
            footprint(
                "U2",
                (30.0, 40.0),
                0.0,
                vec![smd_pad("1", (0.0, 0.0), "3V3")],
            ),
            footprint(
                "U3",
                (80.0, 40.0),
                0.0,
                vec![smd_pad("1", (0.0, 0.0), "3V3")],
            ),
        ];
        pcb.traces = vec![Trace {
            start: Vec2::new(20.0, 40.0),
            end: Vec2::new(30.0, 40.0),
            width: 0.25,
            layer: PcbLayer::FCu,
            net: "3V3".to_string(),
            source: None,
        }];

        let c = analyze_net_continuity(&pcb, "3V3");
        assert!(c.realized);
        assert!(!c.continuous, "stranded pad => not a single plane");
        assert_eq!(c.islands, 2);
        assert_eq!(c.total_pads, 3);
        assert_eq!(c.connected_pads, 2, "main island holds the two tied pads");
        assert!((c.coverage - 2.0 / 3.0).abs() < 1e-9);
        let worst = c.worst_island.expect("a stranded island exists");
        assert_eq!(worst.pad_count, 1, "the stranded pad");
    }

    /// A net with no realized copper cannot be verified — `realized` is false,
    /// which the impedance/PDN gates treat as "unverifiable".
    #[test]
    fn continuity_unrealized_net_is_not_realized() {
        let mut pcb = clean_pcb();
        pcb.traces.clear();
        pcb.vias.clear();
        let c = analyze_net_continuity(&pcb, "NOWHERE");
        assert_eq!(c.islands, 0);
        assert!(!c.realized);
        assert!(!c.continuous);
        assert_eq!(c.total_pads, 0);
    }

    #[test]
    fn power_net_name_heuristic() {
        for t in [
            "+3V3", "GND", "VCC", "3V3", "5V", "-12V", "VBUS", "1V8", "AGND", "vdd",
        ] {
            assert!(is_power_net(t), "{t} should read as power");
        }
        for f in ["SCL", "MISO", "RESET", "D0", "USB_DP", "CLK", "TX"] {
            assert!(!is_power_net(f), "{f} should NOT read as power");
        }
        // Punctuation is not electrical: the same rail spelled with separators
        // reads the same. A motor controller's battery input is usually its
        // highest-current net, and `V_SUPPLY` was being missed on the underscore.
        for t in ["V_SUPPLY", "V-SUPPLY", "VSUPPLY", "P_GND", "V_MOT", "+3.3V"] {
            assert!(is_power_net(t), "{t} should read as power");
        }
    }

    /// `analyze_power_integrity` auto-selects poured/power nets and flags a
    /// plane that fragmented into multiple pours.
    #[test]
    fn power_integrity_flags_fragmented_pour() {
        let mut pcb = clean_pcb();
        pcb.traces.clear();
        pcb.vias.clear();
        pcb.nets.push(Net {
            id: "3V3".to_string(),
            name: "3V3".to_string(),
        });
        let zone = |verts: Vec<Vec2>| Zone {
            outline: verts,
            holes: vec![],
            net: "3V3".to_string(),
            layer: PcbLayer::FCu,
            clearance: 0.2,
            min_area: 0.0,
            fill_type: ZoneFillType::Solid,
            thermal_relief: ThermalReliefStyle::Relief,
            thermal_gap: Some(0.5),
            thermal_spoke_width: Some(0.5),
            priority: 0,
        };
        // Two separate, non-touching 3V3 pours → two galvanic islands.
        pcb.zones = vec![
            zone(vec![
                Vec2::new(5.0, 5.0),
                Vec2::new(25.0, 5.0),
                Vec2::new(25.0, 25.0),
                Vec2::new(5.0, 25.0),
            ]),
            zone(vec![
                Vec2::new(60.0, 50.0),
                Vec2::new(90.0, 50.0),
                Vec2::new(90.0, 70.0),
                Vec2::new(60.0, 70.0),
            ]),
        ];

        let report = analyze_power_integrity(&pcb);
        let v33 = report
            .iter()
            .find(|c| c.net == "3V3")
            .expect("3V3 is poured + power-named");
        assert_eq!(v33.islands, 2, "two disjoint pours");
        assert!(!v33.continuous);
    }

    // ----- NetIslands DRC rule -----

    /// A power net whose inner-layer plane stitches to nothing is two
    /// galvanically-isolated copper groups — the exact defect the FC session
    /// missed. Adding the stitching via merges the plane into the net and the
    /// violation clears. (The plane island carries no pads; its FCu pads form
    /// the other island.)
    #[test]
    fn net_islands_flags_unstitched_plane() {
        let mut pcb = clean_pcb();
        pcb.traces.clear();
        pcb.vias.clear();
        pcb.nets.push(Net {
            id: "3".to_string(),
            name: "3V3".to_string(),
        });
        // Two 3V3 pads joined by an FCu trace — one connected group.
        pcb.footprints.push(footprint(
            "U1",
            (20.0, 40.0),
            0.0,
            vec![smd_pad("1", (0.0, 0.0), "3")],
        ));
        pcb.footprints.push(footprint(
            "U2",
            (40.0, 40.0),
            0.0,
            vec![smd_pad("1", (0.0, 0.0), "3")],
        ));
        pcb.traces.push(trace((20.0, 40.0), (40.0, 40.0), "3"));
        // A 3V3 plane on In1Cu that connects to nothing — no stitching via.
        pcb.zones.push(Zone {
            outline: pcb.outline.vertices.clone(),
            holes: vec![],
            net: "3".to_string(),
            layer: PcbLayer::In1Cu,
            clearance: 0.2,
            min_area: 0.0,
            fill_type: ZoneFillType::Solid,
            thermal_relief: ThermalReliefStyle::Relief,
            thermal_gap: Some(0.5),
            thermal_spoke_width: Some(0.5),
            priority: 0,
        });

        let islands: Vec<_> = check_drc(&pcb)
            .into_iter()
            .filter(|v| v.rule == DrcRuleType::NetIslands && v.message.contains("3V3"))
            .collect();
        assert_eq!(
            islands.len(),
            1,
            "exactly one NetIslands violation for 3V3, got: {islands:?}"
        );
        assert_eq!(
            islands[0].actual, 2.0,
            "FCu pad group + floating plane = 2 islands"
        );
        assert!(
            islands[0].message.contains("U1.1") && islands[0].message.contains("U2.1"),
            "island pad list should name the pads: {}",
            islands[0].message
        );
        assert!(
            islands[0].message.contains("copper only"),
            "the floating plane island should be pad-less: {}",
            islands[0].message
        );

        // Stitch the plane to the net with a via FCu→In1Cu over the trace.
        pcb.vias.push(Via {
            position: Vec2::new(30.0, 40.0),
            diameter: 0.8,
            drill: 0.4,
            start_layer: PcbLayer::FCu,
            end_layer: PcbLayer::In1Cu,
            net: "3".to_string(),
            source: None,
        });
        let after: Vec<_> = check_drc(&pcb)
            .into_iter()
            .filter(|v| v.rule == DrcRuleType::NetIslands)
            .collect();
        assert!(
            after.is_empty(),
            "a stitched plane should be one island, got: {after:?}"
        );
    }

    /// The island count is faithful: three mutually-disconnected stubs on one
    /// net report exactly three islands, even with no pads (a case
    /// `UnconnectedNet`, which only counts pad groups, would miss entirely).
    #[test]
    fn net_islands_counts_every_disjoint_group() {
        let mut pcb = clean_pcb();
        pcb.traces.clear();
        pcb.vias.clear();
        pcb.nets.push(Net {
            id: "9".to_string(),
            name: "SIG".to_string(),
        });
        // Three short FCu stubs 30mm apart — far beyond the touch threshold.
        pcb.traces.push(trace((10.0, 10.0), (15.0, 10.0), "9"));
        pcb.traces.push(trace((10.0, 40.0), (15.0, 40.0), "9"));
        pcb.traces.push(trace((10.0, 70.0), (15.0, 70.0), "9"));

        let islands: Vec<_> = check_drc(&pcb)
            .into_iter()
            .filter(|v| v.rule == DrcRuleType::NetIslands && v.message.contains("SIG"))
            .collect();
        assert_eq!(islands.len(), 1, "one rolled-up violation per net");
        assert_eq!(
            islands[0].actual, 3.0,
            "three disjoint stubs = three islands"
        );
    }

    /// A correctly routed net (every piece of its copper galvanically joined)
    /// produces no NetIslands violation — it is realized as one island.
    #[test]
    fn net_islands_clean_for_connected_net() {
        let mut pcb = clean_pcb();
        pcb.traces.clear();
        pcb.vias.clear();
        pcb.footprints.push(footprint(
            "J1",
            (10.0, 40.0),
            0.0,
            vec![smd_pad("1", (0.0, 0.0), "1")],
        ));
        pcb.footprints.push(footprint(
            "J2",
            (30.0, 40.0),
            0.0,
            vec![smd_pad("1", (0.0, 0.0), "1")],
        ));
        pcb.traces.push(trace((10.0, 40.0), (30.0, 40.0), "1"));

        assert!(
            !check_drc(&pcb)
                .iter()
                .any(|v| v.rule == DrcRuleType::NetIslands),
            "a fully connected net is one island"
        );
    }

    /// Unit-level: a pour that floods into two physically-disjoint pieces splits
    /// into two island ring sets, each preserving its outer + hole rings.
    #[test]
    fn split_pour_islands_separates_disjoint_pieces() {
        // CCW square A, its CW hole, then CCW square B (a second island).
        let square = |ox: f64, oy: f64| {
            vec![
                Vec2::new(ox, oy),
                Vec2::new(ox + 5.0, oy),
                Vec2::new(ox + 5.0, oy + 5.0),
                Vec2::new(ox, oy + 5.0),
            ]
        };
        // Hole is the same square wound clockwise (reverse order).
        let mut hole = square(1.0, 1.0);
        hole.reverse();
        let rings = vec![square(0.0, 0.0), hole, square(20.0, 0.0)];
        let islands = split_pour_islands(&rings);
        assert_eq!(islands.len(), 2, "two CCW outers ⇒ two islands");
        assert_eq!(islands[0].len(), 2, "first island keeps its outer + hole");
        assert_eq!(islands[1].len(), 1, "second island is a bare outer");
    }

    /// Helper: a whole-board plane pour for `net` on `layer`.
    fn plane_zone(outline: &[Vec2], net: &str, layer: PcbLayer) -> Zone {
        Zone {
            outline: outline.to_vec(),
            holes: vec![],
            net: net.to_string(),
            layer,
            clearance: 0.2,
            min_area: 0.0,
            fill_type: ZoneFillType::Solid,
            thermal_relief: ThermalReliefStyle::Relief,
            thermal_gap: Some(0.5),
            thermal_spoke_width: Some(0.5),
            priority: 0,
        }
    }

    /// An SMD pad naming an inner-plane net, with no stitching via, has no
    /// galvanic path to its plane — a first-class Unstitched-Pad violation that
    /// names the exact pad and a suggested escape vector (not just an opaque
    /// net-wide UnconnectedNet).
    #[test]
    fn unstitched_smd_pad_on_plane_flagged() {
        let mut pcb = clean_pcb();
        pcb.traces.clear();
        pcb.vias.clear();
        pcb.nets.push(Net {
            id: "3".to_string(),
            name: "3V3".to_string(),
        });
        let outline = pcb.outline.vertices.clone();
        pcb.zones.push(plane_zone(&outline, "3", PcbLayer::In1Cu));
        // One SMD pad on FCu naming the +3V3 plane net, no via.
        pcb.footprints = vec![footprint(
            "U1",
            (50.0, 40.0),
            0.0,
            vec![smd_pad("1", (0.0, 0.0), "3")],
        )];

        let v = check_drc(&pcb);
        let unstitched: Vec<_> = v
            .iter()
            .filter(|x| x.rule == DrcRuleType::UnstitchedPad)
            .collect();
        assert_eq!(
            unstitched.len(),
            1,
            "expected one Unstitched-Pad, got: {:?}",
            v.iter().map(|x| (&x.rule, &x.message)).collect::<Vec<_>>()
        );
        assert!(
            unstitched[0].message.contains("U1.1") && unstitched[0].message.contains("'3'"),
            "violation must name the pad and plane net: {}",
            unstitched[0].message
        );
    }

    /// Dropping a same-net stitching via at the pad bridges FCu→In1Cu and clears
    /// the Unstitched-Pad violation — read straight off the connectivity union.
    #[test]
    fn stitching_via_clears_unstitched_pad() {
        let mut pcb = clean_pcb();
        pcb.traces.clear();
        pcb.vias.clear();
        pcb.nets.push(Net {
            id: "3".to_string(),
            name: "3V3".to_string(),
        });
        let outline = pcb.outline.vertices.clone();
        pcb.zones.push(plane_zone(&outline, "3", PcbLayer::In1Cu));
        pcb.footprints = vec![footprint(
            "U1",
            (50.0, 40.0),
            0.0,
            vec![smd_pad("1", (0.0, 0.0), "3")],
        )];
        // The stitching via: a +3V3 through-via at the pad reaching the In1Cu plane.
        pcb.vias.push(Via {
            position: Vec2::new(50.0, 40.0),
            diameter: 0.8,
            drill: 0.4,
            start_layer: PcbLayer::FCu,
            end_layer: PcbLayer::BCu,
            net: "3".to_string(),
            source: None,
        });

        let unstitched = check_drc(&pcb)
            .into_iter()
            .filter(|x| x.rule == DrcRuleType::UnstitchedPad)
            .count();
        assert_eq!(unstitched, 0, "a stitching via must clear the violation");
    }

    // ----- SameNetBypass DRC rule -----

    /// Archimedean spiral matching the MCP `add_coil` geometry (48 segments
    /// per turn, r linear in θ, coordinates rounded to 1e-3 mm), pushed as
    /// chained FCu traces. Returns `(inner endpoint, outer endpoint)`.
    fn add_spiral(
        pcb: &mut Pcb,
        center: Vec2,
        turns: usize,
        inner_r: f64,
        outer_r: f64,
        width: f64,
        net: &str,
    ) -> (Vec2, Vec2) {
        let round3 = |v: f64| (v * 1000.0).round() / 1000.0;
        let steps = turns * 48;
        let mut pts: Vec<Vec2> = Vec::new();
        for s in 0..=steps {
            let t = s as f64 / steps as f64;
            let theta = t * turns as f64 * std::f64::consts::TAU;
            let r = inner_r + t * (outer_r - inner_r);
            let p = Vec2::new(
                round3(center.x + r * theta.cos()),
                round3(center.y + r * theta.sin()),
            );
            if pts.last() != Some(&p) {
                pts.push(p);
            }
        }
        for w in pts.windows(2) {
            pcb.traces.push(Trace {
                start: w[0],
                end: w[1],
                width,
                layer: PcbLayer::FCu,
                net: net.to_string(),
                source: None,
            });
        }
        (pts[0], *pts.last().expect("spiral has points"))
    }

    /// A 10-turn coil (the `add_coil` test geometry: inner r 2.6, outer r 7.2,
    /// 0.25mm trace) with its inner terminal via, plus the field failure: a
    /// SAME-NET trace running from the outer endpoint straight along the
    /// terminal ray, over the inner via and across the turns. No net-based
    /// rule can see it, but it short-circuits the spiral → SameNetBypass.
    #[test]
    fn same_net_bypass_flags_trace_over_coil_inner_via() {
        let mut pcb = clean_pcb();
        pcb.traces.clear();
        pcb.vias.clear();
        pcb.nets.push(Net {
            id: "COIL".to_string(),
            name: "COIL".to_string(),
        });
        let center = Vec2::new(50.0, 40.0);
        let (inner, outer) = add_spiral(&mut pcb, center, 10, 2.6, 7.2, 0.25, "COIL");
        assert!((inner.x - 52.6).abs() < 1e-9 && (inner.y - 40.0).abs() < 1e-9);
        assert!((outer.x - 57.2).abs() < 1e-9 && (outer.y - 40.0).abs() < 1e-9);
        // The coil's inner terminal via.
        pcb.vias.push(Via {
            position: inner,
            diameter: 0.6,
            drill: 0.3,
            start_layer: PcbLayer::FCu,
            end_layer: PcbLayer::BCu,
            net: "COIL".to_string(),
            source: None,
        });
        // The bypass: same net, straight from the outer endpoint along the
        // terminal ray, over the inner via.
        pcb.traces.push(Trace {
            start: outer,
            end: Vec2::new(51.0, 40.0),
            width: 0.25,
            layer: PcbLayer::FCu,
            net: "COIL".to_string(),
            source: None,
        });

        let v = check_drc(&pcb);
        let bypass: Vec<_> = v
            .iter()
            .filter(|x| x.rule == DrcRuleType::SameNetBypass)
            .collect();
        assert!(
            !bypass.is_empty(),
            "a same-net trace over the coil body/inner via must warn, got: {:?}",
            v.iter().map(|x| (&x.rule, &x.message)).collect::<Vec<_>>()
        );
        assert!(
            bypass.iter().all(|x| x.severity == DrcSeverity::Warning),
            "same-net bypass is a warning, not an error"
        );
        assert!(
            bypass.iter().any(|x| x.message.contains("'COIL'")),
            "message names the net: {:?}",
            bypass[0].message
        );
        // The via overlap itself is among the flagged contacts.
        assert!(
            bypass
                .iter()
                .any(|x| (x.position.x - 52.6).abs() < 0.5 && (x.position.y - 40.0).abs() < 0.5),
            "the inner-via contact must be flagged, positions: {:?}",
            bypass.iter().map(|x| x.position).collect::<Vec<_>>()
        );
    }

    /// The sanctioned lead-out: the same spiral + inner via, but the lead
    /// leaves the outer terminal jogged 5° off the terminal ray, heading
    /// outward — it meets the coil only at the shared terminal endpoint.
    /// Endpoint-chained copper is an intended junction → no warning.
    #[test]
    fn same_net_bypass_clear_for_jogged_lead() {
        let mut pcb = clean_pcb();
        pcb.traces.clear();
        pcb.vias.clear();
        pcb.nets.push(Net {
            id: "COIL".to_string(),
            name: "COIL".to_string(),
        });
        let center = Vec2::new(50.0, 40.0);
        let (inner, outer) = add_spiral(&mut pcb, center, 10, 2.6, 7.2, 0.25, "COIL");
        pcb.vias.push(Via {
            position: inner,
            diameter: 0.6,
            drill: 0.3,
            start_layer: PcbLayer::FCu,
            end_layer: PcbLayer::BCu,
            net: "COIL".to_string(),
            source: None,
        });
        // Jogged approach: outer endpoint → r 9.5 at +5° off the terminal ray.
        let jog = 5.0_f64.to_radians();
        pcb.traces.push(Trace {
            start: outer,
            end: Vec2::new(center.x + 9.5 * jog.cos(), center.y + 9.5 * jog.sin()),
            width: 0.25,
            layer: PcbLayer::FCu,
            net: "COIL".to_string(),
            source: None,
        });

        let bypass: Vec<_> = check_drc(&pcb)
            .into_iter()
            .filter(|x| x.rule == DrcRuleType::SameNetBypass)
            .collect();
        assert!(
            bypass.is_empty(),
            "a jogged lead touching only the terminal must not warn, got: {bypass:?}"
        );
    }

    /// A GND via-stitched pour stays silent: stitching vias reach the rest of
    /// the net through the pour (an intended junction), and a via dropped
    /// mid-trace resolves within the hop limit via trace → end-via → pour.
    #[test]
    fn same_net_bypass_clear_for_via_stitched_pour() {
        let mut pcb = clean_pcb();
        pcb.traces.clear();
        pcb.vias.clear();
        // GND (net "2") pour on BCu.
        pcb.zones.push(plane_zone(
            &[
                Vec2::new(10.0, 10.0),
                Vec2::new(70.0, 10.0),
                Vec2::new(70.0, 70.0),
                Vec2::new(10.0, 70.0),
            ],
            "2",
            PcbLayer::BCu,
        ));
        // A GND trace on FCu, terminated by a stitching via into the plane…
        pcb.traces.push(trace((20.0, 40.0), (40.0, 40.0), "2"));
        let via = |x: f64, y: f64| Via {
            position: Vec2::new(x, y),
            diameter: 0.8,
            drill: 0.4,
            start_layer: PcbLayer::FCu,
            end_layer: PcbLayer::BCu,
            net: "2".to_string(),
            source: None,
        };
        pcb.vias.push(via(40.0, 40.0));
        // …a stitching via dropped on the trace BODY (not an endpoint)…
        pcb.vias.push(via(30.0, 40.0));
        // …and a free-standing stitching-via array on the pour.
        pcb.vias.push(via(55.0, 55.0));
        pcb.vias.push(via(58.0, 55.0));
        pcb.vias.push(via(55.0, 58.0));

        let bypass: Vec<_> = check_drc(&pcb)
            .into_iter()
            .filter(|x| x.rule == DrcRuleType::SameNetBypass)
            .collect();
        assert!(
            bypass.is_empty(),
            "stitched pours and via arrays are not bypasses, got: {bypass:?}"
        );
    }

    /// A same-net contact inside a declared net-tie region is by design (a
    /// star point / neutral bar): the identical geometry warns without the
    /// tie and stays silent with it.
    #[test]
    fn same_net_bypass_respects_net_tie_regions() {
        let build = || {
            let mut pcb = clean_pcb();
            pcb.traces.clear();
            pcb.vias.clear();
            // A 6-segment U-shaped chain on net "1"…
            pcb.traces.push(trace((10.0, 10.0), (20.0, 10.0), "1"));
            pcb.traces.push(trace((20.0, 10.0), (30.0, 10.0), "1"));
            pcb.traces.push(trace((30.0, 10.0), (30.0, 20.0), "1"));
            pcb.traces.push(trace((30.0, 20.0), (20.0, 20.0), "1"));
            pcb.traces.push(trace((20.0, 20.0), (10.0, 20.0), "1"));
            // …whose last segment loops back onto the FIRST segment's body
            // (5 hops away along the chain — a bypass of the whole U).
            pcb.traces.push(trace((10.0, 20.0), (15.0, 10.0), "1"));
            pcb
        };

        let flagged = check_drc(&build())
            .into_iter()
            .filter(|x| x.rule == DrcRuleType::SameNetBypass)
            .count();
        assert!(
            flagged > 0,
            "a loop-back onto the chain's far end must warn"
        );

        let mut tied = build();
        tied.net_ties.push(NetTie {
            nets: vec!["1".to_string(), "2".to_string()],
            position: Some(Vec2::new(15.0, 10.0)),
            radius: Some(2.0),
        });
        let flagged = check_drc(&tied)
            .into_iter()
            .filter(|x| x.rule == DrcRuleType::SameNetBypass)
            .count();
        assert_eq!(flagged, 0, "the same contact inside a tie region is exempt");
    }

    /// A T-junction — a stub whose endpoint lands on another trace's body and
    /// is the net's ONLY link between the two pieces — is load-bearing, not a
    /// bypass: nothing is short-circuited because nothing else connects them.
    #[test]
    fn same_net_bypass_ignores_t_junction() {
        let mut pcb = clean_pcb();
        pcb.traces.clear();
        pcb.vias.clear();
        // A bus and a stub tapping its middle (net "1").
        pcb.traces.push(trace((10.0, 40.0), (40.0, 40.0), "1"));
        pcb.traces.push(trace((25.0, 40.0), (25.0, 50.0), "1"));

        let bypass: Vec<_> = check_drc(&pcb)
            .into_iter()
            .filter(|x| x.rule == DrcRuleType::SameNetBypass)
            .collect();
        assert!(
            bypass.is_empty(),
            "a T-junction is the net's link, not a bypass, got: {bypass:?}"
        );
    }

    /// A THT pad's plated barrel already bridges to an inner plane, so the rule
    /// is scoped to SMD pads: a THT pad naming a plane net is never flagged.
    #[test]
    fn tht_pad_on_plane_not_flagged() {
        let mut pcb = clean_pcb();
        pcb.traces.clear();
        pcb.vias.clear();
        pcb.nets.push(Net {
            id: "3".to_string(),
            name: "3V3".to_string(),
        });
        let outline = pcb.outline.vertices.clone();
        pcb.zones.push(plane_zone(&outline, "3", PcbLayer::In1Cu));
        let mut th = smd_pad("1", (0.0, 0.0), "3");
        th.pad_type = PadType::THT;
        th.layers = vec![PcbLayer::FCu, PcbLayer::BCu];
        pcb.footprints = vec![footprint("J1", (50.0, 40.0), 0.0, vec![th])];

        let unstitched = check_drc(&pcb)
            .into_iter()
            .filter(|x| x.rule == DrcRuleType::UnstitchedPad)
            .count();
        assert_eq!(
            unstitched, 0,
            "THT pads bridge layers — never Unstitched-Pad"
        );
    }

    /// Board with two spatially-separate clearance faults (near (15,10) and
    /// (80,70)) plus a copper short far from both, for the region-scope tests.
    fn pcb_with_scattered_faults() -> Pcb {
        let mut pcb = clean_pcb();
        let seg = |x0: f64, y0: f64, x1: f64, y1: f64, net: &str| Trace {
            start: Vec2::new(x0, y0),
            end: Vec2::new(x1, y1),
            width: 0.25,
            layer: PcbLayer::FCu,
            net: net.to_string(),
            source: None,
        };
        // Pair A (in the scoped region): 0.35mm center gap ⇒ 0.1mm edge gap
        // < 0.2mm clearance, but not touching (no short).
        pcb.traces.push(seg(10.0, 10.0, 20.0, 10.0, "3"));
        pcb.traces.push(seg(10.0, 10.35, 20.0, 10.35, "4"));
        // Pair B (outside the region): same fault, other corner of the board.
        pcb.traces.push(seg(70.0, 70.0, 90.0, 70.0, "5"));
        pcb.traces.push(seg(70.0, 70.35, 90.0, 70.35, "6"));
        // A hard short (touching cross-net copper), also outside the region.
        pcb.traces.push(seg(70.0, 20.0, 80.0, 20.0, "7"));
        pcb.traces.push(seg(80.0, 20.0, 90.0, 20.0, "8"));
        pcb
    }

    /// `check_drc_in_region` keeps only in-region subjects for the geometric
    /// checks (pair A's clearance faults, not pair B's) while connectivity
    /// stays board-global (the far-away short is still reported).
    #[test]
    fn scoped_drc_filters_geometry_but_keeps_connectivity_global() {
        let pcb = pcb_with_scattered_faults();

        let full = check_drc(&pcb);
        // Each too-close trace pair is judged from both subject traces; the
        // touching pair additionally violates clearance. 3 pairs × 2 = 6.
        assert_eq!(
            full.iter()
                .filter(|v| v.rule == DrcRuleType::Clearance)
                .count(),
            6
        );
        assert_eq!(
            full.iter().filter(|v| v.rule == DrcRuleType::Short).count(),
            1
        );

        let scoped = check_drc_in_region(&pcb, Vec2::new(5.0, 5.0), Vec2::new(25.0, 15.0));
        let clearance: Vec<_> = scoped
            .iter()
            .filter(|v| v.rule == DrcRuleType::Clearance)
            .collect();
        assert_eq!(clearance.len(), 2, "only pair A is in scope");
        assert!(clearance.iter().all(|v| v.position.y < 20.0));
        assert_eq!(
            scoped
                .iter()
                .filter(|v| v.rule == DrcRuleType::Short)
                .count(),
            1,
            "shorts come from the global connectivity pass regardless of scope"
        );
    }

    /// A region covering the whole board reproduces the full run exactly —
    /// scoping changes subject selection, never rule logic.
    #[test]
    fn scoped_drc_covering_board_matches_full_run() {
        let pcb = pcb_with_scattered_faults();
        let full = check_drc(&pcb);
        let scoped = check_drc_in_region(&pcb, Vec2::new(-10.0, -10.0), Vec2::new(110.0, 90.0));
        assert_eq!(full, scoped);
    }
}
