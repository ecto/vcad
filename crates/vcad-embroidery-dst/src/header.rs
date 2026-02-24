//! DST file header (512 bytes, ASCII).
//!
//! The Tajima DST header contains metadata fields at fixed byte offsets,
//! each padded with spaces and terminated with `\r`. The final byte is `0x1A`.

use crate::error::{DstError, Result};

/// DST header size in bytes.
pub const HEADER_SIZE: usize = 512;

/// Parsed DST header fields.
#[derive(Debug, Clone)]
pub struct DstHeader {
    /// Pattern label (up to 16 characters).
    pub label: String,
    /// Total stitch count.
    pub stitch_count: u32,
    /// Number of color changes.
    pub color_changes: u16,
    /// Positive X extent in 0.1mm units.
    pub positive_x: i32,
    /// Negative X extent in 0.1mm units.
    pub negative_x: i32,
    /// Positive Y extent in 0.1mm units.
    pub positive_y: i32,
    /// Negative Y extent in 0.1mm units.
    pub negative_y: i32,
}

/// Read a field value from the header at a fixed prefix.
///
/// Searches for `prefix` (e.g. `"ST:"`) and parses the following numeric value
/// up to the `\r` terminator.
fn read_field(header: &str, prefix: &str) -> Option<String> {
    let start = header.find(prefix)?;
    let value_start = start + prefix.len();
    let rest = &header[value_start..];
    let end = rest.find('\r').unwrap_or(rest.len());
    Some(rest[..end].trim().to_string())
}

/// Parse the 512-byte DST header from raw data.
pub fn read_header(data: &[u8]) -> Result<DstHeader> {
    if data.len() < HEADER_SIZE {
        return Err(DstError::InvalidHeader(format!(
            "data too short: {} bytes (need {})",
            data.len(),
            HEADER_SIZE
        )));
    }

    let header_bytes = &data[..HEADER_SIZE];
    let header_str = String::from_utf8_lossy(header_bytes);

    // Label: "LA:" prefix, up to 16 chars
    let label = read_field(&header_str, "LA:")
        .unwrap_or_default()
        .trim()
        .to_string();

    let stitch_count = read_field(&header_str, "ST:")
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0);

    let color_changes = read_field(&header_str, "CO:")
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);

    let positive_x = read_field(&header_str, "+X:")
        .and_then(|s| s.parse::<i32>().ok())
        .unwrap_or(0);

    let negative_x = read_field(&header_str, "-X:")
        .and_then(|s| s.parse::<i32>().ok())
        .unwrap_or(0);

    let positive_y = read_field(&header_str, "+Y:")
        .and_then(|s| s.parse::<i32>().ok())
        .unwrap_or(0);

    let negative_y = read_field(&header_str, "-Y:")
        .and_then(|s| s.parse::<i32>().ok())
        .unwrap_or(0);

    Ok(DstHeader {
        label,
        stitch_count,
        color_changes,
        positive_x,
        negative_x,
        positive_y,
        negative_y,
    })
}

/// Write a DST header as a 512-byte vector.
pub fn write_header(header: &DstHeader) -> Vec<u8> {
    let mut buf = vec![0x20u8; HEADER_SIZE];

    let mut offset = 0;

    // LA:label (20 bytes total: "LA:" + up to 16 chars + \r)
    offset += write_field(&mut buf, offset, "LA:", &header.label, 20);

    // ST:nnnnnnn\r (11 bytes)
    offset += write_field(&mut buf, offset, "ST:", &format!("{:7}", header.stitch_count), 11);

    // CO:nnn\r (7 bytes)
    offset += write_field(&mut buf, offset, "CO:", &format!("{:3}", header.color_changes), 7);

    // +X:nnnnn\r (9 bytes)
    offset += write_field(&mut buf, offset, "+X:", &format!("{:5}", header.positive_x), 9);

    // -X:nnnnn\r (9 bytes)
    offset += write_field(&mut buf, offset, "-X:", &format!("{:5}", header.negative_x), 9);

    // +Y:nnnnn\r (9 bytes)
    offset += write_field(&mut buf, offset, "+Y:", &format!("{:5}", header.positive_y), 9);

    // -Y:nnnnn\r (9 bytes)
    let _ = write_field(&mut buf, offset, "-Y:", &format!("{:5}", header.negative_y), 9);

    // Final byte is 0x1A (SUB / end-of-file marker)
    buf[HEADER_SIZE - 1] = 0x1A;

    buf
}

/// Write a single header field into the buffer. Returns the total field width consumed.
fn write_field(buf: &mut [u8], offset: usize, prefix: &str, value: &str, total_width: usize) -> usize {
    let prefix_bytes = prefix.as_bytes();
    let value_bytes = value.as_bytes();

    // Write prefix
    let end = (offset + total_width).min(buf.len());
    let mut pos = offset;

    for &b in prefix_bytes {
        if pos >= end {
            break;
        }
        buf[pos] = b;
        pos += 1;
    }

    // Write value
    for &b in value_bytes {
        if pos >= end - 1 {
            break;
        }
        buf[pos] = b;
        pos += 1;
    }

    // Pad remaining with spaces (already 0x20), then write \r at field end
    buf[end - 1] = b'\r';

    total_width
}
