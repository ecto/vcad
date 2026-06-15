//! ECAD (Electronic CAD) types for PCB design within vcad.
//!
//! This module defines all data types needed for schematic capture, PCB layout,
//! and fabrication export. Types follow the same serde-tagged pattern as the
//! rest of vcad-ir.

use serde::{Deserialize, Serialize};

use crate::Vec2;

// ============================================================================
// Common types
// ============================================================================

/// Unique identifier for a net (electrical connection).
pub type NetId = String;

/// A named electrical connection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Net {
    /// Unique net identifier.
    pub id: NetId,
    /// Human-readable net name (e.g. "VCC", "GND", "D0").
    pub name: String,
}

// ============================================================================
// Schematic types
// ============================================================================

/// Pin electrical type for ERC validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PinType {
    /// Input signal pin.
    Input,
    /// Output signal pin.
    Output,
    /// Bidirectional pin (e.g. data bus).
    Bidirectional,
    /// Tri-state output pin.
    TriState,
    /// Passive component pin (resistors, capacitors, etc.).
    Passive,
    /// Power input pin (e.g. VCC on an IC).
    PowerInput,
    /// Power output pin (e.g. voltage regulator output).
    PowerOutput,
    /// Open collector/drain output.
    OpenCollector,
    /// Open emitter/source output.
    OpenEmitter,
    /// Unconnected/no-connect pin.
    NotConnected,
    /// Free/unspecified pin type.
    Free,
}

/// Label scope for schematic net labels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LabelScope {
    /// Local to this sheet only.
    Local,
    /// Global across all sheets.
    Global,
    /// Hierarchical — visible to parent/child sheets.
    Hierarchical,
}

/// A net label on a schematic.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SchematicLabel {
    /// Net name this label assigns.
    pub name: String,
    /// Position on the sheet.
    pub position: Vec2,
    /// Rotation in degrees.
    #[serde(default)]
    pub rotation: f64,
    /// Label scope.
    pub scope: LabelScope,
}

/// A pin on a schematic symbol.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SchematicPin {
    /// Pin number (e.g. "1", "2", "A1").
    pub number: String,
    /// Pin name (e.g. "VCC", "GND", "D0").
    pub name: String,
    /// Electrical type for ERC.
    pub pin_type: PinType,
    /// Position relative to component origin.
    pub position: Vec2,
}

/// A placed component instance on a schematic sheet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SchematicComponent {
    /// Reference designator (e.g. "R1", "U3", "C5").
    #[serde(rename = "ref")]
    pub reference: String,
    /// Component value (e.g. "10k", "100nF", "ATmega328P").
    pub value: String,
    /// Footprint identifier (e.g. "Resistor_SMD:R_0805").
    #[serde(rename = "footprintId")]
    pub footprint_id: String,
    /// Position on the sheet.
    pub position: Vec2,
    /// Rotation in degrees.
    #[serde(default)]
    pub rotation: f64,
    /// Mirror the component horizontally.
    #[serde(default)]
    pub mirror: bool,
    /// Component pins.
    pub pins: Vec<SchematicPin>,
    /// Extra properties (manufacturer, datasheet URL, etc.).
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub properties: std::collections::HashMap<String, String>,
}

/// A wire connection on a schematic.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SchematicWire {
    /// Wire start point.
    pub start: Vec2,
    /// Wire end point.
    pub end: Vec2,
}

/// An explicit wire junction point.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SchematicJunction {
    /// Junction position.
    pub position: Vec2,
}

/// A schematic sheet — top-level schematic container.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SchematicSheet {
    /// Sheet title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Placed component instances.
    #[serde(default)]
    pub components: Vec<SchematicComponent>,
    /// Wire connections.
    #[serde(default)]
    pub wires: Vec<SchematicWire>,
    /// Wire junction points.
    #[serde(default)]
    pub junctions: Vec<SchematicJunction>,
    /// Net labels.
    #[serde(default)]
    pub labels: Vec<SchematicLabel>,
    /// Explicit netlist: net name → pin refs (`"R1.2"`). Merged with (and
    /// taking name precedence over) wire/label-derived connectivity, so
    /// callers can declare nets as data instead of relying on coordinate
    /// coincidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nets: Option<std::collections::BTreeMap<String, Vec<String>>>,
}

// ============================================================================
// Builtin symbol/footprint library types
// ============================================================================

/// A graphic element in a schematic symbol.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SymbolGraphic {
    /// Rectangle.
    #[serde(rename = "rect")]
    Rect {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    },
    /// Line segment.
    #[serde(rename = "line")]
    Line { x1: f64, y1: f64, x2: f64, y2: f64 },
    /// Circle.
    #[serde(rename = "circle")]
    Circle { cx: f64, cy: f64, r: f64 },
    /// Polyline (sequence of connected points).
    #[serde(rename = "polyline")]
    Polyline { points: Vec<Vec2> },
}

/// A parametric footprint template (pads + silkscreen graphics).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FootprintTemplate {
    /// Footprint name (e.g. "0805", "SOIC-8", "DIP-14").
    pub name: String,
    /// Pads.
    pub pads: Vec<Pad>,
    /// Silkscreen / courtyard graphics.
    pub graphics: Vec<FootprintGraphic>,
}

/// A builtin symbol definition for schematic component placement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SymbolDef {
    /// Unique identifier (e.g. "resistor", "capacitor", "npn").
    pub id: String,
    /// Display name (e.g. "Resistor", "Capacitor").
    pub name: String,
    /// Reference designator prefix (e.g. "R", "C", "U").
    pub prefix: String,
    /// Default component value (e.g. "10k", "100nF").
    #[serde(rename = "defaultValue")]
    pub default_value: String,
    /// Pin definitions.
    pub pins: Vec<SchematicPin>,
    /// Graphics for rendering the symbol.
    pub graphics: Vec<SymbolGraphic>,
    /// Associated footprint template (None for power symbols).
    #[serde(rename = "footprintTemplate")]
    pub footprint_template: Option<FootprintTemplate>,
}

// ============================================================================
// PCB Layer types
// ============================================================================

/// PCB layer identifiers (KiCad-compatible).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PcbLayer {
    // Copper layers
    /// Front copper.
    FCu,
    /// Back copper.
    BCu,
    /// Inner copper layer 1.
    In1Cu,
    /// Inner copper layer 2.
    In2Cu,
    /// Inner copper layer 3.
    In3Cu,
    /// Inner copper layer 4.
    In4Cu,
    /// Inner copper layer 5.
    In5Cu,
    /// Inner copper layer 6.
    In6Cu,

    // Mask/paste layers
    /// Front solder mask.
    FSilkS,
    /// Back solder mask.
    BSilkS,
    /// Front solder mask.
    FMask,
    /// Back solder mask.
    BMask,
    /// Front solder paste.
    FPaste,
    /// Back solder paste.
    BPaste,

    // Fabrication/documentation layers
    /// Front fabrication.
    FFab,
    /// Back fabrication.
    BFab,
    /// Front courtyard.
    FCrtYd,
    /// Back courtyard.
    BCrtYd,

    // Mechanical layers
    /// Edge cuts (board outline).
    EdgeCuts,
    /// User drawing layer.
    UserDrawings,
    /// User comments layer.
    UserComments,
}

impl PcbLayer {
    /// Returns true if this is a copper layer.
    pub fn is_copper(&self) -> bool {
        matches!(
            self,
            PcbLayer::FCu
                | PcbLayer::BCu
                | PcbLayer::In1Cu
                | PcbLayer::In2Cu
                | PcbLayer::In3Cu
                | PcbLayer::In4Cu
                | PcbLayer::In5Cu
                | PcbLayer::In6Cu
        )
    }
}

// ============================================================================
// PCB Stackup
// ============================================================================

/// A single layer in the physical board stackup.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StackupLayer {
    /// Layer identifier.
    pub layer: PcbLayer,
    /// Copper thickness in mm (for copper layers).
    #[serde(rename = "copperThickness", skip_serializing_if = "Option::is_none")]
    pub copper_thickness: Option<f64>,
    /// Dielectric thickness in mm (distance to next copper layer).
    #[serde(
        rename = "dielectricThickness",
        skip_serializing_if = "Option::is_none"
    )]
    pub dielectric_thickness: Option<f64>,
    /// Dielectric constant (relative permittivity).
    #[serde(rename = "dielectricEr", skip_serializing_if = "Option::is_none")]
    pub dielectric_er: Option<f64>,
    /// Material name (e.g. "FR4", "Rogers 4350B").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub material: Option<String>,
}

/// Physical layer stackup for impedance calculations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayerStackup {
    /// Ordered layers from top to bottom.
    pub layers: Vec<StackupLayer>,
}

// ============================================================================
// Board outline
// ============================================================================

/// Board outline profile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoardOutline {
    /// Closed outline vertices (last connects to first).
    pub vertices: Vec<Vec2>,
    /// Cutout holes in the board.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cutouts: Vec<Vec<Vec2>>,
    /// Board thickness in mm.
    pub thickness: f64,
}

// ============================================================================
// Design Rules
// ============================================================================

/// Per-net-class design rules.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NetClassRules {
    /// Net class name.
    pub name: String,
    /// Minimum trace width in mm.
    #[serde(rename = "traceWidth")]
    pub trace_width: f64,
    /// Minimum clearance in mm.
    pub clearance: f64,
    /// Via diameter in mm.
    #[serde(rename = "viaDiameter")]
    pub via_diameter: f64,
    /// Via drill diameter in mm.
    #[serde(rename = "viaDrill")]
    pub via_drill: f64,
    /// Differential pair gap in mm.
    #[serde(rename = "diffPairGap", skip_serializing_if = "Option::is_none")]
    pub diff_pair_gap: Option<f64>,
    /// Differential pair trace width in mm.
    #[serde(rename = "diffPairWidth", skip_serializing_if = "Option::is_none")]
    pub diff_pair_width: Option<f64>,
}

/// Board-level design rules.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DesignRules {
    /// Default net class rules.
    #[serde(rename = "defaultRules")]
    pub default_rules: NetClassRules,
    /// Per-class overrides.
    #[serde(rename = "classRules", default, skip_serializing_if = "Vec::is_empty")]
    pub class_rules: Vec<NetClassRules>,
    /// Nets assigned to each class (class name → vec of net IDs).
    #[serde(
        rename = "netClassAssignments",
        default,
        skip_serializing_if = "std::collections::HashMap::is_empty"
    )]
    pub net_class_assignments: std::collections::HashMap<String, Vec<NetId>>,
    /// Minimum board edge clearance in mm.
    #[serde(rename = "edgeClearance")]
    pub edge_clearance: f64,
    /// Minimum hole-to-hole distance in mm.
    #[serde(rename = "holeToHole")]
    pub hole_to_hole: f64,
    /// Minimum annular ring in mm.
    #[serde(rename = "minAnnularRing")]
    pub min_annular_ring: f64,
    /// Minimum drill diameter in mm.
    #[serde(rename = "minDrill")]
    pub min_drill: f64,
}

// ============================================================================
// Pad types
// ============================================================================

/// Pad shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PadShape {
    /// Circular pad.
    Circle {
        /// Pad diameter in mm.
        diameter: f64,
    },
    /// Rectangular pad.
    Rect {
        /// Width in mm.
        width: f64,
        /// Height in mm.
        height: f64,
    },
    /// Oval pad.
    Oval {
        /// Width in mm.
        width: f64,
        /// Height in mm.
        height: f64,
    },
    /// Rounded rectangle pad.
    RoundRect {
        /// Width in mm.
        width: f64,
        /// Height in mm.
        height: f64,
        /// Corner radius ratio (0.0–1.0).
        #[serde(rename = "cornerRatio")]
        corner_ratio: f64,
    },
    /// Custom polygon pad.
    Custom {
        /// Polygon vertices.
        vertices: Vec<Vec2>,
    },
}

/// Pad mounting type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PadType {
    /// Through-hole technology pad.
    THT,
    /// Surface-mount technology pad.
    SMD,
    /// Non-plated through-hole (mechanical only).
    NPTH,
}

/// Drill specification for through-hole pads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DrillSpec {
    /// Drill diameter in mm.
    pub diameter: f64,
    /// If true, drill is oval/slot.
    #[serde(default)]
    pub oval: bool,
    /// Secondary diameter for oval drill.
    #[serde(rename = "ovalHeight", skip_serializing_if = "Option::is_none")]
    pub oval_height: Option<f64>,
}

/// A pad on a footprint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pad {
    /// Pad number/name (e.g. "1", "A1").
    pub number: String,
    /// Pad mounting type.
    #[serde(rename = "padType")]
    pub pad_type: PadType,
    /// Pad shape.
    pub shape: PadShape,
    /// Position relative to footprint origin.
    pub position: Vec2,
    /// Rotation in degrees.
    #[serde(default)]
    pub rotation: f64,
    /// Drill specification (for THT/NPTH pads).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drill: Option<DrillSpec>,
    /// Net this pad is connected to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub net: Option<NetId>,
    /// Layers this pad exists on.
    pub layers: Vec<PcbLayer>,
}

// ============================================================================
// Footprint
// ============================================================================

/// A graphic element on a footprint (silkscreen, courtyard, etc.).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum FootprintGraphic {
    /// A line segment.
    Line {
        /// Start point.
        start: Vec2,
        /// End point.
        end: Vec2,
        /// Line width in mm.
        width: f64,
        /// Layer this graphic is on.
        layer: PcbLayer,
    },
    /// A circle.
    Circle {
        /// Center point.
        center: Vec2,
        /// Radius in mm.
        radius: f64,
        /// Line width in mm.
        width: f64,
        /// Layer this graphic is on.
        layer: PcbLayer,
    },
    /// An arc segment.
    Arc {
        /// Center point.
        center: Vec2,
        /// Radius in mm.
        radius: f64,
        /// Start angle in degrees.
        #[serde(rename = "startAngle")]
        start_angle: f64,
        /// End angle in degrees.
        #[serde(rename = "endAngle")]
        end_angle: f64,
        /// Line width in mm.
        width: f64,
        /// Layer this graphic is on.
        layer: PcbLayer,
    },
    /// A rectangle.
    Rect {
        /// Top-left corner.
        start: Vec2,
        /// Bottom-right corner.
        end: Vec2,
        /// Line width in mm.
        width: f64,
        /// Layer this graphic is on.
        layer: PcbLayer,
    },
    /// A polygon.
    Polygon {
        /// Polygon vertices.
        vertices: Vec<Vec2>,
        /// Line width in mm.
        width: f64,
        /// Layer this graphic is on.
        layer: PcbLayer,
    },
    /// Text annotation.
    Text {
        /// Text content.
        text: String,
        /// Position.
        position: Vec2,
        /// Rotation in degrees.
        #[serde(default)]
        rotation: f64,
        /// Text height in mm.
        height: f64,
        /// Line width in mm.
        width: f64,
        /// Layer this graphic is on.
        layer: PcbLayer,
    },
}

/// A placed footprint on the PCB.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Footprint {
    /// Reference designator (e.g. "R1", "U3").
    #[serde(rename = "ref")]
    pub reference: String,
    /// Component value (e.g. "10k", "100nF").
    pub value: String,
    /// Footprint library name (e.g. "Resistor_SMD:R_0805").
    #[serde(rename = "footprintName")]
    pub footprint_name: String,
    /// Position on the board.
    pub position: Vec2,
    /// Rotation in degrees.
    #[serde(default)]
    pub rotation: f64,
    /// Which side of the board (true = front, false = back).
    #[serde(default = "default_true")]
    pub front: bool,
    /// Pads on this footprint.
    pub pads: Vec<Pad>,
    /// Graphics (silkscreen, courtyard, etc.).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub graphics: Vec<FootprintGraphic>,
    /// Reference to a 3D model file (STEP/VRML).
    #[serde(rename = "model3d", skip_serializing_if = "Option::is_none")]
    pub model_3d: Option<String>,
    /// Extra properties.
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub properties: std::collections::HashMap<String, String>,
}

fn default_true() -> bool {
    true
}

// ============================================================================
// Traces and Vias
// ============================================================================

/// A straight routed copper trace segment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Trace {
    /// Trace start point.
    pub start: Vec2,
    /// Trace end point.
    pub end: Vec2,
    /// Trace width in mm.
    pub width: f64,
    /// Layer this trace is on.
    pub layer: PcbLayer,
    /// Net this trace belongs to.
    pub net: NetId,
}

/// An arc routed copper trace segment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TraceArc {
    /// Arc center point.
    pub center: Vec2,
    /// Arc radius in mm.
    pub radius: f64,
    /// Start angle in degrees.
    #[serde(rename = "startAngle")]
    pub start_angle: f64,
    /// End angle in degrees.
    #[serde(rename = "endAngle")]
    pub end_angle: f64,
    /// Trace width in mm.
    pub width: f64,
    /// Layer this trace arc is on.
    pub layer: PcbLayer,
    /// Net this trace arc belongs to.
    pub net: NetId,
}

/// A via (layer-spanning connection).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Via {
    /// Via center position.
    pub position: Vec2,
    /// Via pad diameter in mm.
    pub diameter: f64,
    /// Drill diameter in mm.
    pub drill: f64,
    /// Start layer (typically FCu).
    #[serde(rename = "startLayer")]
    pub start_layer: PcbLayer,
    /// End layer (typically BCu).
    #[serde(rename = "endLayer")]
    pub end_layer: PcbLayer,
    /// Net this via belongs to.
    pub net: NetId,
}

// ============================================================================
// Zones and Keepouts
// ============================================================================

/// Copper fill type for zones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ZoneFillType {
    /// Solid copper fill.
    Solid,
    /// Hatched copper fill.
    Hatched,
}

/// Thermal relief style for zone connections.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThermalReliefStyle {
    /// Full connection (direct copper).
    Direct,
    /// Thermal relief with cross-shaped gaps.
    Relief,
    /// No connection to zone.
    None,
}

/// A copper pour zone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Zone {
    /// Zone outline vertices (closed polygon).
    pub outline: Vec<Vec2>,
    /// Cutout holes in the zone.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub holes: Vec<Vec<Vec2>>,
    /// Net this zone is assigned to.
    pub net: NetId,
    /// Layer this zone is on.
    pub layer: PcbLayer,
    /// Clearance from other-net copper in mm.
    pub clearance: f64,
    /// Minimum copper island area in mm².
    #[serde(rename = "minArea", default)]
    pub min_area: f64,
    /// Fill type.
    #[serde(rename = "fillType", default = "default_solid_fill")]
    pub fill_type: ZoneFillType,
    /// Thermal relief style for same-net pads.
    #[serde(rename = "thermalRelief", default = "default_thermal_relief")]
    pub thermal_relief: ThermalReliefStyle,
    /// Thermal relief gap width in mm.
    #[serde(rename = "thermalGap", skip_serializing_if = "Option::is_none")]
    pub thermal_gap: Option<f64>,
    /// Thermal relief spoke width in mm.
    #[serde(rename = "thermalSpokeWidth", skip_serializing_if = "Option::is_none")]
    pub thermal_spoke_width: Option<f64>,
    /// Priority (higher priority zones override lower).
    #[serde(default)]
    pub priority: u32,
}

fn default_solid_fill() -> ZoneFillType {
    ZoneFillType::Solid
}

fn default_thermal_relief() -> ThermalReliefStyle {
    ThermalReliefStyle::Relief
}

/// A keepout region (restricted area).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Keepout {
    /// Keepout outline vertices (closed polygon).
    pub outline: Vec<Vec2>,
    /// Layer(s) this keepout applies to.
    pub layers: Vec<PcbLayer>,
    /// Disallow traces in this area.
    #[serde(rename = "noTracks", default)]
    pub no_tracks: bool,
    /// Disallow vias in this area.
    #[serde(rename = "noVias", default)]
    pub no_vias: bool,
    /// Disallow copper pour in this area.
    #[serde(rename = "noPour", default)]
    pub no_pour: bool,
    /// Disallow component placement in this area.
    #[serde(rename = "noComponents", default)]
    pub no_components: bool,
}

/// An intentional connection between two or more otherwise-distinct nets
/// (a "net-tie").
///
/// Models a wye/star neutral point, a transformer center tap, or a split-ground
/// stitch: the joined nets keep their separate identities in the netlist, but
/// are treated as electrically one where they meet, so DRC does not flag the
/// deliberate junction as a short.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NetTie {
    /// Names of the nets joined at this tie (two or more).
    pub nets: Vec<String>,
    /// Optional center of the allowed join region (board coordinates, mm).
    ///
    /// When present together with `radius`, the short exemption only applies
    /// inside the region; outside it the tied nets must still observe clearance.
    /// When absent, the exemption applies board-wide.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<Vec2>,
    /// Optional radius of the allowed join region (mm).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radius: Option<f64>,
}

// ============================================================================
// Top-level PCB
// ============================================================================

/// A complete PCB design.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pcb {
    /// Board outline and thickness.
    pub outline: BoardOutline,
    /// Physical layer stackup.
    pub stackup: LayerStackup,
    /// Named nets.
    #[serde(default)]
    pub nets: Vec<Net>,
    /// Design rules.
    pub rules: DesignRules,
    /// Placed footprints.
    #[serde(default)]
    pub footprints: Vec<Footprint>,
    /// Routed trace segments.
    #[serde(default)]
    pub traces: Vec<Trace>,
    /// Routed trace arcs.
    #[serde(rename = "traceArcs", default, skip_serializing_if = "Vec::is_empty")]
    pub trace_arcs: Vec<TraceArc>,
    /// Vias.
    #[serde(default)]
    pub vias: Vec<Via>,
    /// Copper pour zones.
    #[serde(default)]
    pub zones: Vec<Zone>,
    /// Keepout regions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keepouts: Vec<Keepout>,
    /// Intentional net-ties (wye/star points, center taps, split-ground stitches).
    #[serde(rename = "netTies", default, skip_serializing_if = "Vec::is_empty")]
    pub net_ties: Vec<NetTie>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcb_roundtrip() {
        let pcb = Pcb {
            outline: BoardOutline {
                vertices: vec![
                    Vec2::new(0.0, 0.0),
                    Vec2::new(100.0, 0.0),
                    Vec2::new(100.0, 80.0),
                    Vec2::new(0.0, 80.0),
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
            footprints: vec![Footprint {
                reference: "R1".to_string(),
                value: "10k".to_string(),
                footprint_name: "Resistor_SMD:R_0805".to_string(),
                position: Vec2::new(25.0, 40.0),
                rotation: 0.0,
                front: true,
                pads: vec![
                    Pad {
                        number: "1".to_string(),
                        pad_type: PadType::SMD,
                        shape: PadShape::Rect {
                            width: 1.0,
                            height: 1.2,
                        },
                        position: Vec2::new(-1.0, 0.0),
                        rotation: 0.0,
                        drill: None,
                        net: Some("1".to_string()),
                        layers: vec![PcbLayer::FCu, PcbLayer::FPaste, PcbLayer::FMask],
                    },
                    Pad {
                        number: "2".to_string(),
                        pad_type: PadType::SMD,
                        shape: PadShape::Rect {
                            width: 1.0,
                            height: 1.2,
                        },
                        position: Vec2::new(1.0, 0.0),
                        rotation: 0.0,
                        drill: None,
                        net: Some("2".to_string()),
                        layers: vec![PcbLayer::FCu, PcbLayer::FPaste, PcbLayer::FMask],
                    },
                ],
                graphics: vec![],
                model_3d: None,
                properties: std::collections::HashMap::new(),
            }],
            traces: vec![Trace {
                start: Vec2::new(24.0, 40.0),
                end: Vec2::new(10.0, 40.0),
                width: 0.25,
                layer: PcbLayer::FCu,
                net: "1".to_string(),
            }],
            trace_arcs: vec![],
            vias: vec![Via {
                position: Vec2::new(10.0, 40.0),
                diameter: 0.8,
                drill: 0.4,
                start_layer: PcbLayer::FCu,
                end_layer: PcbLayer::BCu,
                net: "1".to_string(),
            }],
            zones: vec![Zone {
                outline: vec![
                    Vec2::new(0.0, 0.0),
                    Vec2::new(100.0, 0.0),
                    Vec2::new(100.0, 80.0),
                    Vec2::new(0.0, 80.0),
                ],
                holes: vec![],
                net: "2".to_string(),
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
        };

        let json = serde_json::to_string_pretty(&pcb).unwrap();
        let restored: Pcb = serde_json::from_str(&json).unwrap();
        assert_eq!(pcb, restored);
    }

    #[test]
    fn schematic_roundtrip() {
        let sheet = SchematicSheet {
            nets: None,
            title: Some("Test Schematic".to_string()),
            components: vec![SchematicComponent {
                reference: "R1".to_string(),
                value: "10k".to_string(),
                footprint_id: "Resistor_SMD:R_0805".to_string(),
                position: Vec2::new(100.0, 50.0),
                rotation: 0.0,
                mirror: false,
                pins: vec![
                    SchematicPin {
                        number: "1".to_string(),
                        name: "~".to_string(),
                        pin_type: PinType::Passive,
                        position: Vec2::new(-5.08, 0.0),
                    },
                    SchematicPin {
                        number: "2".to_string(),
                        name: "~".to_string(),
                        pin_type: PinType::Passive,
                        position: Vec2::new(5.08, 0.0),
                    },
                ],
                properties: std::collections::HashMap::new(),
            }],
            wires: vec![SchematicWire {
                start: Vec2::new(94.92, 50.0),
                end: Vec2::new(80.0, 50.0),
            }],
            junctions: vec![SchematicJunction {
                position: Vec2::new(80.0, 50.0),
            }],
            labels: vec![SchematicLabel {
                name: "VCC".to_string(),
                position: Vec2::new(80.0, 50.0),
                rotation: 0.0,
                scope: LabelScope::Global,
            }],
        };

        let json = serde_json::to_string_pretty(&sheet).unwrap();
        let restored: SchematicSheet = serde_json::from_str(&json).unwrap();
        assert_eq!(sheet, restored);
    }

    #[test]
    fn pad_shapes_tagged() {
        let circle = PadShape::Circle { diameter: 1.6 };
        let json = serde_json::to_string(&circle).unwrap();
        assert!(json.contains(r#""type":"Circle""#));

        let rrect = PadShape::RoundRect {
            width: 1.0,
            height: 1.5,
            corner_ratio: 0.25,
        };
        let json = serde_json::to_string(&rrect).unwrap();
        assert!(json.contains(r#""type":"RoundRect""#));

        let restored: PadShape = serde_json::from_str(&json).unwrap();
        assert_eq!(rrect, restored);
    }

    #[test]
    fn pcb_layer_copper_check() {
        assert!(PcbLayer::FCu.is_copper());
        assert!(PcbLayer::BCu.is_copper());
        assert!(PcbLayer::In1Cu.is_copper());
        assert!(!PcbLayer::FSilkS.is_copper());
        assert!(!PcbLayer::EdgeCuts.is_copper());
    }

    #[test]
    fn footprint_graphic_tagged() {
        let line = FootprintGraphic::Line {
            start: Vec2::new(0.0, 0.0),
            end: Vec2::new(1.0, 0.0),
            width: 0.12,
            layer: PcbLayer::FSilkS,
        };
        let json = serde_json::to_string(&line).unwrap();
        assert!(json.contains(r#""type":"Line""#));
        let restored: FootprintGraphic = serde_json::from_str(&json).unwrap();
        assert_eq!(line, restored);
    }
}
