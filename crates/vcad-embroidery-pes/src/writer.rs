//! PES file writer.

use crate::error::{PesError, Result};
use crate::pec::write_pec;
use vcad_embroidery::EmbPattern;

/// Write a pattern as a PES v1 file.
///
/// Produces a `#PES0001` file, which has the widest machine compatibility.
pub fn write_pes(pattern: &EmbPattern) -> Result<Vec<u8>> {
    // Validate basic limits
    let total_stitches: usize = pattern.stitch_groups.iter().map(|g| g.commands.len()).sum();
    if total_stitches > 300_000 {
        return Err(PesError::TooManyStitches(total_stitches));
    }
    if pattern.stitch_groups.len() > 127 {
        return Err(PesError::TooManyColors(pattern.stitch_groups.len()));
    }

    let mut out = Vec::new();

    // Header: "#PES0001"
    out.extend_from_slice(b"#PES0001");

    // PEC offset placeholder (will backfill)
    let pec_offset_pos = out.len();
    out.extend_from_slice(&[0u8; 4]);

    // Minimal PES v1 body: CEmbOne block
    // This is the simplest valid PES v1 section.
    write_cembone_block(&mut out, pattern);

    // Record PEC offset and backfill
    let pec_offset = out.len() as u32;
    out[pec_offset_pos..pec_offset_pos + 4].copy_from_slice(&pec_offset.to_le_bytes());

    // Write PEC section
    let pec_data = write_pec(pattern)?;
    out.extend_from_slice(&pec_data);

    Ok(out)
}

/// Write the minimal CEmbOne block for PES v1.
///
/// This contains an identity affine transform and segment count.
fn write_cembone_block(out: &mut Vec<u8>, pattern: &EmbPattern) {
    // CEmbOne header: "CEmbOne" as a length-prefixed string
    let name = b"CEmbOne";
    write_u16_le(out, name.len() as u16);
    out.extend_from_slice(name);

    // Bounds (min/max in 0.1mm units, as i16 LE)
    let stats = pattern.stats();
    let min_x = (stats.bounds_min.0 * 10.0) as i16;
    let min_y = (stats.bounds_min.1 * 10.0) as i16;
    let max_x = (stats.bounds_max.0 * 10.0) as i16;
    let max_y = (stats.bounds_max.1 * 10.0) as i16;

    write_i16_le(out, min_x);
    write_i16_le(out, min_y);
    write_i16_le(out, max_x);
    write_i16_le(out, max_y);

    // Affine transform (identity): scale_x, scale_y, rotate, translate_x, translate_y, unknown
    // 6 x f32 LE
    write_f32_le(out, 1.0); // scale x
    write_f32_le(out, 0.0); // skew
    write_f32_le(out, 0.0); // skew
    write_f32_le(out, 1.0); // scale y
    write_f32_le(out, 0.0); // translate x
    write_f32_le(out, 0.0); // translate y

    // Unknown field
    write_u16_le(out, 1);

    // Segment count
    write_u16_le(out, 0);

    // CSewSeg count (0 for minimal)
    write_u16_le(out, 0xFFFF); // end marker
    write_u16_le(out, 0); // padding
}

fn write_u16_le(out: &mut Vec<u8>, val: u16) {
    out.extend_from_slice(&val.to_le_bytes());
}

fn write_i16_le(out: &mut Vec<u8>, val: i16) {
    out.extend_from_slice(&val.to_le_bytes());
}

fn write_f32_le(out: &mut Vec<u8>, val: f32) {
    out.extend_from_slice(&val.to_le_bytes());
}
