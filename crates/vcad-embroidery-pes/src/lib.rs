#![warn(missing_docs)]

//! PES/PEC embroidery file format read/write.
//!
//! Supports reading and writing Brother PES files (version 1, `#PES0001`),
//! which is the most widely supported embroidery format for Brother machines.
//!
//! # Example
//!
//! ```
//! use vcad_embroidery::{EmbPattern, StitchCommand, StitchGroup, Thread};
//! use vcad_embroidery_pes::{read_pes, write_pes};
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
//! // Write to PES bytes
//! let pes_bytes = write_pes(&pattern).unwrap();
//! assert_eq!(&pes_bytes[0..8], b"#PES0001");
//! ```

pub mod error;
pub mod palette;
pub mod pec;
pub mod reader;
pub mod writer;

pub use error::{PesError, Result};
pub use reader::read_pes;
pub use writer::write_pes;

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
    fn test_write_pes_header() {
        let pattern = make_simple_pattern();
        let data = write_pes(&pattern).unwrap();
        assert_eq!(&data[0..8], b"#PES0001");
    }

    #[test]
    fn test_write_pes_pec_offset() {
        let pattern = make_simple_pattern();
        let data = write_pes(&pattern).unwrap();
        let pec_offset = u32::from_le_bytes([data[8], data[9], data[10], data[11]]) as usize;
        // PEC offset should be within bounds
        assert!(pec_offset > 12);
        assert!(pec_offset < data.len());
    }

    #[test]
    fn test_roundtrip_single_color() {
        let pattern = make_simple_pattern();
        let data = write_pes(&pattern).unwrap();
        let decoded = read_pes(&data).unwrap();

        // Same number of stitch groups
        assert_eq!(decoded.stitch_groups.len(), 1);

        // Count actual stitches (StitchTo only)
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

        // MoveTo gets encoded as a jump, so the decoded stitch count should
        // match the original StitchTo count (jumps are separate).
        assert_eq!(decoded_stitches, orig_stitches);
    }

    #[test]
    fn test_roundtrip_preserves_positions() {
        let pattern = make_simple_pattern();
        let data = write_pes(&pattern).unwrap();
        let decoded = read_pes(&data).unwrap();

        // Extract all stitch positions from the decoded pattern
        let positions: Vec<(f64, f64)> = decoded
            .stitch_groups
            .iter()
            .flat_map(|g| &g.commands)
            .filter_map(|c| match c {
                StitchCommand::StitchTo { x, y } => Some((*x, *y)),
                _ => None,
            })
            .collect();

        // Verify the 4 stitch positions are approximately correct
        // (PEC encoding has 0.1mm resolution)
        assert_eq!(positions.len(), 4);
        assert!((positions[0].0 - 10.0).abs() < 0.15);
        assert!((positions[0].1 - 0.0).abs() < 0.15);
        assert!((positions[1].0 - 10.0).abs() < 0.15);
        assert!((positions[1].1 - 10.0).abs() < 0.15);
        assert!((positions[2].0 - 0.0).abs() < 0.15);
        assert!((positions[2].1 - 10.0).abs() < 0.15);
        assert!((positions[3].0 - 0.0).abs() < 0.15);
        assert!((positions[3].1 - 0.0).abs() < 0.15);
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
            ],
        });
        pattern.stitch_groups.push(StitchGroup {
            thread_index: 1,
            commands: vec![
                StitchCommand::StitchTo { x: 5.0, y: 5.0 },
                StitchCommand::End,
            ],
        });

        let data = write_pes(&pattern).unwrap();
        let decoded = read_pes(&data).unwrap();

        assert_eq!(decoded.stitch_groups.len(), 2);
        assert_eq!(decoded.threads.len(), 2);
    }

    #[test]
    fn test_pec_short_form_encoding() {
        // Small deltas should use short form (1 byte per axis)
        let mut pattern = EmbPattern::new();
        pattern.threads.push(Thread::new([0, 0, 0], "Black"));
        pattern.stitch_groups.push(StitchGroup {
            thread_index: 0,
            commands: vec![
                StitchCommand::MoveTo { x: 0.0, y: 0.0 },
                StitchCommand::StitchTo { x: 1.0, y: 1.0 }, // dx=10 units, fits short form
                StitchCommand::StitchTo { x: 2.0, y: 2.0 },
                StitchCommand::End,
            ],
        });

        let data = write_pes(&pattern).unwrap();
        let decoded = read_pes(&data).unwrap();

        let stitch_count: usize = decoded
            .stitch_groups
            .iter()
            .flat_map(|g| &g.commands)
            .filter(|c| matches!(c, StitchCommand::StitchTo { .. }))
            .count();
        assert_eq!(stitch_count, 2);
    }

    #[test]
    fn test_pec_long_form_encoding() {
        // Large deltas should use long form (2 bytes per axis)
        let mut pattern = EmbPattern::new();
        pattern.threads.push(Thread::new([0, 0, 0], "Black"));
        pattern.stitch_groups.push(StitchGroup {
            thread_index: 0,
            commands: vec![
                StitchCommand::MoveTo { x: 0.0, y: 0.0 },
                StitchCommand::StitchTo { x: 50.0, y: 50.0 }, // dx=500 units, needs long form
                StitchCommand::End,
            ],
        });

        let data = write_pes(&pattern).unwrap();
        let decoded = read_pes(&data).unwrap();

        let positions: Vec<(f64, f64)> = decoded
            .stitch_groups
            .iter()
            .flat_map(|g| &g.commands)
            .filter_map(|c| match c {
                StitchCommand::StitchTo { x, y } => Some((*x, *y)),
                _ => None,
            })
            .collect();
        assert_eq!(positions.len(), 1);
        assert!((positions[0].0 - 50.0).abs() < 0.15);
        assert!((positions[0].1 - 50.0).abs() < 0.15);
    }

    #[test]
    fn test_negative_delta_encoding() {
        // Test negative deltas (moving backwards)
        let mut pattern = EmbPattern::new();
        pattern.threads.push(Thread::new([0, 0, 0], "Black"));
        pattern.stitch_groups.push(StitchGroup {
            thread_index: 0,
            commands: vec![
                StitchCommand::MoveTo { x: 10.0, y: 10.0 },
                StitchCommand::StitchTo { x: 5.0, y: 5.0 }, // negative delta
                StitchCommand::StitchTo { x: 0.0, y: 0.0 },
                StitchCommand::End,
            ],
        });

        let data = write_pes(&pattern).unwrap();
        let decoded = read_pes(&data).unwrap();

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
        assert!((positions[0].0 - 5.0).abs() < 0.15);
        assert!((positions[1].0 - 0.0).abs() < 0.15);
    }

    #[test]
    fn test_invalid_pes_header() {
        let data = b"NOTAPES!xxxx";
        let err = read_pes(data).unwrap_err();
        assert!(matches!(err, PesError::InvalidHeader(_)));
    }

    #[test]
    fn test_pes_too_short() {
        let data = b"#PES";
        let err = read_pes(data).unwrap_err();
        assert!(matches!(err, PesError::InvalidHeader(_)));
    }

    #[test]
    fn test_version_parsing() {
        // All valid version strings should be accepted in the header
        for ver in [1, 10, 20, 30, 40, 50, 60] {
            let header = format!("#PES{:04}", ver);
            let mut data = Vec::from(header.as_bytes());
            // Add a PEC offset that points past end — we just want header parsing
            data.extend_from_slice(&[0x00; 4]); // offset 0 (invalid but we check header first)
                                                // This will fail at PEC parsing, not header parsing
            let err = read_pes(&data).unwrap_err();
            assert!(
                !matches!(err, PesError::InvalidHeader(_)),
                "version {} should be accepted",
                ver
            );
        }
    }

    #[test]
    fn test_too_many_stitches() {
        let mut pattern = EmbPattern::new();
        pattern.threads.push(Thread::new([0, 0, 0], "Black"));
        let commands: Vec<StitchCommand> = (0..300_001)
            .map(|i| StitchCommand::StitchTo {
                x: i as f64 * 0.01,
                y: 0.0,
            })
            .collect();
        pattern.stitch_groups.push(StitchGroup {
            thread_index: 0,
            commands,
        });

        let err = write_pes(&pattern).unwrap_err();
        assert!(matches!(err, PesError::TooManyStitches(_)));
    }

    #[test]
    fn test_palette_nearest_color() {
        use crate::palette::nearest_pec_index;

        // Pure red should match closest Brother red
        let idx = nearest_pec_index([255, 0, 0]);
        assert!(idx > 0 && idx <= 64);

        // Pure black should match index 20
        let idx = nearest_pec_index([0, 0, 0]);
        assert_eq!(idx, 20);
    }

    #[test]
    fn test_pec_thread_lookup() {
        use crate::palette::pec_thread;

        let thread = pec_thread(20).unwrap();
        assert_eq!(thread.color, [0, 0, 0]); // Black

        assert!(pec_thread(0).is_none());
        assert!(pec_thread(65).is_none());
    }

    #[test]
    fn test_jump_stitch_roundtrip() {
        let mut pattern = EmbPattern::new();
        pattern.threads.push(Thread::new([0, 0, 0], "Black"));
        pattern.stitch_groups.push(StitchGroup {
            thread_index: 0,
            commands: vec![
                StitchCommand::MoveTo { x: 0.0, y: 0.0 },
                StitchCommand::StitchTo { x: 5.0, y: 0.0 },
                StitchCommand::Jump { x: 20.0, y: 20.0 },
                StitchCommand::StitchTo { x: 25.0, y: 20.0 },
                StitchCommand::End,
            ],
        });

        let data = write_pes(&pattern).unwrap();
        let decoded = read_pes(&data).unwrap();

        let jumps: usize = decoded
            .stitch_groups
            .iter()
            .flat_map(|g| &g.commands)
            .filter(|c| matches!(c, StitchCommand::Jump { .. }))
            .count();
        // At least 1 jump (the MoveTo also encodes as jump)
        assert!(jumps >= 1);
    }
}
