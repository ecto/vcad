//! Record-level GDSII stream reader/writer.
//!
//! A GDSII file is a flat sequence of records. Every record starts with a
//! 4-byte header: a big-endian `u16` total length (including the header),
//! a record-type byte, and a data-type byte. All multi-byte values are
//! big-endian; strings are padded with a trailing NUL to an even length.

use crate::error::{GdsError, Result};
use crate::real::{f64_to_real8, real8_to_f64};

/// Record type constants (the third header byte).
pub mod rectype {
    /// Stream format version.
    pub const HEADER: u8 = 0x00;
    /// Begin library (modification timestamps).
    pub const BGNLIB: u8 = 0x01;
    /// Library name.
    pub const LIBNAME: u8 = 0x02;
    /// User unit and database unit in meters.
    pub const UNITS: u8 = 0x03;
    /// End of library.
    pub const ENDLIB: u8 = 0x04;
    /// Begin structure (cell).
    pub const BGNSTR: u8 = 0x05;
    /// Structure (cell) name.
    pub const STRNAME: u8 = 0x06;
    /// End of structure.
    pub const ENDSTR: u8 = 0x07;
    /// Boundary (polygon) element.
    pub const BOUNDARY: u8 = 0x08;
    /// Path (wire) element.
    pub const PATH: u8 = 0x09;
    /// Structure reference element.
    pub const SREF: u8 = 0x0a;
    /// Array reference element.
    pub const AREF: u8 = 0x0b;
    /// Text element.
    pub const TEXT: u8 = 0x0c;
    /// Layer number.
    pub const LAYER: u8 = 0x0d;
    /// Datatype number.
    pub const DATATYPE: u8 = 0x0e;
    /// Path width in database units.
    pub const WIDTH: u8 = 0x0f;
    /// Coordinate list.
    pub const XY: u8 = 0x10;
    /// End of element.
    pub const ENDEL: u8 = 0x11;
    /// Referenced structure name.
    pub const SNAME: u8 = 0x12;
    /// Array columns and rows.
    pub const COLROW: u8 = 0x13;
    /// Text type number.
    pub const TEXTTYPE: u8 = 0x16;
    /// Text string.
    pub const STRING: u8 = 0x19;
    /// Transform flags (bit 0 of the first byte = mirror about X axis).
    pub const STRANS: u8 = 0x1a;
    /// Magnification factor.
    pub const MAG: u8 = 0x1b;
    /// Rotation angle in degrees, counterclockwise.
    pub const ANGLE: u8 = 0x1c;
    /// Path end style (0 = flush, 1 = round, 2 = half-square extension).
    pub const PATHTYPE: u8 = 0x21;
}

/// Data type constants (the fourth header byte).
mod dtype {
    pub const NONE: u8 = 0x00;
    pub const BITARRAY: u8 = 0x01;
    pub const I16: u8 = 0x02;
    pub const I32: u8 = 0x03;
    pub const REAL8: u8 = 0x05;
    pub const ASCII: u8 = 0x06;
}

/// A decoded GDSII record.
#[derive(Debug, Clone, PartialEq)]
pub enum Record {
    /// HEADER — stream format version (e.g. 600 for release 6).
    Header(i16),
    /// BGNLIB — twelve i16 timestamp words (last modified + last accessed).
    BgnLib([i16; 12]),
    /// LIBNAME — library name.
    LibName(String),
    /// UNITS — database unit in user units, database unit in meters.
    Units {
        /// Size of a database unit expressed in user units.
        user_unit: f64,
        /// Size of a database unit in meters.
        db_unit_in_meters: f64,
    },
    /// ENDLIB — end of library.
    EndLib,
    /// BGNSTR — twelve i16 timestamp words (creation + last modified).
    BgnStr([i16; 12]),
    /// STRNAME — structure (cell) name.
    StrName(String),
    /// ENDSTR — end of structure.
    EndStr,
    /// BOUNDARY — begin polygon element.
    Boundary,
    /// PATH — begin path element.
    Path,
    /// SREF — begin structure reference element.
    Sref,
    /// AREF — begin array reference element.
    Aref,
    /// TEXT — begin text element.
    Text,
    /// LAYER — layer number.
    Layer(i16),
    /// DATATYPE — datatype number.
    Datatype(i16),
    /// WIDTH — path width in database units (negative = absolute).
    Width(i32),
    /// XY — list of coordinate pairs in database units.
    Xy(Vec<(i32, i32)>),
    /// ENDEL — end of element.
    EndEl,
    /// SNAME — referenced structure name.
    Sname(String),
    /// COLROW — array reference columns and rows.
    ColRow {
        /// Number of columns.
        cols: i16,
        /// Number of rows.
        rows: i16,
    },
    /// TEXTTYPE — text type number.
    TextType(i16),
    /// STRING — text element string.
    TextString(String),
    /// STRANS — transform flag word (bit 15 / mask 0x8000 = mirror about X).
    Strans(u16),
    /// MAG — magnification factor.
    Mag(f64),
    /// ANGLE — rotation angle in degrees, counterclockwise.
    Angle(f64),
    /// PATHTYPE — path end style.
    PathType(i16),
    /// Any record type this crate does not model (ELFLAGS, PLEX,
    /// PROPATTR, …). Carries the raw type byte so callers can skip it.
    Unknown(u8),
}

fn be_i16(b: &[u8]) -> i16 {
    i16::from_be_bytes([b[0], b[1]])
}

fn be_i32(b: &[u8]) -> i32 {
    i32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

fn decode_string(payload: &[u8]) -> String {
    let end = payload.iter().rposition(|&b| b != 0).map_or(0, |ix| ix + 1);
    String::from_utf8_lossy(&payload[..end]).into_owned()
}

fn expect_len(rectype: u8, payload: &[u8], len: usize) -> Result<()> {
    if payload.len() != len {
        return Err(GdsError::InvalidRecord {
            rectype,
            reason: format!("expected {len}-byte payload, got {}", payload.len()),
        });
    }
    Ok(())
}

fn timestamps(rectype: u8, payload: &[u8]) -> Result<[i16; 12]> {
    expect_len(rectype, payload, 24)?;
    let mut out = [0i16; 12];
    for (i, w) in out.iter_mut().enumerate() {
        *w = be_i16(&payload[i * 2..]);
    }
    Ok(out)
}

/// A cursor over a GDSII byte stream that yields decoded [`Record`]s.
#[derive(Debug)]
pub struct RecordReader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> RecordReader<'a> {
    /// Create a reader over `bytes`.
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    /// Read the next record, or `None` at a clean end of stream.
    ///
    /// Trailing NUL padding after ENDLIB (common on tape-image files) is
    /// treated as end of stream.
    pub fn next_record(&mut self) -> Result<Option<Record>> {
        // Skip trailing zero padding.
        while self.pos + 4 <= self.bytes.len()
            && self.bytes[self.pos] == 0
            && self.bytes[self.pos + 1] == 0
        {
            self.pos += 2;
        }
        if self.pos >= self.bytes.len() {
            return Ok(None);
        }
        if self.pos + 4 > self.bytes.len() {
            return Err(GdsError::UnexpectedEof);
        }

        let len = u16::from_be_bytes([self.bytes[self.pos], self.bytes[self.pos + 1]]) as usize;
        let rectype = self.bytes[self.pos + 2];
        if len < 4 || self.pos + len > self.bytes.len() {
            return Err(GdsError::UnexpectedEof);
        }
        let payload = &self.bytes[self.pos + 4..self.pos + len];
        self.pos += len;

        let record = match rectype {
            rectype::HEADER => {
                expect_len(rectype, payload, 2)?;
                Record::Header(be_i16(payload))
            }
            rectype::BGNLIB => Record::BgnLib(timestamps(rectype, payload)?),
            rectype::LIBNAME => Record::LibName(decode_string(payload)),
            rectype::UNITS => {
                expect_len(rectype, payload, 16)?;
                let mut a = [0u8; 8];
                let mut b = [0u8; 8];
                a.copy_from_slice(&payload[..8]);
                b.copy_from_slice(&payload[8..]);
                Record::Units {
                    user_unit: real8_to_f64(a),
                    db_unit_in_meters: real8_to_f64(b),
                }
            }
            rectype::ENDLIB => Record::EndLib,
            rectype::BGNSTR => Record::BgnStr(timestamps(rectype, payload)?),
            rectype::STRNAME => Record::StrName(decode_string(payload)),
            rectype::ENDSTR => Record::EndStr,
            rectype::BOUNDARY => Record::Boundary,
            rectype::PATH => Record::Path,
            rectype::SREF => Record::Sref,
            rectype::AREF => Record::Aref,
            rectype::TEXT => Record::Text,
            rectype::LAYER => {
                expect_len(rectype, payload, 2)?;
                Record::Layer(be_i16(payload))
            }
            rectype::DATATYPE => {
                expect_len(rectype, payload, 2)?;
                Record::Datatype(be_i16(payload))
            }
            rectype::WIDTH => {
                expect_len(rectype, payload, 4)?;
                Record::Width(be_i32(payload))
            }
            rectype::XY => {
                if !payload.len().is_multiple_of(8) {
                    return Err(GdsError::InvalidRecord {
                        rectype,
                        reason: format!("XY payload of {} bytes is not 8-aligned", payload.len()),
                    });
                }
                let pairs = payload
                    .chunks_exact(8)
                    .map(|c| (be_i32(&c[0..4]), be_i32(&c[4..8])))
                    .collect();
                Record::Xy(pairs)
            }
            rectype::ENDEL => Record::EndEl,
            rectype::SNAME => Record::Sname(decode_string(payload)),
            rectype::COLROW => {
                expect_len(rectype, payload, 4)?;
                Record::ColRow {
                    cols: be_i16(&payload[0..2]),
                    rows: be_i16(&payload[2..4]),
                }
            }
            rectype::TEXTTYPE => {
                expect_len(rectype, payload, 2)?;
                Record::TextType(be_i16(payload))
            }
            rectype::STRING => Record::TextString(decode_string(payload)),
            rectype::STRANS => {
                expect_len(rectype, payload, 2)?;
                Record::Strans(u16::from_be_bytes([payload[0], payload[1]]))
            }
            rectype::MAG => {
                expect_len(rectype, payload, 8)?;
                let mut b = [0u8; 8];
                b.copy_from_slice(payload);
                Record::Mag(real8_to_f64(b))
            }
            rectype::ANGLE => {
                expect_len(rectype, payload, 8)?;
                let mut b = [0u8; 8];
                b.copy_from_slice(payload);
                Record::Angle(real8_to_f64(b))
            }
            rectype::PATHTYPE => {
                expect_len(rectype, payload, 2)?;
                Record::PathType(be_i16(payload))
            }
            other => Record::Unknown(other),
        };
        Ok(Some(record))
    }
}

fn emit(out: &mut Vec<u8>, rectype: u8, dtype: u8, payload: &[u8]) {
    debug_assert!(
        payload.len().is_multiple_of(2),
        "GDSII payloads must be even-sized"
    );
    let len = (payload.len() + 4) as u16;
    out.extend_from_slice(&len.to_be_bytes());
    out.push(rectype);
    out.push(dtype);
    out.extend_from_slice(payload);
}

fn encode_string(s: &str) -> Vec<u8> {
    let mut bytes = s.as_bytes().to_vec();
    if !bytes.len().is_multiple_of(2) {
        bytes.push(0);
    }
    bytes
}

impl Record {
    /// Append this record's binary encoding to `out`.
    ///
    /// [`Record::Unknown`] cannot be written (its payload was discarded on
    /// read) and returns [`GdsError::Unencodable`].
    pub fn write(&self, out: &mut Vec<u8>) -> Result<()> {
        match self {
            Record::Header(v) => emit(out, rectype::HEADER, dtype::I16, &v.to_be_bytes()),
            Record::BgnLib(ts) => {
                let mut p = Vec::with_capacity(24);
                for w in ts {
                    p.extend_from_slice(&w.to_be_bytes());
                }
                emit(out, rectype::BGNLIB, dtype::I16, &p);
            }
            Record::LibName(s) => emit(out, rectype::LIBNAME, dtype::ASCII, &encode_string(s)),
            Record::Units {
                user_unit,
                db_unit_in_meters,
            } => {
                let mut p = Vec::with_capacity(16);
                p.extend_from_slice(&f64_to_real8(*user_unit)?);
                p.extend_from_slice(&f64_to_real8(*db_unit_in_meters)?);
                emit(out, rectype::UNITS, dtype::REAL8, &p);
            }
            Record::EndLib => emit(out, rectype::ENDLIB, dtype::NONE, &[]),
            Record::BgnStr(ts) => {
                let mut p = Vec::with_capacity(24);
                for w in ts {
                    p.extend_from_slice(&w.to_be_bytes());
                }
                emit(out, rectype::BGNSTR, dtype::I16, &p);
            }
            Record::StrName(s) => emit(out, rectype::STRNAME, dtype::ASCII, &encode_string(s)),
            Record::EndStr => emit(out, rectype::ENDSTR, dtype::NONE, &[]),
            Record::Boundary => emit(out, rectype::BOUNDARY, dtype::NONE, &[]),
            Record::Path => emit(out, rectype::PATH, dtype::NONE, &[]),
            Record::Sref => emit(out, rectype::SREF, dtype::NONE, &[]),
            Record::Aref => emit(out, rectype::AREF, dtype::NONE, &[]),
            Record::Text => emit(out, rectype::TEXT, dtype::NONE, &[]),
            Record::Layer(v) => emit(out, rectype::LAYER, dtype::I16, &v.to_be_bytes()),
            Record::Datatype(v) => emit(out, rectype::DATATYPE, dtype::I16, &v.to_be_bytes()),
            Record::Width(v) => emit(out, rectype::WIDTH, dtype::I32, &v.to_be_bytes()),
            Record::Xy(points) => {
                let mut p = Vec::with_capacity(points.len() * 8);
                for (x, y) in points {
                    p.extend_from_slice(&x.to_be_bytes());
                    p.extend_from_slice(&y.to_be_bytes());
                }
                emit(out, rectype::XY, dtype::I32, &p);
            }
            Record::EndEl => emit(out, rectype::ENDEL, dtype::NONE, &[]),
            Record::Sname(s) => emit(out, rectype::SNAME, dtype::ASCII, &encode_string(s)),
            Record::ColRow { cols, rows } => {
                let mut p = Vec::with_capacity(4);
                p.extend_from_slice(&cols.to_be_bytes());
                p.extend_from_slice(&rows.to_be_bytes());
                emit(out, rectype::COLROW, dtype::I16, &p);
            }
            Record::TextType(v) => emit(out, rectype::TEXTTYPE, dtype::I16, &v.to_be_bytes()),
            Record::TextString(s) => emit(out, rectype::STRING, dtype::ASCII, &encode_string(s)),
            Record::Strans(bits) => {
                emit(out, rectype::STRANS, dtype::BITARRAY, &bits.to_be_bytes())
            }
            Record::Mag(v) => emit(out, rectype::MAG, dtype::REAL8, &f64_to_real8(*v)?),
            Record::Angle(v) => emit(out, rectype::ANGLE, dtype::REAL8, &f64_to_real8(*v)?),
            Record::PathType(v) => emit(out, rectype::PATHTYPE, dtype::I16, &v.to_be_bytes()),
            Record::Unknown(t) => {
                return Err(GdsError::Unencodable(format!(
                    "unknown record type 0x{t:02x} cannot be written"
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(record: Record) {
        let mut bytes = Vec::new();
        record.write(&mut bytes).unwrap();
        let mut reader = RecordReader::new(&bytes);
        assert_eq!(reader.next_record().unwrap(), Some(record));
        assert_eq!(reader.next_record().unwrap(), None);
    }

    #[test]
    fn record_roundtrips() {
        roundtrip(Record::Header(600));
        roundtrip(Record::BgnLib([0; 12]));
        roundtrip(Record::LibName("mylib".into()));
        roundtrip(Record::Units {
            user_unit: 0.001,
            db_unit_in_meters: 1e-9,
        });
        roundtrip(Record::StrName("odd".into())); // odd length → padded
        roundtrip(Record::Layer(42));
        roundtrip(Record::Width(-250));
        roundtrip(Record::Xy(vec![(0, 0), (-100, 2_000_000_000)]));
        roundtrip(Record::ColRow { cols: 2, rows: 3 });
        roundtrip(Record::Strans(0x8000));
        roundtrip(Record::Mag(2.5));
        roundtrip(Record::Angle(-90.0));
        roundtrip(Record::PathType(2));
        roundtrip(Record::TextString("VDD".into()));
    }

    #[test]
    fn header_is_big_endian() {
        let mut bytes = Vec::new();
        Record::Header(600).write(&mut bytes).unwrap();
        assert_eq!(bytes, vec![0x00, 0x06, 0x00, 0x02, 0x02, 0x58]);
    }

    #[test]
    fn odd_string_is_nul_padded() {
        let mut bytes = Vec::new();
        Record::LibName("abc".into()).write(&mut bytes).unwrap();
        assert_eq!(bytes.len(), 8); // 4 header + "abc\0"
        assert_eq!(&bytes[4..], b"abc\0");
    }

    #[test]
    fn unknown_records_are_skippable() {
        let mut bytes = Vec::new();
        // ELFLAGS (0x26, i16) — not modeled, should surface as Unknown.
        bytes.extend_from_slice(&[0x00, 0x06, 0x26, 0x01, 0x00, 0x00]);
        Record::EndEl.write(&mut bytes).unwrap();
        let mut reader = RecordReader::new(&bytes);
        assert_eq!(reader.next_record().unwrap(), Some(Record::Unknown(0x26)));
        assert_eq!(reader.next_record().unwrap(), Some(Record::EndEl));
    }

    #[test]
    fn truncated_stream_errors() {
        let mut bytes = Vec::new();
        Record::Layer(1).write(&mut bytes).unwrap();
        bytes.pop();
        let mut reader = RecordReader::new(&bytes);
        assert!(matches!(reader.next_record(), Err(GdsError::UnexpectedEof)));
    }
}
