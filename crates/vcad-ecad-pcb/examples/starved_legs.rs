//! Scratch: list pair legs whose copper is shorter than their own pad span.

use vcad_ecad_pcb::router::classes::classify_nets;
use vcad_ecad_pcb::router::length_match::net_routed_length;
use vcad_ecad_symbols::parse_kicad_pcb;
use vcad_ir::ecad::Pcb;
use vcad_ir::Vec2;

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: starved_legs <board>");
    let text = std::fs::read_to_string(&path).expect("read");
    let pcb: Pcb = if path.ends_with(".json") {
        serde_json::from_str(&text).expect("parse")
    } else {
        parse_kicad_pcb(&text).expect("parse")
    };
    let nets: Vec<String> = {
        let mut v: std::collections::BTreeSet<String> = Default::default();
        for f in &pcb.footprints {
            for pad in &f.pads {
                if let Some(n) = &pad.net {
                    if !n.is_empty() {
                        v.insert(n.clone());
                    }
                }
            }
        }
        v.into_iter().collect()
    };
    let c = classify_nets(&nets);
    let span_of = |net: &str| -> (f64, usize) {
        let mut pads: Vec<Vec2> = Vec::new();
        for fp in &pcb.footprints {
            for pad in &fp.pads {
                if pad.net.as_deref() == Some(net) {
                    pads.push(vcad_ecad_pcb::geometry::pad_world_position(fp, pad));
                }
            }
        }
        let mut span = 0.0f64;
        for (i, a) in pads.iter().enumerate() {
            for b in &pads[i + 1..] {
                span = span.max((*a - *b).length());
            }
        }
        (span, pads.len())
    };
    println!("{} pairs classified", c.pairs.len());
    for (p, n) in &c.pairs {
        for net in [p, n] {
            let (span, npads) = span_of(net);
            let l = net_routed_length(&pcb, net);
            if span > 1.0 && l < span * 0.999 {
                println!(
                    "  STARVED {net:28} copper {l:7.2}mm  span {span:7.2}mm  ratio {:.3}  pads {npads}",
                    l / span
                );
            }
        }
    }
}
