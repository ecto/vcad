//! Parametric package-class types — the single source of truth for a
//! generated electronic part.
//!
//! A [`PackageClass`] fully describes the *physical* package of a component
//! (body envelope, lead geometry, pin map, density target). From this one spec
//! the [`vcad-ecad-package`](../../../vcad-ecad-package) crate derives — in a
//! single pass — the PCB land pattern ([`FootprintTemplate`]), the schematic
//! symbol ([`SymbolDef`]), and a real 3D body ([`FootprintBody`]), all sharing
//! one pin numbering so that pad ↔ symbol-pin ↔ body-lead can never disagree.
//!
//! This is the "generate, don't aggregate" core: standard packages get
//! infinite coverage from parametric families instead of a scraped catalog,
//! and the geometry is correct-by-construction rather than three independently
//! authored files that drift.

use serde::{Deserialize, Serialize};

use super::{FootprintTemplate, SymbolDef};
use crate::{Vec2, Vec3};

// ============================================================================
// Geometry helpers
// ============================================================================

/// An axis-aligned 3D bounding box (millimeters), used for component bodies
/// and courtyards.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub struct Box3D {
    /// Minimum corner.
    pub min: Vec3,
    /// Maximum corner.
    pub max: Vec3,
}

impl Box3D {
    /// Create a box from its two corners (no ordering required).
    pub fn new(min: Vec3, max: Vec3) -> Self {
        Self {
            min: Vec3::new(min.x.min(max.x), min.y.min(max.y), min.z.min(max.z)),
            max: Vec3::new(min.x.max(max.x), min.y.max(max.y), min.z.max(max.z)),
        }
    }

    /// A centered box of the given full extents, with its base at `z_min`.
    pub fn centered_xy(len_x: f64, len_y: f64, z_min: f64, z_max: f64) -> Self {
        Self {
            min: Vec3::new(-len_x / 2.0, -len_y / 2.0, z_min),
            max: Vec3::new(len_x / 2.0, len_y / 2.0, z_max),
        }
    }

    /// Grow to include a 2D point on the XY plane (Z untouched).
    pub fn include_xy(&mut self, p: Vec2) {
        self.min.x = self.min.x.min(p.x);
        self.min.y = self.min.y.min(p.y);
        self.max.x = self.max.x.max(p.x);
        self.max.y = self.max.y.max(p.y);
    }

    /// Box height (Z extent).
    pub fn height(&self) -> f64 {
        self.max.z - self.min.z
    }

    /// True if `p`'s XY lies within the box's XY footprint (inclusive, with a
    /// small tolerance to absorb floating-point noise).
    pub fn contains_xy(&self, p: Vec2) -> bool {
        const EPS: f64 = 1e-9;
        p.x >= self.min.x - EPS
            && p.x <= self.max.x + EPS
            && p.y >= self.min.y - EPS
            && p.y <= self.max.y + EPS
    }
}

// ============================================================================
// Package description
// ============================================================================

/// The broad geometric class of a package, which selects the land-pattern
/// generator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub enum PackageFamily {
    /// Two-terminal chip passive (0402, 0603, 0805, …).
    Chip,
    /// Gull-wing leaded SMD (SOIC/SOP/SSOP/TSSOP, QFP/LQFP/TQFP).
    GullWing,
    /// No-lead SMD with terminals on the package periphery (QFN/DFN/SON).
    NoLead,
    /// J-lead SMD (PLCC, SOJ).
    JLead,
    /// Through-hole leaded (DIP, TO-220, radial).
    ThroughHole,
    /// Tabbed power SMD (DPAK/D2PAK).
    TabbedSmd,
    /// Pin header / socket.
    Header,
    /// Screw / spring terminal block.
    Terminal,
    /// Ball-grid array.
    Bga,
}

/// How a single terminal physically attaches to the board.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub enum LeadTerminal {
    /// Surface-mount land (no hole).
    Smd,
    /// Through-hole pin with the given drill diameter (mm).
    ThtPin {
        /// Drill diameter in mm.
        drill: f64,
    },
    /// Castellated half-via edge terminal.
    Castellated {
        /// Drill diameter in mm.
        drill: f64,
    },
}

/// The component body envelope (the molded/ceramic package, excluding leads).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub struct BodyEnvelope {
    /// Body length along X in mm.
    pub length: f64,
    /// Body width along Y in mm.
    pub width: f64,
    /// Body height (Z) in mm.
    pub height: f64,
    /// Standoff above the board surface in mm (0 for most SMD).
    #[serde(default)]
    pub standoff: f64,
}

/// Lead/terminal geometry, shared across all sides of the package.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub struct LeadSpec {
    /// Center-to-center terminal pitch in mm.
    pub pitch: f64,
    /// Number of terminals per populated side.
    pub count_per_side: u32,
    /// Number of populated sides (2 = dual, 4 = quad).
    pub sides: u8,
    /// Terminal contact length (the metallized land on the component, the
    /// dimension that runs radially in/out from the body edge) in mm.
    pub lead_length: f64,
    /// Terminal width (the dimension tangent to the body edge) in mm.
    pub lead_width: f64,
    /// How the terminal attaches to the board.
    pub terminal: LeadTerminal,
}

/// Functional role of a pin, used by ERC, net auto-assignment, and the
/// pin-role hard-reject gate in verified substitution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub enum PinRole {
    /// Power supply input.
    Power,
    /// Ground / return.
    Ground,
    /// Generic digital/analog signal.
    Signal,
    /// Analog signal.
    Analog,
    /// Clock.
    Clock,
    /// Reset / enable.
    Reset,
    /// Bidirectional I/O.
    Io,
    /// No internal connection.
    NoConnect,
    /// Exposed thermal pad (usually tied to ground/power).
    Thermal,
    /// Passive terminal (two-terminal parts).
    Passive,
    /// Diode anode.
    Anode,
    /// Diode cathode.
    Cathode,
    /// FET gate.
    Gate,
    /// FET drain.
    Drain,
    /// FET source.
    Source,
}

/// A single pin's identity within a package.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub struct PinAssignment {
    /// Pad/pin number (e.g. "1", "EP", "A1").
    pub number: String,
    /// Functional pin name (e.g. "VCC", "GND", "PA0").
    pub name: String,
    /// Functional role.
    pub role: PinRole,
}

/// Pin numbering convention for a package.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub enum PinNumbering {
    /// Counter-clockwise from pin 1 (top-left), the IPC convention for
    /// quad/dual SMD packages.
    Ccw,
    /// Dual in-line: down the left side, then up the right (DIP/SOIC).
    DualUpDown,
    /// Simple sequential 1..N (headers, chips).
    Sequential,
}

/// The pin map: numbering convention plus optional per-pin identities.
///
/// When `pins` is empty the generator synthesizes anonymous passive pins
/// `1..=N` (plus an exposed-pad pin if the package has a thermal pad).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub struct PinMap {
    /// Numbering convention.
    pub numbering: PinNumbering,
    /// Explicit pin identities (may be empty — see type docs).
    #[serde(default)]
    pub pins: Vec<PinAssignment>,
    /// Whether pin 1 carries a polarity/orientation marker.
    #[serde(default)]
    pub polarity_marker: bool,
}

/// An exposed thermal pad under a no-lead package.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub struct ThermalPad {
    /// Pad length (X) in mm.
    pub length: f64,
    /// Pad width (Y) in mm.
    pub width: f64,
}

/// IPC-7351 producibility level, controlling fillet (toe/heel/side) goals and
/// courtyard excess. Higher density → smaller lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub enum DensityLevel {
    /// IPC density level A — most land protrusion, lowest component density.
    Most,
    /// IPC density level B — nominal (the default).
    #[default]
    Nominal,
    /// IPC density level C — least land protrusion, highest density.
    Least,
}

/// The complete parametric description of a package — the single source of
/// truth from which footprint, symbol, and 3D body are derived.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub struct PackageClass {
    /// Stable identifier (e.g. "QFN-40_5x5mm_P0.4mm", "0603", "SOIC-8").
    pub id: String,
    /// Geometric family selecting the generator.
    pub family: PackageFamily,
    /// Body envelope.
    pub body: BodyEnvelope,
    /// Lead/terminal geometry.
    pub leads: LeadSpec,
    /// Exposed thermal pad, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-rs", ts(optional))]
    pub thermal_pad: Option<ThermalPad>,
    /// Producibility / density target.
    #[serde(default)]
    pub density: DensityLevel,
    /// Pin map.
    pub pin_map: PinMap,
}

// ============================================================================
// Derived geometry
// ============================================================================

/// IPC-7351 land-pattern fillet goals (mm) used to size pads from terminals.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub struct IpcGoals {
    /// Toe (outward) fillet goal.
    pub toe: f64,
    /// Heel (inward) fillet goal.
    pub heel: f64,
    /// Side fillet goal (may be negative at fine pitch to avoid bridging).
    pub side: f64,
    /// Courtyard excess beyond the maximum of body/land extents.
    pub courtyard_excess: f64,
}

/// A 3D body for a component, attached to its footprint so PCB↔enclosure
/// co-design works from real per-package geometry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub enum FootprintBody {
    /// An axis-aligned box body (the cheap default, sufficient for AABB-based
    /// interference/enclosure-fit).
    Box {
        /// Body extents in footprint-local coordinates (Z up).
        bbox: Box3D,
    },
    /// A cylindrical body (radial caps, TO-cans), centered at `center`.
    Cylinder {
        /// Center on the XY plane.
        center: Vec2,
        /// Radius in mm.
        radius: f64,
        /// Base Z in mm.
        z_min: f64,
        /// Top Z in mm.
        z_max: f64,
    },
}

impl FootprintBody {
    /// The XY/Z axis-aligned bounds of this body.
    pub fn aabb(&self) -> Box3D {
        match self {
            FootprintBody::Box { bbox } => *bbox,
            FootprintBody::Cylinder {
                center,
                radius,
                z_min,
                z_max,
            } => Box3D {
                min: Vec3::new(center.x - radius, center.y - radius, *z_min),
                max: Vec3::new(center.x + radius, center.y + radius, *z_max),
            },
        }
    }
}

/// The full result of deriving a package: footprint, symbol, body, courtyard,
/// and the IPC goals used — all from one [`PackageClass`] in one pass, so pad
/// numbers, symbol pin numbers, and body leads are bijective by construction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub struct DerivedPart {
    /// The PCB land pattern.
    pub footprint: FootprintTemplate,
    /// The schematic symbol (pins numbered identically to the footprint pads).
    pub symbol: SymbolDef,
    /// The 3D component body.
    pub body: FootprintBody,
    /// Assembly courtyard (encloses both body and lands plus excess).
    pub courtyard_aabb: Box3D,
    /// The IPC fillet goals applied.
    pub ipc: IpcGoals,
}
