#![warn(missing_docs)]

//! Embroidery pattern types and utilities for vcad.
//!
//! This crate provides the core data model for embroidery patterns,
//! including stitch commands, thread colors, hoop sizes, and pattern
//! statistics. It is format-agnostic — file format support lives in
//! companion crates like `vcad-embroidery-pes`.
//!
//! # Example
//!
//! ```
//! use vcad_embroidery::{EmbPattern, StitchCommand, StitchGroup, Thread};
//!
//! let mut pattern = EmbPattern::new();
//! pattern.threads.push(Thread::new([237, 23, 31], "Red"));
//! pattern.stitch_groups.push(StitchGroup {
//!     thread_index: 0,
//!     commands: vec![
//!         StitchCommand::MoveTo { x: 0.0, y: 0.0 },
//!         StitchCommand::StitchTo { x: 10.0, y: 0.0 },
//!         StitchCommand::StitchTo { x: 10.0, y: 10.0 },
//!         StitchCommand::End,
//!     ],
//! });
//!
//! let stats = pattern.stats();
//! assert_eq!(stats.stitch_count, 2);
//! ```

pub mod digitize;
pub mod error;
pub mod hoop;
pub mod pattern;
pub mod stats;
pub mod stitch;
pub mod thread;

pub use digitize::{
    fill_stitch, fill_stitch_multi, running_stitch, satin_stitch, underlay_stitch, FillParams,
    Path2D, RunningStitchParams, SatinParams, UnderlayParams,
};
pub use error::{EmbroideryError, Result};
pub use hoop::{brother_pe800, brother_se1900, Hoop, MachineProfile};
pub use pattern::{EmbPattern, PatternMetadata};
pub use stats::{compute_stats, PatternStats};
pub use stitch::{StitchCommand, StitchGroup};
pub use thread::{brother_palette, Thread};

#[cfg(test)]
mod tests {
    use super::*;

    fn make_square_pattern() -> EmbPattern {
        let mut pattern = EmbPattern::new();
        pattern.threads.push(Thread::new([237, 23, 31], "Red"));
        pattern.stitch_groups.push(StitchGroup {
            thread_index: 0,
            commands: vec![
                StitchCommand::MoveTo { x: 0.0, y: 0.0 },
                StitchCommand::StitchTo { x: 10.0, y: 0.0 },
                StitchCommand::StitchTo { x: 10.0, y: 10.0 },
                StitchCommand::StitchTo { x: 0.0, y: 10.0 },
                StitchCommand::StitchTo { x: 0.0, y: 0.0 },
                StitchCommand::End,
            ],
        });
        pattern
    }

    #[test]
    fn test_pattern_stats() {
        let pattern = make_square_pattern();
        let stats = pattern.stats();
        assert_eq!(stats.stitch_count, 4);
        assert_eq!(stats.color_count, 1);
        assert_eq!(stats.color_changes, 0);
        assert!((stats.width - 10.0).abs() < 1e-9);
        assert!((stats.height - 10.0).abs() < 1e-9);
        // 4 sides of 10mm = 40mm thread length
        assert!((stats.thread_length - 40.0).abs() < 1e-9);
    }

    #[test]
    fn test_pattern_validate() {
        let pattern = make_square_pattern();
        assert!(pattern.validate().is_ok());
    }

    #[test]
    fn test_pattern_validate_empty() {
        let pattern = EmbPattern::new();
        assert!(matches!(
            pattern.validate(),
            Err(EmbroideryError::EmptyPattern)
        ));
    }

    #[test]
    fn test_pattern_validate_bad_thread_index() {
        let mut pattern = EmbPattern::new();
        pattern.threads.push(Thread::new([0, 0, 0], "Black"));
        pattern.stitch_groups.push(StitchGroup {
            thread_index: 5, // out of bounds
            commands: vec![StitchCommand::StitchTo { x: 1.0, y: 1.0 }],
        });
        assert!(matches!(
            pattern.validate(),
            Err(EmbroideryError::InvalidThreadIndex { index: 5, count: 1 })
        ));
    }

    #[test]
    fn test_stitch_group_count() {
        let group = StitchGroup {
            thread_index: 0,
            commands: vec![
                StitchCommand::MoveTo { x: 0.0, y: 0.0 },
                StitchCommand::StitchTo { x: 1.0, y: 0.0 },
                StitchCommand::Jump { x: 5.0, y: 5.0 },
                StitchCommand::StitchTo { x: 6.0, y: 5.0 },
                StitchCommand::Trim,
            ],
        };
        assert_eq!(group.stitch_count(), 2);
    }

    #[test]
    fn test_hoop_contains() {
        let hoop = Hoop {
            name: "4x4".into(),
            width: 100.0,
            height: 100.0,
        };
        assert!(hoop.contains((0.0, 0.0, 50.0, 50.0)));
        assert!(hoop.contains((0.0, 0.0, 100.0, 100.0)));
        assert!(!hoop.contains((0.0, 0.0, 150.0, 100.0)));
    }

    #[test]
    fn test_fits_hoop() {
        let pattern = make_square_pattern();
        let small_hoop = Hoop {
            name: "tiny".into(),
            width: 5.0,
            height: 5.0,
        };
        let big_hoop = Hoop {
            name: "big".into(),
            width: 200.0,
            height: 200.0,
        };
        assert!(!pattern.fits_hoop(&small_hoop));
        assert!(pattern.fits_hoop(&big_hoop));
    }

    #[test]
    fn test_brother_palette() {
        let palette = brother_palette();
        assert_eq!(palette.len(), 65);
        // Index 20 = Black
        assert_eq!(palette[20].color, [0, 0, 0]);
        // Index 29 = White
        assert_eq!(palette[29].color, [240, 240, 240]);
    }

    #[test]
    fn test_thread_new() {
        let t = Thread::new([255, 0, 0], "Red");
        assert_eq!(t.color, [255, 0, 0]);
        assert_eq!(t.name, "Red");
        assert!(t.brand.is_none());
    }

    #[test]
    fn test_estimated_time() {
        let pattern = make_square_pattern();
        let stats = pattern.stats();
        // 4 stitches at 800 spm = 0.3 seconds
        assert!(stats.estimated_time_seconds > 0.0);
        assert!((stats.estimated_time_seconds - 4.0 / 800.0 * 60.0).abs() < 1e-9);
    }

    #[test]
    fn test_multicolor_pattern() {
        let mut pattern = EmbPattern::new();
        pattern.threads.push(Thread::new([255, 0, 0], "Red"));
        pattern.threads.push(Thread::new([0, 0, 255], "Blue"));

        pattern.stitch_groups.push(StitchGroup {
            thread_index: 0,
            commands: vec![
                StitchCommand::MoveTo { x: 0.0, y: 0.0 },
                StitchCommand::StitchTo { x: 5.0, y: 0.0 },
                StitchCommand::Trim,
            ],
        });
        pattern.stitch_groups.push(StitchGroup {
            thread_index: 1,
            commands: vec![
                StitchCommand::ColorChange { index: 1 },
                StitchCommand::MoveTo { x: 0.0, y: 5.0 },
                StitchCommand::StitchTo { x: 5.0, y: 5.0 },
                StitchCommand::End,
            ],
        });

        assert!(pattern.validate().is_ok());
        let stats = pattern.stats();
        assert_eq!(stats.stitch_count, 2);
        assert_eq!(stats.color_changes, 1);
        assert_eq!(stats.color_count, 2);
        assert_eq!(stats.trim_count, 1);
    }
}
