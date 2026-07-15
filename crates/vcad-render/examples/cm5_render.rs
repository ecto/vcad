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
use vcad_ir::ecad::{Trace, Via};
use vcad_render::pcb::render_pcb_svg;

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

    let text = std::fs::read_to_string(&path).expect("read kicad_pcb");
    let mut pcb = parse_kicad_pcb(&text).expect("parse kicad_pcb");

    // The human reference, exactly as imported.
    std::fs::write(
        format!("{out_dir}/human.svg"),
        render_pcb_svg(&pcb, &[], 20.0),
    )
    .expect("write human.svg");
    eprintln!("wrote {out_dir}/human.svg");

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
        counts.sort_by(|a, b| b.1.cmp(&a.1));
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

    std::fs::write(format!("{out_dir}/vcad.svg"), render_pcb_svg(&pcb, &[], 20.0))
        .expect("write vcad.svg");
    eprintln!("wrote {out_dir}/vcad.svg");
}
