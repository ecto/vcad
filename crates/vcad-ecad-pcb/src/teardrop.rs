//! Teardrop fillets at trace → pad/via junctions.
//!
//! A teardrop is a tear-shaped copper fillet where a trace meets a pad or via.
//! It widens the connection so a small drill or registration error can't break
//! the trace out of the pad — fab table-stakes that KiCad/Altium add by default.
//!
//! Each teardrop is the convex hull of the pad/via circle and the trace's
//! cross-section a short way back along it: the hull wraps the round land and
//! flares smoothly into the (narrower) trace. The geometry is computed on the
//! fly from the board — like [`crate::copper_pour::fill_zones`] — so it needs no
//! IR field and reaches the Gerber writer and renderer through the same path.

use vcad_ir::ecad::{Pcb, PcbLayer};
use vcad_ir::Vec2;

use crate::spatial::pad_half_extents;

/// Points used to polygonize the round land a teardrop flares from.
const LAND_SEG: usize = 24;
/// A teardrop is only worth adding when the land is at least this much wider
/// than the trace (half-widths), in mm — otherwise it's a no-op flare.
const MIN_WIDEN: f64 = 0.05;

/// A generated teardrop fillet (extra copper at a junction).
#[derive(Debug, Clone, serde::Serialize)]
pub struct Teardrop {
    /// Closed polygon outline in board coordinates.
    pub polygon: Vec<Vec2>,
    /// Copper layer the teardrop is on (its trace's layer).
    pub layer: PcbLayer,
    /// Net.
    pub net: String,
}

/// Generate teardrop fillets for every trace endpoint that lands on a same-net
/// pad or via on the same layer.
pub fn generate_teardrops(pcb: &Pcb) -> Vec<Teardrop> {
    // Round lands a trace can teardrop into: (center, radius, net, on-layer test).
    struct Land {
        center: Vec2,
        r: f64,
        net: String,
        layers: LandLayers,
    }
    enum LandLayers {
        /// A through/blind via spanning a contiguous range — on any copper layer.
        Via,
        /// An SMD/THT pad present only on its declared layers.
        Pad(Vec<PcbLayer>),
    }

    let mut lands: Vec<Land> = Vec::new();
    for via in &pcb.vias {
        lands.push(Land {
            center: via.position,
            r: via.diameter / 2.0,
            net: via.net.clone(),
            layers: LandLayers::Via,
        });
    }
    for fp in &pcb.footprints {
        for pad in &fp.pads {
            let Some(net) = pad.net.clone() else { continue };
            let world = crate::geometry::pad_world_position(fp, pad);
            let (hw, hh) = pad_half_extents(pad);
            lands.push(Land {
                center: world,
                r: hw.min(hh), // inscribed-circle radius of the land
                net,
                layers: LandLayers::Pad(pad.layers.clone()),
            });
        }
    }

    let on_layer = |l: &Land, layer: PcbLayer| match &l.layers {
        LandLayers::Via => true,
        LandLayers::Pad(layers) => layers.contains(&layer),
    };

    let mut out = Vec::new();
    for trace in &pcb.traces {
        let hw = trace.width / 2.0;
        let len = dist(trace.start, trace.end);
        if len < 1e-6 {
            continue;
        }
        for (end, other) in [(trace.start, trace.end), (trace.end, trace.start)] {
            // Nearest same-net land on this layer whose centre the trace ends in.
            let land = lands
                .iter()
                .filter(|l| l.net == trace.net && on_layer(l, trace.layer))
                .filter(|l| dist(l.center, end) <= l.r + hw)
                .min_by(|a, b| {
                    dist(a.center, end)
                        .partial_cmp(&dist(b.center, end))
                        .unwrap()
                });
            let Some(land) = land else { continue };
            if land.r <= hw + MIN_WIDEN {
                continue; // land barely wider than the trace — nothing to flare
            }
            if let Some(poly) = teardrop_poly(land.center, land.r, end, other, hw, len) {
                out.push(Teardrop {
                    polygon: poly,
                    layer: trace.layer,
                    net: trace.net.clone(),
                });
            }
        }
    }
    out
}

/// Build one teardrop: the convex hull of the land circle and the trace
/// cross-section `back` mm along the trace from the junction.
fn teardrop_poly(
    center: Vec2,
    r: f64,
    junction: Vec2,
    other: Vec2,
    hw: f64,
    len: f64,
) -> Option<Vec<Vec2>> {
    // Unit vector from the land along the trace, and the perpendicular.
    let dx = other.x - junction.x;
    let dy = other.y - junction.y;
    let d = (dx * dx + dy * dy).sqrt();
    if d < 1e-9 {
        return None;
    }
    let (ux, uy) = (dx / d, dy / d);
    let (px, py) = (-uy, ux); // perpendicular to the trace direction
                              // Teardrop length: flare out to ~2 radii, bounded by the trace length.
    let back = (2.0 * r).min(len * 0.9);

    let mut pts = Vec::with_capacity(LAND_SEG + 2);
    for i in 0..LAND_SEG {
        let a = std::f64::consts::TAU * (i as f64) / (LAND_SEG as f64);
        pts.push(Vec2::new(center.x + r * a.cos(), center.y + r * a.sin()));
    }
    // Trace-edge points `back` along the trace from the junction.
    let base = Vec2::new(center.x + ux * back, center.y + uy * back);
    pts.push(Vec2::new(base.x + px * hw, base.y + py * hw));
    pts.push(Vec2::new(base.x - px * hw, base.y - py * hw));

    let hull = convex_hull(pts);
    if hull.len() < 3 {
        None
    } else {
        Some(hull)
    }
}

/// Andrew's monotone-chain convex hull (CCW).
fn convex_hull(mut pts: Vec<Vec2>) -> Vec<Vec2> {
    pts.sort_by(|a, b| {
        a.x.partial_cmp(&b.x)
            .unwrap()
            .then(a.y.partial_cmp(&b.y).unwrap())
    });
    pts.dedup_by(|a, b| (a.x - b.x).abs() < 1e-9 && (a.y - b.y).abs() < 1e-9);
    let n = pts.len();
    if n < 3 {
        return pts;
    }
    let cross = |o: Vec2, a: Vec2, b: Vec2| (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x);
    let mut hull: Vec<Vec2> = Vec::with_capacity(2 * n);
    for &p in &pts {
        while hull.len() >= 2 && cross(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0.0 {
            hull.pop();
        }
        hull.push(p);
    }
    let lower = hull.len() + 1;
    for &p in pts.iter().rev() {
        while hull.len() >= lower && cross(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0.0 {
            hull.pop();
        }
        hull.push(p);
    }
    hull.pop();
    hull
}

fn dist(a: Vec2, b: Vec2) -> f64 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_ir::ecad::*;

    fn board(footprints: Vec<Footprint>, traces: Vec<Trace>, vias: Vec<Via>) -> Pcb {
        Pcb {
            outline: BoardOutline {
                vertices: vec![
                    Vec2::new(0.0, 0.0),
                    Vec2::new(40.0, 0.0),
                    Vec2::new(40.0, 40.0),
                    Vec2::new(0.0, 40.0),
                ],
                cutouts: vec![],
                thickness: 1.6,
            },
            stackup: LayerStackup {
                layers: vec![StackupLayer {
                    layer: PcbLayer::FCu,
                    copper_thickness: Some(0.035),
                    dielectric_thickness: None,
                    dielectric_er: None,
                    material: None,
                }],
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
                },
                class_rules: vec![],
                net_class_assignments: Default::default(),
                edge_clearance: 0.5,
                hole_to_hole: 0.5,
                min_annular_ring: 0.15,
                min_drill: 0.2,
            },
            footprints,
            traces,
            trace_arcs: vec![],
            vias,
            zones: vec![],
            keepouts: vec![],
            net_ties: vec![],
        }
    }

    fn area(poly: &[Vec2]) -> f64 {
        let n = poly.len();
        let mut s = 0.0;
        for i in 0..n {
            let a = poly[i];
            let b = poly[(i + 1) % n];
            s += a.x * b.y - b.x * a.y;
        }
        (0.5 * s).abs()
    }

    #[test]
    fn teardrop_on_via() {
        // A trace ending on a same-net via gets one teardrop, bigger than the
        // bare via land.
        let via = Via {
            position: Vec2::new(20.0, 20.0),
            diameter: 1.0,
            drill: 0.5,
            start_layer: PcbLayer::FCu,
            end_layer: PcbLayer::BCu,
            net: "N".into(),
        };
        let trace = Trace {
            start: Vec2::new(20.0, 20.0),
            end: Vec2::new(30.0, 20.0),
            width: 0.25,
            layer: PcbLayer::FCu,
            net: "N".into(),
        };
        let tds = generate_teardrops(&board(vec![], vec![trace], vec![via]));
        assert_eq!(tds.len(), 1, "one endpoint lands on the via");
        let land_area = std::f64::consts::PI * 0.5 * 0.5;
        assert!(
            area(&tds[0].polygon) > land_area,
            "teardrop must be larger than the bare via land"
        );
    }

    #[test]
    fn no_teardrop_for_other_net() {
        let via = Via {
            position: Vec2::new(20.0, 20.0),
            diameter: 1.0,
            drill: 0.5,
            start_layer: PcbLayer::FCu,
            end_layer: PcbLayer::BCu,
            net: "OTHER".into(),
        };
        let trace = Trace {
            start: Vec2::new(20.0, 20.0),
            end: Vec2::new(30.0, 20.0),
            width: 0.25,
            layer: PcbLayer::FCu,
            net: "N".into(),
        };
        let tds = generate_teardrops(&board(vec![], vec![trace], vec![via]));
        assert!(tds.is_empty(), "a different-net via gets no teardrop");
    }

    #[test]
    fn no_teardrop_when_land_not_wider_than_trace() {
        // Via barely wider than the 0.25 trace (radius 0.14 < hw 0.125 + 0.05).
        let via = Via {
            position: Vec2::new(20.0, 20.0),
            diameter: 0.28,
            drill: 0.1,
            start_layer: PcbLayer::FCu,
            end_layer: PcbLayer::BCu,
            net: "N".into(),
        };
        let trace = Trace {
            start: Vec2::new(20.0, 20.0),
            end: Vec2::new(30.0, 20.0),
            width: 0.25,
            layer: PcbLayer::FCu,
            net: "N".into(),
        };
        assert!(generate_teardrops(&board(vec![], vec![trace], vec![via])).is_empty());
    }
}
