//! PES file reader.

use crate::error::{PesError, Result};
use crate::pec::read_pec;
use vcad_embroidery::EmbPattern;

/// Supported PES version prefixes.
const PES_MAGIC: &[u8; 4] = b"#PES";

/// Read a PES file from raw bytes.
///
/// Supports PES versions 1 through 60 (`#PES0001` through `#PES0060`).
/// The actual stitch data is stored in the embedded PEC section.
pub fn read_pes(data: &[u8]) -> Result<EmbPattern> {
    // Minimum size: 8 (header) + 4 (PEC offset) = 12 bytes
    if data.len() < 12 {
        return Err(PesError::InvalidHeader("file too small".into()));
    }

    // Verify magic
    if &data[0..4] != PES_MAGIC {
        return Err(PesError::InvalidHeader(format!(
            "expected #PES, got {:?}",
            &data[0..4]
        )));
    }

    // Parse version (4 ASCII digits)
    let version_str = std::str::from_utf8(&data[4..8])
        .map_err(|_| PesError::InvalidHeader("invalid version bytes".into()))?;
    let _version: u32 = version_str
        .parse()
        .map_err(|_| PesError::InvalidHeader(format!("invalid version: {}", version_str)))?;

    // Read PEC section offset (u32 LE at offset 8)
    let pec_offset = u32::from_le_bytes([data[8], data[9], data[10], data[11]]) as usize;

    if pec_offset >= data.len() {
        return Err(PesError::InvalidPecOffset {
            offset: pec_offset as u32,
            length: data.len(),
        });
    }

    // Parse PEC section
    read_pec(data, pec_offset)
}
