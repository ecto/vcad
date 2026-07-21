//! Coupled differential-pair routing for the auto-router (the phantom-fat-trace
//! method).
//!
//! The last unrouted connections on dense reference boards are almost all
//! `_P`/`_N` pairs whose escape plans succeed but whose *lone* placements fail
//! — a pair leg routed alone blocks the corridor its twin needs. This stage
//! routes the pair as one unit: search ONE phantom trace of the pair's full
//! coupled width (`2·w + gap`) between the endpoint midpoints, then realize it
//! as two offset legs at the declared diff-pair class geometry, connect the
//! legs to the actual pads, and commit both nets atomically (all-or-nothing)
//! through [`validate_and_commit`] — so the DRC-clean invariant is untouched.

use vcad_ir::ecad::{Pcb, PcbLayer};
use vcad_ir::Vec2;

use crate::session::RouteSession;

use super::auto::{copper_layers, dist, validate_and_commit, Candidate, Placed};
use super::congestion::Congestion;
use super::diff_pair::offset_polyline;
use super::maze::route_net_maze3d;

/// The pair partner of `net`, derived from its name. Recognizes:
/// - trailing `_P` ↔ `_N` (any case): `/PCIe.TX_P` ↔ `/PCIe.TX_N`
/// - a bare polarity token after a final `.`: `/X.P` ↔ `/X.N`
/// - DDR true/complement `_C` ↔ `_T` tokens, as a suffix or mid-name
///   (`DQS1_C_B` ↔ `DQS1_T_B`, `CK_C` ↔ `CK_T`)
///
/// Returns `None` for names with no polarity marker (`GND`, `/GPIO4`).
pub(crate) fn pair_partner(net: &str) -> Option<String> {
    // Trailing `_P`/`_N` (the overwhelmingly common convention).
    for (a, b) in [("_P", "_N"), ("_N", "_P"), ("_p", "_n"), ("_n", "_p")] {
        if let Some(base) = net.strip_suffix(a) {
            if !base.is_empty() {
                return Some(format!("{base}{b}"));
            }
        }
    }
    // A bare `P`/`N` token after the final `.`.
    for (a, b) in [(".P", ".N"), (".N", ".P")] {
        if let Some(base) = net.strip_suffix(a) {
            if !base.is_empty() {
                return Some(format!("{base}{b}"));
            }
        }
    }
    // DDR true/complement: swap the last `_C`/`_T` token (suffix or `_C_…`).
    for (a, b) in [("_C", "_T"), ("_T", "_C")] {
        if let Some(base) = net.strip_suffix(a) {
            if !base.is_empty() {
                return Some(format!("{base}{b}"));
            }
        }
        let mid_a = format!("{a}_");
        let mid_b = format!("{b}_");
        if let Some(idx) = net.rfind(&mid_a) {
            if idx > 0 {
                let mut s = net.to_string();
                s.replace_range(idx..idx + mid_a.len(), &mid_b);
                return Some(s);
            }
        }
    }
    None
}

/// The pair's leg width and gap: from a declared diff-pair net class containing
/// `net` when one exists, else the session width and `clearance · 1.5`.
fn pair_geometry(session: &RouteSession, pcb: &Pcb, net: &str, width: f64) -> (f64, f64) {
    for class in &pcb.rules.class_rules {
        let Some(gap) = class.diff_pair_gap else {
            continue;
        };
        let Some(nets) = pcb.rules.net_class_assignments.get(&class.name) else {
            continue;
        };
        if nets.iter().any(|n| n == net) {
            return (class.diff_pair_width.unwrap_or(class.trace_width), gap);
        }
    }
    (
        session.width_for(net, width),
        session.clearance_for(net) * 1.5,
    )
}

/// World positions of every pad on `net`.
fn pads_of_net(pcb: &Pcb, net: &str) -> Vec<Vec2> {
    let mut out = Vec::new();
    for fp in &pcb.footprints {
        for pad in &fp.pads {
            if pad.net.as_deref() == Some(net) {
                out.push(crate::geometry::pad_world_position(fp, pad));
            }
        }
    }
    out
}

/// The pad of `pads` nearest to `p`.
fn nearest_pad(pads: &[Vec2], p: Vec2) -> Option<Vec2> {
    pads.iter()
        .copied()
        .min_by(|a, b| dist(*a, p).partial_cmp(&dist(*b, p)).unwrap())
}

/// One realized leg of the pair: its segments, vias, and endpoints.
struct Leg {
    segments: Vec<(Vec2, Vec2, PcbLayer)>,
    vias: Vec<(Vec2, PcbLayer, PcbLayer)>,
    first: Vec2,
    first_layer: PcbLayer,
    last: Vec2,
    last_layer: PcbLayer,
}

/// Route `net` and its pair partner as a coupled differential pair.
///
/// Searches one phantom fat trace (`2·w + gap` wide) between the endpoint
/// midpoints, offsets it into two legs at ±(w+gap)/2, connects the legs to the
/// four pads, and commits both nets atomically: if either candidate fails
/// validation, everything is rolled back (including any partner routes that
/// were ripped to make room) and the session is left exactly as found.
///
/// Returns `(placed_for_net, placed_for_partner)` on success.
#[allow(clippy::too_many_arguments)]
pub(super) fn try_route_pair(
    session: &mut RouteSession,
    pcb: &Pcb,
    width: f64,
    net: &str,
    from: Vec2,
    to: Vec2,
    placed: &mut Vec<Placed>,
    cong: &Congestion,
    max_expansions: usize,
) -> Option<(Placed, Placed)> {
    let partner = pair_partner(net)?;
    let partner_pads = pads_of_net(pcb, &partner);
    if partner_pads.is_empty() {
        log::debug!("pair: {net} has partner name {partner} but no pads on board");
        return None;
    }
    // MST-similar partner endpoints: the partner pads nearest to this
    // connection's own endpoints. They must be two distinct pads.
    let p_from = nearest_pad(&partner_pads, from)?;
    let p_to = nearest_pad(&partner_pads, to)?;
    if dist(p_from, p_to) < 1e-6 {
        log::debug!("pair: {net}/{partner}: partner endpoints collapse to one pad — bailing");
        return None;
    }

    let (w, gap) = pair_geometry(session, pcb, net, width);
    let half_sep = (w + gap) / 2.0;
    let fat_w = 2.0 * w + gap;
    let via_d = pcb.rules.default_rules.via_diameter;
    let clearance = session.clearance_for(net);
    let copper = copper_layers(pcb);
    let first_layer = *copper.first().unwrap_or(&PcbLayer::FCu);

    // Centerline endpoints: the midpoints, pushed toward each other by a lead
    // so the fat phantom clears the four pads (the pads are other-net copper
    // from the phantom's perspective at the twin's end of the corridor).
    let mid_from = Vec2::new((from.x + p_from.x) / 2.0, (from.y + p_from.y) / 2.0);
    let mid_to = Vec2::new((to.x + p_to.x) / 2.0, (to.y + p_to.y) / 2.0);
    let span = dist(mid_from, mid_to);
    let lead = (fat_w + clearance + w).min(span * 0.25);
    if span < 4.0 * lead.max(0.1) {
        log::debug!("pair: {net}/{partner}: span {span:.2}mm too short for coupled routing");
        return None;
    }
    let dir = {
        let d = mid_to - mid_from;
        d.scale(1.0 / span)
    };
    let start = mid_from + dir.scale(lead);
    let end = mid_to - dir.scale(lead);

    // Rip the partner's committed routes overlapping this span so the phantom
    // corridor is clean; remember the originals for restore-on-failure. Routes
    // with fan-out stubs are left in place (stubs can't ride a Candidate).
    let lo = [
        mid_from.x.min(mid_to.x) - fat_w - clearance,
        mid_from.y.min(mid_to.y) - fat_w - clearance,
    ];
    let hi = [
        mid_from.x.max(mid_to.x) + fat_w + clearance,
        mid_from.y.max(mid_to.y) + fat_w + clearance,
    ];
    let overlaps_span = |p: &Placed| {
        p.segments.iter().any(|(a, b, _)| {
            let slo = [a.x.min(b.x), a.y.min(b.y)];
            let shi = [a.x.max(b.x), a.y.max(b.y)];
            slo[0] <= hi[0] && lo[0] <= shi[0] && slo[1] <= hi[1] && lo[1] <= shi[1]
        })
    };
    let mut ripped: Vec<Placed> = Vec::new();
    let mut kept: Vec<Placed> = Vec::new();
    for p in std::mem::take(placed) {
        if p.net == partner && p.stubs.is_empty() && overlaps_span(&p) {
            for &s in &p.spans {
                session.remove(s);
            }
            ripped.push(p);
        } else {
            kept.push(p);
        }
    }
    *placed = kept;

    // Restore-on-bail: re-commit the ripped partner routes (their space is
    // still free — nothing was committed since the rip).
    let restore = |session: &mut RouteSession, placed: &mut Vec<Placed>, ripped: Vec<Placed>| {
        for orig in ripped {
            let cand = Candidate {
                thin_segments: vec![],
                thin_width: orig.width,
                net: orig.net.clone(),
                from: orig.from,
                to: orig.to,
                width: orig.width,
                segments: orig.segments.clone(),
                vias: orig.via_pts.clone(),
            };
            match validate_and_commit(session, pcb, cand, placed) {
                Some(p) => placed.push(p),
                None => log::debug!("pair: could not restore a ripped {} route", orig.net),
            }
        }
    };

    // Centerline search: one phantom fat trace. No tree goals/sources — a
    // coupled pair needs clean pad-to-pad geometry, not a tap onto a tree.
    //
    // Neck-down retreat (the CM5 bail census: 96/110 failures were this
    // search): near pin fields no fat capsule exists, so on failure the
    // coupling endpoints retreat toward the span middle in escalating steps
    // and the single-width connector stubs cover the necked ends — exactly
    // how a human escapes a BGA with a pair: singles in the field, coupled
    // in the open.
    let budget = max_expansions.max(100_000);
    let mut found = None;
    // Asymmetric ladder: most pairs have only ONE end in a pin field, so
    // necking both ends symmetrically wastes coupled length and misses the
    // cases where only one side needs the retreat.
    for (r_from, r_to) in [
        (0.0, 0.0),
        (1.0, 0.0),
        (0.0, 1.0),
        (1.0, 1.0),
        (2.0, 0.0),
        (0.0, 2.0),
        (2.0, 2.0),
        (4.0, 0.0),
        (0.0, 4.0),
        (4.0, 4.0),
        (8.0, 8.0),
    ] {
        let usable = span - 2.0 * lead;
        if r_from + r_to >= usable - 1.0 {
            continue;
        }
        let s_pt = start + dir.scale(r_from);
        let e_pt = end - dir.scale(r_to);
        let r = route_net_maze3d(
            session,
            &pcb.outline.vertices,
            &copper,
            net,
            s_pt,
            &[first_layer],
            e_pt,
            &[first_layer],
            fat_w,
            via_d,
            Some(cong),
            budget,
            1.0,
            None,
            &[],
            &[],
            true,
        );
        if r.success && !r.segments.is_empty() {
            if r_from + r_to > 0.0 {
                log::debug!(
                    "pair: {net}/{partner}: coupled after {r_from}/{r_to}mm neck-down retreat"
                );
            }
            found = Some(r);
            break;
        }
    }
    let Some(r) = found else {
        log::debug!("pair: {net}/{partner}: phantom centerline search failed");
        restore(session, placed, ripped);
        return None;
    };

    // Realize the two legs from the centerline.
    let via_off = half_sep.max((via_d + clearance) / 2.0 + 0.01);
    let (leg_a, leg_b) = match realize_legs(&r.segments, half_sep, via_off) {
        Some(l) => l,
        None => {
            log::debug!("pair: {net}/{partner}: degenerate centerline — bailing");
            restore(session, placed, ripped);
            return None;
        }
    };

    // Leg → net assignment: the one whose pad connectors are shortest (the
    // non-crossing assignment — crossing connectors are strictly longer).
    let cost = |leg: &Leg, s: Vec2, e: Vec2| dist(s, leg.first) + dist(e, leg.last);
    let a_mine = cost(&leg_a, from, to) + cost(&leg_b, p_from, p_to)
        <= cost(&leg_b, from, to) + cost(&leg_a, p_from, p_to);
    let (mine, theirs) = if a_mine {
        (leg_a, leg_b)
    } else {
        (leg_b, leg_a)
    };

    // Connector from a pad to a leg end: straight when short, maze-routed at
    // single width when the crow-flight would thread a pin field (the second
    // CM5 bail census: 71 leg validations failed on straight stubs crossing
    // field copper after the neck-down retreat).
    // Neck-down width for pad connectors: the board's default single-ended
    // width. A 0.2mm pair leg cannot pass between 0.4mm-pitch pads; the
    // 0.08mm single can — exactly how the human escapes these fields.
    let nw = pcb.rules.default_rules.trace_width.min(w);
    type ConnectorCopper = (Vec<(Vec2, Vec2, PcbLayer)>, Vec<(Vec2, PcbLayer, PcbLayer)>);
    let connect = |session: &RouteSession,
                   net: &str,
                   from: Vec2,
                   to: Vec2,
                   to_layer: PcbLayer|
     -> Option<ConnectorCopper> {
        if dist(from, to) <= 1e-9 {
            return Some((vec![], vec![]));
        }
        if dist(from, to) <= nw * 2.0 {
            return Some((vec![(from, to, to_layer)], vec![]));
        }
        let margin = 4.0 + w;
        let window = (
            Vec2::new(from.x.min(to.x) - margin, from.y.min(to.y) - margin),
            Vec2::new(from.x.max(to.x) + margin, from.y.max(to.y) + margin),
        );
        let r = route_net_maze3d(
            session,
            &pcb.outline.vertices,
            &copper,
            net,
            from,
            &copper,
            to,
            &[to_layer],
            nw,
            via_d,
            Some(cong),
            120_000,
            0.5,
            Some(window),
            &[],
            &[],
            true,
        );
        if r.success {
            Some((r.segments, r.vias))
        } else {
            log::debug!(
                "pair-connector: {net} {:.2}mm ({:.2},{:.2})->({:.2},{:.2}) layer {:?} failed",
                dist(from, to),
                from.x,
                from.y,
                to.x,
                to.y,
                to_layer
            );
            None
        }
    };
    let build =
        |session: &RouteSession, leg: &Leg, net: &str, from: Vec2, to: Vec2| -> Option<Candidate> {
            let (head_segs, head_vias) = connect(session, net, from, leg.first, leg.first_layer)?;
            let (tail_segs, tail_vias) = connect(session, net, to, leg.last, leg.last_layer)?;
            // Connectors are the neck-down: they commit at `nw` via the thin
            // channel, while the coupled leg stays at the class width.
            let mut thin = head_segs;
            // Tail connector was searched pad→leg; reverse into leg→pad order.
            thin.extend(tail_segs.into_iter().rev().map(|(a, b, l)| (b, a, l)));
            let mut vias = leg.vias.clone();
            vias.extend(head_vias);
            vias.extend(tail_vias);
            Some(Candidate {
                net: net.to_string(),
                from,
                to,
                width: w,
                segments: leg.segments.clone(),
                vias,
                thin_segments: thin,
                thin_width: nw,
            })
        };
    // Build-and-commit SEQUENTIALLY: leg 2's connectors are searched against
    // the session that already holds leg 1's copper, so they route around it
    // instead of colliding in the necked corridor (69 of the census-3 bails).
    // Atomicity is preserved by the rollback below.
    let Some(cand_mine) = build(session, &mine, net, from, to) else {
        log::debug!("pair: {net}/{partner}: connector routing failed (mine)");
        restore(session, placed, ripped);
        return None;
    };
    let Some(placed_mine) = validate_and_commit(session, pcb, cand_mine, placed) else {
        log::debug!("pair: {net}/{partner}: leg 1 failed validation");
        restore(session, placed, ripped);
        return None;
    };
    let Some(cand_theirs) = build(session, &theirs, &partner, p_from, p_to) else {
        log::debug!("pair: {net}/{partner}: connector routing failed (partner)");
        for &sp in &placed_mine.spans {
            session.remove(sp);
        }
        restore(session, placed, ripped);
        return None;
    };
    let Some(placed_theirs) = validate_and_commit(session, pcb, cand_theirs, placed) else {
        // Leg 2 failed: rip leg 1 back out, restore the partner originals.
        for &s in &placed_mine.spans {
            session.remove(s);
        }
        log::debug!("pair: {net}/{partner}: leg 2 failed validation — rolled back leg 1");
        restore(session, placed, ripped);
        return None;
    };

    let center_len: f64 = r.segments.iter().map(|(a, b, _)| dist(*a, *b)).sum();
    log::info!(
        "pair: routed {net} + {partner} coupled (centerline {:.1}mm, {} via pair(s), w={w} gap={gap})",
        center_len,
        placed_mine.via_pts.len(),
    );
    Some((placed_mine, placed_theirs))
}

/// Offset the centerline into two legs at ±`half_sep`, with per-leg vias offset
/// ±`via_off` perpendicular at each layer transition (two drills per centerline
/// via, one per leg) and connector jogs where the via offset exceeds the leg
/// offset. Leg A is the +offset side, leg B the −offset side.
fn realize_legs(
    center: &[(Vec2, Vec2, PcbLayer)],
    half_sep: f64,
    via_off: f64,
) -> Option<(Leg, Leg)> {
    // Group the centerline into contiguous same-layer polyline runs.
    let mut runs: Vec<(PcbLayer, Vec<Vec2>)> = Vec::new();
    for &(a, b, l) in center {
        if dist(a, b) < 1e-9 {
            continue;
        }
        match runs.last_mut() {
            Some((rl, pts)) if *rl == l && dist(*pts.last().unwrap(), a) < 1e-6 => pts.push(b),
            _ => runs.push((l, vec![a, b])),
        }
    }
    if runs.is_empty() || runs.iter().any(|(_, pts)| pts.len() < 2) {
        return None;
    }

    let leg = |sign: f64| -> Option<Leg> {
        let mut segments: Vec<(Vec2, Vec2, PcbLayer)> = Vec::new();
        let mut vias: Vec<(Vec2, PcbLayer, PcbLayer)> = Vec::new();
        let mut first: Option<(Vec2, PcbLayer)> = None;
        let mut prev_end: Option<(Vec2, PcbLayer)> = None;
        for (layer, pts) in &runs {
            let off = offset_polyline(pts, sign * half_sep);
            if off.len() < 2 {
                return None;
            }
            if first.is_none() {
                first = Some((off[0], *layer));
            }
            // Layer transition from the previous run: one via for this leg,
            // offset perpendicular from the centerline junction, with jog
            // connectors on both layers when it doesn't sit on the leg line.
            if let Some((pe, pl)) = prev_end {
                let junction = pts[0];
                // Perpendicular at the junction: direction of the new run's
                // first segment (matches the offset used for the leg points).
                let d = (pts[1] - pts[0]).normalize();
                let n = d.perp();
                let vp = junction + n.scale(sign * via_off);
                if dist(pe, vp) > 1e-9 {
                    segments.push((pe, vp, pl));
                }
                if dist(vp, off[0]) > 1e-9 {
                    segments.push((vp, off[0], *layer));
                }
                vias.push((vp, pl, *layer));
            }
            for wpair in off.windows(2) {
                if dist(wpair[0], wpair[1]) > 1e-9 {
                    segments.push((wpair[0], wpair[1], *layer));
                }
            }
            prev_end = Some((*off.last().unwrap(), *layer));
        }
        let (first, first_layer) = first?;
        let (last, last_layer) = prev_end?;
        Some(Leg {
            segments,
            vias,
            first,
            first_layer,
            last,
            last_layer,
        })
    };
    Some((leg(1.0)?, leg(-1.0)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::RouteSession;
    use vcad_ir::ecad::*;

    #[test]
    fn pair_partner_maps_real_names() {
        assert_eq!(pair_partner("/PCIe.TX_P").as_deref(), Some("/PCIe.TX_N"));
        assert_eq!(pair_partner("/PCIe.TX_N").as_deref(), Some("/PCIe.TX_P"));
        assert_eq!(pair_partner("/ETH.2_P").as_deref(), Some("/ETH.2_N"));
        assert_eq!(
            pair_partner("/LPDDR4 RAM/DQS1_C_B").as_deref(),
            Some("/LPDDR4 RAM/DQS1_T_B")
        );
        assert_eq!(
            pair_partner("/LPDDR4 RAM/CK_T_B").as_deref(),
            Some("/LPDDR4 RAM/CK_C_B")
        );
        assert_eq!(pair_partner("USB_D_P").as_deref(), Some("USB_D_N"));
        assert_eq!(pair_partner("CK_C").as_deref(), Some("CK_T"));
        // Negatives: no polarity marker.
        assert_eq!(pair_partner("GND"), None);
        assert_eq!(pair_partner("/GPIO4"), None);
        assert_eq!(pair_partner("VCC3V3"), None);
        assert_eq!(pair_partner("_P"), None);
    }

    fn pad(num: &str, x: f64, y: f64, net: &str) -> Pad {
        Pad {
            number: num.into(),
            pad_type: PadType::SMD,
            shape: PadShape::Circle { diameter: 0.3 },
            position: Vec2::new(x, y),
            rotation: 0.0,
            drill: None,
            net: Some(net.into()),
            layers: vec![PcbLayer::FCu],
        }
    }

    fn fp(reference: &str, x: f64, y: f64, pads: Vec<Pad>) -> Footprint {
        Footprint {
            reference: reference.into(),
            value: "x".into(),
            footprint_name: "test".into(),
            position: Vec2::new(x, y),
            rotation: 0.0,
            front: true,
            pads,
            graphics: vec![],
            model_3d: None,
            properties: Default::default(),
        }
    }

    /// Open two-layer board with a P/N pad pair at each end (0.65 mm apart,
    /// perpendicular to the horizontal route) and a diff-pair class declared
    /// at width 0.2 / gap 0.25.
    fn pair_board() -> Pcb {
        let mut pcb = Pcb {
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
                    clearance: 0.15,
                    via_diameter: 0.8,
                    via_drill: 0.4,
                    diff_pair_gap: None,
                    diff_pair_width: None,
                },
                class_rules: vec![NetClassRules {
                    name: "DIFF".into(),
                    trace_width: 0.2,
                    clearance: 0.15,
                    via_diameter: 0.8,
                    via_drill: 0.4,
                    diff_pair_gap: Some(0.25),
                    diff_pair_width: Some(0.2),
                }],
                net_class_assignments: Default::default(),
                edge_clearance: 0.5,
                hole_to_hole: 0.5,
                min_annular_ring: 0.15,
                min_drill: 0.2,
            },
            footprints: vec![
                fp(
                    "J1",
                    5.0,
                    15.0,
                    vec![
                        pad("1", 0.0, 0.325, "/ETH.2_P"),
                        pad("2", 0.0, -0.325, "/ETH.2_N"),
                    ],
                ),
                fp(
                    "U1",
                    45.0,
                    15.0,
                    vec![
                        pad("1", 0.0, 0.325, "/ETH.2_P"),
                        pad("2", 0.0, -0.325, "/ETH.2_N"),
                    ],
                ),
            ],
            traces: vec![],
            trace_arcs: vec![],
            vias: vec![],
            zones: vec![],
            keepouts: vec![],
            net_ties: vec![],
        };
        pcb.rules
            .net_class_assignments
            .insert("DIFF".into(), vec!["/ETH.2_P".into(), "/ETH.2_N".into()]);
        pcb
    }

    #[test]
    fn routes_open_board_pair_coupled() {
        let pcb = pair_board();
        let mut session = RouteSession::from_pcb(&pcb);
        let cong = Congestion::new(&pcb.outline.vertices);
        let mut placed = Vec::new();
        let from = Vec2::new(5.0, 15.325);
        let to = Vec2::new(45.0, 15.325);
        let result = try_route_pair(
            &mut session,
            &pcb,
            0.25,
            "/ETH.2_P",
            from,
            to,
            &mut placed,
            &cong,
            200_000,
        );
        let (p, n) = result.expect("open-board pair must route coupled");
        assert_eq!(p.net, "/ETH.2_P");
        assert_eq!(n.net, "/ETH.2_N");
        assert!(!p.segments.is_empty() && !n.segments.is_empty());
        // Class geometry: legs at width 0.2, centers (w + gap) = 0.45 apart
        // along the straight coupled run.
        assert!((p.width - 0.2).abs() < 1e-9);
        // Sample the coupled mid-span: the horizontal legs' y-separation.
        let leg_y = |pl: &Placed| -> Option<f64> {
            pl.segments
                .iter()
                .find(|(a, b, _)| (a.y - b.y).abs() < 1e-6 && (a.x - b.x).abs() > 5.0)
                .map(|(a, _, _)| a.y)
        };
        if let (Some(py), Some(ny)) = (leg_y(&p), leg_y(&n)) {
            let sep = (py - ny).abs();
            assert!(
                (sep - 0.45).abs() < 0.02,
                "leg separation {sep} should be w+gap=0.45"
            );
            // No crossing: P stays on the P-pad side (above the centerline).
            assert!(py > ny, "P leg must stay on the P-pad side");
        } else {
            panic!("expected a long horizontal coupled run in both legs");
        }
        // Every committed leg segment probes legal on a fresh session.
        let fresh = RouteSession::from_pcb(&pcb);
        for pl in [&p, &n] {
            // The twin leg is same-session copper here, but on a FRESH session
            // only board copper exists — each leg must clear the pads.
            for (a, b, l) in &pl.segments {
                let seg = crate::spatial::CopperGeom::Segment {
                    a: *a,
                    b: *b,
                    half_w: pl.width / 2.0,
                };
                assert!(
                    fresh
                        .probe(&seg, *l, &pl.net, fresh.clearance_for(&pl.net))
                        .legal,
                    "pair leg emitted illegal copper for {}: {a:?}->{b:?}",
                    pl.net
                );
            }
        }
    }

    #[test]
    fn atomicity_blocked_leg_commits_nothing() {
        let mut pcb = pair_board();
        // Blocker pad in the N leg's end-connector corridor (between the fat
        // corridor's end and the N pad at U1), clear of the P connector: the
        // phantom search cannot see it (it lies past the centerline lead), so
        // leg validation is what catches it — exercising the rollback path.
        pcb.footprints
            .push(fp("R9", 43.6, 14.55, vec![pad("1", 0.0, 0.0, "BLOCK")]));
        let mut session = RouteSession::from_pcb(&pcb);
        let baseline = session.len();
        let cong = Congestion::new(&pcb.outline.vertices);
        let mut placed = Vec::new();
        let result = try_route_pair(
            &mut session,
            &pcb,
            0.25,
            "/ETH.2_P",
            Vec2::new(5.0, 15.325),
            Vec2::new(45.0, 15.325),
            &mut placed,
            &cong,
            200_000,
        );
        // Whether the search bails or a leg fails validation, the contract is
        // the same: on None, NOTHING was committed.
        match result {
            None => {
                assert_eq!(
                    session.len(),
                    baseline,
                    "failed pair routing must leave the session untouched"
                );
                assert!(placed.is_empty());
            }
            // If it routed around the blocker, that's fine too — but then both
            // legs must exist and be legal (covered by validate_and_commit).
            Some((p, n)) => {
                assert!(!p.segments.is_empty() && !n.segments.is_empty());
            }
        }
    }

    #[test]
    fn bails_when_partner_absent() {
        let mut pcb = pair_board();
        // Rename the N pads away: partner name resolves but has no pads.
        for f in &mut pcb.footprints {
            for p in &mut f.pads {
                if p.net.as_deref() == Some("/ETH.2_N") {
                    p.net = Some("OTHER".into());
                }
            }
        }
        let mut session = RouteSession::from_pcb(&pcb);
        let baseline = session.len();
        let cong = Congestion::new(&pcb.outline.vertices);
        let mut placed = Vec::new();
        assert!(try_route_pair(
            &mut session,
            &pcb,
            0.25,
            "/ETH.2_P",
            Vec2::new(5.0, 15.325),
            Vec2::new(45.0, 15.325),
            &mut placed,
            &cong,
            200_000,
        )
        .is_none());
        assert_eq!(session.len(), baseline);
    }
}
