//! Render a PCB to a copper-first SVG — the visual companion to
//! vcad-ecad-pcb's `cm5_bench` scoreboard.
//!
//! Rendering is deliberately decoupled from routing: `cm5_bench` routes and
//! (with its `out.pcb.json` argument) saves the routed board; this renders
//! any board file, so styling iterates without paying for a routing run.
//!
//! ```bash
//! # the human-routed reference
//! cargo run --release -p vcad-render --example cm5_render -- CM5RevEng.kicad_pcb human.svg
//! # a routed board saved by cm5_bench
//! cargo run --release -p vcad-render --example cm5_render -- routed.pcb.json vcad.svg
//! ```

use vcad_ecad_symbols::parse_kicad_pcb;
use vcad_ir::ecad::{Pcb, PcbLayer};
use vcad_render::pcb::{render_pcb_svg_opts, PcbRenderOpts};

/// Every copper layer on the board plus the outline, in z-order. No
/// silkscreen: on a 479-footprint board the refdes/value text is a solid
/// wall of glyphs that buries the copper — the copper is the story here.
fn render_layers(pcb: &Pcb) -> Vec<PcbLayer> {
    let mut layers: Vec<PcbLayer> = pcb
        .stackup
        .layers
        .iter()
        .map(|l| l.layer)
        .filter(|l| l.is_copper())
        .collect();
    layers.push(PcbLayer::EdgeCuts);
    layers
}

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(input), Some(output)) = (args.next(), args.next()) else {
        eprintln!("usage: cm5_render <board.kicad_pcb | board.pcb.json> <out.svg>");
        std::process::exit(2);
    };

    let text = std::fs::read_to_string(&input).expect("read board file");
    let mut pcb: Pcb = if input.ends_with(".json") {
        serde_json::from_str(&text).expect("parse pcb json")
    } else {
        parse_kicad_pcb(&text).expect("parse kicad_pcb")
    };

    // Zone pours are skipped: rendering a pour runs a poly2d boolean per zone
    // (128 on the CM5 — it dominates the whole run; the DRC-side broadphase
    // fix was PR #536, the renderer still needs one).
    pcb.zones.clear();

    // Copper-first styling: no value labels, no net labels, keep the ratsnest
    // (open connections are part of the scoreboard's story).
    let opts = PcbRenderOpts {
        show_values: false,
        show_net_labels: false,
        ..Default::default()
    };
    std::fs::write(
        &output,
        render_pcb_svg_opts(&pcb, &render_layers(&pcb), 12.0, &opts),
    )
    .expect("write svg");
    eprintln!("wrote {output}");
}
