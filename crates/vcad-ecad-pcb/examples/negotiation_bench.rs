//! Negotiated-congestion benchmark: greedy + rip-up vs PathFinder negotiation.
//!
//! The autorouter's negotiation loop (`RouteOptions::negotiation_rounds`) is
//! the OrthoRoute/PathFinder lesson applied to vcad's grid router: rather than
//! letting the first net to claim a contested corridor keep it forever, every
//! round deposits history cost on corridors that starved a connection, and the
//! next round's flexible nets route around them. With `negotiation_rounds: 1`
//! the loop reduces exactly to the historical greedy + rip-up router, which
//! makes an in-repo A/B trivial: same board, same options, one knob.
//!
//! This example generates deterministic congested boards (no external
//! fixtures needed), routes each with negotiation off and on, and prints a
//! completion/via/length/time scoreboard:
//!
//! ```bash
//! cargo run --release -p vcad-ecad-pcb --example negotiation_bench
//! ```
//!
//! Scenarios:
//! - `crossing-bus-N`: N nets between two pad columns with reversed order, so
//!   every net crosses every other — the canonical negotiation stress.
//! - `pin-grid-N`: an N×N grid of pads paired across the grid center, a
//!   BGA-escape-shaped knot where early greedy routes wall off later ones.
//! - `corridor-N`: N nets funneled through a keepout pinch far narrower than
//!   their combined demand, so completion depends on who yields the corridor.

use std::time::Instant;

use vcad_ecad_pcb::drc::check_drc;
use vcad_ecad_pcb::router::{route_all_with_opts, RouteAllResult, RouteOptions};
use vcad_ir::ecad::*;
use vcad_ir::Vec2;

fn pad(num: &str, x: f64, y: f64, net: &str) -> Pad {
    Pad {
        number: num.into(),
        pad_type: PadType::SMD,
        shape: PadShape::Rect {
            width: 1.0,
            height: 1.0,
        },
        position: Vec2::new(x, y),
        rotation: 0.0,
        drill: None,
        net: Some(net.into()),
        layers: vec![PcbLayer::FCu],
    }
}

fn fp(reference: &str, pads: Vec<Pad>) -> Footprint {
    Footprint {
        reference: reference.into(),
        value: "x".into(),
        footprint_name: "bench".into(),
        position: Vec2::new(0.0, 0.0),
        rotation: 0.0,
        front: true,
        pads,
        graphics: vec![],
        model_3d: None,
        properties: Default::default(),
    }
}

fn board(w: f64, h: f64, footprints: Vec<Footprint>, keepouts: Vec<Keepout>) -> Pcb {
    Pcb {
        outline: BoardOutline {
            vertices: vec![
                Vec2::new(0.0, 0.0),
                Vec2::new(w, 0.0),
                Vec2::new(w, h),
                Vec2::new(0.0, h),
            ],
            cutouts: vec![],
            thickness: 1.6,
        },
        stackup: LayerStackup {
            layers: vec![
                StackupLayer {
                    layer: PcbLayer::FCu,
                    copper_thickness: Some(0.035),
                    dielectric_thickness: Some(1.5),
                    dielectric_er: Some(4.5),
                    material: Some("FR4".into()),
                },
                StackupLayer {
                    layer: PcbLayer::BCu,
                    copper_thickness: Some(0.035),
                    dielectric_thickness: None,
                    dielectric_er: None,
                    material: None,
                },
            ],
        },
        nets: vec![],
        rules: DesignRules {
            default_rules: NetClassRules {
                name: "Default".into(),
                trace_width: 0.25,
                clearance: 0.2,
                via_diameter: 0.8,
                via_drill: 0.4,
                diff_pair_gap: None,
                diff_pair_width: None,
                target_impedance: None,
                target_diff_impedance: None,
            },
            class_rules: vec![],
            net_class_assignments: Default::default(),
            edge_clearance: 0.5,
            hole_to_hole: 0.5,
            min_annular_ring: 0.15,
            min_drill: 0.2,
        },
        footprints,
        traces: vec![],
        trace_arcs: vec![],
        vias: vec![],
        zones: vec![],
        keepouts,
        net_ties: vec![],
    }
}

/// N nets between a left and a right pad column, with the right column's net
/// order reversed — every net must cross every other net.
fn crossing_bus(n: usize) -> Pcb {
    let pitch = 2.0;
    let h = (n as f64 + 1.0) * pitch + 4.0;
    let mut left = Vec::new();
    let mut right = Vec::new();
    for i in 0..n {
        let y = 4.0 + i as f64 * pitch;
        let net = format!("N{i}");
        left.push(pad(&format!("L{i}"), 4.0, y, &net));
        // Reversed order on the right: net i lands at row n-1-i.
        let yr = 4.0 + (n - 1 - i) as f64 * pitch;
        right.push(pad(&format!("R{i}"), 46.0, yr, &net));
    }
    board(50.0, h, vec![fp("J1", left), fp("J2", right)], vec![])
}

/// n×n pad grid; each pad in the left half pairs with its point-reflection
/// through the grid center, so every connection threads the middle.
fn pin_grid(n: usize) -> Pcb {
    let pitch = 2.5;
    let span = (n as f64 - 1.0) * pitch;
    let margin = 6.0;
    let size = span + 2.0 * margin;
    let mut pads = Vec::new();
    let mut k = 0usize;
    for r in 0..n {
        for c in 0..n / 2 {
            // Left half column c pairs with mirrored (n-1-r, n-1-c).
            let (mr, mc) = (n - 1 - r, n - 1 - c);
            let net = format!("G{k}");
            pads.push(pad(
                &format!("A{k}"),
                margin + c as f64 * pitch,
                margin + r as f64 * pitch,
                &net,
            ));
            pads.push(pad(
                &format!("B{k}"),
                margin + mc as f64 * pitch,
                margin + mr as f64 * pitch,
                &net,
            ));
            k += 1;
        }
    }
    board(size, size, vec![fp("U1", pads)], vec![])
}

/// N nets that must all pass through a keepout pinch whose width supports far
/// fewer tracks than the demand — completion hinges on negotiation deciding
/// which nets take the long way around.
fn corridor(n: usize) -> Pcb {
    let pitch = 2.0;
    let h = (n as f64 + 1.0) * pitch + 8.0;
    let w = 60.0;
    let gap = 3.0; // pinch aperture (mm) — a handful of 0.25/0.2 tracks
    let slot_y0 = h / 2.0 - gap / 2.0;
    let mut left = Vec::new();
    let mut right = Vec::new();
    for i in 0..n {
        let y = 6.0 + i as f64 * pitch;
        let net = format!("C{i}");
        left.push(pad(&format!("L{i}"), 4.0, y, &net));
        right.push(pad(&format!("R{i}"), w - 4.0, y, &net));
    }
    let wall = |y0: f64, y1: f64| Keepout {
        outline: vec![
            Vec2::new(w / 2.0 - 1.0, y0),
            Vec2::new(w / 2.0 + 1.0, y0),
            Vec2::new(w / 2.0 + 1.0, y1),
            Vec2::new(w / 2.0 - 1.0, y1),
        ],
        layers: vec![PcbLayer::FCu, PcbLayer::BCu],
        no_tracks: true,
        no_vias: true,
        no_pour: false,
        no_components: false,
    };
    board(
        w,
        h,
        vec![fp("J1", left), fp("J2", right)],
        // Lower wall reaches the board edge; the upper wall stops 4 mm short,
        // leaving a long detour over the top. Negotiation decides which nets
        // take the pinch and which yield to the detour.
        vec![wall(0.0, slot_y0), wall(slot_y0 + gap, h - 4.0)],
    )
}

struct Score {
    routed: usize,
    total: usize,
    vias: usize,
    copper_mm: f64,
    secs: f64,
    drc: usize,
}

/// Copper-legality errors only: `UnconnectedNet` is excluded because it counts
/// *incomplete* routing (the completion column already reports that), not
/// *illegal* copper — including it would let the routed/unrouted delta mask
/// clearance violations in the before/after comparison.
fn drc_errors(pcb: &Pcb) -> usize {
    use vcad_ecad_pcb::drc::{DrcRuleType, DrcSeverity};
    check_drc(pcb)
        .iter()
        .filter(|v| v.severity == DrcSeverity::Error && v.rule != DrcRuleType::UnconnectedNet)
        .count()
}

fn run(pcb: &Pcb, negotiation_rounds: usize) -> Score {
    let width = pcb.rules.default_rules.trace_width;
    let t0 = Instant::now();
    let r: RouteAllResult = route_all_with_opts(
        pcb,
        width,
        &[],
        &RouteOptions {
            negotiation_rounds,
            ..Default::default()
        },
    );
    let secs = t0.elapsed().as_secs_f64();
    let copper_mm = r
        .traces
        .iter()
        .map(|t| ((t.end.x - t.start.x).powi(2) + (t.end.y - t.start.y).powi(2)).sqrt())
        .sum();
    // DRC on the applied result — negotiation must never trade legality for
    // completion (history only adds cost; the clearance invariant is intact).
    let mut applied = pcb.clone();
    for t in &r.traces {
        applied.traces.push(Trace {
            start: t.start,
            end: t.end,
            width: t.width,
            layer: t.layer,
            net: t.net.clone(),
            source: None,
        });
    }
    for v in &r.vias {
        applied.vias.push(Via {
            position: v.position,
            diameter: applied.rules.default_rules.via_diameter,
            drill: applied.rules.default_rules.via_drill,
            start_layer: v.start_layer,
            end_layer: v.end_layer,
            net: v.net.clone(),
            source: None,
        });
    }
    // Compare against the bare board so any pre-existing land-pattern
    // violations don't count against the router — only copper it added can
    // add errors (connectivity errors are excluded in `drc_errors`).
    let drc = drc_errors(&applied).saturating_sub(drc_errors(pcb));
    if drc > 0 {
        use vcad_ecad_pcb::drc::{DrcRuleType, DrcSeverity};
        for v in check_drc(&applied)
            .iter()
            .filter(|v| v.severity == DrcSeverity::Error && v.rule != DrcRuleType::UnconnectedNet)
            .take(8)
        {
            eprintln!(
                "  drc {:?} at ({:.2},{:.2}): {}",
                v.rule, v.position.x, v.position.y, v.message
            );
        }
    }
    Score {
        routed: r.routed_nets.len(),
        total: r.routed_nets.len() + r.unrouted_nets.len(),
        vias: r.vias.len(),
        copper_mm,
        secs,
        drc,
    }
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    let scenarios: Vec<(String, Pcb)> = vec![
        ("crossing-bus-12".into(), crossing_bus(12)),
        ("crossing-bus-24".into(), crossing_bus(24)),
        ("pin-grid-8".into(), pin_grid(8)),
        ("pin-grid-12".into(), pin_grid(12)),
        ("corridor-16".into(), corridor(16)),
        ("corridor-24".into(), corridor(24)),
    ];

    println!(
        "{:<18} {:>22} {:>22}  delta",
        "scenario", "greedy+ripup", "negotiated"
    );
    for (name, pcb) in &scenarios {
        let pre = drc_errors(pcb);
        if pre > 0 {
            eprintln!("note: {name} bare board has {pre} pre-existing DRC error(s)");
        }
        let base = run(pcb, 1); // reduces exactly to the historical router
        let nego = run(pcb, vcad_ecad_pcb::router::auto::DEFAULT_NEGOTIATION_ROUNDS);
        assert_eq!(base.drc, 0, "{name}: baseline emitted DRC errors");
        assert_eq!(nego.drc, 0, "{name}: negotiation emitted DRC errors");
        assert!(
            nego.routed >= base.routed,
            "{name}: negotiation regressed below greedy + rip-up ({} < {})",
            nego.routed,
            base.routed
        );
        println!(
            "{:<18} {:>10} {:>6.1}s {:>4}v {:>10} {:>6.1}s {:>4}v  {:+} nets, {:+.0} mm",
            name,
            format!("{}/{}", base.routed, base.total),
            base.secs,
            base.vias,
            format!("{}/{}", nego.routed, nego.total),
            nego.secs,
            nego.vias,
            nego.routed as i64 - base.routed as i64,
            nego.copper_mm - base.copper_mm,
        );
    }
}
