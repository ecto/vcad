//! Flat geometry the CAM modules share: how far a point is from a closed
//! loop, which side of it the point is on, and the way out.
//!
//! Every module that has to answer "is the cutter in the metal?" grew its own
//! copy of this. [`operation::contour`](crate::operation) had a private
//! `Wall` over `geo::Polygon` and a private `Edges` for the escape
//! direction — both of which are now this module — and
//! [`verify2d`](crate::verify2d) still has its own:
//!
//! | `verify2d` | here |
//! |---|---|
//! | `point_segment_distance` | [`point_segment_distance`] |
//! | `signed_area` (and `signed_area2`, twice it) | [`signed_area`] |
//! | `clean_loop` | [`clean_loop`] |
//! | `Poly::contains` | [`point_in_loop`] |
//! | `Poly::distance_to_segment` | — (no twin: segment-to-region) |
//!
//! Pointing `verify2d` at these is wave 3's job, and it is not a blind
//! substitution: its `Poly` carries a uniform-grid index over the edges, so
//! its `contains` and its distance queries are accelerated where these walk
//! every segment. The arithmetic agrees; the complexity does not. A `Wall`
//! over a few thousand points is fine for a toolpath, which asks a few
//! thousand times; the oracle asks millions of times and would need the
//! index brought along.
//!
//! Loops are closed implicitly: the last point joins the first, and a
//! repeated closing point is dropped by [`clean_loop`].

use serde::{Deserialize, Serialize};

/// A closed loop of points, implicitly closed from last back to first.
pub type Loop = Vec<[f64; 2]>;

/// Drop a repeated closing point, so winding and reversal are unambiguous.
pub fn clean_loop(points: &[[f64; 2]]) -> Loop {
    let mut points = points.to_vec();
    while points.len() > 1 {
        let (first, last) = (points[0], points[points.len() - 1]);
        if distance(first, last) < 1e-9 {
            points.pop();
        } else {
            break;
        }
    }
    points
}

/// Distance between two points.
pub fn distance(a: [f64; 2], b: [f64; 2]) -> f64 {
    (b[0] - a[0]).hypot(b[1] - a[1])
}

/// The point of segment `a`–`b` nearest `p`.
pub fn nearest_on_segment(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len2 = dx * dx + dy * dy;
    if len2 <= 0.0 {
        return a;
    }
    let t = (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len2).clamp(0.0, 1.0);
    [a[0] + dx * t, a[1] + dy * t]
}

/// Distance from `p` to segment `a`–`b`.
pub fn point_segment_distance(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    distance(p, nearest_on_segment(p, a, b))
}

/// Signed area of the closed loop: positive counter-clockwise.
pub fn signed_area(points: &[[f64; 2]]) -> f64 {
    let n = points.len();
    if n < 3 {
        return 0.0;
    }
    (0..n)
        .map(|k| {
            let (a, b) = (points[k], points[(k + 1) % n]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum::<f64>()
        / 2.0
}

/// Distance from `p` to the loop's boundary.
pub fn distance_to_loop(p: [f64; 2], points: &[[f64; 2]]) -> f64 {
    let n = points.len();
    if n == 0 {
        return f64::INFINITY;
    }
    if n == 1 {
        return distance(p, points[0]);
    }
    (0..n)
        .map(|k| point_segment_distance(p, points[k], points[(k + 1) % n]))
        .fold(f64::INFINITY, f64::min)
}

/// Whether `p` is inside the closed loop, by crossing count. A point exactly
/// on the boundary may answer either way; ask [`distance_to_loop`] when that
/// matters.
pub fn point_in_loop(p: [f64; 2], points: &[[f64; 2]]) -> bool {
    let n = points.len();
    if n < 3 {
        return false;
    }
    let mut inside = false;
    for k in 0..n {
        let (a, b) = (points[k], points[(k + 1) % n]);
        if (a[1] > p[1]) != (b[1] > p[1]) {
            let t = (p[1] - a[1]) / (b[1] - a[1]);
            if p[0] < a[0] + t * (b[0] - a[0]) {
                inside = !inside;
            }
        }
    }
    inside
}

/// Distance from `p` to the loop, positive inside and negative outside.
pub fn signed_distance_to_loop(p: [f64; 2], points: &[[f64; 2]]) -> f64 {
    let d = distance_to_loop(p, points);
    if point_in_loop(p, points) {
        d
    } else {
        -d
    }
}

/// Which side of a contour the cutter is allowed to be on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WasteSide {
    /// The waste is inside the contour: an opening being cut out.
    Inside,
    /// The waste is outside the contour: a part being cut free.
    Outside,
}

/// A contour and the room a point has on the waste side of it.
#[derive(Debug, Clone)]
pub struct Wall {
    ring: Loop,
    side: WasteSide,
}

impl Wall {
    /// Wrap a closed loop, saying which side of it the cutter runs on.
    pub fn new(ring: &[[f64; 2]], side: WasteSide) -> Self {
        Self {
            ring: clean_loop(ring),
            side,
        }
    }

    /// The loop itself.
    pub fn ring(&self) -> &[[f64; 2]] {
        &self.ring
    }

    /// Room between `p` and the contour on the side the cutter is allowed to
    /// be, in mm. Negative means the tool centre has crossed to the part's
    /// side — which is the definition of cutting into the part, so this is
    /// the number every wall-overcut measurement in the crate comes from.
    pub fn clearance(&self, p: [f64; 2]) -> f64 {
        let signed = signed_distance_to_loop(p, &self.ring);
        match self.side {
            WasteSide::Inside => signed,
            WasteSide::Outside => -signed,
        }
    }

    /// Distance to the nearest wall, and the direction that gets away from it
    /// fastest: the sum of the unit vectors away from every feature that is
    /// (nearly) the nearest one.
    ///
    /// Between the two walls of a slot those cancel and the direction comes
    /// back near zero — that is the centre line, and there is nowhere further
    /// to go. Features within a whisker of the nearest one steer as well, so
    /// a corner sends the point out along the bisector instead of into the
    /// other wall.
    pub fn escape(&self, p: [f64; 2]) -> (f64, [f64; 2]) {
        let n = self.ring.len();
        if n < 2 {
            return (f64::INFINITY, [0.0, 0.0]);
        }
        let mut best = f64::INFINITY;
        for k in 0..n {
            let q = nearest_on_segment(p, self.ring[k], self.ring[(k + 1) % n]);
            best = best.min(distance(p, q));
        }
        let tolerance = (best * 0.05).max(1e-6);
        let mut sum = [0.0, 0.0];
        for k in 0..n {
            let q = nearest_on_segment(p, self.ring[k], self.ring[(k + 1) % n]);
            let d = distance(p, q);
            if d <= best + tolerance && d > 1e-12 {
                sum[0] += (p[0] - q[0]) / d;
                sum[1] += (p[1] - q[1]) / d;
            }
        }
        let norm = sum[0].hypot(sum[1]);
        if norm > 1e-9 {
            sum = [sum[0] / norm, sum[1] / norm];
        } else {
            sum = [0.0, 0.0];
        }
        (best, sum)
    }

    /// Walk a point away from the wall until it has `target` mm of room or
    /// the region runs out (the centre line), whichever comes first.
    pub fn march_to_clearance(&self, mut p: [f64; 2], target: f64) -> [f64; 2] {
        for _ in 0..32 {
            let (clearance, dir) = self.escape(p);
            if clearance >= target - 1e-7 {
                break;
            }
            if dir[0].hypot(dir[1]) < 0.3 {
                break; // Equidistant from both walls: this is the centre line.
            }
            let mut step = target - clearance;
            let mut moved = false;
            for _ in 0..4 {
                let q = [p[0] + dir[0] * step, p[1] + dir[1] * step];
                if self.escape(q).0 > clearance + 1e-9 {
                    p = q;
                    moved = true;
                    break;
                }
                step *= 0.5;
            }
            if !moved {
                break;
            }
        }
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(size: f64) -> Loop {
        vec![[0.0, 0.0], [size, 0.0], [size, size], [0.0, size]]
    }

    /// Clearance is measured from the wall, signed by which side the waste is
    /// on: the same point is 2 mm clear cutting a pocket and 2 mm into the
    /// metal cutting the part out.
    #[test]
    fn clearance_takes_its_sign_from_the_waste_side() {
        let s = square(10.0);
        let inside = Wall::new(&s, WasteSide::Inside);
        let outside = Wall::new(&s, WasteSide::Outside);
        assert!((inside.clearance([2.0, 5.0]) - 2.0).abs() < 1e-12);
        assert!((outside.clearance([2.0, 5.0]) + 2.0).abs() < 1e-12);
        assert!((inside.clearance([-1.5, 5.0]) + 1.5).abs() < 1e-12);
        assert!((outside.clearance([-1.5, 5.0]) - 1.5).abs() < 1e-12);
    }

    /// A point in a slot has nowhere to escape to: the two walls cancel, and
    /// that is what tells the centre-line fallback it has arrived.
    #[test]
    fn the_centre_of_a_slot_has_no_escape_direction() {
        // A 2 mm wide, 20 mm long slot.
        let slot = vec![[0.0, 0.0], [20.0, 0.0], [20.0, 2.0], [0.0, 2.0]];
        let wall = Wall::new(&slot, WasteSide::Inside);
        let (clearance, dir) = wall.escape([10.0, 1.0]);
        assert!((clearance - 1.0).abs() < 1e-12, "{clearance}");
        assert!(dir[0].hypot(dir[1]) < 1e-9, "{dir:?}");

        // Off-centre, the escape points at the far wall.
        let (clearance, dir) = wall.escape([10.0, 0.4]);
        assert!((clearance - 0.4).abs() < 1e-12);
        assert!(dir[1] > 0.99, "{dir:?}");

        // Marching for 1 mm of room gets to the centre line and stops there.
        let marched = wall.march_to_clearance([10.0, 0.4], 1.5);
        assert!((marched[1] - 1.0).abs() < 1e-6, "{marched:?}");
    }

    /// Signed area says which way a loop winds, and a repeated closing point
    /// does not change the answer.
    #[test]
    fn winding_and_closing_points() {
        let s = square(4.0);
        assert!((signed_area(&s) - 16.0).abs() < 1e-12);
        let mut reversed = s.clone();
        reversed.reverse();
        assert!((signed_area(&reversed) + 16.0).abs() < 1e-12);

        let mut closed = s.clone();
        closed.push([0.0, 0.0]);
        assert_eq!(clean_loop(&closed), s);
        assert!((signed_area(&clean_loop(&closed)) - 16.0).abs() < 1e-12);
    }

    #[test]
    fn inside_outside_and_distance() {
        let s = square(10.0);
        assert!(point_in_loop([5.0, 5.0], &s));
        assert!(!point_in_loop([15.0, 5.0], &s));
        assert!((distance_to_loop([5.0, 5.0], &s) - 5.0).abs() < 1e-12);
        assert!((signed_distance_to_loop([15.0, 5.0], &s) + 5.0).abs() < 1e-12);
        assert!((point_segment_distance([5.0, 3.0], [0.0, 0.0], [10.0, 0.0]) - 3.0).abs() < 1e-12);
        assert_eq!(
            nearest_on_segment([5.0, 3.0], [0.0, 0.0], [10.0, 0.0]),
            [5.0, 0.0]
        );
    }
}
