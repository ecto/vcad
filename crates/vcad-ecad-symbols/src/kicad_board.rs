//! Board from a schematic: KiCad library footprints placed on an outline, with
//! nets taken from the sheet's declared netlist.
//!
//! The schematic says *what* connects (`sheet.nets`, "REF.PIN" lists) and *which*
//! footprint each part uses (`footprint_id`, "Library:Name"). This module turns
//! that plus a placement into a [`Pcb`]: it loads each footprint from KiCad's
//! own `.pretty` libraries, converts its pads and graphics, places it, and puts
//! every pad on the net its pin is on (pad number = pin number, KiCad's rule).
//! Routing is left to the router.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use vcad_ir::ecad::*;
use vcad_ir::Vec2;

use crate::kicad_mod::{parse_footprint_file, FootprintDef, GraphicDef};

/// Directories searched for KiCad `.pretty` footprint libraries:
/// `$VCAD_KICAD_FOOTPRINTS` first (a path list, `:`-separated - put your own
/// libraries such as brand art here), then the standard KiCad install locations.
pub fn footprint_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Ok(d) = std::env::var("VCAD_KICAD_FOOTPRINTS") {
        dirs.extend(std::env::split_paths(&d));
    }
    for d in [
        "/Applications/KiCad/KiCad.app/Contents/SharedSupport/footprints",
        "/usr/share/kicad/footprints",
        "/usr/local/share/kicad/footprints",
        "C:/Program Files/KiCad/9.0/share/kicad/footprints",
    ] {
        dirs.push(d.into());
    }
    dirs
}

/// Load a footprint by KiCad id ("Library:Name"). Cached per process.
pub fn load_footprint(id: &str) -> Result<FootprintDef, String> {
    static CACHE: OnceLock<Mutex<HashMap<String, FootprintDef>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(d) = cache.lock().map_err(|e| e.to_string())?.get(id) {
        return Ok(d.clone());
    }
    let (lib, name) = id
        .split_once(':')
        .ok_or_else(|| format!("footprint '{id}' must be Library:Name"))?;
    let dirs = footprint_dirs();
    let path = dirs
        .iter()
        .map(|d| {
            d.join(format!("{lib}.pretty"))
                .join(format!("{name}.kicad_mod"))
        })
        .find(|p| p.exists())
        .ok_or_else(|| {
            format!("footprint '{id}' not found (set VCAD_KICAD_FOOTPRINTS; searched {dirs:?})")
        })?;
    let lib = parse_footprint_file(&path.to_string_lossy())
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let def = lib
        .footprints
        .into_iter()
        .next()
        .ok_or_else(|| format!("{} has no footprint", path.display()))?;
    cache
        .lock()
        .map_err(|e| e.to_string())?
        .insert(id.to_string(), def.clone());
    Ok(def)
}

/// The copper layers of an `n`-layer board, top to bottom.
pub fn copper_layers(n: usize) -> Vec<PcbLayer> {
    let inner = [
        PcbLayer::In1Cu,
        PcbLayer::In2Cu,
        PcbLayer::In3Cu,
        PcbLayer::In4Cu,
        PcbLayer::In5Cu,
        PcbLayer::In6Cu,
    ];
    let mut v = vec![PcbLayer::FCu];
    v.extend(inner.iter().take(n.saturating_sub(2)).copied());
    if n >= 2 {
        v.push(PcbLayer::BCu);
    }
    v
}

/// A KiCad layer name ("F.Cu", "*.Cu", "*.Mask") expanded to layers.
fn expand_layer(name: &str, copper: &[PcbLayer]) -> Vec<PcbLayer> {
    match name {
        "*.Cu" => copper.to_vec(),
        "*.Mask" => vec![PcbLayer::FMask, PcbLayer::BMask],
        "*.Paste" => vec![PcbLayer::FPaste, PcbLayer::BPaste],
        n => serde_json::from_str::<PcbLayer>(&format!("\"{n}\""))
            .map(|l| vec![l])
            .unwrap_or_default(),
    }
}

fn layer_of(name: &str) -> Option<PcbLayer> {
    serde_json::from_str::<PcbLayer>(&format!("\"{name}\"")).ok()
}

/// A footprint definition instanced on the board (front side).
pub fn place_footprint(
    def: &FootprintDef,
    id: &str,
    reference: &str,
    value: &str,
    pos: Vec2,
    rotation: f64,
    copper: &[PcbLayer],
) -> Footprint {
    let pads = def
        .pads
        .iter()
        .map(|p| Pad {
            number: p.number.clone(),
            pad_type: p.pad_type,
            shape: p.shape.clone(),
            position: Vec2::new(p.position.0, p.position.1),
            rotation: p.rotation,
            drill: p.drill.clone(),
            net: None,
            layers: {
                let mut ls: Vec<PcbLayer> = p
                    .layers
                    .iter()
                    .flat_map(|l| expand_layer(l, copper))
                    .collect();
                ls.dedup();
                ls
            },
        })
        .collect();
    let v = |p: (f64, f64)| Vec2::new(p.0, p.1);
    let mut graphics = Vec::new();
    for g in &def.graphics {
        match g {
            GraphicDef::Line {
                start,
                end,
                width,
                layer,
            } => {
                if let Some(layer) = layer_of(layer) {
                    graphics.push(FootprintGraphic::Line {
                        start: v(*start),
                        end: v(*end),
                        width: *width,
                        layer,
                    });
                }
            }
            GraphicDef::Rect {
                start,
                end,
                width,
                layer,
            } => {
                if let Some(layer) = layer_of(layer) {
                    let (a, b) = (v(*start), v(*end));
                    let c = [a, Vec2::new(b.x, a.y), b, Vec2::new(a.x, b.y)];
                    for k in 0..4 {
                        graphics.push(FootprintGraphic::Line {
                            start: c[k],
                            end: c[(k + 1) % 4],
                            width: *width,
                            layer,
                        });
                    }
                }
            }
            GraphicDef::Poly {
                points,
                width,
                layer,
            } => {
                if let Some(layer) = layer_of(layer) {
                    graphics.push(FootprintGraphic::Polygon {
                        vertices: points.iter().map(|p| v(*p)).collect(),
                        width: *width,
                        layer,
                    });
                }
            }
            // user text (art, labels); reference/value fields are generated by the writer
            GraphicDef::Text {
                text_type,
                content,
                position,
                layer,
                rotation,
                size,
                thickness,
            } if text_type == "user" => {
                if let Some(layer) = layer_of(layer) {
                    graphics.push(FootprintGraphic::Text {
                        text: content.clone(),
                        position: v(*position),
                        rotation: *rotation,
                        height: *size,
                        width: *thickness,
                        layer,
                    });
                }
            }
            GraphicDef::Circle {
                center,
                end,
                width,
                layer,
            } => {
                if let Some(layer) = layer_of(layer) {
                    let r = ((end.0 - center.0).powi(2) + (end.1 - center.1).powi(2)).sqrt();
                    graphics.push(FootprintGraphic::Circle {
                        center: v(*center),
                        radius: r,
                        width: *width,
                        layer,
                    });
                }
            }
            _ => {}
        }
    }
    let mut properties = HashMap::new();
    properties.insert("footprint_id".to_string(), id.to_string());
    Footprint {
        reference: reference.to_string(),
        value: value.to_string(),
        footprint_name: id.to_string(),
        position: pos,
        rotation,
        front: true,
        pads,
        graphics,
        model_3d: def.model_3d.clone(),
        properties,
    }
}

/// Where one part goes.
#[derive(Debug, Clone, PartialEq)]
pub struct Placement {
    /// Reference designator.
    pub reference: String,
    /// Position (mm).
    pub position: Vec2,
    /// Rotation (degrees).
    pub rotation: f64,
}

/// Board shape and process.
#[derive(Debug, Clone, PartialEq)]
pub struct BoardSpec {
    /// Outline vertices (closed).
    pub outline: Vec<Vec2>,
    /// Thickness (mm).
    pub thickness: f64,
    /// Copper layer count (2, 4, 6, 8).
    pub layers: usize,
    /// Design rules.
    pub rules: DesignRules,
    /// Copper pours: (net, layer).
    pub pours: Vec<(String, PcbLayer)>,
    /// Footprints that are not on the schematic - logos, art, labels, fiducials:
    /// (reference, footprint id, placement). No nets.
    pub art: Vec<(String, String, Placement)>,
}

/// JLC-class 4-layer defaults: 0.15 mm trace / clearance, 0.45 / 0.25 mm vias.
pub fn default_rules() -> DesignRules {
    DesignRules {
        default_rules: NetClassRules {
            name: "Default".into(),
            trace_width: 0.15,
            clearance: 0.15,
            via_diameter: 0.45,
            via_drill: 0.25,
            diff_pair_gap: None,
            diff_pair_width: None,
            target_impedance: None,
            target_diff_impedance: None,
        },
        class_rules: vec![],
        net_class_assignments: HashMap::new(),
        edge_clearance: 0.3,
        hole_to_hole: 0.25,
        min_annular_ring: 0.1,
        min_drill: 0.2,
    }
}

/// Build the board: every placed schematic component gets its library footprint,
/// and each pad is put on the net its same-numbered pin is on. Components with
/// no footprint (power flags) and unplaced components are skipped; placing a
/// component the sheet doesn't have is an error.
pub fn board_from_sheet(
    sheet: &SchematicSheet,
    places: &[Placement],
    spec: &BoardSpec,
) -> Result<Pcb, String> {
    let copper = copper_layers(spec.layers);
    // pin -> net
    let mut pin_net: HashMap<(String, String), String> = HashMap::new();
    let mut net_names: BTreeMap<String, ()> = BTreeMap::new();
    if let Some(nets) = &sheet.nets {
        for (net, pins) in nets {
            net_names.insert(net.clone(), ());
            for p in pins {
                if let Some((r, n)) = p.split_once('.') {
                    pin_net.insert((r.to_string(), n.to_string()), net.clone());
                }
            }
        }
    }
    let comps: HashMap<&str, &SchematicComponent> = sheet
        .components
        .iter()
        .map(|c| (c.reference.as_str(), c))
        .collect();
    let mut footprints = Vec::new();
    for pl in places {
        let comp = comps
            .get(pl.reference.as_str())
            .ok_or_else(|| format!("placed '{}' is not on the schematic", pl.reference))?;
        if comp.footprint_id.is_empty() {
            continue;
        }
        let def =
            load_footprint(&comp.footprint_id).map_err(|e| format!("{}: {e}", pl.reference))?;
        let mut fp = place_footprint(
            &def,
            &comp.footprint_id,
            &comp.reference,
            &comp.value,
            pl.position,
            pl.rotation,
            &copper,
        );
        for pad in &mut fp.pads {
            pad.net = pin_net
                .get(&(comp.reference.clone(), pad.number.clone()))
                .cloned();
        }
        footprints.push(fp);
    }
    for (reference, id, pl) in &spec.art {
        let def = load_footprint(id).map_err(|e| format!("{reference}: {e}"))?;
        let mut fp = place_footprint(&def, id, reference, "", pl.position, pl.rotation, &copper);
        fp.properties.insert("art".into(), "1".into());
        footprints.push(fp);
    }
    let stackup = LayerStackup {
        layers: copper
            .iter()
            .map(|l| StackupLayer {
                layer: *l,
                copper_thickness: Some(0.035),
                dielectric_thickness: Some(spec.thickness / spec.layers as f64),
                dielectric_er: Some(4.4),
                material: Some("FR4".into()),
            })
            .collect(),
    };
    let zones = spec
        .pours
        .iter()
        .map(|(net, layer)| Zone {
            outline: spec.outline.clone(),
            holes: vec![],
            net: net.clone(),
            layer: *layer,
            clearance: spec.rules.default_rules.clearance.max(0.2),
            min_area: 0.5,
            fill_type: ZoneFillType::Solid,
            thermal_relief: ThermalReliefStyle::Relief,
            thermal_gap: Some(0.3),
            thermal_spoke_width: Some(0.4),
            priority: 0,
        })
        .collect();
    Ok(Pcb {
        outline: BoardOutline {
            vertices: spec.outline.clone(),
            cutouts: vec![],
            thickness: spec.thickness,
        },
        stackup,
        nets: net_names
            .keys()
            .map(|n| Net {
                id: n.clone(),
                name: n.clone(),
            })
            .collect(),
        rules: spec.rules.clone(),
        footprints,
        traces: vec![],
        trace_arcs: vec![],
        vias: vec![],
        zones,
        keepouts: vec![],
        net_ties: vec![],
    })
}

/// Rounded-rectangle outline from (x0, y0) to (x1, y1) with corner radius `r`.
pub fn rounded_rect(x0: f64, y0: f64, x1: f64, y1: f64, r: f64) -> Vec<Vec2> {
    if r <= 0.0 {
        return vec![
            Vec2::new(x0, y0),
            Vec2::new(x1, y0),
            Vec2::new(x1, y1),
            Vec2::new(x0, y1),
        ];
    }
    let mut v = Vec::new();
    let corners = [
        (x1 - r, y0 + r, -90.0f64),
        (x1 - r, y1 - r, 0.0),
        (x0 + r, y1 - r, 90.0),
        (x0 + r, y0 + r, 180.0),
    ];
    for (cx, cy, a0) in corners {
        for k in 0..=8 {
            let a = (a0 + 90.0 * k as f64 / 8.0).to_radians();
            v.push(Vec2::new(cx + r * a.cos(), cy + r * a.sin()));
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kicad_available() -> bool {
        load_footprint("Resistor_SMD:R_0402_1005Metric").is_ok()
    }

    #[test]
    fn layers_expand_for_tht_and_smd() {
        let cu = copper_layers(4);
        assert_eq!(
            cu,
            vec![
                PcbLayer::FCu,
                PcbLayer::In1Cu,
                PcbLayer::In2Cu,
                PcbLayer::BCu
            ]
        );
        assert_eq!(expand_layer("*.Cu", &cu).len(), 4);
        assert_eq!(expand_layer("F.Cu", &cu), vec![PcbLayer::FCu]);
    }

    #[test]
    fn pads_get_the_nets_of_their_pins() {
        if !kicad_available() {
            return; // needs a KiCad install
        }
        let mut nets = BTreeMap::new();
        nets.insert("A".to_string(), vec!["R1.1".to_string()]);
        nets.insert("B".to_string(), vec!["R1.2".to_string()]);
        let sheet = SchematicSheet {
            title: None,
            components: vec![SchematicComponent {
                reference: "R1".into(),
                value: "10k".into(),
                footprint_id: "Resistor_SMD:R_0402_1005Metric".into(),
                position: Vec2::new(0.0, 0.0),
                rotation: 0.0,
                mirror: false,
                pins: vec![],
                pads_override: None,
                properties: HashMap::new(),
            }],
            wires: vec![],
            junctions: vec![],
            labels: vec![],
            nets: Some(nets),
        };
        let spec = BoardSpec {
            outline: rounded_rect(0.0, 0.0, 10.0, 10.0, 1.0),
            thickness: 1.6,
            layers: 4,
            rules: default_rules(),
            pours: vec![],
            art: vec![],
        };
        let pcb = board_from_sheet(
            &sheet,
            &[Placement {
                reference: "R1".into(),
                position: Vec2::new(5.0, 5.0),
                rotation: 0.0,
            }],
            &spec,
        )
        .unwrap();
        let fp = &pcb.footprints[0];
        assert_eq!(fp.pads.len(), 2);
        let by: HashMap<_, _> = fp
            .pads
            .iter()
            .map(|p| (p.number.as_str(), p.net.clone()))
            .collect();
        assert_eq!(by["1"].as_deref(), Some("A"));
        assert_eq!(by["2"].as_deref(), Some("B"));
        assert!(fp.pads[0].layers.contains(&PcbLayer::FCu));
    }
}
