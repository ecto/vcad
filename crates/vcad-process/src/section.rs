//! Cross-section interval extraction.
//!
//! Intersecting a film's plan-view footprint with a [`CutLine`] yields a
//! set of 1D intervals along the cut — the spans where the film exists.
//! Each `(interval × film z-range)` later becomes one rectangle of the
//! textbook cross-section.

use geo::{MultiPolygon, Polygon};

use crate::recipe::{Axis, CutLine};

/// Merge tolerance: crossings closer than this (µm) collapse.
const EPS: f64 = 1e-9;

/// Where an edge crosses the scan line, using the half-open rule
/// (`lo <= pos < hi`) so vertices shared by two edges count once.
fn edge_crossing(axis: Axis, pos: f64, a: geo::Coord<f64>, b: geo::Coord<f64>) -> Option<f64> {
    // `across` is the coordinate the cut is fixed in; `along` is the
    // coordinate we report. An Axis::X cut fixes y and reports x.
    let (a_across, a_along, b_across, b_along) = match axis {
        Axis::X => (a.y, a.x, b.y, b.x),
        Axis::Y => (a.x, a.y, b.x, b.y),
    };
    let crosses = (a_across <= pos && pos < b_across) || (b_across <= pos && pos < a_across);
    crosses.then(|| a_along + (pos - a_across) * (b_along - a_along) / (b_across - a_across))
}

/// Intervals of a single polygon (with holes) along the cut, via even-odd
/// pairing of all ring crossings.
fn polygon_intervals(polygon: &Polygon<f64>, cut: &CutLine) -> Vec<[f64; 2]> {
    let mut crossings: Vec<f64> = Vec::new();
    let rings = std::iter::once(polygon.exterior()).chain(polygon.interiors());
    for ring in rings {
        for line in ring.lines() {
            if let Some(x) = edge_crossing(cut.axis, cut.position_um, line.start, line.end) {
                crossings.push(x);
            }
        }
    }
    crossings.sort_by(f64::total_cmp);
    crossings
        .chunks_exact(2)
        .filter(|pair| pair[1] - pair[0] > EPS)
        .map(|pair| [pair[0], pair[1]])
        .collect()
}

/// Union a set of intervals (sorted-merge).
fn merge_intervals(mut intervals: Vec<[f64; 2]>) -> Vec<[f64; 2]> {
    intervals.sort_by(|a, b| a[0].total_cmp(&b[0]));
    let mut merged: Vec<[f64; 2]> = Vec::with_capacity(intervals.len());
    for iv in intervals {
        match merged.last_mut() {
            Some(last) if iv[0] <= last[1] + EPS => last[1] = last[1].max(iv[1]),
            _ => merged.push(iv),
        }
    }
    merged
}

/// Intersect `footprint` with the cut line and return the surviving
/// intervals along the cut, clipped to `cut.span`, sorted and disjoint.
///
/// Etched-away regions simply produce no interval — gaps in the returned
/// list are gaps in the film.
pub fn footprint_intervals(footprint: &MultiPolygon<f64>, cut: &CutLine) -> Vec<[f64; 2]> {
    let (lo, hi) = (cut.span[0].min(cut.span[1]), cut.span[0].max(cut.span[1]));
    let all: Vec<[f64; 2]> = footprint
        .iter()
        .flat_map(|p| polygon_intervals(p, cut))
        .filter_map(|iv| {
            let (a, b) = (iv[0].max(lo), iv[1].min(hi));
            (b - a > EPS).then_some([a, b])
        })
        .collect();
    merge_intervals(all)
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::{polygon, BooleanOps, MultiPolygon};

    fn cut_x(position_um: f64, span: [f64; 2]) -> CutLine {
        CutLine {
            axis: Axis::X,
            position_um,
            span,
        }
    }

    #[test]
    fn square_crossing_the_cut_yields_exactly_its_interval() {
        let square: MultiPolygon<f64> = MultiPolygon::new(vec![polygon![
            (x: 2.0, y: 1.0),
            (x: 7.0, y: 1.0),
            (x: 7.0, y: 4.0),
            (x: 2.0, y: 4.0),
        ]]);
        let intervals = footprint_intervals(&square, &cut_x(2.5, [0.0, 10.0]));
        assert_eq!(intervals, vec![[2.0, 7.0]]);
        // A cut that misses the square entirely yields nothing.
        assert!(footprint_intervals(&square, &cut_x(5.0, [0.0, 10.0])).is_empty());
        // The span clips the interval.
        let clipped = footprint_intervals(&square, &cut_x(2.5, [3.0, 5.0]));
        assert_eq!(clipped, vec![[3.0, 5.0]]);
    }

    #[test]
    fn etch_subtraction_removes_the_interval() {
        // Film across x = 0..10, etched (subtracted) over x = 4..6:
        // the cut must report two intervals with a real gap.
        let film: MultiPolygon<f64> = MultiPolygon::new(vec![polygon![
            (x: 0.0, y: 0.0),
            (x: 10.0, y: 0.0),
            (x: 10.0, y: 2.0),
            (x: 0.0, y: 2.0),
        ]]);
        let etched = film.difference(&MultiPolygon::new(vec![polygon![
            (x: 4.0, y: -1.0),
            (x: 6.0, y: -1.0),
            (x: 6.0, y: 3.0),
            (x: 4.0, y: 3.0),
        ]]));
        let intervals = footprint_intervals(&etched, &cut_x(1.0, [0.0, 10.0]));
        assert_eq!(intervals.len(), 2);
        assert!((intervals[0][0] - 0.0).abs() < 1e-9 && (intervals[0][1] - 4.0).abs() < 1e-9);
        assert!((intervals[1][0] - 6.0).abs() < 1e-9 && (intervals[1][1] - 10.0).abs() < 1e-9);
    }

    #[test]
    fn holes_produce_gaps() {
        let with_hole: MultiPolygon<f64> = MultiPolygon::new(vec![Polygon::new(
            polygon![
                (x: 0.0, y: 0.0),
                (x: 8.0, y: 0.0),
                (x: 8.0, y: 8.0),
                (x: 0.0, y: 8.0),
            ]
            .exterior()
            .clone(),
            vec![polygon![
                (x: 3.0, y: 3.0),
                (x: 5.0, y: 3.0),
                (x: 5.0, y: 5.0),
                (x: 3.0, y: 5.0),
            ]
            .exterior()
            .clone()],
        )]);
        let intervals = footprint_intervals(&with_hole, &cut_x(4.0, [0.0, 8.0]));
        assert_eq!(intervals, vec![[0.0, 3.0], [5.0, 8.0]]);
    }

    #[test]
    fn vertical_cut_uses_y_axis() {
        let square: MultiPolygon<f64> = MultiPolygon::new(vec![polygon![
            (x: 1.0, y: 2.0),
            (x: 3.0, y: 2.0),
            (x: 3.0, y: 9.0),
            (x: 1.0, y: 9.0),
        ]]);
        let cut = CutLine {
            axis: Axis::Y,
            position_um: 2.0,
            span: [0.0, 10.0],
        };
        assert_eq!(footprint_intervals(&square, &cut), vec![[2.0, 9.0]]);
    }

    #[test]
    fn overlapping_multipolygon_parts_merge() {
        // Two disjoint parts and one adjacent — merged output is disjoint
        // and sorted.
        let mp = MultiPolygon::new(vec![
            polygon![(x: 0.0, y: 0.0), (x: 2.0, y: 0.0), (x: 2.0, y: 2.0), (x: 0.0, y: 2.0)],
            polygon![(x: 2.0, y: 0.0), (x: 4.0, y: 0.0), (x: 4.0, y: 2.0), (x: 2.0, y: 2.0)],
            polygon![(x: 6.0, y: 0.0), (x: 7.0, y: 0.0), (x: 7.0, y: 2.0), (x: 6.0, y: 2.0)],
        ]);
        let intervals = footprint_intervals(&mp, &cut_x(1.0, [0.0, 10.0]));
        assert_eq!(intervals, vec![[0.0, 4.0], [6.0, 7.0]]);
    }
}
