//! Export a routed board (`.pcb.json` from cm5_bench) to every supported
//! fabrication/interchange format: Gerbers, Excellon drills, KiCad board,
//! BOM CSV, and pick-and-place CSV.
//!
//! ```bash
//! cargo run --release -p vcad-ecad-pcb --example board_export -- routed.pcb.json out_dir/
//! ```

use std::fs;
use std::path::Path;
use vcad_ir::ecad::Pcb;

fn main() {
    let mut args = std::env::args().skip(1);
    let input = args
        .next()
        .expect("usage: board_export <board.pcb.json> <out_dir>");
    let out = args
        .next()
        .expect("usage: board_export <board.pcb.json> <out_dir>");
    let out = Path::new(&out);
    fs::create_dir_all(out).expect("create out dir");

    let pcb: Pcb = serde_json::from_str(&fs::read_to_string(&input).expect("read board"))
        .expect("parse board json");

    let gerbers = vcad_ecad_export::gerber::generate_gerbers(&pcb).expect("gerbers");
    for (name, content) in &gerbers {
        fs::write(out.join(name), content).expect("write gerber");
    }
    println!("gerbers: {} files", gerbers.len());

    let drills = vcad_ecad_export::excellon::generate_drill_files(&pcb).expect("excellon");
    for (name, content) in &drills {
        fs::write(out.join(name), content).expect("write drill");
    }
    println!("drill files: {} spans", drills.len());

    fs::write(
        out.join("board.kicad_pcb"),
        vcad_ecad_symbols::write_kicad_pcb(&pcb),
    )
    .expect("write kicad");

    let mut bom = Vec::new();
    vcad_ecad_export::bom::write_bom(&mut bom, &pcb).expect("bom");
    fs::write(out.join("bom.csv"), &bom).expect("write bom");

    let mut pnp = Vec::new();
    vcad_ecad_export::pick_place::write_pick_place(&mut pnp, &pcb).expect("pick place");
    fs::write(out.join("pick_place.csv"), &pnp).expect("write pnp");

    println!("exported to {}", out.display());
}
