//! Material-driven feeds and speeds.
//!
//! The native app had no material setting at all: on the first real cut the
//! feeds were typed by hand for "aluminium" and the plate turned out to be
//! copper (friction log item 54), and changing them meant editing five
//! operations one at a time (item 52). This module is the missing data: a
//! table of materials, a machine-rigidity derating, a spindle model that
//! knows a relay-switched router ignores the `S` word, and two entry points:
//!
//! * [`recommend`] — "what should I run?", with the arithmetic in the notes.
//! * [`check`] — "here is what I typed; what is wrong with it, and by how
//!   much?"
//!
//! # Conventions
//!
//! Units are mm and mm/min throughout; rpm is rev/min; surface speed is
//! m/min. Chipload means feed per tooth (`f_z`), in mm.
//!
//! # Honesty
//!
//! The target machine class is a hobby gantry router (an 800 W trim router on
//! an aluminium-extrusion gantry). Every number here is a *starting* value
//! chosen to survive, not to be fast, and the deratings that get it there are
//! reported in the notes so a machinist can disagree with a specific step
//! rather than with a black box. The anchor the table is calibrated against is
//! a cut that actually happened: 1 mm copper plate, Ø2 mm 2-flute carbide,
//! ~13 500 rpm, 250 mm/min, 0.17 mm passes, 40 mm/min plunge — clean profile,
//! cutter survived.
//!
//! # Example
//!
//! ```
//! use vcad_kernel_cam::materials::{
//!     check, ids, material, recommend, Machine, OpKind, Spindle, ToolKind, ToolSpec,
//! };
//!
//! let copper = material(ids::COPPER_C110).unwrap();
//! let tool = ToolSpec::new(2.0, 2, ToolKind::FlatEndMill, 6.0);
//! let rec = recommend(
//!     copper,
//!     &tool,
//!     OpKind::Slot,
//!     &Machine::anolex_ultra2(),
//!     &Spindle::router_dial(),
//! )
//! .unwrap();
//!
//! // Same neighbourhood as the cut that worked.
//! assert!(rec.feed_mm_min > 180.0 && rec.feed_mm_min < 350.0);
//! assert_eq!(rec.dial.as_deref(), Some("2"));
//! # let _ = check;
//! ```

mod data;

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::tool::Tool;
use crate::CamSettings;

/// Reference diameter the chipload table is quoted at: 1/8 inch, the size
/// every hobby chipload chart is written around.
pub const CHIPLOAD_REF_DIAMETER_MM: f64 = 3.175;

/// Smallest tool diameter the chipload curve is defined for (mm).
pub const MIN_TABLE_DIAMETER_MM: f64 = 0.5;

/// Largest tool diameter the chipload curve is defined for (mm).
pub const MAX_TABLE_DIAMETER_MM: f64 = 12.0;

/// Chipload as a fraction of the reference chipload, against tool diameter.
///
/// Manufacturer chipload charts (Onsrud 190-series router-bit charts, Harvey
/// Tool's miniature end-mill speeds-and-feeds pages, Amana's CNC chipload
/// chart) all publish one row per diameter, and the *ratio* between the rows
/// is close to material-independent: roughly linear in diameter over
/// 1/8"–1/2", and falling away faster than linear below 1/16" because a micro
/// tool's limit stops being chip evacuation and becomes the shank. Normalising
/// those rows at 1/8" gives this one curve, which reproduces
///
/// * Harvey's Ø0.76 mm aluminium row (0.0003" ≈ 0.0076 mm, i.e. 0.20 × the
///   1/8" value) — this curve interpolates 0.203 there, and
/// * Onsrud's 1/4" MDF row at ~1.9 × the 1/8" value — this curve is 1.72,
///   deliberately on the low side.
///
/// Values between knots are linearly interpolated; outside the range the
/// endpoint value is held and [`recommend`] says so in a note.
pub const CHIPLOAD_SHAPE: [(f64, f64); 6] = [
    (0.5, 0.12),
    (1.0, 0.28),
    (2.0, 0.66),
    (CHIPLOAD_REF_DIAMETER_MM, 1.00),
    (6.0, 1.72),
    (12.0, 2.80),
];

/// Above this diameter the "chipload must not exceed 1 % of diameter" rule is
/// not applied: it is a micro-end-mill rule about shank strength, not a
/// general one.
const MICRO_TOOL_LIMIT_MM: f64 = CHIPLOAD_REF_DIAMETER_MM;

/// The micro-tool chipload ceiling as a fraction of diameter. Widely published
/// for miniature carbide end mills (Harvey Tool, Performance Micro Tool): a
/// chip thicker than ~1 % of the cutter diameter snaps the neck before it
/// overloads the flute.
const MICRO_TOOL_CHIPLOAD_FRAC: f64 = 0.01;

/// Radial chip thinning is capped here. The formula runs away as the radial
/// engagement goes to zero, and a hobby router should not be asked to double
/// its feed twice over on the strength of an idealised chip model.
const MAX_CHIP_THINNING: f64 = 2.0;

/// Stepover used for a finishing pass, as a fraction of tool diameter.
const FINISH_STEPOVER_FRAC: f64 = 0.10;

/// Finish allowance is clamped into this band (mm) whatever the diameter
/// says: below the lower bound the finish pass rubs instead of cutting, above
/// the upper it is a second roughing pass.
const FINISH_ALLOWANCE_BOUNDS_MM: (f64, f64) = (0.05, 0.6);

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors from the feeds-and-speeds recommender.
///
/// These are the cases the recommender refuses rather than guesses at: a
/// silently plausible feed for a zero-diameter tool is exactly the failure
/// mode this module exists to remove.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum MaterialsError {
    /// The tool diameter is zero, negative or not finite.
    #[error("invalid tool diameter: {0} mm (must be finite and > 0)")]
    InvalidDiameter(f64),

    /// The tool has no cutting edges.
    #[error("invalid flute count: {0} (must be >= 1)")]
    InvalidFlutes(u8),

    /// The flute length is zero, negative or not finite.
    #[error("invalid flute length: {0} mm (must be finite and > 0)")]
    InvalidFluteLength(f64),

    /// The machine declares a non-positive feed or plunge ceiling.
    #[error("invalid machine limit: {0} (must be finite and > 0)")]
    InvalidMachineLimit(f64),

    /// The spindle has no usable speed: an empty dial table, or a controlled
    /// spindle whose range is empty or inverted.
    #[error("spindle has no usable speed range: {0}")]
    UnusableSpindle(String),

    /// No material in the table carries this id.
    #[error("unknown material id: {0}")]
    UnknownMaterial(String),
}

// ---------------------------------------------------------------------------
// Notes
// ---------------------------------------------------------------------------

/// How loudly a note should be shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum NoteLevel {
    /// Working shown, or a fact worth surfacing. Nothing is wrong.
    Info,
    /// Something to be aware of before pressing go.
    Caution,
    /// The cut will probably be poor, or the tool will wear fast.
    Warning,
    /// Expect a broken cutter, a ruined part, or a hazard to the operator.
    Danger,
}

/// A single piece of advice attached to a recommendation or a check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Note {
    /// How loudly to show it.
    pub level: NoteLevel,
    /// Human-readable text. Written for a machinist, not a compiler.
    pub text: String,
}

impl Note {
    /// Build a note.
    pub fn new(level: NoteLevel, text: impl Into<String>) -> Self {
        Self {
            level,
            text: text.into(),
        }
    }

    /// An informational note (usually the arithmetic).
    pub fn info(text: impl Into<String>) -> Self {
        Self::new(NoteLevel::Info, text)
    }

    /// A caution.
    pub fn caution(text: impl Into<String>) -> Self {
        Self::new(NoteLevel::Caution, text)
    }

    /// A warning.
    pub fn warning(text: impl Into<String>) -> Self {
        Self::new(NoteLevel::Warning, text)
    }

    /// A danger.
    pub fn danger(text: impl Into<String>) -> Self {
        Self::new(NoteLevel::Danger, text)
    }
}

/// A note that belongs to a material rather than to one recommendation:
/// hazards and standing guidance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaterialNote {
    /// How loudly to show it.
    pub level: NoteLevel,
    /// Human-readable text.
    pub text: String,
}

impl From<&MaterialNote> for Note {
    fn from(n: &MaterialNote) -> Self {
        Note {
            level: n.level,
            text: n.text.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Material
// ---------------------------------------------------------------------------

/// Broad material family. Drives the rules that are about the class of
/// material rather than the specific alloy (peck depth, drill feed per
/// revolution, whether the micro-tool chipload ceiling applies).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MaterialFamily {
    /// Wrought aluminium alloys.
    Aluminium,
    /// Copper and copper alloys (brass, bronze, pure copper).
    CopperAlloy,
    /// Carbon and low-alloy steels.
    Steel,
    /// Austenitic stainless steels.
    Stainless,
    /// Wood and wood-based sheet.
    Wood,
    /// Thermoplastics.
    Plastic,
    /// Fibre-reinforced composites and laminates.
    Composite,
}

impl MaterialFamily {
    /// Whether the micro-end-mill "chipload <= 1 % of diameter" ceiling
    /// applies. It is a metal/composite rule: in wood and plastic the limit is
    /// the machine's feed rate, not the cutter neck.
    pub fn applies_micro_chipload_cap(&self) -> bool {
        matches!(
            self,
            MaterialFamily::Aluminium
                | MaterialFamily::CopperAlloy
                | MaterialFamily::Steel
                | MaterialFamily::Stainless
                | MaterialFamily::Composite
        )
    }

    /// Peck depth for drilling, as a fraction of drill diameter. Half a
    /// diameter per peck in metal is the standard conservative figure; wood
    /// and plastic evacuate a full diameter without packing the flutes.
    pub fn peck_depth_frac(&self) -> f64 {
        match self {
            MaterialFamily::Wood | MaterialFamily::Plastic => 1.0,
            _ => 0.5,
        }
    }

    /// Drill feed per revolution, as a fraction of drill diameter. The usual
    /// small-drill rule of thumb is 0.01–0.02 × D per rev in metal; wood and
    /// plastic take two to three times that.
    pub fn drill_feed_per_rev_frac(&self) -> f64 {
        match self {
            MaterialFamily::Wood => 0.030,
            MaterialFamily::Plastic => 0.025,
            MaterialFamily::Composite => 0.010,
            _ => 0.012,
        }
    }

    /// Drills run slower than end mills at the same material: the cutting
    /// speed at the periphery is the whole speed, and the centre is rubbing
    /// regardless. This is the fraction of the milling surface speed to use.
    pub fn drill_surface_speed_frac(&self) -> f64 {
        0.6
    }
}

/// A surface-speed range for carbide tooling, in m/min.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SpeedRange {
    /// Lowest sensible cutting speed (m/min).
    pub min: f64,
    /// The speed [`recommend`] aims for (m/min).
    pub target: f64,
    /// Highest sensible cutting speed (m/min).
    pub max: f64,
}

/// How much help the cut needs to carry heat and chips away.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CoolantNeed {
    /// Cuts dry. Chip clearing only.
    Dry,
    /// Air blast to clear chips (and, in plastics, to stop them re-welding).
    AirBlast,
    /// Mist or a periodic brushed-on cutting fluid.
    Mist,
    /// Needs a lubricant on the cut; running dry welds the material to the
    /// flutes.
    Lubricant,
    /// Flood coolant. Not something a hobby router usually has.
    Flood,
}

/// How readily the material welds itself to the cutting edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChipWelding {
    /// Does not stick.
    Negligible,
    /// Sticks only when the cut goes dull or hot.
    Low,
    /// Built-up edge is likely without lubricant.
    Moderate,
    /// Welds to the flutes readily; lubricant or air is not optional.
    High,
}

/// One material in the table.
///
/// The `id` is stable and is what a document stores; `name` is for display and
/// may change. Values are starting points for carbide tooling.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Material {
    /// Stable identifier, e.g. `"aluminium-6061-t6"`. Never renamed.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Broad family.
    pub family: MaterialFamily,
    /// Coarse ordinal of how hard this is to machine on a *light* machine —
    /// not a hardness. Copper ranks above 7075 here because on a router its
    /// gumming, not its strength, is what ends the cut. Used for ordering in
    /// UIs and for the monotonicity checks.
    pub difficulty_rank: u16,
    /// Chipload at [`CHIPLOAD_REF_DIAMETER_MM`] for a rigid machine, mm/tooth.
    /// Scaled to other diameters by [`CHIPLOAD_SHAPE`] and derated by the
    /// machine class.
    pub chipload_ref_mm: f64,
    /// Below this chipload the edge rubs and burnishes instead of cutting:
    /// heat goes into the tool and the work instead of into the chip. Set
    /// around the edge hone of sharp carbide for metals, higher for fibrous
    /// materials that need a real bite.
    pub min_chipload_mm: f64,
    /// Surface-speed range for carbide, m/min.
    pub surface_speed: SpeedRange,
    /// Axial depth per pass when slotting (cutter buried), as a fraction of
    /// diameter, before the machine-class derating.
    pub slot_depth_frac: f64,
    /// Axial depth per pass when side-cutting, as a fraction of diameter,
    /// before the machine-class derating.
    pub profile_depth_frac: f64,
    /// Radial engagement for roughing, as a fraction of diameter, before the
    /// machine-class derating.
    pub radial_frac: f64,
    /// Plunge feed as a fraction of the cutting feed.
    pub plunge_frac: f64,
    /// Ramp angle for a ramped entry, degrees.
    pub ramp_angle_deg: f64,
    /// What the cut needs for cooling/lubrication.
    pub coolant: CoolantNeed,
    /// How readily it welds to the cutter.
    pub chip_welding: ChipWelding,
    /// Stock to leave for a finish pass, as a fraction of diameter.
    pub finish_allowance_frac: f64,
    /// Whether this material is a reasonable thing to cut on a router-class
    /// machine at all. `false` produces a warning on [`MachineClass::Hobby`].
    pub router_class_ok: bool,
    /// Hazards: dust, fume, fire, swarf. Always emitted by [`recommend`] and
    /// [`check`].
    pub hazards: Vec<MaterialNote>,
    /// Standing machining guidance for this material.
    pub guidance: Vec<MaterialNote>,
}

impl Material {
    /// Chipload for a given tool diameter, before any machine derating
    /// (mm/tooth). Diameters outside the table are clamped to its ends.
    pub fn chipload_at(&self, diameter_mm: f64) -> f64 {
        self.chipload_ref_mm * chipload_shape(diameter_mm)
    }

    /// Axial depth per pass for an operation, before the machine derating, as
    /// a fraction of diameter.
    pub fn depth_frac(&self, op: OpKind) -> f64 {
        match op {
            // A slot buries the cutter: both flutes cut, and the chips have
            // nowhere to go but up the slot.
            OpKind::Slot => self.slot_depth_frac,
            // A pocket steps over, so the radial engagement is a fraction of
            // the diameter and the axial depth can go deeper than a slot.
            OpKind::Pocket => self.slot_depth_frac * 1.5,
            OpKind::Profile | OpKind::Finish => self.profile_depth_frac,
            OpKind::Drill => self.family.peck_depth_frac(),
        }
    }
}

/// Interpolate [`CHIPLOAD_SHAPE`] at a diameter.
fn chipload_shape(diameter_mm: f64) -> f64 {
    let d = diameter_mm.clamp(MIN_TABLE_DIAMETER_MM, MAX_TABLE_DIAMETER_MM);
    let knots = &CHIPLOAD_SHAPE;
    if d <= knots[0].0 {
        return knots[0].1;
    }
    for w in knots.windows(2) {
        let (d0, f0) = w[0];
        let (d1, f1) = w[1];
        if d <= d1 {
            let t = (d - d0) / (d1 - d0);
            return f0 + t * (f1 - f0);
        }
    }
    knots[knots.len() - 1].1
}

/// Every material in the table, ordered by [`Material::difficulty_rank`]
/// (softest/easiest first). Suitable for a UI list.
pub fn materials() -> &'static [Material] {
    static TABLE: OnceLock<Vec<Material>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut v = data::build();
        v.sort_by_key(|m| m.difficulty_rank);
        v
    })
}

/// Look a material up by its stable id.
pub fn material(id: &str) -> Option<&'static Material> {
    materials().iter().find(|m| m.id == id)
}

/// Stable material ids. A document stores one of these strings; they are never
/// renamed.
pub mod ids {
    /// Aluminium 6061-T6.
    pub const ALUMINIUM_6061_T6: &str = "aluminium-6061-t6";
    /// Aluminium 7075-T6.
    pub const ALUMINIUM_7075_T6: &str = "aluminium-7075-t6";
    /// Free-machining brass C360.
    pub const BRASS_C360: &str = "brass-c360";
    /// Electrolytic tough-pitch copper C110.
    pub const COPPER_C110: &str = "copper-c110";
    /// Mild (low-carbon) steel.
    pub const MILD_STEEL: &str = "mild-steel";
    /// Austenitic stainless 304.
    pub const STAINLESS_304: &str = "stainless-304";
    /// Medium-density fibreboard.
    pub const MDF: &str = "mdf";
    /// Plywood.
    pub const PLYWOOD: &str = "plywood";
    /// Hardwood (oak, maple, ash).
    pub const HARDWOOD: &str = "hardwood";
    /// Softwood (pine, fir, cedar).
    pub const SOFTWOOD: &str = "softwood";
    /// Cast acrylic (PMMA).
    pub const ACRYLIC_CAST: &str = "acrylic-cast";
    /// Polycarbonate.
    pub const POLYCARBONATE: &str = "polycarbonate";
    /// POM / acetal (Delrin).
    pub const POM_ACETAL: &str = "pom-acetal";
    /// High-density polyethylene.
    pub const HDPE: &str = "hdpe";
    /// FR4 glass-epoxy laminate, copper-clad.
    pub const FR4_COPPER_CLAD: &str = "fr4-copper-clad";
    /// Cured carbon-fibre sheet.
    pub const CARBON_FIBRE_SHEET: &str = "carbon-fibre-sheet";
}

// ---------------------------------------------------------------------------
// Machine
// ---------------------------------------------------------------------------

/// How rigid the machine is. This is the single biggest lever on whether a
/// published chipload is a starting point or a broken cutter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MachineClass {
    /// Hobby gantry router: extruded-aluminium frame, belt or lead screws, a
    /// trim router in a clamp. Deflects visibly under load; chatter, not
    /// power, is the limit.
    Hobby,
    /// Benchtop mill or a heavy steel-frame router: cast or welded, ballscrews,
    /// a real spindle.
    Benchtop,
    /// Rigid VMC: cast iron, servo drives, a tool changer.
    Rigid,
}

impl MachineClass {
    /// Chipload derating. The hobby figure is set so that the table's
    /// published values land back on the hobby-router guidance everybody
    /// actually quotes — 0.001"–0.002" per tooth at 1/8" in aluminium — and so
    /// that the copper anchor cut reproduces.
    pub fn chipload_factor(&self) -> f64 {
        match self {
            MachineClass::Hobby => 0.60,
            MachineClass::Benchtop => 0.85,
            MachineClass::Rigid => 1.00,
        }
    }

    /// Axial depth derating. Halving the table on a hobby router turns the
    /// aluminium slot figure into ~0.12 × D (the commonly quoted 0.1 × D for
    /// small cutters) and the wood figure into D/2.
    pub fn depth_factor(&self) -> f64 {
        match self {
            MachineClass::Hobby => 0.50,
            MachineClass::Benchtop => 0.80,
            MachineClass::Rigid => 1.00,
        }
    }

    /// Radial-engagement derating.
    pub fn radial_factor(&self) -> f64 {
        match self {
            MachineClass::Hobby => 0.70,
            MachineClass::Benchtop => 0.90,
            MachineClass::Rigid => 1.00,
        }
    }

    /// Display name used in the notes.
    pub fn label(&self) -> &'static str {
        match self {
            MachineClass::Hobby => "hobby gantry router",
            MachineClass::Benchtop => "benchtop mill",
            MachineClass::Rigid => "rigid VMC",
        }
    }
}

/// A machine: its rigidity class and its motion limits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Machine {
    /// Display name.
    pub name: String,
    /// Rigidity class.
    pub class: MachineClass,
    /// Maximum cutting feed the machine will accept (mm/min). Read this off
    /// the controller (`$110`/`$111` on Grbl) rather than trusting the
    /// default.
    pub max_feed_mm_min: f64,
    /// Maximum plunge feed (mm/min). Usually the Z axis (`$112`) and usually
    /// much lower than XY.
    pub max_plunge_mm_min: f64,
}

impl Machine {
    /// A generic hobby gantry router.
    pub fn hobby_router() -> Self {
        Self {
            name: "hobby gantry router".into(),
            class: MachineClass::Hobby,
            // Deliberately below typical Grbl `$110` rapids: this is a cutting
            // ceiling, not a traverse rate. Override from the controller.
            max_feed_mm_min: 4000.0,
            max_plunge_mm_min: 800.0,
        }
    }

    /// The AnoleX 4030-Evo Ultra 2 as it was configured for the 2026-09-17
    /// cut: Grbl_ESP32, 800 W trim router on a relay, `$30 = 10000`.
    ///
    /// The feed ceilings are conservative defaults, not readings from that
    /// machine's `$$` dump — override them from `$110`/`$111`/`$112` once the
    /// machine profile exists.
    pub fn anolex_ultra2() -> Self {
        Self {
            name: "AnoleX 4030-Evo Ultra 2".into(),
            class: MachineClass::Hobby,
            // `$110`/`$111`/`$112` = 3000 on the machine (read 2026-09-17).
            max_feed_mm_min: 3000.0,
            max_plunge_mm_min: 800.0,
        }
    }

    /// A benchtop mill.
    pub fn benchtop_mill() -> Self {
        Self {
            name: "benchtop mill".into(),
            class: MachineClass::Benchtop,
            max_feed_mm_min: 3000.0,
            max_plunge_mm_min: 1000.0,
        }
    }

    /// A rigid VMC.
    pub fn rigid_vmc() -> Self {
        Self {
            name: "rigid VMC".into(),
            class: MachineClass::Rigid,
            max_feed_mm_min: 10000.0,
            max_plunge_mm_min: 5000.0,
        }
    }

    fn validate(&self) -> Result<(), MaterialsError> {
        if !self.max_feed_mm_min.is_finite() || self.max_feed_mm_min <= 0.0 {
            return Err(MaterialsError::InvalidMachineLimit(self.max_feed_mm_min));
        }
        if !self.max_plunge_mm_min.is_finite() || self.max_plunge_mm_min <= 0.0 {
            return Err(MaterialsError::InvalidMachineLimit(self.max_plunge_mm_min));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Spindle
// ---------------------------------------------------------------------------

/// One position on a router's speed dial.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DialSetting {
    /// What is printed on the dial, e.g. `"2"`.
    pub label: String,
    /// Approximate no-load speed at that position (rpm).
    pub rpm: f64,
}

impl DialSetting {
    /// Build a dial setting.
    pub fn new(label: impl Into<String>, rpm: f64) -> Self {
        Self {
            label: label.into(),
            rpm,
        }
    }
}

/// The spindle, and specifically whether the `S` word means anything.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Spindle {
    /// A spindle whose speed the controller commands with `S`.
    Controlled {
        /// Lowest commandable speed (rpm).
        min_rpm: f64,
        /// Highest commandable speed (rpm).
        max_rpm: f64,
    },
    /// A trim router switched on and off by a relay, with a manual speed dial.
    /// `M3 S…` starts it; the `S` value is discarded, and the actual speed is
    /// whatever the dial was left at.
    Dial {
        /// Dial positions, in any order.
        settings: Vec<DialSetting>,
    },
}

impl Spindle {
    /// The speed dial fitted to Makita-pattern trim routers (RT0700C/RT0701C
    /// and the AnoleX-supplied clones): six positions from ~10 000 to
    /// ~30 000 rpm no-load.
    ///
    /// Position 2 is listed at 13 500 rpm, which is the value observed on the
    /// AnoleX Ultra 2 during the 2026-09-17 cut; Makita's own manual prints
    /// 13 000 for that position. These are no-load figures and every one of
    /// them droops under load — treat the table as user-overridable data, not
    /// as a calibration.
    pub fn router_dial() -> Self {
        Spindle::Dial {
            settings: vec![
                DialSetting::new("1", 10_000.0),
                DialSetting::new("2", 13_500.0),
                DialSetting::new("3", 17_000.0),
                DialSetting::new("4", 21_000.0),
                DialSetting::new("5", 25_000.0),
                DialSetting::new("6", 30_000.0),
            ],
        }
    }

    /// A typical VFD spindle.
    pub fn vfd_spindle() -> Self {
        Spindle::Controlled {
            min_rpm: 6_000.0,
            max_rpm: 24_000.0,
        }
    }

    /// Whether the `S` word in the G-code actually sets the speed.
    pub fn honours_s_word(&self) -> bool {
        matches!(self, Spindle::Controlled { .. })
    }

    /// Lowest speed available.
    pub fn min_rpm(&self) -> Option<f64> {
        match self {
            Spindle::Controlled { min_rpm, .. } => Some(*min_rpm),
            Spindle::Dial { settings } => settings
                .iter()
                .map(|s| s.rpm)
                .fold(None, |a: Option<f64>, r| Some(a.map_or(r, |a| a.min(r)))),
        }
    }

    /// Highest speed available.
    pub fn max_rpm(&self) -> Option<f64> {
        match self {
            Spindle::Controlled { max_rpm, .. } => Some(*max_rpm),
            Spindle::Dial { settings } => settings
                .iter()
                .map(|s| s.rpm)
                .fold(None, |a: Option<f64>, r| Some(a.map_or(r, |a| a.max(r)))),
        }
    }

    fn validate(&self) -> Result<(), MaterialsError> {
        match self {
            Spindle::Controlled { min_rpm, max_rpm } => {
                if !min_rpm.is_finite() || !max_rpm.is_finite() || *min_rpm <= 0.0 {
                    return Err(MaterialsError::UnusableSpindle(format!(
                        "min {min_rpm} max {max_rpm}"
                    )));
                }
                if max_rpm < min_rpm {
                    return Err(MaterialsError::UnusableSpindle(format!(
                        "max {max_rpm} rpm is below min {min_rpm} rpm"
                    )));
                }
                Ok(())
            }
            Spindle::Dial { settings } => {
                if settings.is_empty() {
                    return Err(MaterialsError::UnusableSpindle("empty dial table".into()));
                }
                if settings.iter().any(|s| !s.rpm.is_finite() || s.rpm <= 0.0) {
                    return Err(MaterialsError::UnusableSpindle(
                        "a dial position has a non-positive rpm".into(),
                    ));
                }
                Ok(())
            }
        }
    }

    /// Resolve a wanted speed to one the spindle can actually deliver.
    ///
    /// A controlled spindle clamps into its range. A dial spindle clamps into
    /// the range of its positions and then picks the *nearest* position; ties
    /// go to the slower one.
    pub fn resolve(&self, target_rpm: f64) -> SpindleSetting {
        let (lo, hi) = (
            self.min_rpm().unwrap_or(target_rpm),
            self.max_rpm().unwrap_or(target_rpm),
        );
        let clamped_low = target_rpm < lo;
        let clamped_high = target_rpm > hi;
        let wanted = target_rpm.clamp(lo, hi);

        match self {
            Spindle::Controlled { .. } => SpindleSetting {
                rpm: wanted,
                dial: None,
                clamped_low,
                clamped_high,
            },
            Spindle::Dial { settings } => {
                let mut best: Option<&DialSetting> = None;
                for s in settings {
                    best = match best {
                        None => Some(s),
                        Some(b) => {
                            let (db, ds) = ((b.rpm - wanted).abs(), (s.rpm - wanted).abs());
                            // Ties go to the slower position: on a router the
                            // slower one is the one that does not burn.
                            if ds < db || (ds == db && s.rpm < b.rpm) {
                                Some(s)
                            } else {
                                Some(b)
                            }
                        }
                    };
                }
                let best = best.expect("validated non-empty");
                SpindleSetting {
                    rpm: best.rpm,
                    dial: Some(best.label.clone()),
                    clamped_low,
                    clamped_high,
                }
            }
        }
    }

    /// Resolve to the fastest setting that does not exceed `target_rpm`,
    /// falling back to the slowest available. Used when the speed has to come
    /// *down* to keep the chipload above the rubbing floor.
    pub fn resolve_at_most(&self, target_rpm: f64) -> SpindleSetting {
        match self {
            Spindle::Controlled { min_rpm, max_rpm } => {
                let rpm = target_rpm.clamp(*min_rpm, *max_rpm);
                SpindleSetting {
                    rpm,
                    dial: None,
                    clamped_low: target_rpm < *min_rpm,
                    clamped_high: target_rpm > *max_rpm,
                }
            }
            Spindle::Dial { settings } => {
                let mut best: Option<&DialSetting> = None;
                for s in settings {
                    if s.rpm <= target_rpm && best.is_none_or(|b| s.rpm > b.rpm) {
                        best = Some(s);
                    }
                }
                match best {
                    Some(s) => SpindleSetting {
                        rpm: s.rpm,
                        dial: Some(s.label.clone()),
                        clamped_low: false,
                        clamped_high: false,
                    },
                    None => {
                        // Everything on the dial is faster than we want.
                        let slowest = settings
                            .iter()
                            .min_by(|a, b| a.rpm.total_cmp(&b.rpm))
                            .expect("validated non-empty");
                        SpindleSetting {
                            rpm: slowest.rpm,
                            dial: Some(slowest.label.clone()),
                            clamped_low: true,
                            clamped_high: false,
                        }
                    }
                }
            }
        }
    }
}

/// A resolved spindle speed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpindleSetting {
    /// The speed that will actually be used in the arithmetic (rpm).
    pub rpm: f64,
    /// Which dial position to set by hand, if this is a dial spindle.
    pub dial: Option<String>,
    /// The wanted speed was below what the spindle can do.
    pub clamped_low: bool,
    /// The wanted speed was above what the spindle can do.
    pub clamped_high: bool,
}

// ---------------------------------------------------------------------------
// Tool + operation
// ---------------------------------------------------------------------------

/// What kind of cutter this is. Kept separate from [`Tool`] on purpose: this
/// module takes the cutter's numbers as plain parameters so it does not have
/// to match on a tool enum that other work is extending.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolKind {
    /// Flat end mill.
    FlatEndMill,
    /// Ball end mill.
    BallEndMill,
    /// Bull (corner-radius) end mill.
    BullEndMill,
    /// V-bit / engraver.
    VBit,
    /// Twist drill.
    Drill,
    /// Face mill with inserts.
    FaceMill,
}

/// The cutter's numbers, as this module needs them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    /// Cutting diameter (mm).
    pub diameter_mm: f64,
    /// Number of cutting edges.
    pub flutes: u8,
    /// Kind of cutter.
    pub kind: ToolKind,
    /// Usable flute (cutting) length (mm).
    pub flute_length_mm: f64,
}

impl ToolSpec {
    /// Build a tool spec.
    pub fn new(diameter_mm: f64, flutes: u8, kind: ToolKind, flute_length_mm: f64) -> Self {
        Self {
            diameter_mm,
            flutes,
            kind,
            flute_length_mm,
        }
    }

    /// Build a tool spec from a [`Tool`] using only its accessors, plus the
    /// kind, which the caller states. Tools with no flute length (drills,
    /// V-bits, face mills) fall back to three diameters of usable length.
    pub fn from_tool(tool: &Tool, kind: ToolKind) -> Self {
        let diameter_mm = tool.diameter();
        Self {
            diameter_mm,
            flutes: tool.flutes(),
            kind,
            flute_length_mm: tool.max_depth().unwrap_or(diameter_mm * 3.0),
        }
    }

    fn validate(&self) -> Result<(), MaterialsError> {
        if !self.diameter_mm.is_finite() || self.diameter_mm <= 0.0 {
            return Err(MaterialsError::InvalidDiameter(self.diameter_mm));
        }
        if self.flutes == 0 {
            return Err(MaterialsError::InvalidFlutes(self.flutes));
        }
        if !self.flute_length_mm.is_finite() || self.flute_length_mm <= 0.0 {
            return Err(MaterialsError::InvalidFluteLength(self.flute_length_mm));
        }
        Ok(())
    }
}

/// What the operation is doing to the material, which is what sets the
/// engagement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpKind {
    /// The cutter is buried: material on both sides, chips only escape
    /// upwards. **A profile that cuts right through sheet stock to free a part
    /// is a `Slot`, not a [`OpKind::Profile`]** — there is material on both
    /// sides of the cutter the whole way round.
    Slot,
    /// A side cut with material on one side only: the waste beside the cut has
    /// already gone, or the cutter leads in from outside the blank.
    Profile,
    /// Clearing an area by stepping over.
    Pocket,
    /// Drilling or pecking a hole.
    Drill,
    /// A light finishing pass taking the allowance off a wall.
    Finish,
}

impl OpKind {
    /// Display name used in the notes.
    pub fn label(&self) -> &'static str {
        match self {
            OpKind::Slot => "slot",
            OpKind::Profile => "profile",
            OpKind::Pocket => "pocket",
            OpKind::Drill => "drill",
            OpKind::Finish => "finish",
        }
    }
}

// ---------------------------------------------------------------------------
// Recommendation
// ---------------------------------------------------------------------------

/// A complete set of starting numbers for one operation, with the arithmetic
/// that produced them in [`Recommendation::notes`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Recommendation {
    /// Stable id of the material this is for.
    pub material_id: String,
    /// The operation.
    pub op: OpKind,
    /// Cutter diameter used in the arithmetic (mm).
    pub tool_diameter_mm: f64,
    /// Flute count used in the arithmetic.
    pub flutes: u8,
    /// Spindle speed the feed assumes (rpm).
    pub rpm: f64,
    /// Which dial position to set by hand, on a dial spindle. When this is
    /// `Some`, the `S` word in the G-code does nothing.
    pub dial: Option<String>,
    /// Surface speed actually achieved at `rpm` (m/min).
    pub surface_speed_m_min: f64,
    /// Feed per tooth the feed is built from (mm).
    pub chipload_mm: f64,
    /// Cutting feed (mm/min).
    pub feed_mm_min: f64,
    /// Plunge feed (mm/min).
    pub plunge_mm_min: f64,
    /// Ramp angle for a ramped entry (degrees).
    pub ramp_angle_deg: f64,
    /// Axial depth per pass (mm).
    pub stepdown_mm: f64,
    /// Radial engagement per pass (mm). Zero for drilling.
    pub stepover_mm: f64,
    /// Stock to leave for a later finish pass (mm). Zero for a finish pass or
    /// a drill.
    pub finish_allowance_mm: f64,
    /// What the cut needs for cooling/lubrication.
    pub coolant: CoolantNeed,
    /// Working, cautions, warnings and hazards.
    pub notes: Vec<Note>,
}

impl Recommendation {
    /// Copy the recommendation into a [`CamSettings`], leaving the clearance
    /// heights alone. This is the "apply to all operations" button behind
    /// friction-log item 52.
    pub fn apply_to(&self, settings: &mut CamSettings) {
        settings.feed_rate = self.feed_mm_min;
        settings.plunge_rate = self.plunge_mm_min;
        settings.spindle_rpm = self.rpm;
        settings.stepdown = self.stepdown_mm;
        if self.stepover_mm > 0.0 {
            settings.stepover = self.stepover_mm;
        }
    }

    /// The worst note level present, if any.
    pub fn worst_level(&self) -> Option<NoteLevel> {
        self.notes.iter().map(|n| n.level).max()
    }
}

// ---------------------------------------------------------------------------
// recommend
// ---------------------------------------------------------------------------

/// Radial chip thinning factor for a radial engagement `ae` on a cutter of
/// diameter `d`.
///
/// Below half-diameter engagement the maximum chip a tooth takes is thinner
/// than the feed per tooth, by `sqrt(1 - (1 - 2·ae/d)²)`; feeding faster by the
/// reciprocal restores the chip. Capped at [`MAX_CHIP_THINNING`].
fn chip_thinning_factor(ae: f64, d: f64) -> f64 {
    if d <= 0.0 || ae <= 0.0 {
        return 1.0;
    }
    let a = (ae / d).clamp(0.0, 1.0);
    if a >= 0.5 {
        return 1.0;
    }
    let s = 1.0 - (1.0 - 2.0 * a).powi(2);
    if s <= 0.0 {
        return MAX_CHIP_THINNING;
    }
    (1.0 / s.sqrt()).min(MAX_CHIP_THINNING)
}

/// Recommend feeds and speeds for one operation.
///
/// The arithmetic is, in order:
///
/// 1. rpm from the material's target surface speed and the cutter diameter,
///    resolved through the spindle (nearest dial position on a router).
/// 2. chipload from the table at that diameter, times the machine-class
///    derating, times the radial chip-thinning factor, capped for micro tools
///    and floored at the material's rubbing threshold.
/// 3. `feed = chipload × flutes × rpm`, clamped to the machine's feed ceiling.
/// 4. If the clamp pushed the chipload back under the rubbing floor, the rpm
///    comes *down* until it does not.
///
/// Every step that moved a number leaves a note saying so.
pub fn recommend(
    material: &Material,
    tool: &ToolSpec,
    op: OpKind,
    machine: &Machine,
    spindle: &Spindle,
) -> Result<Recommendation, MaterialsError> {
    tool.validate()?;
    machine.validate()?;
    spindle.validate()?;

    let mut notes: Vec<Note> = Vec::new();
    let d = tool.diameter_mm;

    if d < MIN_TABLE_DIAMETER_MM || d > MAX_TABLE_DIAMETER_MM {
        notes.push(Note::caution(format!(
            "Ø{d:.3} mm is outside the {MIN_TABLE_DIAMETER_MM}–{MAX_TABLE_DIAMETER_MM} mm chipload table; the nearest end of the table was used. Treat this as a guess and take a test cut."
        )));
    }

    if !material.router_class_ok && machine.class == MachineClass::Hobby {
        notes.push(Note::warning(format!(
            "{} is not a router-class material: a {} has neither the rigidity nor the low-speed torque for it. Expect work hardening, chatter and short tool life; these numbers keep the cutter alive, they do not make the job sensible.",
            material.name,
            machine.class.label()
        )));
    }

    // --- 1. rpm -----------------------------------------------------------
    let speed_frac = if op == OpKind::Drill {
        material.family.drill_surface_speed_frac()
    } else {
        1.0
    };
    let target_vc = material.surface_speed.target * speed_frac;
    let ideal_rpm = 1000.0 * target_vc / (std::f64::consts::PI * d);
    let mut setting = spindle.resolve(ideal_rpm);

    notes.push(Note::info(format!(
        "rpm: {target_vc:.0} m/min ÷ (π × Ø{d:.3} mm) × 1000 = {ideal_rpm:.0} rpm → {} {:.0} rpm",
        match &setting.dial {
            Some(l) => format!("dial {l},"),
            None => "spindle set to".into(),
        },
        setting.rpm
    )));
    if op == OpKind::Drill {
        notes.push(Note::info(format!(
            "drilling runs at {:.0} % of the milling surface speed ({:.0} of {:.0} m/min): the drill's centre cannot cut at any speed.",
            speed_frac * 100.0,
            target_vc,
            material.surface_speed.target
        )));
    }

    let mut achieved_vc = std::f64::consts::PI * d * setting.rpm / 1000.0;
    if setting.clamped_high {
        notes.push(Note::caution(format!(
            "the spindle tops out at {:.0} rpm, so Ø{d:.3} mm only reaches {achieved_vc:.0} m/min against the {:.0} m/min wanted. The cut will be slower than ideal, not unsafe.",
            setting.rpm, target_vc
        )));
    }
    if achieved_vc > material.surface_speed.max * speed_frac {
        notes.push(Note::warning(format!(
            "the slowest speed available turns Ø{d:.3} mm at {achieved_vc:.0} m/min, above the {:.0} m/min ceiling for {}. Expect fast wear or burning; use a smaller cutter, or a spindle that goes slower.",
            material.surface_speed.max * speed_frac,
            material.name
        )));
    } else if setting.clamped_low {
        notes.push(Note::caution(format!(
            "the spindle will not go below {:.0} rpm, so Ø{d:.3} mm runs at {achieved_vc:.0} m/min against the {target_vc:.0} m/min wanted. Still inside the material's band, but the cut is hotter than it needs to be.",
            setting.rpm
        )));
    }

    // --- 2. chipload ------------------------------------------------------
    let base_chipload = material.chipload_at(d);
    let derate = machine.class.chipload_factor();
    let mut chipload = base_chipload * derate;
    notes.push(Note::info(format!(
        "chipload: {base_chipload:.4} mm/tooth (table, Ø{d:.3} mm) × {derate:.2} ({}) = {chipload:.4} mm/tooth",
        machine.class.label()
    )));

    // Radial engagement, which sets both the stepover and the chip thinning.
    let radial_frac = material.radial_frac * machine.class.radial_factor();
    let ae = match op {
        OpKind::Slot => d,
        OpKind::Profile | OpKind::Pocket => radial_frac * d,
        OpKind::Finish => FINISH_STEPOVER_FRAC * d,
        OpKind::Drill => 0.0,
    };

    if op == OpKind::Drill {
        // A drill is fed per revolution, not per tooth: the usual small-drill
        // rule of thumb is a fixed fraction of the drill diameter per turn.
        let fpr = material.family.drill_feed_per_rev_frac() * d;
        chipload = fpr / f64::from(tool.flutes);
        notes.push(Note::info(format!(
            "drilling is fed per revolution: {:.3} × Ø{d:.3} mm = {fpr:.4} mm/rev, i.e. {chipload:.4} mm per lip over {} lips.",
            material.family.drill_feed_per_rev_frac(),
            tool.flutes
        )));
    } else {
        let thinning = chip_thinning_factor(ae, d);
        if thinning > 1.0 {
            let before = chipload;
            chipload *= thinning;
            notes.push(Note::info(format!(
                "chip thinning: the radial engagement is {:.0} % of diameter ({ae:.3} mm), so each tooth takes a chip thinner than its feed; feed per tooth raised ×{thinning:.3} to {chipload:.4} mm/tooth (was {before:.4}).",
                100.0 * ae / d
            )));
        }
    }

    // Micro-tool ceiling and rubbing floor. When they cross, the cutter is
    // simply the wrong size for the material and we say so.
    let micro_cap = if d <= MICRO_TOOL_LIMIT_MM && material.family.applies_micro_chipload_cap() {
        Some(MICRO_TOOL_CHIPLOAD_FRAC * d)
    } else {
        None
    };
    let floor = material.min_chipload_mm;

    if let Some(cap) = micro_cap {
        if floor > cap {
            notes.push(Note::warning(format!(
                "Ø{d:.3} mm is the wrong cutter for {}: the chipload that stops it rubbing ({floor:.4} mm/tooth) is above the {:.0} %-of-diameter limit that keeps a cutter this small from snapping ({cap:.4} mm/tooth). Held at the deflection limit, so the cut will burnish and run hot — use a larger cutter.",
                material.name,
                MICRO_TOOL_CHIPLOAD_FRAC * 100.0
            )));
        }
        if chipload > cap {
            notes.push(Note::caution(format!(
                "chipload capped at {cap:.4} mm/tooth ({:.0} % of Ø{d:.3} mm): a chip thicker than that breaks the neck of a cutter this size before it overloads the flute.",
                MICRO_TOOL_CHIPLOAD_FRAC * 100.0
            )));
            chipload = cap;
        }
    }

    let cap_value = micro_cap.unwrap_or(f64::INFINITY);
    if chipload < floor {
        let raised = floor.min(cap_value);
        if raised > chipload {
            notes.push(Note::caution(format!(
                "chipload raised from {chipload:.4} to {raised:.4} mm/tooth: below {floor:.4} the edge rubs {} instead of cutting it, and the heat goes into the tool.",
                material.name
            )));
            chipload = raised;
        }
    }

    // --- 3. feed ----------------------------------------------------------
    let flutes = f64::from(tool.flutes);
    let mut feed = chipload * flutes * setting.rpm;
    notes.push(Note::info(format!(
        "feed = {chipload:.4} mm/{} × {} {} × {:.0} rpm = {feed:.0} mm/min",
        if op == OpKind::Drill { "lip" } else { "tooth" },
        tool.flutes,
        if op == OpKind::Drill {
            "lips"
        } else {
            "flutes"
        },
        setting.rpm
    )));

    // A drill only ever moves in Z, so its ceiling is the Z axis, not XY.
    let feed_ceiling = if op == OpKind::Drill {
        machine.max_plunge_mm_min
    } else {
        machine.max_feed_mm_min
    };

    if feed > feed_ceiling {
        let capped = feed_ceiling;
        notes.push(Note::caution(format!(
            "feed clamped to the machine's {capped:.0} mm/min ceiling (wanted {feed:.0})."
        )));
        feed = capped;
        chipload = feed / (flutes * setting.rpm);

        if chipload < floor {
            // Slowing the spindle is the only way back above the floor once
            // the feed is pinned. This is the "never recommend a feed that
            // rubs" rule.
            let rpm_needed = feed / (flutes * floor);
            let slower = spindle.resolve_at_most(rpm_needed);
            let new_chipload = feed / (flutes * slower.rpm);
            let dial = match &slower.dial {
                Some(l) => format!("dial {l}, "),
                None => String::new(),
            };
            // `resolve_at_most` cannot always get there: a spindle with a
            // minimum, or a dial whose slowest position is still faster than
            // the speed this needs, comes back `clamped_low` and the chipload
            // is *still* under the floor. Saying "so each tooth takes
            // 0.0075 mm" when the floor is 0.02 reads as a fix; it is not one.
            if slower.clamped_low || new_chipload < floor {
                notes.push(Note::danger(format!(
                    "at {:.0} rpm the clamped feed is only {chipload:.4} mm/tooth, below the {floor:.4} rubbing floor, and this spindle will not go slower than {}{:.0} rpm — at which each tooth still takes only {new_chipload:.4} mm. There is no feed and speed here that cuts instead of rubbing: use a cutter with fewer flutes, a smaller diameter, or a machine that will feed faster than {feed:.0} mm/min.",
                    setting.rpm, dial, slower.rpm
                )));
            } else {
                notes.push(Note::warning(format!(
                    "at {:.0} rpm the clamped feed is only {chipload:.4} mm/tooth, below the {floor:.4} rubbing floor. Speed dropped to {}{:.0} rpm so each tooth takes {new_chipload:.4} mm — or keep the speed and use a cutter with fewer flutes.",
                    setting.rpm, dial, slower.rpm
                )));
            }
            setting = slower;
            chipload = new_chipload;
            achieved_vc = std::f64::consts::PI * d * setting.rpm / 1000.0;
        }
    }

    // --- 4. plunge, ramp --------------------------------------------------
    let mut plunge = if op == OpKind::Drill {
        // Drilling *is* plunging.
        feed
    } else {
        feed * material.plunge_frac
    };
    if op == OpKind::Drill {
        notes.push(Note::info(
            "plunge = the drilling feed: the whole cut is the Z move.".to_string(),
        ));
    } else {
        notes.push(Note::info(format!(
            "plunge = {feed:.0} mm/min × {:.2} = {plunge:.0} mm/min",
            material.plunge_frac
        )));
    }
    if plunge > machine.max_plunge_mm_min {
        notes.push(Note::caution(format!(
            "plunge clamped to the machine's {:.0} mm/min Z ceiling (wanted {plunge:.0}).",
            machine.max_plunge_mm_min
        )));
        plunge = machine.max_plunge_mm_min;
    }
    if op != OpKind::Drill {
        notes.push(Note::info(format!(
            "prefer a {:.1}° ramp or a helix over a straight plunge: a flat end mill has no cutting edge at its centre.",
            material.ramp_angle_deg
        )));
    }

    // --- 5. depths --------------------------------------------------------
    let depth_frac = material.depth_frac(op);
    let depth_derate = machine.class.depth_factor();
    let mut stepdown = depth_frac * d * depth_derate;
    notes.push(Note::info(format!(
        "{}: {depth_frac:.2} × Ø{d:.3} mm × {depth_derate:.2} ({}) = {stepdown:.3} mm per pass",
        if op == OpKind::Drill {
            "peck depth"
        } else {
            "stepdown"
        },
        machine.class.label()
    )));
    if stepdown > tool.flute_length_mm {
        notes.push(Note::caution(format!(
            "stepdown reduced to the {:.3} mm flute length.",
            tool.flute_length_mm
        )));
        stepdown = tool.flute_length_mm;
    }
    stepdown = stepdown.max(0.02);

    let stepover = ae;
    if op == OpKind::Slot {
        notes.push(Note::caution(format!(
            "this is a buried cut: the cutter is engaged over its full Ø{d:.3} mm and the chips only leave upwards. If the waste beside the line has already been cleared, use a profile instead and the numbers get friendlier."
        )));
    }

    let finish_allowance = match op {
        OpKind::Finish | OpKind::Drill => 0.0,
        _ => (material.finish_allowance_frac * d)
            .clamp(FINISH_ALLOWANCE_BOUNDS_MM.0, FINISH_ALLOWANCE_BOUNDS_MM.1),
    };

    // --- 6. standing material advice -------------------------------------
    match material.chip_welding {
        ChipWelding::High => notes.push(Note::caution(format!(
            "{} welds to the cutting edge readily. Keep the chip moving, clear it between passes, and use {}.",
            material.name,
            coolant_phrase(material.coolant)
        ))),
        ChipWelding::Moderate => notes.push(Note::info(format!(
            "built-up edge is likely on {} without {}.",
            material.name,
            coolant_phrase(material.coolant)
        ))),
        ChipWelding::Low | ChipWelding::Negligible => {}
    }
    notes.extend(material.guidance.iter().map(Note::from));
    notes.extend(material.hazards.iter().map(Note::from));

    if let Some(dial) = &setting.dial {
        notes.push(Note::warning(format!(
            "this spindle has no speed control from the controller: the S word in the G-code is discarded and only the relay is switched. Set the router dial to {dial} (≈{:.0} rpm) by hand — every feed above assumes that speed.",
            setting.rpm
        )));
    }

    if tool.kind == ToolKind::Drill && op != OpKind::Drill {
        notes.push(Note::warning(
            "a twist drill cannot cut sideways: this operation moves in XY.".to_string(),
        ));
    }
    if tool.kind != ToolKind::Drill && op == OpKind::Drill {
        notes.push(Note::caution(
            "drilling with an end mill: peck, and expect a hole a little over size and a flat bottom.".to_string(),
        ));
    }

    Ok(Recommendation {
        material_id: material.id.clone(),
        op,
        tool_diameter_mm: d,
        flutes: tool.flutes,
        rpm: setting.rpm,
        dial: setting.dial,
        surface_speed_m_min: achieved_vc,
        chipload_mm: chipload,
        feed_mm_min: feed,
        plunge_mm_min: plunge,
        ramp_angle_deg: material.ramp_angle_deg,
        stepdown_mm: stepdown,
        stepover_mm: stepover,
        finish_allowance_mm: finish_allowance,
        coolant: material.coolant,
        notes,
    })
}

fn coolant_phrase(c: CoolantNeed) -> &'static str {
    match c {
        CoolantNeed::Dry => "no coolant (keep it dry and clear the chips)",
        CoolantNeed::AirBlast => "an air blast",
        CoolantNeed::Mist => "mist or a brushed-on cutting fluid",
        CoolantNeed::Lubricant => "a cutting lubricant (paste, WD-40, kerosene)",
        CoolantNeed::Flood => "flood coolant",
    }
}

// ---------------------------------------------------------------------------
// check
// ---------------------------------------------------------------------------

/// Chipload below this multiple of the recommendation reads as rubbing.
const CHECK_LOW_FACTOR: f64 = 0.5;
/// Chipload above this multiple of the recommendation reads as overloaded.
const CHECK_HIGH_FACTOR: f64 = 1.6;

/// Audit settings a user typed against the material, tool, operation and
/// machine, and say what is off and by how much.
///
/// This never changes anything: it returns notes. An empty return, apart from
/// hazards, means nothing stood out.
///
/// Takes the spindle in addition to the machine because the single most
/// expensive misunderstanding on a router-class machine is that the `S` word
/// does something.
pub fn check(
    settings: &CamSettings,
    material: &Material,
    tool: &ToolSpec,
    op: OpKind,
    machine: &Machine,
    spindle: &Spindle,
) -> Vec<Note> {
    let mut notes = Vec::new();

    if let Err(e) = tool.validate() {
        notes.push(Note::danger(format!("tool is not usable: {e}")));
        return notes;
    }
    let d = tool.diameter_mm;
    let flutes = f64::from(tool.flutes);

    if !material.router_class_ok && machine.class == MachineClass::Hobby {
        notes.push(Note::warning(format!(
            "{} on a {}: not a combination this machine class is meant for.",
            material.name,
            machine.class.label()
        )));
    }

    // --- spindle ----------------------------------------------------------
    if !spindle.honours_s_word() {
        let resolved = spindle.resolve(settings.spindle_rpm);
        let mismatch = (resolved.rpm - settings.spindle_rpm).abs() > 250.0;
        if mismatch {
            notes.push(Note::warning(format!(
                "S{:.0} in the G-code does nothing: this spindle is switched by a relay and its speed is set by hand. The nearest dial position is {} at ≈{:.0} rpm — the feed check below uses S{:.0} as typed, so if the dial is really at {:.0} the chipload is not what you think.",
                settings.spindle_rpm,
                resolved.dial.clone().unwrap_or_default(),
                resolved.rpm,
                settings.spindle_rpm,
                resolved.rpm
            )));
        } else {
            notes.push(Note::caution(format!(
                "S{:.0} in the G-code does nothing — set the router dial to {} (≈{:.0} rpm) by hand.",
                settings.spindle_rpm,
                resolved.dial.clone().unwrap_or_default(),
                resolved.rpm
            )));
        }
    } else {
        if let Some(min) = spindle.min_rpm() {
            if settings.spindle_rpm < min {
                notes.push(Note::warning(format!(
                    "S{:.0} is below the spindle's {min:.0} rpm minimum.",
                    settings.spindle_rpm
                )));
            }
        }
        if let Some(max) = spindle.max_rpm() {
            if settings.spindle_rpm > max {
                notes.push(Note::warning(format!(
                    "S{:.0} is above the spindle's {max:.0} rpm maximum.",
                    settings.spindle_rpm
                )));
            }
        }
    }

    let vc = std::f64::consts::PI * d * settings.spindle_rpm / 1000.0;
    if settings.spindle_rpm > 0.0 && vc > material.surface_speed.max {
        notes.push(Note::caution(format!(
            "Ø{d:.3} mm at {:.0} rpm is {vc:.0} m/min, above the {:.0} m/min ceiling for {}: the edge will wear fast.",
            settings.spindle_rpm, material.surface_speed.max, material.name
        )));
    }

    // --- chipload ---------------------------------------------------------
    if settings.spindle_rpm > 0.0 && settings.feed_rate > 0.0 {
        let actual = settings.feed_rate / (flutes * settings.spindle_rpm);
        // `.ok()` here dropped the reason and then silently skipped the whole
        // comparison, so a check that could not be made read exactly like a
        // check that passed.
        let reference = match recommend(material, tool, op, machine, spindle) {
            Ok(r) => Some(r.chipload_mm),
            Err(e) => {
                notes.push(Note::warning(format!(
                    "your feed and speed could not be compared with a recommendation for {} here: {e}. Everything below still applies; the chipload ratio does not.",
                    material.name
                )));
                None
            }
        };

        notes.push(Note::info(format!(
            "your chipload = {:.0} mm/min ÷ ({} flutes × {:.0} rpm) = {actual:.4} mm/tooth",
            settings.feed_rate, tool.flutes, settings.spindle_rpm
        )));

        if actual < material.min_chipload_mm {
            notes.push(Note::warning(format!(
                "{actual:.4} mm/tooth is below the {:.4} mm rubbing floor for {}: the edge burnishes instead of cutting, the heat goes into the tool and the work, and the cutter dulls in minutes. Feed faster, or turn the spindle down.",
                material.min_chipload_mm, material.name
            )));
        }

        if let Some(rec) = reference {
            let ratio = actual / rec;
            if ratio < CHECK_LOW_FACTOR {
                notes.push(Note::warning(format!(
                    "chipload is {:.1}× lower than the {rec:.4} mm/tooth recommended here ({actual:.4}). Too slow a feed rubs and heats, it does not play safe.",
                    1.0 / ratio
                )));
            } else if ratio > CHECK_HIGH_FACTOR {
                notes.push(Note::warning(format!(
                    "chipload is {ratio:.1}× the {rec:.4} mm/tooth recommended here ({actual:.4}). That is cutter-breaking territory on a {}.",
                    machine.class.label()
                )));
            }
        }
    } else {
        notes.push(Note::danger(
            "feed or spindle speed is zero: the chipload cannot be checked.".to_string(),
        ));
    }

    if settings.feed_rate > machine.max_feed_mm_min {
        notes.push(Note::warning(format!(
            "feed {:.0} mm/min is above the machine's {:.0} mm/min ceiling; the controller will clamp it and the real chipload will be lower than planned.",
            settings.feed_rate, machine.max_feed_mm_min
        )));
    }

    // --- depth ------------------------------------------------------------
    let rigid_limit = material.depth_frac(op) * d;
    let recommended = rigid_limit * machine.class.depth_factor();
    if settings.stepdown > tool.flute_length_mm {
        notes.push(Note::danger(format!(
            "stepdown {:.3} mm is deeper than the {:.3} mm flute length: the shank would be cutting.",
            settings.stepdown, tool.flute_length_mm
        )));
    }
    if settings.stepdown > rigid_limit {
        notes.push(Note::danger(format!(
            "stepdown {:.3} mm is {:.1}× the {rigid_limit:.3} mm a Ø{d:.3} mm cutter takes in {} even on a rigid machine ({:.2} × D for a {}). On a {} expect deflection, chatter, and a broken cutter.",
            settings.stepdown,
            settings.stepdown / rigid_limit,
            material.name,
            material.depth_frac(op),
            op.label(),
            machine.class.label()
        )));
    } else if settings.stepdown > recommended * 1.5 {
        notes.push(Note::caution(format!(
            "stepdown {:.3} mm is {:.1}× the {recommended:.3} mm recommended for a {} here.",
            settings.stepdown,
            settings.stepdown / recommended,
            machine.class.label()
        )));
    }

    // --- stepover ---------------------------------------------------------
    if op != OpKind::Drill {
        if settings.stepover > d {
            notes.push(Note::danger(format!(
                "stepover {:.3} mm is wider than the Ø{d:.3} mm cutter: it will leave uncut ribs between passes.",
                settings.stepover
            )));
        } else if op != OpKind::Slot && settings.stepover > material.radial_frac * d * 1.5 {
            notes.push(Note::caution(format!(
                "stepover {:.3} mm is {:.0} % of diameter; {} likes {:.0} % or less for a {}.",
                settings.stepover,
                100.0 * settings.stepover / d,
                material.name,
                material.radial_frac * 100.0,
                op.label()
            )));
        }
    }

    // --- plunge -----------------------------------------------------------
    if settings.plunge_rate > settings.feed_rate && settings.feed_rate > 0.0 {
        notes.push(Note::warning(format!(
            "plunge {:.0} mm/min is faster than the cutting feed {:.0}: a flat end mill has no edge at its centre, so a straight plunge is the one move it cannot do.",
            settings.plunge_rate, settings.feed_rate
        )));
    } else if settings.feed_rate > 0.0 {
        let frac = settings.plunge_rate / settings.feed_rate;
        if frac > material.plunge_frac * 2.0 {
            notes.push(Note::caution(format!(
                "plunge is {:.0} % of the cutting feed; {:.0} % is the usual limit in {}. Ramp in at {:.1}° instead.",
                frac * 100.0,
                material.plunge_frac * 100.0,
                material.name,
                material.ramp_angle_deg
            )));
        }
    }
    if settings.plunge_rate > machine.max_plunge_mm_min {
        notes.push(Note::warning(format!(
            "plunge {:.0} mm/min is above the machine's {:.0} mm/min Z ceiling.",
            settings.plunge_rate, machine.max_plunge_mm_min
        )));
    }

    notes.extend(material.hazards.iter().map(Note::from));
    notes
}

#[cfg(test)]
mod tests;
