//! Gerber RS-274X writer.
//!
//! Generates one `.gbr` file per PCB layer following the RS-274X extended Gerber
//! format. Coordinates use 4.6 format (4 integer digits, 6 decimal digits) in
//! millimetres, so 1 mm = 1_000_000 integer units.

use std::collections::HashMap;
use std::io::Write;

use vcad_ir::ecad::*;

/// Errors that can occur during Gerber generation.
#[derive(Debug, thiserror::Error)]
pub enum GerberError {
    /// An I/O error occurred while writing.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// The PCB contains no data for the requested layer.
    #[error("no data for layer {0:?}")]
    EmptyLayer(PcbLayer),
}

// ---------------------------------------------------------------------------
// Coordinate conversion
// ---------------------------------------------------------------------------

/// Convert an mm value to the integer representation used by 4.6 format.
/// 1 mm = 1_000_000 units.
fn mm_to_coord(mm: f64) -> i64 {
    (mm * 1_000_000.0).round() as i64
}

/// Format an integer coordinate for Gerber output (no decimal point, leading
/// sign only when negative).
fn fmt_coord(val: i64) -> String {
    format!("{val}")
}

// ---------------------------------------------------------------------------
// Aperture helpers
// ---------------------------------------------------------------------------

/// Aperture shape used in the aperture definition table.
#[derive(Debug, Clone, PartialEq)]
enum ApertureShape {
    Circle { diameter: f64 },
    Rect { width: f64, height: f64 },
    Oval { width: f64, height: f64 },
}

/// Unique key for de-duplicating apertures.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ApertureKey {
    /// Discriminant tag.
    kind: &'static str,
    /// Dimensions encoded as integer nanometres for exact hashing.
    dims: [i64; 2],
}

impl ApertureShape {
    fn key(&self) -> ApertureKey {
        match self {
            ApertureShape::Circle { diameter } => ApertureKey {
                kind: "C",
                dims: [mm_to_coord(*diameter), 0],
            },
            ApertureShape::Rect { width, height } => ApertureKey {
                kind: "R",
                dims: [mm_to_coord(*width), mm_to_coord(*height)],
            },
            ApertureShape::Oval { width, height } => ApertureKey {
                kind: "O",
                dims: [mm_to_coord(*width), mm_to_coord(*height)],
            },
        }
    }

    fn definition(&self, code: u32) -> String {
        match self {
            ApertureShape::Circle { diameter } => {
                format!("%ADD{code}C,{diameter:.6}*%")
            }
            ApertureShape::Rect { width, height } => {
                format!("%ADD{code}R,{width:.6}X{height:.6}*%")
            }
            ApertureShape::Oval { width, height } => {
                format!("%ADD{code}O,{width:.6}X{height:.6}*%")
            }
        }
    }
}

/// Maps unique aperture shapes to D-codes (starting at D10 per convention).
struct ApertureTable {
    map: HashMap<ApertureKey, u32>,
    shapes: Vec<ApertureShape>,
    next_code: u32,
}

impl ApertureTable {
    fn new() -> Self {
        Self {
            map: HashMap::new(),
            shapes: Vec::new(),
            next_code: 10,
        }
    }

    /// Register an aperture and return its D-code.
    fn register(&mut self, shape: ApertureShape) -> u32 {
        let key = shape.key();
        if let Some(&code) = self.map.get(&key) {
            return code;
        }
        let code = self.next_code;
        self.next_code += 1;
        self.map.insert(key, code);
        self.shapes.push(shape);
        code
    }

    /// Write all `%ADD...` definitions.
    fn write_definitions<W: Write>(&self, w: &mut W) -> Result<(), std::io::Error> {
        for (i, shape) in self.shapes.iter().enumerate() {
            let code = 10 + i as u32;
            writeln!(w, "{}", shape.definition(code))?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Layer name / file name helpers
// ---------------------------------------------------------------------------

/// Return the conventional Gerber filename suffix for a layer.
fn layer_filename(layer: PcbLayer) -> &'static str {
    match layer {
        PcbLayer::FCu => "F_Cu",
        PcbLayer::BCu => "B_Cu",
        PcbLayer::In1Cu => "In1_Cu",
        PcbLayer::In2Cu => "In2_Cu",
        PcbLayer::In3Cu => "In3_Cu",
        PcbLayer::In4Cu => "In4_Cu",
        PcbLayer::In5Cu => "In5_Cu",
        PcbLayer::In6Cu => "In6_Cu",
        PcbLayer::FSilkS => "F_SilkS",
        PcbLayer::BSilkS => "B_SilkS",
        PcbLayer::FMask => "F_Mask",
        PcbLayer::BMask => "B_Mask",
        PcbLayer::FPaste => "F_Paste",
        PcbLayer::BPaste => "B_Paste",
        PcbLayer::FFab => "F_Fab",
        PcbLayer::BFab => "B_Fab",
        PcbLayer::FCrtYd => "F_CrtYd",
        PcbLayer::BCrtYd => "B_CrtYd",
        PcbLayer::EdgeCuts => "Edge_Cuts",
        PcbLayer::UserDrawings => "User_Drawings",
        PcbLayer::UserComments => "User_Comments",
    }
}

/// Return the Gerber file attribute string for a layer (`.FileFunction`).
fn layer_file_function(layer: PcbLayer) -> &'static str {
    match layer {
        PcbLayer::FCu => "Copper,L1,Top",
        PcbLayer::BCu => "Copper,L2,Bot",
        PcbLayer::In1Cu => "Copper,L2,Inr",
        PcbLayer::In2Cu => "Copper,L3,Inr",
        PcbLayer::In3Cu => "Copper,L4,Inr",
        PcbLayer::In4Cu => "Copper,L5,Inr",
        PcbLayer::In5Cu => "Copper,L6,Inr",
        PcbLayer::In6Cu => "Copper,L7,Inr",
        PcbLayer::FSilkS => "Legend,Top",
        PcbLayer::BSilkS => "Legend,Bot",
        PcbLayer::FMask => "Soldermask,Top",
        PcbLayer::BMask => "Soldermask,Bot",
        PcbLayer::FPaste => "Paste,Top",
        PcbLayer::BPaste => "Paste,Bot",
        PcbLayer::FFab => "AssemblyDrawing,Top",
        PcbLayer::BFab => "AssemblyDrawing,Bot",
        PcbLayer::FCrtYd => "Other,FCrtYd",
        PcbLayer::BCrtYd => "Other,BCrtYd",
        PcbLayer::EdgeCuts => "Profile,NP",
        PcbLayer::UserDrawings => "Other,User",
        PcbLayer::UserComments => "Other,User",
    }
}

// ---------------------------------------------------------------------------
// Pad geometry on a specific layer
// ---------------------------------------------------------------------------

/// Resolved absolute position and rotation of a pad.
struct AbsolutePad {
    x: f64,
    y: f64,
    shape: ApertureShape,
}

fn resolve_pads_on_layer(footprint: &Footprint, layer: PcbLayer) -> Vec<AbsolutePad> {
    let cos_r = footprint.rotation.to_radians().cos();
    let sin_r = footprint.rotation.to_radians().sin();

    footprint
        .pads
        .iter()
        .filter(|pad| pad.layers.contains(&layer))
        .map(|pad| {
            // Rotate pad position by footprint rotation.
            let lx = pad.position.x * cos_r - pad.position.y * sin_r;
            let ly = pad.position.x * sin_r + pad.position.y * cos_r;
            let x = footprint.position.x + lx;
            let y = footprint.position.y + ly;

            let shape = match &pad.shape {
                PadShape::Circle { diameter } => ApertureShape::Circle {
                    diameter: *diameter,
                },
                PadShape::Rect { width, height } => ApertureShape::Rect {
                    width: *width,
                    height: *height,
                },
                PadShape::Oval { width, height } => ApertureShape::Oval {
                    width: *width,
                    height: *height,
                },
                PadShape::RoundRect { width, height, .. } => {
                    // Approximate round-rect as rectangle in Gerber output.
                    ApertureShape::Rect {
                        width: *width,
                        height: *height,
                    }
                }
                PadShape::Custom { .. } => {
                    // Custom pads are not directly representable as standard
                    // apertures; emit a 0.1 mm circle placeholder. Real
                    // fabrication would need region primitives (future work).
                    ApertureShape::Circle { diameter: 0.1 }
                }
            };

            AbsolutePad { x, y, shape }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Collect what layers actually have data
// ---------------------------------------------------------------------------

fn layers_with_data(pcb: &Pcb) -> Vec<PcbLayer> {
    let mut set = std::collections::HashSet::new();

    // Copper layers from traces.
    for trace in &pcb.traces {
        set.insert(trace.layer);
    }
    for arc in &pcb.trace_arcs {
        set.insert(arc.layer);
    }
    // Copper layers from zones.
    for zone in &pcb.zones {
        set.insert(zone.layer);
    }
    // Layers from pads (copper, mask, paste, silk).
    for fp in &pcb.footprints {
        for pad in &fp.pads {
            for &layer in &pad.layers {
                set.insert(layer);
            }
        }
        for gfx in &fp.graphics {
            match gfx {
                FootprintGraphic::Line { layer, .. }
                | FootprintGraphic::Circle { layer, .. }
                | FootprintGraphic::Arc { layer, .. }
                | FootprintGraphic::Rect { layer, .. }
                | FootprintGraphic::Polygon { layer, .. }
                | FootprintGraphic::Text { layer, .. } => {
                    set.insert(*layer);
                }
            }
        }
    }
    // Board outline always produces EdgeCuts.
    if !pcb.outline.vertices.is_empty() {
        set.insert(PcbLayer::EdgeCuts);
    }

    let mut layers: Vec<PcbLayer> = set.into_iter().collect();
    layers.sort_by_key(|l| format!("{l:?}"));
    layers
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Generate Gerber RS-274X content for a single layer.
///
/// Writes a complete Gerber file (header, aperture definitions, draw/flash
/// commands, and footer) to `writer`.
pub fn write_gerber_layer<W: Write>(
    writer: &mut W,
    pcb: &Pcb,
    layer: PcbLayer,
) -> Result<(), GerberError> {
    let mut apertures = ApertureTable::new();

    // -- Pre-scan: collect apertures & commands into a buffer so we can write
    //    definitions before commands. --

    // 1) Pads on this layer (flash commands).
    struct FlashCmd {
        x: i64,
        y: i64,
        dcode: u32,
    }
    let mut flashes: Vec<FlashCmd> = Vec::new();

    for fp in &pcb.footprints {
        for abs_pad in resolve_pads_on_layer(fp, layer) {
            let dcode = apertures.register(abs_pad.shape);
            flashes.push(FlashCmd {
                x: mm_to_coord(abs_pad.x),
                y: mm_to_coord(abs_pad.y),
                dcode,
            });
        }
    }

    // 2) Via pads on copper layers.
    if layer.is_copper() {
        for via in &pcb.vias {
            let dcode = apertures.register(ApertureShape::Circle {
                diameter: via.diameter,
            });
            flashes.push(FlashCmd {
                x: mm_to_coord(via.position.x),
                y: mm_to_coord(via.position.y),
                dcode,
            });
        }
    }

    // 3) Traces on this layer (draw commands).
    struct DrawCmd {
        x1: i64,
        y1: i64,
        x2: i64,
        y2: i64,
        dcode: u32,
    }
    let mut draws: Vec<DrawCmd> = Vec::new();

    for trace in &pcb.traces {
        if trace.layer == layer {
            let dcode = apertures.register(ApertureShape::Circle {
                diameter: trace.width,
            });
            draws.push(DrawCmd {
                x1: mm_to_coord(trace.start.x),
                y1: mm_to_coord(trace.start.y),
                x2: mm_to_coord(trace.end.x),
                y2: mm_to_coord(trace.end.y),
                dcode,
            });
        }
    }

    // 4) Zones on this layer (region fills).
    let zones_on_layer: Vec<&Zone> = pcb.zones.iter().filter(|z| z.layer == layer).collect();

    // 5) Board outline segments on EdgeCuts layer.
    let outline_segments: Vec<(i64, i64, i64, i64)> = if layer == PcbLayer::EdgeCuts {
        let verts = &pcb.outline.vertices;
        if verts.len() >= 2 {
            (0..verts.len())
                .map(|i| {
                    let j = (i + 1) % verts.len();
                    (
                        mm_to_coord(verts[i].x),
                        mm_to_coord(verts[i].y),
                        mm_to_coord(verts[j].x),
                        mm_to_coord(verts[j].y),
                    )
                })
                .collect()
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };

    // 6) Footprint graphics on this layer.
    struct GraphicLineCmd {
        x1: i64,
        y1: i64,
        x2: i64,
        y2: i64,
        dcode: u32,
    }
    let mut graphic_lines: Vec<GraphicLineCmd> = Vec::new();

    for fp in &pcb.footprints {
        let cos_r = fp.rotation.to_radians().cos();
        let sin_r = fp.rotation.to_radians().sin();

        for gfx in &fp.graphics {
            match gfx {
                FootprintGraphic::Line {
                    start,
                    end,
                    width,
                    layer: gfx_layer,
                } if *gfx_layer == layer => {
                    let dcode = apertures.register(ApertureShape::Circle { diameter: *width });
                    let sx = fp.position.x + start.x * cos_r - start.y * sin_r;
                    let sy = fp.position.y + start.x * sin_r + start.y * cos_r;
                    let ex = fp.position.x + end.x * cos_r - end.y * sin_r;
                    let ey = fp.position.y + end.x * sin_r + end.y * cos_r;
                    graphic_lines.push(GraphicLineCmd {
                        x1: mm_to_coord(sx),
                        y1: mm_to_coord(sy),
                        x2: mm_to_coord(ex),
                        y2: mm_to_coord(ey),
                        dcode,
                    });
                }
                FootprintGraphic::Rect {
                    start,
                    end,
                    width,
                    layer: gfx_layer,
                } if *gfx_layer == layer => {
                    let dcode = apertures.register(ApertureShape::Circle { diameter: *width });
                    // Emit 4 edges of the rectangle.
                    let corners = [
                        (start.x, start.y),
                        (end.x, start.y),
                        (end.x, end.y),
                        (start.x, end.y),
                    ];
                    for k in 0..4 {
                        let (ax, ay) = corners[k];
                        let (bx, by) = corners[(k + 1) % 4];
                        let sx = fp.position.x + ax * cos_r - ay * sin_r;
                        let sy = fp.position.y + ax * sin_r + ay * cos_r;
                        let ex = fp.position.x + bx * cos_r - by * sin_r;
                        let ey = fp.position.y + bx * sin_r + by * cos_r;
                        graphic_lines.push(GraphicLineCmd {
                            x1: mm_to_coord(sx),
                            y1: mm_to_coord(sy),
                            x2: mm_to_coord(ex),
                            y2: mm_to_coord(ey),
                            dcode,
                        });
                    }
                }
                _ => {}
            }
        }
    }

    // Register a thin aperture for board outline if needed.
    let outline_dcode = if !outline_segments.is_empty() {
        Some(apertures.register(ApertureShape::Circle { diameter: 0.05 }))
    } else {
        None
    };

    // -- Write the Gerber file --

    // Header.
    writeln!(writer, "G04 Generated by vcad-ecad-export*")?;
    writeln!(writer, "%FSLAX46Y46*%")?;
    writeln!(writer, "%MOMM*%")?;
    writeln!(writer, "%TF.FileFunction,{}*%", layer_file_function(layer))?;
    writeln!(writer, "%TF.GenerationSoftware,vcad,vcad-ecad-export*%")?;

    // Aperture definitions.
    apertures.write_definitions(writer)?;

    // Draw traces.
    let mut current_dcode: Option<u32> = None;
    for draw in &draws {
        if current_dcode != Some(draw.dcode) {
            writeln!(writer, "D{}*", draw.dcode)?;
            current_dcode = Some(draw.dcode);
        }
        writeln!(writer, "X{}Y{}D02*", fmt_coord(draw.x1), fmt_coord(draw.y1))?;
        writeln!(writer, "X{}Y{}D01*", fmt_coord(draw.x2), fmt_coord(draw.y2))?;
    }

    // Draw footprint graphics.
    for gl in &graphic_lines {
        if current_dcode != Some(gl.dcode) {
            writeln!(writer, "D{}*", gl.dcode)?;
            current_dcode = Some(gl.dcode);
        }
        writeln!(writer, "X{}Y{}D02*", fmt_coord(gl.x1), fmt_coord(gl.y1))?;
        writeln!(writer, "X{}Y{}D01*", fmt_coord(gl.x2), fmt_coord(gl.y2))?;
    }

    // Flash pads.
    for flash in &flashes {
        if current_dcode != Some(flash.dcode) {
            writeln!(writer, "D{}*", flash.dcode)?;
            current_dcode = Some(flash.dcode);
        }
        writeln!(writer, "X{}Y{}D03*", fmt_coord(flash.x), fmt_coord(flash.y))?;
    }

    // Region fills (zones).
    for zone in &zones_on_layer {
        if zone.outline.len() < 3 {
            continue;
        }
        writeln!(writer, "G36*")?;
        let first = &zone.outline[0];
        writeln!(
            writer,
            "X{}Y{}D02*",
            fmt_coord(mm_to_coord(first.x)),
            fmt_coord(mm_to_coord(first.y))
        )?;
        for pt in &zone.outline[1..] {
            writeln!(
                writer,
                "X{}Y{}D01*",
                fmt_coord(mm_to_coord(pt.x)),
                fmt_coord(mm_to_coord(pt.y))
            )?;
        }
        // Close the polygon.
        writeln!(
            writer,
            "X{}Y{}D01*",
            fmt_coord(mm_to_coord(first.x)),
            fmt_coord(mm_to_coord(first.y))
        )?;
        writeln!(writer, "G37*")?;
    }

    // Board outline (EdgeCuts).
    if let Some(dcode) = outline_dcode {
        if current_dcode != Some(dcode) {
            writeln!(writer, "D{}*", dcode)?;
        }
        for (x1, y1, x2, y2) in &outline_segments {
            writeln!(writer, "X{}Y{}D02*", fmt_coord(*x1), fmt_coord(*y1))?;
            writeln!(writer, "X{}Y{}D01*", fmt_coord(*x2), fmt_coord(*y2))?;
        }
    }

    // End of file.
    writeln!(writer, "M02*")?;

    Ok(())
}

/// Generate all Gerber files for a PCB.
///
/// Returns a map of `filename` to `content` where the filename follows the
/// convention `<board>-<Layer>.gbr`.
pub fn generate_gerbers(pcb: &Pcb) -> Result<HashMap<String, String>, GerberError> {
    let layers = layers_with_data(pcb);
    let mut result = HashMap::new();

    for layer in layers {
        let filename = format!("{}.gbr", layer_filename(layer));
        let mut buf = Vec::new();
        write_gerber_layer(&mut buf, pcb, layer)?;
        let content = String::from_utf8_lossy(&buf).into_owned();
        result.insert(filename, content);
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_ir::Vec2;

    /// Build a minimal PCB for testing.
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
                        dielectric_thickness: Some(0.2),
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
            nets: vec![
                Net {
                    id: "1".into(),
                    name: "VCC".into(),
                },
                Net {
                    id: "2".into(),
                    name: "GND".into(),
                },
            ],
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
            footprints: vec![Footprint {
                reference: "R1".into(),
                value: "10k".into(),
                footprint_name: "Resistor_SMD:R_0805".into(),
                position: Vec2::new(25.0, 20.0),
                rotation: 0.0,
                front: true,
                pads: vec![
                    Pad {
                        number: "1".into(),
                        pad_type: PadType::SMD,
                        shape: PadShape::Rect {
                            width: 1.0,
                            height: 1.2,
                        },
                        position: Vec2::new(-1.0, 0.0),
                        rotation: 0.0,
                        drill: None,
                        net: Some("1".into()),
                        layers: vec![PcbLayer::FCu, PcbLayer::FPaste, PcbLayer::FMask],
                    },
                    Pad {
                        number: "2".into(),
                        pad_type: PadType::SMD,
                        shape: PadShape::Rect {
                            width: 1.0,
                            height: 1.2,
                        },
                        position: Vec2::new(1.0, 0.0),
                        rotation: 0.0,
                        drill: None,
                        net: Some("2".into()),
                        layers: vec![PcbLayer::FCu, PcbLayer::FPaste, PcbLayer::FMask],
                    },
                ],
                graphics: vec![],
                model_3d: None,
                properties: Default::default(),
            }],
            traces: vec![Trace {
                start: Vec2::new(24.0, 20.0),
                end: Vec2::new(10.0, 20.0),
                width: 0.25,
                layer: PcbLayer::FCu,
                net: "1".into(),
            }],
            trace_arcs: vec![],
            vias: vec![Via {
                position: Vec2::new(10.0, 20.0),
                diameter: 0.8,
                drill: 0.4,
                start_layer: PcbLayer::FCu,
                end_layer: PcbLayer::BCu,
                net: "1".into(),
            }],
            zones: vec![Zone {
                outline: vec![
                    Vec2::new(0.0, 0.0),
                    Vec2::new(50.0, 0.0),
                    Vec2::new(50.0, 40.0),
                    Vec2::new(0.0, 40.0),
                ],
                holes: vec![],
                net: "2".into(),
                layer: PcbLayer::BCu,
                clearance: 0.3,
                min_area: 0.0,
                fill_type: ZoneFillType::Solid,
                thermal_relief: ThermalReliefStyle::Relief,
                thermal_gap: Some(0.5),
                thermal_spoke_width: Some(0.5),
                priority: 0,
            }],
            keepouts: vec![],
        }
    }

    #[test]
    fn gerber_header_format() {
        let pcb = test_pcb();
        let mut buf = Vec::new();
        write_gerber_layer(&mut buf, &pcb, PcbLayer::FCu).unwrap();
        let output = String::from_utf8(buf).unwrap();

        assert!(output.contains("%FSLAX46Y46*%"), "missing format spec");
        assert!(output.contains("%MOMM*%"), "missing unit spec");
        assert!(output.contains("M02*"), "missing end-of-file");
        assert!(
            output.contains("%TF.FileFunction,Copper,L1,Top*%"),
            "missing file function attribute"
        );
    }

    #[test]
    fn gerber_aperture_definitions() {
        let pcb = test_pcb();
        let mut buf = Vec::new();
        write_gerber_layer(&mut buf, &pcb, PcbLayer::FCu).unwrap();
        let output = String::from_utf8(buf).unwrap();

        // Should have at least one %ADD aperture definition.
        assert!(output.contains("%ADD10"), "missing aperture definition");
    }

    #[test]
    fn gerber_flash_commands() {
        let pcb = test_pcb();
        let mut buf = Vec::new();
        write_gerber_layer(&mut buf, &pcb, PcbLayer::FCu).unwrap();
        let output = String::from_utf8(buf).unwrap();

        // Pads should produce D03 flash commands.
        assert!(output.contains("D03*"), "missing flash command");
    }

    #[test]
    fn gerber_draw_commands() {
        let pcb = test_pcb();
        let mut buf = Vec::new();
        write_gerber_layer(&mut buf, &pcb, PcbLayer::FCu).unwrap();
        let output = String::from_utf8(buf).unwrap();

        // Traces should produce D01 draw and D02 move commands.
        assert!(output.contains("D01*"), "missing draw command");
        assert!(output.contains("D02*"), "missing move command");
    }

    #[test]
    fn gerber_region_fill() {
        let pcb = test_pcb();
        let mut buf = Vec::new();
        write_gerber_layer(&mut buf, &pcb, PcbLayer::BCu).unwrap();
        let output = String::from_utf8(buf).unwrap();

        assert!(output.contains("G36*"), "missing region start");
        assert!(output.contains("G37*"), "missing region end");
    }

    #[test]
    fn gerber_edge_cuts() {
        let pcb = test_pcb();
        let mut buf = Vec::new();
        write_gerber_layer(&mut buf, &pcb, PcbLayer::EdgeCuts).unwrap();
        let output = String::from_utf8(buf).unwrap();

        assert!(
            output.contains("%TF.FileFunction,Profile,NP*%"),
            "missing EdgeCuts file function"
        );
        // 4 edges of the rectangle.
        let d01_count = output.matches("D01*").count();
        assert_eq!(d01_count, 4, "expected 4 outline edges, got {d01_count}");
    }

    #[test]
    fn gerber_coordinate_format() {
        // 25.0 mm should become 25000000.
        assert_eq!(mm_to_coord(25.0), 25_000_000);
        // 0.25 mm should become 250000.
        assert_eq!(mm_to_coord(0.25), 250_000);
    }

    #[test]
    fn generate_all_gerbers() {
        let pcb = test_pcb();
        let files = generate_gerbers(&pcb).unwrap();

        // Should have at least FCu, BCu, EdgeCuts, FPaste, FMask.
        assert!(files.contains_key("F_Cu.gbr"), "missing F_Cu.gbr");
        assert!(files.contains_key("B_Cu.gbr"), "missing B_Cu.gbr");
        assert!(files.contains_key("Edge_Cuts.gbr"), "missing Edge_Cuts.gbr");
        assert!(files.contains_key("F_Paste.gbr"), "missing F_Paste.gbr");
        assert!(files.contains_key("F_Mask.gbr"), "missing F_Mask.gbr");

        // Each file should be a valid gerber with header and footer.
        for (name, content) in &files {
            assert!(
                content.contains("%FSLAX46Y46*%"),
                "{name}: missing format spec"
            );
            assert!(content.contains("M02*"), "{name}: missing end-of-file");
        }
    }
}
