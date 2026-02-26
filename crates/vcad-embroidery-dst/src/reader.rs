//! DST file reader.
//!
//! Parses Tajima DST binary data into an [`EmbPattern`]. Each stitch record
//! is 3 bytes encoding a delta displacement and a record type (normal stitch,
//! jump, color change, or end).

use crate::error::{DstError, Result};
use crate::header::{read_header, HEADER_SIZE};
use vcad_embroidery::{EmbPattern, StitchCommand, StitchGroup, Thread};

/// Maximum number of stitch records we will process (safety limit).
const MAX_STITCHES: usize = 1_000_000;

/// DST record types decoded from the third byte of each 3-byte record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecordType {
    /// Normal stitch (needle down movement).
    Stitch,
    /// Jump (needle up movement, no thread laid).
    Jump,
    /// Color change (switch to next thread).
    ColorChange,
    /// End of stitch data.
    End,
}

/// Decode a single 3-byte DST stitch record into a delta and record type.
///
/// The displacement is encoded across specific bits of the three bytes using
/// a ternary-like scheme with values 1, 3, 9, 27, and 81.
fn decode_record(b: [u8; 3]) -> (i32, i32, RecordType) {
    let mut dx: i32 = 0;
    let mut dy: i32 = 0;

    // Decode dx from bits
    if b[0] & 0x01 != 0 {
        dx += 1;
    }
    if b[0] & 0x02 != 0 {
        dx -= 1;
    }
    if b[0] & 0x04 != 0 {
        dx += 9;
    }
    if b[0] & 0x08 != 0 {
        dx -= 9;
    }
    if b[1] & 0x01 != 0 {
        dx += 3;
    }
    if b[1] & 0x02 != 0 {
        dx -= 3;
    }
    if b[1] & 0x04 != 0 {
        dx += 27;
    }
    if b[1] & 0x08 != 0 {
        dx -= 27;
    }
    if b[2] & 0x04 != 0 {
        dx += 81;
    }
    if b[2] & 0x08 != 0 {
        dx -= 81;
    }

    // Decode dy from bits
    if b[0] & 0x80 != 0 {
        dy += 1;
    }
    if b[0] & 0x40 != 0 {
        dy -= 1;
    }
    if b[0] & 0x20 != 0 {
        dy += 9;
    }
    if b[0] & 0x10 != 0 {
        dy -= 9;
    }
    if b[1] & 0x80 != 0 {
        dy += 3;
    }
    if b[1] & 0x40 != 0 {
        dy -= 3;
    }
    if b[1] & 0x20 != 0 {
        dy += 27;
    }
    if b[1] & 0x10 != 0 {
        dy -= 27;
    }
    if b[2] & 0x20 != 0 {
        dy += 81;
    }
    if b[2] & 0x10 != 0 {
        dy -= 81;
    }

    let rec_type = if b[0] == 0x00 && b[1] == 0x00 && b[2] == 0xF3 {
        RecordType::End
    } else if b[2] & 0xC0 == 0xC0 {
        RecordType::ColorChange
    } else if b[2] & 0x80 != 0 {
        RecordType::Jump
    } else {
        RecordType::Stitch
    };

    (dx, dy, rec_type)
}

/// Read a Tajima DST file from raw bytes into an [`EmbPattern`].
///
/// DST files consist of a 512-byte ASCII header followed by 3-byte stitch
/// records. Coordinates are in 0.1mm units internally and converted to
/// millimeters for the returned pattern.
///
/// DST does not store thread colors, so all threads default to gray.
pub fn read_dst(data: &[u8]) -> Result<EmbPattern> {
    if data.len() < HEADER_SIZE {
        return Err(DstError::InvalidHeader(format!(
            "data too short: {} bytes (need at least {})",
            data.len(),
            HEADER_SIZE
        )));
    }

    let header = read_header(data)?;

    let stitch_data = &data[HEADER_SIZE..];
    if stitch_data.len() < 3 {
        return Err(DstError::EmptyPattern);
    }

    // Parse 3-byte records, tracking absolute position in 0.1mm units
    let mut abs_x: i32 = 0;
    let mut abs_y: i32 = 0;
    let mut thread_index: usize = 0;
    let mut current_commands: Vec<StitchCommand> = Vec::new();
    let mut stitch_groups: Vec<StitchGroup> = Vec::new();
    let mut record_count: usize = 0;

    // Start each group with a MoveTo at the origin
    current_commands.push(StitchCommand::MoveTo { x: 0.0, y: 0.0 });

    let mut offset = 0;
    while offset + 3 <= stitch_data.len() {
        let b = [
            stitch_data[offset],
            stitch_data[offset + 1],
            stitch_data[offset + 2],
        ];
        let (dx, dy, rec_type) = decode_record(b);

        record_count += 1;
        if record_count > MAX_STITCHES {
            return Err(DstError::TooManyStitches(record_count));
        }

        match rec_type {
            RecordType::End => {
                current_commands.push(StitchCommand::End);
                break;
            }
            RecordType::ColorChange => {
                abs_x += dx;
                abs_y += dy;

                // Finish current group
                if !current_commands.is_empty() {
                    stitch_groups.push(StitchGroup {
                        thread_index,
                        commands: std::mem::take(&mut current_commands),
                    });
                }

                thread_index += 1;

                // Start new group with MoveTo at current position
                let mm_x = abs_x as f64 / 10.0;
                let mm_y = abs_y as f64 / 10.0;
                current_commands.push(StitchCommand::MoveTo { x: mm_x, y: mm_y });
            }
            RecordType::Jump => {
                abs_x += dx;
                abs_y += dy;
                let mm_x = abs_x as f64 / 10.0;
                let mm_y = abs_y as f64 / 10.0;
                current_commands.push(StitchCommand::Jump { x: mm_x, y: mm_y });
            }
            RecordType::Stitch => {
                abs_x += dx;
                abs_y += dy;
                let mm_x = abs_x as f64 / 10.0;
                let mm_y = abs_y as f64 / 10.0;
                current_commands.push(StitchCommand::StitchTo { x: mm_x, y: mm_y });
            }
        }

        offset += 3;
    }

    // Push the last group if non-empty
    if !current_commands.is_empty() {
        stitch_groups.push(StitchGroup {
            thread_index,
            commands: current_commands,
        });
    }

    if stitch_groups.is_empty() {
        return Err(DstError::EmptyPattern);
    }

    // DST stores no thread colors — create default gray threads
    let thread_count = thread_index + 1;
    let threads: Vec<Thread> = (0..thread_count)
        .map(|i| Thread::new([128, 128, 128], format!("Thread {}", i + 1)))
        .collect();

    let mut pattern = EmbPattern::new();
    pattern.threads = threads;
    pattern.stitch_groups = stitch_groups;
    pattern.metadata.name = header.label;

    Ok(pattern)
}
