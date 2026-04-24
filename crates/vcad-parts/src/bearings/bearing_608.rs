//! 608 skate bearing (8 × 22 × 7 mm).
//!
//! Simplified outer ring only, without balls / inner race / cage. Most CAD
//! consumers of a "608" just need the envelope for a shaft pass-through and
//! a housing bore.

use crate::types::{Param, Params, PartEntry, PartMetadata, Xref};
use crate::Builder;
use vcad_ir::Document;

/// Registry entry.
pub const ENTRY: PartEntry = PartEntry {
    meta: PartMetadata {
        id: "bearing.608",
        name: "Bearing 608 (8×22×7)",
        category: "Bearings",
        description: Some(
            "608 skate bearing: 8 mm bore, 22 mm outer, 7 mm wide. Simplified envelope.",
        ),
        params: &[Param::Boolean {
            name: "sealed",
            default: true,
        }],
        xrefs: &[Xref {
            params: &[],
            mcmaster: Some("6153K410"),
            iso: Some("ISO 15:2011"),
            din: None,
        }],
        synonyms: &["skate bearing", "608zz", "608-2RS"],
        version: "1.0",
        thumb: include_bytes!("../../thumbs/bearing-608.svg"),
    },
    build,
};

fn build(_p: &Params) -> Result<Document, String> {
    let bore = 8.0;
    let outer = 22.0;
    let width = 7.0;

    let mut b = Builder::new();
    let outer_cyl = b.cylinder(outer / 2.0, width);
    let bore_cyl = b.cylinder(bore / 2.0, width + 0.2);
    let bore_centered = b.translate(bore_cyl, 0.0, 0.0, -0.1);
    let root = b.difference(outer_cyl, bore_centered);
    Ok(b.finish(root, "steel"))
}
