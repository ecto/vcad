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

use vcad_ir::ecad::{Pcb, PcbLayer, Trace, Via};
use vcad_ir::Vec2;

use crate::session::RouteSession;
use crate::spatial::CopperGeom;

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
    let via_d = pcb.rules.default_rules.via_diameter;
    let clearance = session.clearance_for(net);
    // The phantom corridor must cover everything realize_legs emits: the two
    // legs AND the via dog-bones, which sit at ±via_off with via_d discs
    // (census 11: leg segments/jogs stepped outside a 2w+gap corridor onto
    // unprobed copper whenever the search dropped a layer change).
    let via_off = half_sep.max((via_d + clearance) / 2.0 + 0.01);
    let voff_max = via_off.max(via_d / 2.0 + 1.5 * clearance + w / 2.0 - half_sep);
    let fat_w = (2.0 * w + gap).max(2.0 * (voff_max + via_d / 2.0));
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
    let (leg_a, leg_b) =
        match realize_legs(&r.segments, half_sep, via_off, via_d / 2.0, clearance, w) {
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
                   to_layer: PcbLayer,
                   leg: &Leg|
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
        // Multi-goal attachment: the connector may terminate anywhere along
        // the leg's copper, not only at its endpoint — an offset leg end can
        // sit against a neighbouring pad where no legal approach exists.
        let goals: Vec<(CopperGeom, [f64; 2], [f64; 2], PcbLayer)> = leg
            .segments
            .iter()
            .map(|&(a, b, l)| {
                (
                    CopperGeom::Segment {
                        a,
                        b,
                        half_w: w / 2.0,
                    },
                    [a.x.min(b.x) - w, a.y.min(b.y) - w],
                    [a.x.max(b.x) + w, a.y.max(b.y) + w],
                    l,
                )
            })
            .collect();
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
            &goals,
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
    // Ordering (census 9's lesson): commit BOTH legs first — they are
    // parallel and disjoint by construction — then route the four pad
    // connectors against the complete picture, so no connector can cut the
    // coupled corridor before the other leg exists. Rollback keeps the
    // all-or-nothing contract.
    let leg_cand = |leg: &Leg, net: &str, from: Vec2, to: Vec2| -> Candidate {
        Candidate {
            net: net.to_string(),
            from,
            to,
            width: w,
            segments: leg.segments.clone(),
            vias: leg.vias.clone(),
            thin_segments: vec![],
            thin_width: nw,
        }
    };
    let Some(mut placed_mine) =
        validate_and_commit(session, pcb, leg_cand(&mine, net, from, to), placed)
    else {
        log::debug!("pair: {net}/{partner}: leg 1 failed validation");
        restore(session, placed, ripped);
        return None;
    };
    let Some(mut placed_theirs) = validate_and_commit(
        session,
        pcb,
        leg_cand(&theirs, &partner, p_from, p_to),
        placed,
    ) else {
        log::debug!("pair: {net}/{partner}: leg 2 failed validation — rolled back leg 1");
        for &sp in &placed_mine.spans {
            session.remove(sp);
        }
        restore(session, placed, ripped);
        return None;
    };
    // Connectors, all four against both committed legs. Committed as thin
    // copper directly into each leg's Placed (stubs channel).
    let rollback_all = |session: &mut RouteSession,
                        placed: &mut Vec<Placed>,
                        a: &Placed,
                        b: &Placed,
                        ripped: Vec<Placed>| {
        for &sp in a.spans.iter().chain(b.spans.iter()) {
            session.remove(sp);
        }
        restore(session, placed, ripped);
    };
    let attach = |session: &mut RouteSession,
                  pl: &mut Placed,
                  leg: &Leg,
                  pad_a: Vec2,
                  pad_b: Vec2|
     -> bool {
        let net = pl.net.clone();
        let Some((head, hv)) = connect(session, &net, pad_a, leg.first, leg.first_layer, leg)
        else {
            log::debug!("pair-connector: {net} head failed");
            return false;
        };
        for (a, b, l) in &head {
            let id = crate::spatial::CopperElement {
                min: [a.x.min(b.x) - nw, a.y.min(b.y) - nw],
                max: [a.x.max(b.x) + nw, a.y.max(b.y) + nw],
                net: net.clone(),
                layer: *l,
                geom: CopperGeom::Segment {
                    a: *a,
                    b: *b,
                    half_w: nw / 2.0,
                },
            };
            pl.spans.push(session.commit(id));
            pl.stubs.push((*a, *b, *l));
        }
        pl.via_pts.extend(hv);
        let Some((tail, tv)) = connect(session, &net, pad_b, leg.last, leg.last_layer, leg) else {
            log::debug!("pair-connector: {net} tail failed");
            return false;
        };
        for (a, b, l) in &tail {
            let id = crate::spatial::CopperElement {
                min: [a.x.min(b.x) - nw, a.y.min(b.y) - nw],
                max: [a.x.max(b.x) + nw, a.y.max(b.y) + nw],
                net: net.clone(),
                layer: *l,
                geom: CopperGeom::Segment {
                    a: *a,
                    b: *b,
                    half_w: nw / 2.0,
                },
            };
            pl.spans.push(session.commit(id));
            pl.stubs.push((*a, *b, *l));
        }
        pl.via_pts.extend(tv);
        true
    };
    if !attach(session, &mut placed_mine, &mine, from, to)
        || !attach(session, &mut placed_theirs, &theirs, p_from, p_to)
    {
        rollback_all(session, placed, &placed_mine, &placed_theirs, ripped);
        return None;
    }

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
    _via_off: f64,
    via_r: f64,
    clearance: f64,
    w: f64,
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

    // Simplify each centerline run before offsetting: merge collinear steps
    // and dissolve segments shorter than the offset distance. Offsetting a
    // grid staircase whose steps are shorter than half_sep folds the offset
    // polyline back over itself — census 14's twin-blocker shorts.
    let min_seg = half_sep * 2.0;
    let simplify = |pts: &[Vec2]| -> Vec<Vec2> {
        let mut out: Vec<Vec2> = vec![pts[0]];
        for &p in &pts[1..pts.len() - 1] {
            let a = *out.last().unwrap();
            if dist(a, p) < min_seg {
                continue;
            }
            // Drop collinear interior points.
            if out.len() >= 2 {
                let b = out[out.len() - 2];
                let d0 = (a - b).normalize();
                let d1 = (p - a).normalize();
                if (d0.x * d1.y - d0.y * d1.x).abs() < 1e-9 && d0.dot(d1) > 0.0 {
                    out.pop();
                }
            }
            out.push(p);
        }
        let last = *pts.last().unwrap();
        if out.len() >= 2 && dist(*out.last().unwrap(), last) < min_seg {
            out.pop();
        }
        out.push(last);
        out
    };
    let runs: Vec<(PcbLayer, Vec<Vec2>)> = runs
        .into_iter()
        .map(|(l, pts)| (l, simplify(&pts)))
        .collect();
    if runs.iter().any(|(_, pts)| pts.len() < 2) {
        return None;
    }

    // One combined centerline: runs concatenated with junction vertices kept
    // (a junction is an interior vertex of the whole polyline, so both its
    // sides offset with ONE mitered normal — per-run offsetting gave each
    // side a different normal and shaved the pair gap at every junction).
    let mut pts: Vec<Vec2> = Vec::new();
    let mut seg_layers: Vec<PcbLayer> = Vec::new();
    for (layer, rp) in &runs {
        for (k, p) in rp.iter().enumerate() {
            match pts.last() {
                Some(last) if dist(*last, *p) < 1e-6 => {
                    if k > 0 {
                        // interior duplicate — skip
                    }
                }
                _ => pts.push(*p),
            }
            if pts.len() >= 2 && seg_layers.len() < pts.len() - 1 {
                seg_layers.push(*layer);
            }
        }
    }
    // Trim terminal reversals (maze end-approach overshoot-and-return):
    // offsetting a cusp hurls one leg across the other's lane. Interior
    // cusps (rare) make the pair bail rather than emit crossing copper.
    let rev = |a: Vec2, b: Vec2, c: Vec2| -> bool {
        let d0 = (b - a).normalize();
        let d1 = (c - b).normalize();
        d0.dot(d1) < -0.5
    };
    while pts.len() >= 3 && rev(pts[pts.len() - 3], pts[pts.len() - 2], pts[pts.len() - 1]) {
        pts.pop();
        seg_layers.pop();
    }
    while pts.len() >= 3 && rev(pts[0], pts[1], pts[2]) {
        pts.remove(0);
        seg_layers.remove(0);
    }
    for k in 1..pts.len().saturating_sub(1) {
        if rev(pts[k - 1], pts[k], pts[k + 1]) {
            log::debug!("pair: interior centerline cusp — bailing");
            return None;
        }
    }
    if pts.len() < 2 || seg_layers.len() != pts.len() - 1 {
        log::debug!(
            "pair: combined centerline malformed: {} pts {} seg layers",
            pts.len(),
            seg_layers.len()
        );
        return None;
    }
    // In-line vias demand the disc clears the other leg's line — but only
    // when the centerline actually changes layers.
    let has_transition = seg_layers.windows(2).any(|w2| w2[0] != w2[1]);
    if has_transition && 2.0 * half_sep < via_r + clearance + w / 2.0 + 0.01 {
        log::debug!(
            "pair: in-line via gate: 2*half_sep {} too small",
            2.0 * half_sep
        );
        return None;
    }
    let need = 2.0 * via_r + clearance + 0.04;
    let lat = 2.0 * half_sep;
    let stagger = if lat >= need {
        0.0
    } else {
        (need * need - lat * lat).sqrt()
    };

    let leg = |sign: f64| -> Option<Leg> {
        let off = offset_polyline(&pts, sign * half_sep);
        let other = offset_polyline(&pts, -sign * half_sep);
        if off.len() != pts.len() || other.len() != pts.len() {
            return None;
        }
        let mut segments: Vec<(Vec2, Vec2, PcbLayer)> = Vec::new();
        let mut vias: Vec<(Vec2, PcbLayer, PcbLayer)> = Vec::new();
        for k in 0..off.len() - 1 {
            let (a, b, l) = (off[k], off[k + 1], seg_layers[k]);
            // Layer change at vertex k (k>0): via on this leg's own line at
            // the mitered junction vertex; the `sign<0` leg pulls its via
            // back along its previous segment so the two discs clear.
            if k > 0 && seg_layers[k - 1] != l {
                let pl = seg_layers[k - 1];
                let vp = if sign > 0.0 {
                    off[k]
                } else {
                    // Pull back along this leg's own segment until the disc
                    // truly clears the twin's via at ITS mitered vertex —
                    // the nominal stagger under-counts when the miter shifts
                    // the twin's vertex along the corner.
                    let (pa, pb) = (off[k - 1], off[k]);
                    let seg_len = dist(pa, pb);
                    let d = (pb - pa).normalize();
                    let need_d = 2.0 * via_r + clearance + 0.04;
                    let mut t = stagger.min(seg_len - 1e-6).max(0.0);
                    let mut vp = pb - d.scale(t);
                    while dist(vp, other[k]) < need_d && t + 0.05 < seg_len {
                        t += 0.05;
                        vp = pb - d.scale(t);
                    }
                    if dist(vp, other[k]) < need_d {
                        return None;
                    }
                    vp
                };
                // Retrace from the (possibly pulled-back) via to the mitered
                // vertex on the NEW layer, staying on this leg's line.
                if dist(vp, off[k]) > 1e-9 {
                    segments.push((vp, off[k], l));
                }
                vias.push((vp, pl, l));
            }
            if dist(a, b) > 1e-9 {
                segments.push((a, b, l));
            }
        }
        let first = *off.first().unwrap();
        let last = *off.last().unwrap();
        Some(Leg {
            segments,
            vias,
            first,
            first_layer: seg_layers[0],
            last,
            last_layer: *seg_layers.last().unwrap(),
        })
    };
    Some((leg(1.0)?, leg(-1.0)?))
}

/// Pair polish: on a FINISHED board, rip each still-uncoupled pair and
/// re-route it coupled against the settled copper. The board is quiet, the
/// pair gets a focused high-effort attempt, and failure restores the
/// original copper — strictly non-regressive per pair.
///
/// Returns `(polished, attempted)`.
pub fn polish_pairs(pcb: &mut Pcb, effort_expansions: usize) -> (usize, usize) {
    let nets: Vec<String> = {
        let mut v: std::collections::BTreeSet<String> = Default::default();
        for f in &pcb.footprints {
            for pad in &f.pads {
                if let Some(n) = &pad.net {
                    if !n.is_empty() {
                        v.insert(n.clone());
                    }
                }
            }
        }
        v.into_iter().collect()
    };
    let classifier = super::classes::classify_nets(&nets);
    let width = pcb.rules.default_rules.trace_width;
    let (mut polished, mut attempted) = (0usize, 0usize);
    for (pn, nn) in &classifier.pairs {
        let (w, gap) = {
            // Peek the class geometry for the coupling test pitch.
            let s = RouteSession::from_pcb(pcb);
            pair_geometry(&s, pcb, pn, width)
        };
        let frac = crate::router::si_claims::coupled_fraction(pcb, pn, nn, (w + gap) * 1.75);
        if frac >= 0.5 {
            continue;
        }
        let p_pads = pads_of_net(pcb, pn);
        if p_pads.len() < 2 {
            continue;
        }
        attempted += 1;
        // Rip both nets' routed copper off a working copy of the board.
        let mut work = pcb.clone();
        work.traces.retain(|t| &t.net != pn && &t.net != nn);
        work.vias.retain(|v| &v.net != pn && &v.net != nn);
        // MST endpoints: the two farthest pads of the P net.
        let (mut from, mut to, mut best) = (p_pads[0], p_pads[0], -1.0);
        for i in 0..p_pads.len() {
            for j in i + 1..p_pads.len() {
                let d = dist(p_pads[i], p_pads[j]);
                if d > best {
                    best = d;
                    from = p_pads[i];
                    to = p_pads[j];
                }
            }
        }
        // Corridor rip: single-ended (non-pair, non-plane) traces crossing
        // the pair's corridor band move aside — the same negotiation right
        // singles hold against each other. Ripped singles re-route after the
        // pair commits; any that fail restore verbatim only if the pair
        // failed (the pair outranks a flexible single).
        let band = 2.0 * (w + gap) + 1.0;
        let (lo, hi) = (
            Vec2::new(from.x.min(to.x) - band, from.y.min(to.y) - band),
            Vec2::new(from.x.max(to.x) + band, from.y.max(to.y) + band),
        );
        let plane_nets: std::collections::BTreeSet<&str> = pcb
            .zones
            .iter()
            .filter(|z| !z.net.is_empty())
            .map(|z| z.net.as_str())
            .collect();
        let in_band = |a: Vec2, b: Vec2| {
            a.x.max(b.x) >= lo.x
                && a.x.min(b.x) <= hi.x
                && a.y.max(b.y) >= lo.y
                && a.y.min(b.y) <= hi.y
        };
        let mut ripped_nets: std::collections::BTreeSet<String> = Default::default();
        for t in &work.traces {
            if !classifier.is_pair_member(&t.net)
                && !plane_nets.contains(t.net.as_str())
                && in_band(t.start, t.end)
            {
                ripped_nets.insert(t.net.clone());
            }
        }
        let ripped_copper: Vec<Trace> = work
            .traces
            .iter()
            .filter(|t| ripped_nets.contains(&t.net))
            .cloned()
            .collect();
        let ripped_vias: Vec<Via> = work
            .vias
            .iter()
            .filter(|v| ripped_nets.contains(&v.net))
            .cloned()
            .collect();
        work.traces.retain(|t| !ripped_nets.contains(&t.net));
        work.vias.retain(|v| !ripped_nets.contains(&v.net));

        let mut session = RouteSession::from_pcb(&work);
        let mut placed: Vec<Placed> = Vec::new();
        let cong = Congestion::new(&work.outline.vertices);
        let pair_result = try_route_pair(
            &mut session,
            &work,
            width,
            pn,
            from,
            to,
            &mut placed,
            &cong,
            effort_expansions,
        );
        let Some((mine, theirs)) = pair_result else {
            continue; // original board untouched — nothing was committed to pcb
        };
        // Re-route each ripped single against the pair-carrying session; a
        // single that fails brings the whole attempt down (restore original).
        let mut singles_ok = true;
        let mut rerouted: Vec<Placed> = Vec::new();
        'singles: for net in &ripped_nets {
            let pads = pads_of_net(&work, net);
            if pads.len() < 2 {
                continue;
            }
            for i in 1..pads.len() {
                let r = route_net_maze3d(
                    &session,
                    &work.outline.vertices,
                    &copper_layers(&work),
                    net,
                    pads[i - 1],
                    &copper_layers(&work),
                    pads[i],
                    &copper_layers(&work),
                    session.width_for(net, width),
                    work.rules.default_rules.via_diameter,
                    Some(&cong),
                    effort_expansions,
                    1.0,
                    None,
                    &[],
                    &[],
                    true,
                );
                if !r.success {
                    singles_ok = false;
                    break 'singles;
                }
                let cand = Candidate {
                    net: net.clone(),
                    from: pads[i - 1],
                    to: pads[i],
                    width: session.width_for(net, width),
                    segments: r.segments,
                    vias: r.vias,
                    thin_segments: vec![],
                    thin_width: width,
                };
                match validate_and_commit(&mut session, &work, cand, &placed) {
                    Some(pl) => rerouted.push(pl),
                    None => {
                        singles_ok = false;
                        break 'singles;
                    }
                }
            }
        }
        if !singles_ok {
            log::debug!("pair-polish: {pn}: displaced single failed to re-route — reverting");
            continue;
        }
        // Success: write pair + rerouted singles back onto the working
        // board (the ripped originals were removed above; reroutes replace
        // them).
        drop(ripped_copper);
        drop(ripped_vias);
        for pl in rerouted.iter() {
            for &(a, b, l) in &pl.segments {
                work.traces.push(Trace {
                    start: a,
                    end: b,
                    width: pl.width,
                    layer: l,
                    net: pl.net.clone(),
                    source: None,
                });
            }
            for &(pt, la, lb) in &pl.via_pts {
                work.vias.push(Via {
                    position: pt,
                    diameter: work.rules.default_rules.via_diameter,
                    drill: work.rules.default_rules.via_drill,
                    start_layer: la,
                    end_layer: lb,
                    net: pl.net.clone(),
                    source: None,
                });
            }
        }
        {
            for pl in [&mine, &theirs] {
                for &(a, b, l) in &pl.segments {
                    work.traces.push(Trace {
                        start: a,
                        end: b,
                        width: pl.width,
                        layer: l,
                        net: pl.net.clone(),
                        source: None,
                    });
                }
                for &(a, b, l) in &pl.stubs {
                    work.traces.push(Trace {
                        start: a,
                        end: b,
                        width: pl.stub_width,
                        layer: l,
                        net: pl.net.clone(),
                        source: None,
                    });
                }
                for &(pt, la, lb) in &pl.via_pts {
                    work.vias.push(Via {
                        position: pt,
                        diameter: work.rules.default_rules.via_diameter,
                        drill: work.rules.default_rules.via_drill,
                        start_layer: la,
                        end_layer: lb,
                        net: pl.net.clone(),
                        source: None,
                    });
                }
            }
            *pcb = work;
            polished += 1;
            log::info!("pair-polish: {pn} + {nn} now coupled");
        }
    }
    (polished, attempted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::RouteSession;
    use crate::spatial::CopperGeom;
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
                    via_diameter: 0.35,
                    via_drill: 0.2,
                    diff_pair_gap: None,
                    diff_pair_width: None,
                },
                class_rules: vec![NetClassRules {
                    name: "DIFF".into(),
                    trace_width: 0.2,
                    clearance: 0.15,
                    via_diameter: 0.35,
                    via_drill: 0.2,
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

    /// Deterministic repro for the layer-transition geometry (censuses
    /// 11-17): a copper wall on FCu forces the pair through vias to BCu and
    /// back. The pair must still route, and every committed P element must
    /// clear every N element — the twin-blocker class of failures rendered
    /// as a unit test.
    #[test]
    fn pair_layer_transition_keeps_twin_clearance() {
        let _ = env_logger::builder().is_test(true).try_init();
        let mut pcb = pair_board();
        // Wall of foreign copper across FCu at x=25, leaving no FCu route.
        pcb.traces.push(Trace {
            start: Vec2::new(25.0, 0.0),
            end: Vec2::new(25.0, 30.0),
            width: 0.4,
            layer: PcbLayer::FCu,
            net: "WALL".into(),
            source: None,
        });
        let mut session = RouteSession::from_pcb(&pcb);
        let mut placed: Vec<Placed> = Vec::new();
        let cong = Congestion::new(&pcb.outline.vertices);
        let r = try_route_pair(
            &mut session,
            &pcb,
            0.25,
            "/ETH.2_P",
            Vec2::new(5.0, 15.325),
            Vec2::new(45.0, 15.325),
            &mut placed,
            &cong,
            400_000,
        );
        let (mine, theirs) = r.expect("pair must route across the FCu wall via BCu");
        assert!(
            !mine.via_pts.is_empty() && !theirs.via_pts.is_empty(),
            "route must actually change layers"
        );
        // Twin clearance: every P segment vs every N segment on shared layers.
        let clearance = 0.15;
        let seg_seg = |a1: Vec2, b1: Vec2, a2: Vec2, b2: Vec2| -> f64 {
            let pt_seg = |p: Vec2, a: Vec2, b: Vec2| -> f64 {
                let ab = b - a;
                let l2 = ab.x * ab.x + ab.y * ab.y;
                if l2 < 1e-18 {
                    return dist(p, a);
                }
                let t = (((p.x - a.x) * ab.x + (p.y - a.y) * ab.y) / l2).clamp(0.0, 1.0);
                dist(p, a + ab.scale(t))
            };
            pt_seg(a1, a2, b2)
                .min(pt_seg(b1, a2, b2))
                .min(pt_seg(a2, a1, b1))
                .min(pt_seg(b2, a1, b1))
        };
        let all = |p: &Placed| -> Vec<(Vec2, Vec2, PcbLayer, f64)> {
            let mut v: Vec<(Vec2, Vec2, PcbLayer, f64)> = p
                .segments
                .iter()
                .map(|&(a, b, l)| (a, b, l, p.width))
                .collect();
            v.extend(p.stubs.iter().map(|&(a, b, l)| (a, b, l, p.stub_width)));
            v
        };
        let mut worst = f64::INFINITY;
        for &(a1, b1, l1, w1) in &all(&mine) {
            for &(a2, b2, l2, w2) in &all(&theirs) {
                if l1 != l2 {
                    continue;
                }
                let edge = seg_seg(a1, b1, a2, b2) - w1 / 2.0 - w2 / 2.0;
                worst = worst.min(edge);
            }
        }
        assert!(
            worst >= clearance - 1e-9,
            "twin edge clearance {worst:.3}mm < {clearance}"
        );
    }

    /// Twin-clearance check shared by the transition repros.
    fn assert_twin_clear(mine: &Placed, theirs: &Placed, clearance: f64) {
        let seg_seg = |a1: Vec2, b1: Vec2, a2: Vec2, b2: Vec2| -> f64 {
            let pt_seg = |p: Vec2, a: Vec2, b: Vec2| -> f64 {
                let ab = b - a;
                let l2 = ab.x * ab.x + ab.y * ab.y;
                if l2 < 1e-18 {
                    return dist(p, a);
                }
                let t = (((p.x - a.x) * ab.x + (p.y - a.y) * ab.y) / l2).clamp(0.0, 1.0);
                dist(p, a + ab.scale(t))
            };
            pt_seg(a1, a2, b2)
                .min(pt_seg(b1, a2, b2))
                .min(pt_seg(a2, a1, b1))
                .min(pt_seg(b2, a1, b1))
        };
        let all = |p: &Placed| -> Vec<(Vec2, Vec2, PcbLayer, f64)> {
            let mut v: Vec<(Vec2, Vec2, PcbLayer, f64)> = p
                .segments
                .iter()
                .map(|&(a, b, l)| (a, b, l, p.width))
                .collect();
            v.extend(p.stubs.iter().map(|&(a, b, l)| (a, b, l, p.stub_width)));
            v
        };
        let mut worst = f64::INFINITY;
        for &(a1, b1, l1, w1) in &all(mine) {
            for &(a2, b2, l2, w2) in &all(theirs) {
                if l1 == l2 {
                    worst = worst.min(seg_seg(a1, b1, a2, b2) - w1 / 2.0 - w2 / 2.0);
                }
            }
        }
        assert!(
            worst >= clearance - 1e-9,
            "twin edge clearance {worst:.3}mm < {clearance}"
        );
    }

    /// Corner transition: walls force the pair through a via field at an
    /// L-turn — the corner-normal rotation case the straight-wall repro
    /// cannot exercise.
    #[test]
    fn pair_corner_transition_keeps_twin_clearance() {
        let _ = env_logger::builder().is_test(true).try_init();
        let mut pcb = pair_board();
        // Move the destination pads to force an L (right then up).
        pcb.footprints[1].position = Vec2::new(45.0, 15.0);
        for pad in &mut pcb.footprints[1].pads {
            pad.position = Vec2::new(pad.position.y, pad.position.x + 10.0);
        }
        // Wall on FCu with a vertical jog channel only reachable on BCu.
        pcb.traces.push(Trace {
            start: Vec2::new(30.0, 0.0),
            end: Vec2::new(30.0, 30.0),
            width: 0.4,
            layer: PcbLayer::FCu,
            net: "WALL".into(),
            source: None,
        });
        let mut session = RouteSession::from_pcb(&pcb);
        let mut placed: Vec<Placed> = Vec::new();
        let cong = Congestion::new(&pcb.outline.vertices);
        let r = try_route_pair(
            &mut session,
            &pcb,
            0.25,
            "/ETH.2_P",
            Vec2::new(5.0, 15.325),
            Vec2::new(45.325, 25.0),
            &mut placed,
            &cong,
            400_000,
        );
        let (mine, theirs) = r.expect("pair must route the L across the wall");
        assert_twin_clear(&mine, &theirs, 0.15);
    }
}
