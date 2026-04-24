//! Flat washer (ISO 7089).
//!
//! Geometry: a thin disc with a concentric hole, centered at origin, axis = Z.

use crate::types::{Param, PartEntry, PartMetadata, Params, Xref};
use crate::Builder;
use vcad_ir::Document;

const SIZES: &[&str] = &["M2", "M3", "M4", "M5", "M6", "M8", "M10", "M12", "M16", "M20"];

/// Registry entry.
pub const ENTRY: PartEntry = PartEntry {
    meta: PartMetadata {
        id: "fastener.washer.flat",
        name: "Washer (flat)",
        category: "Fasteners",
        description: Some("ISO 7089 flat washer. Outer and inner diameter follow ISO standard."),
        params: &[Param::Enum {
            name: "size",
            values: SIZES,
            default: "M6",
        }],
        xrefs: &[
            Xref {
                params: &[("size", "M3")],
                mcmaster: Some("98032A210"),
                iso: Some("ISO 7089"),
                din: Some("DIN 125"),
            },
            Xref {
                params: &[("size", "M6")],
                mcmaster: Some("98032A420"),
                iso: Some("ISO 7089"),
                din: Some("DIN 125"),
            },
            Xref {
                params: &[("size", "M8")],
                mcmaster: Some("98032A440"),
                iso: Some("ISO 7089"),
                din: Some("DIN 125"),
            },
        ],
        synonyms: &["plain washer", "flat washer"],
        version: "1.0",
        thumb: include_bytes!("../../thumbs/washer-flat.svg"),
    },
    build,
};

fn build(p: &Params) -> Result<Document, String> {
    let size = p.str("size");
    let outer = super::washer_outer_dia(&size);
    let inner = super::metric_major_dia(&size) + 0.4;
    let thickness = super::washer_thickness(&size);

    if outer <= inner {
        return Err(format!("washer: inner diameter exceeds outer for size {size}"));
    }

    let mut b = Builder::new();
    let outer_cyl = b.cylinder(outer / 2.0, thickness);
    let inner_cyl = b.cylinder(inner / 2.0, thickness + 0.2);
    let hole_centered = b.translate(inner_cyl, 0.0, 0.0, -0.1);
    let root = b.difference(outer_cyl, hole_centered);

    Ok(b.finish(root, "steel"))
}
