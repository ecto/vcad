//! DST file writer.
//!
//! Converts an [`EmbPattern`] into the Tajima DST binary format. Coordinates
//! are converted from millimeters to 0.1mm integer units, and large deltas
//! are split into multiple jump records (max +-121 per record).

use crate::error::{DstError, Result};
use crate::header::{write_header, DstHeader};
use vcad_embroidery::{EmbPattern, StitchCommand};

/// Maximum displacement per single DST record in 0.1mm units.
const MAX_DELTA: i32 = 121;

/// DST record types for the encoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecordType {
    /// Normal stitch (needle down movement).
    Stitch,
    /// Jump (needle up movement).
    Jump,
    /// Color change (switch to next thread).
    ColorChange,
    /// End of stitch data.
    End,
}

/// Decompose a value into balanced ternary digits for the DST place values
/// (3^0=1, 3^1=3, 3^2=9, 3^3=27, 3^4=81). Each digit is -1, 0, or +1.
///
/// Returns `[d1, d3, d9, d27, d81]` such that
/// `d1*1 + d3*3 + d9*9 + d27*27 + d81*81 == value`.
fn balanced_ternary(value: i32) -> [i8; 5] {
    let neg = value < 0;
    let mut v = value.unsigned_abs();

    // Convert to base-3 digits, then adjust to balanced ternary
    let mut digits = [0i8; 6]; // extra slot for carry
    for d in digits.iter_mut().take(5) {
        let rem = v % 3;
        v /= 3;
        *d = rem as i8;
    }
    digits[5] = v as i8;

    // Convert to balanced: if digit > 1, set it to -1 and carry +1
    for i in 0..5 {
        if digits[i] > 1 {
            digits[i] -= 3;
            digits[i + 1] += 1;
        }
    }

    // Apply sign
    if neg {
        for d in digits.iter_mut().take(5) {
            *d = -*d;
        }
    }

    [digits[0], digits[1], digits[2], digits[3], digits[4]]
}

/// Encode a delta and record type into a 3-byte DST stitch record.
///
/// The delta values must be in the range -121..=121. Larger deltas must
/// be split across multiple records before calling this function.
fn encode_record(dx: i32, dy: i32, rec_type: RecordType) -> [u8; 3] {
    if rec_type == RecordType::End {
        return [0x00, 0x00, 0xF3];
    }

    let mut b = [0u8; 3];

    // Decompose dx into balanced ternary: [d1, d3, d9, d27, d81]
    let xd = balanced_ternary(dx);
    // d1  -> b[0] bit 0 (+1) / bit 1 (-1)
    if xd[0] > 0 {
        b[0] |= 0x01;
    }
    if xd[0] < 0 {
        b[0] |= 0x02;
    }
    // d3  -> b[1] bit 0 (+3) / bit 1 (-3)
    if xd[1] > 0 {
        b[1] |= 0x01;
    }
    if xd[1] < 0 {
        b[1] |= 0x02;
    }
    // d9  -> b[0] bit 2 (+9) / bit 3 (-9)
    if xd[2] > 0 {
        b[0] |= 0x04;
    }
    if xd[2] < 0 {
        b[0] |= 0x08;
    }
    // d27 -> b[1] bit 2 (+27) / bit 3 (-27)
    if xd[3] > 0 {
        b[1] |= 0x04;
    }
    if xd[3] < 0 {
        b[1] |= 0x08;
    }
    // d81 -> b[2] bit 2 (+81) / bit 3 (-81)
    if xd[4] > 0 {
        b[2] |= 0x04;
    }
    if xd[4] < 0 {
        b[2] |= 0x08;
    }

    // Decompose dy into balanced ternary: [d1, d3, d9, d27, d81]
    let yd = balanced_ternary(dy);
    // d1  -> b[0] bit 7 (+1) / bit 6 (-1)
    if yd[0] > 0 {
        b[0] |= 0x80;
    }
    if yd[0] < 0 {
        b[0] |= 0x40;
    }
    // d3  -> b[1] bit 7 (+3) / bit 6 (-3)
    if yd[1] > 0 {
        b[1] |= 0x80;
    }
    if yd[1] < 0 {
        b[1] |= 0x40;
    }
    // d9  -> b[0] bit 5 (+9) / bit 4 (-9)
    if yd[2] > 0 {
        b[0] |= 0x20;
    }
    if yd[2] < 0 {
        b[0] |= 0x10;
    }
    // d27 -> b[1] bit 5 (+27) / bit 4 (-27)
    if yd[3] > 0 {
        b[1] |= 0x20;
    }
    if yd[3] < 0 {
        b[1] |= 0x10;
    }
    // d81 -> b[2] bit 5 (+81) / bit 4 (-81)
    if yd[4] > 0 {
        b[2] |= 0x20;
    }
    if yd[4] < 0 {
        b[2] |= 0x10;
    }

    match rec_type {
        RecordType::Stitch => {
            b[2] |= 0x03;
        }
        RecordType::Jump => {
            b[2] |= 0x83;
        }
        RecordType::ColorChange => {
            b[2] |= 0xC3;
        }
        RecordType::End => unreachable!(
            "encode_record called with RecordType::End; End must be written via write_end_record"
        ),
    }

    b
}

/// Emit stitch records for a delta that may exceed the per-record limit.
///
/// For deltas larger than +-121, multiple jump records are emitted to cover
/// the distance, followed by a final record of the requested type.
fn emit_records(out: &mut Vec<u8>, dx: i32, dy: i32, rec_type: RecordType) {
    let mut remaining_x = dx;
    let mut remaining_y = dy;

    // Split into jumps if the delta exceeds the max
    while remaining_x.abs() > MAX_DELTA || remaining_y.abs() > MAX_DELTA {
        let chunk_x = remaining_x.clamp(-MAX_DELTA, MAX_DELTA);
        let chunk_y = remaining_y.clamp(-MAX_DELTA, MAX_DELTA);
        let record = encode_record(chunk_x, chunk_y, RecordType::Jump);
        out.extend_from_slice(&record);
        remaining_x -= chunk_x;
        remaining_y -= chunk_y;
    }

    let record = encode_record(remaining_x, remaining_y, rec_type);
    out.extend_from_slice(&record);
}

/// Write an [`EmbPattern`] as Tajima DST binary data.
///
/// The pattern's millimeter coordinates are converted to 0.1mm integer units.
/// Stitch groups are separated by color-change records. Returns the complete
/// DST file contents including the 512-byte header.
pub fn write_dst(pattern: &EmbPattern) -> Result<Vec<u8>> {
    if pattern.stitch_groups.is_empty() {
        return Err(DstError::EmptyPattern);
    }

    let total_commands: usize = pattern.stitch_groups.iter().map(|g| g.commands.len()).sum();
    if total_commands > 1_000_000 {
        return Err(DstError::TooManyStitches(total_commands));
    }

    // Flatten pattern to absolute positions, then compute deltas in 0.1mm
    let mut stitch_data = Vec::new();
    let mut last_x: i32 = 0;
    let mut last_y: i32 = 0;
    let mut stitch_count: u32 = 0;
    let mut color_changes: u16 = 0;

    // Track extents for header
    let mut min_x: i32 = 0;
    let mut max_x: i32 = 0;
    let mut min_y: i32 = 0;
    let mut max_y: i32 = 0;

    for (group_idx, group) in pattern.stitch_groups.iter().enumerate() {
        // Emit color change between groups (not before the first)
        if group_idx > 0 {
            emit_records(&mut stitch_data, 0, 0, RecordType::ColorChange);
            color_changes += 1;
        }

        for cmd in &group.commands {
            match cmd {
                StitchCommand::MoveTo { x, y } | StitchCommand::Jump { x, y } => {
                    let tx = (*x * 10.0).round() as i32;
                    let ty = (*y * 10.0).round() as i32;
                    let dx = tx - last_x;
                    let dy = ty - last_y;
                    emit_records(&mut stitch_data, dx, dy, RecordType::Jump);
                    last_x = tx;
                    last_y = ty;
                    update_extents(&mut min_x, &mut max_x, &mut min_y, &mut max_y, tx, ty);
                }
                StitchCommand::StitchTo { x, y } => {
                    let tx = (*x * 10.0).round() as i32;
                    let ty = (*y * 10.0).round() as i32;
                    let dx = tx - last_x;
                    let dy = ty - last_y;
                    emit_records(&mut stitch_data, dx, dy, RecordType::Stitch);
                    last_x = tx;
                    last_y = ty;
                    stitch_count += 1;
                    update_extents(&mut min_x, &mut max_x, &mut min_y, &mut max_y, tx, ty);
                }
                StitchCommand::ColorChange { .. } => {
                    // Handled at group boundaries
                }
                StitchCommand::Trim => {
                    // DST has no explicit trim; just continue
                }
                StitchCommand::Stop => {
                    // DST has no explicit stop; just continue
                }
                StitchCommand::End => {
                    // Will be written after all groups
                }
            }
        }
    }

    // Write end record
    stitch_data.extend_from_slice(&encode_record(0, 0, RecordType::End));

    // Build header
    let label = if pattern.metadata.name.is_empty() {
        "Untitled".to_string()
    } else {
        pattern.metadata.name.chars().take(16).collect()
    };

    let header = DstHeader {
        label,
        stitch_count,
        color_changes,
        positive_x: max_x.max(0),
        negative_x: min_x.min(0).abs(),
        positive_y: max_y.max(0),
        negative_y: min_y.min(0).abs(),
    };

    let mut out = write_header(&header);
    out.extend_from_slice(&stitch_data);

    Ok(out)
}

/// Update running min/max extents.
fn update_extents(
    min_x: &mut i32,
    max_x: &mut i32,
    min_y: &mut i32,
    max_y: &mut i32,
    x: i32,
    y: i32,
) {
    if x < *min_x {
        *min_x = x;
    }
    if x > *max_x {
        *max_x = x;
    }
    if y < *min_y {
        *min_y = y;
    }
    if y > *max_y {
        *max_y = y;
    }
}
