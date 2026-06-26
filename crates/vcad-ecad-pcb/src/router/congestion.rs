//! PathFinder-style negotiated-congestion cost map.
//!
//! The maze router ([`super::maze`]) keeps a hard legality invariant: it never
//! routes copper onto another net. That makes every emitted route DRC-clean,
//! but it also makes whoever grabs a contested corridor *first* keep it — a
//! later net with no alternative is simply left unrouted. Plain rip-up swaps the
//! same two victims round after round without ever discovering that one of them
//! could have detoured.
//!
//! [`Congestion`] adds the missing ingredient from Nayak/Ebeling's PathFinder:
//! a per-cell **history cost** that accumulates over negotiation rounds. After a
//! round leaves a connection unrouted, the cells along the corridor it wanted
//! (and the copper occupying that corridor) get more expensive. On the next
//! round every net re-routes from scratch with these costs folded into the A*
//! step cost, so *flexible* nets are nudged off the contested corridor, freeing
//! it for the net that has no other choice. The legality invariant is untouched
//! — history only adds **cost**, never removes the clearance constraint — so the
//! board stays DRC-clean at every step while the global solution renegotiates.
//!
//! The grid is deliberately coarse (cells ~1 mm, far larger than the routing
//! pitch): history is a *regional* pressure signal, not a per-track reservation.

use vcad_ir::Vec2;

/// Coarse cell size (mm) for the congestion grid. Larger than the routing pitch
/// on purpose — history is a regional pressure field, not a per-track lock.
const CELL: f64 = 1.0;

/// A coarse history-cost field over the board, in routing-cost (≈ mm) units.
///
/// `cost_at` returns the accumulated history cost at a point; the maze A* adds
/// it to each step so routes avoid persistently-contested regions. `add_corridor`
/// raises the cost along a segment band — the negotiation feedback.
#[derive(Debug, Clone)]
pub struct Congestion {
    origin: Vec2,
    cell: f64,
    nx: usize,
    ny: usize,
    /// Accumulated history cost per cell (row-major, `iy * nx + ix`).
    history: Vec<f64>,
}

impl Congestion {
    /// Build a zero-cost congestion field covering the board's bounding box.
    ///
    /// `outline` is the board polygon; an empty/degenerate outline yields a
    /// 1×1 field (`cost_at` is then a no-op), which is fine — a board with no
    /// real extent has nothing to negotiate.
    pub fn new(outline: &[Vec2]) -> Self {
        let (min, max) = if outline.len() >= 3 {
            bbox(outline)
        } else {
            (Vec2::new(0.0, 0.0), Vec2::new(0.0, 0.0))
        };
        // One-cell margin so corridors that hug the board edge still land in-grid.
        let origin = Vec2::new(min.x - CELL, min.y - CELL);
        let span_x = (max.x - min.x).max(0.0) + 2.0 * CELL;
        let span_y = (max.y - min.y).max(0.0) + 2.0 * CELL;
        let nx = (span_x / CELL).ceil() as usize + 1;
        let ny = (span_y / CELL).ceil() as usize + 1;
        Self {
            origin,
            cell: CELL,
            nx: nx.max(1),
            ny: ny.max(1),
            history: vec![0.0; nx.max(1) * ny.max(1)],
        }
    }

    /// True while no history has been deposited — the maze can then skip the
    /// per-step cost lookup entirely (and round one reproduces baseline routing).
    pub fn is_flat(&self) -> bool {
        self.history.iter().all(|&h| h == 0.0)
    }

    fn cell_of(&self, p: Vec2) -> Option<usize> {
        let ix = ((p.x - self.origin.x) / self.cell).floor();
        let iy = ((p.y - self.origin.y) / self.cell).floor();
        if ix < 0.0 || iy < 0.0 {
            return None;
        }
        let (ix, iy) = (ix as usize, iy as usize);
        if ix >= self.nx || iy >= self.ny {
            return None;
        }
        Some(iy * self.nx + ix)
    }

    /// History cost at a world point (0 outside the grid).
    pub fn cost_at(&self, p: Vec2) -> f64 {
        self.cell_of(p).map(|i| self.history[i]).unwrap_or(0.0)
    }

    /// Raise the history cost of every cell within `half_w` of the segment
    /// `a`–`b` by `amount`. This is the negotiation deposit: the corridor a
    /// connection wanted but couldn't get (and the copper sitting in it) becomes
    /// costlier, so the next round's flexible nets route around it.
    pub fn add_corridor(&mut self, a: Vec2, b: Vec2, half_w: f64, amount: f64) {
        if amount <= 0.0 {
            return;
        }
        // Rasterize the inflated segment AABB and test each cell center against
        // the true segment distance — coarse cells make this cheap.
        let lo = Vec2::new(a.x.min(b.x) - half_w, a.y.min(b.y) - half_w);
        let hi = Vec2::new(a.x.max(b.x) + half_w, a.y.max(b.y) + half_w);
        let ix0 = (((lo.x - self.origin.x) / self.cell).floor() as i64).max(0) as usize;
        let iy0 = (((lo.y - self.origin.y) / self.cell).floor() as i64).max(0) as usize;
        let ix1 = (((hi.x - self.origin.x) / self.cell).ceil() as i64).max(0) as usize;
        let iy1 = (((hi.y - self.origin.y) / self.cell).ceil() as i64).max(0) as usize;
        for iy in iy0..=iy1.min(self.ny.saturating_sub(1)) {
            for ix in ix0..=ix1.min(self.nx.saturating_sub(1)) {
                let c = Vec2::new(
                    self.origin.x + (ix as f64 + 0.5) * self.cell,
                    self.origin.y + (iy as f64 + 0.5) * self.cell,
                );
                if point_seg_dist(c, a, b) <= half_w + self.cell {
                    self.history[iy * self.nx + ix] += amount;
                }
            }
        }
    }
}

/// Distance from point `p` to segment `a`–`b`.
fn point_seg_dist(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let abx = b.x - a.x;
    let aby = b.y - a.y;
    let len2 = abx * abx + aby * aby;
    if len2 < 1e-12 {
        return ((p.x - a.x).powi(2) + (p.y - a.y).powi(2)).sqrt();
    }
    let t = (((p.x - a.x) * abx + (p.y - a.y) * aby) / len2).clamp(0.0, 1.0);
    let cx = a.x + t * abx;
    let cy = a.y + t * aby;
    ((p.x - cx).powi(2) + (p.y - cy).powi(2)).sqrt()
}

fn bbox(poly: &[Vec2]) -> (Vec2, Vec2) {
    let mut min = Vec2::new(f64::INFINITY, f64::INFINITY);
    let mut max = Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
    for p in poly {
        min.x = min.x.min(p.x);
        min.y = min.y.min(p.y);
        max.x = max.x.max(p.x);
        max.y = max.y.max(p.y);
    }
    (min, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> Vec<Vec2> {
        vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(36.0, 0.0),
            Vec2::new(36.0, 36.0),
            Vec2::new(0.0, 36.0),
        ]
    }

    #[test]
    fn fresh_field_is_flat_and_zero_cost() {
        let c = Congestion::new(&square());
        assert!(c.is_flat());
        assert_eq!(c.cost_at(Vec2::new(18.0, 18.0)), 0.0);
        // Off-board points read zero, never panic.
        assert_eq!(c.cost_at(Vec2::new(-100.0, -100.0)), 0.0);
        assert_eq!(c.cost_at(Vec2::new(1000.0, 1000.0)), 0.0);
    }

    #[test]
    fn corridor_raises_cost_on_the_band_only() {
        let mut c = Congestion::new(&square());
        // A horizontal corridor across the middle.
        c.add_corridor(Vec2::new(2.0, 18.0), Vec2::new(34.0, 18.0), 0.5, 1.0);
        assert!(!c.is_flat());
        // On the corridor: cost deposited.
        assert!(c.cost_at(Vec2::new(18.0, 18.0)) > 0.0);
        // Well off the corridor (near the top edge): still zero.
        assert_eq!(c.cost_at(Vec2::new(18.0, 34.0)), 0.0);
    }

    #[test]
    fn cost_accumulates_across_rounds() {
        let mut c = Congestion::new(&square());
        let p = Vec2::new(18.0, 18.0);
        c.add_corridor(Vec2::new(2.0, 18.0), Vec2::new(34.0, 18.0), 0.5, 1.0);
        let first = c.cost_at(p);
        c.add_corridor(Vec2::new(2.0, 18.0), Vec2::new(34.0, 18.0), 0.5, 1.0);
        let second = c.cost_at(p);
        assert!(second > first, "history must accumulate: {first} -> {second}");
    }

    #[test]
    fn zero_amount_is_a_noop() {
        let mut c = Congestion::new(&square());
        c.add_corridor(Vec2::new(2.0, 18.0), Vec2::new(34.0, 18.0), 0.5, 0.0);
        assert!(c.is_flat());
    }
}
