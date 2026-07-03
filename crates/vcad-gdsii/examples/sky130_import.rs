//! THROWAWAY: import an OpenLane/LibreLane sky130 GDS, flatten the top
//! cell, print per-layer polygon counts, and emit a .vcad document.
//!
//! ```sh
//! cargo run -p vcad-gdsii --example sky130_import -- input.gds output.vcad [TOP]
//! ```

use vcad_gdsii::{flatten, read_library, to_vcad_document, DEFAULT_VIEW_SCALE};

fn main() {
    let mut args = std::env::args().skip(1);
    let gds_path = args
        .next()
        .expect("usage: sky130_import <in.gds> <out.vcad> [top-cell]");
    let vcad_path = args
        .next()
        .expect("usage: sky130_import <in.gds> <out.vcad> [top-cell]");
    let top_arg = args.next();

    let bytes = std::fs::read(&gds_path).expect("read gds file");
    println!("read {} bytes from {gds_path}", bytes.len());

    let lib = read_library(&bytes).expect("parse gds");
    println!(
        "library `{}`: {} cells, db_unit = {} m",
        lib.name,
        lib.cells.len(),
        lib.db_unit_in_meters
    );

    // Top cell: explicit arg, else the cell no other cell references.
    let top = top_arg.unwrap_or_else(|| {
        let referenced: std::collections::HashSet<&str> = lib
            .cells
            .iter()
            .flat_map(|c| c.elements.iter())
            .filter_map(|e| match e {
                vcad_gdsii::Element::Sref { sname, .. } => Some(sname.as_str()),
                vcad_gdsii::Element::Aref { sname, .. } => Some(sname.as_str()),
                _ => None,
            })
            .collect();
        let tops: Vec<&str> = lib
            .cells
            .iter()
            .map(|c| c.name.as_str())
            .filter(|n| !referenced.contains(n))
            .collect();
        println!("unreferenced (top) cells: {tops:?}");
        tops.first().expect("no top cell found").to_string()
    });
    println!("flattening top cell `{top}`");

    let flat = match flatten(&lib, &top) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("FLATTEN ERROR: {e}");
            eprintln!("debug: {e:?}");
            std::process::exit(1);
        }
    };
    println!("flattened layers (gds layer -> polygons / vertices):");
    let mut total = 0usize;
    for lp in &flat {
        let verts: usize = lp.polygons.iter().map(|p| p.len()).sum();
        total += lp.polygons.len();
        println!(
            "  layer {:>3}: {:>6} polygons, {:>7} vertices",
            lp.layer,
            lp.polygons.len(),
            verts
        );
    }
    println!("total polygons: {total}");

    // sky130-ish film stack: (gds layer, z_bottom_um, thickness_um, name).
    // Z heights approximate the sky130A metal stack.
    let stack = [
        (65, 0.00, 0.12, "diff"),
        (66, 0.30, 0.18, "poly"),
        (67, 0.94, 0.10, "li1"),
        (68, 1.38, 0.36, "met1"),
        (69, 2.00, 0.36, "met2"),
        (70, 2.79, 0.85, "met3"),
        (71, 4.02, 0.85, "met4"),
        (72, 5.37, 1.26, "met5"),
    ];
    let doc = match to_vcad_document(&lib, &top, &stack, DEFAULT_VIEW_SCALE) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("BRIDGE ERROR: {e}");
            eprintln!("debug: {e:?}");
            std::process::exit(1);
        }
    };
    std::fs::write(&vcad_path, doc.to_json().expect("json")).expect("write vcad");
    println!("wrote {vcad_path}");
}
