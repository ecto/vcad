//! Hex nut (ISO 4032 / DIN 934).
//!
//! Geometry: six-sided prism with a threaded-through bore, centered at origin.

use crate::types::{Param, PartEntry, PartMetadata, Params, Xref};
use crate::Builder;
use vcad_ir::Document;

const SIZES: &[&str] = &["M2", "M3", "M4", "M5", "M6", "M8", "M10", "M12", "M16", "M20"];

/// Registry entry.
pub const ENTRY: PartEntry = PartEntry {
    meta: PartMetadata {
        id: "fastener.nut.hex",
        name: "Nut (hex)",
        category: "Fasteners",
        description: Some("ISO 4032 / DIN 934 hex nut."),
        params: &[Param::Enum {
            name: "size",
            values: SIZES,
            default: "M6",
        }],
        xrefs: &[
            Xref {
                params: &[("size", "M3")],
                mcmaster: Some("91828A211"),
                iso: Some("ISO 4032"),
                din: Some("DIN 934"),
            },
            Xref {
                params: &[("size", "M6")],
                mcmaster: Some("91828A251"),
                iso: Some("ISO 4032"),
                din: Some("DIN 934"),
            },
        ],
        synonyms: &["hex nut", "machine nut"],
        version: "1.0",
        thumb: include_bytes!("../../thumbs/nut-hex.svg"),
    },
    build,
};

fn build(p: &Params) -> Result<Document, String> {
    let size = p.str("size");
    let across_flats = super::nut_across_flats(&size);
    let thickness = super::nut_thickness(&size);
    let bore_dia = super::metric_major_dia(&size) + 0.2;

    let mut b = Builder::new();

    // Hex prism via a 6-segment cylinder. Radius = half across-flats / cos(30°).
    let hex_circum_r = (across_flats / 2.0) / 0.866_025_403_784_438_7;
    let body = b.cylinder_segments(hex_circum_r, thickness, 6);

    let bore = b.cylinder(bore_dia / 2.0, thickness + 0.2);
    let bore_centered = b.translate(bore, 0.0, 0.0, -0.1);

    let root = b.difference(body, bore_centered);
    Ok(b.finish(root, "steel"))
}
