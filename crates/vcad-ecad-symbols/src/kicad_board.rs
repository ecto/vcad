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

/// The `.kicad_mod` source of a footprint by KiCad id, cached - carried on the
/// placed footprint so the writer can emit the library footprint faithfully.
pub fn footprint_source(id: &str) -> Result<String, String> {
    static CACHE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(t) = cache.lock().map_err(|e| e.to_string())?.get(id) {
        return Ok(t.clone());
    }
    let (lib, name) = id
        .split_once(':')
        .ok_or_else(|| format!("footprint '{id}' must be Library:Name"))?;
    let path = footprint_dirs()
        .iter()
        .map(|d| {
            d.join(format!("{lib}.pretty"))
                .join(format!("{name}.kicad_mod"))
        })
        .find(|p| p.exists())
        .ok_or_else(|| format!("footprint '{id}' not found"))?;
    let t = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    cache
        .lock()
        .map_err(|e| e.to_string())?
        .insert(id.to_string(), t.clone());
    Ok(t)
}

/// The copper layers of an `n`-layer board, top to bottom. Errors above 32
/// (KiCad's maximum) rather than silently building a thinner board.
pub fn copper_layers(n: usize) -> Result<Vec<PcbLayer>, String> {
    PcbLayer::copper_stack(n)
        .ok_or_else(|| format!("{n}-layer board exceeds the 32 copper layers vcad supports"))
}

/// A KiCad layer name ("F.Cu", "*.Cu", "F&B.Cu", "*.Mask") expanded to layers.
/// A name with no vcad layer (e.g. "F.Adhes") expands to nothing, with a
/// warning on stderr.
fn expand_layer(name: &str, copper: &[PcbLayer]) -> Vec<PcbLayer> {
    match name {
        "*.Cu" => copper.to_vec(),
        "F&B.Cu" => vec![PcbLayer::FCu, PcbLayer::BCu],
        "*.Mask" | "F&B.Mask" => vec![PcbLayer::FMask, PcbLayer::BMask],
        "*.Paste" | "F&B.Paste" => vec![PcbLayer::FPaste, PcbLayer::BPaste],
        "*.SilkS" | "F&B.SilkS" => vec![PcbLayer::FSilkS, PcbLayer::BSilkS],
        n => match layer_of(n) {
            Some(l) => vec![l],
            None => {
                eprintln!("kicad footprint: layer {n:?} has no vcad equivalent; dropped");
                Vec::new()
            }
        },
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
    // Designator: on the fab layer for small parts (an 0402's silkscreen
    // designator lands on its own pads and its neighbours'), just above the
    // courtyard for the rest.
    let crt: Vec<Vec2> = graphics
        .iter()
        .filter_map(|g| match g {
            FootprintGraphic::Line {
                start,
                end,
                layer: PcbLayer::FCrtYd,
                ..
            } => Some([*start, *end]),
            _ => None,
        })
        .flatten()
        .collect();
    if !crt.is_empty() {
        let (x0, x1) = crt
            .iter()
            .fold((f64::MAX, f64::MIN), |a, p| (a.0.min(p.x), a.1.max(p.x)));
        let (y0, y1) = crt
            .iter()
            .fold((f64::MAX, f64::MIN), |a, p| (a.0.min(p.y), a.1.max(p.y)));
        if (x1 - x0).max(y1 - y0) < 4.0 {
            properties.insert("ref_layer".into(), "F.Fab".into());
        } else {
            properties.insert("ref_at".into(), format!("{} {}", (x0 + x1) / 2.0, y0 - 0.8));
        }
    }
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
    /// On the back side (KiCad's flip: layers swap, local y mirrors).
    pub back: bool,
}

/// Flip a placed footprint to the back side the way KiCad does: every front
/// layer becomes its back twin, local coordinates mirror in y (`y -> -y`) and
/// pad angles negate, while position and rotation stay. World geometry then
/// matches KiCad's `FOOTPRINT::Flip(_, FLIP_LEFT_RIGHT)` at the same angle.
pub fn flip_to_back(fp: &mut Footprint) {
    let m = |p: Vec2| Vec2::new(p.x, -p.y);
    for pad in &mut fp.pads {
        pad.position = m(pad.position);
        pad.rotation = -pad.rotation;
        for l in &mut pad.layers {
            *l = l.flipped();
        }
    }
    for g in &mut fp.graphics {
        match g {
            FootprintGraphic::Line {
                start, end, layer, ..
            } => {
                *start = m(*start);
                *end = m(*end);
                *layer = layer.flipped();
            }
            FootprintGraphic::Polygon {
                vertices, layer, ..
            } => {
                for v in vertices.iter_mut() {
                    *v = m(*v);
                }
                *layer = layer.flipped();
            }
            FootprintGraphic::Circle { center, layer, .. } => {
                *center = m(*center);
                *layer = layer.flipped();
            }
            FootprintGraphic::Text {
                position,
                rotation,
                layer,
                ..
            } => {
                *position = m(*position);
                *rotation = -*rotation;
                *layer = layer.flipped();
            }
            #[allow(unreachable_patterns)]
            _ => {}
        }
    }
    if let Some(at) = fp.properties.get("ref_at").cloned() {
        let mut it = at.split_whitespace().filter_map(|v| v.parse::<f64>().ok());
        if let (Some(x), Some(y)) = (it.next(), it.next()) {
            fp.properties.insert("ref_at".into(), format!("{x} {}", -y));
        }
    }
    if let Some(l) = fp.properties.get("ref_layer").cloned() {
        fp.properties
            .insert("ref_layer".into(), l.replacen("F.", "B.", 1));
    }
    fp.front = false;
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
    /// Pours over a region: (net, layer, outline, priority). Higher priority fills first.
    pub pour_regions: Vec<(String, PcbLayer, Vec<Vec2>, u32)>,
    /// Footprints that are not on the schematic - logos, art, labels, fiducials:
    /// (reference, footprint id, placement). No nets.
    pub art: Vec<(String, String, Placement)>,
}

/// JLC-class 4-layer defaults: 0.15 mm trace / clearance, 0.45 / 0.25 mm vias.
pub fn default_rules() -> DesignRules {
    DesignRules {
        min_microvia_drill: 0.1,
        min_microvia_diameter: 0.2,
        default_rules: NetClassRules {
            neck_width: None,
            neck_length: None,
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
    let copper = copper_layers(spec.layers)?;
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
    let mut keepouts = Vec::new();
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
        if pl.back {
            flip_to_back(&mut fp);
        }
        for pad in &mut fp.pads {
            pad.net = pin_net
                .get(&(comp.reference.clone(), pad.number.clone()))
                .cloned();
        }
        // Every connected pin must land on a pad: KiCad matches pin numbers to
        // pad numbers, so a symbol whose pins are named (G/D/S) on a footprint
        // whose pads are numbered (1/2/3) silently leaves the part unconnected.
        let missing: Vec<&str> = pin_net
            .keys()
            .filter(|(r, _)| r == &comp.reference)
            .map(|(_, n)| n.as_str())
            .filter(|n| !fp.pads.iter().any(|p| p.number == *n))
            .collect();
        if !missing.is_empty() {
            let mut m = missing.clone();
            m.sort_unstable();
            return Err(format!(
                "{}: connected pin(s) {} have no pad on footprint {} (pads: {})",
                comp.reference,
                m.join(", "),
                comp.footprint_id,
                {
                    let mut ps: Vec<&str> = fp
                        .pads
                        .iter()
                        .map(|p| p.number.as_str())
                        .filter(|n| !n.is_empty())
                        .collect();
                    ps.sort_unstable();
                    ps.dedup();
                    ps.join(", ")
                }
            ));
        }
        if let Ok(src) = footprint_source(&comp.footprint_id) {
            keepouts.extend(footprint_keepouts(&src, &fp, &copper));
            fp.properties.insert("kicad_mod".into(), src);
        }
        footprints.push(fp);
    }
    for (reference, id, pl) in &spec.art {
        let def = load_footprint(id).map_err(|e| format!("{reference}: {e}"))?;
        let mut fp = place_footprint(&def, id, reference, "", pl.position, pl.rotation, &copper);
        fp.properties.insert("art".into(), "1".into());
        if let Ok(src) = footprint_source(id) {
            // art can carry keepouts too (a copper logo, a gold edge rim)
            keepouts.extend(footprint_keepouts(&src, &fp, &copper));
            fp.properties.insert("kicad_mod".into(), src);
        }
        footprints.push(fp);
    }
    place_designators(&mut footprints, &spec.outline);
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
        .chain(
            spec.pour_regions
                .iter()
                .map(|(net, layer, outline, priority)| Zone {
                    outline: outline.clone(),
                    holes: vec![],
                    net: net.clone(),
                    layer: *layer,
                    clearance: spec.rules.default_rules.clearance.max(0.2),
                    min_area: 0.5,
                    fill_type: ZoneFillType::Solid,
                    thermal_relief: ThermalReliefStyle::Direct, // high-current pads connect solid
                    thermal_gap: None,
                    thermal_spoke_width: None,
                    priority: *priority,
                }),
        )
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
        keepouts,
        net_ties: vec![],
    })
}

/// A footprint's keepout zones (`(zone … (keepout …))` in its library source),
/// placed in board coordinates, so the router and DRC respect them - a
/// Tag-Connect's no-via field between its pogo pins, for one.
fn footprint_keepouts(src: &str, fp: &Footprint, copper: &[PcbLayer]) -> Vec<Keepout> {
    use crate::sexpr::parse_sexpr;
    let Ok((_, root)) = parse_sexpr(src) else {
        return vec![];
    };
    let (sn, cs) = fp.rotation.to_radians().sin_cos();
    let mut out = Vec::new();
    for z in root.find_all("zone") {
        let Some(k) = z.find("keepout") else { continue };
        let rule = |name: &str| {
            k.find(name)
                .and_then(|n| n.children())
                .and_then(|c| c.get(1))
                .and_then(|v| v.as_str())
                == Some("not_allowed")
        };
        let pts: Vec<Vec2> = z
            .find("polygon")
            .and_then(|p| p.find("pts"))
            .map(|p| {
                p.find_all("xy")
                    .into_iter()
                    .filter_map(|xy| {
                        let c = xy.children()?;
                        let (x, y) = (c.get(1)?.as_f64()?, c.get(2)?.as_f64()?);
                        Some(Vec2::new(
                            fp.position.x + x * cs - y * sn,
                            fp.position.y + x * sn + y * cs,
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        if pts.len() < 3 {
            continue;
        }
        let mut layers: Vec<PcbLayer> = Vec::new();
        for key in ["layer", "layers"] {
            if let Some(c) = z.find(key).and_then(|n| n.children()) {
                for v in c.iter().skip(1).filter_map(|v| v.as_str()) {
                    layers.extend(expand_layer(v, copper));
                }
            }
        }
        out.push(Keepout {
            outline: pts,
            layers,
            no_tracks: rule("tracks"),
            no_vias: rule("vias"),
            no_pour: rule("copperpour"),
            no_components: rule("footprints"),
        });
    }
    out
}

/// World-space axis-aligned box of a footprint's graphics on `layers` (courtyard,
/// or silkscreen for art), using vcad's footprint rotation.
fn world_box(fp: &Footprint, layers: &[PcbLayer]) -> Option<(f64, f64, f64, f64)> {
    let (s, c) = fp.rotation.to_radians().sin_cos();
    let w = |p: Vec2| {
        (
            fp.position.x + p.x * c - p.y * s,
            fp.position.y + p.x * s + p.y * c,
        )
    };
    let mut b: Option<(f64, f64, f64, f64)> = None;
    let mut add = |q: (f64, f64)| {
        b = Some(match b {
            None => (q.0, q.1, q.0, q.1),
            Some(o) => (o.0.min(q.0), o.1.min(q.1), o.2.max(q.0), o.3.max(q.1)),
        })
    };
    for g in &fp.graphics {
        match g {
            FootprintGraphic::Line {
                start, end, layer, ..
            } if layers.contains(layer) => {
                add(w(*start));
                add(w(*end));
            }
            FootprintGraphic::Circle {
                center,
                radius,
                layer,
                ..
            } if layers.contains(layer) => {
                let q = w(*center);
                add((q.0 - radius, q.1 - radius));
                add((q.0 + radius, q.1 + radius));
            }
            _ => {}
        }
    }
    b
}

/// Put each silkscreen designator where it is readable: just outside its own
/// courtyard (above, below, left, right - first free spot), clear of every
/// other part's courtyard, art, the board edge and the designators already
/// placed. A designator with nowhere to go moves to the fab layer (it stays in
/// the assembly drawing). Runs after placement, because a designator collides
/// with the *neighbours*, which only the whole board knows.
#[allow(clippy::needless_range_loop)] // fps[i] is mutated after reading the others
fn place_designators(fps: &mut [Footprint], outline: &[Vec2]) {
    let (bx0, by0, bx1, by1) = outline
        .iter()
        .fold((f64::MAX, f64::MAX, f64::MIN, f64::MIN), |a, p| {
            (a.0.min(p.x), a.1.min(p.y), a.2.max(p.x), a.3.max(p.y))
        });
    let edge = 0.4;
    let obstacles: Vec<(usize, (f64, f64, f64, f64))> = fps
        .iter()
        .enumerate()
        .filter_map(|(i, f)| {
            let layers: &[PcbLayer] = if f.properties.contains_key("art") {
                &[PcbLayer::FSilkS]
            } else {
                // a footprint's silk can reach past its courtyard
                &[PcbLayer::FCrtYd, PcbLayer::FSilkS]
            };
            world_box(f, layers).map(|b| (i, b))
        })
        .collect();
    let hit = |a: (f64, f64, f64, f64), b: (f64, f64, f64, f64)| {
        !(a.2 <= b.0 || b.2 <= a.0 || a.3 <= b.1 || b.3 <= a.1)
    };
    let mut taken: Vec<(f64, f64, f64, f64)> = Vec::new();
    for i in 0..fps.len() {
        let f = &fps[i];
        if f.properties.contains_key("art")
            || f.properties.get("ref_layer").map(String::as_str) == Some("F.Fab")
        {
            continue;
        }
        // Small passives carry their designator on the fab layer (assembly
        // drawing), not the silk: a 0402 field of R/C labels is noise, and it
        // crowds out the labels that matter (ICs, connectors, power parts).
        let prefix: String = f
            .reference
            .chars()
            .take_while(|c| c.is_ascii_alphabetic())
            .collect();
        if matches!(prefix.as_str(), "R" | "C" | "FB" | "TH" | "PF") {
            fps[i].properties.insert("ref_layer".into(), "F.Fab".into());
            fps[i].properties.remove("ref_at");
            continue;
        }
        let Some(own) = world_box(f, &[PcbLayer::FCrtYd]) else {
            continue;
        };
        // text: 1 mm font, ~0.8 mm advance per character, plus a little margin
        let (tw, th) = (0.8 * f.reference.chars().count() as f64 + 0.3, 1.3);
        let (cx, cy) = ((own.0 + own.2) / 2.0, (own.1 + own.3) / 2.0);
        let spots = [
            (cx, own.1 - th / 2.0 - 0.2),
            (cx, own.3 + th / 2.0 + 0.2),
            (own.0 - tw / 2.0 - 0.2, cy),
            (own.2 + tw / 2.0 + 0.2, cy),
        ];
        let mut chosen = None;
        for (x, y) in spots {
            // text keeps the footprint's rotation: 90/270 swaps its extents
            let r = f.rotation.rem_euclid(180.0);
            let (hw, hh) = if (r - 90.0).abs() < 1.0 {
                (th / 2.0, tw / 2.0)
            } else {
                (tw / 2.0, th / 2.0)
            };
            let b = (x - hw, y - hh, x + hw, y + hh);
            // stroke width and glyph overhang: test a slightly grown box
            let m = 0.15;
            let g = (b.0 - m, b.1 - m, b.2 + m, b.3 + m);
            let inside =
                b.0 >= bx0 + edge && b.1 >= by0 + edge && b.2 <= bx1 - edge && b.3 <= by1 - edge;
            if inside
                && !obstacles.iter().any(|(j, o)| *j != i && hit(g, *o))
                && !taken.iter().any(|t| hit(g, *t))
            {
                chosen = Some(((x, y), b));
                break;
            }
        }
        let f = &mut fps[i];
        match chosen {
            Some(((x, y), b)) => {
                // back to footprint coordinates (inverse of vcad's rotation)
                let (s, c) = f.rotation.to_radians().sin_cos();
                let (dx, dy) = (x - f.position.x, y - f.position.y);
                let local = (dx * c + dy * s, -dx * s + dy * c);
                f.properties
                    .insert("ref_at".into(), format!("{} {}", local.0, local.1));
                taken.push(b);
            }
            None => {
                f.properties.insert("ref_layer".into(), "F.Fab".into());
                f.properties.remove("ref_at");
            }
        }
    }
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
    fn a_connected_pin_without_a_pad_is_an_error() {
        if !kicad_available() {
            return;
        }
        let mut nets = BTreeMap::new();
        nets.insert("A".to_string(), vec!["R1.G".to_string()]);
        let sheet = SchematicSheet {
            title: None,
            components: vec![SchematicComponent {
                reference: "R1".into(),
                value: "x".into(),
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
            layers: 2,
            rules: default_rules(),
            pours: vec![],
            pour_regions: vec![],
            art: vec![],
        };
        let e = board_from_sheet(
            &sheet,
            &[Placement {
                reference: "R1".into(),
                position: Vec2::new(5.0, 5.0),
                rotation: 0.0,
                back: false,
            }],
            &spec,
        )
        .unwrap_err();
        assert!(e.contains("R1") && e.contains("G"), "{e}");
    }

    #[test]
    fn back_side_mirrors_local_y_and_swaps_layers() {
        if !kicad_available() {
            return;
        }
        let def = load_footprint("Package_TO_SOT_SMD:SOT-23-5").unwrap();
        let cu = copper_layers(4).unwrap();
        let front = place_footprint(&def, "x", "U1", "", Vec2::new(0.0, 0.0), 90.0, &cu);
        let mut back = front.clone();
        flip_to_back(&mut back);
        assert!(!back.front);
        for (a, b) in front.pads.iter().zip(&back.pads) {
            assert_eq!(a.position.x, b.position.x);
            assert_eq!(a.position.y, -b.position.y);
            assert!(b.layers.contains(&PcbLayer::BCu) && !b.layers.contains(&PcbLayer::FCu));
        }
    }

    #[test]
    fn layers_expand_for_tht_and_smd() {
        let cu = copper_layers(4).unwrap();
        assert_eq!(copper_layers(12).unwrap()[10], PcbLayer::In10Cu);
        assert!(copper_layers(33).is_err());
        assert_eq!(
            expand_layer("F&B.Cu", &cu),
            vec![PcbLayer::FCu, PcbLayer::BCu]
        );
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
            pour_regions: vec![],
            art: vec![],
        };
        let pcb = board_from_sheet(
            &sheet,
            &[Placement {
                reference: "R1".into(),
                position: Vec2::new(5.0, 5.0),
                rotation: 0.0,
                back: false,
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
