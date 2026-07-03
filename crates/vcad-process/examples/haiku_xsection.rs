//! End-to-end PoC: run a simplified sky130-ish process flow over a real
//! GDS die and emit both deliverables:
//!
//! - `haiku_xsection.vcad` — cross-section cut horizontally through the
//!   die middle (y = 63.5 µm, x = 40..90 µm). Render with `--view front`.
//! - `haiku_stack3d.vcad` — 3D film stack over a 20×20 µm window. Render
//!   with `--view iso`.
//!
//! Usage: `haiku_xsection <in.gds> <out_dir>`
//!
//! Expects sky130-style layer numbers: diff=65, poly=66, li1=67,
//! met1=68, met2=69.

use vcad_gdsii::read_library;
use vcad_process::{cross_section, simulate_3d, Axis, CutLine, Polarity, ProcessStep, Recipe};

fn sky130ish_recipe() -> Recipe {
    use ProcessStep::*;
    Recipe {
        substrate_material: "silicon".into(),
        substrate_thickness_um: 0.8,
        steps: vec![
            // Field oxide, opened over active area, then dope the openings.
            GrowOxide { thickness_um: 0.3 },
            PatternEtch {
                mask_layer: 65,
                polarity: Polarity::RemoveMasked,
                depth_um: 0.3,
            },
            Implant {
                mask_layer: 65,
                dopant: "ndiff".into(),
                depth_um: 0.12,
            },
            // Poly gate.
            Deposit {
                material: "poly".into(),
                thickness_um: 0.18,
            },
            PatternEtch {
                mask_layer: 66,
                polarity: Polarity::KeepMasked,
                depth_um: 0.18,
            },
            // ILD + CMP, local interconnect.
            Deposit {
                material: "sio2".into(),
                thickness_um: 0.45,
            },
            Planarize { to_um: 0.75 },
            Deposit {
                material: "li".into(),
                thickness_um: 0.10,
            },
            PatternEtch {
                mask_layer: 67,
                polarity: Polarity::KeepMasked,
                depth_um: 0.10,
            },
            // ILD2 + CMP, metal 1.
            Deposit {
                material: "sio2".into(),
                thickness_um: 0.40,
            },
            Planarize { to_um: 1.20 },
            Deposit {
                material: "aluminum".into(),
                thickness_um: 0.36,
            },
            PatternEtch {
                mask_layer: 68,
                polarity: Polarity::KeepMasked,
                depth_um: 0.36,
            },
            // IMD + CMP, metal 2.
            Deposit {
                material: "sio2".into(),
                thickness_um: 0.55,
            },
            Planarize { to_um: 2.05 },
            Deposit {
                material: "aluminum".into(),
                thickness_um: 0.36,
            },
            PatternEtch {
                mask_layer: 69,
                polarity: Polarity::KeepMasked,
                depth_um: 0.36,
            },
        ],
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let gds_path = args.next().expect("arg 1: in.gds");
    let out_dir = std::path::PathBuf::from(args.next().expect("arg 2: out dir"));

    let bytes = std::fs::read(&gds_path).expect("read gds");
    let lib = read_library(&bytes).expect("parse gds");
    let top = lib.cells.last().map(|c| c.name.clone()).expect("cells");
    // Prefer a cell literally named like the design if present.
    let top = lib
        .cells
        .iter()
        .find(|c| c.name.contains("haiku"))
        .map(|c| c.name.clone())
        .unwrap_or(top);
    let recipe = sky130ish_recipe();

    // (a) Cross-section through the die middle.
    let cut = CutLine {
        axis: Axis::X,
        position_um: 63.5,
        span: [40.0, 90.0],
    };
    let section = cross_section(&lib, &top, &recipe, &cut).expect("cross section");
    let section_path = out_dir.join("haiku_xsection.vcad");
    std::fs::write(&section_path, section.to_json().expect("json")).expect("write");
    println!(
        "wrote {} ({} parts)",
        section_path.display(),
        section.roots.len()
    );

    // (b) 3D stack over a 20×20 µm window centered on the cut.
    let window = [54.0, 53.5, 74.0, 73.5];
    let stack = simulate_3d(&lib, &top, &recipe, Some(window)).expect("3d stack");
    let stack_path = out_dir.join("haiku_stack3d.vcad");
    std::fs::write(&stack_path, stack.to_json().expect("json")).expect("write");
    println!(
        "wrote {} ({} parts)",
        stack_path.display(),
        stack.roots.len()
    );
}
