//! [`Library`] → GDSII stream serializer.

use crate::error::Result;
use crate::model::{Element, Library, Strans};
use crate::record::Record;

/// Timestamp words written into BGNLIB/BGNSTR.
///
/// GDSII timestamps carry no semantic weight for layout data; a fixed value
/// keeps output byte-for-byte deterministic.
const TIMESTAMPS: [i16; 12] = [2026, 1, 1, 0, 0, 0, 2026, 1, 1, 0, 0, 0];

/// Serialize a [`Library`] to GDSII stream-format bytes.
///
/// Writes a release-6 stream (`HEADER 600`). Element transforms are only
/// emitted when they differ from the identity, so a written-then-read
/// library compares equal to the original.
pub fn write_library(lib: &Library) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    Record::Header(600).write(&mut out)?;
    Record::BgnLib(TIMESTAMPS).write(&mut out)?;
    Record::LibName(lib.name.clone()).write(&mut out)?;
    Record::Units {
        user_unit: lib.user_unit,
        db_unit_in_meters: lib.db_unit_in_meters,
    }
    .write(&mut out)?;

    for cell in &lib.cells {
        Record::BgnStr(TIMESTAMPS).write(&mut out)?;
        Record::StrName(cell.name.clone()).write(&mut out)?;
        for element in &cell.elements {
            write_element(element, &mut out)?;
        }
        Record::EndStr.write(&mut out)?;
    }

    Record::EndLib.write(&mut out)?;
    Ok(out)
}

fn write_strans(strans: &Strans, out: &mut Vec<u8>) -> Result<()> {
    if strans.is_identity() {
        return Ok(());
    }
    let bits = if strans.mirror_x { 0x8000 } else { 0 };
    Record::Strans(bits).write(out)?;
    if strans.mag != 1.0 {
        Record::Mag(strans.mag).write(out)?;
    }
    if strans.angle_deg != 0.0 {
        Record::Angle(strans.angle_deg).write(out)?;
    }
    Ok(())
}

fn write_element(element: &Element, out: &mut Vec<u8>) -> Result<()> {
    match element {
        Element::Boundary {
            layer,
            datatype,
            xy,
        } => {
            Record::Boundary.write(out)?;
            Record::Layer(*layer).write(out)?;
            Record::Datatype(*datatype).write(out)?;
            Record::Xy(xy.clone()).write(out)?;
        }
        Element::Path {
            layer,
            datatype,
            pathtype,
            width,
            xy,
        } => {
            Record::Path.write(out)?;
            Record::Layer(*layer).write(out)?;
            Record::Datatype(*datatype).write(out)?;
            if *pathtype != 0 {
                Record::PathType(*pathtype).write(out)?;
            }
            if *width != 0 {
                Record::Width(*width).write(out)?;
            }
            Record::Xy(xy.clone()).write(out)?;
        }
        Element::Text {
            layer,
            texttype,
            origin,
            strans,
            string,
        } => {
            Record::Text.write(out)?;
            Record::Layer(*layer).write(out)?;
            Record::TextType(*texttype).write(out)?;
            write_strans(strans, out)?;
            Record::Xy(vec![*origin]).write(out)?;
            Record::TextString(string.clone()).write(out)?;
        }
        Element::Sref {
            sname,
            strans,
            origin,
        } => {
            Record::Sref.write(out)?;
            Record::Sname(sname.clone()).write(out)?;
            write_strans(strans, out)?;
            Record::Xy(vec![*origin]).write(out)?;
        }
        Element::Aref {
            sname,
            strans,
            cols,
            rows,
            xy,
        } => {
            Record::Aref.write(out)?;
            Record::Sname(sname.clone()).write(out)?;
            write_strans(strans, out)?;
            Record::ColRow {
                cols: *cols,
                rows: *rows,
            }
            .write(out)?;
            Record::Xy(xy.to_vec()).write(out)?;
        }
    }
    Record::EndEl.write(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Cell;
    use crate::reader::read_library;

    /// Build the reference library used by the round-trip test: two cells,
    /// an SREF with rotation + mirror, a PATH, and a 2×3 AREF.
    fn sample_library() -> Library {
        let mut unit = Cell::new("unit");
        unit.elements.push(Element::Boundary {
            layer: 1,
            datatype: 0,
            xy: vec![(0, 0), (1000, 0), (1000, 1000), (0, 1000), (0, 0)],
        });
        unit.elements.push(Element::Path {
            layer: 2,
            datatype: 5,
            pathtype: 0,
            width: 100,
            xy: vec![(0, 500), (2000, 500), (2000, 2500)],
        });
        unit.elements.push(Element::Text {
            layer: 63,
            texttype: 0,
            origin: (500, 500),
            strans: Strans {
                mirror_x: false,
                mag: 2.0,
                angle_deg: 0.0,
            },
            string: "VDD".to_string(),
        });

        let mut top = Cell::new("top");
        top.elements.push(Element::Sref {
            sname: "unit".to_string(),
            strans: Strans {
                mirror_x: true,
                mag: 1.0,
                angle_deg: 90.0,
            },
            origin: (5000, -3000),
        });
        top.elements.push(Element::Aref {
            sname: "unit".to_string(),
            strans: Strans::default(),
            cols: 2,
            rows: 3,
            xy: [(0, 0), (4000, 0), (0, 9000)],
        });

        let mut lib = Library::new("testlib");
        lib.cells.push(unit);
        lib.cells.push(top);
        lib
    }

    #[test]
    fn library_roundtrip() {
        let lib = sample_library();
        let bytes = write_library(&lib).unwrap();
        let parsed = read_library(&bytes).unwrap();
        assert_eq!(parsed, lib);
    }

    #[test]
    fn output_is_deterministic() {
        let lib = sample_library();
        assert_eq!(write_library(&lib).unwrap(), write_library(&lib).unwrap());
    }

    #[test]
    fn units_survive_roundtrip_exactly() {
        let mut lib = Library::new("u");
        lib.user_unit = 0.001;
        lib.db_unit_in_meters = 1e-9;
        let parsed = read_library(&write_library(&lib).unwrap()).unwrap();
        assert_eq!(parsed.user_unit, 0.001);
        assert_eq!(parsed.db_unit_in_meters, 1e-9);
    }
}
