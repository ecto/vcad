//! GDSII stream → [`Library`] parser.

use crate::error::{GdsError, Result};
use crate::model::{Cell, Element, Library, Strans};
use crate::record::{Record, RecordReader};

/// Parse a complete GDSII stream into a [`Library`].
///
/// Unmodeled records (ELFLAGS, PLEX, properties, …) are skipped. Errors on
/// truncated streams, malformed records, and missing required records.
pub fn read_library(bytes: &[u8]) -> Result<Library> {
    let mut reader = RecordReader::new(bytes);

    match reader.next_record()? {
        Some(Record::Header(_)) => {}
        _ => return Err(GdsError::MissingRecord("HEADER")),
    }

    let mut name: Option<String> = None;
    let mut units: Option<(f64, f64)> = None;
    let mut cells = Vec::new();
    let mut saw_bgnlib = false;
    let mut saw_endlib = false;

    while let Some(record) = reader.next_record()? {
        match record {
            Record::BgnLib(_) => saw_bgnlib = true,
            Record::LibName(s) => name = Some(s),
            Record::Units {
                user_unit,
                db_unit_in_meters,
            } => units = Some((user_unit, db_unit_in_meters)),
            Record::BgnStr(_) => cells.push(read_cell(&mut reader)?),
            Record::EndLib => {
                saw_endlib = true;
                break;
            }
            Record::Unknown(_) => {}
            other => {
                return Err(GdsError::InvalidRecord {
                    rectype: 0,
                    reason: format!("unexpected record at library level: {other:?}"),
                });
            }
        }
    }

    if !saw_bgnlib {
        return Err(GdsError::MissingRecord("BGNLIB"));
    }
    if !saw_endlib {
        return Err(GdsError::MissingRecord("ENDLIB"));
    }
    let (user_unit, db_unit_in_meters) = units.ok_or(GdsError::MissingRecord("UNITS"))?;

    Ok(Library {
        name: name.ok_or(GdsError::MissingRecord("LIBNAME"))?,
        user_unit,
        db_unit_in_meters,
        cells,
    })
}

fn read_cell(reader: &mut RecordReader<'_>) -> Result<Cell> {
    let mut name: Option<String> = None;
    let mut elements = Vec::new();

    while let Some(record) = reader.next_record()? {
        match record {
            Record::StrName(s) => name = Some(s),
            Record::EndStr => {
                return Ok(Cell {
                    name: name.ok_or(GdsError::MissingRecord("STRNAME"))?,
                    elements,
                });
            }
            Record::Boundary | Record::Path | Record::Sref | Record::Aref | Record::Text => {
                elements.push(read_element(reader, record)?);
            }
            Record::Unknown(_) => {}
            other => {
                return Err(GdsError::InvalidRecord {
                    rectype: 0,
                    reason: format!("unexpected record inside structure: {other:?}"),
                });
            }
        }
    }
    Err(GdsError::MissingRecord("ENDSTR"))
}

/// Mutable accumulator for the records that can appear inside an element.
#[derive(Default)]
struct ElementFields {
    layer: i16,
    datatype: i16,
    texttype: i16,
    pathtype: i16,
    width: i32,
    xy: Vec<(i32, i32)>,
    sname: Option<String>,
    string: Option<String>,
    colrow: Option<(i16, i16)>,
    strans: Strans,
}

fn read_element(reader: &mut RecordReader<'_>, kind: Record) -> Result<Element> {
    let mut f = ElementFields::default();

    loop {
        match reader.next_record()? {
            None => return Err(GdsError::MissingRecord("ENDEL")),
            Some(Record::EndEl) => break,
            Some(Record::Layer(v)) => f.layer = v,
            Some(Record::Datatype(v)) => f.datatype = v,
            Some(Record::TextType(v)) => f.texttype = v,
            Some(Record::PathType(v)) => f.pathtype = v,
            Some(Record::Width(v)) => f.width = v,
            Some(Record::Xy(points)) => f.xy = points,
            Some(Record::Sname(s)) => f.sname = Some(s),
            Some(Record::TextString(s)) => f.string = Some(s),
            Some(Record::ColRow { cols, rows }) => f.colrow = Some((cols, rows)),
            Some(Record::Strans(bits)) => f.strans.mirror_x = bits & 0x8000 != 0,
            Some(Record::Mag(v)) => f.strans.mag = v,
            Some(Record::Angle(v)) => f.strans.angle_deg = v,
            Some(Record::Unknown(_)) => {}
            Some(other) => {
                return Err(GdsError::InvalidRecord {
                    rectype: 0,
                    reason: format!("unexpected record inside element: {other:?}"),
                });
            }
        }
    }

    let one_point = |xy: &[(i32, i32)], what: &'static str| -> Result<(i32, i32)> {
        xy.first().copied().ok_or(GdsError::MissingRecord(what))
    };

    match kind {
        Record::Boundary => Ok(Element::Boundary {
            layer: f.layer,
            datatype: f.datatype,
            xy: f.xy,
        }),
        Record::Path => Ok(Element::Path {
            layer: f.layer,
            datatype: f.datatype,
            pathtype: f.pathtype,
            width: f.width,
            xy: f.xy,
        }),
        Record::Text => Ok(Element::Text {
            layer: f.layer,
            texttype: f.texttype,
            origin: one_point(&f.xy, "XY (TEXT)")?,
            strans: f.strans,
            string: f.string.ok_or(GdsError::MissingRecord("STRING"))?,
        }),
        Record::Sref => Ok(Element::Sref {
            sname: f.sname.ok_or(GdsError::MissingRecord("SNAME"))?,
            strans: f.strans,
            origin: one_point(&f.xy, "XY (SREF)")?,
        }),
        Record::Aref => {
            let (cols, rows) = f.colrow.ok_or(GdsError::MissingRecord("COLROW"))?;
            if f.xy.len() != 3 {
                return Err(GdsError::InvalidRecord {
                    rectype: crate::record::rectype::XY,
                    reason: format!("AREF requires exactly 3 XY points, got {}", f.xy.len()),
                });
            }
            Ok(Element::Aref {
                sname: f.sname.ok_or(GdsError::MissingRecord("SNAME"))?,
                strans: f.strans,
                cols,
                rows,
                xy: [f.xy[0], f.xy[1], f.xy[2]],
            })
        }
        _ => unreachable!("read_element called with a non-element record"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::writer::write_library;

    #[test]
    fn rejects_missing_header() {
        assert!(matches!(
            read_library(&[]),
            Err(GdsError::MissingRecord("HEADER"))
        ));
    }

    #[test]
    fn rejects_truncated_library() {
        let mut lib = Library::new("t");
        lib.cells.push(Cell::new("a"));
        let bytes = write_library(&lib).unwrap();
        // Chop off ENDLIB (4 bytes).
        let truncated = &bytes[..bytes.len() - 4];
        assert!(matches!(
            read_library(truncated),
            Err(GdsError::MissingRecord("ENDLIB"))
        ));
    }

    #[test]
    fn skips_unknown_records() {
        let mut lib = Library::new("t");
        lib.cells.push(Cell::new("a"));
        let bytes = write_library(&lib).unwrap();
        // Splice a GENERATIONS record (0x22, i16) right after HEADER (6 bytes).
        let mut spliced = bytes[..6].to_vec();
        spliced.extend_from_slice(&[0x00, 0x06, 0x22, 0x02, 0x00, 0x03]);
        spliced.extend_from_slice(&bytes[6..]);
        let parsed = read_library(&spliced).unwrap();
        assert_eq!(parsed, lib);
    }
}
