//! Length tuning meander generator.
//!
//! Generates serpentine/meander patterns to match trace lengths for
//! timing-critical signals (DDR, high-speed serial, clock distribution).

use vcad_ir::Vec2;

/// Meander pattern style.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeanderStyle {
    /// Trombone-style meanders (U-shaped bends).
    Trombone,
    /// Sawtooth meanders (zigzag).
    Sawtooth,
}

/// Length tuning parameters.
#[derive(Debug, Clone)]
pub struct LengthTuneParams {
    /// Target trace length in mm.
    pub target_length: f64,
    /// Maximum meander amplitude in mm.
    pub max_amplitude: f64,
    /// Meander spacing in mm.
    pub spacing: f64,
    /// Meander pattern style.
    pub style: MeanderStyle,
}

/// A meander segment to insert into a trace.
#[derive(Debug, Clone)]
pub struct MeanderSegment {
    /// Meander waypoints in board coordinates.
    pub points: Vec<Vec2>,
    /// Total added length from this meander.
    pub added_length: f64,
    /// Which segment of the input polyline this replaces.
    pub segment_index: usize,
}

/// Maximum number of amplitude reduction attempts in clearance-checked generation.
const MAX_REDUCTION_ATTEMPTS: u32 = 10;

/// Amplitude reduction factor per attempt (10% reduction each time).
const AMPLITUDE_REDUCTION: f64 = 0.9;

/// Generate meander segments to achieve a target trace length.
///
/// Returns `Some(vec![])` if the path already meets or exceeds the target,
/// `None` if the meanders cannot fit within the max amplitude constraint,
/// or `Some(segments)` with meander waypoints for each modified segment.
pub fn generate_meanders(
    existing_points: &[Vec2],
    params: &LengthTuneParams,
) -> Option<Vec<MeanderSegment>> {
    generate_meanders_inner(existing_points, params, params.max_amplitude)
}

/// Core meander generation with an explicit amplitude cap.
fn generate_meanders_inner(
    existing_points: &[Vec2],
    params: &LengthTuneParams,
    amplitude_cap: f64,
) -> Option<Vec<MeanderSegment>> {
    let current_length = path_length(existing_points);
    let deficit = params.target_length - current_length;

    if deficit <= 0.0 {
        return Some(vec![]);
    }

    // Find candidate segments: those long enough to fit at least one period.
    let mut candidates: Vec<(usize, f64)> = Vec::new();
    let mut total_periods = 0u32;
    for i in 0..existing_points.len().saturating_sub(1) {
        let seg_len = (existing_points[i + 1] - existing_points[i]).length();
        if seg_len >= params.spacing {
            let n = (seg_len / params.spacing).floor() as u32;
            candidates.push((i, seg_len));
            total_periods += n;
        }
    }

    if total_periods == 0 {
        return None;
    }

    // Solve amplitude from deficit and total periods.
    let n = total_periods as f64;
    let s = params.spacing;
    let ideal_amplitude = match params.style {
        MeanderStyle::Trombone => {
            // Each U-bend adds 2*A extra length.
            deficit / (2.0 * n)
        }
        MeanderStyle::Sawtooth => {
            // Each zigzag period adds 2*sqrt((S/2)^2 + A^2) - S.
            let half_hyp = (deficit / n + s) / 2.0;
            let half_s = s / 2.0;
            let a_sq = half_hyp * half_hyp - half_s * half_s;
            if a_sq < 0.0 {
                return None;
            }
            a_sq.sqrt()
        }
    };

    if ideal_amplitude > params.max_amplitude {
        return None;
    }

    let amplitude = ideal_amplitude.min(amplitude_cap);

    // Generate meander waypoints per candidate segment.
    let mut segments = Vec::new();
    for &(seg_idx, seg_len) in &candidates {
        let p0 = existing_points[seg_idx];
        let p1 = existing_points[seg_idx + 1];
        let dir = (p1 - p0).normalize();
        let normal = dir.perp();

        let n_periods = (seg_len / s).floor() as u32;
        let meander_block_len = n_periods as f64 * s;
        let margin = (seg_len - meander_block_len) / 2.0;

        let mut points = Vec::new();
        // Start at segment start.
        points.push(p0);

        let block_start = p0 + dir.scale(margin);

        match params.style {
            MeanderStyle::Trombone => {
                for k in 0..n_periods {
                    let sign = if k % 2 == 0 { 1.0 } else { -1.0 };
                    let base = block_start + dir.scale(k as f64 * s);
                    // Jog perpendicular.
                    points.push(base + normal.scale(sign * amplitude));
                    // Advance along segment.
                    points.push(base + dir.scale(s) + normal.scale(sign * amplitude));
                    // Jog back to baseline.
                    points.push(base + dir.scale(s));
                }
            }
            MeanderStyle::Sawtooth => {
                for k in 0..n_periods {
                    let sign = if k % 2 == 0 { 1.0 } else { -1.0 };
                    let base = block_start + dir.scale(k as f64 * s);
                    // Diagonal to peak at midpoint.
                    points.push(base + dir.scale(s / 2.0) + normal.scale(sign * amplitude));
                    // Diagonal back to baseline.
                    points.push(base + dir.scale(s));
                }
            }
        }

        // End at segment end.
        points.push(p1);

        let added = path_length(&points) - seg_len;
        segments.push(MeanderSegment {
            points,
            added_length: added,
            segment_index: seg_idx,
        });
    }

    Some(segments)
}

/// Compute the minimum distance from a point to a line segment.
pub fn point_to_segment_distance(point: Vec2, seg_start: Vec2, seg_end: Vec2) -> f64 {
    let d = seg_end - seg_start;
    let len_sq = d.dot(d);

    if len_sq < 1e-12 {
        return (point - seg_start).length();
    }

    let t = ((point - seg_start).dot(d) / len_sq).clamp(0.0, 1.0);
    let proj = seg_start + d.scale(t);
    (point - proj).length()
}

/// Check whether all waypoints maintain minimum clearance from obstacles.
fn check_clearances(
    segments: &[MeanderSegment],
    obstacles: &[(Vec2, Vec2, f64)],
    min_clearance: f64,
) -> bool {
    for seg in segments {
        for pt in &seg.points {
            for &(obs_start, obs_end, extra) in obstacles {
                // Per-obstacle requirement: a diff-pair twin demands its GAP
                // (carried in `extra`); everything else the base clearance.
                let req = min_clearance.max(extra);
                if point_to_segment_distance(*pt, obs_start, obs_end) < req {
                    return false;
                }
            }
        }
    }
    true
}

/// Generate meander segments with DRC-aware clearance checking.
///
/// Works like [`generate_meanders`] but checks that every meander waypoint
/// maintains at least `min_clearance` distance from all obstacle segments.
/// If a clearance violation is found, amplitude is reduced by 10% and
/// regeneration is attempted, up to 10 times.
pub fn generate_meanders_checked(
    existing_points: &[Vec2],
    params: &LengthTuneParams,
    min_clearance: f64,
    obstacles: &[(Vec2, Vec2, f64)],
) -> Option<Vec<MeanderSegment>> {
    let mut amplitude_cap = params.max_amplitude;

    for _ in 0..MAX_REDUCTION_ATTEMPTS {
        let segments = generate_meanders_inner(existing_points, params, amplitude_cap)?;

        if obstacles.is_empty() || check_clearances(&segments, obstacles, min_clearance) {
            return Some(segments);
        }

        amplitude_cap *= AMPLITUDE_REDUCTION;
    }

    // Final attempt after all reductions.
    let segments = generate_meanders_inner(existing_points, params, amplitude_cap)?;
    if obstacles.is_empty() || check_clearances(&segments, obstacles, min_clearance) {
        return Some(segments);
    }

    None
}

/// Calculate the total length of a polyline path.
pub fn path_length(points: &[Vec2]) -> f64 {
    if points.len() < 2 {
        return 0.0;
    }
    let mut length = 0.0;
    for i in 1..points.len() {
        length += (points[i] - points[i - 1]).length();
    }
    length
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_path_length() {
        let points = vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(3.0, 0.0),
            Vec2::new(3.0, 4.0),
        ];
        let len = path_length(&points);
        assert!((len - 7.0).abs() < 1e-10);
    }

    #[test]
    fn test_path_length_empty() {
        assert!((path_length(&[]) - 0.0).abs() < f64::EPSILON);
        assert!((path_length(&[Vec2::new(1.0, 2.0)]) - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn generate_meanders_stub() {
        let params = LengthTuneParams {
            target_length: 100.0,
            max_amplitude: 2.0,
            spacing: 1.0,
            style: MeanderStyle::Trombone,
        };
        let points = vec![Vec2::new(0.0, 0.0), Vec2::new(50.0, 0.0)];
        let result = generate_meanders(&points, &params);
        // Now returns Some with meander segments.
        assert!(result.is_some());
        let segs = result.unwrap();
        assert!(!segs.is_empty());
    }

    #[test]
    fn trombone_horizontal() {
        let points = vec![Vec2::new(0.0, 0.0), Vec2::new(50.0, 0.0)];
        let params = LengthTuneParams {
            target_length: 80.0,
            max_amplitude: 5.0,
            spacing: 2.0,
            style: MeanderStyle::Trombone,
        };
        let segs = generate_meanders(&points, &params).unwrap();
        assert_eq!(segs.len(), 1);
        let total_added: f64 = segs.iter().map(|s| s.added_length).sum();
        assert!(
            (total_added - 30.0).abs() < 0.5,
            "added_length={total_added}"
        );
    }

    #[test]
    fn sawtooth_horizontal() {
        let points = vec![Vec2::new(0.0, 0.0), Vec2::new(50.0, 0.0)];
        let params = LengthTuneParams {
            target_length: 70.0,
            max_amplitude: 5.0,
            spacing: 2.0,
            style: MeanderStyle::Sawtooth,
        };
        let segs = generate_meanders(&points, &params).unwrap();
        assert_eq!(segs.len(), 1);
        let total_added: f64 = segs.iter().map(|s| s.added_length).sum();
        let deficit = 70.0 - 50.0;
        assert!(
            (total_added - deficit).abs() < 0.5,
            "added={total_added}, deficit={deficit}"
        );
    }

    #[test]
    fn angled_trace() {
        // 45° trace: (0,0) → (50,50), length ≈ 70.71
        let points = vec![Vec2::new(0.0, 0.0), Vec2::new(50.0, 50.0)];
        let params = LengthTuneParams {
            target_length: 100.0,
            max_amplitude: 5.0,
            spacing: 2.0,
            style: MeanderStyle::Trombone,
        };
        let segs = generate_meanders(&points, &params).unwrap();
        assert_eq!(segs.len(), 1);
        // Verify meander points jog perpendicular to the 45° direction.
        let dir = Vec2::new(1.0, 1.0).normalize();
        let normal = dir.perp();
        // Check a meander point that should be offset from the baseline.
        let p = &segs[0].points[1]; // first jog point
        let from_start = *p - points[0];
        let perp_component = from_start.dot(normal);
        assert!(
            perp_component.abs() > 0.1,
            "meander should jog perpendicular"
        );
    }

    #[test]
    fn multi_segment() {
        // L-shaped path: two segments, each long enough for meanders.
        let points = vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(30.0, 0.0),
            Vec2::new(30.0, 30.0),
        ];
        let params = LengthTuneParams {
            target_length: 90.0,
            max_amplitude: 5.0,
            spacing: 2.0,
            style: MeanderStyle::Trombone,
        };
        let segs = generate_meanders(&points, &params).unwrap();
        assert_eq!(segs.len(), 2, "should produce meanders on both segments");
        assert_eq!(segs[0].segment_index, 0);
        assert_eq!(segs[1].segment_index, 1);
    }

    #[test]
    fn zero_deficit() {
        let points = vec![Vec2::new(0.0, 0.0), Vec2::new(50.0, 0.0)];
        let params = LengthTuneParams {
            target_length: 50.0,
            max_amplitude: 2.0,
            spacing: 1.0,
            style: MeanderStyle::Trombone,
        };
        let result = generate_meanders(&points, &params).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn deficit_exceeds_max() {
        let points = vec![Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0)];
        let params = LengthTuneParams {
            target_length: 1000.0,
            max_amplitude: 0.5,
            spacing: 1.0,
            style: MeanderStyle::Trombone,
        };
        assert!(generate_meanders(&points, &params).is_none());
    }

    #[test]
    fn segment_too_short() {
        let points = vec![Vec2::new(0.0, 0.0), Vec2::new(0.5, 0.0)];
        let params = LengthTuneParams {
            target_length: 10.0,
            max_amplitude: 2.0,
            spacing: 1.0,
            style: MeanderStyle::Trombone,
        };
        // Segment is shorter than spacing, no candidates → None.
        assert!(generate_meanders(&points, &params).is_none());
    }

    #[test]
    fn length_matches_target() {
        let points = vec![Vec2::new(0.0, 0.0), Vec2::new(50.0, 0.0)];
        let target = 80.0;
        let params = LengthTuneParams {
            target_length: target,
            max_amplitude: 5.0,
            spacing: 2.0,
            style: MeanderStyle::Trombone,
        };
        let segs = generate_meanders(&points, &params).unwrap();
        let original = path_length(&points);
        let total_added: f64 = segs.iter().map(|s| s.added_length).sum();
        let achieved = original + total_added;
        assert!(
            (achieved - target).abs() < 1.0,
            "achieved={achieved}, target={target}"
        );
    }

    #[test]
    fn endpoints_preserved() {
        let points = vec![Vec2::new(5.0, 10.0), Vec2::new(55.0, 10.0)];
        let params = LengthTuneParams {
            target_length: 80.0,
            max_amplitude: 3.0,
            spacing: 2.0,
            style: MeanderStyle::Sawtooth,
        };
        let segs = generate_meanders(&points, &params).unwrap();
        for seg in &segs {
            let first = seg.points.first().unwrap();
            let last = seg.points.last().unwrap();
            let p0 = points[seg.segment_index];
            let p1 = points[seg.segment_index + 1];
            assert!(
                (first.x - p0.x).abs() < 1e-10 && (first.y - p0.y).abs() < 1e-10,
                "first point should match segment start"
            );
            assert!(
                (last.x - p1.x).abs() < 1e-10 && (last.y - p1.y).abs() < 1e-10,
                "last point should match segment end"
            );
        }
    }

    #[test]
    fn point_to_segment_distance_basic() {
        // Point above midpoint of horizontal segment.
        let dist = point_to_segment_distance(
            Vec2::new(1.0, 1.0),
            Vec2::new(0.0, 0.0),
            Vec2::new(2.0, 0.0),
        );
        assert!((dist - 1.0).abs() < 1e-10);

        // Point at endpoint.
        let dist = point_to_segment_distance(
            Vec2::new(0.0, 0.0),
            Vec2::new(0.0, 0.0),
            Vec2::new(2.0, 0.0),
        );
        assert!(dist.abs() < 1e-10);

        // Point beyond segment end.
        let dist = point_to_segment_distance(
            Vec2::new(3.0, 0.0),
            Vec2::new(0.0, 0.0),
            Vec2::new(2.0, 0.0),
        );
        assert!((dist - 1.0).abs() < 1e-10);

        // Degenerate (zero-length) segment.
        let dist = point_to_segment_distance(
            Vec2::new(3.0, 4.0),
            Vec2::new(0.0, 0.0),
            Vec2::new(0.0, 0.0),
        );
        assert!((dist - 5.0).abs() < 1e-10);
    }

    #[test]
    fn clearance_no_obstacles() {
        let params = LengthTuneParams {
            target_length: 80.0,
            max_amplitude: 5.0,
            spacing: 2.0,
            style: MeanderStyle::Trombone,
        };
        let points = vec![Vec2::new(0.0, 0.0), Vec2::new(50.0, 0.0)];

        let unchecked = generate_meanders(&points, &params).unwrap();
        let checked = generate_meanders_checked(&points, &params, 0.2, &[]).unwrap();

        assert_eq!(unchecked.len(), checked.len());
        for (u, c) in unchecked.iter().zip(checked.iter()) {
            assert_eq!(u.points.len(), c.points.len());
            assert!((u.added_length - c.added_length).abs() < 1e-10);
        }
    }

    #[test]
    fn clearance_reduces_amplitude() {
        // Ideal amplitude = 30/(2*5) = 3.0mm. Obstacle at y=2.6 with 0.5mm
        // clearance means peaks at y=3.0 violate (only 0.4mm gap).
        let params = LengthTuneParams {
            target_length: 80.0,
            max_amplitude: 3.0,
            spacing: 10.0,
            style: MeanderStyle::Trombone,
        };
        let points = vec![Vec2::new(0.0, 0.0), Vec2::new(50.0, 0.0)];
        let obstacles = vec![(Vec2::new(-5.0, 2.6), Vec2::new(55.0, 2.6), 0.0)];

        // Full amplitude should violate.
        let full = generate_meanders(&points, &params).unwrap();
        assert!(!check_clearances(&full, &obstacles, 0.5));

        // Checked version should find a reduced-amplitude solution.
        let result = generate_meanders_checked(&points, &params, 0.5, &obstacles);
        assert!(result.is_some());

        let segs = result.unwrap();
        let max_y = segs
            .iter()
            .flat_map(|s| s.points.iter())
            .map(|p| p.y.abs())
            .fold(0.0_f64, f64::max);
        assert!(max_y < 3.0, "amplitude should be reduced: max_y={max_y}");
    }

    #[test]
    fn clearance_impossible() {
        let params = LengthTuneParams {
            target_length: 80.0,
            max_amplitude: 5.0,
            spacing: 5.0,
            style: MeanderStyle::Trombone,
        };
        let points = vec![Vec2::new(0.0, 0.0), Vec2::new(50.0, 0.0)];
        // Obstacle on the trace itself — 1mm clearance is impossible.
        let obstacles = vec![(Vec2::new(0.0, 0.0), Vec2::new(50.0, 0.0), 0.0)];

        assert!(generate_meanders_checked(&points, &params, 1.0, &obstacles).is_none());
    }
}
