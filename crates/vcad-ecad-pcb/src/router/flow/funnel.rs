//! Shortest homotopic path through a sleeve of portals (the funnel algorithm,
//! Lee–Preparata / Hershberger–Snoeyink; the "simple stupid funnel"
//! formulation).
//!
//! A route's homotopy word on a layer is its sequence of crossed cuts; each
//! crossing is a *portal* — the free part of the cut the wire may pass
//! through, already in centerline space (obstacles are grown by clearance plus
//! half the track width), so the shortest path through the portals is a legal
//! centerline as far as the triangulated obstacles go. Several wires through
//! one cut get point portals at spaced slots ([`spread_slots`]).

use super::cdt::orient;

/// A portal: the segment a path must cross, seen in the direction of travel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Portal {
    /// Left endpoint.
    pub left: [f64; 2],
    /// Right endpoint.
    pub right: [f64; 2],
}

fn same(a: [f64; 2], b: [f64; 2]) -> bool {
    (a[0] - b[0]).abs() < 1e-12 && (a[1] - b[1]).abs() < 1e-12
}

/// Shortest path from `start` to `end` crossing `portals` in order.
pub fn funnel(start: [f64; 2], end: [f64; 2], portals: &[Portal]) -> Vec<[f64; 2]> {
    let mut ps: Vec<Portal> = Vec::with_capacity(portals.len() + 2);
    ps.push(Portal {
        left: start,
        right: start,
    });
    ps.extend_from_slice(portals);
    ps.push(Portal {
        left: end,
        right: end,
    });
    let mut path = vec![start];
    let mut apex = start;
    let mut left = ps[0].left;
    let mut right = ps[0].right;
    let (mut left_i, mut right_i) = (0usize, 0usize);
    let mut i = 1usize;
    let mut guard = 0usize;
    while i < ps.len() {
        guard += 1;
        if guard > 16 * ps.len() + 64 {
            break;
        }
        let l = ps[i].left;
        let r = ps[i].right;
        // Right side: tighten when r is on or left of apex→right.
        if orient(apex, right, r) >= 0.0 {
            if same(apex, right) || orient(apex, left, r) < 0.0 {
                right = r;
                right_i = i;
            } else {
                // r crossed the left boundary: left becomes the new apex.
                if !same(*path.last().unwrap(), left) {
                    path.push(left);
                }
                apex = left;
                let apex_i = left_i;
                left = apex;
                right = apex;
                right_i = apex_i;
                i = apex_i + 1;
                continue;
            }
        }
        // Left side: tighten when l is on or right of apex→left.
        if orient(apex, left, l) <= 0.0 {
            if same(apex, left) || orient(apex, right, l) > 0.0 {
                left = l;
                left_i = i;
            } else {
                if !same(*path.last().unwrap(), right) {
                    path.push(right);
                }
                apex = right;
                let apex_i = right_i;
                left = apex;
                right = apex;
                left_i = apex_i;
                i = apex_i + 1;
                continue;
            }
        }
        i += 1;
    }
    if !same(*path.last().unwrap(), end) {
        path.push(end);
    }
    path
}

/// Where polyline `path` crosses each portal, as a parameter in `[0, 1]`
/// from the portal's left end (portals are visited in order).
pub fn portal_params(path: &[[f64; 2]], portals: &[Portal]) -> Vec<f64> {
    let mut out = Vec::with_capacity(portals.len());
    let mut seg = 0usize;
    for p in portals {
        let d = [p.right[0] - p.left[0], p.right[1] - p.left[1]];
        let mut best: Option<f64> = None;
        let mut j = seg;
        while j + 1 < path.len() {
            let a = path[j];
            let b = path[j + 1];
            let e = [b[0] - a[0], b[1] - a[1]];
            let den = d[0] * e[1] - d[1] * e[0];
            if den.abs() > 1e-15 {
                let w = [a[0] - p.left[0], a[1] - p.left[1]];
                let t = (w[0] * e[1] - w[1] * e[0]) / den;
                let u = (w[0] * d[1] - w[1] * d[0]) / den;
                if (-1e-6..=1.0 + 1e-6).contains(&t) && (-1e-6..=1.0 + 1e-6).contains(&u) {
                    best = Some(t.clamp(0.0, 1.0));
                    seg = j;
                    break;
                }
            } else if orient(p.left, p.right, a).abs() < 1e-12 {
                // degenerate (point portal or colinear): nearest endpoint
                best = Some(0.5);
                seg = j;
                break;
            }
            j += 1;
        }
        out.push(best.unwrap_or(0.5));
    }
    out
}

/// Spread wires through one free part so neighbours keep `pitch[i]`
/// center-to-center (`pitch[i]` between wire `i` and `i+1`), as close as
/// possible to their desired positions `want` (mm along the part, sorted
/// ascending), inside `[lo, hi]`. Returns the slot positions; when the wires
/// cannot fit, they are packed from `lo` (and overflow `hi`).
pub fn spread_slots(want: &[f64], pitch: &[f64], lo: f64, hi: f64) -> Vec<f64> {
    let n = want.len();
    if n == 0 {
        return Vec::new();
    }
    let need: f64 = pitch.iter().take(n - 1).sum();
    let mut x: Vec<f64> = want.iter().map(|w| w.clamp(lo, hi)).collect();
    if need > hi - lo {
        let mut acc = lo;
        for i in 0..n {
            x[i] = acc;
            if i + 1 < n {
                acc += pitch[i];
            }
        }
        return x;
    }
    // forward: keep spacing, backward: keep within hi, forward again for lo.
    for i in 1..n {
        x[i] = x[i].max(x[i - 1] + pitch[i - 1]);
    }
    if x[n - 1] > hi {
        x[n - 1] = hi;
        for i in (0..n - 1).rev() {
            x[i] = x[i].min(x[i + 1] - pitch[i]);
        }
    }
    if x[0] < lo {
        x[0] = lo;
        for i in 1..n {
            x[i] = x[i].max(x[i - 1] + pitch[i - 1]);
        }
    }
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn straight_when_portals_wide() {
        let ps = [Portal {
            left: [1.0, 5.0],
            right: [1.0, -5.0],
        }];
        let p = funnel([0.0, 0.0], [2.0, 0.0], &ps);
        assert_eq!(p, vec![[0.0, 0.0], [2.0, 0.0]]);
    }

    #[test]
    fn bends_around_corner() {
        // Travelling +x; the portal at x=1 only admits y in [1, 2]: the path
        // must bend at its right (lower) end (1, 1).
        let ps = [Portal {
            left: [1.0, 2.0],
            right: [1.0, 1.0],
        }];
        let p = funnel([0.0, 0.0], [2.0, 0.0], &ps);
        assert_eq!(p, vec![[0.0, 0.0], [1.0, 1.0], [2.0, 0.0]]);
        let t = portal_params(&p, &ps);
        assert!((t[0] - 1.0).abs() < 1e-9);
    }

    #[test]
    fn bends_twice_through_zigzag() {
        // Upper gap then lower gap.
        let ps = [
            Portal {
                left: [1.0, 3.0],
                right: [1.0, 2.0],
            },
            Portal {
                left: [2.0, -2.0],
                right: [2.0, -3.0],
            },
        ];
        let p = funnel([0.0, 0.0], [3.0, 0.0], &ps);
        assert_eq!(p, vec![[0.0, 0.0], [1.0, 2.0], [2.0, -2.0], [3.0, 0.0]]);
    }

    #[test]
    fn slots_keep_pitch_and_bounds() {
        let s = spread_slots(&[0.5, 0.5, 0.5], &[0.2, 0.2], 0.0, 1.0);
        assert!((s[1] - s[0] - 0.2).abs() < 1e-12 && (s[2] - s[1] - 0.2).abs() < 1e-12);
        assert!(s[2] <= 1.0 + 1e-12);
        let s = spread_slots(&[0.95, 1.0], &[0.3], 0.0, 1.0);
        assert!((s[1] - 1.0).abs() < 1e-12 && (s[0] - 0.7).abs() < 1e-12);
    }
}
