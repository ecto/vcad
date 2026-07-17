//! True shove: displace blocking routed traces sideways (KiCad PNS-style)
//! instead of ripping them, as the router's last-resort stage.
//!
//! When a connection's search fails and existing *routed* traces block the
//! corridor, [`try_route_shove`] finds the blocking [`Placed`] routes along
//! the direct corridor, translates each blocker's offending middle segments
//! perpendicular to the corridor by the smallest displacement that both stays
//! clearance-legal and lets the stuck net's fresh maze search succeed — then
//! commits displaced victim and new route together, transactionally.
//!
//! Deliberate bounds (legality safety over completeness):
//! - Displacement magnitude is capped at [`MAX_SHOVE_MM`].
//! - Recursion depth is 1: a displaced trace must itself probe legal in its
//!   new position — it may never displace *other* copper in turn.
//! - Only MIDDLE segments whose endpoints are trace-trace joints move;
//!   a crossing segment anchored at a pad, via, or fan-out stub endpoint
//!   refuses the shove for that victim.
//! - Victims are shoved one at a time (no joint multi-victim plans).

use vcad_ir::ecad::{Pcb, PcbLayer};
use vcad_ir::Vec2;

use crate::session::RouteSession;
use crate::spatial::CopperGeom;

use super::auto::{
    commit_seg, commit_via, copper_layers, dist, search_route, validate_and_commit, Placed,
};
use super::congestion::Congestion;

/// Hard cap (mm) on how far a shove may translate a victim's segments.
pub(super) const MAX_SHOVE_MM: f64 = 1.2;

/// Displacement step (mm); candidates are tried at `±STEP, ±2·STEP, …` up to
/// [`MAX_SHOVE_MM`], alternating signs so the smallest workable move wins.
const SHOVE_STEP_MM: f64 = 0.2;

/// At most this many distinct blocking routes are considered per connection.
const MAX_SHOVE_VICTIMS: usize = 3;

/// Two points are "the same joint" below this distance (mm).
const JOINT_EPS: f64 = 1e-6;

/// A point is "anchored" to a pad/via/stub endpoint below this distance (mm).
const ANCHOR_EPS: f64 = 0.05;

/// The candidate displacement magnitudes, smallest first, alternating signs.
fn deltas() -> impl Iterator<Item = f64> {
    let n = (MAX_SHOVE_MM / SHOVE_STEP_MM).round() as usize;
    (1..=n).flat_map(|k| {
        let d = k as f64 * SHOVE_STEP_MM;
        [d, -d]
    })
}

/// The plan for one victim: which segment indices move, and the set of joint
/// points (per layer) that translate with them.
struct ShovePlan {
    /// Indices into the victim's `segments` that cross the corridor.
    offending: Vec<usize>,
    /// Joints to translate: `(point, layer)`.
    moved: Vec<(Vec2, PcbLayer)>,
}

/// A shove translates copper sideways; only segments running roughly ALONG
/// the corridor can be freed that way (a perpendicular crossing slides along
/// itself and never clears the channel). Segments at least this aligned with
/// the corridor direction (|cos|) are shove targets.
const MIN_ALIGNMENT: f64 = 0.5;

/// Segments of `victim` whose copper lies within `corridor_hw` of the
/// corridor centerline `from -> to` AND runs roughly parallel to it — the
/// segments a perpendicular translation could move out of the way.
fn offending_segments(victim: &Placed, from: Vec2, to: Vec2, corridor_hw: f64) -> Vec<usize> {
    let corridor = CopperGeom::Segment {
        a: from,
        b: to,
        half_w: 0.0,
    };
    let cdir = (to - from).normalize();
    victim
        .segments
        .iter()
        .enumerate()
        .filter(|(_, (a, b, _))| {
            let sdir = (*b - *a).normalize();
            if cdir.dot(sdir).abs() < MIN_ALIGNMENT {
                return false;
            }
            let seg = CopperGeom::Segment {
                a: *a,
                b: *b,
                half_w: victim.width / 2.0,
            };
            seg.distance_to(&corridor) < corridor_hw
        })
        .map(|(i, _)| i)
        .collect()
}

/// Build the shove plan for `victim`, or `None` when any offending segment is
/// anchored: an endpoint at the victim's pads (`from`/`to`), at one of its
/// vias, at a fan-out stub, or dangling (shared with no neighbor segment).
/// Only interior trace-trace joints may move — that keeps the displaced
/// polyline connected by construction (neighbors sharing a moved joint are
/// re-stitched by moving the shared endpoint with it).
fn plan_shove(victim: &Placed, from: Vec2, to: Vec2, corridor_hw: f64) -> Option<ShovePlan> {
    let offending = offending_segments(victim, from, to, corridor_hw);
    if offending.is_empty() {
        return None;
    }
    let anchored = |p: Vec2| -> bool {
        dist(p, victim.from) < ANCHOR_EPS
            || dist(p, victim.to) < ANCHOR_EPS
            || victim
                .via_pts
                .iter()
                .any(|&(vp, _, _)| dist(p, vp) < ANCHOR_EPS)
            || victim
                .stubs
                .iter()
                .any(|&(a, b, _)| dist(p, a) < ANCHOR_EPS || dist(p, b) < ANCHOR_EPS)
    };
    let mut moved: Vec<(Vec2, PcbLayer)> = Vec::new();
    for &i in &offending {
        let (a, b, layer) = victim.segments[i];
        for p in [a, b] {
            if anchored(p) {
                return None;
            }
            // A movable joint must be shared with at least one other segment
            // on the same layer — an unshared endpoint that isn't a pad or
            // via is a dangling end we don't understand; refuse.
            let shared = victim.segments.iter().enumerate().any(|(j, (sa, sb, sl))| {
                j != i && *sl == layer && (dist(*sa, p) < JOINT_EPS || dist(*sb, p) < JOINT_EPS)
            });
            if !shared {
                return None;
            }
            if !moved
                .iter()
                .any(|&(mp, ml)| ml == layer && dist(mp, p) < JOINT_EPS)
            {
                moved.push((p, layer));
            }
        }
    }
    Some(ShovePlan { offending, moved })
}

/// The victim's segments with every planned joint translated by `offset`.
/// Neighbors sharing a moved joint stretch to follow it, so the polyline
/// stays connected. Returns the new segment list plus the indices of every
/// segment whose geometry changed (the ones that need re-probing).
fn displaced_segments(
    victim: &Placed,
    plan: &ShovePlan,
    offset: Vec2,
) -> (Vec<(Vec2, Vec2, PcbLayer)>, Vec<usize>) {
    let is_moved = |p: Vec2, l: PcbLayer| {
        plan.moved
            .iter()
            .any(|&(mp, ml)| ml == l && dist(mp, p) < JOINT_EPS)
    };
    let mut out = Vec::with_capacity(victim.segments.len());
    let mut changed = Vec::new();
    for (i, &(a, b, layer)) in victim.segments.iter().enumerate() {
        let na = if is_moved(a, layer) { a + offset } else { a };
        let nb = if is_moved(b, layer) { b + offset } else { b };
        if na != a || nb != b {
            changed.push(i);
        }
        out.push((na, nb, layer));
    }
    (out, changed)
}

/// Commit `segments` and the victim's (unchanged) vias to the session,
/// returning the new span ids.
fn commit_victim_copper(
    session: &mut RouteSession,
    pcb: &Pcb,
    victim: &Placed,
    segments: &[(Vec2, Vec2, PcbLayer)],
) -> Vec<crate::session::SpanId> {
    let hw = victim.width / 2.0;
    let via_r = pcb.rules.default_rules.via_diameter / 2.0;
    let copper = copper_layers(pcb);
    let mut spans = Vec::new();
    for &(a, b, l) in segments {
        spans.push(commit_seg(session, &victim.net, a, b, hw, l));
    }
    for &(p, la, lb) in &victim.via_pts {
        let ia = copper.iter().position(|&l| l == la).unwrap_or(0);
        let ib = copper
            .iter()
            .position(|&l| l == lb)
            .unwrap_or(copper.len().saturating_sub(1));
        let slice = &copper[ia.min(ib)..=ia.max(ib)];
        commit_via(session, &victim.net, p, via_r, slice, &mut spans);
    }
    // Fan-out stubs never ride a shove (plan_shove refuses stub-anchored
    // segments), but a victim may still CARRY stubs elsewhere on its route;
    // they are part of its committed copper and must come back with it.
    for &(a, b, l) in &victim.stubs {
        spans.push(commit_seg(session, &victim.net, a, b, hw, l));
    }
    spans
}

/// Last-resort shove stage for a connection every search stage failed:
/// displace the routed traces blocking the direct corridor sideways and
/// re-search. On success the shoved victim's entry in `placed` is updated in
/// place (new segments + spans) and the stuck connection's new [`Placed`] is
/// returned; on failure everything is restored exactly as it was and `None`
/// is returned.
#[allow(clippy::too_many_arguments)]
pub(super) fn try_route_shove(
    session: &mut RouteSession,
    pcb: &Pcb,
    width: f64,
    net: &str,
    from: Vec2,
    to: Vec2,
    placed: &mut [Placed],
    cong: &Congestion,
    max_expansions: usize,
) -> Option<Placed> {
    if dist(from, to) < JOINT_EPS {
        return None;
    }
    let w = session.width_for(net, width);
    let clearance = session.clearance_for(net);
    // Wide enough to catch parallel traces a shove could move out of the way:
    // the trace's own footprint, a couple of trace pitches of slack, and the
    // full shove range.
    let corridor_hw = w / 2.0 + clearance + w * 2.0 + MAX_SHOVE_MM;
    let corridor = CopperGeom::Segment {
        a: from,
        b: to,
        half_w: corridor_hw,
    };
    let copper = copper_layers(pcb);

    // Which placed routes (other nets) block the corridor.
    let mut blocker_spans = std::collections::HashSet::new();
    for &layer in &copper {
        for b in session.probe(&corridor, layer, net, clearance).blockers {
            blocker_spans.insert(b.span);
        }
    }
    let victims: Vec<usize> = placed
        .iter()
        .enumerate()
        .filter(|(_, p)| p.net != net && p.spans.iter().any(|s| blocker_spans.contains(s)))
        .map(|(i, _)| i)
        .take(MAX_SHOVE_VICTIMS)
        .collect();
    if victims.is_empty() {
        return None;
    }

    // Perpendicular to the corridor: the direction a shove translates along.
    let d = to - from;
    let len = dist(from, to);
    let perp = Vec2::new(-d.y / len, d.x / len);

    for vi in victims {
        let victim = placed[vi].clone();
        let Some(plan) = plan_shove(&victim, from, to, corridor_hw) else {
            log::debug!(
                "shove: {} blocking {net} has pad/via-anchored crossing segments, skipping",
                victim.net
            );
            continue;
        };
        // The victim's CURRENT span ids: a rolled-back attempt re-commits the
        // original copper under fresh ids, which the next attempt must lift.
        let mut cur_spans = victim.spans.clone();
        for delta in deltas() {
            let offset = Vec2::new(perp.x * delta, perp.y * delta);
            let (new_segments, changed) = displaced_segments(&victim, &plan, offset);

            // Transaction: lift the victim's copper out, try the displaced
            // geometry plus the stuck net's fresh search, and restore the
            // original copper on any failure (the space is free — it was
            // only just removed).
            for &s in &cur_spans {
                session.remove(s);
            }

            // Depth-1 legality: every changed segment must probe legal as-is
            // — a displaced trace may never displace others in turn.
            let vhw = victim.width / 2.0;
            let vclr = session.clearance_for(&victim.net);
            let legal = changed.iter().all(|&i| {
                let (a, b, l) = new_segments[i];
                session
                    .probe(
                        &CopperGeom::Segment { a, b, half_w: vhw },
                        l,
                        &victim.net,
                        vclr,
                    )
                    .legal
            });
            if !legal {
                cur_spans = commit_victim_copper(session, pcb, &victim, &victim.segments);
                placed[vi].spans = cur_spans.clone();
                log::debug!(
                    "shove: {} by {delta:+.1}mm for {net} is not clearance-legal",
                    victim.net
                );
                continue;
            }

            // Commit the displaced victim, then re-search the stuck net in a
            // corridor window around the connection.
            let new_spans = commit_victim_copper(session, pcb, &victim, &new_segments);
            let routed = search_route(
                session,
                pcb,
                width,
                net,
                from,
                to,
                cong,
                false,
                max_expansions,
                true,
                Some((from, to)),
            )
            .and_then(|cand| validate_and_commit(session, pcb, cand, placed));

            match routed {
                Some(p) => {
                    placed[vi].segments = new_segments;
                    placed[vi].spans = new_spans;
                    log::info!(
                        "shove: displaced {} by {delta:+.1}mm to free {net} ({} segment(s) moved)",
                        victim.net,
                        plan.offending.len()
                    );
                    return Some(p);
                }
                None => {
                    // Roll back: lift the displaced copper, restore the
                    // original.
                    for &s in &new_spans {
                        session.remove(s);
                    }
                    cur_spans = commit_victim_copper(session, pcb, &victim, &victim.segments);
                    placed[vi].spans = cur_spans.clone();
                    log::debug!(
                        "shove: {} by {delta:+.1}mm did not free {net}, rolled back",
                        victim.net
                    );
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn victim(segments: Vec<(Vec2, Vec2, PcbLayer)>, from: Vec2, to: Vec2) -> Placed {
        Placed {
            net: "B".into(),
            from,
            to,
            width: 0.25,
            segments,
            stubs: Vec::new(),
            via_pts: Vec::new(),
            spans: Vec::new(),
        }
    }

    #[test]
    fn plan_moves_only_interior_joints() {
        // Three-segment U: pads at the tips, middle segment parallel to and
        // near the corridor. The plan must move exactly the two interior
        // joints and refuse nothing.
        let l = PcbLayer::FCu;
        let v = victim(
            vec![
                (Vec2::new(10.0, 5.0), Vec2::new(10.0, 3.0), l),
                (Vec2::new(10.0, 3.0), Vec2::new(40.0, 3.0), l),
                (Vec2::new(40.0, 3.0), Vec2::new(40.0, 5.0), l),
            ],
            Vec2::new(10.0, 5.0),
            Vec2::new(40.0, 5.0),
        );
        let plan = plan_shove(&v, Vec2::new(5.0, 3.2), Vec2::new(45.0, 3.2), 1.0)
            .expect("interior middle segment must be shovable");
        assert_eq!(plan.offending, vec![1]);
        assert_eq!(plan.moved.len(), 2);

        let (segs, changed) = displaced_segments(&v, &plan, Vec2::new(0.0, -0.4));
        // All three segments change: the middle translates, the flanks
        // stretch to the moved joints.
        assert_eq!(changed, vec![0, 1, 2]);
        assert_eq!(segs[1].0, Vec2::new(10.0, 2.6));
        assert_eq!(segs[1].1, Vec2::new(40.0, 2.6));
        // Flank pad ends stay put; joint ends follow.
        assert_eq!(segs[0].0, Vec2::new(10.0, 5.0));
        assert_eq!(segs[0].1, Vec2::new(10.0, 2.6));
        assert_eq!(segs[2].1, Vec2::new(40.0, 5.0));
    }

    #[test]
    fn plan_refuses_pad_anchored_crossing_segment() {
        // A single pad-to-pad segment lying in the corridor: both endpoints
        // are anchors, so the shove must refuse.
        let l = PcbLayer::FCu;
        let v = victim(
            vec![(Vec2::new(10.0, 3.0), Vec2::new(40.0, 3.0), l)],
            Vec2::new(10.0, 3.0),
            Vec2::new(40.0, 3.0),
        );
        assert!(plan_shove(&v, Vec2::new(5.0, 3.2), Vec2::new(45.0, 3.2), 1.0).is_none());
    }

    #[test]
    fn plan_refuses_via_anchored_joint() {
        let l = PcbLayer::FCu;
        let mut v = victim(
            vec![
                (Vec2::new(10.0, 5.0), Vec2::new(10.0, 3.0), l),
                (Vec2::new(10.0, 3.0), Vec2::new(40.0, 3.0), l),
                (Vec2::new(40.0, 3.0), Vec2::new(40.0, 5.0), l),
            ],
            Vec2::new(10.0, 5.0),
            Vec2::new(40.0, 5.0),
        );
        // A via sits exactly on one of the joints the shove would move.
        v.via_pts
            .push((Vec2::new(10.0, 3.0), PcbLayer::FCu, PcbLayer::BCu));
        assert!(plan_shove(&v, Vec2::new(5.0, 3.2), Vec2::new(45.0, 3.2), 1.0).is_none());
    }

    #[test]
    fn no_offending_segments_means_no_plan() {
        let l = PcbLayer::FCu;
        let v = victim(
            vec![(Vec2::new(10.0, 20.0), Vec2::new(40.0, 20.0), l)],
            Vec2::new(10.0, 20.0),
            Vec2::new(40.0, 20.0),
        );
        assert!(plan_shove(&v, Vec2::new(5.0, 3.0), Vec2::new(45.0, 3.0), 1.0).is_none());
    }
}
