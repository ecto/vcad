//! Brother PEC thread palette for PES files.
//!
//! Re-exports the palette from `vcad-embroidery` and provides PEC-specific
//! index mapping utilities.

use vcad_embroidery::Thread;

/// Get the Brother thread for a PEC palette index (1-64).
///
/// Returns `None` for index 0 or indices > 64.
pub fn pec_thread(index: u8) -> Option<Thread> {
    let palette = vcad_embroidery::brother_palette();
    if index == 0 || index as usize >= palette.len() {
        return None;
    }
    Some(palette[index as usize].clone())
}

/// Find the closest PEC palette index for an arbitrary RGB color.
///
/// Returns a 1-based index into the Brother 64-color palette.
pub fn nearest_pec_index(color: [u8; 3]) -> u8 {
    let palette = vcad_embroidery::brother_palette();
    let mut best_idx = 1u8;
    let mut best_dist = u32::MAX;

    for (i, entry) in palette.iter().enumerate().skip(1) {
        let c = entry.color;
        let dr = color[0] as i32 - c[0] as i32;
        let dg = color[1] as i32 - c[1] as i32;
        let db = color[2] as i32 - c[2] as i32;
        let dist = (dr * dr + dg * dg + db * db) as u32;
        if dist < best_dist {
            best_dist = dist;
            best_idx = i as u8;
        }
    }

    best_idx
}
