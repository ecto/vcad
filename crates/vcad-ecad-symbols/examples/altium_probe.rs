//! Probe the Altium importer against a real file: `cargo run -p
//! vcad-ecad-symbols --example altium_probe -- <file.PcbDoc|file.PcbLib>`.
use std::env;
use std::fs;

fn main() {
    for path in env::args().skip(1) {
        let bytes = fs::read(&path).expect("read");
        let name = path.rsplit('/').next().unwrap_or(&path);
        if path.to_lowercase().ends_with(".brd") {
            let text = String::from_utf8_lossy(&bytes).to_string();
            match vcad_ecad_symbols::parse_eagle_brd(&text) {
                Ok(pcb) => println!(
                    "OK   {name}: {} fps, {} nets, {} traces, {} vias, outline {} verts [{:.1}x{:.1} mm]",
                    pcb.footprints.len(),
                    pcb.nets.len(),
                    pcb.traces.len(),
                    pcb.vias.len(),
                    pcb.outline.vertices.len(),
                    pcb.outline.vertices.iter().map(|v| v.x).fold(f64::MIN, f64::max)
                        - pcb.outline.vertices.iter().map(|v| v.x).fold(f64::MAX, f64::min),
                    pcb.outline.vertices.iter().map(|v| v.y).fold(f64::MIN, f64::max)
                        - pcb.outline.vertices.iter().map(|v| v.y).fold(f64::MAX, f64::min),
                ),
                Err(e) => println!("FAIL {name}: {e}"),
            }
            continue;
        }
        if path.to_lowercase().ends_with(".pcblib") {
            match vcad_ecad_symbols::parse_altium_pcblib(&bytes) {
                Ok(lib) => {
                    println!("OK   {name}: {} patterns", lib.footprints.len());
                    for f in lib.footprints.iter().take(3) {
                        println!("       {} ({} pads)", f.name, f.pads.len());
                    }
                }
                Err(e) => println!("FAIL {name}: {e}"),
            }
            continue;
        }
        match vcad_ecad_symbols::parse_altium_pcbdoc(&bytes) {
            Ok(pcb) => {
                let (mut lo, mut hi) = ((f64::MAX, f64::MAX), (f64::MIN, f64::MIN));
                for v in &pcb.outline.vertices {
                    lo = (lo.0.min(v.x), lo.1.min(v.y));
                    hi = (hi.0.max(v.x), hi.1.max(v.y));
                }
                println!(
                    "OK   {name}: {} fps, {} nets, {} traces, {} arcs, {} vias, outline {} verts \
                     [{:.1}x{:.1} mm]",
                    pcb.footprints.len(),
                    pcb.nets.len(),
                    pcb.traces.len(),
                    pcb.trace_arcs.len(),
                    pcb.vias.len(),
                    pcb.outline.vertices.len(),
                    hi.0 - lo.0,
                    hi.1 - lo.1,
                );
                {
                    use std::collections::BTreeMap;
                    let mut hist: BTreeMap<String, usize> = BTreeMap::new();
                    for t in &pcb.traces {
                        *hist.entry(format!("{:?}", t.layer)).or_default() += 1;
                    }
                    println!("       trace layers: {hist:?}");
                    let mut vh: BTreeMap<String, usize> = BTreeMap::new();
                    for v in &pcb.vias {
                        *vh.entry(format!("{:?}->{:?}", v.start_layer, v.end_layer))
                            .or_default() += 1;
                    }
                    println!("       via spans:    {vh:?}");
                    let unnetted = pcb.traces.iter().filter(|t| t.net.is_empty()).count();
                    println!("       unnetted traces: {unnetted}");
                }
                let pads: usize = pcb.footprints.iter().map(|f| f.pads.len()).sum();
                println!(
                    "       {pads} pads, stackup {} copper layers",
                    pcb.stackup.layers.len()
                );
                for l in &pcb.stackup.layers {
                    println!(
                        "         {:?} cu={:?} diel={:?} er={:?} {:?}",
                        l.layer,
                        l.copper_thickness,
                        l.dielectric_thickness,
                        l.dielectric_er,
                        l.material
                    );
                }
            }
            Err(e) => println!("FAIL {name}: {e}"),
        }
    }
}
