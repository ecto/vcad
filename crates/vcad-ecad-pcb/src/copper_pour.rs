//! Zone fill (copper pour) algorithm.
//!
//! Generates filled zone polygons for PCB copper pours. A zone is a region
//! of copper connected to a net. The pour is the zone outline with exact
//! clearance voids knocked out around every other-net copper element (traces,
//! pads, vias) and thermal-relief spokes formed around same-net pads.
//!
//! Geometry is computed with the kernel-native [`poly2d`] polygon-boolean
//! engine — the same snap-rounded exact-arithmetic engine behind sheet-metal
//! flat patterns — so voids are real subtracted regions, not discarded intent.

use vcad_ir::ecad::{Pad, PadShape, Pcb, PcbLayer, ThermalReliefStyle, Zone};
use vcad_ir::Vec2;
use vcad_kernel_math::Point2;
use vcad_kernel_sheet::poly2d::{self, Poly};

/// Segments used to polygonize a full circle (via/round clearance).
const CIRCLE_SEG: usize = 32;
/// Segments used to polygonize each semicircular trace cap.
const CAP_SEG: usize = 10;
/// Fallback thermal-relief spoke width (mm) when the zone leaves it unset.
const DEFAULT_SPOKE_WIDTH: f64 = 0.5;

/// Filled zone result after copper pour calculation.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FilledZone {
    /// Filled polygons. Each entry is a closed ring in board coordinates;
    /// outer rings (copper) are CCW and clearance voids (holes) are CW, so an
    /// even-odd or non-zero fill renders the pour with its cut-outs correctly.
    pub polygons: Vec<Vec<Vec2>>,
    /// Net this zone is assigned to.
    pub net: String,
    /// Layer this zone is on.
    pub layer: PcbLayer,
}

/// Fill all zones on a PCB.
///
/// For each zone, this computes filled copper polygons with exact clearance
/// voids around copper elements belonging to other nets and thermal relief
/// around same-net pads.
///
/// # Returns
///
/// A vector of [`FilledZone`] results, one per input zone.
pub fn fill_zones(pcb: &Pcb) -> Vec<FilledZone> {
    pcb.zones.iter().map(|zone| fill_zone(pcb, zone)).collect()
}

/// Fill a single zone by subtracting every clearance region from its outline.
fn fill_zone(pcb: &Pcb, zone: &Zone) -> FilledZone {
    // Subject: the zone outline (CCW) with its user-declared holes (CW).
    let subject = Poly {
        outer: ccw(ring_to_pts(&zone.outline)),
        holes: zone
            .holes
            .iter()
            .filter(|h| h.len() >= 3)
            .map(|h| cw(ring_to_pts(h)))
            .collect(),
    };

    // Clip: the union of every clearance void to remove from the pour.
    let clips = collect_clearance_regions(pcb, zone);

    let filled = if clips.is_empty() {
        vec![subject]
    } else {
        poly2d::difference(&[subject], &clips)
    };

    // Emit outer + hole rings, dropping copper islands below the minimum area.
    let mut polygons = Vec::new();
    for poly in &filled {
        if zone.min_area > 0.0 && poly.area() < zone.min_area {
            continue;
        }
        polygons.push(pts_to_ring(&poly.outer));
        for hole in &poly.holes {
            polygons.push(pts_to_ring(hole));
        }
    }

    FilledZone {
        polygons,
        net: zone.net.clone(),
        layer: zone.layer,
    }
}

/// Build the set of clearance regions to subtract from a zone's pour.
fn collect_clearance_regions(pcb: &Pcb, zone: &Zone) -> Vec<Poly> {
    let clearance = zone.clearance;
    let mut clips: Vec<Poly> = Vec::new();

    // Other-net traces on this layer: capsule (thick segment) clearance.
    for trace in &pcb.traces {
        if trace.net == zone.net || trace.layer != zone.layer {
            continue;
        }
        let r = trace.width / 2.0 + clearance;
        clips.push(capsule_poly(trace.start, trace.end, r));
    }

    // Other-net vias: circular clearance. (Same-net vias flood into the pour.)
    for via in &pcb.vias {
        if via.net == zone.net {
            continue;
        }
        clips.push(circle_poly(via.position, via.diameter / 2.0 + clearance));
    }

    // Pads: other-net pads get a full clearance void; same-net pads get
    // thermal relief (or flood, or a void) per the zone's thermal style.
    let gap = zone.thermal_gap.unwrap_or(clearance);
    let spoke = zone.thermal_spoke_width.unwrap_or(DEFAULT_SPOKE_WIDTH);
    for footprint in &pcb.footprints {
        let fr = footprint.rotation.to_radians();
        let (fc, fs) = (fr.cos(), fr.sin());
        for pad in &footprint.pads {
            if !pad.layers.contains(&zone.layer) {
                continue;
            }
            let world = Vec2::new(
                footprint.position.x + pad.position.x * fc - pad.position.y * fs,
                footprint.position.y + pad.position.x * fs + pad.position.y * fc,
            );
            let ang = fr + pad.rotation.to_radians();
            let (hw, hh) = pad_half_extents(pad);
            let same_net = pad.net.as_deref() == Some(zone.net.as_str());

            if !same_net {
                clips.push(oriented_rect_poly(
                    world,
                    hw + clearance,
                    hh + clearance,
                    ang,
                ));
                continue;
            }
            match zone.thermal_relief {
                // Solid copper to the pad — nothing knocked out.
                ThermalReliefStyle::Direct => {}
                // Full antipad — the pad is not tied to the pour.
                ThermalReliefStyle::None => {
                    clips.push(oriented_rect_poly(world, hw + gap, hh + gap, ang));
                }
                // Cross-spoke thermal relief: clear the ring but keep 4 spokes.
                ThermalReliefStyle::Relief => {
                    clips.extend(thermal_relief_regions(world, hw, hh, ang, gap, spoke));
                }
            }
        }
    }

    clips
}

/// The cleared region of a cross-spoke thermal relief: the annulus around a
/// same-net pad (pad expanded by `gap`, minus the pad) with four axis spokes
/// of width `spoke` left as copper so the pad still ties to the pour.
fn thermal_relief_regions(
    center: Vec2,
    hw: f64,
    hh: f64,
    ang: f64,
    gap: f64,
    spoke: f64,
) -> Vec<Poly> {
    let outer = oriented_rect_poly(center, hw + gap, hh + gap, ang);
    let pad = oriented_rect_poly(center, hw, hh, ang);
    let reach = (hw.max(hh) + gap) * 2.0;
    let spoke_h = oriented_rect_poly(center, reach, spoke / 2.0, ang);
    let spoke_v = oriented_rect_poly(center, spoke / 2.0, reach, ang);
    // annulus minus spokes = the copper-clear region to subtract from the pour.
    poly2d::difference(&[outer], &[pad, spoke_h, spoke_v])
}

// --- polygon construction helpers ------------------------------------------

fn ring_to_pts(ring: &[Vec2]) -> Vec<Point2> {
    ring.iter().map(|v| Point2::new(v.x, v.y)).collect()
}

fn pts_to_ring(ring: &[Point2]) -> Vec<Vec2> {
    ring.iter().map(|p| Vec2::new(p.x, p.y)).collect()
}

/// Force a ring counter-clockwise (poly2d outer-ring convention).
fn ccw(mut ring: Vec<Point2>) -> Vec<Point2> {
    if poly2d::signed_area_f(&ring) < 0.0 {
        ring.reverse();
    }
    ring
}

/// Force a ring clockwise (poly2d hole-ring convention).
fn cw(mut ring: Vec<Point2>) -> Vec<Point2> {
    if poly2d::signed_area_f(&ring) > 0.0 {
        ring.reverse();
    }
    ring
}

/// A regular polygon approximating a disc of radius `r` about `c`.
fn circle_poly(c: Vec2, r: f64) -> Poly {
    let mut pts = Vec::with_capacity(CIRCLE_SEG);
    for i in 0..CIRCLE_SEG {
        let a = std::f64::consts::TAU * (i as f64) / (CIRCLE_SEG as f64);
        pts.push(Point2::new(c.x + r * a.cos(), c.y + r * a.sin()));
    }
    Poly::new(ccw(pts))
}

/// A capsule (stadium) — the Minkowski sum of segment `a`–`b` with a disc of
/// radius `r` — approximated with `CAP_SEG`-segment semicircular caps.
fn capsule_poly(a: Vec2, b: Vec2, r: f64) -> Poly {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-9 {
        return circle_poly(a, r);
    }
    let (px, py) = (-dy / len * r, dx / len * r); // left normal * r
    let base = py.atan2(px); // angle of +normal
    let pi = std::f64::consts::PI;

    let mut pts = Vec::with_capacity(2 * CAP_SEG + 4);
    // Left offset edge a->b.
    pts.push(Point2::new(a.x + px, a.y + py));
    pts.push(Point2::new(b.x + px, b.y + py));
    // Cap around b, sweeping over the +direction front to the right side.
    for k in 1..CAP_SEG {
        let t = base - pi * (k as f64) / (CAP_SEG as f64);
        pts.push(Point2::new(b.x + r * t.cos(), b.y + r * t.sin()));
    }
    // Right offset edge b->a.
    pts.push(Point2::new(b.x - px, b.y - py));
    pts.push(Point2::new(a.x - px, a.y - py));
    // Cap around a, sweeping over the back to the left side.
    for k in 1..CAP_SEG {
        let t = base + pi - pi * (k as f64) / (CAP_SEG as f64);
        pts.push(Point2::new(a.x + r * t.cos(), a.y + r * t.sin()));
    }
    Poly::new(ccw(pts))
}

/// An axis-or-rotated rectangle of half-extents `hw`,`hh` about `center`.
fn oriented_rect_poly(center: Vec2, hw: f64, hh: f64, ang: f64) -> Poly {
    let (ca, sa) = (ang.cos(), ang.sin());
    let corners = [(-hw, -hh), (hw, -hh), (hw, hh), (-hw, hh)];
    let pts = corners
        .iter()
        .map(|(x, y)| Point2::new(center.x + x * ca - y * sa, center.y + x * sa + y * ca))
        .collect();
    Poly::new(ccw(pts))
}

/// Get the half-width and half-height of a pad's bounding box.
fn pad_half_extents(pad: &Pad) -> (f64, f64) {
    match &pad.shape {
        PadShape::Circle { diameter } => {
            let r = diameter / 2.0;
            (r, r)
        }
        PadShape::Rect { width, height }
        | PadShape::Oval { width, height }
        | PadShape::RoundRect { width, height, .. } => (width / 2.0, height / 2.0),
        PadShape::Custom { vertices } => {
            if vertices.is_empty() {
                return (0.0, 0.0);
            }
            let mut max_x: f64 = 0.0;
            let mut max_y: f64 = 0.0;
            for v in vertices {
                max_x = max_x.max(v.x.abs());
                max_y = max_y.max(v.y.abs());
            }
            (max_x, max_y)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_ir::ecad::*;

    /// Create a minimal test PCB.
    fn test_pcb() -> Pcb {
        Pcb {
            outline: BoardOutline {
                vertices: vec![
                    Vec2::new(0.0, 0.0),
                    Vec2::new(50.0, 0.0),
                    Vec2::new(50.0, 40.0),
                    Vec2::new(0.0, 40.0),
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
                        material: Some("FR4".to_string()),
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
            nets: vec![
                Net {
                    id: "1".to_string(),
                    name: "VCC".to_string(),
                },
                Net {
                    id: "2".to_string(),
                    name: "GND".to_string(),
                },
            ],
            rules: DesignRules {
                default_rules: NetClassRules {
                    name: "Default".to_string(),
                    trace_width: 0.25,
                    clearance: 0.2,
                    via_diameter: 0.8,
                    via_drill: 0.4,
                    diff_pair_gap: None,
                    diff_pair_width: None,
                },
                class_rules: vec![],
                net_class_assignments: std::collections::HashMap::new(),
                edge_clearance: 0.5,
                hole_to_hole: 0.5,
                min_annular_ring: 0.15,
                min_drill: 0.2,
            },
            footprints: vec![],
            traces: vec![Trace {
                start: Vec2::new(10.0, 20.0),
                end: Vec2::new(30.0, 20.0),
                width: 0.25,
                layer: PcbLayer::FCu,
                net: "1".to_string(),
            }],
            trace_arcs: vec![],
            vias: vec![],
            zones: vec![Zone {
                outline: vec![
                    Vec2::new(0.0, 0.0),
                    Vec2::new(50.0, 0.0),
                    Vec2::new(50.0, 40.0),
                    Vec2::new(0.0, 40.0),
                ],
                holes: vec![],
                net: "2".to_string(),
                layer: PcbLayer::FCu,
                clearance: 0.3,
                min_area: 0.0,
                fill_type: ZoneFillType::Solid,
                thermal_relief: ThermalReliefStyle::Relief,
                thermal_gap: Some(0.5),
                thermal_spoke_width: Some(0.5),
                priority: 0,
            }],
            keepouts: vec![],
            net_ties: vec![],
        }
    }

    /// Net copper area: signed shoelace sum (CCW outer adds, CW hole subtracts).
    fn area_of(filled: &FilledZone) -> f64 {
        filled
            .polygons
            .iter()
            .map(|ring| {
                let n = ring.len();
                let mut s = 0.0;
                for i in 0..n {
                    let a = ring[i];
                    let b = ring[(i + 1) % n];
                    s += a.x * b.y - b.x * a.y;
                }
                0.5 * s
            })
            .sum()
    }

    fn gnd_pad_at(pos: Vec2) -> Footprint {
        Footprint {
            reference: "TP1".into(),
            value: String::new(),
            footprint_name: "pad".into(),
            position: pos,
            rotation: 0.0,
            front: true,
            pads: vec![Pad {
                number: "1".into(),
                pad_type: PadType::SMD,
                shape: PadShape::Rect {
                    width: 2.0,
                    height: 2.0,
                },
                position: Vec2::new(0.0, 0.0),
                rotation: 0.0,
                drill: None,
                net: Some("2".into()), // same as the zone net (GND)
                layers: vec![PcbLayer::FCu],
            }],
            graphics: vec![],
            model_3d: None,
            properties: Default::default(),
        }
    }

    #[test]
    fn pour_knocks_out_other_net_trace() {
        // test_pcb has an other-net trace (net "1") crossing the GND zone.
        let filled = fill_zones(&test_pcb());
        assert_eq!(filled.len(), 1);
        assert!(
            filled[0].polygons.len() >= 2,
            "other-net trace should carve a void (outer + hole), got {} rings",
            filled[0].polygons.len()
        );
        let area = area_of(&filled[0]);
        assert!(
            area > 0.0 && area < 2000.0,
            "pour area {area} should be below the 2000 mm^2 outline"
        );
    }

    #[test]
    fn thermal_relief_is_between_flood_and_antipad() {
        let mut pcb = test_pcb();
        pcb.traces.clear(); // isolate the same-net pad's effect
        pcb.footprints.push(gnd_pad_at(Vec2::new(25.0, 20.0)));

        let area_for = |style: ThermalReliefStyle| {
            let mut p = pcb.clone();
            p.zones[0].thermal_relief = style;
            area_of(&fill_zones(&p)[0])
        };
        let direct = area_for(ThermalReliefStyle::Direct);
        let relief = area_for(ThermalReliefStyle::Relief);
        let antipad = area_for(ThermalReliefStyle::None);

        // Flood floods fully; relief clears a spoked ring; antipad clears it all.
        assert!(
            direct > relief,
            "flood {direct} should exceed relief {relief}"
        );
        assert!(
            relief > antipad,
            "relief {relief} (spokes keep copper) should exceed antipad {antipad}"
        );
    }

    #[test]
    fn fill_zones_produces_output() {
        let pcb = test_pcb();
        let filled = fill_zones(&pcb);
        assert_eq!(filled.len(), 1);
        assert_eq!(filled[0].net, "2");
        assert_eq!(filled[0].layer, PcbLayer::FCu);
        assert!(!filled[0].polygons.is_empty());
        assert_eq!(filled[0].polygons[0].len(), 4); // zone outline
    }

    #[test]
    fn fill_zones_empty_pcb() {
        let mut pcb = test_pcb();
        pcb.zones.clear();
        let filled = fill_zones(&pcb);
        assert!(filled.is_empty());
    }

    #[test]
    fn pad_half_extents_circle() {
        let pad = Pad {
            number: "1".to_string(),
            pad_type: PadType::SMD,
            shape: PadShape::Circle { diameter: 2.0 },
            position: Vec2::new(0.0, 0.0),
            rotation: 0.0,
            drill: None,
            net: None,
            layers: vec![PcbLayer::FCu],
        };
        let (hw, hh) = pad_half_extents(&pad);
        assert!((hw - 1.0).abs() < 1e-10);
        assert!((hh - 1.0).abs() < 1e-10);
    }

    #[test]
    fn pad_half_extents_rect() {
        let pad = Pad {
            number: "1".to_string(),
            pad_type: PadType::SMD,
            shape: PadShape::Rect {
                width: 3.0,
                height: 2.0,
            },
            position: Vec2::new(0.0, 0.0),
            rotation: 0.0,
            drill: None,
            net: None,
            layers: vec![PcbLayer::FCu],
        };
        let (hw, hh) = pad_half_extents(&pad);
        assert!((hw - 1.5).abs() < 1e-10);
        assert!((hh - 1.0).abs() < 1e-10);
    }
}
