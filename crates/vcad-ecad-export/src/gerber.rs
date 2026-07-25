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

/// Angles within this many degrees of a quadrant are treated as exactly on it.
///
/// Pad angles arrive from KiCad as decimal degrees (`90`, `-90`, `45`), so the
/// only error to absorb is float formatting, not real off-axis intent.
const ANGLE_EPS_DEG: f64 = 1e-6;

/// Aperture shape used in the aperture definition table.
///
/// `Rect`/`Oval` are the axis-aligned standard apertures; `RotRect`/`RotOval`
/// carry an off-axis rotation and are emitted as aperture macros (`%AM`). A
/// rotation that is a multiple of 90° never reaches the macro forms — see
/// [`ApertureShape::rect`] and [`ApertureShape::oval`], which fold it into a
/// width/height swap, which is exact and keeps the aperture table small.
#[derive(Debug, Clone, PartialEq)]
enum ApertureShape {
    Circle {
        diameter: f64,
    },
    Rect {
        width: f64,
        height: f64,
    },
    Oval {
        width: f64,
        height: f64,
    },
    /// Rectangle rotated `angle` degrees CCW about its centre.
    RotRect {
        width: f64,
        height: f64,
        angle: f64,
    },
    /// Obround rotated `angle` degrees CCW about its centre. `length` is the
    /// long extent (along the unrotated X axis), `girth` the short one.
    RotOval {
        length: f64,
        girth: f64,
        angle: f64,
    },
}

/// Unique key for de-duplicating apertures.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ApertureKey {
    /// Discriminant tag.
    kind: &'static str,
    /// Dimensions encoded as integer nanometres for exact hashing, plus the
    /// rotation in micro-degrees. Two pads that differ only in rotation must
    /// land on different keys, or they collapse into one wrongly-turned
    /// aperture.
    dims: [i64; 3],
}

/// Encode an angle in micro-degrees for exact hashing.
fn angle_key(deg: f64) -> i64 {
    (deg * 1_000_000.0).round() as i64
}

/// Reduce an angle to `[0, period)`.
fn norm_angle(deg: f64, period: f64) -> f64 {
    let a = deg % period;
    let a = if a < 0.0 { a + period } else { a };
    // Snap a hair below the period back to zero (e.g. 179.9999999 -> 0).
    if (a - period).abs() < ANGLE_EPS_DEG {
        0.0
    } else {
        a
    }
}

impl ApertureShape {
    /// Build a rectangular aperture rotated `angle` degrees CCW.
    ///
    /// A rectangle is 180°-symmetric, so the angle reduces mod 180; 0° and 90°
    /// then collapse to a plain `R` aperture (with the sides swapped at 90°).
    fn rect(width: f64, height: f64, angle: f64) -> Self {
        let a = norm_angle(angle, 180.0);
        if a < ANGLE_EPS_DEG {
            ApertureShape::Rect { width, height }
        } else if (a - 90.0).abs() < ANGLE_EPS_DEG {
            ApertureShape::Rect {
                width: height,
                height: width,
            }
        } else {
            ApertureShape::RotRect {
                width,
                height,
                angle: a,
            }
        }
    }

    /// Build an obround aperture rotated `angle` degrees CCW.
    ///
    /// Normalised so the long axis is X at angle 0; a square obround is a
    /// circle and rotation-invariant.
    fn oval(width: f64, height: f64, angle: f64) -> Self {
        // Put the long extent on X, folding a vertical obround into a
        // horizontal one turned 90°.
        let (length, girth, angle) = if width >= height {
            (width, height, angle)
        } else {
            (height, width, angle + 90.0)
        };
        if (length - girth).abs() < 1e-9 {
            return ApertureShape::Circle { diameter: girth };
        }
        let a = norm_angle(angle, 180.0);
        if a < ANGLE_EPS_DEG {
            ApertureShape::Oval {
                width: length,
                height: girth,
            }
        } else if (a - 90.0).abs() < ANGLE_EPS_DEG {
            ApertureShape::Oval {
                width: girth,
                height: length,
            }
        } else {
            ApertureShape::RotOval {
                length,
                girth,
                angle: a,
            }
        }
    }

    fn key(&self) -> ApertureKey {
        match self {
            ApertureShape::Circle { diameter } => ApertureKey {
                kind: "C",
                dims: [mm_to_coord(*diameter), 0, 0],
            },
            ApertureShape::Rect { width, height } => ApertureKey {
                kind: "R",
                dims: [mm_to_coord(*width), mm_to_coord(*height), 0],
            },
            ApertureShape::Oval { width, height } => ApertureKey {
                kind: "O",
                dims: [mm_to_coord(*width), mm_to_coord(*height), 0],
            },
            ApertureShape::RotRect {
                width,
                height,
                angle,
            } => ApertureKey {
                kind: "RR",
                dims: [mm_to_coord(*width), mm_to_coord(*height), angle_key(*angle)],
            },
            ApertureShape::RotOval {
                length,
                girth,
                angle,
            } => ApertureKey {
                kind: "RO",
                dims: [mm_to_coord(*length), mm_to_coord(*girth), angle_key(*angle)],
            },
        }
    }

    /// Name of the aperture macro backing this shape, if it needs one.
    fn macro_name(&self, code: u32) -> Option<String> {
        match self {
            ApertureShape::RotRect { .. } => Some(format!("VCADRR{code}")),
            ApertureShape::RotOval { .. } => Some(format!("VCADRO{code}")),
            _ => None,
        }
    }

    /// The `%AM...*%` macro body for this shape, if it needs one.
    ///
    /// Follows what KiCad emits for off-axis pads: primitive 21 (centre line —
    /// a rectangle with a rotation parameter) for the body, and primitive 1
    /// (circle) for an obround's end caps. One macro per distinct
    /// (shape, angle) with literal numbers — no macro parameters, so there is
    /// nothing for a downstream CAM tool to get wrong. Cap centres are
    /// pre-rotated here rather than passed through the circle primitive's
    /// rotation parameter, which older CAM tools do not accept.
    fn macro_definition(&self, code: u32) -> Option<String> {
        let name = self.macro_name(code)?;
        match self {
            ApertureShape::RotRect {
                width,
                height,
                angle,
            } => Some(format!(
                "%AM{name}*\n21,1,{width:.6},{height:.6},0,0,{angle:.6}*%"
            )),
            ApertureShape::RotOval {
                length,
                girth,
                angle,
            } => {
                // Body: a (length - girth) x girth bar, plus a circle of
                // diameter `girth` at each end, all turned by `angle`.
                let bar = length - girth;
                let off = bar / 2.0;
                let (s, c) = angle.to_radians().sin_cos();
                let (cx, cy) = (off * c, off * s);
                Some(format!(
                    "%AM{name}*\n\
                     21,1,{bar:.6},{girth:.6},0,0,{angle:.6}*\n\
                     1,1,{girth:.6},{cx:.6},{cy:.6}*\n\
                     1,1,{girth:.6},{nx:.6},{ny:.6}*%",
                    nx = -cx,
                    ny = -cy
                ))
            }
            _ => None,
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
            ApertureShape::RotRect { .. } | ApertureShape::RotOval { .. } => {
                let name = self.macro_name(code).expect("rotated shape has a macro");
                format!("%ADD{code}{name}*%")
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

    /// Write all aperture macros and `%ADD...` definitions.
    ///
    /// A macro must be defined before the `%ADD` that instantiates it, so each
    /// shape emits its `%AM` immediately ahead of its own `%ADD`.
    fn write_definitions<W: Write>(&self, w: &mut W) -> Result<(), std::io::Error> {
        for (i, shape) in self.shapes.iter().enumerate() {
            let code = 10 + i as u32;
            if let Some(mac) = shape.macro_definition(code) {
                writeln!(w, "{mac}")?;
            }
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
        PcbLayer::In7Cu => "In7_Cu",
        PcbLayer::In8Cu => "In8_Cu",
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
        PcbLayer::In7Cu => "Copper,L8,Inr",
        PcbLayer::In8Cu => "Copper,L9,Inr",
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

            // The aperture turns with the footprint AND with the pad's own
            // angle. vcad's IR stores the pad angle RELATIVE to its
            // footprint, so the effective board-frame angle is the sum.
            // Without this, every non-square pad on a rotated footprint was
            // flashed axis-aligned — a 0.25 x 0.875mm QFN pad on a 90°-turned
            // part came out 0.25mm wide, overlapping its neighbours.
            let angle = footprint.rotation + pad.rotation;

            let shape = match &pad.shape {
                // A circle is rotation-invariant.
                PadShape::Circle { diameter } => ApertureShape::Circle {
                    diameter: *diameter,
                },
                PadShape::Rect { width, height } => ApertureShape::rect(*width, *height, angle),
                PadShape::Oval { width, height } => ApertureShape::oval(*width, *height, angle),
                PadShape::RoundRect { width, height, .. } => {
                    // Approximate round-rect as rectangle in Gerber output.
                    ApertureShape::rect(*width, *height, angle)
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

    // 4) Zones on this layer are poured (clearance knockouts + thermal relief)
    //    by the copper-pour engine and emitted as region fills further below.

    // 5) Board outline segments on EdgeCuts layer.
    //    Emit the outer profile followed by any interior cutouts (center bores,
    //    keyed/D-shaped shaft holes, slots). Cutouts are routed on the profile
    //    layer so the fabricated board actually has the holes — dropping them
    //    silently ships a board missing its bore.
    let outline_segments: Vec<(i64, i64, i64, i64)> = if layer == PcbLayer::EdgeCuts {
        let mut segs: Vec<(i64, i64, i64, i64)> = Vec::new();
        let loops = std::iter::once(&pcb.outline.vertices).chain(pcb.outline.cutouts.iter());
        for verts in loops {
            if verts.len() < 2 {
                continue;
            }
            for i in 0..verts.len() {
                let j = (i + 1) % verts.len();
                segs.push((
                    mm_to_coord(verts[i].x),
                    mm_to_coord(verts[i].y),
                    mm_to_coord(verts[j].x),
                    mm_to_coord(verts[j].y),
                ));
            }
        }
        segs
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
                FootprintGraphic::Circle {
                    center,
                    radius,
                    width,
                    layer: gfx_layer,
                } if *gfx_layer == layer => {
                    let dcode = apertures.register(ApertureShape::Circle { diameter: *width });
                    // Tessellate the ring into ~48 short stroked segments.
                    const SEGMENTS: usize = 48;
                    let mut local: Vec<(f64, f64)> = Vec::with_capacity(SEGMENTS + 1);
                    for i in 0..=SEGMENTS {
                        let theta = std::f64::consts::TAU * (i as f64) / (SEGMENTS as f64);
                        local.push((
                            center.x + radius * theta.cos(),
                            center.y + radius * theta.sin(),
                        ));
                    }
                    for pair in local.windows(2) {
                        let (ax, ay) = pair[0];
                        let (bx, by) = pair[1];
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
                FootprintGraphic::Arc {
                    center,
                    radius,
                    start_angle,
                    end_angle,
                    width,
                    layer: gfx_layer,
                } if *gfx_layer == layer => {
                    let dcode = apertures.register(ApertureShape::Circle { diameter: *width });
                    // Tessellate the arc into segments proportional to its sweep
                    // (matching the ~48-per-full-turn density of the circle case).
                    let a0 = start_angle.to_radians();
                    let a1 = end_angle.to_radians();
                    let sweep = (a1 - a0).abs();
                    let segments =
                        ((sweep / std::f64::consts::TAU) * 48.0).ceil().max(1.0) as usize;
                    let mut local: Vec<(f64, f64)> = Vec::with_capacity(segments + 1);
                    for i in 0..=segments {
                        let t = (i as f64) / (segments as f64);
                        let theta = a0 + (a1 - a0) * t;
                        local.push((
                            center.x + radius * theta.cos(),
                            center.y + radius * theta.sin(),
                        ));
                    }
                    for pair in local.windows(2) {
                        let (ax, ay) = pair[0];
                        let (bx, by) = pair[1];
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
                FootprintGraphic::Polygon {
                    vertices,
                    width,
                    layer: gfx_layer,
                } if *gfx_layer == layer && vertices.len() >= 2 => {
                    let dcode = apertures.register(ApertureShape::Circle { diameter: *width });
                    // Connect consecutive points, closing the loop.
                    for i in 0..vertices.len() {
                        let a = vertices[i];
                        let b = vertices[(i + 1) % vertices.len()];
                        let sx = fp.position.x + a.x * cos_r - a.y * sin_r;
                        let sy = fp.position.y + a.x * sin_r + a.y * cos_r;
                        let ex = fp.position.x + b.x * cos_r - b.y * sin_r;
                        let ey = fp.position.y + b.x * sin_r + b.y * cos_r;
                        graphic_lines.push(GraphicLineCmd {
                            x1: mm_to_coord(sx),
                            y1: mm_to_coord(sy),
                            x2: mm_to_coord(ex),
                            y2: mm_to_coord(ey),
                            dcode,
                        });
                    }
                }
                FootprintGraphic::Text {
                    text,
                    position,
                    rotation,
                    height,
                    width,
                    layer: gfx_layer,
                } if *gfx_layer == layer => {
                    // Stroke width: fall back to a fraction of the cap height
                    // when unset/non-positive so the legend is still visible.
                    let stroke = if *width > 0.0 { *width } else { *height * 0.12 };
                    let dcode = apertures.register(ApertureShape::Circle { diameter: stroke });
                    // Local polylines from the shared stroke font (y up,
                    // baseline at y=0, origin at the text's local origin).
                    let strokes = vcad_ir::stroke_font::text_strokes(text, *height);
                    // Apply the text's own position + rotation first, then the
                    // footprint transform.
                    let tcos = rotation.to_radians().cos();
                    let tsin = rotation.to_radians().sin();
                    for polyline in &strokes {
                        for pair in polyline.windows(2) {
                            let (lax, lay) = pair[0];
                            let (lbx, lby) = pair[1];
                            // Text-local → footprint-local (rotate by text
                            // rotation, translate by text position).
                            let pax = position.x + lax * tcos - lay * tsin;
                            let pay = position.y + lax * tsin + lay * tcos;
                            let pbx = position.x + lbx * tcos - lby * tsin;
                            let pby = position.y + lbx * tsin + lby * tcos;
                            // Footprint-local → board.
                            let sx = fp.position.x + pax * cos_r - pay * sin_r;
                            let sy = fp.position.y + pax * sin_r + pay * cos_r;
                            let ex = fp.position.x + pbx * cos_r - pby * sin_r;
                            let ey = fp.position.y + pbx * sin_r + pby * cos_r;
                            graphic_lines.push(GraphicLineCmd {
                                x1: mm_to_coord(sx),
                                y1: mm_to_coord(sy),
                                x2: mm_to_coord(ex),
                                y2: mm_to_coord(ey),
                                dcode,
                            });
                        }
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

    // Region fills (zones). Each zone is poured — its outline minus exact
    // clearance voids around other-net copper, with thermal-relief spokes on
    // same-net pads — by the copper-pour engine, so the fabricated plane has
    // the gaps that keep it from shorting to every net it crosses. Each filled
    // ring (CCW copper outer, CW clearance hole) is one contour of a single
    // G36/G37 region; nested contours cut holes per the Gerber region rule.
    for filled in vcad_ecad_pcb::copper_pour::fill_zones(pcb)
        .iter()
        .filter(|f| f.layer == layer)
    {
        let rings: Vec<&Vec<vcad_ir::Vec2>> =
            filled.polygons.iter().filter(|r| r.len() >= 3).collect();
        if rings.is_empty() {
            continue;
        }
        writeln!(writer, "G36*")?;
        for ring in rings {
            let first = &ring[0];
            writeln!(
                writer,
                "X{}Y{}D02*",
                fmt_coord(mm_to_coord(first.x)),
                fmt_coord(mm_to_coord(first.y))
            )?;
            for pt in &ring[1..] {
                writeln!(
                    writer,
                    "X{}Y{}D01*",
                    fmt_coord(mm_to_coord(pt.x)),
                    fmt_coord(mm_to_coord(pt.y))
                )?;
            }
            // Close the contour.
            writeln!(
                writer,
                "X{}Y{}D01*",
                fmt_coord(mm_to_coord(first.x)),
                fmt_coord(mm_to_coord(first.y))
            )?;
        }
        writeln!(writer, "G37*")?;
    }

    // Teardrop fillets at trace→pad/via junctions on this layer, as region fills.
    for td in vcad_ecad_pcb::generate_teardrops(pcb)
        .iter()
        .filter(|t| t.layer == layer)
    {
        if td.polygon.len() < 3 {
            continue;
        }
        writeln!(writer, "G36*")?;
        let first = &td.polygon[0];
        writeln!(
            writer,
            "X{}Y{}D02*",
            fmt_coord(mm_to_coord(first.x)),
            fmt_coord(mm_to_coord(first.y))
        )?;
        for pt in &td.polygon[1..] {
            writeln!(
                writer,
                "X{}Y{}D01*",
                fmt_coord(mm_to_coord(pt.x)),
                fmt_coord(mm_to_coord(pt.y))
            )?;
        }
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
                source: None,
            }],
            trace_arcs: vec![],
            vias: vec![Via {
                position: Vec2::new(10.0, 20.0),
                diameter: 0.8,
                drill: 0.4,
                start_layer: PcbLayer::FCu,
                end_layer: PcbLayer::BCu,
                net: "1".into(),
                source: None,
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
            net_ties: vec![],
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

    /// A PCB carrying one rectangular pad, on a footprint turned `fp_rot`
    /// with the pad itself turned `pad_rot` relative to it.
    fn rotated_pad_pcb(fp_rot: f64, pad_rot: f64, shape: PadShape) -> Pcb {
        let mut pcb = test_pcb();
        let fp = &mut pcb.footprints[0];
        fp.rotation = fp_rot;
        fp.pads.truncate(1);
        fp.pads[0].shape = shape;
        fp.pads[0].rotation = pad_rot;
        pcb
    }

    /// The long axis of a rectangular pad must follow the footprint round.
    ///
    /// A 0.25 x 0.875mm QFN pad on a 90°-rotated part is 0.875mm wide on the
    /// board, not 0.25mm. Before apertures carried a rotation, every such pad
    /// was flashed axis-aligned and overlapped its neighbours.
    #[test]
    fn rect_pad_on_rotated_footprint_swaps_axes() {
        let shape = PadShape::Rect {
            width: 0.25,
            height: 0.875,
        };

        let mut buf = Vec::new();
        write_gerber_layer(
            &mut buf,
            &rotated_pad_pcb(0.0, 0.0, shape.clone()),
            PcbLayer::FCu,
        )
        .unwrap();
        let unrotated = String::from_utf8(buf).unwrap();
        assert!(
            unrotated.contains("R,0.250000X0.875000"),
            "unrotated pad should stay 0.25 wide:\n{unrotated}"
        );

        for (fp_rot, pad_rot) in [(90.0, 0.0), (0.0, 90.0), (45.0, 45.0), (-90.0, 0.0)] {
            let mut buf = Vec::new();
            write_gerber_layer(
                &mut buf,
                &rotated_pad_pcb(fp_rot, pad_rot, shape.clone()),
                PcbLayer::FCu,
            )
            .unwrap();
            let out = String::from_utf8(buf).unwrap();
            assert!(
                out.contains("R,0.875000X0.250000"),
                "fp {fp_rot}° + pad {pad_rot}° should put the long axis on X:\n{out}"
            );
        }
    }

    /// An off-quadrant angle needs an aperture macro, not an axis-aligned box.
    #[test]
    fn rect_pad_at_45_degrees_emits_rotated_macro() {
        let pcb = rotated_pad_pcb(
            45.0,
            0.0,
            PadShape::Rect {
                width: 0.25,
                height: 0.875,
            },
        );
        let mut buf = Vec::new();
        write_gerber_layer(&mut buf, &pcb, PcbLayer::FCu).unwrap();
        let out = String::from_utf8(buf).unwrap();

        // Primitive 21 (centre line) carries the rotation; the macro must be
        // defined before the %ADD that instantiates it.
        let mac = out
            .find("%AMVCADRR")
            .unwrap_or_else(|| panic!("missing rotated-rect macro:\n{out}"));
        assert!(
            out.contains("21,1,0.250000,0.875000,0,0,45.000000*%"),
            "macro body should be a 45°-turned centre line:\n{out}"
        );
        let name_start = mac + "%AM".len();
        let name: String = out[name_start..]
            .chars()
            .take_while(|c| *c != '*')
            .collect();
        let add = out
            .find(&format!("{name}*%"))
            .filter(|&i| i > mac)
            .unwrap_or_else(|| panic!("macro {name} never instantiated by an %ADD:\n{out}"));
        assert!(add > mac, "%ADD must follow its %AM");
        assert!(
            !out.contains("%ADD10R,"),
            "45° pad must not degrade to an axis-aligned rect:\n{out}"
        );
    }

    /// Two pads differing only in rotation must not share an aperture.
    #[test]
    fn aperture_dedup_separates_rotations() {
        let mut table = ApertureTable::new();
        let a = table.register(ApertureShape::rect(0.25, 0.875, 0.0));
        let b = table.register(ApertureShape::rect(0.25, 0.875, 90.0));
        let c = table.register(ApertureShape::rect(0.25, 0.875, 45.0));
        let d = table.register(ApertureShape::rect(0.25, 0.875, 135.0));
        assert_ne!(a, b, "0° and 90° are different apertures");
        assert_ne!(c, d, "45° and 135° are different apertures");
        assert_ne!(a, c);

        // ...but the same rotation, however it is spelled, still dedups.
        assert_eq!(a, table.register(ApertureShape::rect(0.25, 0.875, 180.0)));
        assert_eq!(b, table.register(ApertureShape::rect(0.25, 0.875, -90.0)));
        assert_eq!(c, table.register(ApertureShape::rect(0.25, 0.875, 225.0)));
    }

    /// A rotated obround keeps its end caps on the long axis.
    #[test]
    fn oval_pad_rotation() {
        // 90° folds into a plain O aperture with the sides swapped.
        assert_eq!(
            ApertureShape::oval(2.0, 1.0, 90.0),
            ApertureShape::Oval {
                width: 1.0,
                height: 2.0
            }
        );
        // A vertical obround is a horizontal one turned 90°, so -90 (i.e. 90)
        // lands back on the horizontal form.
        assert_eq!(
            ApertureShape::oval(1.0, 2.0, 90.0),
            ApertureShape::Oval {
                width: 2.0,
                height: 1.0
            }
        );
        // Square obround == circle, rotation-invariant.
        assert_eq!(
            ApertureShape::oval(1.0, 1.0, 37.0),
            ApertureShape::Circle { diameter: 1.0 }
        );

        // 45°: bar of (2.0 - 1.0) x 1.0 turned 45°, caps at ±(0.5cos45,
        // 0.5sin45) = ±(0.353553, 0.353553).
        let shape = ApertureShape::oval(2.0, 1.0, 45.0);
        let body = shape
            .macro_definition(11)
            .expect("rotated oval needs a macro");
        assert!(
            body.contains("21,1,1.000000,1.000000,0,0,45.000000*"),
            "bar primitive wrong:\n{body}"
        );
        assert!(
            body.contains("1,1,1.000000,0.353553,0.353553*"),
            "cap centre should be pre-rotated onto the long axis:\n{body}"
        );
        assert!(
            body.contains("1,1,1.000000,-0.353553,-0.353553*"),
            "opposite cap missing:\n{body}"
        );
    }

    /// RoundRect still degrades to a rect — but a rotated one.
    #[test]
    fn roundrect_pad_rotates() {
        let shape = PadShape::RoundRect {
            width: 0.25,
            height: 0.875,
            corner_ratio: 0.25,
        };
        let mut buf = Vec::new();
        write_gerber_layer(&mut buf, &rotated_pad_pcb(90.0, 0.0, shape), PcbLayer::FCu).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(
            out.contains("R,0.875000X0.250000"),
            "rotated roundrect should swap axes:\n{out}"
        );
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

        // The BCu zone (net "2") is crossed by an other-net via (net "1"), so
        // the poured region must contain a clearance-void contour in addition
        // to its outline — a solid flood would emit a single contour and short
        // the plane to the via. Count D02 (contour-start) moves inside G36/G37.
        let start = output.find("G36*").unwrap();
        let end = output[start..].find("G37*").unwrap() + start;
        let contours = output[start..end].matches("D02*").count();
        assert!(
            contours >= 2,
            "poured region must knock out the other-net via, found {contours} contour(s)"
        );
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
    fn gerber_edge_cuts_includes_cutouts() {
        let mut pcb = test_pcb();
        // A square cutout standing in for a center bore / keyed shaft hole.
        pcb.outline.cutouts = vec![vec![
            Vec2::new(20.0, 15.0),
            Vec2::new(30.0, 15.0),
            Vec2::new(30.0, 25.0),
            Vec2::new(20.0, 25.0),
        ]];
        let mut buf = Vec::new();
        write_gerber_layer(&mut buf, &pcb, PcbLayer::EdgeCuts).unwrap();
        let output = String::from_utf8(buf).unwrap();

        // 4 outer edges + 4 cutout edges = 8 draw commands.
        let d01_count = output.matches("D01*").count();
        assert_eq!(
            d01_count, 8,
            "expected 4 outer + 4 cutout edges, got {d01_count}"
        );
        // A cutout-only coordinate must appear in the routed profile.
        assert!(
            output.contains(&format!("X{}", mm_to_coord(20.0))),
            "cutout coordinate missing from Edge_Cuts"
        );
    }

    #[test]
    fn gerber_footprint_graphics_circle_arc_polygon_text() {
        // A footprint placed at the origin with no rotation so coordinates are
        // easy to reason about, carrying one of each non-Line/Rect graphic on
        // the front silkscreen.
        let mut pcb = test_pcb();
        let text_height = 1.0;
        let text_pos = Vec2::new(2.0, 3.0);
        pcb.footprints.push(Footprint {
            reference: "U1".into(),
            value: "MCU".into(),
            footprint_name: "Package_QFP:LQFP-48".into(),
            position: Vec2::new(0.0, 0.0),
            rotation: 0.0,
            front: true,
            pads: vec![],
            graphics: vec![
                FootprintGraphic::Circle {
                    center: Vec2::new(0.0, 0.0),
                    radius: 2.0,
                    width: 0.15,
                    layer: PcbLayer::FSilkS,
                },
                FootprintGraphic::Arc {
                    center: Vec2::new(0.0, 0.0),
                    radius: 2.0,
                    start_angle: 0.0,
                    end_angle: 90.0,
                    width: 0.15,
                    layer: PcbLayer::FSilkS,
                },
                FootprintGraphic::Polygon {
                    vertices: vec![
                        Vec2::new(-1.0, -1.0),
                        Vec2::new(1.0, -1.0),
                        Vec2::new(0.0, 1.0),
                    ],
                    width: 0.15,
                    layer: PcbLayer::FSilkS,
                },
                FootprintGraphic::Text {
                    text: "L".into(),
                    position: text_pos,
                    rotation: 0.0,
                    height: text_height,
                    width: 0.12,
                    layer: PcbLayer::FSilkS,
                },
            ],
            model_3d: None,
            properties: Default::default(),
        });

        let mut buf = Vec::new();
        write_gerber_layer(&mut buf, &pcb, PcbLayer::FSilkS).unwrap();
        let output = String::from_utf8(buf).unwrap();

        // It must be the front legend layer.
        assert!(
            output.contains("%TF.FileFunction,Legend,Top*%"),
            "missing FSilkS file function"
        );
        // All four graphics stroke out as D01 draw commands.
        assert!(
            output.contains("D01*"),
            "no draw commands emitted on FSilkS"
        );

        // Circle: 48 segments → 48 draw commands. Arc (0..90°): ceil(48/4)=12.
        // Polygon: 3 closing edges. Plus the text strokes.
        // We don't pin the exact total, but it must clearly exceed the
        // circle+arc+polygon minimum, proving text was also rendered.
        let circle_arc_poly = 48 + 12 + 3;
        let d01_count = output.matches("D01*").count();
        assert!(
            d01_count > circle_arc_poly,
            "expected circle+arc+polygon ({circle_arc_poly}) plus text strokes, got {d01_count}"
        );

        // A known text coordinate must appear. The writer applies the text's
        // own position then the (identity) footprint transform; with the
        // footprint at the origin and no rotation, the first stroke point of
        // the laid-out glyph lands at text_pos + local. Derive it from the
        // public stroke-font API so the test tracks the font, not its bytes.
        let strokes = vcad_ir::stroke_font::text_strokes("L", text_height);
        let (lx, ly) = strokes[0][0];
        let expected_x = mm_to_coord(text_pos.x + lx);
        let expected_y = mm_to_coord(text_pos.y + ly);
        assert!(
            output.contains(&format!(
                "X{}Y{}",
                fmt_coord(expected_x),
                fmt_coord(expected_y)
            )),
            "expected text coordinate X{}Y{} missing from FSilkS output",
            fmt_coord(expected_x),
            fmt_coord(expected_y)
        );
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
