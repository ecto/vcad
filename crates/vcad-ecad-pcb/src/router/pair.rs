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
/// - the USB `DP` ↔ `DM` (and `D+` ↔ `D-`) convention, which is not a
///   symmetric suffix: `/USB3-0.DP` ↔ `/USB3-0.DM`
///
/// This is the single source of truth for pair membership — [`super::classes`]
/// builds the diff-pair class from it, and this stage routes what that class
/// declares. When the two disagreed, the classifier put USB pairs in the class
/// and this function then reported `NoPartnerName`, so every USB pair fell
/// straight through to independent single routing (CM5 census: 3 pairs).
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
    // USB `DP`/`DM` (and `D+`/`D-`). Require a separator before the token so
    // e.g. "LDP" or a net literally named "DP" does not match.
    for (a, b) in [("DP", "DM"), ("DM", "DP"), ("D+", "D-"), ("D-", "D+")] {
        if let Some(base) = net.strip_suffix(a) {
            if base.ends_with('.') || base.ends_with('_') || base.ends_with('-') {
                return Some(format!("{base}{b}"));
            }
        }
    }
    None
}

/// Tolerance on the class's target impedance, in percent, within which a layer
/// counts as impedance-correct for the declared geometry. ±10% is the usual
/// fab spec for a controlled-impedance layer.
const IMPEDANCE_TOL_PCT: f64 = 10.0;

/// The diff-pair net class `net` belongs to, if any.
fn diff_class_of<'a>(pcb: &'a Pcb, net: &str) -> Option<&'a vcad_ir::ecad::NetClassRules> {
    pcb.rules.class_rules.iter().find(|class| {
        class.diff_pair_gap.is_some()
            && pcb
                .rules
                .net_class_assignments
                .get(&class.name)
                .is_some_and(|nets| nets.iter().any(|n| n == net))
    })
}

/// Copper layers on which `net`'s class geometry is impedance-correct.
///
/// `None` — the common case — means "no preference": the class declares no
/// target impedance, the stackup cannot be solved, no layer qualifies, or
/// every available layer does. The caller then searches the full stack exactly
/// as it always has.
fn preferred_layers(pcb: &Pcb, net: &str, copper: &[PcbLayer]) -> Option<Vec<PcbLayer>> {
    let class = diff_class_of(pcb, net)?;
    let ok = crate::impedance::impedance_correct_layers(pcb, class, IMPEDANCE_TOL_PCT)?;
    let pref: Vec<PcbLayer> = copper.iter().copied().filter(|l| ok.contains(l)).collect();
    // Only a proper, non-empty subset is worth a separate pass.
    if pref.is_empty() || pref.len() == copper.len() {
        return None;
    }
    Some(pref)
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

/// The copper layers the `net` pad at `pos` actually occupies.
///
/// A pad connector may only *leave* the pad on a layer the pad is on: a
/// connector that starts at the pad's XY on some other layer never touches
/// it electrically, and no clearance check catches that (the copper is
/// same-net, so the probe is happy). This mattered little while coupled legs
/// always began on the outer layer; once they may begin anywhere in the
/// stack, an unconstrained connector start silently disconnects the net.
fn pad_layers_at(pcb: &Pcb, net: &str, pos: Vec2) -> Vec<PcbLayer> {
    let mut best: Option<(f64, Vec<PcbLayer>)> = None;
    for fp in &pcb.footprints {
        for pad in &fp.pads {
            if pad.net.as_deref() != Some(net) {
                continue;
            }
            let d = dist(crate::geometry::pad_world_position(fp, pad), pos);
            if best.as_ref().map(|(bd, _)| d < *bd).unwrap_or(true) {
                best = Some((d, pad.layers.clone()));
            }
        }
    }
    best.map(|(_, l)| l).unwrap_or_default()
}

/// The pad of `pads` nearest to `p`.
fn nearest_pad(pads: &[Vec2], p: Vec2) -> Option<Vec2> {
    pads.iter()
        .copied()
        .min_by(|a, b| dist(*a, p).partial_cmp(&dist(*b, p)).unwrap())
}

/// Why a coupled-pair attempt gave up.
///
/// The bail census — histogram these across a board's pairs, kill the
/// dominant mode, re-census — is the method that has driven this stage's
/// coupled fraction up (censuses 9-17 are quoted throughout this file). The
/// reasons are typed rather than log-only so the census is a measurement,
/// not a grep.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PairBail {
    /// The net name carries no polarity marker — not a pair at all.
    NoPartnerName,
    /// The partner name resolves but no pad on the board carries it.
    PartnerNoPads,
    /// Both partner endpoints snap to the same pad.
    PartnerEndpointsCollapse,
    /// The span is too short to fit leads plus a coupled run.
    SpanTooShort,
    /// The phantom fat-trace centerline search failed on every retreat rung.
    CenterlineSearch,
    /// The centerline could not be offset into two legs (degenerate,
    /// interior cusp, or the in-line via gate).
    DegenerateCenterline,
    /// A realized leg failed the exact oracle.
    LegValidation,
    /// A pad connector (breakout stub) could not reach its leg.
    Connector,
}

impl PairBail {
    /// Short stable slug for census tables.
    pub fn slug(self) -> &'static str {
        match self {
            PairBail::NoPartnerName => "no-partner-name",
            PairBail::PartnerNoPads => "partner-no-pads",
            PairBail::PartnerEndpointsCollapse => "partner-endpoints-collapse",
            PairBail::SpanTooShort => "span-too-short",
            PairBail::CenterlineSearch => "centerline-search",
            PairBail::DegenerateCenterline => "degenerate-centerline",
            PairBail::LegValidation => "leg-validation",
            PairBail::Connector => "connector",
        }
    }
}

/// A polyline of routed copper: each segment with the layer it sits on.
/// Used for the phantom centerline before it is offset into two legs.
type Centerline = Vec<(Vec2, Vec2, PcbLayer)>;

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
    try_route_pair_reason(
        session,
        pcb,
        width,
        net,
        from,
        to,
        placed,
        cong,
        max_expansions,
    )
    .ok()
}

/// [`try_route_pair`] reporting *why* it gave up — the census entry point.
#[allow(clippy::too_many_arguments)]
pub(super) fn try_route_pair_reason(
    session: &mut RouteSession,
    pcb: &Pcb,
    width: f64,
    net: &str,
    from: Vec2,
    to: Vec2,
    placed: &mut Vec<Placed>,
    cong: &Congestion,
    max_expansions: usize,
) -> Result<(Placed, Placed), PairBail> {
    let partner = pair_partner(net).ok_or(PairBail::NoPartnerName)?;
    let partner_pads = pads_of_net(pcb, &partner);
    if partner_pads.is_empty() {
        log::debug!("pair: {net} has partner name {partner} but no pads on board");
        return Err(PairBail::PartnerNoPads);
    }
    // MST-similar partner endpoints: the partner pads nearest to this
    // connection's own endpoints. They must be two distinct pads.
    let p_from = nearest_pad(&partner_pads, from).ok_or(PairBail::PartnerNoPads)?;
    let p_to = nearest_pad(&partner_pads, to).ok_or(PairBail::PartnerNoPads)?;
    if dist(p_from, p_to) < 1e-6 {
        log::debug!("pair: {net}/{partner}: partner endpoints collapse to one pad — bailing");
        return Err(PairBail::PartnerEndpointsCollapse);
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

    // Centerline endpoints: the midpoints, pushed toward each other by a lead
    // so the fat phantom clears the four pads (the pads are other-net copper
    // from the phantom's perspective at the twin's end of the corridor).
    let mid_from = Vec2::new((from.x + p_from.x) / 2.0, (from.y + p_from.y) / 2.0);
    let mid_to = Vec2::new((to.x + p_to.x) / 2.0, (to.y + p_to.y) / 2.0);
    let span = dist(mid_from, mid_to);
    let lead = (fat_w + clearance + w).min(span * 0.25);
    // Below SHORT_PAIR_SPAN_MM the lead inset is the whole problem: a ~1.8mm
    // pad-to-pad pair keeps only ~0.9mm of searchable centerline, both ladders
    // fail, and the pair falls back to two singles whose legs land on
    // DIFFERENT layers — scoring coupled_fraction exactly 0.0 and pinning the
    // receipt's pair claim at zero however well the rest of the board routes
    // (M7: the `/HS.*` group). Such a pair does not need a search at all; it
    // needs the straight centerline it was always going to have. Route it
    // directly below, so only spans too long for that path still bail here.
    if span < 4.0 * lead.max(0.1) && span > SHORT_PAIR_SPAN_MM {
        log::debug!("pair: {net}/{partner}: span {span:.2}mm too short for coupled routing");
        return Err(PairBail::SpanTooShort);
    }
    if span < 2.0 * half_sep {
        // Degenerate: the twin legs would be further apart than the span.
        log::debug!("pair: {net}/{partner}: span {span:.2}mm below one pair separation");
        return Err(PairBail::SpanTooShort);
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
    // Short pairs first: a straight, lead-free centerline on a layer all four
    // pads share. Exact and probe-validated, so it costs four point probes
    // rather than a maze search, and it keeps both legs on ONE layer — which
    // is what `coupled_fraction` measures.
    let direct = if span <= SHORT_PAIR_SPAN_MM {
        direct_centerline(
            session,
            pcb,
            net,
            &partner,
            (mid_from, mid_to),
            [from, to, p_from, p_to],
            half_sep,
            via_d / 2.0,
            clearance,
            w,
            &copper,
        )
    } else {
        None
    };
    if direct.is_some() {
        log::debug!("pair: {net}/{partner}: short span {span:.2}mm — direct coupled centerline");
    }
    // Impedance-preferred layers (see [`crate::impedance`]). The pair commits
    // to ONE leg width for its whole run, so on the layers where that width is
    // not the impedance-correct one the trace is the wrong width. Until a leg
    // can change width at its transition vias, the honest lever is preference:
    // search first restricted to the layers where the class's declared
    // geometry does hit its target impedance, and fall back to the full stack
    // when no path exists there. Routability is unchanged — the fallback pass
    // is the search that used to run — and a board that declares no target
    // impedance (or whose stackup cannot be solved) takes the single-pass path
    // exactly as before.
    let search_sets: Vec<Vec<PcbLayer>> = match preferred_layers(pcb, net, &copper) {
        Some(pref) => {
            log::debug!(
                "pair: {net}: impedance-preferred layers {pref:?} of {} copper layers",
                copper.len()
            );
            vec![pref, copper.clone()]
        }
        None => vec![copper.clone()],
    };
    // The direct short-span centerline, when it exists, is already exact and
    // single-layer — no ladder, and no layer preference to apply.
    let mut center: Option<Centerline> = direct;
    let usable_span = span - 2.0 * lead;
    // MEASURED neck-down (census 18). The fixed table below guesses how far
    // to retreat; on the CM5 that guess is the single dominant bail — 23 of
    // 27 remaining failures — because a pair born at the centre of a large
    // BGA needs more retreat than the table's largest rung that still fits
    // the span. Probe instead: walk each end inward until a fat capsule is
    // actually legal somewhere in the stack, and start the ladder there. This
    // costs point probes, not searches, and it is exact — no capsule fits
    // before that point, so every rung the table spends below it is wasted.
    let fat_legal_at = |p: Vec2| -> bool {
        copper.iter().any(|&l| {
            session
                .probe(
                    &CopperGeom::Segment {
                        a: p,
                        b: p,
                        half_w: fat_w / 2.0,
                    },
                    l,
                    net,
                    clearance,
                )
                .legal
        })
    };
    let measure_inset = |from_start: bool| -> f64 {
        let limit = (usable_span * 0.45).max(0.0);
        let mut r = 0.0;
        while r <= limit {
            let p = if from_start {
                start + dir.scale(r)
            } else {
                end - dir.scale(r)
            };
            if fat_legal_at(p) {
                return r;
            }
            r += 0.5;
        }
        0.0
    };
    let (m_from, m_to) = (measure_inset(true), measure_inset(false));
    if m_from > 0.0 || m_to > 0.0 {
        log::debug!("pair: {net}/{partner}: measured neck-down {m_from:.1}/{m_to:.1}mm");
    }
    // Measured rungs first (they are the only ones that can succeed at all
    // near a dense field), then escalating slack around them, then the
    // original asymmetric table as a backstop for the cases the point probe
    // clears but the corridor does not.
    let mut rungs: Vec<(f64, f64)> = Vec::new();
    for extra in [0.0, 1.0, 2.0, 4.0, 8.0] {
        rungs.push((m_from + extra, m_to + extra));
    }
    rungs.extend([
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
        (8.0, 4.0),
        (4.0, 8.0),
        (8.0, 8.0),
        (12.0, 12.0),
        (16.0, 16.0),
    ]);
    for search in &search_sets {
        if center.is_some() {
            break;
        }
        let mut found = None;
        for (r_from, r_to) in rungs.iter().copied() {
            let usable = span - 2.0 * lead;
            if r_from + r_to >= usable - 1.0 {
                continue;
            }
            let s_pt = start + dir.scale(r_from);
            let e_pt = end - dir.scale(r_to);
            // Endpoint layers: the WHOLE stack, not just the outer layer. A pair
            // escaping a BGA goes down a via and couples on an inner layer under
            // the field, where there are no pads at all — that is how the human
            // board does it. Pinning the phantom's ends to FCu instead forced the
            // coupled run to begin inside the pin field, where no fat capsule can
            // ever sit; the pad connectors already route across the full stack to
            // reach `leg.first_layer`, so this costs nothing and matches them.
            let r = route_net_maze3d(
                session,
                &pcb.outline.vertices,
                search,
                net,
                s_pt,
                search,
                e_pt,
                search,
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
        // Ladder 2 — narrow centerline with a MEASURED coupled window.
        //
        // The bail census says the fat search above is the dominant failure mode
        // by an order of magnitude (CM5: 28 of 32 bails, and 48 of 56 in a full
        // route). The reason is structural, not budgetary: the phantom is
        // `2w + gap` = 0.66mm wide, and the channels between 0.4mm-pitch BGA pads
        // are single-track. No retreat rung helps when BOTH ends sit in pin
        // fields, because the ladder is *guessing* how far to neck down.
        //
        // So: search a NARROWER centerline — one that threads what a single
        // thread — then discover the coupled extent instead of guessing it. Offset
        // the centerline into two legs, probe both legs per centerline segment,
        // and keep the longest contiguous window where both are legal. That window
        // is the real coupled run; the pad connectors cover the necked ends, and
        // the amount of neck-down is measured rather than drawn from a 15-rung
        // table. Widths descend so a wider (better-coupled) corridor always wins
        // when one exists; the best trimmed run across the ladder is kept.
        if let Some(r) = found {
            center = Some(r.segments);
            break;
        }
        {
            // Minimum worthwhile coupling: below this the pair is better off as
            // two singles, so we fall through rather than commit a token stub.
            let min_coupled = (0.5 * span).max(1.0);
            let mut best: Option<(f64, Centerline)> = None;
            for cw in [
                0.75 * fat_w,
                0.5 * fat_w,
                w,
                pcb.rules.default_rules.trace_width,
            ] {
                if cw < pcb.rules.default_rules.trace_width - 1e-9 {
                    continue;
                }
                let r = route_net_maze3d(
                    session,
                    &pcb.outline.vertices,
                    search,
                    net,
                    start,
                    search,
                    end,
                    search,
                    cw,
                    via_d,
                    Some(cong),
                    budget,
                    1.0,
                    None,
                    &[],
                    &[],
                    true,
                );
                if !r.success || r.segments.is_empty() {
                    if cw <= pcb.rules.default_rules.trace_width + 1e-9 {
                        // Endpoint diagnosis: a search that fails even at single
                        // width on every layer is usually an illegal endpoint, not
                        // a blocked corridor.
                        let probe_pt = |p: Vec2| -> usize {
                            search
                                .iter()
                                .filter(|&&l| {
                                    session
                                        .probe(
                                            &CopperGeom::Segment {
                                                a: p,
                                                b: p,
                                                half_w: cw / 2.0,
                                            },
                                            l,
                                            net,
                                            clearance,
                                        )
                                        .legal
                                })
                                .count()
                        };
                        log::debug!(
                        "pair: {net}/{partner}: narrow search failed at w={cw}; start legal on {}/{} layers, end on {}/{}",
                        probe_pt(start),
                        search.len(),
                        probe_pt(end),
                        search.len()
                    );
                    }
                    continue;
                }
                let Some((i, j)) = longest_coupled_window(session, &r.segments, net, half_sep, w)
                else {
                    continue;
                };
                let win = &r.segments[i..=j];
                let len: f64 = win.iter().map(|&(a, b, _)| dist(a, b)).sum();
                if len < min_coupled {
                    continue;
                }
                if best.as_ref().map(|(bl, _)| len > *bl).unwrap_or(true) {
                    best = Some((len, win.to_vec()));
                }
            }
            if let Some((len, win)) = best {
                log::debug!(
                "pair: {net}/{partner}: narrow centerline coupled {len:.1}mm of {span:.1}mm span"
            );
                center = Some(win);
            }
        }
        if center.is_some() {
            break;
        }
    }
    let Some(center) = center else {
        log::debug!("pair: {net}/{partner}: phantom centerline search failed");
        restore(session, placed, ripped);
        return Err(PairBail::CenterlineSearch);
    };

    // Leg → net assignment: the one whose pad connectors are shortest (the
    // non-crossing assignment — crossing connectors are strictly longer).
    //
    // Scoring these two choices on PREDICTED intra-pair skew instead was
    // tried and measured worse (subset board: 1.499mm -> 2.100mm worst skew).
    // The prediction has to model the connectors as straight stubs, but they
    // are maze routes that detour around whatever is in the way, so the
    // ranking it produces is mostly noise.
    let cost = |leg: &Leg, s: Vec2, e: Vec2| dist(s, leg.first) + dist(e, leg.last);

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
        // Straight-hop shortcut, but ONLY when the leg's layer is a layer the
        // pad is on — otherwise this emits a short segment that starts at the
        // pad's XY on a layer the pad does not occupy and connects nothing.
        let pad_layers = pad_layers_at(pcb, net, from);
        let hop_ok = pad_layers.is_empty() || pad_layers.contains(&to_layer);
        if hop_ok && dist(from, to) <= nw * 2.0 {
            // PROBE the hop before taking it. This shortcut used to emit
            // copper unchecked, which is the one path in this stage that
            // bypasses the oracle — and it shows up as intra-pair clearance
            // violations, because both twins take their own unchecked hop at
            // opposite ends of the same gap (CM5: /USB3-1.DP to /USB3-1.DM at
            // 0.058mm against a 0.080mm base clearance). Fall through to the
            // maze when the straight line is not legal.
            let hop = CopperGeom::Segment {
                a: from,
                b: to,
                half_w: nw / 2.0,
            };
            if session
                .probe(&hop, to_layer, net, session.clearance_for(net))
                .legal
            {
                return Some((vec![(from, to, to_layer)], vec![]));
            }
        }
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
        // Leave the pad only on a layer the pad is actually on; a via in the
        // connector carries it to the leg's layer.
        let from_layers: Vec<PcbLayer> = {
            let on_board: Vec<PcbLayer> = pad_layers
                .iter()
                .copied()
                .filter(|l| copper.contains(l))
                .collect();
            if on_board.is_empty() {
                copper.clone()
            } else {
                on_board
            }
        };
        // Escalating window. The hop itself is sub-millimetre, but its LENGTH
        // is the wrong scale to size the search by: a pad inside a fine-pitch
        // field can only reach another layer by escaping the field laterally
        // first and dropping its via outside — the channels between 1.37mm
        // pads on a 1.7mm pitch are 0.33mm, wide enough for the 0.08mm neck
        // but not for a 0.21mm via plus clearance. A window drawn 4mm around
        // a 0.8mm hop cannot contain that detour, so the connector failed
        // while a perfectly good coupled corridor sat 0.8mm away (CM5: the
        // last five pairs, all of them `/ETH.*` and `/PCIe.*` at U4).
        for margin in [4.0 + w, 10.0 + w, 20.0 + w] {
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
                &from_layers,
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
                if margin > 4.0 + w + 1e-9 {
                    log::debug!(
                        "pair-connector: {net} reached at {margin:.0}mm window ({:.2}mm hop)",
                        dist(from, to)
                    );
                }
                return Some((r.segments, r.vias));
            }
        }
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
    // Connectors, all four against both committed legs. Every one of them goes
    // through `validate_and_commit` — the same exact oracle the legs use — and
    // its output is merged into the leg's `Placed` (copper on the stubs
    // channel, vias on `via_pts`).
    //
    // This used to hand-roll `session.commit` for the connector's segments and
    // push its VIAS straight onto `pl.via_pts` without committing them at all.
    // Those vias were real: they were written onto the output board like any
    // other. But the session never learned about them, so no later route could
    // be refused for crossing one — the oracle cannot decline copper it has
    // never been shown. On a full CM5 route that was 61 of the 72 exact
    // (0.000mm) cross-net overlaps: foreign traces driven clean through a pair
    // connector's via barrel, every offending via on a `_P`/`_N` net.
    let attach = |session: &mut RouteSession,
                  placed: &[Placed],
                  pl: &mut Placed,
                  leg: &Leg,
                  pad_a: Vec2,
                  pad_b: Vec2|
     -> bool {
        let net = pl.net.clone();
        for (pad, end, end_layer, which) in [
            (pad_a, leg.first, leg.first_layer, "head"),
            (pad_b, leg.last, leg.last_layer, "tail"),
        ] {
            let Some((copper, vias)) = connect(session, &net, pad, end, end_layer, leg) else {
                log::debug!("pair-connector: {net} {which} failed");
                return false;
            };
            // Same-net context for the commit's via-reuse test: the leg's own
            // vias live on `pl`, which is not in `placed` yet.
            let mut ctx: Vec<Placed> = placed.iter().filter(|p| p.net == net).cloned().collect();
            ctx.push(pl.clone());
            let cand = Candidate {
                net: net.clone(),
                from: pad,
                to: end,
                width: nw,
                segments: vec![],
                vias,
                thin_segments: copper,
                thin_width: nw,
            };
            let Some(p) = validate_and_commit(session, pcb, cand, &ctx) else {
                log::debug!("pair-connector: {net} {which} failed validation");
                return false;
            };
            pl.spans.extend(p.spans);
            pl.stubs.extend(p.stubs);
            pl.via_pts.extend(p.via_pts);
        }
        true
    };
    // Connector retreat. A coupled corridor can be perfectly good and still be
    // unreachable: the leg end lands inside a through-hole field on a deep
    // inner layer, and the sub-millimetre hop from the pad to it has no legal
    // path (CM5 census: every remaining bail was a *head* connector of 0.6-1.2mm
    // into In4Cu/BCu/In8Cu under the 100-pin connector). Giving up there throws
    // away the whole coupled run over its last half-millimetre.
    //
    // So trim the centerline back from the end that failed and try again: a
    // shorter coupled run whose ends sit in open copper, with slightly longer
    // neck-down connectors covering the difference. Rungs are asymmetric
    // because the two ends fail independently, and each is small next to the
    // spans this rescues (0.6-3mm off a 21-33mm pair).
    const RETREAT_RUNGS: [(f64, f64); 7] = [
        (0.0, 0.0),
        (0.6, 0.0),
        (0.0, 0.6),
        (1.5, 0.0),
        (0.0, 1.5),
        (1.5, 1.5),
        (3.0, 3.0),
    ];
    let mut outcome: Option<(Placed, Placed, f64)> = None;
    let mut last_bail = PairBail::Connector;
    for (r_from, r_to) in RETREAT_RUNGS {
        let Some(center_t) = trim_centerline(&center, r_from, r_to) else {
            continue;
        };
        let Some((leg_a, leg_b)) =
            realize_legs(&center_t, half_sep, via_off, via_d / 2.0, clearance, w)
        else {
            last_bail = PairBail::DegenerateCenterline;
            continue;
        };
        let a_mine = cost(&leg_a, from, to) + cost(&leg_b, p_from, p_to)
            <= cost(&leg_b, from, to) + cost(&leg_a, p_from, p_to);
        let (mine, theirs) = if a_mine {
            (leg_a, leg_b)
        } else {
            (leg_b, leg_a)
        };
        // Ordering (census 9's lesson): commit BOTH legs first — they are
        // parallel and disjoint by construction — then route the four pad
        // connectors against the complete picture, so no connector can cut the
        // coupled corridor before the other leg exists.
        let Some(mut placed_mine) =
            validate_and_commit(session, pcb, leg_cand(&mine, net, from, to), placed)
        else {
            log::debug!("pair: {net}/{partner}: leg 1 failed validation");
            last_bail = PairBail::LegValidation;
            continue;
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
            last_bail = PairBail::LegValidation;
            continue;
        };
        if attach(session, placed, &mut placed_mine, &mine, from, to)
            && attach(session, placed, &mut placed_theirs, &theirs, p_from, p_to)
        {
            if r_from + r_to > 0.0 {
                log::debug!(
                    "pair: {net}/{partner}: connectors reached after {r_from}/{r_to}mm retreat"
                );
            }
            let len: f64 = center_t.iter().map(|(a, b, _)| dist(*a, *b)).sum();
            outcome = Some((placed_mine, placed_theirs, len));
            break;
        }
        // Connectors failed: drop this attempt's copper (keeping the ripped
        // partner routes for the next rung) and retreat further.
        for &sp in placed_mine.spans.iter().chain(placed_theirs.spans.iter()) {
            session.remove(sp);
        }
        last_bail = PairBail::Connector;
    }
    let Some((placed_mine, placed_theirs, center_len)) = outcome else {
        restore(session, placed, ripped);
        return Err(last_bail);
    };

    log::info!(
        "pair: routed {net} + {partner} coupled (centerline {:.1}mm, {} via pair(s), w={w} gap={gap})",
        center_len,
        placed_mine.via_pts.len(),
    );
    Ok((placed_mine, placed_theirs))
}

/// Shorten a centerline by `r_from` millimetres at its start and `r_to` at its
/// end, measured along the polyline. Returns `None` if the trim would leave
/// less than a via pitch of coupled run — at that point the pair is better off
/// as two singles than as a token stub.
fn trim_centerline(center: &Centerline, r_from: f64, r_to: f64) -> Option<Centerline> {
    if r_from <= 0.0 && r_to <= 0.0 {
        return Some(center.clone());
    }
    let total: f64 = center.iter().map(|(a, b, _)| dist(*a, *b)).sum();
    const MIN_COUPLED_MM: f64 = 1.0;
    if total - r_from - r_to < MIN_COUPLED_MM {
        return None;
    }
    let mut out: Centerline = Vec::new();
    let mut walked = 0.0;
    for &(a, b, l) in center {
        let seg = dist(a, b);
        if seg < 1e-9 {
            continue;
        }
        let (s0, s1) = (walked, walked + seg);
        walked = s1;
        // Clip this segment to the surviving [r_from, total - r_to] interval.
        let lo = r_from.max(s0);
        let hi = (total - r_to).min(s1);
        if hi - lo < 1e-9 {
            continue;
        }
        let dir = (b - a).scale(1.0 / seg);
        out.push((a + dir.scale(lo - s0), a + dir.scale(hi - s0), l));
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// The longest contiguous run of `center` (as an inclusive segment index
/// range) over which BOTH offset legs probe legal at the pair's leg width.
///
/// This is what makes a narrow centerline usable: the search finds a path a
/// single-width trace fits, and this measures how much of that path actually
/// has room for the full coupled pair. Segments whose offsets collide with
/// pads or foreign copper — the pin-field breakout at each end — fall outside
/// the window and become neck-down connectors.
fn longest_coupled_window(
    session: &RouteSession,
    center: &[(Vec2, Vec2, PcbLayer)],
    net: &str,
    half_sep: f64,
    w: f64,
) -> Option<(usize, usize)> {
    let clearance = session.clearance_for(net);
    let legal = |k: usize| -> bool {
        let (a, b, layer) = center[k];
        let d = b - a;
        let len = dist(a, b);
        if len < 1e-9 {
            return false;
        }
        // Perpendicular offset of this segment, both sides.
        let nrm = Vec2::new(-d.y / len, d.x / len);
        [half_sep, -half_sep].iter().all(|&off| {
            let o = nrm.scale(off);
            let geom = CopperGeom::Segment {
                a: a + o,
                b: b + o,
                half_w: w / 2.0,
            };
            session.probe(&geom, layer, net, clearance).legal
        })
    };
    let (mut best, mut run_start): (Option<(usize, usize)>, Option<usize>) = (None, None);
    for k in 0..center.len() {
        if legal(k) {
            run_start.get_or_insert(k);
        } else if let Some(s) = run_start.take() {
            let cand = (s, k - 1);
            if best
                .map(|(bs, be)| cand.1 - cand.0 > be - bs)
                .unwrap_or(true)
            {
                best = Some(cand);
            }
        }
    }
    if let Some(s) = run_start {
        let cand = (s, center.len() - 1);
        if best
            .map(|(bs, be)| cand.1 - cand.0 > be - bs)
            .unwrap_or(true)
        {
            best = Some(cand);
        }
    }
    // A single segment cannot carry a coupled run through realize_legs
    // (which needs at least two points per layer run after simplification).
    best.filter(|(s, e)| e > s)
}

/// Offset the centerline into two legs at ±`half_sep`, with per-leg vias offset
/// ±`via_off` perpendicular at each layer transition (two drills per centerline
/// via, one per leg) and connector jogs where the via offset exceeds the leg
/// offset. Leg A is the +offset side, leg B the −offset side.
/// Spans at or below this route as short pairs: one straight centerline, no
/// lead inset, no search. Above it the phantom corridor is the right tool.
const SHORT_PAIR_SPAN_MM: f64 = 4.0;

/// A straight, lead-free centerline for a short pair, on the best layer that
/// admits both offset legs.
///
/// Layer order prefers the layers all four pads share, so the pair needs no
/// vias at all (which is also what keeps `vias_per_si_net` low); the rest of
/// the stack follows as a fallback for pads on different layers.
///
/// The returned centerline is a *proposal*: both legs are probed here only to
/// choose the layer. The authoritative check is the same `validate_and_commit`
/// every other centerline goes through — this path adds no way to commit
/// copper the oracle has not seen.
#[allow(clippy::too_many_arguments)]
fn direct_centerline(
    session: &RouteSession,
    pcb: &Pcb,
    net: &str,
    partner: &str,
    (mid_from, mid_to): (Vec2, Vec2),
    pads: [Vec2; 4],
    half_sep: f64,
    via_r: f64,
    clearance: f64,
    w: f64,
    copper: &[PcbLayer],
) -> Option<Centerline> {
    let [from, to, p_from, p_to] = pads;
    let pad_layers = [
        pad_layers_at(pcb, net, from),
        pad_layers_at(pcb, net, to),
        pad_layers_at(pcb, partner, p_from),
        pad_layers_at(pcb, partner, p_to),
    ];
    let shared = |l: PcbLayer| pad_layers.iter().all(|ls| ls.is_empty() || ls.contains(&l));
    let order = copper
        .iter()
        .filter(|&&l| shared(l))
        .chain(copper.iter().filter(|&&l| !shared(l)));

    for &layer in order {
        let center: Centerline = vec![(mid_from, mid_to, layer)];
        let Some((leg_a, leg_b)) = realize_legs(&center, half_sep, half_sep, via_r, clearance, w)
        else {
            continue;
        };
        // Each leg must be legal for ONE of the two nets — the assignment
        // itself is decided downstream by connector length.
        let leg_legal = |leg: &Leg, as_net: &str| {
            leg.segments.iter().all(|&(a, b, l)| {
                session
                    .probe(
                        &CopperGeom::Segment {
                            a,
                            b,
                            half_w: w / 2.0,
                        },
                        l,
                        as_net,
                        clearance,
                    )
                    .legal
            })
        };
        let straight = leg_legal(&leg_a, net) && leg_legal(&leg_b, partner);
        let crossed = leg_legal(&leg_a, partner) && leg_legal(&leg_b, net);
        if straight || crossed {
            return Some(center);
        }
    }
    None
}

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

/// One pair's outcome in a [`census_pairs`] run.
#[derive(Debug, Clone)]
pub struct PairCensusRow {
    /// Positive leg net name.
    pub net_p: String,
    /// Negative leg net name.
    pub net_n: String,
    /// Crow-flight distance between the P net's two farthest pads (mm).
    pub span_mm: f64,
    /// `None` when the pair routed coupled, else why it bailed.
    pub bail: Option<PairBail>,
    /// Coupled fraction the committed copper achieved (successes only) —
    /// the quantity `min_pair_coupled_fraction` actually judges. A pair can
    /// route "coupled" and still score badly here when a long neck-down
    /// retreat leaves most of its length in uncoupled breakout stubs.
    pub coupled_fraction: f64,
}

/// Census of a board's coupled-pair construction.
#[derive(Debug, Clone, Default)]
pub struct PairCensus {
    /// Per-pair outcomes, in attempt order.
    pub rows: Vec<PairCensusRow>,
}

impl PairCensus {
    /// Pairs that routed coupled.
    pub fn coupled(&self) -> usize {
        self.rows.iter().filter(|r| r.bail.is_none()).count()
    }
    /// Pairs that routed coupled AND cleared `min_frac` coupled fraction —
    /// the count that actually helps the receipt claim.
    pub fn coupled_above(&self, min_frac: f64) -> usize {
        self.rows
            .iter()
            .filter(|r| r.bail.is_none() && r.coupled_fraction >= min_frac)
            .count()
    }
    /// Bail histogram, most frequent first.
    pub fn histogram(&self) -> Vec<(PairBail, usize)> {
        let mut counts: std::collections::BTreeMap<PairBail, usize> = Default::default();
        for r in &self.rows {
            if let Some(b) = r.bail {
                *counts.entry(b).or_default() += 1;
            }
        }
        let mut v: Vec<(PairBail, usize)> = counts.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        v
    }
}

/// Run the pair-first stage in isolation on `pcb` with all routed copper
/// stripped, and report why each classified pair did or did not construct
/// coupled.
///
/// This is the fast development loop behind lever 1: the full board takes
/// hours to route, but the pair stage sees a near-empty board in round 0
/// anyway, so censusing it standalone measures the same thing in seconds —
/// and isolates *geometry and logic* failures from congestion (on an empty
/// board, congestion is not the explanation).
pub fn census_pairs(pcb: &Pcb, max_expansions: usize) -> PairCensus {
    let mut board = pcb.clone();
    board.traces.clear();
    board.trace_arcs.clear();
    board.vias.clear();

    let nets: Vec<String> = {
        let mut v: std::collections::BTreeSet<String> = Default::default();
        for f in &board.footprints {
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
    super::classes::apply_classes(&mut board, &classifier);
    let width = board.rules.default_rules.trace_width;

    let mut session = RouteSession::from_pcb(&board);
    let mut placed: Vec<Placed> = Vec::new();
    let cong = Congestion::new(&board.outline.vertices);
    let mut census = PairCensus::default();

    for (pn, nn) in &classifier.pairs {
        let p_pads = pads_of_net(&board, pn);
        if p_pads.len() < 2 {
            continue;
        }
        // MST endpoints: the two farthest pads of the P net (the same
        // endpoints polish_pairs uses).
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
        let before = placed.len();
        let outcome = try_route_pair_reason(
            &mut session,
            &board,
            width,
            pn,
            from,
            to,
            &mut placed,
            &cong,
            max_expansions,
        );
        let (bail, frac) = match outcome {
            Ok((mine, theirs)) => {
                // Measure the committed copper the way the receipt does.
                let (w, gap) = pair_geometry(&session, &board, pn, width);
                let mut probe = board.clone();
                for pl in [&mine, &theirs] {
                    for &(a, b, l) in &pl.segments {
                        probe.traces.push(Trace {
                            start: a,
                            end: b,
                            width: pl.width,
                            layer: l,
                            net: pl.net.clone(),
                            source: None,
                        });
                    }
                    for &(a, b, l) in &pl.stubs {
                        probe.traces.push(Trace {
                            start: a,
                            end: b,
                            width: pl.stub_width,
                            layer: l,
                            net: pl.net.clone(),
                            source: None,
                        });
                    }
                }
                let f =
                    crate::router::si_claims::coupled_fraction(&probe, pn, nn, (w + gap) * 1.75);
                placed.push(mine);
                placed.push(theirs);
                (None, f)
            }
            Err(b) => {
                debug_assert_eq!(placed.len(), before, "failed pair must commit nothing");
                (Some(b), 0.0)
            }
        };
        census.rows.push(PairCensusRow {
            net_p: pn.clone(),
            net_n: nn.clone(),
            span_mm: best,
            bail,
            coupled_fraction: frac,
        });
    }
    census
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
        let plane_nets: std::collections::BTreeSet<&str> = pcb
            .zones
            .iter()
            .filter(|z| !z.net.is_empty())
            .map(|z| z.net.as_str())
            .collect();
        // Corridor test: distance from the segment ends to the from→to LINE
        // — a bbox test on a long diagonal span sweeps half the board into
        // the rip set and the all-must-reroute bar becomes unmeetable.
        let span_dir = {
            let d = to - from;
            let l = dist(from, to).max(1e-9);
            d.scale(1.0 / l)
        };
        let span_len = dist(from, to);
        let to_line = |p: Vec2| -> f64 {
            let v = p - from;
            let t = (v.x * span_dir.x + v.y * span_dir.y).clamp(0.0, span_len);
            dist(p, from + span_dir.scale(t))
        };
        let in_band = |a: Vec2, b: Vec2| to_line(a) <= band || to_line(b) <= band;
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
        log::debug!(
            "pair-polish: {pn}: ripping {} corridor singles",
            ripped_nets.len()
        );

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
                    // Fall back to the single's ORIGINAL copper when it is
                    // still legal beside the new pair.
                    let orig: Vec<(Vec2, Vec2, PcbLayer)> = ripped_copper
                        .iter()
                        .filter(|t| &t.net == net)
                        .map(|t| (t.start, t.end, t.layer))
                        .collect();
                    let ovias: Vec<(Vec2, PcbLayer, PcbLayer)> = ripped_vias
                        .iter()
                        .filter(|v| &v.net == net)
                        .map(|v| (v.position, v.start_layer, v.end_layer))
                        .collect();
                    let cand = Candidate {
                        net: net.clone(),
                        from: pads[0],
                        to: pads[pads.len() - 1],
                        width: session.width_for(net, width),
                        segments: orig,
                        vias: ovias,
                        thin_segments: vec![],
                        thin_width: width,
                    };
                    match validate_and_commit(&mut session, &work, cand, &placed) {
                        Some(pl) => {
                            rerouted.push(pl);
                            continue 'singles;
                        }
                        None => {
                            singles_ok = false;
                            break 'singles;
                        }
                    }
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
        // board (the ripped originals were removed above; reroutes or
        // restored originals replace them).
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
            // Final fail-closed gate on the assembled board, judged by the
            // DRC rather than the session probe.
            //
            // The stages above each probe their own copper as they place it,
            // but `work` is an ASSEMBLY — the pair's legs, its four breakout
            // connectors, and every displaced single re-routed around it —
            // and nothing re-checks the whole. Worse, the incremental probe
            // and the DRC do not agree at mitred pair corners: the probe let
            // through legs 0.191mm apart that the DRC rejects against the
            // 0.245mm pair-gap rule. The DRC is the standard the receipt is
            // judged by, so gate on the DRC, restricted to the pair's own
            // bounding box to keep it affordable. Measured on the CM5 subset:
            // routed board 0 intra-pair violations, polished board 3.
            let (mut lo_b, mut hi_b) = (
                Vec2::new(f64::INFINITY, f64::INFINITY),
                Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY),
            );
            for t in work.traces.iter().filter(|t| &t.net == pn || &t.net == nn) {
                for p in [t.start, t.end] {
                    lo_b.x = lo_b.x.min(p.x - 1.0);
                    lo_b.y = lo_b.y.min(p.y - 1.0);
                    hi_b.x = hi_b.x.max(p.x + 1.0);
                    hi_b.y = hi_b.y.max(p.y + 1.0);
                }
            }
            let hard_here = |b: &Pcb| -> usize {
                if !lo_b.x.is_finite() {
                    return 0;
                }
                crate::drc::check_drc_in_region(b, lo_b, hi_b)
                    .iter()
                    .filter(|v| {
                        matches!(
                            v.rule,
                            crate::drc::DrcRuleType::Clearance | crate::drc::DrcRuleType::Short
                        ) && matches!(v.severity, crate::drc::DrcSeverity::Error)
                    })
                    .count()
            };
            let pair_legal = hard_here(&work) <= hard_here(pcb);
            if !pair_legal {
                log::debug!("pair-polish: {pn} assembled board is illegal — reverting");
                continue;
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
        // USB DP/DM: the classifier declares these as pairs, so this must
        // agree or every USB pair falls through to independent singles.
        assert_eq!(pair_partner("/USB3-0.DP").as_deref(), Some("/USB3-0.DM"));
        assert_eq!(pair_partner("/USB3-0.DM").as_deref(), Some("/USB3-0.DP"));
        assert_eq!(pair_partner("/USBC.DP").as_deref(), Some("/USBC.DM"));
        assert_eq!(pair_partner("USB_D+").as_deref(), Some("USB_D-"));
        // Separator required: a name merely ENDING in "DP" is not a pair.
        assert_eq!(pair_partner("LDP"), None);
        assert_eq!(pair_partner("DP"), None);
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
                    target_impedance: None,
                    target_diff_impedance: None,
                },
                class_rules: vec![NetClassRules {
                    name: "DIFF".into(),
                    trace_width: 0.2,
                    clearance: 0.15,
                    via_diameter: 0.35,
                    via_drill: 0.2,
                    diff_pair_gap: Some(0.25),
                    diff_pair_width: Some(0.2),
                    target_impedance: None,
                    target_diff_impedance: None,
                }],
                net_class_assignments: Default::default(),
                edge_clearance: 0.5,
                // A pair transitions layers by dropping the two legs' vias
                // side by side, so the rule has to admit the pitch the pair
                // itself declares: legs sit gap + width = 0.45mm apart, and
                // two 0.2mm drills there leave a 0.25mm hole gap. A 0.5mm
                // rule would make *every* transition on this board
                // unmanufacturable — the router now refuses those at probe
                // time instead of emitting them (see
                // `RouteSession::probe_hole`).
                hole_to_hole: 0.2,
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

    /// A pair whose pads sit ~2 mm apart — too short for a lead-inset phantom
    /// centerline, which is exactly the `/HS.*` case that pinned the SI
    /// receipt's `min_pair_coupled_fraction` at 0.000: routed as two singles
    /// the legs land on different layers and score zero coupling however well
    /// the rest of the board routes.
    #[test]
    fn routes_short_span_pair_coupled_on_one_layer() {
        let mut pcb = pair_board();
        // Move the far connector to 2 mm from the near one.
        pcb.footprints[1].position = Vec2::new(7.0, 15.0);
        let mut session = RouteSession::from_pcb(&pcb);
        let cong = Congestion::new(&pcb.outline.vertices);
        let mut placed = Vec::new();
        let (p, n) = try_route_pair(
            &mut session,
            &pcb,
            0.25,
            "/ETH.2_P",
            Vec2::new(5.0, 15.325),
            Vec2::new(7.0, 15.325),
            &mut placed,
            &cong,
            200_000,
        )
        .expect("a 2mm pad-to-pad pair must still route coupled");
        assert!(!p.segments.is_empty() && !n.segments.is_empty());
        // The point of the short-pair path: both legs on ONE layer, which is
        // what `si_claims::coupled_fraction` measures (it only counts twin
        // copper on the SAME layer).
        let layers = |pl: &Placed| -> Vec<PcbLayer> {
            let mut ls: Vec<_> = pl.segments.iter().map(|(_, _, l)| *l).collect();
            ls.sort_by_key(|l| format!("{l:?}"));
            ls.dedup();
            ls
        };
        assert_eq!(
            layers(&p),
            layers(&n),
            "short pair legs must share their layer, else coupled_fraction is 0"
        );
        // And the coupled run is measurable, not a token stub.
        let frac = crate::router::si_claims::coupled_fraction(
            &{
                let mut b = pcb.clone();
                for pl in [&p, &n] {
                    for (a, e, l) in &pl.segments {
                        b.traces.push(Trace {
                            start: *a,
                            end: *e,
                            width: pl.width,
                            layer: *l,
                            net: pl.net.clone(),
                            source: None,
                        });
                    }
                }
                b
            },
            "/ETH.2_P",
            "/ETH.2_N",
            0.45 + 0.1,
        );
        assert!(
            frac >= 0.5,
            "short pair should be at least half coupled, got {frac}"
        );
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

    /// Every via a pair route REPORTS must be a via the session KNOWS about.
    ///
    /// The four pad connectors are maze routes, and when the pads sit on a
    /// layer the coupled legs don't use, each connector carries a via to get
    /// there. Those vias are written onto the output board like any other — so
    /// if the session never receives them, the oracle cannot refuse a later
    /// route that drives straight through the barrel. It is not a near-miss:
    /// the copper physically overlaps at 0.000mm, and no probe was ever wrong,
    /// because no probe was ever asked.
    #[test]
    fn pair_connector_vias_are_visible_to_the_oracle() {
        let mut pcb = pair_board();
        // Put all four pads on BCu only, so the FCu coupled legs can only be
        // reached through a connector via.
        for f in &mut pcb.footprints {
            for p in &mut f.pads {
                p.layers = vec![PcbLayer::BCu];
            }
        }
        // ...and wall BCu off mid-board so the coupled run has to live on FCu.
        pcb.traces.push(vcad_ir::ecad::Trace {
            start: Vec2::new(25.0, 0.0),
            end: Vec2::new(25.0, 30.0),
            width: 1.0,
            layer: PcbLayer::BCu,
            net: "WALL".into(),
            source: None,
        });
        let mut session = RouteSession::from_pcb(&pcb);
        let cong = Congestion::new(&pcb.outline.vertices);
        let mut placed = Vec::new();
        let Some((p, n)) = try_route_pair(
            &mut session,
            &pcb,
            0.25,
            "/ETH.2_P",
            Vec2::new(5.0, 15.325),
            Vec2::new(45.0, 15.325),
            &mut placed,
            &cong,
            200_000,
        ) else {
            panic!("pair with BCu pads must still route");
        };
        let vias: Vec<_> = p.via_pts.iter().chain(n.via_pts.iter()).collect();
        assert!(
            !vias.is_empty(),
            "fixture is vacuous: the connectors placed no vias"
        );
        let via_r = pcb.rules.default_rules.via_diameter / 2.0;
        for &&(pt, la, lb) in &vias {
            for layer in [la, lb] {
                let pr = session.probe(
                    &crate::spatial::CopperGeom::Disc {
                        center: pt,
                        r: via_r,
                    },
                    layer,
                    "SOME-OTHER-NET",
                    pcb.rules.default_rules.clearance,
                );
                assert!(
                    !pr.legal,
                    "via at ({:.3},{:.3}) on {layer:?} is reported on the board but \
                     absent from the session — a foreign net can be routed through it",
                    pt.x, pt.y
                );
            }
        }
        // And the drill barrel likewise: hole-to-hole must see it.
        for &&(pt, _, _) in &vias {
            assert!(
                !session.probe_via_drill(pt, "SOME-OTHER-NET").legal,
                "via drill at ({:.3},{:.3}) is absent from the session's hole census",
                pt.x,
                pt.y
            );
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
        assert_twin_holes_clear(&pcb, &mine, &theirs);
    }

    /// The twins' via DRILLS must keep the board's hole-to-hole rule — the
    /// check that has no copper-layer counterpart when the two vias land on
    /// disjoint layer spans.
    fn assert_twin_holes_clear(pcb: &Pcb, mine: &Placed, theirs: &Placed) {
        let r = pcb.rules.default_rules.via_drill / 2.0;
        for &(p, _, _) in &mine.via_pts {
            for &(q, _, _) in &theirs.via_pts {
                let gap = dist(p, q) - 2.0 * r;
                assert!(
                    gap >= pcb.rules.hole_to_hole - 1e-6,
                    "twin via hole gap {gap:.3}mm < {}",
                    pcb.rules.hole_to_hole
                );
            }
        }
    }

    /// Twin-clearance check shared by the transition repros, using the
    /// DRC's own pair semantics: LEG copper must keep the declared gap
    /// (minus the 5um tolerance); stub/connector copper (the pad breakout)
    /// needs only the base clearance.
    fn assert_twin_clear(mine: &Placed, theirs: &Placed, clearance: f64) {
        let gap_req = 0.25 - 0.005;
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
        // Legs vs legs: full gap. Anything touching a stub: base clearance.
        let legs = |p: &Placed| -> Vec<(Vec2, Vec2, PcbLayer, f64)> {
            p.segments
                .iter()
                .map(|&(a, b, l)| (a, b, l, p.width))
                .collect()
        };
        let mut worst_leg = f64::INFINITY;
        for &(a1, b1, l1, w1) in &legs(mine) {
            for &(a2, b2, l2, w2) in &legs(theirs) {
                if l1 == l2 {
                    worst_leg = worst_leg.min(seg_seg(a1, b1, a2, b2) - w1 / 2.0 - w2 / 2.0);
                }
            }
        }
        assert!(
            worst_leg >= gap_req - 1e-9,
            "twin LEG gap {worst_leg:.3}mm < {gap_req} (DRC pair-gap rule)"
        );
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

    /// The SI finishing pass is non-regressive and stays DRC-clean.
    ///
    /// `si_finish` rewrites committed copper (polish rips and re-routes whole
    /// pairs; descent moves polyline interiors), so its contract is the one
    /// thing every caller relies on: it may improve the pair claims or do
    /// nothing, but it must never leave the board worse or dirtier than it
    /// found it. Both stages are individually oracle-gated — this pins the
    /// composition.
    #[test]
    fn si_finish_is_non_regressive_and_drc_clean() {
        use super::super::auto::{route_all_with_opts, RouteOptions};
        let mut pcb = pair_board();
        let r = route_all_with_opts(&pcb, 0.25, &[], &RouteOptions::default());
        assert_eq!(r.unrouted_nets.len(), 0, "both legs must route");
        for t in &r.traces {
            pcb.traces.push(Trace {
                start: t.start,
                end: t.end,
                width: t.width,
                layer: t.layer,
                net: t.net.clone(),
                source: None,
            });
        }
        for v in &r.vias {
            pcb.vias.push(Via {
                position: v.position,
                diameter: pcb.rules.default_rules.via_diameter,
                drill: pcb.rules.default_rules.via_drill,
                start_layer: v.start_layer,
                end_layer: v.end_layer,
                net: v.net.clone(),
                source: None,
            });
        }
        let clearance_errs = |p: &Pcb| -> usize {
            crate::drc::check_drc(p)
                .iter()
                .filter(|v| {
                    matches!(v.rule, crate::drc::DrcRuleType::Clearance)
                        && matches!(v.severity, crate::drc::DrcSeverity::Error)
                })
                .count()
        };
        let before_errs = clearance_errs(&pcb);
        let frac = |p: &Pcb| {
            crate::router::si_claims::coupled_fraction(p, "/ETH.2_P", "/ETH.2_N", 0.45 * 1.75)
        };
        let before_frac = frac(&pcb);
        let before_skew = {
            let lp = super::super::length_match::net_routed_length(&pcb, "/ETH.2_P");
            let ln = super::super::length_match::net_routed_length(&pcb, "/ETH.2_N");
            (lp - ln).abs()
        };

        crate::router::si_finish(&mut pcb, 200_000, 500);

        assert!(
            clearance_errs(&pcb) <= before_errs,
            "si_finish introduced clearance violations"
        );
        assert!(
            frac(&pcb) >= before_frac - 1e-9,
            "coupled fraction regressed: {before_frac:.3} -> {:.3}",
            frac(&pcb)
        );
        let after_skew = {
            let lp = super::super::length_match::net_routed_length(&pcb, "/ETH.2_P");
            let ln = super::super::length_match::net_routed_length(&pcb, "/ETH.2_N");
            (lp - ln).abs()
        };
        assert!(
            after_skew <= before_skew + 1e-6,
            "intra-pair skew regressed: {before_skew:.3} -> {after_skew:.3}"
        );
    }

    /// Connectivity contract: every pad of the pair must be left on a layer
    /// that pad is actually on.
    ///
    /// Coupled legs may begin anywhere in the stack (that is how a pair
    /// escapes a BGA — down a via, coupled on an inner layer under the
    /// field). The pad connectors therefore have to carry the layer change
    /// themselves. Nothing else catches a miss: copper starting at the pad's
    /// XY on the wrong layer is same-net, so the clearance probe is perfectly
    /// happy while the net is electrically open.
    #[test]
    fn pair_connectors_leave_pads_on_pad_layers() {
        let _ = env_logger::builder().is_test(true).try_init();
        let mut pcb = pair_board();
        // Wall across FCu forces the coupled run onto BCu, so the legs no
        // longer start on the pads' own layer.
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
        let (mine, theirs) = try_route_pair(
            &mut session,
            &pcb,
            0.25,
            "/ETH.2_P",
            Vec2::new(5.0, 15.325),
            Vec2::new(45.0, 15.325),
            &mut placed,
            &cong,
            400_000,
        )
        .expect("pair must route across the FCu wall");

        for pl in [&mine, &theirs] {
            let pads = pads_of_net(&pcb, &pl.net);
            assert!(!pads.is_empty());
            // All copper this net committed, with its layer.
            let copper: Vec<(Vec2, Vec2, PcbLayer)> =
                pl.segments.iter().chain(pl.stubs.iter()).copied().collect();
            for pad in pads {
                let pad_layers = pad_layers_at(&pcb, &pl.net, pad);
                let touched = copper.iter().any(|&(a, b, l)| {
                    pad_layers.contains(&l) && (dist(a, pad) < 1e-6 || dist(b, pad) < 1e-6)
                });
                assert!(
                    touched,
                    "{}: pad ({:.3},{:.3}) is not left on any of its own layers {:?} — \
                     the net is electrically open there",
                    pl.net, pad.x, pad.y, pad_layers
                );
            }
        }
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
        assert_twin_holes_clear(&pcb, &mine, &theirs);
    }

    /// Probe-level contract: with a declared pair class, leg-width copper of
    /// one twin at less than the gap from the other twin's leg-width copper
    /// must probe ILLEGAL — the rule the DRC enforces, enforced at the
    /// source. (The si8 boards carried 0.127mm leg pinches the probe let
    /// through; this test pins the contract.)
    #[test]
    fn probe_enforces_intra_pair_gap() {
        let pcb = pair_board();
        let mut session = RouteSession::from_pcb(&pcb);
        // Commit a P leg.
        session.commit(crate::spatial::CopperElement {
            min: [10.0 - 0.2, 20.0 - 0.2],
            max: [30.0 + 0.2, 20.0 + 0.2],
            net: "/ETH.2_P".into(),
            layer: PcbLayer::FCu,
            geom: CopperGeom::Segment {
                a: Vec2::new(10.0, 20.0),
                b: Vec2::new(30.0, 20.0),
                half_w: 0.1,
            },
        });
        // N leg-width copper 0.13mm edge-to-edge away: legal at base
        // clearance (0.15... test board clearance 0.15 — use 0.33 center =
        // 0.13 edge) but ILLEGAL under the 0.25 gap.
        let n_leg = CopperGeom::Segment {
            a: Vec2::new(10.0, 20.33),
            b: Vec2::new(30.0, 20.33),
            half_w: 0.1,
        };
        // Edge distance = 0.33 - 0.2 = 0.13 < 0.245.
        let pr = session.probe(&n_leg, PcbLayer::FCu, "/ETH.2_N", 0.15);
        assert!(
            !pr.legal,
            "leg-width twin copper at 0.13mm must be illegal (gap rule), min_clearance={:.3}",
            pr.min_clearance
        );
        // Thin (neck) copper at the same distance stays legal vs base 0.08.
        let n_neck = CopperGeom::Segment {
            a: Vec2::new(10.0, 20.33),
            b: Vec2::new(30.0, 20.33),
            half_w: 0.04,
        };
        let pr2 = session.probe(&n_neck, PcbLayer::FCu, "/ETH.2_N", 0.08);
        assert!(pr2.legal, "neck copper at base clearance stays legal");
    }

    /// End-to-end: routing the twins as independent SINGLES through the
    /// ordinary maze must still respect the pair gap — the session probe is
    /// the enforcement point every stage shares. (si8 carried parallel
    /// leg-width singles at 0.33mm separation = fallback-geometry spacing;
    /// this pins the e2e contract the probe test alone cannot.)
    #[test]
    fn singles_of_a_pair_keep_the_gap() {
        use super::super::auto::{route_all_with_opts, RouteOptions};
        let pcb = pair_board();
        let r = route_all_with_opts(&pcb, 0.25, &[], &RouteOptions::default());
        assert_eq!(r.unrouted_nets.len(), 0, "both legs must route");
        // Min distance between P and N copper (leg width only).
        let seg_seg = |a1: Vec2, b1: Vec2, a2: Vec2, b2: Vec2| -> f64 {
            let pt = |p: Vec2, a: Vec2, b: Vec2| -> f64 {
                let ab = b - a;
                let l2 = ab.x * ab.x + ab.y * ab.y;
                if l2 < 1e-18 {
                    return dist(p, a);
                }
                let t = (((p.x - a.x) * ab.x + (p.y - a.y) * ab.y) / l2).clamp(0.0, 1.0);
                dist(p, a + ab.scale(t))
            };
            pt(a1, a2, b2)
                .min(pt(b1, a2, b2))
                .min(pt(a2, a1, b1))
                .min(pt(b2, a1, b1))
        };
        let mut worst = f64::INFINITY;
        for t1 in r
            .traces
            .iter()
            .filter(|t| t.net == "/ETH.2_P" && t.width >= 0.19)
        {
            for t2 in r
                .traces
                .iter()
                .filter(|t| t.net == "/ETH.2_N" && t.width >= 0.19)
            {
                if t1.layer == t2.layer {
                    worst = worst.min(
                        seg_seg(t1.start, t1.end, t2.start, t2.end)
                            - t1.width / 2.0
                            - t2.width / 2.0,
                    );
                }
            }
        }
        assert!(
            worst >= 0.245 - 1e-9,
            "leg-width singles of a pair must keep the gap: worst={worst:.3}"
        );
    }
}
