//! Socket-head cap screw (ISO 4762 / DIN 912).
//!
//! Geometry: cylindrical head with hex socket on top, cylindrical shaft
//! extending downward along −Z from the head underside. Centered on the Z
//! axis; the head's top face sits at Z=0.

use crate::types::{Param, PartEntry, PartMetadata, Params, Xref};
use crate::Builder;
use vcad_ir::{Document, Vec3};

const SIZES: &[&str] = &["M2", "M3", "M4", "M5", "M6", "M8", "M10", "M12"];

/// Registry entry.
pub const ENTRY: PartEntry = PartEntry {
    meta: PartMetadata {
        id: "fastener.bolt.socket-head",
        name: "Bolt (socket head)",
        category: "Fasteners",
        description: Some("ISO 4762 / DIN 912 socket-head cap screw."),
        params: &[
            Param::Enum {
                name: "size",
                values: SIZES,
                default: "M6",
            },
            Param::Length {
                name: "length",
                min: 4.0,
                max: 200.0,
                default: 20.0,
                unit: "mm",
            },
        ],
        xrefs: &[
            Xref {
                params: &[("size", "M3"), ("length", "10")],
                mcmaster: Some("91290A115"),
                iso: Some("ISO 4762"),
                din: Some("DIN 912"),
            },
            Xref {
                params: &[("size", "M6"), ("length", "20")],
                mcmaster: Some("91290A320"),
                iso: Some("ISO 4762"),
                din: Some("DIN 912"),
            },
            Xref {
                params: &[("size", "M8"), ("length", "25")],
                mcmaster: Some("91290A420"),
                iso: Some("ISO 4762"),
                din: Some("DIN 912"),
            },
        ],
        synonyms: &["SHCS", "allen bolt", "cap screw", "hex socket screw"],
        version: "1.0",
        thumb: include_bytes!("../../thumbs/bolt-socket-head.svg"),
    },
    build,
};

fn build(p: &Params) -> Result<Document, String> {
    let size = p.str("size");
    let length = p.f64("length");

    let shaft_dia = super::metric_major_dia(&size);
    let head_dia = super::socket_head_dia(&size);
    let head_h = super::socket_head_height(&size);
    let hex_w = super::socket_hex_width(&size);

    let mut b = Builder::new();

    // Head sits with its base at Z=0 and extends up to +head_h.
    let head = b.cylinder(head_dia / 2.0, head_h);

    // Hex socket: approximate by a 6-sided "cylinder" (use the segments
    // override so the profile is a hexagon). Diameter = hex_w across flats.
    // Depth: half the head height.
    let hex_depth = head_h * 0.5;
    let hex_socket = b.cylinder_segments(hex_w / 2.0 / 0.866, hex_depth + 0.1, 6);
    let hex_top = b.translate(hex_socket, 0.0, 0.0, head_h - hex_depth);
    let head_with_socket = b.difference(head, hex_top);

    // Shaft: drops below Z=0 to Z = -length.
    let shaft = b.cylinder(shaft_dia / 2.0, length);
    let shaft_positioned = b.translate(shaft, 0.0, 0.0, -length);

    let root = b.union(head_with_socket, shaft_positioned);
    // Touch Vec3 import so the builder stays convenient for derived parts.
    let _warm: Vec3 = Vec3::new(0.0, 0.0, 0.0);

    Ok(b.finish(root, "steel"))
}
