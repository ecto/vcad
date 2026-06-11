//! Merged cut silhouette of a flat pattern.
//!
//! A [`crate::unfold::FlatPattern`] stores one outline per panel, with the
//! bend-allowance strips between panels covered by no outline. Fab services
//! (SendCutSend et al.) reject that: they need a **single closed cut
//! profile** whose interior is the whole blank — panels *plus* allowance
//! strips.
//!
//! [`merged_silhouette`] computes that profile without a general polygon
//! boolean. It exploits an invariant of the sheet-metal model: every
//! allowance strip shares its inner edge with the parent panel's hinge edge
//! *exactly* (same endpoints) and its outer edge with the child panel's
//! first edge exactly — [`crate::edge_flange::add_edge_flange`] constructs
//! the child as an `edge_len × length` rectangle offset by the allowance.
//! So the union boundary falls out of **directed-edge cancellation**:
//! orient every ring CCW, collect directed edges, cancel coincident
//! opposite pairs (interior seams), and chain what survives into closed
//! loops.

use std::collections::HashMap;
use vcad_kernel_math::Point2;

/// Quantisation grid for endpoint matching (1e-6 mm — far below any
/// manufacturing tolerance, far above f64 noise at part scale).
const KEY_SCALE: f64 = 1e6;

type Key = (i64, i64);

fn key(p: Point2) -> Key {
    (
        (p.x * KEY_SCALE).round() as i64,
        (p.y * KEY_SCALE).round() as i64,
    )
}

fn signed_area(ring: &[Point2]) -> f64 {
    let mut sum = 0.0;
    for i in 0..ring.len() {
        let a = ring[i];
        let b = ring[(i + 1) % ring.len()];
        sum += a.x * b.y - b.x * a.y;
    }
    0.5 * sum
}

/// Compute the boundary loops of the union of `rings` (panel outlines +
/// allowance strips).
///
/// Rings may be in either orientation; each is normalised to CCW first.
/// Returns the surviving boundary loops — for a well-formed single-blank
/// flat pattern that is one exterior loop (plus interior loops if the
/// union genuinely encloses a hole). Collinear runs are merged so the
/// profile is minimal. Returns an empty `Vec` if `rings` is empty.
pub fn merged_silhouette(rings: &[Vec<Point2>]) -> Vec<Vec<Point2>> {
    // 1. Normalise to CCW and collect raw directed edges. Degenerate rings
    //    (< 3 points) are skipped.
    let mut raw_edges: Vec<(Point2, Point2)> = Vec::new();
    // Real coordinates per vertex key (first writer wins — coincident
    // points agree to within the grid by construction).
    let mut coords: HashMap<Key, Point2> = HashMap::new();
    for ring in rings {
        if ring.len() < 3 {
            continue;
        }
        let ccw = signed_area(ring) >= 0.0;
        let pts: Vec<Point2> = if ccw {
            ring.clone()
        } else {
            ring.iter().rev().copied().collect()
        };
        for i in 0..pts.len() {
            let a = pts[i];
            let b = pts[(i + 1) % pts.len()];
            if key(a) == key(b) {
                continue; // zero-length edge
            }
            coords.entry(key(a)).or_insert(a);
            coords.entry(key(b)).or_insert(b);
            raw_edges.push((a, b));
        }
    }

    // 1b. Split every edge at any other vertex lying on it, so seams that
    //     overlap only partially still cancel edge-for-edge. This happens
    //     when a relief notch shortens the parent's hinge boundary while
    //     the allowance strip still spans the full hinge — the strip's
    //     edge must be split at the notch corners to cancel against the
    //     surviving hinge fragments.
    let vertices: Vec<Point2> = coords.values().copied().collect();
    let mut edge_count: HashMap<(Key, Key), usize> = HashMap::new();
    for (a, b) in raw_edges {
        let ab = b - a;
        let len2 = ab.x * ab.x + ab.y * ab.y;
        // Interior split vertices with their parameter t ∈ (0, 1) along a→b.
        let mut splits: Vec<(f64, Point2)> = Vec::new();
        for &v in &vertices {
            let kv = key(v);
            if kv == key(a) || kv == key(b) {
                continue;
            }
            let av = v - a;
            let t = (av.x * ab.x + av.y * ab.y) / len2;
            if t <= 0.0 || t >= 1.0 {
                continue;
            }
            let cross = av.x * ab.y - av.y * ab.x;
            // Perpendicular distance below the key grid ⇒ collinear.
            if cross.abs() / len2.sqrt() < 1.0 / KEY_SCALE {
                splits.push((t, v));
            }
        }
        splits.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap());
        let mut prev = a;
        for (_, v) in splits {
            if key(prev) != key(v) {
                *edge_count.entry((key(prev), key(v))).or_insert(0) += 1;
                prev = v;
            }
        }
        if key(prev) != key(b) {
            *edge_count.entry((key(prev), key(b))).or_insert(0) += 1;
        }
    }

    // 2. Cancel opposite directed pairs — those are interior seams where a
    //    strip meets its parent or child panel.
    let keys: Vec<(Key, Key)> = edge_count.keys().copied().collect();
    for k in keys {
        let rev = (k.1, k.0);
        if k.0 >= k.1 {
            continue; // visit each unordered pair once
        }
        let fwd_n = edge_count.get(&k).copied().unwrap_or(0);
        let rev_n = edge_count.get(&rev).copied().unwrap_or(0);
        let cancel = fwd_n.min(rev_n);
        if cancel > 0 {
            *edge_count.get_mut(&k).unwrap() -= cancel;
            *edge_count.get_mut(&rev).unwrap() -= cancel;
        }
    }

    // 3. Chain surviving edges into closed loops. Vertices are almost
    //    always degree 1-in/1-out; where loops touch at a point (e.g. two
    //    bends sharing a body corner), prefer the leftmost turn so the
    //    walk stays on one merged boundary instead of pinching off.
    let mut out_edges: HashMap<Key, Vec<Key>> = HashMap::new();
    let mut remaining = 0usize;
    for (&(a, b), &n) in &edge_count {
        for _ in 0..n {
            out_edges.entry(a).or_default().push(b);
            remaining += 1;
        }
    }

    let mut loops: Vec<Vec<Point2>> = Vec::new();
    while remaining > 0 {
        // Start anywhere with an unused outgoing edge.
        let (&start, _) = match out_edges.iter().find(|(_, v)| !v.is_empty()) {
            Some(kv) => kv,
            None => break,
        };
        let mut walk: Vec<Key> = vec![start];
        let mut prev_dir: Option<Point2> = None;
        let mut cur = start;
        loop {
            let nexts = match out_edges.get_mut(&cur) {
                Some(v) if !v.is_empty() => v,
                _ => break, // dangling — drop this walk
            };
            let next = if nexts.len() == 1 {
                nexts.remove(0)
            } else {
                // Leftmost turn relative to the incoming direction.
                let pd = prev_dir.unwrap_or(Point2::new(1.0, 0.0));
                let cur_pt = coords[&cur];
                let mut best = 0usize;
                let mut best_angle = f64::NEG_INFINITY;
                for (i, cand) in nexts.iter().enumerate() {
                    let d = coords[cand] - cur_pt;
                    // CCW angle from incoming dir to candidate, in (-π, π].
                    let angle = (pd.x * d.y - pd.y * d.x).atan2(pd.x * d.x + pd.y * d.y);
                    if angle > best_angle {
                        best_angle = angle;
                        best = i;
                    }
                }
                nexts.remove(best)
            };
            remaining -= 1;
            prev_dir = Some(Point2::new(
                coords[&next].x - coords[&cur].x,
                coords[&next].y - coords[&cur].y,
            ));
            cur = next;
            if cur == start {
                break;
            }
            walk.push(cur);
        }
        if cur == start && walk.len() >= 3 {
            loops.push(simplify_collinear(walk.iter().map(|k| coords[k]).collect()));
        }
    }
    loops
}

/// Drop interior points of collinear runs (cross product below tolerance
/// relative to segment lengths).
fn simplify_collinear(ring: Vec<Point2>) -> Vec<Point2> {
    let n = ring.len();
    if n < 4 {
        return ring;
    }
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let prev = ring[(i + n - 1) % n];
        let here = ring[i];
        let next = ring[(i + 1) % n];
        let u = here - prev;
        let v = next - here;
        let cross = u.x * v.y - u.y * v.x;
        // Tolerance scales with edge length so long edges don't lose
        // genuinely angled vertices.
        let tol = 1e-9 * u.norm().max(v.norm()).max(1.0);
        if cross.abs() > tol || u.norm() < 1e-12 {
            out.push(here);
        }
    }
    if out.len() >= 3 {
        out
    } else {
        ring
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<Point2> {
        vec![
            Point2::new(x0, y0),
            Point2::new(x1, y0),
            Point2::new(x1, y1),
            Point2::new(x0, y1),
        ]
    }

    #[test]
    fn two_rects_sharing_an_edge_merge_into_one_loop() {
        // Panel [0,10]×[0,10] and strip [0,10]×[-2,0] share edge y=0.
        let rings = vec![rect(0.0, 0.0, 10.0, 10.0), rect(0.0, -2.0, 10.0, 0.0)];
        let loops = merged_silhouette(&rings);
        assert_eq!(loops.len(), 1, "expected single merged loop");
        let loop0 = &loops[0];
        assert_eq!(loop0.len(), 4, "collinear seam corners must be merged");
        let area = signed_area(loop0).abs();
        assert!((area - 120.0).abs() < 1e-9, "area {area}");
    }

    #[test]
    fn panel_strip_child_chain_is_one_loop() {
        // panel / strip / child stacked along -y, exactly touching.
        let rings = vec![
            rect(0.0, 0.0, 10.0, 10.0),
            rect(0.0, -1.5, 10.0, 0.0),
            rect(0.0, -6.5, 10.0, -1.5),
        ];
        let loops = merged_silhouette(&rings);
        assert_eq!(loops.len(), 1);
        let area = signed_area(&loops[0]).abs();
        assert!((area - (100.0 + 15.0 + 50.0)).abs() < 1e-9);
    }

    #[test]
    fn cw_input_rings_are_normalised() {
        let mut a = rect(0.0, 0.0, 10.0, 10.0);
        a.reverse();
        let rings = vec![a, rect(0.0, -2.0, 10.0, 0.0)];
        assert_eq!(merged_silhouette(&rings).len(), 1);
    }

    #[test]
    fn disjoint_rings_stay_separate_loops() {
        let rings = vec![rect(0.0, 0.0, 10.0, 10.0), rect(20.0, 0.0, 30.0, 10.0)];
        let loops = merged_silhouette(&rings);
        assert_eq!(loops.len(), 2);
    }

    #[test]
    fn point_touching_loops_merge_via_leftmost_turn() {
        // Two squares sharing only the corner (10, 10).
        let rings = vec![rect(0.0, 0.0, 10.0, 10.0), rect(10.0, 10.0, 20.0, 20.0)];
        let loops = merged_silhouette(&rings);
        // One merged walk visiting the shared vertex twice is acceptable;
        // two separate loops is also geometrically valid. Either way the
        // total area must be preserved.
        let total: f64 = loops.iter().map(|l| signed_area(l).abs()).sum();
        assert!((total - 200.0).abs() < 1e-9, "area {total}");
    }

    #[test]
    fn empty_input_gives_empty_output() {
        assert!(merged_silhouette(&[]).is_empty());
    }

    #[test]
    fn partial_seam_overlap_splits_and_cancels() {
        // Notched panel: its boundary covers only [0,8] of the y=0 seam
        // (a 2-wide × 1-deep relief notch eats the rest), while the strip
        // below spans the full [0,10]. The strip's seam edge must split at
        // x=8 so the shared run cancels and the notch mouth survives.
        let panel = vec![
            Point2::new(0.0, 0.0),
            Point2::new(8.0, 0.0),  // notch corner on the seam
            Point2::new(8.0, 1.0),  // up the near wall
            Point2::new(10.0, 1.0), // across
            Point2::new(10.0, 10.0),
            Point2::new(0.0, 10.0),
        ];
        let strip = rect(0.0, -2.0, 10.0, 0.0);
        let loops = merged_silhouette(&[panel.clone(), strip]);
        let total: f64 = loops.iter().map(|l| signed_area(l).abs()).sum();
        let expected = (signed_area(&panel).abs()) + 20.0;
        assert!(
            (total - expected).abs() < 1e-9,
            "area {total} != {expected} ({} loops)",
            loops.len()
        );
        // The notch mouth (8,0)→(10,0) must be on the boundary: some loop
        // contains both endpoints.
        let has_mouth = loops.iter().any(|l| {
            l.iter()
                .any(|p| (p.x - 8.0).abs() < 1e-9 && p.y.abs() < 1e-9)
                && l.iter()
                    .any(|p| (p.x - 10.0).abs() < 1e-9 && p.y.abs() < 1e-9)
        });
        assert!(has_mouth, "notch mouth missing from boundary: {loops:?}");
    }
}
