//! Render the CM5 benchmark visually: the human-routed reference board and
//! the vcad-autorouted board, as SVGs — the eyeball companion to
//! vcad-ecad-pcb's `cm5_bench` scoreboard.
//!
//! ```bash
//! cargo run --release -p vcad-render --example cm5_render -- \
//!     CM5RevEng.kicad_pcb out_dir [effort] [max_nets]
//! ```
//!
//! Writes `out_dir/human.svg` (the reference routing) and `out_dir/vcad.svg`
//! (copper stripped, autorouted with the layer-aware router).

use vcad_ecad_pcb::router::{route_all_with_opts, RouteOptions};
use vcad_ecad_symbols::parse_kicad_pcb;
use vcad_ir::ecad::{Pcb, PcbLayer, Trace, Via};
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

/// Copper-first styling: no value labels, no net labels, keep the ratsnest
/// (open connections are part of the scoreboard's story).
fn render_opts() -> PcbRenderOpts {
    PcbRenderOpts {
        show_values: false,
        show_net_labels: false,
        ..Default::default()
    }
}

fn write_svg(out_dir: &str, name: &str, pcb: &Pcb) {
    std::fs::write(
        format!("{out_dir}/{name}.svg"),
        render_pcb_svg_opts(pcb, &render_layers(pcb), 12.0, &render_opts()),
    )
    .unwrap_or_else(|e| panic!("write {name}.svg: {e}"));
    eprintln!("wrote {out_dir}/{name}.svg");
}

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(path), Some(out_dir)) = (args.next(), args.next()) else {
        eprintln!("usage: cm5_render <board.kicad_pcb> <out_dir> [effort] [max_nets]");
        std::process::exit(2);
    };
    let effort: f64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(1.0);
    let max_nets: usize = args
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(usize::MAX);
    std::fs::create_dir_all(&out_dir).expect("create out_dir");

    let text = std::fs::read_to_string(&path).expect("read board file");
    // A saved `.pcb.json` (written below after routing) re-renders directly —
    // iterate on styling without paying for another routing run.
    let mut pcb = if path.ends_with(".pcb.json") {
        serde_json::from_str(&text).expect("parse pcb json")
    } else {
        parse_kicad_pcb(&text).expect("parse kicad_pcb")
    };

    // Zones are dropped for this visual demo: rendering a pour runs a poly2d
    // boolean per zone (128 here — it dominates the whole run; the DRC-side
    // broadphase fix was PR #536, the renderer still needs one), and with the
    // planes gone the router routes power nets as ordinary traces, which makes
    // a better picture than invisible plane stitching anyway.
    pcb.zones.clear();

    // The human reference, exactly as imported.
    write_svg(&out_dir, "human", &pcb);

    // `max_nets == 0`: render the imported reference only, skip routing.
    if max_nets == 0 {
        return;
    }

    // Strip the routed copper and autoroute (planes/zones stay: design intent).
    pcb.traces.clear();
    pcb.trace_arcs.clear();
    pcb.vias.clear();

    let filter: Vec<String> = if max_nets == usize::MAX {
        Vec::new()
    } else {
        let mut counts: Vec<(String, usize)> = pcb
            .nets
            .iter()
            .map(|n| {
                let pads = pcb
                    .footprints
                    .iter()
                    .flat_map(|f| f.pads.iter())
                    .filter(|p| p.net.as_deref() == Some(n.id.as_str()))
                    .count();
                (n.id.clone(), pads)
            })
            .filter(|(_, c)| *c >= 2)
            .collect();
        // Smallest nets first: this is a visual demo, so favor many quick
        // completions over the monster power nets (cm5_bench stresses those).
        counts.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
        counts.into_iter().take(max_nets).map(|(n, _)| n).collect()
    };

    let width = pcb.rules.default_rules.trace_width;
    let r = route_all_with_opts(
        &pcb,
        width,
        &filter,
        &RouteOptions {
            effort,
            ..Default::default()
        },
    );
    eprintln!(
        "routed {} / unrouted {} nets, {} segments, {} vias",
        r.routed_nets.len(),
        r.unrouted_nets.len(),
        r.traces.len(),
        r.vias.len()
    );

    // Fold the routed copper back into the board for rendering.
    for t in &r.traces {
        pcb.traces.push(Trace {
            start: t.start,
            end: t.end,
            width: t.width,
            layer: t.layer,
            net: t.net.clone(),
            source: None,
        });
    }
    let (start_layer, end_layer) = {
        let copper: Vec<_> = pcb
            .stackup
            .layers
            .iter()
            .map(|l| l.layer)
            .filter(|l| l.is_copper())
            .collect();
        (
            *copper.first().unwrap_or(&vcad_ir::ecad::PcbLayer::FCu),
            *copper.last().unwrap_or(&vcad_ir::ecad::PcbLayer::BCu),
        )
    };
    for v in &r.vias {
        pcb.vias.push(Via {
            position: v.position,
            diameter: pcb.rules.default_rules.via_diameter,
            drill: pcb.rules.default_rules.via_drill,
            start_layer,
            end_layer,
            net: v.net.clone(),
            source: None,
        });
    }

    write_svg(&out_dir, "vcad", &pcb);

    // Save the routed board so styling can be iterated without re-routing
    // (deserialize and call render_pcb_svg_opts directly).
    std::fs::write(
        format!("{out_dir}/vcad.pcb.json"),
        serde_json::to_string(&pcb).expect("serialize routed board"),
    )
    .expect("write vcad.pcb.json");
}
