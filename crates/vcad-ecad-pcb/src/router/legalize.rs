//! Post-route legalization: the router's own output, made legal before it is
//! returned.
//!
//! The autorouter composes copper from several independent emitters (maze
//! routes, fan-out rescue vias, plane stitching, joint repair). Each emitter is
//! probed against the board, but their *combined* output can still contain
//! copper no one would ship — the three recurring field failures:
//!
//! 1. **Duplicate / overlapping vias** — two emitters drop a via for the same
//!    net at (nearly) the same spot, and the drills intersect: a `HoleToHole`
//!    DRC error with negative spacing.
//! 2. **Same-net bypass noodles** — a route brushes its own net's copper far
//!    from any intended junction, short-circuiting the conductor between the
//!    touch points: a `SameNetBypass` DRC warning.
//! 3. **Dangling copper** — traces and vias whose island reaches no pad and no
//!    pour, stranded by rip-up/restore: `NetIslands`, "copper only, no pads".
//!
//! [`legalize`] runs after routing settles and before the result is returned:
//! it merges same-net vias whose holes are hole-to-hole illegal (reconnecting
//! their traces), prunes redundant self-touching trace segments while a
//! continuity oracle proves the net stays connected, and finally demotes —
//! fail-closed — any net whose new copper is still hole-to-hole illegal.
//! [`prune_dangling`] then gets the last word, dropping the dead islands. The
//! invariant they enforce together: an autoroute pass on a clean placement
//! never introduces `HoleToHole` or `SameNetBypass` violations of its own
//! making, and never returns copper connected to nothing.

use std::collections::BTreeSet;

use vcad_ir::ecad::{CopperSource, Pcb, PcbLayer, Trace, Via};
use vcad_ir::Vec2;

use super::auto::{copper_layers, RoutedTrace, RoutedVia};
use crate::drc::{
    analyze_net_continuity, check_drc_in_region, dangling_copper_mask, DrcRuleType, DrcViolation,
};

/// Two points closer than this are the same routing coordinate.
const POS_EPS: f64 = 1e-4;

/// Iteration cap for the DRC-driven fix loop. Every iteration either merges,
/// prunes, demotes, or marks a violation unfixable, so this only bounds
/// pathological boards.
const MAX_FIX_ROUNDS: usize = 4;

/// What [`legalize`] did to the router's output.
#[derive(Debug, Default)]
pub(super) struct LegalizeReport {
    /// Vias removed by merging into a coincident/overlapping same-net via.
    pub merged_vias: usize,
    /// Trace segments pruned as redundant same-net bypass copper.
    pub pruned_traces: usize,
    /// Nets whose new copper was removed entirely because it remained
    /// hole-to-hole illegal after merging (fail-closed: better unrouted than
    /// unmanufacturable). Each entry carries the position of the offending via.
    pub demoted: Vec<(String, Vec2)>,
}

/// Legalize the router's flattened output in place. See the module docs.
pub(super) fn legalize(
    pcb: &Pcb,
    traces: &mut Vec<RoutedTrace>,
    vias: &mut Vec<RoutedVia>,
) -> LegalizeReport {
    let mut report = LegalizeReport::default();
    if traces.is_empty() && vias.is_empty() {
        return report;
    }

    report.merged_vias = merge_same_net_vias(pcb, traces, vias);

    // DRC-driven cleanup: judge the candidate board (existing copper + the
    // router's output) with the real checker and act only on violations the
    // new copper participates in. Loop because one fix can unmask another.
    let mut unfixable: BTreeSet<(u64, u64)> = BTreeSet::new();
    for _ in 0..MAX_FIX_ROUNDS {
        let candidate = candidate_pcb(pcb, traces, vias);
        let (min, max) = new_copper_bbox(traces, vias);
        let violations = check_drc_in_region(&candidate, min, max);

        let mut progressed = false;
        for v in &violations {
            match v.rule {
                DrcRuleType::HoleToHole => {
                    // Only act when one of the holes is a via we placed; a
                    // pad-vs-pad conflict is the placement's, not ours.
                    let Some(net) = new_via_net_at_holes(v, vias) else {
                        continue;
                    };
                    demote_net(&net, v.position, traces, vias, &mut report);
                    progressed = true;
                    break; // copper changed — re-judge from scratch
                }
                DrcRuleType::SameNetBypass => {
                    let key = pos_key(v.position);
                    if unfixable.contains(&key) {
                        continue;
                    }
                    if prune_bypass(pcb, traces, vias, v) {
                        report.pruned_traces += 1;
                        progressed = true;
                        break;
                    }
                    unfixable.insert(key);
                }
                _ => {}
            }
        }
        if !progressed {
            break;
        }
    }
    report
}

/// Drop the router's own dead copper: new traces and vias whose galvanic
/// island on the candidate board holds no pad and no pour fragment.
///
/// The debris comes from the emitters that add copper speculatively — dog-bone
/// escape vias for a connection rip-up later abandoned, fan-out stubs whose
/// route was replaced, stitching vias whose plane never materialized — and it
/// shows up in DRC as `NetIslands` "copper only, no pads" (4185 traces and 130
/// vias on one CM5 pass, 172 island violations down to 18 once removed).
///
/// Soundness — this can only remove electrically dead copper:
///
/// * Islands are *maximal* connected components, so they are disjoint. Deleting
///   an entire unanchored island cannot disconnect anything that stays, and the
///   single pass is already the fixpoint.
/// * Every connection the router counts as placed runs pad to pad, so its
///   copper lives in a pad-anchored island and is kept. `routability`, the
///   routed/unrouted net split, and the `pending` diagnostics therefore mean
///   exactly what they meant before the prune.
/// * Only *new* copper is dropped. Board copper — including anything the caller
///   placed by hand — is judged (it anchors islands) but never removed, which is
///   also what keeps the arc blind spot harmless: connectivity does not model
///   `trace_arcs`, but the router emits none and never deletes the board's.
///
/// Returns `(traces_removed, vias_removed)`.
pub(super) fn prune_dangling(
    pcb: &Pcb,
    traces: &mut Vec<RoutedTrace>,
    vias: &mut Vec<RoutedVia>,
) -> (usize, usize) {
    if traces.is_empty() && vias.is_empty() {
        return (0, 0);
    }
    // Judge the board as the caller will commit it: `candidate_pcb` appends the
    // new copper after the existing copper, so the new traces occupy
    // `pcb.traces.len()..` of the mask and the new vias `pcb.vias.len()..`.
    let candidate = candidate_pcb(pcb, traces, vias);
    let (keep_trace, keep_via) = dangling_copper_mask(&candidate);
    let (t0, v0) = (pcb.traces.len(), pcb.vias.len());

    let mut ti = 0;
    traces.retain(|_| {
        let k = keep_trace[t0 + ti];
        ti += 1;
        k
    });
    let mut vi = 0;
    vias.retain(|_| {
        let k = keep_via[v0 + vi];
        vi += 1;
        k
    });
    (
        keep_trace[t0..].iter().filter(|k| !**k).count(),
        keep_via[v0..].iter().filter(|k| !**k).count(),
    )
}

/// Quantized position, as a set key for the unfixable-violation cache.
fn pos_key(p: Vec2) -> (u64, u64) {
    (
        ((p.x * 1e4).round() as i64) as u64,
        ((p.y * 1e4).round() as i64) as u64,
    )
}

/// Via pad/drill geometry for `net` from its net class, defaulting to the
/// board's default rules (mirrors how the committed via is sized).
fn via_geom_for(pcb: &Pcb, net: &str) -> (f64, f64) {
    let rules = &pcb.rules;
    for (class, nets) in &rules.net_class_assignments {
        if nets.iter().any(|n| n == net) {
            if let Some(c) = rules.class_rules.iter().find(|c| c.name == *class) {
                return (c.via_diameter, c.via_drill);
            }
        }
    }
    (
        rules.default_rules.via_diameter,
        rules.default_rules.via_drill,
    )
}

fn d2(a: Vec2, b: Vec2) -> f64 {
    let (dx, dy) = (a.x - b.x, a.y - b.y);
    dx * dx + dy * dy
}

/// Merge same-net new vias whose drilled holes are closer than the
/// hole-to-hole rule allows (including exact duplicates). The survivor takes
/// the union of the cluster's layer spans; traces that ended on a dropped via
/// are reconnected to the survivor with a short same-net jog on their own
/// layer. Returns the number of vias removed.
fn merge_same_net_vias(
    pcb: &Pcb,
    traces: &mut Vec<RoutedTrace>,
    vias: &mut Vec<RoutedVia>,
) -> usize {
    if vias.len() < 2 {
        return 0;
    }
    let min_spacing = pcb.rules.hole_to_hole;
    let stack = copper_layers(pcb);
    let layer_idx = |l: PcbLayer| stack.iter().position(|&s| s == l).unwrap_or(0);

    // Union-find over the new vias: same net + illegal hole spacing → cluster.
    let mut parent: Vec<usize> = (0..vias.len()).collect();
    fn find(parent: &mut [usize], i: usize) -> usize {
        let mut r = i;
        while parent[r] != r {
            r = parent[r];
        }
        let mut c = i;
        while parent[c] != r {
            let next = parent[c];
            parent[c] = r;
            c = next;
        }
        r
    }
    for i in 0..vias.len() {
        let (_, drill_i) = via_geom_for(pcb, &vias[i].net);
        for j in (i + 1)..vias.len() {
            if vias[i].net != vias[j].net {
                continue;
            }
            let (_, drill_j) = via_geom_for(pcb, &vias[j].net);
            let edge =
                d2(vias[i].position, vias[j].position).sqrt() - drill_i / 2.0 - drill_j / 2.0;
            if edge < min_spacing - 1e-6 {
                let (ri, rj) = (find(&mut parent, i), find(&mut parent, j));
                if ri != rj {
                    parent[ri] = rj;
                }
            }
        }
    }

    // Resolve each cluster: keep the member nearest the cluster centroid.
    let mut clusters: std::collections::BTreeMap<usize, Vec<usize>> = Default::default();
    for i in 0..vias.len() {
        let r = find(&mut parent, i);
        clusters.entry(r).or_default().push(i);
    }

    let mut drop: BTreeSet<usize> = BTreeSet::new();
    let mut connectors: Vec<RoutedTrace> = Vec::new();
    for members in clusters.values() {
        if members.len() < 2 {
            continue;
        }
        let n = members.len() as f64;
        let cx = members.iter().map(|&i| vias[i].position.x).sum::<f64>() / n;
        let cy = members.iter().map(|&i| vias[i].position.y).sum::<f64>() / n;
        let centroid = Vec2::new(cx, cy);
        let &keep = members
            .iter()
            .min_by(|&&a, &&b| {
                d2(vias[a].position, centroid)
                    .partial_cmp(&d2(vias[b].position, centroid))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .expect("cluster is non-empty");

        // Union of layer spans, in stackup order.
        let lo = members
            .iter()
            .map(|&i| layer_idx(vias[i].start_layer))
            .min()
            .unwrap_or(0);
        let hi = members
            .iter()
            .map(|&i| layer_idx(vias[i].end_layer))
            .max()
            .unwrap_or(stack.len().saturating_sub(1));
        let keep_pos = vias[keep].position;
        vias[keep].start_layer = stack[lo];
        vias[keep].end_layer = stack[hi];

        for &i in members {
            if i == keep {
                continue;
            }
            drop.insert(i);
            let gone = vias[i].position;
            if d2(gone, keep_pos).sqrt() <= POS_EPS {
                continue; // exact duplicate — nothing to reconnect
            }
            // Reconnect: every new trace that ended on the dropped via gets a
            // same-net jog from the old via position to the survivor, on the
            // trace's own layer. The holes overlapped, so the jog is shorter
            // than a via pad — it stays inside copper the via already claimed.
            let mut jogged: BTreeSet<usize> = BTreeSet::new();
            for t in traces.iter() {
                if t.net != vias[i].net {
                    continue;
                }
                if d2(t.start, gone).sqrt() > POS_EPS && d2(t.end, gone).sqrt() > POS_EPS {
                    continue;
                }
                let li = layer_idx(t.layer);
                if jogged.insert(li) {
                    connectors.push(RoutedTrace {
                        start: gone,
                        end: keep_pos,
                        width: t.width,
                        layer: t.layer,
                        net: t.net.clone(),
                    });
                }
            }
        }
    }

    let removed = drop.len();
    if removed > 0 {
        let mut idx = 0;
        vias.retain(|_| {
            let keep = !drop.contains(&idx);
            idx += 1;
            keep
        });
        traces.extend(connectors);
    }
    removed
}

/// The board plus the router's output, as the caller would commit it — the
/// subject the DRC judges.
fn candidate_pcb(pcb: &Pcb, traces: &[RoutedTrace], vias: &[RoutedVia]) -> Pcb {
    let mut c = pcb.clone();
    for t in traces {
        c.traces.push(Trace {
            start: t.start,
            end: t.end,
            width: t.width,
            layer: t.layer,
            net: t.net.clone(),
            source: Some(CopperSource::Autoroute),
        });
    }
    for v in vias {
        let (diameter, drill) = via_geom_for(pcb, &v.net);
        c.vias.push(Via {
            position: v.position,
            diameter,
            drill,
            start_layer: v.start_layer,
            end_layer: v.end_layer,
            net: v.net.clone(),
            source: Some(CopperSource::Autoroute),
        });
    }
    c
}

/// Bounding box of the router's new copper, inflated so scoped DRC sees every
/// interaction with neighboring board copper.
fn new_copper_bbox(traces: &[RoutedTrace], vias: &[RoutedVia]) -> (Vec2, Vec2) {
    let mut min = Vec2::new(f64::INFINITY, f64::INFINITY);
    let mut max = Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
    let mut grow = |p: Vec2| {
        min.x = min.x.min(p.x);
        min.y = min.y.min(p.y);
        max.x = max.x.max(p.x);
        max.y = max.y.max(p.y);
    };
    for t in traces {
        grow(t.start);
        grow(t.end);
    }
    for v in vias {
        grow(v.position);
    }
    const MARGIN: f64 = 5.0;
    (
        Vec2::new(min.x - MARGIN, min.y - MARGIN),
        Vec2::new(max.x + MARGIN, max.y + MARGIN),
    )
}

/// If either hole of a `HoleToHole` violation is a via the router placed,
/// return that via's net. The violation's position is the midpoint between the
/// two hole centers, so membership is tested against both actual hole centers
/// via the message-independent geometry: a new via whose center is within its
/// pad diameter of the reported midpoint and whose hole pair distance matches.
fn new_via_net_at_holes(v: &DrcViolation, vias: &[RoutedVia]) -> Option<String> {
    // The midpoint sits between the two holes; each hole center is within
    // (center distance)/2 of it. Center distance = actual + r_a + r_b, and all
    // router drills are sub-millimeter, so a generous radius bound suffices.
    let reach = (v.actual.abs() + 2.0).max(2.0);
    vias.iter()
        .filter(|via| d2(via.position, v.position).sqrt() <= reach)
        .map(|via| (d2(via.position, v.position).sqrt(), via.net.clone()))
        .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(_, net)| net)
}

/// Fail-closed: strip every piece of new copper on `net` and record the
/// demotion. An unrouted net is honest; hole-to-hole-illegal copper is not.
fn demote_net(
    net: &str,
    at: Vec2,
    traces: &mut Vec<RoutedTrace>,
    vias: &mut Vec<RoutedVia>,
    report: &mut LegalizeReport,
) {
    traces.retain(|t| t.net != net);
    vias.retain(|v| v.net != net);
    report.demoted.push((net.to_string(), at));
}

/// Try to clear one `SameNetBypass` violation by deleting a redundant new
/// trace segment at the contact point. Candidates are the new segments whose
/// copper covers the violation position, longest first (drop the noodle, keep
/// the shortest connecting subtree). A deletion is accepted only when the
/// continuity oracle proves the net is no worse off without the segment.
fn prune_bypass(
    pcb: &Pcb,
    traces: &mut Vec<RoutedTrace>,
    vias: &[RoutedVia],
    v: &DrcViolation,
) -> bool {
    // The net is named in the message; recover it geometrically instead of
    // parsing: any new segment covering the contact point names the net.
    let mut candidates: Vec<usize> = traces
        .iter()
        .enumerate()
        .filter(|(_, t)| point_on_trace(v.position, t))
        .map(|(i, _)| i)
        .collect();
    if candidates.is_empty() {
        return false;
    }
    candidates.sort_by(|&a, &b| {
        let la = d2(traces[a].start, traces[a].end);
        let lb = d2(traces[b].start, traces[b].end);
        lb.partial_cmp(&la).unwrap_or(std::cmp::Ordering::Equal)
    });

    for &i in &candidates {
        let net = traces[i].net.clone();
        let before = analyze_net_continuity(&candidate_pcb(pcb, traces, vias), &net);
        let removed = traces.remove(i);
        let after = analyze_net_continuity(&candidate_pcb(pcb, traces, vias), &net);
        let ok = after.islands <= before.islands && after.coverage >= before.coverage - 1e-9;
        if ok {
            return true;
        }
        traces.insert(i, removed);
    }
    false
}

/// True when `p` lies on the copper of trace `t` (within half its width plus
/// tolerance).
fn point_on_trace(p: Vec2, t: &RoutedTrace) -> bool {
    let (a, b) = (t.start, t.end);
    let ab = Vec2::new(b.x - a.x, b.y - a.y);
    let len2 = ab.x * ab.x + ab.y * ab.y;
    let u = if len2 <= f64::EPSILON {
        0.0
    } else {
        (((p.x - a.x) * ab.x + (p.y - a.y) * ab.y) / len2).clamp(0.0, 1.0)
    };
    let closest = Vec2::new(a.x + ab.x * u, a.y + ab.y * u);
    d2(p, closest).sqrt() <= t.width / 2.0 + 1e-3
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_ir::ecad::*;

    fn board() -> Pcb {
        Pcb {
            outline: BoardOutline {
                vertices: vec![
                    Vec2::new(0.0, 0.0),
                    Vec2::new(50.0, 0.0),
                    Vec2::new(50.0, 30.0),
                    Vec2::new(0.0, 30.0),
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
                        material: Some("FR4".into()),
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
            nets: vec![],
            rules: DesignRules {
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
                net_class_assignments: Default::default(),
                edge_clearance: 0.5,
                hole_to_hole: 0.5,
                min_annular_ring: 0.15,
                min_drill: 0.2,
            },
            footprints: vec![],
            traces: vec![],
            trace_arcs: vec![],
            vias: vec![],
            zones: vec![],
            keepouts: vec![],
            net_ties: vec![],
        }
    }

    fn rvia(x: f64, y: f64, net: &str) -> RoutedVia {
        RoutedVia {
            position: Vec2::new(x, y),
            net: net.into(),
            start_layer: PcbLayer::FCu,
            end_layer: PcbLayer::BCu,
        }
    }

    fn rtrace(a: (f64, f64), b: (f64, f64), net: &str, layer: PcbLayer) -> RoutedTrace {
        RoutedTrace {
            start: Vec2::new(a.0, a.1),
            end: Vec2::new(b.0, b.1),
            width: 0.25,
            layer,
            net: net.into(),
        }
    }

    /// Two same-net vias at the exact same spot collapse to one.
    #[test]
    fn coincident_same_net_vias_deduped() {
        let pcb = board();
        let mut traces = vec![rtrace((5.0, 5.0), (10.0, 10.0), "A", PcbLayer::FCu)];
        let mut vias = vec![rvia(10.0, 10.0, "A"), rvia(10.0, 10.0, "A")];
        let report = legalize(&pcb, &mut traces, &mut vias);
        assert_eq!(report.merged_vias, 1);
        assert_eq!(vias.len(), 1);
        assert!(report.demoted.is_empty());
    }

    /// Overlapping same-net drills (negative hole-to-hole spacing) merge, and
    /// the trace that ended on the dropped via is reconnected with a jog.
    #[test]
    fn overlapping_same_net_vias_merged_and_reconnected() {
        let pcb = board();
        // 0.3mm apart: drills (0.4) overlap outright — edge dist = -0.1mm.
        let mut traces = vec![
            rtrace((5.0, 10.0), (10.0, 10.0), "A", PcbLayer::FCu),
            rtrace((10.3, 10.0), (20.0, 10.0), "A", PcbLayer::BCu),
        ];
        let mut vias = vec![rvia(10.0, 10.0, "A"), rvia(10.3, 10.0, "A")];
        let report = legalize(&pcb, &mut traces, &mut vias);
        assert_eq!(report.merged_vias, 1);
        assert_eq!(vias.len(), 1);
        // The BCu trace's end at the dropped via must still reach the survivor:
        // a jog on BCu connects the old position to the kept one.
        let kept = vias[0].position;
        assert!(
            traces.iter().any(|t| t.layer == PcbLayer::BCu
                && (d2(t.start, kept).sqrt() <= POS_EPS || d2(t.end, kept).sqrt() <= POS_EPS)),
            "dropped via's trace must be jogged to the surviving via, traces: {traces:?}"
        );
        // And the merged output is hole-to-hole clean.
        let candidate = candidate_pcb(&pcb, &traces, &vias);
        let h2h: Vec<_> = crate::drc::check_drc(&candidate)
            .into_iter()
            .filter(|v| v.rule == DrcRuleType::HoleToHole)
            .collect();
        assert!(h2h.is_empty(), "merge must clear HoleToHole, got {h2h:?}");
    }

    /// Different-net vias can't merge: the net whose via is illegally close to
    /// a fixed board hole is stripped, fail-closed.
    #[test]
    fn different_net_hole_conflict_demotes_net() {
        let mut pcb = board();
        // A pre-existing (manual) via on net B — a fixed hole the router must
        // respect.
        pcb.vias.push(Via {
            position: Vec2::new(10.0, 10.0),
            diameter: 0.8,
            drill: 0.4,
            start_layer: PcbLayer::FCu,
            end_layer: PcbLayer::BCu,
            net: "B".into(),
            source: Some(CopperSource::Manual),
        });
        let mut traces = vec![rtrace((10.3, 10.0), (20.0, 10.0), "A", PcbLayer::FCu)];
        let mut vias = vec![rvia(10.3, 10.0, "A")];
        let report = legalize(&pcb, &mut traces, &mut vias);
        assert_eq!(report.demoted.len(), 1);
        assert_eq!(report.demoted[0].0, "A");
        assert!(vias.is_empty() && traces.is_empty());
    }

    /// A board with two pads on net A and a trace running between them, plus a
    /// floating stub the router left behind somewhere else on the same net.
    fn board_with_pads() -> Pcb {
        let mut pcb = board();
        let pad = |x: f64, y: f64, num: &str, net: &str| Footprint {
            reference: format!("U{num}"),
            value: String::new(),
            footprint_name: "test".into(),
            position: Vec2::new(x, y),
            rotation: 0.0,
            front: true,
            pads: vec![Pad {
                number: num.into(),
                pad_type: PadType::SMD,
                shape: PadShape::Rect {
                    width: 1.0,
                    height: 1.0,
                },
                position: Vec2::new(0.0, 0.0),
                rotation: 0.0,
                drill: None,
                layers: vec![PcbLayer::FCu],
                net: Some(net.into()),
            }],
            graphics: vec![],
            model_3d: None,
            properties: Default::default(),
        };
        pcb.footprints.push(pad(5.0, 5.0, "1", "A"));
        pcb.footprints.push(pad(20.0, 5.0, "2", "A"));
        pcb
    }

    /// Copper that reaches a pad stays; a floating island of the same net —
    /// trace plus the via that hangs off it — is removed.
    #[test]
    fn dangling_island_pruned_live_copper_kept() {
        let pcb = board_with_pads();
        let mut traces = vec![
            // Pad-to-pad: the live route.
            rtrace((5.0, 5.0), (20.0, 5.0), "A", PcbLayer::FCu),
            // Orphan: touches neither pad nor pour.
            rtrace((5.0, 20.0), (12.0, 20.0), "A", PcbLayer::FCu),
        ];
        let mut vias = vec![rvia(12.0, 20.0, "A")];
        let (dead_t, dead_v) = prune_dangling(&pcb, &mut traces, &mut vias);
        assert_eq!((dead_t, dead_v), (1, 1));
        assert_eq!(traces.len(), 1);
        assert_eq!(traces[0].start, Vec2::new(5.0, 5.0));
        assert!(vias.is_empty());
    }

    /// The prune only judges the router's own copper: board traces stay even
    /// when they are the dangling ones.
    #[test]
    fn existing_board_copper_never_pruned() {
        let mut pcb = board_with_pads();
        pcb.traces.push(Trace {
            start: Vec2::new(5.0, 20.0),
            end: Vec2::new(12.0, 20.0),
            width: 0.25,
            layer: PcbLayer::FCu,
            net: "A".into(),
            source: Some(CopperSource::Manual),
        });
        let mut traces = vec![rtrace((5.0, 5.0), (20.0, 5.0), "A", PcbLayer::FCu)];
        let mut vias = vec![];
        let (dead_t, dead_v) = prune_dangling(&pcb, &mut traces, &mut vias);
        assert_eq!((dead_t, dead_v), (0, 0));
        assert_eq!(traces.len(), 1);
        assert_eq!(pcb.traces.len(), 1, "board copper is judged, never removed");
    }

    /// A chain of new copper is kept as a whole when any link reaches a pad —
    /// the island is the unit, not the individual segment.
    #[test]
    fn chained_copper_anchored_through_via_kept() {
        let pcb = board_with_pads();
        let mut traces = vec![
            rtrace((5.0, 5.0), (10.0, 5.0), "A", PcbLayer::FCu),
            rtrace((10.0, 5.0), (20.0, 5.0), "A", PcbLayer::BCu),
        ];
        let mut vias = vec![rvia(10.0, 5.0, "A")];
        let (dead_t, dead_v) = prune_dangling(&pcb, &mut traces, &mut vias);
        assert_eq!((dead_t, dead_v), (0, 0));
        assert_eq!((traces.len(), vias.len()), (2, 1));
    }

    /// A redundant same-net segment that brushes distant copper of its own net
    /// (SameNetBypass) is pruned when the net stays connected without it.
    #[test]
    fn bypass_noodle_pruned_when_redundant() {
        let mut pcb = board();
        // A long conductor chain on net A already on the board: a U shape.
        let t = |a: (f64, f64), b: (f64, f64)| Trace {
            start: Vec2::new(a.0, a.1),
            end: Vec2::new(b.0, b.1),
            width: 0.25,
            layer: PcbLayer::FCu,
            net: "A".into(),
            source: Some(CopperSource::Manual),
        };
        pcb.traces.push(t((5.0, 5.0), (5.0, 7.0)));
        pcb.traces.push(t((5.0, 7.0), (5.0, 9.0)));
        pcb.traces.push(t((5.0, 9.0), (5.0, 11.0)));
        pcb.traces.push(t((5.0, 11.0), (5.0, 13.0)));
        // The router adds a noodle attached at the chain's start that wanders
        // around and lands mid-body on the chain's third segment — a same-net
        // contact 5 conductor hops from its own attachment point.
        let mut traces = vec![
            rtrace((5.0, 5.0), (8.0, 5.0), "A", PcbLayer::FCu),
            rtrace((8.0, 5.0), (8.0, 10.0), "A", PcbLayer::FCu),
            rtrace((8.0, 10.0), (5.0, 10.0), "A", PcbLayer::FCu),
        ];
        let mut vias = vec![];
        let report = legalize(&pcb, &mut traces, &mut vias);
        assert!(report.pruned_traces >= 1, "the noodle must be pruned");
        // The result must be bypass-free.
        let candidate = candidate_pcb(&pcb, &traces, &vias);
        let bypass: Vec<_> = crate::drc::check_drc(&candidate)
            .into_iter()
            .filter(|v| v.rule == DrcRuleType::SameNetBypass)
            .collect();
        assert!(
            bypass.is_empty(),
            "prune must clear the bypass, got {bypass:?}"
        );
    }
}
