#![warn(missing_docs)]

//! Tajima DST embroidery file format read/write for vcad.
//!
//! The DST (Tajima) format is one of the most widely supported embroidery
//! machine formats. It encodes stitch data as 3-byte records with delta
//! displacements in 0.1mm units.
//!
//! # Example
//!
//! ```
//! use vcad_embroidery::{EmbPattern, StitchCommand, StitchGroup, Thread};
//! use vcad_embroidery_dst::{read_dst, write_dst};
//!
//! // Build a simple pattern
//! let mut pattern = EmbPattern::new();
//! pattern.threads.push(Thread::new([237, 23, 31], "Red"));
//! pattern.stitch_groups.push(StitchGroup {
//!     thread_index: 0,
//!     commands: vec![
//!         StitchCommand::MoveTo { x: 0.0, y: 0.0 },
//!         StitchCommand::StitchTo { x: 5.0, y: 0.0 },
//!         StitchCommand::StitchTo { x: 5.0, y: 5.0 },
//!         StitchCommand::End,
//!     ],
//! });
//!
//! // Write to DST bytes and read back
//! let dst_bytes = write_dst(&pattern).unwrap();
//! assert_eq!(dst_bytes.len() % 3, 512 % 3); // header is 512 bytes
//! let decoded = read_dst(&dst_bytes).unwrap();
//! assert_eq!(decoded.stitch_groups.len(), 1);
//! ```

pub mod error;
pub mod header;
pub mod reader;
pub mod writer;

pub use error::{DstError, Result};
pub use reader::read_dst;
pub use writer::write_dst;

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_embroidery::{EmbPattern, StitchCommand, StitchGroup, Thread};

    fn make_simple_pattern() -> EmbPattern {
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
    fn test_roundtrip_single_color() {
        let pattern = make_simple_pattern();
        let data = write_dst(&pattern).unwrap();

        // Must start with 512-byte header
        assert!(data.len() >= 512);

        let decoded = read_dst(&data).unwrap();
        assert_eq!(decoded.stitch_groups.len(), 1);

        // Count actual stitches
        let orig_stitches: usize = pattern
            .stitch_groups
            .iter()
            .flat_map(|g| &g.commands)
            .filter(|c| matches!(c, StitchCommand::StitchTo { .. }))
            .count();
        let decoded_stitches: usize = decoded
            .stitch_groups
            .iter()
            .flat_map(|g| &g.commands)
            .filter(|c| matches!(c, StitchCommand::StitchTo { .. }))
            .count();
        assert_eq!(decoded_stitches, orig_stitches);
    }

    #[test]
    fn test_roundtrip_preserves_positions() {
        let pattern = make_simple_pattern();
        let data = write_dst(&pattern).unwrap();
        let decoded = read_dst(&data).unwrap();

        let positions: Vec<(f64, f64)> = decoded
            .stitch_groups
            .iter()
            .flat_map(|g| &g.commands)
            .filter_map(|c| match c {
                StitchCommand::StitchTo { x, y } => Some((*x, *y)),
                _ => None,
            })
            .collect();

        // 4 StitchTo commands
        assert_eq!(positions.len(), 4);
        // DST resolution is 0.1mm, so positions should be within that tolerance
        assert!((positions[0].0 - 10.0).abs() < 0.1);
        assert!((positions[0].1 - 0.0).abs() < 0.1);
        assert!((positions[1].0 - 10.0).abs() < 0.1);
        assert!((positions[1].1 - 10.0).abs() < 0.1);
        assert!((positions[2].0 - 0.0).abs() < 0.1);
        assert!((positions[2].1 - 10.0).abs() < 0.1);
        assert!((positions[3].0 - 0.0).abs() < 0.1);
        assert!((positions[3].1 - 0.0).abs() < 0.1);
    }

    #[test]
    fn test_multicolor_roundtrip() {
        let mut pattern = EmbPattern::new();
        pattern.threads.push(Thread::new([237, 23, 31], "Red"));
        pattern.threads.push(Thread::new([0, 0, 255], "Blue"));

        pattern.stitch_groups.push(StitchGroup {
            thread_index: 0,
            commands: vec![
                StitchCommand::MoveTo { x: 0.0, y: 0.0 },
                StitchCommand::StitchTo { x: 5.0, y: 0.0 },
                StitchCommand::StitchTo { x: 5.0, y: 5.0 },
            ],
        });
        pattern.stitch_groups.push(StitchGroup {
            thread_index: 1,
            commands: vec![
                StitchCommand::StitchTo { x: 10.0, y: 5.0 },
                StitchCommand::StitchTo { x: 10.0, y: 10.0 },
                StitchCommand::End,
            ],
        });

        let data = write_dst(&pattern).unwrap();
        let decoded = read_dst(&data).unwrap();

        // Should have 2 stitch groups split at the color change
        assert_eq!(decoded.stitch_groups.len(), 2);
        // DST doesn't store colors, but should have 2 default threads
        assert_eq!(decoded.threads.len(), 2);

        // Verify stitch counts per group
        let group0_stitches = decoded.stitch_groups[0]
            .commands
            .iter()
            .filter(|c| matches!(c, StitchCommand::StitchTo { .. }))
            .count();
        let group1_stitches = decoded.stitch_groups[1]
            .commands
            .iter()
            .filter(|c| matches!(c, StitchCommand::StitchTo { .. }))
            .count();
        assert_eq!(group0_stitches, 2);
        assert_eq!(group1_stitches, 2);
    }

    #[test]
    fn test_negative_deltas() {
        let mut pattern = EmbPattern::new();
        pattern.threads.push(Thread::new([0, 0, 0], "Black"));
        pattern.stitch_groups.push(StitchGroup {
            thread_index: 0,
            commands: vec![
                StitchCommand::MoveTo { x: 10.0, y: 10.0 },
                StitchCommand::StitchTo { x: 5.0, y: 5.0 },
                StitchCommand::StitchTo { x: 0.0, y: 0.0 },
                StitchCommand::End,
            ],
        });

        let data = write_dst(&pattern).unwrap();
        let decoded = read_dst(&data).unwrap();

        let positions: Vec<(f64, f64)> = decoded
            .stitch_groups
            .iter()
            .flat_map(|g| &g.commands)
            .filter_map(|c| match c {
                StitchCommand::StitchTo { x, y } => Some((*x, *y)),
                _ => None,
            })
            .collect();

        assert_eq!(positions.len(), 2);
        assert!((positions[0].0 - 5.0).abs() < 0.1);
        assert!((positions[0].1 - 5.0).abs() < 0.1);
        assert!((positions[1].0 - 0.0).abs() < 0.1);
        assert!((positions[1].1 - 0.0).abs() < 0.1);
    }

    #[test]
    fn test_invalid_header_too_short() {
        let data = vec![0u8; 100];
        let err = read_dst(&data).unwrap_err();
        assert!(matches!(err, DstError::InvalidHeader(_)));
    }

    #[test]
    fn test_empty_pattern_error() {
        let pattern = EmbPattern::new();
        let err = write_dst(&pattern).unwrap_err();
        assert!(matches!(err, DstError::EmptyPattern));
    }

    #[test]
    fn test_large_delta_splits_into_jumps() {
        // A 20mm jump = 200 units, exceeds single-record max of 121
        let mut pattern = EmbPattern::new();
        pattern.threads.push(Thread::new([0, 0, 0], "Black"));
        pattern.stitch_groups.push(StitchGroup {
            thread_index: 0,
            commands: vec![
                StitchCommand::MoveTo { x: 0.0, y: 0.0 },
                StitchCommand::StitchTo { x: 20.0, y: 20.0 },
                StitchCommand::End,
            ],
        });

        let data = write_dst(&pattern).unwrap();
        let decoded = read_dst(&data).unwrap();

        // The final stitch position should be correct despite splitting
        let positions: Vec<(f64, f64)> = decoded
            .stitch_groups
            .iter()
            .flat_map(|g| &g.commands)
            .filter_map(|c| match c {
                StitchCommand::StitchTo { x, y } => Some((*x, *y)),
                _ => None,
            })
            .collect();

        assert!(!positions.is_empty());
        let last = positions.last().unwrap();
        assert!((last.0 - 20.0).abs() < 0.1);
        assert!((last.1 - 20.0).abs() < 0.1);
    }

    #[test]
    fn test_header_roundtrip() {
        let pattern = make_simple_pattern();
        let data = write_dst(&pattern).unwrap();
        let header = header::read_header(&data).unwrap();

        assert_eq!(header.stitch_count, 4); // 4 StitchTo commands
        assert_eq!(header.color_changes, 0);
        assert!(header.positive_x > 0);
        assert!(header.positive_y > 0);
    }

    #[test]
    fn test_header_label() {
        let mut pattern = make_simple_pattern();
        pattern.metadata.name = "My Design".to_string();

        let data = write_dst(&pattern).unwrap();
        let header = header::read_header(&data).unwrap();
        assert_eq!(header.label, "My Design");
    }

    #[test]
    fn test_encode_decode_roundtrip_all_deltas() {
        // Test that encode/decode are inverse for all valid deltas
        use crate::reader::read_dst;
        use crate::writer::write_dst;

        for dx in [-121, -81, -27, -9, -3, -1, 0, 1, 3, 9, 27, 81, 121] {
            for dy in [-121, -81, -27, -9, -3, -1, 0, 1, 3, 9, 27, 81, 121] {
                let mm_x = dx as f64 / 10.0;
                let mm_y = dy as f64 / 10.0;

                let mut pattern = EmbPattern::new();
                pattern.threads.push(Thread::new([0, 0, 0], "Black"));
                pattern.stitch_groups.push(StitchGroup {
                    thread_index: 0,
                    commands: vec![
                        StitchCommand::MoveTo { x: 0.0, y: 0.0 },
                        StitchCommand::StitchTo { x: mm_x, y: mm_y },
                        StitchCommand::End,
                    ],
                });

                let data = write_dst(&pattern).unwrap();
                let decoded = read_dst(&data).unwrap();

                let positions: Vec<(f64, f64)> = decoded
                    .stitch_groups
                    .iter()
                    .flat_map(|g| &g.commands)
                    .filter_map(|c| match c {
                        StitchCommand::StitchTo { x, y } => Some((*x, *y)),
                        _ => None,
                    })
                    .collect();

                assert_eq!(positions.len(), 1, "dx={dx}, dy={dy}");
                assert!(
                    (positions[0].0 - mm_x).abs() < 0.1,
                    "dx={dx}: expected {mm_x}, got {}",
                    positions[0].0
                );
                assert!(
                    (positions[0].1 - mm_y).abs() < 0.1,
                    "dy={dy}: expected {mm_y}, got {}",
                    positions[0].1
                );
            }
        }
    }

    #[test]
    fn test_very_large_delta() {
        // A 100mm jump = 1000 units, needs many split records
        let mut pattern = EmbPattern::new();
        pattern.threads.push(Thread::new([0, 0, 0], "Black"));
        pattern.stitch_groups.push(StitchGroup {
            thread_index: 0,
            commands: vec![
                StitchCommand::MoveTo { x: 0.0, y: 0.0 },
                StitchCommand::StitchTo { x: 100.0, y: -50.0 },
                StitchCommand::End,
            ],
        });

        let data = write_dst(&pattern).unwrap();
        let decoded = read_dst(&data).unwrap();

        let positions: Vec<(f64, f64)> = decoded
            .stitch_groups
            .iter()
            .flat_map(|g| &g.commands)
            .filter_map(|c| match c {
                StitchCommand::StitchTo { x, y } => Some((*x, *y)),
                _ => None,
            })
            .collect();

        assert!(!positions.is_empty());
        let last = positions.last().unwrap();
        assert!((last.0 - 100.0).abs() < 0.1);
        assert!((last.1 - (-50.0)).abs() < 0.1);
    }

    #[test]
    fn test_header_end_marker() {
        let pattern = make_simple_pattern();
        let data = write_dst(&pattern).unwrap();
        // Last byte of 512-byte header should be 0x1A
        assert_eq!(data[511], 0x1A);
    }

    #[test]
    fn test_jump_commands_preserved() {
        let mut pattern = EmbPattern::new();
        pattern.threads.push(Thread::new([0, 0, 0], "Black"));
        pattern.stitch_groups.push(StitchGroup {
            thread_index: 0,
            commands: vec![
                StitchCommand::MoveTo { x: 0.0, y: 0.0 },
                StitchCommand::StitchTo { x: 5.0, y: 0.0 },
                StitchCommand::Jump { x: 10.0, y: 10.0 },
                StitchCommand::StitchTo { x: 15.0, y: 10.0 },
                StitchCommand::End,
            ],
        });

        let data = write_dst(&pattern).unwrap();
        let decoded = read_dst(&data).unwrap();

        // Should have jumps in the decoded output
        let jumps: usize = decoded
            .stitch_groups
            .iter()
            .flat_map(|g| &g.commands)
            .filter(|c| matches!(c, StitchCommand::Jump { .. }))
            .count();
        assert!(jumps >= 1);

        // Final stitch position should be correct
        let stitch_positions: Vec<(f64, f64)> = decoded
            .stitch_groups
            .iter()
            .flat_map(|g| &g.commands)
            .filter_map(|c| match c {
                StitchCommand::StitchTo { x, y } => Some((*x, *y)),
                _ => None,
            })
            .collect();
        assert_eq!(stitch_positions.len(), 2);
        assert!((stitch_positions[1].0 - 15.0).abs() < 0.1);
        assert!((stitch_positions[1].1 - 10.0).abs() < 0.1);
    }
}
