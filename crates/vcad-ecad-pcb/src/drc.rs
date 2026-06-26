//! Design Rule Checking (DRC) engine.
//!
//! Validates a PCB layout against its design rules and reports violations.
//! Uses the spatial index from [`crate::spatial`] for efficient proximity queries.

use std::collections::HashMap;

use vcad_ir::ecad::{Footprint, FootprintGraphic, Pad, PadShape, PadType, Pcb, PcbLayer};
use vcad_ir::Vec2;

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
}

/// DRC violation severity.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub enum DrcSeverity {
    /// Must be fixed before fabrication.
    Error,
    /// Should be reviewed but may be acceptable.
    Warning,
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
pub fn check_drc(pcb: &Pcb) -> Vec<DrcViolation> {
    let mut violations = Vec::new();
    let index = SpatialIndex::from_pcb(pcb);
    let net_ties = NetTieGroups::from_pcb(pcb);
    let dp_map = build_diff_pair_gap_map(pcb);

    check_clearance(pcb, &index, &net_ties, &dp_map, &mut violations);
    check_pad_clearance(pcb, &net_ties, &dp_map, &mut violations);
    check_min_trace_width(pcb, &mut violations);
    check_min_drill(pcb, &mut violations);
    check_edge_clearance(pcb, &mut violations);
    check_hole_to_hole(pcb, &mut violations);
    check_annular_ring(pcb, &mut violations);
    check_keepout(pcb, &mut violations);
    check_courtyard_overlap(pcb, &mut violations);
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
fn check_courtyard_overlap(pcb: &Pcb, violations: &mut Vec<DrcViolation>) {
    let boxes: Vec<(usize, (Vec2, Vec2))> = pcb
        .footprints
        .iter()
        .enumerate()
        .map(|(i, fp)| (i, courtyard_bounds(fp)))
        .collect();

    for a in 0..boxes.len() {
        for b in (a + 1)..boxes.len() {
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
                });
            }
        }
    }
}

// ============================================================================
// Net-tie grouping (intentional net junctions)
// ============================================================================

/// A single net-tie group with an optional spatial restriction.
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
        if a == b {
            return true;
        }
        for g in &self.groups {
            let joins = g.nets.iter().any(|n| n == a) && g.nets.iter().any(|n| n == b);
            if !joins {
                continue;
            }
            match g.region {
                None => return true,
                Some((c, r2)) => {
                    let dx = at.x - c.x;
                    let dy = at.y - c.y;
                    if dx * dx + dy * dy <= r2 {
                        return true;
                    }
                }
            }
        }
        false
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
    dp_map: &HashMap<(String, String), f64>,
    violations: &mut Vec<DrcViolation>,
) {
    let default_clearance = pcb.rules.default_rules.clearance;

    // Build net class clearance lookup
    let net_clearance = build_net_clearance_map(pcb);

    // Check each trace against nearby elements on the same layer
    for trace in &pcb.traces {
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
            let required = pair_aware_clearance(dp_map, &trace.net, &elem.net, clearance);
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
    dp_map: &HashMap<(String, String), f64>,
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
    }

    let mut boxes: Vec<PadBox> = Vec::new();
    for fp in &pcb.footprints {
        for pad in &fp.pads {
            // Pads without a net can't short two nets together.
            let Some(net) = pad.net.as_deref() else {
                continue;
            };
            let center = Vec2::new(
                fp.position.x + pad.position.x,
                fp.position.y + pad.position.y,
            );
            let rot = (fp.rotation + pad.rotation).to_radians();
            boxes.push(PadBox {
                center,
                geom: pad_geom(pad, center, rot),
                net,
                layers: &pad.layers,
                fp_ref: &fp.reference,
                number: &pad.number,
            });
        }
    }

    for i in 0..boxes.len() {
        for j in (i + 1)..boxes.len() {
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
            // Diff-pair pads only need their gap, not the full clearance.
            let clearance = pair_aware_clearance(dp_map, a.net, b.net, base);

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
                });
            }
        }
    }
}

/// Check that all traces meet the minimum trace width.
fn check_min_trace_width(pcb: &Pcb, violations: &mut Vec<DrcViolation>) {
    let net_width = build_net_trace_width_map(pcb);
    let default_width = pcb.rules.default_rules.trace_width;

    for trace in &pcb.traces {
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
            });
        }
    }
}

/// Check that all drills meet the minimum drill diameter.
fn check_min_drill(pcb: &Pcb, violations: &mut Vec<DrcViolation>) {
    let min_drill = pcb.rules.min_drill;

    // Check via drills
    for via in &pcb.vias {
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
            });
        }
    }

    // Check pad drills
    for footprint in &pcb.footprints {
        for pad in &footprint.pads {
            if let Some(drill) = &pad.drill {
                if drill.diameter < min_drill - 1e-6 {
                    let abs_pos = Vec2::new(
                        footprint.position.x + pad.position.x,
                        footprint.position.y + pad.position.y,
                    );
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
                    });
                }
            }
        }
    }
}

/// Check that all copper elements maintain edge clearance.
fn check_edge_clearance(pcb: &Pcb, violations: &mut Vec<DrcViolation>) {
    let edge_clearance = pcb.rules.edge_clearance;
    let outline = &pcb.outline.vertices;

    if outline.is_empty() {
        return;
    }

    // Check traces against board edges
    for trace in &pcb.traces {
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
                });
                break; // one violation per trace
            }
        }
    }

    // Check vias against board edges
    for via in &pcb.vias {
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
            });
        }
    }
}

/// Check hole-to-hole spacing.
fn check_hole_to_hole(pcb: &Pcb, violations: &mut Vec<DrcViolation>) {
    let min_spacing = pcb.rules.hole_to_hole;

    // Collect all hole positions and radii
    let mut holes: Vec<(Vec2, f64)> = Vec::new();

    for via in &pcb.vias {
        holes.push((via.position, via.drill / 2.0));
    }

    for footprint in &pcb.footprints {
        for pad in &footprint.pads {
            if let Some(drill) = &pad.drill {
                let abs_pos = Vec2::new(
                    footprint.position.x + pad.position.x,
                    footprint.position.y + pad.position.y,
                );
                holes.push((abs_pos, drill.diameter / 2.0));
            }
        }
    }

    // O(n^2) check — fine for typical PCB sizes; use spatial index for large boards
    for i in 0..holes.len() {
        for j in (i + 1)..holes.len() {
            let dx = holes[i].0.x - holes[j].0.x;
            let dy = holes[i].0.y - holes[j].0.y;
            let center_dist = (dx * dx + dy * dy).sqrt();
            let edge_dist = center_dist - holes[i].1 - holes[j].1;

            if edge_dist < min_spacing - 1e-6 {
                let mid = Vec2::new(
                    (holes[i].0.x + holes[j].0.x) / 2.0,
                    (holes[i].0.y + holes[j].0.y) / 2.0,
                );
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
                });
            }
        }
    }
}

/// Check annular ring width on through-hole pads and vias.
fn check_annular_ring(pcb: &Pcb, violations: &mut Vec<DrcViolation>) {
    let min_ring = pcb.rules.min_annular_ring;

    // Check vias
    for via in &pcb.vias {
        let ring = (via.diameter - via.drill) / 2.0;
        if ring < min_ring - 1e-6 {
            violations.push(DrcViolation {
                rule: DrcRuleType::AnnularRing,
                severity: DrcSeverity::Error,
                position: via.position,
                message: format!("Via annular ring {:.3}mm < {:.3}mm", ring, min_ring),
                actual: ring,
                required: min_ring,
            });
        }
    }

    // Check THT pads
    for footprint in &pcb.footprints {
        for pad in &footprint.pads {
            if pad.pad_type != PadType::THT {
                continue;
            }
            if let Some(drill) = &pad.drill {
                let pad_min_dim = pad_min_dimension(pad);
                let ring = (pad_min_dim - drill.diameter) / 2.0;
                if ring < min_ring - 1e-6 {
                    let abs_pos = Vec2::new(
                        footprint.position.x + pad.position.x,
                        footprint.position.y + pad.position.y,
                    );
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
                    });
                }
            }
        }
    }
}

/// Enforce keepout regions: no-tracks / no-vias / no-components.
fn check_keepout(pcb: &Pcb, violations: &mut Vec<DrcViolation>) {
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
                    });
                }
            }
        }

        // no_components: any footprint whose pads or courtyard fall inside.
        if keepout.no_components {
            for fp in &pcb.footprints {
                let mut hit = false;
                let mut hit_pos = fp.position;
                for pad in &fp.pads {
                    let pad_pos = Vec2::new(
                        fp.position.x + pad.position.x,
                        fp.position.y + pad.position.y,
                    );
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
    Pour(Vec<Vec<Vec2>>),
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

/// Even-odd point-in-pour test over the filled rings.
fn point_in_pour(rings: &[Vec<Vec2>], p: Vec2) -> bool {
    rings.iter().filter(|r| point_in_polygon(p, r)).count() % 2 == 1
}

/// Minimum distance from a point to the nearest edge of any ring.
fn min_dist_point_to_pour(p: Vec2, rings: &[Vec<Vec2>]) -> f64 {
    rings
        .iter()
        .map(|r| min_distance_to_polygon(&p, r))
        .fold(f64::MAX, f64::min)
}

/// True if a copper geom touches/overlaps a filled pour (even-odd over rings).
///
/// Copper that the plane floods over reads as inside (odd ring count); copper
/// sitting in a clearance void reads as outside (its hole adds an even count),
/// and is also a full `clearance` from the nearest void edge, so the proximity
/// check never false-connects it.
fn copper_touches_pour(g: &CopperGeom, rings: &[Vec<Vec2>]) -> bool {
    match g {
        CopperGeom::Disc { center, r } => {
            point_in_pour(rings, *center)
                || min_dist_point_to_pour(*center, rings) <= *r + TOUCH_EPS
        }
        CopperGeom::Segment { a, b, half_w } => {
            let mid = Vec2::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0);
            if point_in_pour(rings, *a) || point_in_pour(rings, *b) || point_in_pour(rings, mid) {
                return true;
            }
            min_dist_segment_to_pour(*a, *b, rings) <= *half_w + TOUCH_EPS
        }
        CopperGeom::Rect { center, .. } => {
            if point_in_pour(rings, *center) {
                return true;
            }
            rect_corners(g).iter().any(|c| point_in_pour(rings, *c))
        }
    }
}

/// Minimum distance from a segment to the nearest edge of any ring.
fn min_dist_segment_to_pour(a: Vec2, b: Vec2, rings: &[Vec<Vec2>]) -> f64 {
    rings
        .iter()
        .map(|r| min_dist_segment_to_polygon_edges(a, b, r))
        .fold(f64::MAX, f64::min)
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
fn pours_touch(a: &[Vec<Vec2>], b: &[Vec<Vec2>]) -> bool {
    if a.iter().flatten().any(|p| point_in_pour(b, *p))
        || b.iter().flatten().any(|p| point_in_pour(a, *p))
    {
        return true;
    }
    // Any ring edge of one crossing a ring of the other.
    for ra in a {
        for i in 0..ra.len() {
            let (s, e) = (ra[i], ra[(i + 1) % ra.len()]);
            if b.iter().any(|rb| segment_polygon_intersects(s, e, rb)) {
                return true;
            }
        }
    }
    false
}

/// Connectivity flood-fill: detects shorts (one component, ≥2 distinct nets)
/// and unrouted nets (one net split across ≥2 components).
fn check_connectivity(pcb: &Pcb, net_ties: &NetTieGroups, violations: &mut Vec<DrcViolation>) {
    let nodes = build_conn_nodes(pcb);
    if nodes.is_empty() {
        return;
    }

    // Union-find over geometric touch (same-layer, overlapping/touching).
    let mut dsu = Dsu::new(nodes.len());
    for i in 0..nodes.len() {
        for j in (i + 1)..nodes.len() {
            if nodes[i].touches(&nodes[j]) {
                dsu.union(i, j);
            }
        }
    }

    detect_shorts(&nodes, &mut dsu, net_ties, violations);
    detect_unrouted(pcb, &nodes, &mut dsu, net_ties, violations);
    detect_net_islands(pcb, &nodes, &mut dsu, violations);
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
            let center = Vec2::new(
                fp.position.x + pad.position.x,
                fp.position.y + pad.position.y,
            );
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
                geom: NodeGeom::Pour(island),
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

/// Emit a `Short` violation for any component carrying ≥2 distinct declared
/// nets that are not net-tied.
fn detect_shorts(
    nodes: &[ConnNode],
    dsu: &mut Dsu,
    net_ties: &NetTieGroups,
    violations: &mut Vec<DrcViolation>,
) {
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

    let mut seen_pairs: std::collections::HashSet<(String, String)> = Default::default();
    for nets in comp_nets.values() {
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
                let key = if na <= nb {
                    (na.clone(), nb.clone())
                } else {
                    (nb.clone(), na.clone())
                };
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
fn build_diff_pair_gap_map(pcb: &Pcb) -> HashMap<(String, String), f64> {
    let mut map = HashMap::new();
    for dp in diff_pairs(pcb) {
        let key = if dp.net_p <= dp.net_n {
            (dp.net_p.clone(), dp.net_n.clone())
        } else {
            (dp.net_n.clone(), dp.net_p.clone())
        };
        map.insert(key, dp.gap);
    }
    map
}

/// The required clearance between two nets: their diff-pair gap if they are a
/// declared pair, else `fallback`.
fn pair_aware_clearance(
    dp_map: &HashMap<(String, String), f64>,
    a: &str,
    b: &str,
    fallback: f64,
) -> f64 {
    let key = if a <= b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    };
    dp_map.get(&key).copied().unwrap_or(fallback)
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
                },
                Trace {
                    start: Vec2::new(20.0, 50.0),
                    end: Vec2::new(50.0, 50.0),
                    width: 0.25,
                    layer: PcbLayer::FCu,
                    net: "2".to_string(),
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

    #[test]
    fn detect_min_trace_width_violation() {
        let mut pcb = clean_pcb();
        pcb.traces.push(Trace {
            start: Vec2::new(20.0, 60.0),
            end: Vec2::new(50.0, 60.0),
            width: 0.1, // below 0.25 minimum
            layer: PcbLayer::FCu,
            net: "1".to_string(),
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
        });
        pcb.traces.push(Trace {
            start: Vec2::new(20.0, 40.3),
            end: Vec2::new(80.0, 40.3),
            width: 0.25,
            layer: PcbLayer::FCu,
            net: "2".to_string(),
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
}
