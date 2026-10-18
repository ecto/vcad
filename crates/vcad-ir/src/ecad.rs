//! ECAD (Electronic CAD) types for PCB design within vcad.
//!
//! This module defines all data types needed for schematic capture, PCB layout,
//! and fabrication export. Types follow the same serde-tagged pattern as the
//! rest of vcad-ir.

use serde::{Deserialize, Serialize};

use crate::Vec2;

pub mod package;
pub use package::{
    BodyEnvelope, Box3D, DensityLevel, DerivedPart, FootprintBody, IpcGoals, LeadSpec,
    LeadTerminal, PackageClass, PackageFamily, PinAssignment, PinMap, PinNumbering, PinRole,
    ThermalPad,
};

pub mod receipt;
pub use receipt::{
    DrcSummary, PartReceiptLine, PowerIntegrityLine, Receipt, ReceiptStatus, RuleCount,
    SourcingLine, SourcingSnapshot,
};

// ============================================================================
// Common types
// ============================================================================

/// Unique identifier for a net (electrical connection).
pub type NetId = String;

/// A named electrical connection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
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
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
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
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
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
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
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
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
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
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
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
    /// Optional explicit pad geometry, overriding footprint-library resolution.
    ///
    /// When present, PCB placement uses these pads verbatim (assigning each
    /// pad's net/layers from the schematic) instead of resolving the
    /// `footprint_id` through the parametric footprint engine — an escape hatch
    /// for parts the library does not cover, or for fully agent-authored
    /// footprints. Pad `number`s should match this component's pin numbers.
    #[serde(rename = "pads", default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-rs", ts(rename = "pads", optional))]
    pub pads_override: Option<Vec<Pad>>,
    /// Extra properties (manufacturer, datasheet URL, etc.).
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub properties: std::collections::HashMap<String, String>,
}

/// A wire connection on a schematic.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub struct SchematicWire {
    /// Wire start point.
    pub start: Vec2,
    /// Wire end point.
    pub end: Vec2,
}

/// An explicit wire junction point.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub struct SchematicJunction {
    /// Junction position.
    pub position: Vec2,
}

/// A schematic sheet — top-level schematic container.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub struct SchematicSheet {
    /// Sheet title.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-rs", ts(optional))]
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
    #[cfg_attr(feature = "ts-rs", ts(optional))]
    pub nets: Option<std::collections::BTreeMap<String, Vec<String>>>,
}

// ============================================================================
// Builtin symbol/footprint library types
// ============================================================================

/// A graphic element in a schematic symbol.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
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
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
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
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
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
///
/// Each variant carries a `serde(alias = …)` for the dotted KiCad spelling
/// (`In1.Cu`, `Edge.Cuts`, …) so documents that were corrupted with those
/// names still deserialize — and re-serialize back to the canonical form. The
/// MCP write boundaries reject the dotted spellings outright (see
/// `validateLayer` in `packages/mcp/src/tools/ecad.ts`); the aliases are only a
/// safety net for data already on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub enum PcbLayer {
    // Copper layers
    /// Front copper.
    #[serde(alias = "F.Cu")]
    FCu,
    /// Back copper.
    #[serde(alias = "B.Cu")]
    BCu,
    /// Inner copper layer 1.
    #[serde(alias = "In1.Cu")]
    In1Cu,
    /// Inner copper layer 2.
    #[serde(alias = "In2.Cu")]
    In2Cu,
    /// Inner copper layer 3.
    #[serde(alias = "In3.Cu")]
    In3Cu,
    /// Inner copper layer 4.
    #[serde(alias = "In4.Cu")]
    In4Cu,
    /// Inner copper layer 5.
    #[serde(alias = "In5.Cu")]
    In5Cu,
    /// Inner copper layer 6.
    #[serde(alias = "In6.Cu")]
    In6Cu,
    /// Inner copper layer 7.
    #[serde(alias = "In7.Cu")]
    In7Cu,
    /// Inner copper layer 8.
    #[serde(alias = "In8.Cu")]
    In8Cu,
    /// Inner copper layer 9.
    #[serde(alias = "In9.Cu")]
    In9Cu,
    /// Inner copper layer 10.
    #[serde(alias = "In10.Cu")]
    In10Cu,
    /// Inner copper layer 11.
    #[serde(alias = "In11.Cu")]
    In11Cu,
    /// Inner copper layer 12.
    #[serde(alias = "In12.Cu")]
    In12Cu,
    /// Inner copper layer 13.
    #[serde(alias = "In13.Cu")]
    In13Cu,
    /// Inner copper layer 14.
    #[serde(alias = "In14.Cu")]
    In14Cu,
    /// Inner copper layer 15.
    #[serde(alias = "In15.Cu")]
    In15Cu,
    /// Inner copper layer 16.
    #[serde(alias = "In16.Cu")]
    In16Cu,
    /// Inner copper layer 17.
    #[serde(alias = "In17.Cu")]
    In17Cu,
    /// Inner copper layer 18.
    #[serde(alias = "In18.Cu")]
    In18Cu,
    /// Inner copper layer 19.
    #[serde(alias = "In19.Cu")]
    In19Cu,
    /// Inner copper layer 20.
    #[serde(alias = "In20.Cu")]
    In20Cu,
    /// Inner copper layer 21.
    #[serde(alias = "In21.Cu")]
    In21Cu,
    /// Inner copper layer 22.
    #[serde(alias = "In22.Cu")]
    In22Cu,
    /// Inner copper layer 23.
    #[serde(alias = "In23.Cu")]
    In23Cu,
    /// Inner copper layer 24.
    #[serde(alias = "In24.Cu")]
    In24Cu,
    /// Inner copper layer 25.
    #[serde(alias = "In25.Cu")]
    In25Cu,
    /// Inner copper layer 26.
    #[serde(alias = "In26.Cu")]
    In26Cu,
    /// Inner copper layer 27.
    #[serde(alias = "In27.Cu")]
    In27Cu,
    /// Inner copper layer 28.
    #[serde(alias = "In28.Cu")]
    In28Cu,
    /// Inner copper layer 29.
    #[serde(alias = "In29.Cu")]
    In29Cu,
    /// Inner copper layer 30.
    #[serde(alias = "In30.Cu")]
    In30Cu,

    // Mask/paste layers
    /// Front solder mask.
    #[serde(alias = "F.SilkS")]
    FSilkS,
    /// Back solder mask.
    #[serde(alias = "B.SilkS")]
    BSilkS,
    /// Front solder mask.
    #[serde(alias = "F.Mask")]
    FMask,
    /// Back solder mask.
    #[serde(alias = "B.Mask")]
    BMask,
    /// Front solder paste.
    #[serde(alias = "F.Paste")]
    FPaste,
    /// Back solder paste.
    #[serde(alias = "B.Paste")]
    BPaste,

    // Fabrication/documentation layers
    /// Front fabrication.
    #[serde(alias = "F.Fab")]
    FFab,
    /// Back fabrication.
    #[serde(alias = "B.Fab")]
    BFab,
    /// Front courtyard.
    #[serde(alias = "F.CrtYd")]
    FCrtYd,
    /// Back courtyard.
    #[serde(alias = "B.CrtYd")]
    BCrtYd,

    // Mechanical layers
    /// Edge cuts (board outline).
    #[serde(alias = "Edge.Cuts")]
    EdgeCuts,
    /// User drawing layer.
    #[serde(alias = "User.Drawings")]
    UserDrawings,
    /// User comments layer.
    #[serde(alias = "User.Comments")]
    UserComments,
}

impl PcbLayer {
    /// Every copper layer the IR can represent, ordered top → bottom.
    ///
    /// The single source of truth for copper-stack enumeration: DRC, DFM and
    /// geometry all derive their layer masks / Z order from this. Hand-copied
    /// lists drifted (In7Cu/In8Cu went missing in three places, so a 10-layer
    /// board's In7/In8 copper touched nothing).
    pub const COPPER_STACK: [PcbLayer; 32] = [
        PcbLayer::FCu,
        PcbLayer::In1Cu,
        PcbLayer::In2Cu,
        PcbLayer::In3Cu,
        PcbLayer::In4Cu,
        PcbLayer::In5Cu,
        PcbLayer::In6Cu,
        PcbLayer::In7Cu,
        PcbLayer::In8Cu,
        PcbLayer::In9Cu,
        PcbLayer::In10Cu,
        PcbLayer::In11Cu,
        PcbLayer::In12Cu,
        PcbLayer::In13Cu,
        PcbLayer::In14Cu,
        PcbLayer::In15Cu,
        PcbLayer::In16Cu,
        PcbLayer::In17Cu,
        PcbLayer::In18Cu,
        PcbLayer::In19Cu,
        PcbLayer::In20Cu,
        PcbLayer::In21Cu,
        PcbLayer::In22Cu,
        PcbLayer::In23Cu,
        PcbLayer::In24Cu,
        PcbLayer::In25Cu,
        PcbLayer::In26Cu,
        PcbLayer::In27Cu,
        PcbLayer::In28Cu,
        PcbLayer::In29Cu,
        PcbLayer::In30Cu,
        PcbLayer::BCu,
    ];

    /// Every inner copper layer, `In1Cu..=In30Cu` (KiCad's full inner range).
    pub const INNER_COPPER: [PcbLayer; 30] = [
        PcbLayer::In1Cu,
        PcbLayer::In2Cu,
        PcbLayer::In3Cu,
        PcbLayer::In4Cu,
        PcbLayer::In5Cu,
        PcbLayer::In6Cu,
        PcbLayer::In7Cu,
        PcbLayer::In8Cu,
        PcbLayer::In9Cu,
        PcbLayer::In10Cu,
        PcbLayer::In11Cu,
        PcbLayer::In12Cu,
        PcbLayer::In13Cu,
        PcbLayer::In14Cu,
        PcbLayer::In15Cu,
        PcbLayer::In16Cu,
        PcbLayer::In17Cu,
        PcbLayer::In18Cu,
        PcbLayer::In19Cu,
        PcbLayer::In20Cu,
        PcbLayer::In21Cu,
        PcbLayer::In22Cu,
        PcbLayer::In23Cu,
        PcbLayer::In24Cu,
        PcbLayer::In25Cu,
        PcbLayer::In26Cu,
        PcbLayer::In27Cu,
        PcbLayer::In28Cu,
        PcbLayer::In29Cu,
        PcbLayer::In30Cu,
    ];

    /// Inner copper layer `n` (1-based), or `None` outside `1..=30`.
    pub fn inner(n: usize) -> Option<PcbLayer> {
        n.checked_sub(1)
            .and_then(|i| Self::INNER_COPPER.get(i).copied())
    }

    /// 1-based inner-layer number (`In3Cu` → 3); `None` for outer/non-copper.
    pub fn inner_index(&self) -> Option<usize> {
        Self::INNER_COPPER
            .iter()
            .position(|l| l == self)
            .map(|i| i + 1)
    }

    /// Copper stack of a board with `copper_count` copper layers, top →
    /// bottom: `FCu, In1Cu..In(n-2)Cu, BCu`. A count of 1 is `[FCu]`; counts
    /// above 32 return `None` — never silently truncated.
    pub fn copper_stack(copper_count: usize) -> Option<Vec<PcbLayer>> {
        match copper_count {
            0 => Some(Vec::new()),
            1 => Some(vec![PcbLayer::FCu]),
            n if n <= Self::COPPER_STACK.len() => {
                let mut v = vec![PcbLayer::FCu];
                v.extend_from_slice(&Self::INNER_COPPER[..n - 2]);
                v.push(PcbLayer::BCu);
                Some(v)
            }
            _ => None,
        }
    }

    /// Returns true if this is a copper layer.
    /// The same layer seen from the other side of the board: front and back
    /// swap (copper, silk, mask, paste, fab, courtyard); inner copper and the
    /// board-wide layers stay. What KiCad does to a footprint it flips.
    pub fn flipped(&self) -> PcbLayer {
        use PcbLayer::*;
        match self {
            FCu => BCu,
            BCu => FCu,
            FSilkS => BSilkS,
            BSilkS => FSilkS,
            FMask => BMask,
            BMask => FMask,
            FPaste => BPaste,
            BPaste => FPaste,
            FFab => BFab,
            BFab => FFab,
            FCrtYd => BCrtYd,
            BCrtYd => FCrtYd,
            other => *other,
        }
    }

    pub fn is_copper(&self) -> bool {
        self.copper_position().is_some()
    }

    /// Position of a copper layer in the physical stack, for span ordering:
    /// front copper is 0, inner layers are 1..=30, back copper is always the
    /// deepest regardless of how many inner layers the board actually has.
    /// Returns `None` for non-copper layers.
    pub fn copper_position(&self) -> Option<u8> {
        match self {
            PcbLayer::FCu => Some(0),
            PcbLayer::BCu => Some(u8::MAX),
            l => l.inner_index().map(|i| i as u8),
        }
    }

    /// Whether a via spanning `start..=end` (in either order) lands on this
    /// copper layer. Non-copper layers are never spanned.
    pub fn spanned_by(&self, start: PcbLayer, end: PcbLayer) -> bool {
        let (Some(pos), Some(a), Some(b)) = (
            self.copper_position(),
            start.copper_position(),
            end.copper_position(),
        ) else {
            return false;
        };
        pos >= a.min(b) && pos <= a.max(b)
    }
}

// ============================================================================
// PCB Stackup
// ============================================================================

/// A single layer in the physical board stackup.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub struct StackupLayer {
    /// Layer identifier.
    pub layer: PcbLayer,
    /// Copper thickness in mm (for copper layers).
    #[serde(rename = "copperThickness", skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-rs", ts(rename = "copperThickness", optional))]
    pub copper_thickness: Option<f64>,
    /// Dielectric thickness in mm (distance to next copper layer).
    #[serde(
        rename = "dielectricThickness",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "ts-rs", ts(rename = "dielectricThickness", optional))]
    pub dielectric_thickness: Option<f64>,
    /// Dielectric constant (relative permittivity).
    #[serde(rename = "dielectricEr", skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-rs", ts(rename = "dielectricEr", optional))]
    pub dielectric_er: Option<f64>,
    /// Material name (e.g. "FR4", "Rogers 4350B").
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-rs", ts(optional))]
    pub material: Option<String>,
    /// Copper layer role (signal / plane / split plane). `None`: unknown —
    /// treated as signal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-rs", ts(optional))]
    pub role: Option<LayerRole>,
    /// True when `role` was INFERRED (e.g. from pour coverage on import)
    /// rather than authored.
    #[serde(
        rename = "roleInferred",
        default,
        skip_serializing_if = "std::ops::Not::not"
    )]
    #[cfg_attr(
        feature = "ts-rs",
        ts(rename = "roleInferred", optional, as = "Option<bool>")
    )]
    pub role_inferred: bool,
}

/// What a copper layer is for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum LayerRole {
    /// Routing layer.
    Signal,
    /// A solid reference plane for one net: no signal traces (vias and their
    /// anti-pads only).
    Plane {
        /// The plane's net.
        net: NetId,
    },
    /// Power layer split between several pour nets: no signal traces.
    #[serde(rename = "split")]
    SplitPlane {
        /// The pour nets sharing the layer.
        nets: Vec<NetId>,
    },
}

impl LayerRole {
    /// True for a layer no signal trace may run on.
    pub fn is_plane(&self) -> bool {
        !matches!(self, LayerRole::Signal)
    }
}

/// Physical layer stackup for impedance calculations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub struct LayerStackup {
    /// Ordered layers from top to bottom.
    pub layers: Vec<StackupLayer>,
}

// ============================================================================
// Board outline
// ============================================================================

/// Board outline profile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
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
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
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
    #[cfg_attr(feature = "ts-rs", ts(rename = "diffPairGap", optional))]
    pub diff_pair_gap: Option<f64>,
    /// Differential pair trace width in mm.
    #[serde(rename = "diffPairWidth", skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-rs", ts(rename = "diffPairWidth", optional))]
    pub diff_pair_width: Option<f64>,
    /// Target single-ended characteristic impedance in ohms.
    ///
    /// When set — and when the stackup carries the dielectric data needed to
    /// solve — the trace width that hits this impedance is derived *per layer*
    /// (microstrip on the outers, stripline on the inners), rather than reusing
    /// [`Self::trace_width`] on every layer. Absent the data, the declared
    /// width stands and the impedance is reported unverified.
    #[serde(rename = "targetImpedance", skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-rs", ts(rename = "targetImpedance", optional))]
    pub target_impedance: Option<f64>,
    /// Target differential impedance in ohms (e.g. 90 for USB, 100 for PCIe).
    ///
    /// Same contract as [`Self::target_impedance`], solving the pair leg width
    /// at the class's declared [`Self::diff_pair_gap`].
    #[serde(
        rename = "targetDiffImpedance",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "ts-rs", ts(rename = "targetDiffImpedance", optional))]
    pub target_diff_impedance: Option<f64>,
    /// Neck-down minimum width (mm): within the neck region of a same-net pad
    /// (see [`Self::neck_length`]) a trace may narrow to this width instead of
    /// [`Self::trace_width`] — the pad-entry neck of a BGA / fine-pitch land.
    /// `None`: no neck allowance, the class width applies everywhere.
    #[serde(rename = "neckWidth", default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-rs", ts(rename = "neckWidth", optional))]
    pub neck_width: Option<f64>,
    /// Neck region length (mm), measured from the pad's copper edge: a neck
    /// segment must lie entirely within this distance of a same-net pad.
    /// `None` with a `neck_width` set: [`DEFAULT_NECK_LENGTH`].
    #[serde(
        rename = "neckLength",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "ts-rs", ts(rename = "neckLength", optional))]
    pub neck_length: Option<f64>,
}

/// Board-level design rules.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub struct DesignRules {
    /// Default net class rules.
    #[serde(rename = "defaultRules")]
    pub default_rules: NetClassRules,
    /// Per-class overrides.
    #[serde(rename = "classRules", default, skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(feature = "ts-rs", ts(rename = "classRules"))]
    pub class_rules: Vec<NetClassRules>,
    /// Nets assigned to each class (class name → vec of net IDs).
    #[serde(
        rename = "netClassAssignments",
        default,
        skip_serializing_if = "std::collections::HashMap::is_empty"
    )]
    #[cfg_attr(feature = "ts-rs", ts(rename = "netClassAssignments"))]
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
    /// Minimum microvia drill (mm) — KiCad's `uvia_min_drill`. Microvias
    /// ([`ViaType::Micro`]) are judged by these two minimums instead of
    /// `min_drill` / `min_annular_ring` (laser-drilled, one dielectric deep).
    #[serde(rename = "minMicroviaDrill", default = "default_min_microvia_drill")]
    #[cfg_attr(feature = "ts-rs", ts(rename = "minMicroviaDrill"))]
    pub min_microvia_drill: f64,
    /// Minimum microvia pad diameter (mm) — KiCad's `uvia_min_size`.
    #[serde(
        rename = "minMicroviaDiameter",
        default = "default_min_microvia_diameter"
    )]
    #[cfg_attr(feature = "ts-rs", ts(rename = "minMicroviaDiameter"))]
    pub min_microvia_diameter: f64,
    /// Provenance: names of the floors above (`minDrill`, `minAnnularRing`)
    /// that were INFERRED from the board's own copper because the source
    /// file authored none. DRC findings against an inferred floor say so.
    #[serde(
        rename = "inferredFloors",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    #[cfg_attr(feature = "ts-rs", ts(rename = "inferredFloors"))]
    pub inferred_floors: Vec<String>,
    /// Whether the fab may STACK microvias (a microvia landing on the one
    /// below, which needs filled/planarized microvias). `false` (default):
    /// consecutive microvias must be staggered (DRC `MicroviaStack`). An
    /// importer may infer `true` from a board that already stacks them; it
    /// then lists `allowStackedMicrovias` in [`Self::inferred_floors`].
    #[serde(
        rename = "allowStackedMicrovias",
        default,
        skip_serializing_if = "std::ops::Not::not"
    )]
    #[cfg_attr(
        feature = "ts-rs",
        ts(rename = "allowStackedMicrovias", optional, as = "Option<bool>")
    )]
    pub allow_stacked_microvias: bool,
}

/// Default neck region length from a pad's copper edge (mm) — the same
/// 1.5 mm pad-breakout region the diff-pair gap rule exempts.
pub const DEFAULT_NECK_LENGTH: f64 = 1.5;

/// KiCad's default minimum microvia drill (board setup `uvia_min_drill`).
pub const DEFAULT_MIN_MICROVIA_DRILL: f64 = 0.1;
/// KiCad's default minimum microvia diameter (board setup `uvia_min_size`).
pub const DEFAULT_MIN_MICROVIA_DIAMETER: f64 = 0.2;

fn default_min_microvia_drill() -> f64 {
    DEFAULT_MIN_MICROVIA_DRILL
}

fn default_min_microvia_diameter() -> f64 {
    DEFAULT_MIN_MICROVIA_DIAMETER
}

/// Via construction type, KiCad semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
#[serde(rename_all = "lowercase")]
pub enum ViaType {
    /// Through via (outer to outer).
    Through,
    /// Blind or buried via (KiCad `blind`).
    Blind,
    /// Laser microvia (KiCad `micro`): spans exactly two ADJACENT copper
    /// layers; judged by the microvia minimums.
    Micro,
}

// ============================================================================
// Pad types
// ============================================================================

/// Pad shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
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
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
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
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub struct DrillSpec {
    /// Drill diameter in mm.
    pub diameter: f64,
    /// If true, drill is oval/slot.
    #[serde(default)]
    pub oval: bool,
    /// Secondary diameter for oval drill.
    #[serde(rename = "ovalHeight", skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-rs", ts(rename = "ovalHeight", optional))]
    pub oval_height: Option<f64>,
}

/// A pad on a footprint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
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
    #[cfg_attr(feature = "ts-rs", ts(optional))]
    pub drill: Option<DrillSpec>,
    /// Net this pad is connected to.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-rs", ts(optional))]
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
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
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
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
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
    #[cfg_attr(feature = "ts-rs", ts(rename = "model3d", optional))]
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

/// Provenance of a piece of copper: who placed it.
///
/// The autorouter treats its own output as disposable — re-running `route_nets`
/// rips up `Autoroute` copper on the nets it routes and lays it fresh — while
/// `Manual` copper (hand-placed traces/vias, coils) is preserved. Copper with
/// no recorded source (documents from before provenance existed) is treated as
/// autorouted, matching the rip-up behavior those documents already had.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
#[serde(rename_all = "lowercase")]
pub enum CopperSource {
    /// Placed by the autorouter (`route_nets` / diff-pair routing).
    Autoroute,
    /// Hand-placed by a user or agent (`add_trace`, `add_via`, coil tools).
    Manual,
}

/// A straight routed copper trace segment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
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
    /// Who placed this copper. Absent on pre-provenance documents (treated as
    /// autorouted, i.e. rippable by `route_nets`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-rs", ts(optional))]
    pub source: Option<CopperSource>,
}

/// An arc routed copper trace segment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
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
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
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
    /// Who placed this via. Absent on pre-provenance documents (treated as
    /// autorouted, i.e. rippable by `route_nets`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-rs", ts(optional))]
    pub source: Option<CopperSource>,
    /// Declared construction type. `None` on documents that predate it: such
    /// a via is judged by the through/blind rules (never assumed micro).
    #[serde(rename = "viaType", default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-rs", ts(rename = "viaType", optional))]
    pub via_type: Option<ViaType>,
}

impl Via {
    /// True when declared a microvia.
    pub fn is_micro(&self) -> bool {
        self.via_type == Some(ViaType::Micro)
    }
}

// ============================================================================
// Zones and Keepouts
// ============================================================================

/// Copper fill type for zones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub enum ZoneFillType {
    /// Solid copper fill.
    Solid,
    /// Hatched copper fill.
    Hatched,
}

/// Thermal relief style for zone connections.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
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
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
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
    #[cfg_attr(feature = "ts-rs", ts(rename = "thermalGap", optional))]
    pub thermal_gap: Option<f64>,
    /// Thermal relief spoke width in mm.
    #[serde(rename = "thermalSpokeWidth", skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-rs", ts(rename = "thermalSpokeWidth", optional))]
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
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
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
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub struct NetTie {
    /// Names of the nets joined at this tie (two or more).
    pub nets: Vec<String>,
    /// Optional center of the allowed join region (board coordinates, mm).
    ///
    /// When present together with `radius`, the short exemption only applies
    /// inside the region; outside it the tied nets must still observe clearance.
    /// When absent, the exemption applies board-wide.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-rs", ts(optional))]
    pub position: Option<Vec2>,
    /// Optional radius of the allowed join region (mm).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-rs", ts(optional))]
    pub radius: Option<f64>,
}

// ============================================================================
// Top-level PCB
// ============================================================================

/// A complete PCB design.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
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
    #[cfg_attr(feature = "ts-rs", ts(rename = "traceArcs"))]
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
    #[cfg_attr(feature = "ts-rs", ts(rename = "netTies"))]
    pub net_ties: Vec<NetTie>,
}

/// Shoelace area of a closed polygon (absolute).
fn polygon_area(pts: &[Vec2]) -> f64 {
    let n = pts.len();
    if n < 3 {
        return 0.0;
    }
    let mut a = 0.0;
    for k in 0..n {
        let (p, q) = (pts[k], pts[(k + 1) % n]);
        a += p.x * q.y - q.x * p.y;
    }
    (a / 2.0).abs()
}

/// Share of all trace length a layer may carry and still be a plane.
pub const PLANE_MAX_SIGNAL_SHARE: f64 = 0.03;
/// Board-area share a single net's pour must cover for a plane layer (or the
/// pour nets together, for a split plane).
pub const PLANE_MIN_COVERAGE: f64 = 0.6;

impl Pcb {
    /// Copper layers of this board, front to back (the full stack when the
    /// stackup is empty).
    pub fn copper_stack(&self) -> Vec<PcbLayer> {
        PcbLayer::COPPER_STACK
            .iter()
            .copied()
            .filter(|l| {
                self.stackup.layers.is_empty() || self.stackup.layers.iter().any(|s| s.layer == *l)
            })
            .collect()
    }

    /// Index pairs of STACKED microvias: two declared microvias, each on one
    /// adjacent layer pair, on consecutive spans (sharing one copper layer),
    /// whose drills overlap in plan (centre distance below the larger drill).
    /// Staggered microvias are not listed.
    pub fn stacked_microvia_pairs(&self) -> Vec<(usize, usize)> {
        let stack = self.copper_stack();
        let idx = |l: PcbLayer| stack.iter().position(|&c| c == l);
        let micro: Vec<(usize, usize, usize)> = self
            .vias
            .iter()
            .enumerate()
            .filter(|(_, v)| v.is_micro())
            .filter_map(|(i, v)| {
                let (a, b) = (idx(v.start_layer)?, idx(v.end_layer)?);
                let (lo, hi) = (a.min(b), a.max(b));
                (hi == lo + 1).then_some((i, lo, hi))
            })
            .collect();
        let mut out = Vec::new();
        for x in 0..micro.len() {
            for y in (x + 1)..micro.len() {
                let ((i, alo, ahi), (j, blo, bhi)) = (micro[x], micro[y]);
                if !(ahi == blo || bhi == alo) {
                    continue;
                }
                let (va, vb) = (&self.vias[i], &self.vias[j]);
                let d = ((va.position.x - vb.position.x).powi(2)
                    + (va.position.y - vb.position.y).powi(2))
                .sqrt();
                if d < va.drill.max(vb.drill) - 1e-6 {
                    out.push((i, j));
                }
            }
        }
        out
    }

    /// Infer each copper layer's [`LayerRole`] from its copper, for layers
    /// with no authored role. A layer carrying at most
    /// [`PLANE_MAX_SIGNAL_SHARE`] of the board's trace length whose dominant
    /// pour net covers at least [`PLANE_MIN_COVERAGE`] of the board area is a
    /// [`LayerRole::Plane`] for that net; if instead several pour nets (each at
    /// least 5%) together cover that much, a [`LayerRole::SplitPlane`]; anything
    /// else is signal. Coverage is the zone OUTLINE area (minus holes), capped
    /// at the board area. Returns one log line per layer inferred a plane; marks
    /// each inferred layer `role_inferred`.
    pub fn infer_layer_roles(&mut self) -> Vec<String> {
        let board = polygon_area(&self.outline.vertices);
        let total: f64 = self.traces.iter().map(|t| (t.end - t.start).length()).sum();
        let mut log = Vec::new();
        if board <= 0.0 {
            return log;
        }
        for sl in &mut self.stackup.layers {
            if !sl.layer.is_copper() || (sl.role.is_some() && !sl.role_inferred) {
                continue;
            }
            let on: f64 = self
                .traces
                .iter()
                .filter(|t| t.layer == sl.layer)
                .map(|t| (t.end - t.start).length())
                .sum();
            let mut by_net: std::collections::BTreeMap<&str, f64> = Default::default();
            for z in self
                .zones
                .iter()
                .filter(|z| z.layer == sl.layer && !z.net.is_empty())
            {
                let a =
                    polygon_area(&z.outline) - z.holes.iter().map(|h| polygon_area(h)).sum::<f64>();
                *by_net.entry(z.net.as_str()).or_default() += a.max(0.0);
            }
            let cover = |a: f64| (a / board).min(1.0);
            let share = if total > 0.0 { on / total } else { 0.0 };
            let best = by_net
                .iter()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .map(|(n, a)| (n.to_string(), cover(*a)));
            let split: Vec<(String, f64)> = by_net
                .iter()
                .map(|(n, a)| (n.to_string(), cover(*a)))
                .filter(|(_, c)| *c >= 0.05)
                .collect();
            let split_cover: f64 = split.iter().map(|(_, c)| c).sum::<f64>().min(1.0);
            let role = if share > PLANE_MAX_SIGNAL_SHARE {
                LayerRole::Signal
            } else if let Some((net, c)) = best.clone().filter(|(_, c)| *c >= PLANE_MIN_COVERAGE) {
                let _ = c;
                LayerRole::Plane { net }
            } else if split.len() >= 2 && split_cover >= PLANE_MIN_COVERAGE {
                LayerRole::SplitPlane {
                    nets: split.iter().map(|(n, _)| n.clone()).collect(),
                }
            } else {
                LayerRole::Signal
            };
            // Log the plane calls (the ones that change routing); a layer
            // left signal is the default and stays quiet.
            if role.is_plane() {
                log.push(format!(
                "INFERRED layer role {:?}: {:?} (signal {:.0} mm = {:.1}% of all trace length; \
                 dominant pour {} {:.0}% of board; split-pour cover {:.0}%)",
                sl.layer,
                role,
                on.max(0.0),
                100.0 * share.max(0.0),
                best.as_ref().map(|b| b.0.as_str()).unwrap_or("-"),
                100.0 * best.as_ref().map(|b| b.1).unwrap_or(0.0),
                100.0 * split_cover
            ));
            }
            sl.role = Some(role);
            sl.role_inferred = true;
        }
        log
    }

    /// Copper layers no signal trace may run on (plane / split-plane role).
    pub fn plane_layers(&self) -> Vec<PcbLayer> {
        self.stackup
            .layers
            .iter()
            .filter(|l| l.role.as_ref().is_some_and(|r| r.is_plane()))
            .map(|l| l.layer)
            .collect()
    }
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
                        role: None,
                        role_inferred: false,
                        layer: PcbLayer::FCu,
                        copper_thickness: Some(0.035),
                        dielectric_thickness: Some(0.2),
                        dielectric_er: Some(4.5),
                        material: Some("FR4".to_string()),
                    },
                    StackupLayer {
                        role: None,
                        role_inferred: false,
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
                allow_stacked_microvias: false,
                inferred_floors: Vec::new(),
                min_microvia_drill: 0.1,
                min_microvia_diameter: 0.2,
                default_rules: NetClassRules {
                    neck_width: None,
                    neck_length: None,
                    name: "Default".to_string(),
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
                source: None,
            }],
            trace_arcs: vec![],
            vias: vec![Via {
                via_type: None,
                position: Vec2::new(10.0, 40.0),
                diameter: 0.8,
                drill: 0.4,
                start_layer: PcbLayer::FCu,
                end_layer: PcbLayer::BCu,
                net: "1".to_string(),
                source: None,
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
                pads_override: None,
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
        assert!(PcbLayer::In7Cu.is_copper());
        assert!(PcbLayer::In8Cu.is_copper());
        assert!(PcbLayer::In30Cu.is_copper());
    }

    #[test]
    fn copper_stack_by_count() {
        assert_eq!(
            PcbLayer::copper_stack(2).unwrap(),
            vec![PcbLayer::FCu, PcbLayer::BCu]
        );
        let s32 = PcbLayer::copper_stack(32).unwrap();
        assert_eq!(s32, PcbLayer::COPPER_STACK.to_vec());
        assert_eq!(PcbLayer::copper_stack(12).unwrap()[10], PcbLayer::In10Cu);
        assert!(PcbLayer::copper_stack(33).is_none());
        assert_eq!(PcbLayer::inner(30), Some(PcbLayer::In30Cu));
        assert_eq!(PcbLayer::inner(31), None);
        assert_eq!(PcbLayer::In17Cu.inner_index(), Some(17));
        let l: PcbLayer = serde_json::from_str("\"In22.Cu\"").unwrap();
        assert_eq!(l, PcbLayer::In22Cu);
    }

    #[test]
    fn pcb_layer_via_span() {
        // Through via spans everything.
        assert!(PcbLayer::In3Cu.spanned_by(PcbLayer::FCu, PcbLayer::BCu));
        assert!(PcbLayer::In8Cu.spanned_by(PcbLayer::FCu, PcbLayer::BCu));
        // Blind via FCu..In2 does not reach In3 or BCu.
        assert!(PcbLayer::In1Cu.spanned_by(PcbLayer::FCu, PcbLayer::In2Cu));
        assert!(!PcbLayer::In3Cu.spanned_by(PcbLayer::FCu, PcbLayer::In2Cu));
        assert!(!PcbLayer::BCu.spanned_by(PcbLayer::FCu, PcbLayer::In2Cu));
        // Buried via, order-insensitive.
        assert!(PcbLayer::In2Cu.spanned_by(PcbLayer::In3Cu, PcbLayer::In1Cu));
        assert!(!PcbLayer::FCu.spanned_by(PcbLayer::In1Cu, PcbLayer::In3Cu));
        // Non-copper is never spanned.
        assert!(!PcbLayer::FMask.spanned_by(PcbLayer::FCu, PcbLayer::BCu));
    }

    #[test]
    fn layer_roles_inferred_from_pour_coverage_and_signal_share() {
        let sq = |s: f64| {
            vec![
                Vec2::new(0.0, 0.0),
                Vec2::new(s, 0.0),
                Vec2::new(s, s),
                Vec2::new(0.0, s),
            ]
        };
        let zone = |net: &str, layer: PcbLayer, pts: Vec<Vec2>| Zone {
            outline: pts,
            holes: vec![],
            net: net.into(),
            layer,
            clearance: 0.2,
            min_area: 0.0,
            fill_type: ZoneFillType::Solid,
            thermal_relief: ThermalReliefStyle::Relief,
            thermal_gap: None,
            thermal_spoke_width: None,
            priority: 0,
        };
        let sl = |layer| StackupLayer {
            layer,
            copper_thickness: None,
            dielectric_thickness: None,
            dielectric_er: None,
            material: None,
            role: None,
            role_inferred: false,
        };
        let mut pcb: Pcb = serde_json::from_str(
            r#"{"outline":{"vertices":[],"cutouts":[],"thickness":1.6},"stackup":{"layers":[]},
                "rules":{"defaultRules":{"name":"D","traceWidth":0.1,"clearance":0.1,"viaDiameter":0.4,"viaDrill":0.2},
                "edgeClearance":0.3,"holeToHole":0.25,"minAnnularRing":0.1,"minDrill":0.2}}"#,
        )
        .unwrap();
        pcb.outline.vertices = sq(10.0);
        pcb.stackup.layers = vec![
            sl(PcbLayer::FCu),
            sl(PcbLayer::In1Cu),
            sl(PcbLayer::In2Cu),
            sl(PcbLayer::BCu),
        ];
        // In1: one GND pour over the board; In2: two half-board power pours.
        let half = |x0: f64| {
            vec![
                Vec2::new(x0, 0.0),
                Vec2::new(x0 + 5.0, 0.0),
                Vec2::new(x0 + 5.0, 10.0),
                Vec2::new(x0, 10.0),
            ]
        };
        pcb.zones = vec![
            zone("GND", PcbLayer::FCu, sq(10.0)),
            zone("GND", PcbLayer::In1Cu, sq(10.0)),
            zone("V1", PcbLayer::In2Cu, half(0.0)),
            zone("V2", PcbLayer::In2Cu, half(5.0)),
        ];
        // FCu carries the signal; BCu a bit too.
        for (l, len) in [(PcbLayer::FCu, 50.0), (PcbLayer::BCu, 40.0)] {
            pcb.traces.push(Trace {
                start: Vec2::new(0.0, 0.0),
                end: Vec2::new(len, 0.0),
                width: 0.1,
                layer: l,
                net: "S".into(),
                source: None,
            });
        }
        // An authored role wins over inference.
        pcb.stackup.layers[3].role = Some(LayerRole::Signal);
        let log = pcb.infer_layer_roles();
        assert_eq!(log.len(), 2, "only the plane calls are logged");
        let r = |i: usize| pcb.stackup.layers[i].role.clone().unwrap();
        assert_eq!(r(0), LayerRole::Signal, "FCu has a pour but carries signal");
        assert_eq!(r(1), LayerRole::Plane { net: "GND".into() });
        assert_eq!(
            r(2),
            LayerRole::SplitPlane {
                nets: vec!["V1".into(), "V2".into()]
            }
        );
        assert!(!pcb.stackup.layers[3].role_inferred);
        assert_eq!(pcb.plane_layers(), vec![PcbLayer::In1Cu, PcbLayer::In2Cu]);
    }

    #[test]
    fn copper_stack_lists_every_copper_layer_in_order() {
        use PcbLayer::*;
        // Exhaustive match: adding a PcbLayer variant breaks this compile,
        // forcing a decision about COPPER_STACK.
        fn all(l: PcbLayer) -> PcbLayer {
            match l {
                FCu | BCu | In1Cu | In2Cu | In3Cu | In4Cu | In5Cu | In6Cu | In7Cu | In8Cu
                | In9Cu | In10Cu | In11Cu | In12Cu | In13Cu | In14Cu | In15Cu | In16Cu | In17Cu
                | In18Cu | In19Cu | In20Cu | In21Cu | In22Cu | In23Cu | In24Cu | In25Cu
                | In26Cu | In27Cu | In28Cu | In29Cu | In30Cu | FSilkS | BSilkS | FMask | BMask
                | FPaste | BPaste | FFab | BFab | FCrtYd | BCrtYd | EdgeCuts | UserDrawings
                | UserComments => l,
            }
        }
        let every = [
            FCu,
            BCu,
            In1Cu,
            In2Cu,
            In3Cu,
            In4Cu,
            In5Cu,
            In6Cu,
            In7Cu,
            In8Cu,
            In9Cu,
            In10Cu,
            In11Cu,
            In12Cu,
            In13Cu,
            In14Cu,
            In15Cu,
            In16Cu,
            In17Cu,
            In18Cu,
            In19Cu,
            In20Cu,
            In21Cu,
            In22Cu,
            In23Cu,
            In24Cu,
            In25Cu,
            In26Cu,
            In27Cu,
            In28Cu,
            In29Cu,
            In30Cu,
            FSilkS,
            BSilkS,
            FMask,
            BMask,
            FPaste,
            BPaste,
            FFab,
            BFab,
            FCrtYd,
            BCrtYd,
            EdgeCuts,
            UserDrawings,
            UserComments,
        ]
        .map(all);
        let copper: Vec<_> = every.iter().filter(|l| l.is_copper()).collect();
        assert_eq!(copper.len(), PcbLayer::COPPER_STACK.len());
        for l in copper {
            assert!(PcbLayer::COPPER_STACK.contains(l), "{l:?} missing");
        }
        let pos: Vec<_> = PcbLayer::COPPER_STACK
            .iter()
            .map(|l| l.copper_position().unwrap())
            .collect();
        assert!(
            pos.windows(2).all(|w| w[0] < w[1]),
            "not top→bottom: {pos:?}"
        );
    }

    #[test]
    fn pcb_layer_serializes_canonical() {
        // The wire form is always the canonical (dotless) variant name.
        assert_eq!(
            serde_json::to_string(&PcbLayer::In1Cu).unwrap(),
            r#""In1Cu""#
        );
        assert_eq!(
            serde_json::to_string(&PcbLayer::EdgeCuts).unwrap(),
            r#""EdgeCuts""#
        );
    }

    #[test]
    fn pcb_layer_dotted_alias_deserializes() {
        // Documents corrupted with the dotted KiCad spelling still load — and
        // re-serialize back to the canonical form, healing the data on save.
        for (dotted, want) in [
            (r#""F.Cu""#, PcbLayer::FCu),
            (r#""B.Cu""#, PcbLayer::BCu),
            (r#""In1.Cu""#, PcbLayer::In1Cu),
            (r#""In6.Cu""#, PcbLayer::In6Cu),
            (r#""F.SilkS""#, PcbLayer::FSilkS),
            (r#""Edge.Cuts""#, PcbLayer::EdgeCuts),
        ] {
            let got: PcbLayer = serde_json::from_str(dotted).unwrap();
            assert_eq!(got, want, "alias {dotted} should map to {want:?}");
            // Round-trips to the canonical spelling, not back to the dotted form.
            assert!(!serde_json::to_string(&got).unwrap().contains('.'));
        }
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
