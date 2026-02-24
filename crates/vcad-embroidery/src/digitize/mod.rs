//! Stitch generators that convert 2D paths into embroidery stitch commands.
//!
//! This module provides digitization algorithms for common embroidery
//! techniques: running stitch, satin column, tatami fill, and underlay.

pub mod fill;
pub mod running;
pub mod satin;
pub mod underlay;

pub use fill::{fill_stitch, FillParams};
pub use running::{running_stitch, RunningStitchParams};
pub use satin::{satin_stitch, SatinParams};
pub use underlay::{underlay_stitch, UnderlayParams};

/// A simple 2D path in mm coordinates.
#[derive(Debug, Clone)]
pub struct Path2D {
    /// Points in mm coordinates.
    pub points: Vec<(f64, f64)>,
    /// Whether the path forms a closed loop.
    pub closed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StitchCommand;

    /// Helper: count StitchTo commands in a list.
    fn count_stitches(cmds: &[StitchCommand]) -> usize {
        cmds.iter()
            .filter(|c| matches!(c, StitchCommand::StitchTo { .. }))
            .count()
    }

    #[test]
    fn test_running_stitch_line() {
        // 10mm horizontal line at 2.5mm stitch length -> 4 StitchTo commands.
        let path = Path2D {
            points: vec![(0.0, 0.0), (10.0, 0.0)],
            closed: false,
        };
        let params = RunningStitchParams { stitch_length: 2.5 };
        let cmds = running_stitch(&path, &params);

        assert!(matches!(cmds[0], StitchCommand::MoveTo { x, y } if x == 0.0 && y == 0.0));
        assert_eq!(count_stitches(&cmds), 4);

        // Verify the last stitch lands at the end.
        if let StitchCommand::StitchTo { x, .. } = cmds.last().unwrap() {
            assert!((*x - 10.0).abs() < 1e-9);
        } else {
            panic!("last command should be StitchTo");
        }
    }

    #[test]
    fn test_satin_stitch_zigzag() {
        // Straight horizontal path, satin should alternate above/below.
        let path = Path2D {
            points: vec![(0.0, 0.0), (10.0, 0.0)],
            closed: false,
        };
        let params = SatinParams {
            width: 4.0,
            density: 2.0,
            pull_compensation: 0.0,
        };
        let cmds = satin_stitch(&path, &params);

        // Collect y-coordinates of stitch points (skip MoveTo).
        let ys: Vec<f64> = cmds
            .iter()
            .filter_map(|c| match c {
                StitchCommand::StitchTo { y, .. } => Some(*y),
                _ => None,
            })
            .collect();

        // Should have stitches on both sides of the centerline (y=0).
        assert!(ys.iter().any(|&y| y > 0.5), "expected stitches above centerline");
        assert!(ys.iter().any(|&y| y < -0.5), "expected stitches below centerline");

        // Adjacent stitches should alternate sides.
        for pair in ys.windows(2) {
            assert!(
                pair[0] * pair[1] < 0.0,
                "adjacent satin stitches should be on opposite sides"
            );
        }
    }

    #[test]
    fn test_fill_stitch_square() {
        // 10x10mm square fill.
        let region = Path2D {
            points: vec![
                (0.0, 0.0),
                (10.0, 0.0),
                (10.0, 10.0),
                (0.0, 10.0),
            ],
            closed: true,
        };
        let params = FillParams::default();
        let cmds = fill_stitch(&region, &params);

        // Should have many stitches (10mm / 0.4mm spacing = 25 rows).
        assert!(
            count_stitches(&cmds) > 50,
            "expected many stitches for 10x10 fill, got {}",
            count_stitches(&cmds)
        );

        // All stitch coordinates should be within bounds (with small tolerance).
        let eps = 0.1;
        for cmd in &cmds {
            let (x, y) = match cmd {
                StitchCommand::MoveTo { x, y } | StitchCommand::StitchTo { x, y } => (*x, *y),
                _ => continue,
            };
            assert!(
                x >= -eps && x <= 10.0 + eps && y >= -eps && y <= 10.0 + eps,
                "stitch at ({x}, {y}) outside 10x10 bounds"
            );
        }
    }

    #[test]
    fn test_fill_stitch_rotated() {
        // 10x10mm square with 45-degree fill angle. All stitches still inside bounds.
        let region = Path2D {
            points: vec![
                (0.0, 0.0),
                (10.0, 0.0),
                (10.0, 10.0),
                (0.0, 10.0),
            ],
            closed: true,
        };
        let params = FillParams {
            angle: 45.0,
            ..FillParams::default()
        };
        let cmds = fill_stitch(&region, &params);

        assert!(
            count_stitches(&cmds) > 10,
            "expected stitches for rotated fill"
        );

        let eps = 0.5;
        for cmd in &cmds {
            let (x, y) = match cmd {
                StitchCommand::MoveTo { x, y } | StitchCommand::StitchTo { x, y } => (*x, *y),
                _ => continue,
            };
            assert!(
                x >= -eps && x <= 10.0 + eps && y >= -eps && y <= 10.0 + eps,
                "rotated fill stitch at ({x}, {y}) outside bounds"
            );
        }
    }

    #[test]
    fn test_underlay_looser_than_fill() {
        let region = Path2D {
            points: vec![
                (0.0, 0.0),
                (10.0, 0.0),
                (10.0, 10.0),
                (0.0, 10.0),
            ],
            closed: true,
        };

        let fill_cmds = fill_stitch(&region, &FillParams::default());
        let underlay_cmds = underlay_stitch(&region, &UnderlayParams::default());

        let fill_count = count_stitches(&fill_cmds);
        let underlay_count = count_stitches(&underlay_cmds);

        assert!(
            underlay_count < fill_count,
            "underlay ({underlay_count}) should have fewer stitches than fill ({fill_count})"
        );
    }
}
