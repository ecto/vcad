//! Group length matching for routed nets — `length_match_traces`.
//!
//! Takes a group of timing-critical nets (a DDR byte lane, a clock tree, a
//! SPI bus), measures each net's routed copper, and grows the shorter nets
//! with clearance-checked meanders (via [`length_tune`](super::length_tune))
//! until every net in the group reaches the target length within tolerance.
//! The target defaults to the longest net in the group.
//!
//! The matcher is conservative by design: it only tunes nets whose copper
//! forms a single unbranched polyline on one layer, and reports every other
//! net as skipped with a reason instead of guessing. Replacement traces are
//! returned as data — the caller decides whether to commit them.

use std::collections::HashMap;

use vcad_ir::ecad::{Pcb, Trace};
use vcad_ir::Vec2;

use super::length_tune::{generate_meanders_checked, LengthTuneParams, MeanderStyle};
use crate::session::RouteSession;

/// Endpoint-matching tolerance when chaining trace segments into a polyline.
const CHAIN_EPS: f64 = 1e-6;

/// Options for [`match_lengths`].
#[derive(Debug, Clone)]
pub struct LengthMatchOptions {
    /// Target routed length in mm. Defaults to the longest net in the group.
    pub target_length: Option<f64>,
    /// A net counts as matched when within this of the target (mm).
    pub tolerance: f64,
    /// Maximum meander amplitude in mm.
    pub max_amplitude: f64,
    /// Meander period spacing in mm.
    pub spacing: f64,
    /// Meander pattern style.
    pub style: MeanderStyle,
}

impl Default for LengthMatchOptions {
    fn default() -> Self {
        Self {
            target_length: None,
            tolerance: 0.1,
            max_amplitude: 2.0,
            spacing: 1.0,
            style: MeanderStyle::Trombone,
        }
    }
}

/// Per-net outcome of a length-matching pass.
#[derive(Debug, Clone, serde::Serialize)]
pub struct NetLengthReport {
    /// Net name.
    pub net: String,
    /// Routed length before tuning (mm).
    pub length_before: f64,
    /// Routed length after tuning (mm) — equals `length_before` when untouched.
    pub length_after: f64,
    /// Whether `length_after` is within tolerance of the target.
    pub matched: bool,
    /// Whether meanders were generated for this net.
    pub tuned: bool,
    /// Why the net could not be tuned, when it needed tuning but wasn't.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skip_reason: Option<String>,
    /// Replacement traces for this net (present only when `tuned`). These
    /// replace ALL of the net's existing straight traces.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub new_traces: Vec<Trace>,
}

/// Result of [`match_lengths`] over a net group.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LengthMatchResult {
    /// The length every net was tuned toward (mm).
    pub target_length: f64,
    /// Match tolerance used (mm).
    pub tolerance: f64,
    /// Whether every net in the group ended within tolerance of the target.
    pub all_matched: bool,
    /// Per-net reports, in input order.
    pub nets: Vec<NetLengthReport>,
}

/// Total routed straight-trace length of a net (mm).
pub fn net_routed_length(pcb: &Pcb, net: &str) -> f64 {
    pcb.traces
        .iter()
        .filter(|t| t.net == net)
        .map(|t| (t.end - t.start).length())
        .sum()
}

/// Chain a net's traces into one ordered polyline, or explain why we can't.
///
/// Requirements: at least one straight trace, all on one layer, no arcs on the
/// net, every endpoint shared by at most two segments (no branches), and the
/// segments form a single connected open chain.
fn net_polyline(pcb: &Pcb, net: &str) -> Result<(Vec<Vec2>, usize), String> {
    let segs: Vec<&Trace> = pcb.traces.iter().filter(|t| t.net == net).collect();
    if segs.is_empty() {
        return Err("net has no routed traces".into());
    }
    if pcb.trace_arcs.iter().any(|a| a.net == net) {
        return Err("net has arc traces (arc tuning unsupported)".into());
    }
    let layer = segs[0].layer;
    if segs.iter().any(|t| t.layer != layer) {
        return Err("net spans multiple copper layers".into());
    }

    // Quantized-endpoint adjacency map.
    let key = |p: Vec2| -> (i64, i64) {
        (
            (p.x / CHAIN_EPS).round() as i64,
            (p.y / CHAIN_EPS).round() as i64,
        )
    };
    let mut adj: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, t) in segs.iter().enumerate() {
        adj.entry(key(t.start)).or_default().push(i);
        adj.entry(key(t.end)).or_default().push(i);
    }
    if adj.values().any(|v| v.len() > 2) {
        return Err("net routing branches (T-junction)".into());
    }
    let endpoints: Vec<_> = adj.iter().filter(|(_, v)| v.len() == 1).collect();
    if endpoints.len() != 2 {
        return Err("net traces do not form a single open chain".into());
    }

    // Walk from one endpoint to the other.
    let start_key = *endpoints[0].0;
    let mut points = Vec::with_capacity(segs.len() + 1);
    let mut current = start_key;
    let mut prev_seg: Option<usize> = None;
    while let Some(next_seg) = adj[&current].iter().copied().find(|s| Some(*s) != prev_seg) {
        let t = segs[next_seg];
        let (from, to) = if key(t.start) == current {
            (t.start, t.end)
        } else {
            (t.end, t.start)
        };
        points.push(from);
        current = key(to);
        prev_seg = Some(next_seg);
        if adj[&current].len() == 1 {
            points.push(to);
            break;
        }
    }
    if points.len() != segs.len() + 1 {
        return Err("net traces do not form a single connected chain".into());
    }
    Ok((points, segs.len()))
}

/// Centerline segments of every trace NOT on `net`, each carrying the
/// obstacle-side centerline requirement: its own half-width plus its rule —
/// the pair GAP for the net's diff-pair twin (a meander hugging the twin is
/// exactly the pinch the DRC flags), the base clearance otherwise. Add the
/// tuned trace's half-width on the caller side for the full requirement.
fn other_net_obstacles(pcb: &Pcb, net: &str, base_clearance: f64) -> Vec<(Vec2, Vec2, f64)> {
    let twin_gap: Option<(String, f64)> = crate::drc::diff_pairs(pcb).into_iter().find_map(|dp| {
        if dp.net_p == net {
            Some((dp.net_n, dp.gap))
        } else if dp.net_n == net {
            Some((dp.net_p, dp.gap))
        } else {
            None
        }
    });
    pcb.traces
        .iter()
        .filter(|t| t.net != net)
        .map(|t| {
            let rule = match &twin_gap {
                Some((twin, gap)) if &t.net == twin => (*gap - 0.005).max(base_clearance),
                _ => base_clearance,
            };
            (t.start, t.end, t.width / 2.0 + rule)
        })
        .collect()
}

/// Splice meander waypoints into the base polyline and emit replacement traces.
fn polyline_to_traces(
    points: &[Vec2],
    meanders: &[super::length_tune::MeanderSegment],
    template: &Trace,
) -> Vec<Trace> {
    // Meander points replace segment `segment_index`; other segments pass through.
    let by_index: HashMap<usize, &super::length_tune::MeanderSegment> =
        meanders.iter().map(|m| (m.segment_index, m)).collect();

    let mut out: Vec<Vec2> = vec![points[0]];
    for i in 0..points.len() - 1 {
        if let Some(m) = by_index.get(&i) {
            // Meander points include the segment endpoints; skip the first to
            // avoid duplicating the previous point.
            out.extend(m.points.iter().skip(1).copied());
        } else {
            out.push(points[i + 1]);
        }
    }

    out.windows(2)
        .filter(|w| (w[1] - w[0]).length() > CHAIN_EPS)
        .map(|w| Trace {
            start: w[0],
            end: w[1],
            width: template.width,
            layer: template.layer,
            net: template.net.clone(),
            source: template.source,
        })
        .collect()
}

/// Length-match a group of nets by meandering the shorter ones.
///
/// Pure: returns replacement traces as data and mutates nothing. For each net
/// that needs lengthening, meanders are generated clearance-checked against
/// every other net's copper centerlines (the required clearance plus both
/// half-widths, conservatively using the tuned net's width for both).
pub fn match_lengths(pcb: &Pcb, nets: &[String], opts: &LengthMatchOptions) -> LengthMatchResult {
    let lengths: Vec<f64> = nets.iter().map(|n| net_routed_length(pcb, n)).collect();
    let longest = lengths.iter().cloned().fold(0.0_f64, f64::max);
    let target = opts.target_length.unwrap_or(longest);

    let session = RouteSession::from_pcb(pcb);

    let mut reports = Vec::with_capacity(nets.len());
    for (net, &before) in nets.iter().zip(&lengths) {
        let deficit = target - before;
        if deficit <= opts.tolerance {
            reports.push(NetLengthReport {
                net: net.clone(),
                length_before: before,
                length_after: before,
                matched: deficit >= -opts.tolerance,
                tuned: false,
                skip_reason: if deficit < -opts.tolerance {
                    Some("net is longer than the target (shortening unsupported)".into())
                } else {
                    None
                },
                new_traces: vec![],
            });
            continue;
        }

        let (points, _) = match net_polyline(pcb, net) {
            Ok(p) => p,
            Err(reason) => {
                reports.push(NetLengthReport {
                    net: net.clone(),
                    length_before: before,
                    length_after: before,
                    matched: false,
                    tuned: false,
                    skip_reason: Some(reason),
                    new_traces: vec![],
                });
                continue;
            }
        };

        let template = pcb
            .traces
            .iter()
            .find(|t| t.net == *net)
            .expect("net_polyline guarantees at least one trace")
            .clone();
        let params = LengthTuneParams {
            target_length: target,
            max_amplitude: opts.max_amplitude,
            spacing: opts.spacing,
            style: opts.style,
        };
        // Waypoints are trace centerline points: the caller side contributes
        // its half-width; each obstacle carries its own half-width + rule.
        let min_clearance = template.width / 2.0;
        let obstacles = other_net_obstacles(pcb, net, session.clearance_for(net));

        match generate_meanders_checked(&points, &params, min_clearance, &obstacles) {
            Some(meanders) if !meanders.is_empty() => {
                let new_traces = polyline_to_traces(&points, &meanders, &template);
                let after: f64 = new_traces.iter().map(|t| (t.end - t.start).length()).sum();
                reports.push(NetLengthReport {
                    net: net.clone(),
                    length_before: before,
                    length_after: after,
                    matched: (target - after).abs() <= opts.tolerance,
                    tuned: true,
                    skip_reason: None,
                    new_traces,
                });
            }
            Some(_) => {
                // Already at/over target (shouldn't happen given deficit check).
                reports.push(NetLengthReport {
                    net: net.clone(),
                    length_before: before,
                    length_after: before,
                    matched: true,
                    tuned: false,
                    skip_reason: None,
                    new_traces: vec![],
                });
            }
            None => {
                reports.push(NetLengthReport {
                    net: net.clone(),
                    length_before: before,
                    length_after: before,
                    matched: false,
                    tuned: false,
                    skip_reason: Some(
                        "meanders do not fit: amplitude/clearance constraints or segments \
                         shorter than the meander spacing"
                            .into(),
                    ),
                    new_traces: vec![],
                });
            }
        }
    }

    let all_matched = reports.iter().all(|r| r.matched);
    LengthMatchResult {
        target_length: target,
        tolerance: opts.tolerance,
        all_matched,
        nets: reports,
    }
}

/// Verify a length-match constraint without changing anything: per-net routed
/// lengths, the group target (longest or explicit), and each net's deviation.
pub fn check_length_match(
    pcb: &Pcb,
    nets: &[String],
    target_length: Option<f64>,
    tolerance: f64,
) -> LengthMatchResult {
    let lengths: Vec<f64> = nets.iter().map(|n| net_routed_length(pcb, n)).collect();
    let longest = lengths.iter().cloned().fold(0.0_f64, f64::max);
    let target = target_length.unwrap_or(longest);
    let reports: Vec<NetLengthReport> = nets
        .iter()
        .zip(&lengths)
        .map(|(net, &len)| NetLengthReport {
            net: net.clone(),
            length_before: len,
            length_after: len,
            matched: (target - len).abs() <= tolerance,
            tuned: false,
            skip_reason: None,
            new_traces: vec![],
        })
        .collect();
    LengthMatchResult {
        target_length: target,
        tolerance,
        all_matched: reports.iter().all(|r| r.matched),
        nets: reports,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_ir::ecad::*;

    fn board(traces: Vec<Trace>) -> Pcb {
        Pcb {
            outline: BoardOutline {
                vertices: vec![
                    Vec2::new(0.0, 0.0),
                    Vec2::new(60.0, 0.0),
                    Vec2::new(60.0, 60.0),
                    Vec2::new(0.0, 60.0),
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
            traces,
            trace_arcs: vec![],
            vias: vec![],
            zones: vec![],
            keepouts: vec![],
            net_ties: vec![],
        }
    }

    fn trace(net: &str, a: (f64, f64), b: (f64, f64)) -> Trace {
        Trace {
            start: Vec2::new(a.0, a.1),
            end: Vec2::new(b.0, b.1),
            width: 0.25,
            layer: PcbLayer::FCu,
            net: net.into(),
            source: None,
        }
    }

    #[test]
    fn matches_short_net_to_longest() {
        // LONG is 50mm, SHORT is 30mm — SHORT gains ~20mm of meander.
        let pcb = board(vec![
            trace("LONG", (5.0, 10.0), (55.0, 10.0)),
            trace("SHORT", (5.0, 30.0), (35.0, 30.0)),
        ]);
        let r = match_lengths(
            &pcb,
            &["LONG".into(), "SHORT".into()],
            &LengthMatchOptions {
                max_amplitude: 3.0,
                spacing: 2.0,
                tolerance: 0.5,
                ..Default::default()
            },
        );
        assert!((r.target_length - 50.0).abs() < 1e-9);
        assert!(r.all_matched, "reports: {:?}", r.nets);
        let short = &r.nets[1];
        assert!(short.tuned);
        assert!((short.length_after - 50.0).abs() <= 0.5);
        assert!(!short.new_traces.is_empty());
        // Endpoints preserved (the chain may be walked in either direction).
        let first = short.new_traces.first().unwrap().start;
        let last = short.new_traces.last().unwrap().end;
        let mut ends = [(first.x, first.y), (last.x, last.y)];
        ends.sort_by(|a, b| a.0.total_cmp(&b.0));
        assert!((ends[0].0 - 5.0).abs() < 1e-9 && (ends[0].1 - 30.0).abs() < 1e-9);
        assert!((ends[1].0 - 35.0).abs() < 1e-9 && (ends[1].1 - 30.0).abs() < 1e-9);
        // Replacement copper carries the net's width/layer.
        assert!(short.new_traces.iter().all(|t| t.width == 0.25));
    }

    #[test]
    fn explicit_target_overrides_longest() {
        let pcb = board(vec![trace("A", (5.0, 10.0), (45.0, 10.0))]);
        let r = match_lengths(
            &pcb,
            &["A".into()],
            &LengthMatchOptions {
                target_length: Some(60.0),
                max_amplitude: 4.0,
                spacing: 2.0,
                tolerance: 0.5,
                ..Default::default()
            },
        );
        assert!((r.target_length - 60.0).abs() < 1e-9);
        assert!(r.all_matched, "reports: {:?}", r.nets);
        assert!((r.nets[0].length_after - 60.0).abs() <= 0.5);
    }

    #[test]
    fn multi_segment_chain_is_tuned() {
        // L-shaped SHORT (out of order in the trace list) still chains.
        let pcb = board(vec![
            trace("LONG", (5.0, 5.0), (55.0, 5.0)),
            trace("SHORT", (25.0, 30.0), (25.0, 45.0)),
            trace("SHORT", (5.0, 30.0), (25.0, 30.0)),
        ]);
        let r = match_lengths(
            &pcb,
            &["LONG".into(), "SHORT".into()],
            &LengthMatchOptions {
                max_amplitude: 3.0,
                spacing: 2.0,
                tolerance: 0.5,
                ..Default::default()
            },
        );
        assert!(r.all_matched, "reports: {:?}", r.nets);
        assert!(r.nets[1].tuned);
    }

    #[test]
    fn branching_net_is_skipped_with_reason() {
        let pcb = board(vec![
            trace("LONG", (5.0, 5.0), (55.0, 5.0)),
            trace("T", (5.0, 30.0), (15.0, 30.0)),
            trace("T", (15.0, 30.0), (25.0, 30.0)),
            trace("T", (15.0, 30.0), (15.0, 40.0)),
        ]);
        let r = match_lengths(&pcb, &["LONG".into(), "T".into()], &Default::default());
        assert!(!r.all_matched);
        let t = &r.nets[1];
        assert!(!t.tuned);
        assert!(t.skip_reason.as_deref().unwrap().contains("branches"));
    }

    #[test]
    fn unrouted_net_is_skipped() {
        let pcb = board(vec![trace("LONG", (5.0, 5.0), (55.0, 5.0))]);
        let r = match_lengths(&pcb, &["LONG".into(), "NOPE".into()], &Default::default());
        assert!(!r.all_matched);
        assert!(r.nets[1]
            .skip_reason
            .as_deref()
            .unwrap()
            .contains("no routed traces"));
    }

    #[test]
    fn longer_than_target_reports_unmatchable() {
        let pcb = board(vec![trace("A", (5.0, 5.0), (55.0, 5.0))]);
        let r = match_lengths(
            &pcb,
            &["A".into()],
            &LengthMatchOptions {
                target_length: Some(30.0),
                ..Default::default()
            },
        );
        assert!(!r.all_matched);
        assert!(r.nets[0]
            .skip_reason
            .as_deref()
            .unwrap()
            .contains("longer than the target"));
    }

    #[test]
    fn nearby_copper_reduces_amplitude() {
        // A GND rail 1.5mm above the SHORT trace caps meander amplitude.
        let pcb = board(vec![
            trace("LONG", (5.0, 50.0), (55.0, 50.0)),
            trace("SHORT", (5.0, 30.0), (45.0, 30.0)),
            trace("GND", (0.0, 31.5), (60.0, 31.5)),
        ]);
        let r = match_lengths(
            &pcb,
            &["LONG".into(), "SHORT".into()],
            &LengthMatchOptions {
                max_amplitude: 3.0,
                spacing: 1.0,
                tolerance: 0.5,
                ..Default::default()
            },
        );
        let short = &r.nets[1];
        if short.tuned {
            // Whatever amplitude was used must clear the GND rail.
            let max_y = short
                .new_traces
                .iter()
                .flat_map(|t| [t.start.y, t.end.y])
                .fold(f64::MIN, f64::max);
            assert!(
                max_y < 31.5 - (0.2 + 0.25) + 1e-9,
                "meanders must hold clearance to GND, max_y={max_y}"
            );
        }
    }

    #[test]
    fn check_length_match_reports_deviation() {
        let pcb = board(vec![
            trace("A", (5.0, 10.0), (55.0, 10.0)),
            trace("B", (5.0, 30.0), (35.0, 30.0)),
        ]);
        let r = check_length_match(&pcb, &["A".into(), "B".into()], None, 0.1);
        assert!((r.target_length - 50.0).abs() < 1e-9);
        assert!(!r.all_matched);
        assert!(r.nets[0].matched);
        assert!(!r.nets[1].matched);
        assert!((r.nets[1].length_before - 30.0).abs() < 1e-9);
    }
}

// ============================================================================
// Multi-layer tuning: meander the longest single-layer run
// ============================================================================

/// Chain `segs` (all one net, one layer) into open unbranched chains and
/// return the longest one as ordered points. `None` when the layer branches
/// (T-junction) or holds no open chain — the conservative refusal inherited
/// from [`net_polyline`], scoped to one layer instead of the whole net.
fn longest_chain(segs: &[&Trace]) -> Option<Vec<Vec2>> {
    if segs.is_empty() {
        return None;
    }
    let key = |p: Vec2| {
        (
            (p.x / CHAIN_EPS).round() as i64,
            (p.y / CHAIN_EPS).round() as i64,
        )
    };
    let mut adj: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, t) in segs.iter().enumerate() {
        adj.entry(key(t.start)).or_default().push(i);
        adj.entry(key(t.end)).or_default().push(i);
    }
    if adj.values().any(|v| v.len() > 2) {
        return None;
    }
    // Walk each open chain from an endpoint; keep the longest by length.
    let mut used: std::collections::HashSet<usize> = Default::default();
    let mut best: Option<(f64, Vec<Vec2>)> = None;
    let endpoints: Vec<(i64, i64)> = adj
        .iter()
        .filter(|(_, v)| v.len() == 1)
        .map(|(k, _)| *k)
        .collect();
    for start in endpoints {
        let first = adj[&start][0];
        if used.contains(&first) {
            continue;
        }
        let mut points = Vec::new();
        let mut current = start;
        let mut prev_seg: Option<usize> = None;
        let mut len = 0.0;
        while let Some(next_seg) = adj[&current]
            .iter()
            .copied()
            .find(|s| Some(*s) != prev_seg && !used.contains(s))
        {
            let t = segs[next_seg];
            used.insert(next_seg);
            let (from, to) = if key(t.start) == current {
                (t.start, t.end)
            } else {
                (t.end, t.start)
            };
            if points.is_empty() {
                points.push(from);
            }
            len += (to - from).length();
            points.push(to);
            current = key(to);
            prev_seg = Some(next_seg);
        }
        if points.len() >= 2 && best.as_ref().map(|(l, _)| len > *l).unwrap_or(true) {
            best = Some((len, points));
        }
    }
    best.map(|(_, p)| p)
}

/// Group length matching for nets whose routing spans layers and vias — the
/// production case [`match_lengths`]'s whole-net-polyline contract refuses.
///
/// Strategy: the net's total routed length is measured across all layers, but
/// the *meander is inserted into the longest single-layer run* (the longest
/// unbranched chain of same-layer segments). Clearance is checked against
/// other-net traces on that layer only. `new_traces` in each tuned report is
/// a full-net replacement (untouched runs pass through verbatim), preserving
/// the existing consumer contract: delete the net's traces, insert
/// `new_traces`.
pub fn match_lengths_runs(
    pcb: &Pcb,
    nets: &[String],
    opts: &LengthMatchOptions,
) -> LengthMatchResult {
    let lengths: Vec<f64> = nets.iter().map(|n| net_routed_length(pcb, n)).collect();
    let longest = lengths.iter().cloned().fold(0.0_f64, f64::max);
    let target = opts.target_length.unwrap_or(longest);
    let session = RouteSession::from_pcb(pcb);

    let mut reports = Vec::with_capacity(nets.len());
    for (net, &before) in nets.iter().zip(&lengths) {
        let deficit = target - before;
        if deficit <= opts.tolerance {
            reports.push(NetLengthReport {
                net: net.clone(),
                length_before: before,
                length_after: before,
                matched: deficit >= -opts.tolerance,
                tuned: false,
                skip_reason: if deficit < -opts.tolerance {
                    Some("net is longer than the target (shortening unsupported)".into())
                } else {
                    None
                },
                new_traces: vec![],
            });
            continue;
        }

        // Longest single-layer run across the net's layers.
        let mut layers: Vec<vcad_ir::ecad::PcbLayer> = pcb
            .traces
            .iter()
            .filter(|t| t.net == *net)
            .map(|t| t.layer)
            .collect();
        layers.sort();
        layers.dedup();
        let mut run: Option<(f64, Vec<Vec2>, vcad_ir::ecad::PcbLayer)> = None;
        for layer in layers {
            let segs: Vec<&Trace> = pcb
                .traces
                .iter()
                .filter(|t| t.net == *net && t.layer == layer)
                .collect();
            if let Some(points) = longest_chain(&segs) {
                let len: f64 = points.windows(2).map(|w| (w[1] - w[0]).length()).sum();
                if run.as_ref().map(|(l, _, _)| len > *l).unwrap_or(true) {
                    run = Some((len, points, layer));
                }
            }
        }
        let Some((run_len, points, layer)) = run else {
            reports.push(NetLengthReport {
                net: net.clone(),
                length_before: before,
                length_after: before,
                matched: false,
                tuned: false,
                skip_reason: Some("no unbranched single-layer run to tune".into()),
                new_traces: vec![],
            });
            continue;
        };

        let template = pcb
            .traces
            .iter()
            .find(|t| t.net == *net && t.layer == layer)
            .expect("run implies at least one trace on the layer")
            .clone();
        let params = LengthTuneParams {
            // The tuner grows THIS run by the whole-net deficit.
            target_length: run_len + deficit,
            max_amplitude: opts.max_amplitude,
            spacing: opts.spacing,
            style: opts.style,
        };
        let min_clearance = template.width / 2.0;
        let clearance = session.clearance_for(net);
        let twin_gap: Option<(String, f64)> =
            crate::drc::diff_pairs(pcb).into_iter().find_map(|dp| {
                if &dp.net_p == net {
                    Some((dp.net_n, dp.gap))
                } else if &dp.net_n == net {
                    Some((dp.net_p, dp.gap))
                } else {
                    None
                }
            });
        let obstacles: Vec<(Vec2, Vec2, f64)> = pcb
            .traces
            .iter()
            .filter(|t| t.net != *net && t.layer == layer)
            .map(|t| {
                let rule = match &twin_gap {
                    Some((twin, gap)) if &t.net == twin => (*gap - 0.005).max(clearance),
                    _ => clearance,
                };
                (t.start, t.end, t.width / 2.0 + rule)
            })
            .collect();

        match generate_meanders_checked(&points, &params, min_clearance, &obstacles) {
            Some(meanders) if !meanders.is_empty() => {
                let tuned_run = polyline_to_traces(&points, &meanders, &template);
                // Full-net replacement: untouched segments (other layers and
                // other chains on this layer) pass through verbatim.
                let key = |p: Vec2| {
                    (
                        (p.x / CHAIN_EPS).round() as i64,
                        (p.y / CHAIN_EPS).round() as i64,
                    )
                };
                let run_keys: std::collections::HashSet<((i64, i64), (i64, i64))> = points
                    .windows(2)
                    .flat_map(|w| [(key(w[0]), key(w[1])), (key(w[1]), key(w[0]))])
                    .collect();
                let mut new_traces: Vec<Trace> = pcb
                    .traces
                    .iter()
                    .filter(|t| {
                        t.net == *net
                            && !(t.layer == layer && run_keys.contains(&(key(t.start), key(t.end))))
                    })
                    .cloned()
                    .collect();
                new_traces.extend(tuned_run);
                let after: f64 = new_traces.iter().map(|t| (t.end - t.start).length()).sum();
                reports.push(NetLengthReport {
                    net: net.clone(),
                    length_before: before,
                    length_after: after,
                    matched: (target - after).abs() <= opts.tolerance,
                    tuned: true,
                    skip_reason: None,
                    new_traces,
                });
            }
            _ => {
                reports.push(NetLengthReport {
                    net: net.clone(),
                    length_before: before,
                    length_after: before,
                    matched: false,
                    tuned: false,
                    skip_reason: Some(
                        "meanders do not fit on the longest run: amplitude/clearance \
                         constraints or run shorter than the meander spacing"
                            .into(),
                    ),
                    new_traces: vec![],
                });
            }
        }
    }

    let all_matched = reports.iter().all(|r| r.matched);
    LengthMatchResult {
        target_length: target,
        tolerance: opts.tolerance,
        all_matched,
        nets: reports,
    }
}

#[cfg(test)]
mod run_tests {
    use super::*;
    use vcad_ir::ecad::PcbLayer;

    fn seg(net: &str, layer: PcbLayer, a: (f64, f64), b: (f64, f64)) -> Trace {
        Trace {
            start: Vec2::new(a.0, a.1),
            end: Vec2::new(b.0, b.1),
            width: 0.2,
            layer,
            net: net.into(),
            source: None,
        }
    }

    fn board(traces: Vec<Trace>) -> Pcb {
        let mut pcb: Pcb = serde_json::from_value(serde_json::json!({
            "outline": {"vertices": [
                {"x": -5.0, "y": -5.0}, {"x": 60.0, "y": -5.0},
                {"x": 60.0, "y": 20.0}, {"x": -5.0, "y": 20.0}
            ], "cutouts": [], "thickness": 1.6},
            "stackup": {"layers": []},
            "nets": [],
            "rules": {
                "defaultRules": {"name": "Default", "traceWidth": 0.2, "clearance": 0.15,
                                  "viaDiameter": 0.6, "viaDrill": 0.3},
                "edgeClearance": 0.2, "holeToHole": 0.2, "minAnnularRing": 0.05, "minDrill": 0.1
            },
            "footprints": [], "traces": [], "traceArcs": [], "vias": [], "zones": []
        }))
        .expect("test board");
        pcb.traces = traces;
        pcb
    }

    #[test]
    fn multilayer_net_tunes_its_longest_run() {
        // "A": 30mm front run + 10mm back run (via-joined in spirit).
        // "B": 50mm front run. Target = 50; A needs +10, grown on the front run.
        let pcb = board(vec![
            seg("A", PcbLayer::FCu, (0.0, 0.0), (30.0, 0.0)),
            seg("A", PcbLayer::BCu, (30.0, 0.0), (40.0, 0.0)),
            seg("B", PcbLayer::FCu, (0.0, 10.0), (50.0, 10.0)),
        ]);
        let r = match_lengths_runs(
            &pcb,
            &["A".into(), "B".into()],
            &LengthMatchOptions::default(),
        );
        let a = &r.nets[0];
        assert!(a.tuned, "A must tune: {:?}", a.skip_reason);
        assert!(
            (a.length_after - 50.0).abs() <= r.tolerance,
            "A grew to target: {}",
            a.length_after
        );
        // The BCu run passes through untouched.
        assert!(a.new_traces.iter().any(|t| t.layer == PcbLayer::BCu));
        assert!(r.nets[1].matched && !r.nets[1].tuned);
    }

    #[test]
    fn branched_layer_reports_skip() {
        // T-junction on the only layer: no tunable run.
        let pcb = board(vec![
            seg("A", PcbLayer::FCu, (0.0, 0.0), (10.0, 0.0)),
            seg("A", PcbLayer::FCu, (10.0, 0.0), (20.0, 0.0)),
            seg("A", PcbLayer::FCu, (10.0, 0.0), (10.0, 5.0)),
            seg("B", PcbLayer::FCu, (0.0, 10.0), (50.0, 10.0)),
        ]);
        let r = match_lengths_runs(
            &pcb,
            &["A".into(), "B".into()],
            &LengthMatchOptions::default(),
        );
        assert!(!r.nets[0].tuned);
        assert!(r.nets[0].skip_reason.is_some());
    }
}
