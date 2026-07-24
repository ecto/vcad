//! Eagle `.brd` (XML, Eagle 6+) board import.
//!
//! Parses the format the ODrive/moteus generation of open hardware shipped
//! in: `<board>` with `<plain>` outline wires (layer 20), `<libraries>` of
//! `<package>` definitions (`<smd>`/`<pad>`), `<elements>` placing them, and
//! `<signals>` carrying the netlist (`<contactref>`) plus the human-routed
//! ground truth (`<wire>`/`<via>`). Coordinates are millimetres.
//!
//! Layer mapping: Eagle copper layers are 1 (Top) .. 16 (Bottom) with 2–15
//! inner. Only layers actually used by the board's wires/vias are mapped,
//! in Eagle order, onto [`PcbLayer`] Top→Inner…→Bottom.

use std::collections::HashMap;

use vcad_ir::ecad::*;
use vcad_ir::Vec2;

/// Parse an Eagle `.brd` XML document into a [`Pcb`].
pub fn parse_eagle_brd(text: &str) -> Result<Pcb, String> {
    let opts = roxmltree::ParsingOptions {
        allow_dtd: true,
        ..Default::default()
    };
    let doc = roxmltree::Document::parse_with_options(text, opts)
        .map_err(|e| format!("XML parse: {e}"))?;
    let board = doc
        .descendants()
        .find(|n| n.has_tag_name("board"))
        .ok_or("no <board> element")?;

    // --- Copper layer mapping (used layers only, Eagle order). ---
    let mut used: Vec<u8> = board
        .descendants()
        .filter(|n| n.has_tag_name("wire") || n.has_tag_name("via"))
        .filter_map(|n| {
            if n.has_tag_name("via") {
                return None; // vias span; layers come from wires
            }
            n.attribute("layer").and_then(|l| l.parse::<u8>().ok())
        })
        .filter(|&l| (1..=16).contains(&l))
        .collect();
    used.sort_unstable();
    used.dedup();
    if !used.contains(&1) {
        used.insert(0, 1);
    }
    if !used.contains(&16) {
        used.push(16);
    }
    // Fail closed on boards deeper than the IR's copper vocabulary
    // (FCu + In1..In8 + BCu = 10): silently merging extra inner layers
    // would fabricate connectivity.
    if used.len() > 10 {
        return Err(format!(
            "board uses {} copper layers; importer supports at most 10 (FCu + In1..In8 + BCu)",
            used.len()
        ));
    }
    let n_inner = used.len().saturating_sub(2);
    let map_layer = |e: u8| -> Option<PcbLayer> {
        let idx = used.iter().position(|&u| u == e)?;
        Some(match idx {
            0 => PcbLayer::FCu,
            i if i == used.len() - 1 => PcbLayer::BCu,
            i => match i {
                1 => PcbLayer::In1Cu,
                2 => PcbLayer::In2Cu,
                3 => PcbLayer::In3Cu,
                4 => PcbLayer::In4Cu,
                5 => PcbLayer::In5Cu,
                6 => PcbLayer::In6Cu,
                7 => PcbLayer::In7Cu,
                8 => PcbLayer::In8Cu,
                _ => return None,
            },
        })
    };

    let fattr = |n: roxmltree::Node<'_, '_>, a: &str| -> f64 {
        n.attribute(a).and_then(|v| v.parse().ok()).unwrap_or(0.0)
    };

    // --- Outline: chain layer-20 wires from <plain>. ---
    let mut outline_segs: Vec<(Vec2, Vec2)> = Vec::new();
    if let Some(plain) = board.descendants().find(|n| n.has_tag_name("plain")) {
        for w in plain.children().filter(|n| n.has_tag_name("wire")) {
            if w.attribute("layer") == Some("20") {
                outline_segs.push((
                    Vec2::new(fattr(w, "x1"), fattr(w, "y1")),
                    Vec2::new(fattr(w, "x2"), fattr(w, "y2")),
                ));
            }
        }
    }
    let vertices = chain_outline(&outline_segs);

    // --- Packages. ---
    struct PkgPad {
        name: String,
        pos: Vec2,
        rot: f64,
        smd: bool,
        size: (f64, f64),
        drill: f64,
    }
    let mut packages: HashMap<(String, String), Vec<PkgPad>> = HashMap::new();
    for lib in board.descendants().filter(|n| n.has_tag_name("library")) {
        let lib_name = lib.attribute("name").unwrap_or("").to_string();
        for pkg in lib.descendants().filter(|n| n.has_tag_name("package")) {
            let mut pads = Vec::new();
            for p in pkg.children() {
                if p.has_tag_name("smd") {
                    pads.push(PkgPad {
                        name: p.attribute("name").unwrap_or("").into(),
                        pos: Vec2::new(fattr(p, "x"), fattr(p, "y")),
                        rot: parse_rot(p.attribute("rot")).1,
                        smd: true,
                        size: (fattr(p, "dx"), fattr(p, "dy")),
                        drill: 0.0,
                    });
                } else if p.has_tag_name("pad") {
                    let drill = fattr(p, "drill");
                    let dia = match p.attribute("diameter") {
                        Some(d) => d.parse().unwrap_or(drill * 1.5),
                        None => (drill * 1.5).max(drill + 0.5),
                    };
                    pads.push(PkgPad {
                        name: p.attribute("name").unwrap_or("").into(),
                        pos: Vec2::new(fattr(p, "x"), fattr(p, "y")),
                        rot: 0.0,
                        smd: false,
                        size: (dia, dia),
                        drill,
                    });
                }
            }
            packages.insert(
                (lib_name.clone(), pkg.attribute("name").unwrap_or("").into()),
                pads,
            );
        }
    }

    // --- Signals first (net per contactref). ---
    let mut pad_nets: HashMap<(String, String), String> = HashMap::new();
    let mut nets: Vec<Net> = Vec::new();
    let mut traces: Vec<Trace> = Vec::new();
    let mut vias: Vec<Via> = Vec::new();
    for (i, sig) in board
        .descendants()
        .filter(|n| n.has_tag_name("signal"))
        .enumerate()
    {
        let net_name = sig.attribute("name").unwrap_or("").to_string();
        nets.push(Net {
            id: format!("{i}"),
            name: net_name.clone(),
        });
        for c in sig.children() {
            if c.has_tag_name("contactref") {
                pad_nets.insert(
                    (
                        c.attribute("element").unwrap_or("").into(),
                        c.attribute("pad").unwrap_or("").into(),
                    ),
                    net_name.clone(),
                );
            } else if c.has_tag_name("wire") {
                let Some(layer) = c
                    .attribute("layer")
                    .and_then(|l| l.parse::<u8>().ok())
                    .and_then(map_layer)
                else {
                    continue;
                };
                traces.push(Trace {
                    start: Vec2::new(fattr(c, "x1"), fattr(c, "y1")),
                    end: Vec2::new(fattr(c, "x2"), fattr(c, "y2")),
                    width: fattr(c, "width").max(0.05),
                    layer,
                    net: net_name.clone(),
                    source: None,
                });
            } else if c.has_tag_name("via") {
                let drill = fattr(c, "drill").max(0.1);
                let dia = match c.attribute("diameter") {
                    Some(d) => d.parse().unwrap_or(drill * 2.0),
                    None => drill * 2.0,
                };
                vias.push(Via {
                    position: Vec2::new(fattr(c, "x"), fattr(c, "y")),
                    diameter: dia,
                    drill,
                    start_layer: PcbLayer::FCu,
                    end_layer: PcbLayer::BCu,
                    net: net_name.clone(),
                    source: None,
                });
            }
        }
    }

    // --- Elements → footprints. ---
    let mut footprints = Vec::new();
    for el in board.descendants().filter(|n| n.has_tag_name("element")) {
        let name = el.attribute("name").unwrap_or("").to_string();
        let lib = el.attribute("library").unwrap_or("").to_string();
        let pkg = el.attribute("package").unwrap_or("").to_string();
        let (mirrored, rot) = parse_rot(el.attribute("rot"));
        let origin = Vec2::new(fattr(el, "x"), fattr(el, "y"));
        let Some(pkg_pads) = packages.get(&(lib.clone(), pkg.clone())) else {
            continue;
        };
        let pads: Vec<Pad> = pkg_pads
            .iter()
            .map(|pp| {
                // Element rotation (+ mirror = bottom side, x flips).
                let mut p = pp.pos;
                if mirrored {
                    p.x = -p.x;
                }
                let (s, c) = rot.to_radians().sin_cos();
                let local = Vec2::new(p.x * c - p.y * s, p.x * s + p.y * c);
                let side = if mirrored {
                    PcbLayer::BCu
                } else {
                    PcbLayer::FCu
                };
                Pad {
                    number: pp.name.clone(),
                    pad_type: if pp.smd { PadType::SMD } else { PadType::THT },
                    shape: if pp.smd {
                        PadShape::Rect {
                            width: pp.size.0,
                            height: pp.size.1,
                        }
                    } else {
                        PadShape::Circle {
                            diameter: pp.size.0,
                        }
                    },
                    position: local,
                    rotation: pp.rot + rot,
                    drill: (!pp.smd).then_some(DrillSpec {
                        diameter: pp.drill,
                        oval: false,
                        oval_height: None,
                    }),
                    net: pad_nets.get(&(name.clone(), pp.name.clone())).cloned(),
                    layers: if pp.smd {
                        vec![side]
                    } else {
                        vec![PcbLayer::FCu, PcbLayer::BCu]
                    },
                }
            })
            .collect();
        footprints.push(Footprint {
            reference: name,
            value: el.attribute("value").unwrap_or("").to_string(),
            footprint_name: format!("{lib}:{pkg}"),
            position: origin,
            rotation: rot,
            front: !mirrored,
            pads,
            graphics: vec![],
            model_3d: None,
            properties: Default::default(),
        });
    }

    // --- Stackup: FCu + inners + BCu with plausible thicknesses. ---
    let mut layers = vec![StackupLayer {
        layer: PcbLayer::FCu,
        copper_thickness: Some(0.035),
        dielectric_thickness: Some(0.2),
        dielectric_er: Some(4.5),
        material: Some("FR4".into()),
    }];
    for i in 0..n_inner {
        layers.push(StackupLayer {
            layer: map_layer(used[i + 1]).unwrap_or(PcbLayer::In1Cu),
            copper_thickness: Some(0.035),
            dielectric_thickness: Some(0.2),
            dielectric_er: Some(4.5),
            material: Some("FR4".into()),
        });
    }
    layers.push(StackupLayer {
        layer: PcbLayer::BCu,
        copper_thickness: Some(0.035),
        dielectric_thickness: None,
        dielectric_er: None,
        material: None,
    });

    Ok(Pcb {
        outline: BoardOutline {
            vertices,
            cutouts: vec![],
            thickness: 1.6,
        },
        stackup: LayerStackup { layers },
        nets,
        rules: DesignRules {
            default_rules: NetClassRules {
                name: "Default".into(),
                trace_width: 0.25,
                clearance: 0.15,
                via_diameter: 0.6,
                via_drill: 0.3,
                diff_pair_gap: None,
                diff_pair_width: None,
            },
            class_rules: vec![],
            net_class_assignments: Default::default(),
            edge_clearance: 0.25,
            hole_to_hole: 0.25,
            min_annular_ring: 0.13,
            min_drill: 0.2,
        },
        footprints,
        traces,
        trace_arcs: vec![],
        vias,
        zones: vec![],
        keepouts: vec![],
        net_ties: vec![],
    })
}

/// Eagle rotation attribute: `R90`, `MR180` (mirrored), absent = 0.
fn parse_rot(attr: Option<&str>) -> (bool, f64) {
    let Some(a) = attr else { return (false, 0.0) };
    let mirrored = a.starts_with('M');
    let deg: f64 = a.trim_start_matches(['M', 'R']).parse().unwrap_or(0.0);
    (mirrored, deg)
}

/// Chain unordered outline segments into one vertex loop (greedy nearest-end).
fn chain_outline(segs: &[(Vec2, Vec2)]) -> Vec<Vec2> {
    if segs.is_empty() {
        return vec![];
    }
    let mut rest: Vec<(Vec2, Vec2)> = segs.to_vec();
    let (a0, b0) = rest.remove(0);
    let mut out = vec![a0, b0];
    while !rest.is_empty() {
        let tail = *out.last().unwrap();
        let mut best: Option<(usize, bool, f64)> = None;
        for (i, (a, b)) in rest.iter().enumerate() {
            let da = (tail - *a).length();
            let db = (tail - *b).length();
            if best.map(|(_, _, d)| da < d).unwrap_or(true) {
                best = Some((i, false, da));
            }
            if best.map(|(_, _, d)| db < d).unwrap_or(true) {
                best = Some((i, true, db));
            }
        }
        let Some((i, flip, d)) = best else { break };
        if d > 1.0 {
            break; // disconnected graphics; keep the main loop
        }
        let (a, b) = rest.remove(i);
        out.push(if flip { a } else { b });
    }
    // Drop the closing duplicate if present.
    if out.len() > 2 && (*out.first().unwrap() - *out.last().unwrap()).length() < 1e-3 {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_board() {
        let brd = r##"<?xml version="1.0"?>
<eagle version="9.6.0"><drawing><board>
<plain>
  <wire x1="0" y1="0" x2="10" y2="0" width="0.05" layer="20"/>
  <wire x1="10" y1="0" x2="10" y2="8" width="0.05" layer="20"/>
  <wire x1="10" y1="8" x2="0" y2="8" width="0.05" layer="20"/>
  <wire x1="0" y1="8" x2="0" y2="0" width="0.05" layer="20"/>
</plain>
<libraries><library name="l">
  <package name="R"><smd name="1" x="-0.5" y="0" dx="0.6" dy="0.5" layer="1"/>
                    <smd name="2" x="0.5" y="0" dx="0.6" dy="0.5" layer="1"/></package>
</library></libraries>
<elements>
  <element name="R1" library="l" package="R" x="2" y="4"/>
  <element name="R2" library="l" package="R" x="8" y="4" rot="MR90"/>
</elements>
<signals>
  <signal name="N1">
    <contactref element="R1" pad="2"/><contactref element="R2" pad="1"/>
    <wire x1="2.5" y1="4" x2="7.5" y2="4" width="0.2" layer="1"/>
    <via x="5" y="4" extent="1-16" drill="0.3" diameter="0.6"/>
  </signal>
</signals>
</board></drawing></eagle>"##;
        let pcb = parse_eagle_brd(brd).expect("parse");
        assert_eq!(pcb.outline.vertices.len(), 4);
        assert_eq!(pcb.footprints.len(), 2);
        assert_eq!(pcb.traces.len(), 1);
        assert_eq!(pcb.vias.len(), 1);
        // Net assignment reached the pads.
        let r1 = &pcb.footprints[0];
        assert_eq!(
            r1.pads.iter().find(|p| p.number == "2").unwrap().net,
            Some("N1".into())
        );
        // Mirrored element lands on the back.
        assert!(!pcb.footprints[1].front);
        assert_eq!(pcb.footprints[1].pads[0].layers, vec![PcbLayer::BCu]);
    }
}
