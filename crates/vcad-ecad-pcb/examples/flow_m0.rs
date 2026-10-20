//! Flow router M0: build the cut graph of a routed board with its copper
//! removed, map the existing (human) routing onto it, and compare per-cut
//! load against capacity.
//!
//! ```bash
//! cargo run --release -p vcad-ecad-pcb --example flow_m0 -- board.pcb.json [out.json]
//! ```
//! `VCAD_FLOW_CLEARANCE` / `VCAD_FLOW_WIDTH` override the spacing / width model.

use std::time::Instant;

use vcad_ecad_pcb::router::flow::cdt::{CopperFilter, CutGraph, CutParams};
use vcad_ecad_pcb::router::flow::human::human_load;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .expect("usage: flow_m0 <board.pcb.json|.kicad_pcb> [out.json]");
    let out = args.next();
    let text = std::fs::read_to_string(&path).expect("read board");
    let mut pcb: vcad_ir::ecad::Pcb = if path.ends_with(".json") {
        serde_json::from_str(&text).expect("parse json")
    } else {
        vcad_ecad_symbols::parse_kicad_pcb(&text).expect("parse kicad")
    };
    if pcb.stackup.layers.iter().all(|l| l.role.is_none()) {
        for l in pcb.infer_layer_roles() {
            println!("{l}");
        }
    }
    let mut params = CutParams::from_pcb(&pcb);
    if let Some(c) = std::env::var("VCAD_FLOW_CLEARANCE")
        .ok()
        .and_then(|v| v.parse().ok())
    {
        params.clearance = c;
    }
    if let Some(w) = std::env::var("VCAD_FLOW_WIDTH")
        .ok()
        .and_then(|v| v.parse().ok())
    {
        params.width = w;
    }
    println!(
        "model: width {} spacing {} edge {}; plane layers {:?}",
        params.width,
        params.clearance,
        params.edge_clearance,
        pcb.plane_layers()
    );
    let t0 = Instant::now();
    let mut g = CutGraph::build(&pcb, params, CopperFilter::PadsOnly);
    if std::env::var("VCAD_FLOW_SITES").as_deref() == Ok("1") {
        let mut stripped = pcb.clone();
        stripped.traces.clear();
        stripped.vias.clear();
        let specs =
            vcad_ecad_pcb::router::flow::flow::choose_sites(&stripped, &g, &Default::default());
        let pts: Vec<[f64; 2]> = specs.iter().map(|s| s.pos).collect();
        println!("with {} via sites as Steiner vertices", pts.len());
        g = CutGraph::build_with(&pcb, params, CopperFilter::PadsOnly, &pts);
    }
    let build_ms = t0.elapsed().as_secs_f64() * 1e3;
    let t1 = Instant::now();
    let rep = human_load(&pcb, &g);
    let map_ms = t1.elapsed().as_secs_f64() * 1e3;
    println!("graph build {build_ms:.0} ms, human map {map_ms:.0} ms");
    println!(
        "{:<6} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7} {:>6} {:>6} {:>6}",
        "layer", "verts", "faces", "cuts", "segs", "loaded", "ovf_w", "ovf_n", "intru", "wfail"
    );
    for l in &rep.layers {
        println!(
            "{:<6} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7} {:>6} {:>6} {:>6}",
            l.layer.trim_end_matches("Cu"),
            l.verts,
            l.faces,
            l.cuts,
            l.segments,
            l.loaded,
            l.overflow_weighted,
            l.overflow_count,
            l.intrusions,
            l.walk_failures
        );
    }
    let (cuts, loaded, ow, oc, intr) = rep.totals();
    let sat: usize = rep.layers.iter().map(|l| l.saturated).sum();
    println!("saturated (count form, load == cap): {sat}");
    println!(
        "TOTAL cuts {cuts} loaded {loaded} | overflow weighted {ow} ({:.2}% of loaded, {:.3}% of all) \
         | count-form {oc} ({:.2}% of loaded) | intrusions {intr} | human segs on plane layers {}",
        100.0 * ow as f64 / loaded.max(1) as f64,
        100.0 * ow as f64 / cuts.max(1) as f64,
        100.0 * oc as f64 / loaded.max(1) as f64,
        rep.plane_layer_segments
    );
    let mut worst = rep.overflows.clone();
    worst.sort_by(|a, b| b.excess.total_cmp(&a.excess));
    for o in worst.iter().take(12) {
        println!(
            "  {} @({:.3},{:.3}) len {:.3} free {:.3} cap {} wires {} excess {:.3} {:?}",
            o.layer, o.at[0], o.at[1], o.len, o.free, o.cap, o.wires, o.excess, o.nets
        );
    }
    // Free faces (usable by any net) that nevertheless overlap a grown
    // obstacle: the triangulation failing to respect an obstacle.
    for lg in &g.layers {
        let mut bad = 0usize;
        let mut free = 0usize;
        for f in 0..lg.tris.len() {
            if !lg.tri_live[f] || lg.tri_block[f] || lg.tri_obs[f] != u32::MAX {
                continue;
            }
            free += 1;
            let t = lg.tris[f];
            let (a, b, c) = (lg.vpos(t[0]), lg.vpos(t[1]), lg.vpos(t[2]));
            let mut hit = false;
            for (u, v) in [
                (0.2, 0.2),
                (0.6, 0.2),
                (0.2, 0.6),
                (0.4, 0.4),
                (0.1, 0.45),
                (0.45, 0.1),
            ] {
                let w = 1.0 - u - v;
                let p = [
                    w * a[0] + u * b[0] + v * c[0],
                    w * a[1] + u * b[1] + v * c[1],
                ];
                if !lg.obstacles_at(p, -1e-4).is_empty() {
                    hit = true;
                }
            }
            if hit {
                bad += 1;
                if bad <= 3 {
                    println!("  overlap {:?} face {f}: {a:?} {b:?} {c:?}", lg.layer);
                }
            }
        }
        println!(
            "{:?}: {free} free faces, {bad} overlap a grown obstacle",
            lg.layer
        );
    }
    // VCAD_FLOW_PROBE=layer,x,y: dump the obstacles and cuts near a point.
    if let Ok(pr) = std::env::var("VCAD_FLOW_PROBE") {
        let f: Vec<&str> = pr.split(',').collect();
        let (x, y): (f64, f64) = (f[1].parse().unwrap(), f[2].parse().unwrap());
        if let Some(lg) = g.layers.iter().find(|l| format!("{:?}", l.layer) == f[0]) {
            for o in &lg.obstacles {
                let c = match o.geom {
                    vcad_ecad_pcb::spatial::CopperGeom::Disc { center, .. } => center,
                    vcad_ecad_pcb::spatial::CopperGeom::Rect { center, .. } => center,
                    vcad_ecad_pcb::spatial::CopperGeom::Segment { a, .. } => a,
                };
                if (c.x - x).abs() < 0.8 && (c.y - y).abs() < 0.8 {
                    println!(
                        "obs {:?} {:?} net {:?} grow {}",
                        o.kind, o.geom, o.net, o.grow
                    );
                }
            }
            for (i, c) in lg.cuts.iter().enumerate() {
                let a = lg.vpos(c.a);
                let b = lg.vpos(c.b);
                let m = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
                if (m[0] - x).abs() < 0.05 && (m[1] - y).abs() < 0.05 {
                    println!(
                        "cut {i} {:?}->{:?} len {:.3} parts {:?} cap {} faces {:?} vobs {} {}",
                        a,
                        b,
                        c.len,
                        c.parts,
                        c.cap,
                        c.faces,
                        lg.vobs[c.a as usize],
                        lg.vobs[c.b as usize]
                    );
                    let o = &lg.obstacles[lg.vobs[c.a as usize] as usize];
                    println!("  vobs a: {:?} {:?} {:?}", o.kind, o.net, o.geom);
                }
            }
        }
    }
    if let Some(o) = out {
        std::fs::write(o, serde_json::to_string_pretty(&rep).unwrap()).unwrap();
    }
}
