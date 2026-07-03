//! End-to-end demo: build a tiny NMOS-style die in memory, write it as
//! GDSII, read it back, flatten the hierarchy, and emit a .vcad document.
//!
//! ```sh
//! cargo run -p vcad-gdsii --example haiku_die -- /tmp/haiku_die
//! ```
//! writes `/tmp/haiku_die.gds` and `/tmp/haiku_die.vcad`.

use vcad_gdsii::{
    flatten, read_library, to_vcad_document, write_library, Cell, Element, Library, Strans,
    DEFAULT_VIEW_SCALE,
};

/// Closed rectangular boundary on `layer`, coords in database units (nm).
fn rect(layer: i16, x0: i32, y0: i32, x1: i32, y1: i32) -> Element {
    Element::Boundary {
        layer,
        datatype: 0,
        xy: vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1), (x0, y0)],
    }
}

fn main() {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "haiku_die".into());

    // Layers of a garage-scale NMOS process.
    const DIFF: i16 = 1;
    const POLY: i16 = 2;
    const CONTACT: i16 = 3;
    const METAL1: i16 = 4;

    // One inverter-ish cell, dimensions in nm (µm-class feature sizes).
    let mut inv = Cell::new("INV");
    inv.elements.push(rect(DIFF, 0, 0, 30_000, 12_000));
    inv.elements
        .push(rect(POLY, 13_000, -4_000, 17_000, 16_000));
    inv.elements.push(rect(CONTACT, 3_000, 4_000, 7_000, 8_000));
    inv.elements
        .push(rect(CONTACT, 23_000, 4_000, 27_000, 8_000));
    inv.elements.push(rect(METAL1, 2_000, 2_000, 8_000, 10_000));
    inv.elements
        .push(rect(METAL1, 22_000, 2_000, 28_000, 10_000));

    // Top cell: 4x3 array of inverters, one rotated instance, a metal bus.
    let mut top = Cell::new("TOP");
    top.elements.push(Element::Aref {
        sname: "INV".into(),
        strans: Strans::default(),
        cols: 4,
        rows: 3,
        xy: [(10_000, 10_000), (170_000, 10_000), (10_000, 100_000)],
    });
    top.elements.push(Element::Sref {
        sname: "INV".into(),
        strans: Strans {
            mirror_x: false,
            mag: 1.0,
            angle_deg: 90.0,
        },
        origin: (200_000, 10_000),
    });
    top.elements.push(Element::Path {
        layer: METAL1,
        datatype: 0,
        pathtype: 0,
        width: 6_000,
        xy: vec![(0, -15_000), (220_000, -15_000)],
    });

    let mut lib = Library::new("LOON1");
    lib.user_unit = 0.001; // 1 db unit = 0.001 user units (µm)
    lib.db_unit_in_meters = 1e-9; // 1 db unit = 1 nm
    lib.cells.push(inv);
    lib.cells.push(top);

    // Write GDSII, then read our own bytes back — the full round trip.
    let bytes = write_library(&lib).expect("write gds");
    let gds_path = format!("{out}.gds");
    std::fs::write(&gds_path, &bytes).expect("write gds file");
    let lib2 = read_library(&bytes).expect("read gds back");

    let flat = flatten(&lib2, "TOP").expect("flatten");
    println!("flattened layers:");
    for lp in &flat {
        println!("  layer {:>2}: {:>3} polygons", lp.layer, lp.polygons.len());
    }

    // Physical film stack: (gds layer, z_bottom_um, thickness_um, name).
    let stack = [
        (DIFF, 0.0, 0.5, "diffusion"),
        (CONTACT, 0.5, 1.5, "contact"),
        (POLY, 0.8, 0.7, "poly"),
        (METAL1, 2.0, 1.0, "metal1"),
    ];
    let doc = to_vcad_document(&lib2, "TOP", &stack, DEFAULT_VIEW_SCALE).expect("bridge");
    let vcad_path = format!("{out}.vcad");
    std::fs::write(&vcad_path, doc.to_json().expect("json")).expect("write vcad file");
    println!("wrote {gds_path} and {vcad_path}");
}
