//! PEC section read/write.
//!
//! The PEC (Brother Embroidery Card) section is embedded within PES files
//! and contains the actual stitch data. It can also appear standalone in `.pec` files.

use crate::error::{PesError, Result};
use crate::palette::{nearest_pec_index, pec_thread};
use vcad_embroidery::{EmbPattern, PatternMetadata, StitchCommand, StitchGroup, Thread};

/// PEC header label length (padded with spaces).
const PEC_LABEL_SIZE: usize = 19;

/// Read the PEC section from data starting at `offset`.
///
/// Returns the parsed pattern. The `offset` should point to the start of the
/// PEC header (the `"LA:"` marker).
pub fn read_pec(data: &[u8], offset: usize) -> Result<EmbPattern> {
    let mut pos = offset;

    // Label: "LA:" + 16 chars + CR
    if pos + PEC_LABEL_SIZE > data.len() {
        return Err(PesError::UnexpectedEof(pos));
    }
    let label_bytes = &data[pos..pos + PEC_LABEL_SIZE];
    let label = String::from_utf8_lossy(&label_bytes[3..19])
        .trim()
        .to_string();
    pos += PEC_LABEL_SIZE;

    // Skip 11 bytes of PEC header fields we don't need
    pos += 11;
    if pos >= data.len() {
        return Err(PesError::UnexpectedEof(pos));
    }

    // Color count: number of color changes + 1
    let num_colors = data[pos] as usize + 1;
    pos += 1;

    // Read palette indices (one per color)
    if pos + num_colors > data.len() {
        return Err(PesError::UnexpectedEof(pos));
    }
    let palette_indices: Vec<u8> = data[pos..pos + num_colors].to_vec();
    pos += num_colors;

    // Skip to stitch data: advance to 512-byte aligned offset from PEC start,
    // then skip the thumbnail data.
    // The stitch data starts after 512 - (pos - offset) % 512 padding + 512 bytes of thumbnail.
    let remainder = (pos - offset) % 512;
    if remainder != 0 {
        pos += 512 - remainder;
    }
    // Skip two thumbnail blocks (each is variable but typically 6 + width*height bytes).
    // Instead, scan for the stitch data by advancing past the thumbnail section.
    // The thumbnail section is 512 bytes from the aligned position.
    pos += 512;

    if pos >= data.len() {
        return Err(PesError::UnexpectedEof(pos));
    }

    // Build threads from palette indices
    let threads: Vec<Thread> = palette_indices
        .iter()
        .map(|&idx| pec_thread(idx).unwrap_or_else(|| Thread::new([128, 128, 128], "Unknown")))
        .collect();

    // Decode stitch data
    let mut commands: Vec<StitchCommand> = Vec::new();
    let mut abs_x = 0.0f64;
    let mut abs_y = 0.0f64;

    while pos < data.len() {
        let b0 = data[pos];

        if b0 == 0xFF {
            // End of stitches
            commands.push(StitchCommand::End);
            break;
        }

        if b0 == 0xFE && pos + 1 < data.len() && data[pos + 1] == 0xB0 {
            // Color change
            if pos + 2 >= data.len() {
                return Err(PesError::UnexpectedEof(pos));
            }
            let color_idx = data[pos + 2] as u32;
            commands.push(StitchCommand::ColorChange { index: color_idx });
            pos += 3;
            continue;
        }

        // Decode dx, dy (variable length)
        let (dx, dy, is_jump, bytes_consumed) = decode_stitch_delta(data, pos)?;

        abs_x += dx;
        abs_y += dy;
        pos += bytes_consumed;

        if is_jump {
            commands.push(StitchCommand::Jump { x: abs_x, y: abs_y });
        } else {
            commands.push(StitchCommand::StitchTo { x: abs_x, y: abs_y });
        }
    }

    // Split commands into stitch groups by color
    let stitch_groups = split_into_groups(commands, num_colors);

    Ok(EmbPattern {
        threads,
        stitch_groups,
        metadata: PatternMetadata {
            name: label,
            ..Default::default()
        },
    })
}

/// Decode a single stitch delta from the PEC byte stream.
///
/// Returns `(dx_mm, dy_mm, is_jump, bytes_consumed)`.
fn decode_stitch_delta(data: &[u8], pos: usize) -> Result<(f64, f64, bool, usize)> {
    let mut cur = pos;
    let mut is_jump = false;

    if cur >= data.len() {
        return Err(PesError::UnexpectedEof(cur));
    }

    // Decode X axis
    let b0 = data[cur];
    let dx = if b0 & 0x80 != 0 {
        // Long form: 2 bytes for this axis
        if cur + 1 >= data.len() {
            return Err(PesError::UnexpectedEof(cur));
        }
        if b0 & 0x20 != 0 {
            is_jump = true;
        }
        if b0 & 0x10 != 0 {
            is_jump = true; // trim implies jump
        }
        let raw = (((b0 & 0x0F) as u16) << 8) | data[cur + 1] as u16;
        let val = if raw & 0x0800 != 0 {
            (raw | 0xF000) as i16
        } else {
            raw as i16
        };
        cur += 2;
        val as f64 * 0.1
    } else {
        // Short form: 1 byte
        let val = if b0 & 0x40 != 0 {
            (b0 | 0x80) as i8
        } else {
            b0 as i8
        };
        cur += 1;
        val as f64 * 0.1
    };

    // Decode Y axis
    if cur >= data.len() {
        return Err(PesError::UnexpectedEof(cur));
    }
    let yb0 = data[cur];
    let dy = if yb0 & 0x80 != 0 {
        // Long form: 2 bytes
        if cur + 1 >= data.len() {
            return Err(PesError::UnexpectedEof(cur));
        }
        if yb0 & 0x20 != 0 {
            is_jump = true;
        }
        if yb0 & 0x10 != 0 {
            is_jump = true;
        }
        let raw = (((yb0 & 0x0F) as u16) << 8) | data[cur + 1] as u16;
        let val = if raw & 0x0800 != 0 {
            (raw | 0xF000) as i16
        } else {
            raw as i16
        };
        cur += 2;
        val as f64 * 0.1
    } else {
        let val = if yb0 & 0x40 != 0 {
            (yb0 | 0x80) as i8
        } else {
            yb0 as i8
        };
        cur += 1;
        val as f64 * 0.1
    };

    Ok((dx, dy, is_jump, cur - pos))
}

/// Split a flat command list into stitch groups by color changes.
fn split_into_groups(commands: Vec<StitchCommand>, num_colors: usize) -> Vec<StitchGroup> {
    let mut groups: Vec<StitchGroup> = Vec::new();
    let mut current_thread = 0usize;
    let mut current_cmds: Vec<StitchCommand> = Vec::new();
    let mut color_sequence_idx = 0usize;

    for cmd in commands {
        match cmd {
            StitchCommand::ColorChange { .. } => {
                // Flush current group
                if !current_cmds.is_empty() {
                    groups.push(StitchGroup {
                        thread_index: current_thread,
                        commands: std::mem::take(&mut current_cmds),
                    });
                }
                color_sequence_idx += 1;
                current_thread = color_sequence_idx.min(num_colors - 1);
            }
            StitchCommand::End => {
                current_cmds.push(StitchCommand::End);
                // Flush final group
                if !current_cmds.is_empty() {
                    groups.push(StitchGroup {
                        thread_index: current_thread,
                        commands: std::mem::take(&mut current_cmds),
                    });
                }
            }
            other => {
                current_cmds.push(other);
            }
        }
    }

    // Flush any remaining commands
    if !current_cmds.is_empty() {
        groups.push(StitchGroup {
            thread_index: current_thread,
            commands: current_cmds,
        });
    }

    groups
}

/// Write the PEC section for a pattern, returning the raw bytes.
pub fn write_pec(pattern: &vcad_embroidery::EmbPattern) -> Result<Vec<u8>> {
    let mut out = Vec::new();

    // Label: "LA:" + 16 chars padded with spaces + CR (0x0D)
    let label = &pattern.metadata.name;
    if label.len() > 16 {
        return Err(PesError::LabelTooLong(label.clone()));
    }
    out.extend_from_slice(b"LA:");
    let padded_label = format!("{:16}", label);
    out.extend_from_slice(padded_label.as_bytes());
    // Note: PEC_LABEL_SIZE = 19 = 3 ("LA:") + 16 (label), but we also need trailing CR
    // Actually label_size = 19 bytes: "LA:" (3) + name (16) = 19. The CR follows.
    // Some implementations include CR in the 19; let's add a 0x0D plus padding.
    // We'll pad out to fill the header correctly.

    // 11 bytes of PEC header fields (zeroed for minimal output)
    // Byte 20: CR, then 10 more zeros
    out.push(0x0D); // CR after label
    out.extend_from_slice(&[0x00; 6]); // 6 padding bytes
    out.push(0xFF); // unknown field (commonly 0xFF)
    out.push(0x00);
    out.push(0x06); // thumbnail width (6 bytes = 48 pixels)
    out.push(0x26); // thumbnail height (38 pixels)

    // Color count = number of color changes (num_groups - 1)
    let num_groups = pattern.stitch_groups.len();
    if num_groups > 127 {
        return Err(PesError::TooManyColors(num_groups));
    }
    let color_changes = if num_groups > 0 { num_groups - 1 } else { 0 };
    out.push(color_changes as u8);

    // Palette indices — map each thread to nearest PEC color
    for group in &pattern.stitch_groups {
        let thread = &pattern.threads[group.thread_index];
        let pec_idx = nearest_pec_index(thread.color);
        out.push(pec_idx);
    }

    // Pad to 512-byte alignment from the start
    let current_len = out.len();
    let remainder = current_len % 512;
    if remainder != 0 {
        let padding = 512 - remainder;
        out.extend(std::iter::repeat_n(0x20u8, padding));
    }

    // Blank thumbnail (512 bytes)
    out.extend(std::iter::repeat_n(0x00u8, 512));

    // Encode stitch data
    let stitch_data = encode_pec_stitches(pattern)?;
    out.extend_from_slice(&stitch_data);

    Ok(out)
}

/// Encode all stitch commands as PEC stitch bytes.
fn encode_pec_stitches(pattern: &vcad_embroidery::EmbPattern) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut last_x = 0.0f64;
    let mut last_y = 0.0f64;

    for (group_idx, group) in pattern.stitch_groups.iter().enumerate() {
        // Insert color change before each group (except the first)
        if group_idx > 0 {
            out.push(0xFE);
            out.push(0xB0);
            out.push(group_idx as u8);
        }

        for cmd in &group.commands {
            match *cmd {
                StitchCommand::StitchTo { x, y } => {
                    let dx = x - last_x;
                    let dy = y - last_y;
                    encode_delta(&mut out, dx, dy, false);
                    last_x = x;
                    last_y = y;
                }
                StitchCommand::MoveTo { x, y } | StitchCommand::Jump { x, y } => {
                    let dx = x - last_x;
                    let dy = y - last_y;
                    encode_delta(&mut out, dx, dy, true);
                    last_x = x;
                    last_y = y;
                }
                StitchCommand::Trim => {
                    // Trim is encoded as a special jump with trim flag
                    // We just encode it as a zero-distance jump with trim bit
                    // In practice, trims usually precede a jump, so this is a no-op marker
                }
                StitchCommand::ColorChange { .. } => {
                    // Handled at group level above
                }
                StitchCommand::Stop => {
                    // Stop is not directly encoded in PEC; skip
                }
                StitchCommand::End => {
                    // End marker
                    out.push(0xFF);
                }
            }
        }
    }

    // Ensure we have an end marker
    if out.last() != Some(&0xFF) {
        out.push(0xFF);
    }

    Ok(out)
}

/// Encode a single dx/dy delta into PEC format.
fn encode_delta(out: &mut Vec<u8>, dx_mm: f64, dy_mm: f64, is_jump: bool) {
    let dx = (dx_mm * 10.0).round() as i32; // mm to 0.1mm units
    let dy = (dy_mm * 10.0).round() as i32;

    encode_axis(out, dx, is_jump);
    encode_axis(out, dy, is_jump);
}

/// Encode a single axis value (dx or dy) in PEC short or long form.
fn encode_axis(out: &mut Vec<u8>, val: i32, is_jump: bool) {
    if !is_jump && (-63..=63).contains(&val) {
        // Short form: 7-bit signed, MSB clear
        out.push((val & 0x7F) as u8);
    } else {
        // Long form: 12-bit signed, MSB set
        let mut hi = 0x80u8;
        if is_jump {
            hi |= 0x20; // jump flag
        }
        let raw = if val < 0 {
            (val & 0x0FFF) as u16
        } else {
            val as u16 & 0x0FFF
        };
        hi |= ((raw >> 8) & 0x0F) as u8;
        let lo = (raw & 0xFF) as u8;
        out.push(hi);
        out.push(lo);
    }
}
